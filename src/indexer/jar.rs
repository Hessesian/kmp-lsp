//! Gradle cache JAR/AAR scanning, sidecar-based symbol indexing, and
//! sources-JAR auto-mounting.
//!
//! Two parallel pipelines:
//!
//! 1. **Compiled JARs** (.jar, .aar, excluding *-sources.jar/*-javadoc.jar):
//!    Sent to the `kmp-jar-indexer` sidecar process which emits `SidecarSymbol`
//!    items.  These are stored in the separate `jar_files` / `jar_definitions`
//!    DashMaps so they never mix with workspace-source symbols.
//!
//! 2. **Sources JARs** (*-sensors.jar):
//!    Unzipped in-memory; each `.kt` / `.java` entry is parsed by tree-sitter
//!    (`parse_file`) and applied through `apply_file_result` into the main
//!    `files` / `definitions` / `qualified` maps, marked `SourceSet::Library`.
//!    This replaces the external `extract-sources` CLI step.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use super::FileContributions;
use crate::cli::extract_sources::{default_gradle_home, parse_jar_meta, version_key};
use crate::sidecar::SidecarHandle;
use crate::types::{ExtensionEntry, FileData, SourceSet, SymbolEntry, Visibility};

// ── Gradle cache discovery ────────────────────────────────────────────────────

fn gradle_cache_root(gradle_home: Option<&Path>) -> PathBuf {
    gradle_home
        .map(|p| p.to_owned())
        .unwrap_or_else(default_gradle_home)
        .join("caches")
        .join("modules-2")
        .join("files-2.1")
}

/// Walk the Gradle module cache and collect all JAR/AAR paths, separated by
/// kind.  Deduplication: for each `(group, artifact)` pair keep only the
/// highest-version directory.
pub(crate) fn scan_gradle_jars_split(
    gradle_home: Option<&Path>,
) -> (
    Vec<PathBuf>, /* compiled */
    Vec<PathBuf>, /* sources */
) {
    let search_root = gradle_cache_root(gradle_home);

    if !search_root.exists() {
        log::debug!("jar: Gradle cache not found at {}", search_root.display());
        return (Vec::new(), Vec::new());
    }

    let mut all: Vec<PathBuf> = Vec::new();
    collect_all_jars(&search_root, &mut all);

    let mut compiled_best: HashMap<
        (String, String),
        (Vec<crate::cli::extract_sources::VersionPart>, PathBuf),
    > = HashMap::new();
    let mut sources_best: HashMap<
        (String, String),
        (Vec<crate::cli::extract_sources::VersionPart>, PathBuf),
    > = HashMap::new();

    for jar in all {
        let Some(meta) = parse_jar_meta(&jar) else {
            continue;
        };
        let vk = version_key(&meta.version);
        let key = (meta.group, meta.artifact);
        let is_sources = jar
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains("-sources") || n.contains("-javadoc"))
            .unwrap_or(false);

        let best = if is_sources {
            &mut sources_best
        } else {
            &mut compiled_best
        };
        match best.get(&key) {
            None => {
                best.insert(key, (vk, jar));
            }
            Some((best_vk, _)) if &vk > best_vk => {
                best.insert(key, (vk, jar));
            }
            _ => {}
        }
    }

    let compiled = compiled_best.into_values().map(|(_, path)| path).collect();
    let sources = sources_best.into_values().map(|(_, path)| path).collect();
    (compiled, sources)
}

/// Scan for compiled (non-sources) JARs only — backwards-compatible wrapper.
pub(crate) fn scan_gradle_jars(gradle_home: Option<&Path>) -> Vec<PathBuf> {
    scan_gradle_jars_split(gradle_home).0
}

/// Scan for sources JARs only.
fn scan_gradle_sources_jars(gradle_home: Option<&Path>) -> Vec<PathBuf> {
    scan_gradle_jars_split(gradle_home).1
}

fn collect_all_jars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_all_jars(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let is_jar = name.ends_with(".jar") || name.ends_with(".aar");
            let is_javadoc = name.contains("-javadoc");
            if is_jar && !is_javadoc {
                out.push(path);
            }
        }
    }
}

// ── Sources-JAR auto-mount ─────────────────────────────────────────────────────

/// Index *-sources.jar files from the Gradle cache by unpacking them
/// in-memory and parsing each `.kt` / `.java` entry with tree-sitter.
///
/// Results go into the main `files` / `definitions` / `qualified` maps
/// (via `apply_file_result`), marked `SourceSet::Library`, so they are
/// visible to go-to-definition / hover / completion without needing the
/// external `extract-sources` CLI step.
///
/// This is the slow I/O-bound path that walks the Gradle cache and reads
/// ZIPs.  For unit tests of the parse / apply phase, use
/// [`index_jar_entries`] with a pre-built `Vec<(Url, String)>` of entries
/// — no tempdir, no zip crate, no filesystem.
pub(crate) fn index_sources_jars(
    indexer: &crate::indexer::Indexer,
    gradle_home: Option<&Path>,
) -> usize {
    let sources = scan_gradle_sources_jars(gradle_home);
    if sources.is_empty() {
        log::debug!("jar: no sources JARs found in Gradle cache");
        return 0;
    }

    // Phase 1: Extract (URI, content) pairs from all JARs sequentially.
    // ZIP reading is I/O bound; parallelizing per-JAR adds complexity for
    // diminishing returns since the bottleneck is usually CPU parsing.
    let mut all_entries: Vec<(Url, String)> = Vec::new();
    let mut total_files = 0usize;
    for jar_path in &sources {
        match extract_sources_jar_entries(jar_path) {
            Ok(entries) => {
                total_files += entries.len();
                all_entries.extend(entries);
            }
            Err(err) => {
                log::warn!(
                    "jar: failed to read sources JAR {}: {err}",
                    jar_path.display()
                );
            }
        }
    }

    if all_entries.is_empty() {
        log::info!(
            "jar: zero source files found in {} sources JARs",
            sources.len()
        );
        return 0;
    }

    let total_symbols = index_jar_entries(indexer, all_entries);

    if total_symbols > 0 {
        log::info!(
            "jar: indexed {total_symbols} symbols from {total_files} source files in {} sources JARs",
            sources.len()
        );
    } else {
        log::info!("jar: zero symbols from {} sources JARs", sources.len());
    }

    total_symbols
}

/// In-memory sources-JAR indexing path.  Takes pre-extracted `(uri, content)`
/// pairs and runs the parse + apply phase.  This is the function unit tests
/// call directly with mocked entries — no Gradle cache walk, no ZIP reading.
///
/// Returns the number of symbols indexed (sum of `result.data.symbols.len()`
/// across all entries).
pub(crate) fn index_jar_entries(
    indexer: &crate::indexer::Indexer,
    entries: Vec<(Url, String)>,
) -> usize {
    if entries.is_empty() {
        return 0;
    }

    // Pre-pass: remove stale entries for all URIs we're about to insert.
    //
    // Without this, re-running `index_jar_entries` on the same set (or
    // loading the workspace cache and then re-parsing) would double-count
    // symbols in `definitions` / `packages` / `subtypes` /
    // `extension_by_receiver` since the parallel insert below uses
    // `entry().or_default().extend(...)`.
    //
    // Touches only per-file maps.  Compiled-JAR entries in `jar_files` /
    // `jar_definitions` use a different stale-removal path (`jar_uri_to_defs`).
    let planned_uris: Vec<String> = entries.iter().map(|(u, _)| u.to_string()).collect();
    for uri_str in &planned_uris {
        indexer.remove_stale_for_uri(uri_str);
    }

    // Parse, compute contributions, and insert into DashMaps — all in
    // parallel across thread-local chunks.  DashMap is thread-safe so
    // concurrent inserts from multiple threads are safe and efficient.
    let total_symbols: usize = std::thread::scope(|scope| {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let chunk_size = (entries.len() / num_threads).max(1);

        let handles: Vec<_> = entries
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local_symbols = 0usize;
                    for (uri, content) in chunk {
                        let result = crate::indexer::Indexer::parse_file(uri, content);
                        if result.error.is_some() {
                            continue;
                        }
                        indexer.library_uris.insert(result.uri.to_string());
                        let contrib = crate::indexer::apply::file_contributions(&result);
                        apply_contribution_to_index(indexer, contrib);
                        local_symbols += result.data.symbols.len();
                    }
                    local_symbols
                })
            })
            .collect();

        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });

    // Rebuild derived caches once after all threads finish.
    indexer
        .bare_names_dirty
        .store(true, std::sync::atomic::Ordering::Release);
    if let Ok(mut last) = indexer.last_completion.lock() {
        *last = None;
    }
    indexer
        .completion_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Release);

    total_symbols
}

/// Inline helper: insert a single FileContributions into the DashMaps.
/// Extracted so it can be called from parallel threads without capturing
/// &self on Indexer (which is already borrowed by DashMap).
#[inline]
fn apply_contribution_to_index(indexer: &crate::indexer::Indexer, contrib: FileContributions) {
    let (uri_str, mut file_data) = contrib.file_data;
    let (hash_key, hash_val) = contrib.content_hash;
    // Override source set: sources JAR entries are always Library.
    if file_data.source_set != SourceSet::Library {
        file_data = Arc::new(FileData {
            source_set: SourceSet::Library,
            ..(*file_data).clone()
        });
    }
    indexer.content_hashes.insert(hash_key, hash_val);
    indexer.files.insert(uri_str.clone(), file_data);

    for (name, locs) in contrib.definitions {
        let mut entry = indexer.definitions.entry(name).or_default();
        entry.extend(locs);
    }
    for (key, loc) in contrib.qualified {
        indexer.qualified.insert(key, loc);
    }
    for (pkg, uris) in contrib.packages {
        let mut entry = indexer.packages.entry(pkg).or_default();
        entry.extend(uris);
    }
    for (super_name, locs) in contrib.subtypes {
        let mut entry = indexer.subtypes.entry(super_name).or_default();
        entry.extend(locs);
    }
    for (receiver, new_entries) in contrib.extensions {
        let mut slot = indexer.extension_by_receiver.entry(receiver).or_default();
        slot.extend(new_entries);
    }
}

/// Extract `.kt` / `.java` entries from a sources-JAR.
/// Returns Vec of (synthetic_uri, content) pairs.
fn extract_sources_jar_entries(jar_path: &Path) -> Result<Vec<(Url, String)>, String> {
    let file = std::fs::File::open(jar_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip open failed: {e}"))?;

    let jar_uri_str = format!("jar:file://{}", jar_path.display());
    let mut entries = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry
            .enclosed_name()
            .map(|p| p.to_string_lossy().into_owned())
        else {
            continue;
        };

        let is_kotlin = name.ends_with(".kt");
        let is_java = name.ends_with(".java");
        if !is_kotlin && !is_java {
            continue;
        }

        let entry_uri_str = format!("{}!/{}", jar_uri_str, name);
        let Ok(entry_uri) = Url::parse(&entry_uri_str) else {
            continue;
        };

        let mut content = String::new();
        if entry.read_to_string(&mut content).is_err() {
            continue;
        }

        entries.push((entry_uri, content));
    }

    Ok(entries)
}

// ── Sidecar dispatch (compiled JARs) ──────────────────────────────────────────

/// Index the given JAR/AAR files using the sidecar (with disk cache), inserting
/// results into the indexer's symbol maps.  The sidecar handle is borrowed
/// mutably so it can be set to `None` on crash.
pub(crate) fn index_jars(
    indexer: &crate::indexer::Indexer,
    paths: &[PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) -> usize {
    if paths.is_empty() {
        return 0;
    }

    // Clear stale JAR symbols before re-indexing to prevent duplicates.
    indexer.jar_files.clear();
    indexer.jar_definitions.clear();
    indexer.jar_uri_to_defs.clear();

    let mut jar_cache = super::jar_cache::load_jar_cache();
    let mut total = 0usize;
    let mut cache_hits = 0usize;
    let mut cache_dirty = false;
    let mut missed: Vec<(PathBuf, String)> = Vec::new();

    for path in paths {
        let path_key = path.to_string_lossy().to_string();

        // Cache hit — borrow entry directly without cloning the symbols vec.
        if let Some(entry) = jar_cache.get(&path_key) {
            if super::jar_cache::cache_entry_is_fresh(entry, path) {
                let count = populate_from_symbols(indexer, path, &entry.symbols);
                total += count;
                cache_hits += 1;
                continue;
            }
        }

        // Cache miss — collect for batch sidecar call.
        missed.push((path.clone(), path_key));
    }

    // Batch-process cache misses.
    if !missed.is_empty() {
        if let Some(ref mut sidecar_guard) = sidecar {
            let sidecar_paths: Vec<&Path> = missed.iter().map(|(p, _)| p.as_path()).collect();
            match sidecar_guard.index_jars(&sidecar_paths) {
                Ok(results) => {
                    for ((path, path_key), symbols) in missed.into_iter().zip(results) {
                        let count = populate_from_symbols(indexer, &path, &symbols);
                        total += count;
                        if let Some(entry) = super::jar_cache::make_cache_entry(&path, symbols) {
                            jar_cache.insert(path_key, entry);
                            cache_dirty = true;
                        }
                    }
                }
                Err(err) => {
                    log::warn!("jar: sidecar batch error: {err} — disabling sidecar");
                    *sidecar = None;
                }
            }
        }
    }

    if cache_dirty {
        super::jar_cache::save_jar_cache(&jar_cache);
    }

    if total > 0 {
        log::info!(
            "jar: indexed {total} symbols from {} compiled JARs/AARs ({cache_hits} from cache)",
            paths.len()
        );
        indexer
            .bare_names_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        // Invalidate cached completion results.
        if let Ok(mut last) = indexer.last_completion.lock() {
            *last = None;
        }
        indexer
            .completion_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    } else {
        log::info!(
            "jar: zero symbols from {} compiled JARs (sidecar={}, cache_hits={cache_hits})",
            paths.len(),
            sidecar.is_some()
        );
    }
    total
}

/// Insert symbols for one JAR into the indexer.  Returns the symbol count.
pub(crate) fn populate_from_symbols(
    indexer: &crate::indexer::Indexer,
    path: &Path,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    if sidecar_symbols.is_empty() {
        return 0;
    }
    let fake_uri = match Url::parse(&format!("jar:file://{}", path.display())) {
        Ok(u) => u,
        Err(e) => {
            log::warn!("jar: cannot build URI for {}: {e}", path.display());
            return 0;
        }
    };
    let fake_uri_str = fake_uri.to_string();

    // Remove stale data for this JAR using reverse index.
    if let Some((_, names)) = indexer.jar_uri_to_defs.remove(&fake_uri_str) {
        for name in &names {
            if let Some(mut entry) = indexer.jar_definitions.get_mut(name) {
                entry.retain(|l| l.uri != fake_uri);
                if entry.is_empty() {
                    drop(entry);
                    indexer.jar_definitions.remove(name);
                }
            }
        }
    }
    indexer.jar_files.remove(&fake_uri_str);

    build_jar_file_data(indexer, &fake_uri, &fake_uri_str, sidecar_symbols)
}

/// Build `FileData` + definition entries for one JAR and insert them into the index.
fn build_jar_file_data(
    indexer: &crate::indexer::Indexer,
    fake_uri: &Url,
    fake_uri_str: &str,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    let mut symbols: Vec<SymbolEntry> = Vec::with_capacity(sidecar_symbols.len());
    let mut jar_names: Vec<String> = Vec::with_capacity(sidecar_symbols.len());

    for (line_idx, sym) in sidecar_symbols.iter().enumerate() {
        let synthetic_range = tower_lsp::lsp_types::Range {
            start: tower_lsp::lsp_types::Position {
                line: line_idx as u32,
                character: 0,
            },
            end: tower_lsp::lsp_types::Position {
                line: line_idx as u32,
                character: sym.name.len() as u32,
            },
        };
        let extension_receiver = sym
            .extension_receiver_type
            .split('<')
            .next()
            .unwrap_or("")
            .to_owned();
        symbols.push(SymbolEntry {
            name: sym.name.clone(),
            kind: kind_str_to_lsp(&sym.kind),
            visibility: Visibility::Public,
            range: synthetic_range,
            selection_range: synthetic_range,
            detail: sym.detail.clone(),
            container: if sym.container.is_empty() {
                None
            } else {
                Some(sym.container.clone())
            },
            params: String::new(),
            param_counts: (0, 0),
            type_params: sym.type_params.clone(),
            extension_receiver,
            extension_receiver_type: sym.extension_receiver_type.clone(),
            doc: sym.doc.clone(),
            trailing_lambda: sym.trailing_lambda,
        });
        indexer
            .jar_definitions
            .entry(sym.name.clone())
            .or_default()
            .push(tower_lsp::lsp_types::Location {
                uri: fake_uri.clone(),
                range: synthetic_range,
            });
        jar_names.push(sym.name.clone());
    }

    // Populate reverse index so removal can be O(symbols_in_jar).
    indexer
        .jar_uri_to_defs
        .insert(fake_uri_str.to_owned(), jar_names);

    let lines: Vec<String> = sidecar_symbols.iter().map(|s| s.detail.clone()).collect();

    let count = symbols.len();

    // Infer package from a class-like symbol's detail (e.g. "class androidx.lifecycle.ViewModel").
    //
    // Only class / interface / object / typealias have reliable package info: their detail
    // is the FQN "kind pkg.Name". Function and property details use dot syntax internally
    // (e.g. "fun CoroutineScope.launch(...)", "val Foo.bar: Type") where the last dot is
    // a member-access separator, not a package separator — so we must not look at them.
    //
    // We also validate the FQN by requiring the segment after the last dot to start with
    // an uppercase letter (type-name convention).
    let package: Option<String> = symbols.iter().find_map(|sym| {
        if !matches!(
            sym.kind,
            tower_lsp::lsp_types::SymbolKind::CLASS
                | tower_lsp::lsp_types::SymbolKind::INTERFACE
                | tower_lsp::lsp_types::SymbolKind::OBJECT
        ) {
            return None;
        }
        let detail = &sym.detail;
        let after_kind = detail.find(' ').map(|pos| pos + 1).unwrap_or(0);
        let fqn = &detail[after_kind..];
        // Extract only the leading dotted-identifier part (stop at '(', ':', etc.)
        let fqn = fqn.split(&['(', ':', '<', ' ']).next().unwrap_or(fqn);
        fqn.rfind('.').and_then(|dot| {
            let candidate = &fqn[..dot];
            let after_dot = &fqn[dot + 1..];
            // The segment after the last dot must start with uppercase (a type name)
            let is_type_name = after_dot
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase());
            if is_type_name && !candidate.is_empty() {
                Some(candidate.to_owned())
            } else {
                None
            }
        })
    });

    // Add to qualified index so FQN resolution works for JAR symbols.
    // For nested classes (container is set), use container.name as the FQN.
    if let Some(ref pkg) = package {
        for sym in &symbols {
            let fqn = match &sym.container {
                Some(container) if !container.is_empty() => {
                    format!("{}.{}.{}", pkg, container, sym.name)
                }
                _ => format!("{}.{}", pkg, sym.name),
            };
            indexer.qualified.insert(
                fqn,
                tower_lsp::lsp_types::Location {
                    uri: fake_uri.clone(),
                    range: sym.range,
                },
            );
        }
    }

    // Populate extension_by_receiver.
    for sym in &symbols {
        if sym.extension_receiver.is_empty() {
            continue;
        }
        indexer
            .extension_by_receiver
            .entry(sym.extension_receiver.clone())
            .or_default()
            .push(ExtensionEntry {
                file_uri: fake_uri_str.to_owned(),
                name: sym.name.clone(),
                kind: sym.kind,
                detail: sym.detail.clone(),
                visibility: Visibility::Public,
                package: package.clone(),
                trailing_lambda: sym.trailing_lambda,
            });
    }

    indexer.jar_files.insert(
        fake_uri_str.to_owned(),
        Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            lines: Arc::new(lines),
            package,
            ..Default::default()
        }),
    );
    indexer.library_uris.insert(fake_uri_str.to_owned());
    count
}

fn kind_str_to_lsp(kind: &str) -> tower_lsp::lsp_types::SymbolKind {
    match kind {
        "class" => tower_lsp::lsp_types::SymbolKind::CLASS,
        "interface" => tower_lsp::lsp_types::SymbolKind::INTERFACE,
        "object" => tower_lsp::lsp_types::SymbolKind::OBJECT,
        "fun" => tower_lsp::lsp_types::SymbolKind::FUNCTION,
        "val" => tower_lsp::lsp_types::SymbolKind::PROPERTY,
        "var" => tower_lsp::lsp_types::SymbolKind::VARIABLE,
        "typealias" => tower_lsp::lsp_types::SymbolKind::CLASS,
        _ => tower_lsp::lsp_types::SymbolKind::NULL,
    }
}
