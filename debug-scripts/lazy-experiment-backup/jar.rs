//! Gradle cache JAR/AAR scanning and sidecar-based symbol indexing.
//!
//! Scans `~/.gradle/caches/modules-2/files-2.1/` for non-sources JARs and AARs,
//! deduplicates by `(group, artifact, latest-version)`, and sends each file to
//! the `kmp-jar-indexer` sidecar process to produce `SymbolEntry` items.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::SymbolKind;

use crate::cli::extract_sources::{default_gradle_home, parse_jar_meta, version_key, GradleMeta};
use crate::sidecar::SidecarHandle;
use crate::types::{ExtensionEntry, FileData, SourceSet, SymbolEntry, Visibility};

// ── Gradle cache discovery ────────────────────────────────────────────────────

/// Scan the Gradle module cache and return deduplicated JAR/AAR paths.
///
/// Deduplication: for each `(group, artifact)` pair keep only the file
/// belonging to the highest-version directory — same logic as `extract-sources`.
/// `-sources.jar` and `-javadoc.jar` files are excluded (source already handled
/// by the extract-sources path; javadoc not useful for symbol indexing).
pub(crate) fn scan_gradle_jars(gradle_home: Option<&Path>) -> Vec<PathBuf> {
    let search_root = gradle_home
        .map(|p| p.to_owned())
        .unwrap_or_else(default_gradle_home)
        .join("caches")
        .join("modules-2")
        .join("files-2.1");

    if !search_root.exists() {
        log::debug!("jar: Gradle cache not found at {}", search_root.display());
        return Vec::new();
    }

    // Walk: collect all JAR/AAR paths that are not sources/javadoc.
    let mut candidates: Vec<PathBuf> = Vec::new();
    collect_jars(&search_root, &mut candidates);

    // Deduplicate: (group, artifact) → (version_key, path)
    let mut best: HashMap<
        (String, String),
        (Vec<crate::cli::extract_sources::VersionPart>, PathBuf),
    > = HashMap::new();

    for jar in candidates {
        let Some(GradleMeta {
            group,
            artifact,
            version,
        }) = parse_jar_meta(&jar)
        else {
            continue;
        };
        let vk = version_key(&version);
        let key = (group, artifact);
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

    best.into_values().map(|(_, path)| path).collect()
}

fn collect_jars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jars(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let is_jar = name.ends_with(".jar") || name.ends_with(".aar");
            let is_sources = name.contains("-sources") || name.contains("-javadoc");
            if is_jar && !is_sources {
                out.push(path);
            }
        }
    }
}

// ── Sidecar dispatch ──────────────────────────────────────────────────────────

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
        // The `continue` ends the iteration before any mutable borrow of jar_cache.
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
            "jar: indexed {total} symbols from {} JARs/AARs ({cache_hits} from cache)",
            paths.len()
        );
        indexer
            .bare_names_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        // Invalidate cached completion results: JAR extensions are now available.
        // Clear the entry and bump the epoch so any in-flight completion that
        // started before JAR indexing cannot overwrite with stale empty results.
        if let Ok(mut last) = indexer.last_completion.lock() {
            *last = None;
        }
        indexer
            .completion_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    total
}

/// Insert symbols for one JAR into the indexer.  Returns the symbol count.
fn populate_from_symbols(
    indexer: &crate::indexer::Indexer,
    path: &Path,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    if sidecar_symbols.is_empty() {
        return 0;
    }
    let fake_uri = match tower_lsp::lsp_types::Url::parse(&format!("jar:file://{}", path.display()))
    {
        Ok(u) => u,
        Err(e) => {
            log::warn!("jar: cannot build URI for {}: {e}", path.display());
            return 0;
        }
    };
    let fake_uri_str = fake_uri.to_string();

    // Remove stale data for this JAR using reverse index — O(symbols_in_this_jar)
    // instead of O(total_jar_symbols).
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
    fake_uri: &tower_lsp::lsp_types::Url,
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
        // Derive the bare receiver name (without generics) from the full receiver type.
        // e.g. "ImmutableList<T>" → "ImmutableList", "String" → "String".
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

    // Populate extension_by_receiver so that e.g. CoroutineScope.launch appears
    // in dot-completion. LibraryBatch (cache path) does the same for cached JARs;
    // this covers the fresh-parse path (no cache yet, or cache invalidated).
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
                package: None,
                trailing_lambda: sym.trailing_lambda,
            });
    }

    indexer.jar_files.insert(
        fake_uri_str.to_owned(),
        Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            lines: Arc::new(lines),
            ..Default::default()
        }),
    );
    indexer.library_uris.insert(fake_uri_str.to_owned());
    count
}

fn kind_str_to_lsp(kind: &str) -> SymbolKind {
    match kind {
        "class" => SymbolKind::CLASS,
        "interface" => SymbolKind::INTERFACE,
        "object" => SymbolKind::OBJECT,
        "fun" => SymbolKind::FUNCTION,
        "val" => SymbolKind::PROPERTY,
        "var" => SymbolKind::VARIABLE,
        "typealias" => SymbolKind::CLASS,
        _ => SymbolKind::NULL,
    }
}

// ── Sources-JAR auto-mount (lazy-loading) ─────────────────────────────────────

/// Scan for sources JARs only.
fn scan_gradle_sources_jars(gradle_home: Option<&Path>) -> Vec<PathBuf> {
    scan_gradle_jars_split(gradle_home).1
}

/// Index *-sources.jar files from the Gradle cache.
///
/// Unlike compiled JARs (which are indexed eagerly via the sidecar), sources JARs
/// are indexed lazily: symbols are parsed and saved to a cache, but only loaded
/// into the main index on demand (when the user hovers over or navigates to a
/// symbol that's imported from a sources JAR).
pub(crate) fn index_sources_jars(
    indexer: &crate::indexer::Indexer,
    gradle_home: Option<&Path>,
    cache: &mut super::sources_cache::SourcesCache,
) -> usize {
    let sources = scan_gradle_sources_jars(gradle_home);
    if sources.is_empty() {
        log::debug!("jar: no sources JARs found in Gradle cache");
        return 0;
    }

    let mut total_symbols = 0usize;

    for jar_path in &sources {
        // Skip if already cached and fresh.
        if cache.is_fresh(jar_path) {
            log::debug!("jar: sources JAR {} already cached, skipping", jar_path.display());
            continue;
        }

        // Extract and parse entries.
        let entries = match extract_sources_jar_entries(jar_path) {
            Ok(e) => e,
            Err(err) => {
                log::warn!("jar: failed to read sources JAR {}: {err}", jar_path.display());
                continue;
            }
        };

        if entries.is_empty() {
            continue;
        }

        // Parse all entries and collect symbols.
        let mut jar_symbols: Vec<SymbolEntry> = Vec::new();
        let mut package = None;

        for (uri, content) in entries {
            let mut result = crate::indexer::Indexer::parse_file(&uri, &content);
            if result.error.is_some() {
                continue;
            }
            // Only keep public and internal symbols.
            result.data.symbols.retain(|s| {
                matches!(s.visibility, Visibility::Public | Visibility::Internal)
            });
            if package.is_none() {
                package = result.data.package.clone();
            }
            jar_symbols.extend(result.data.symbols);
        }

        if !jar_symbols.is_empty() {
            total_symbols += jar_symbols.len();
            // Save to cache (but don't load into main index yet).
            cache.save_jar(jar_path, package, jar_symbols);
        }
    }

    if total_symbols > 0 {
        log::info!(
            "jar: indexed {total_symbols} symbols from {} sources JARs (lazy-loaded on demand)",
            sources.len()
        );
    }

    total_symbols
}

/// Extract `.kt` / `.java` entries from a sources-JAR.
fn extract_sources_jar_entries(jar_path: &Path) -> Result<Vec<(tower_lsp::lsp_types::Url, String)>, String> {
    let file = std::fs::File::open(jar_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip open failed: {e}"))?;

    let jar_uri_str = format!("jar:file://{}", jar_path.display());
    let mut entries = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name().map(|p| p.to_string_lossy().into_owned()) else {
            continue;
        };
        if !name.ends_with(".kt") && !name.ends_with(".java") {
            continue;
        }
        let entry_uri_str = format!("{}!/{}", jar_uri_str, name);
        let Ok(entry_uri) = tower_lsp::lsp_types::Url::parse(&entry_uri_str) else {
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

/// Split JAR paths into compiled and sources.
pub(crate) fn scan_gradle_jars_split(
    gradle_home: Option<&Path>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let search_root = gradle_home
        .map(|p| p.to_owned())
        .unwrap_or_else(default_gradle_home)
        .join("caches")
        .join("modules-2")
        .join("files-2.1");

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
