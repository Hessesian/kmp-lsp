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
//! 2. **Sources JARs** (*-sources.jar):
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
use crate::types::{ExtensionEntry, FileData, FileIndexResult, SourceSet, SymbolEntry, Visibility};

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
/// Results go into the main `files` / `definitions` / `qualified` maps,
/// marked `SourceSet::Library`, so they are visible to go-to-definition /
/// hover / completion without needing the external `extract-sources` CLI step.
///
/// Parse results are cached to disk per JAR keyed by `(mtime, size)`.
/// Unchanged JARs skip extraction AND parsing on subsequent startups — the
/// dominant startup cost (see `docs/startup-speed-plan.md`).
///
/// `cache_dir` overrides the parse-cache location (pass `None` in production
/// for `~/.cache/kmp-lsp/`; tests pass an isolated tmpdir).
pub(crate) fn index_sources_jars(
    indexer: &crate::indexer::Indexer,
    gradle_home: Option<&Path>,
    cache_dir: Option<&Path>,
) -> usize {
    let sources = scan_gradle_sources_jars(gradle_home);
    if sources.is_empty() {
        log::debug!("jar: no sources JARs found in Gradle cache");
        return 0;
    }

    let mut cache = super::sources_jar_cache::load_sources_jar_cache(cache_dir);
    let pruned = super::sources_jar_cache::prune_deleted_jars(&mut cache);

    let (cache_hits, mut contributions, missed) = partition_sources_jars(&cache, &sources);
    let cache_dirty = pruned || !missed.is_empty();

    contributions.extend(parse_missed_sources_jars(&mut cache, missed));
    let total_files = contributions.len();

    let total_symbols = apply_sources_contributions(indexer, contributions);

    if cache_dirty {
        super::sources_jar_cache::save_sources_jar_cache(cache_dir, &cache);
    }

    if total_symbols > 0 {
        log::info!(
            "jar: indexed {total_symbols} symbols from {total_files} source files in {} sources JARs ({cache_hits} JARs from parse cache)",
            sources.len()
        );
    } else {
        log::info!("jar: zero symbols from {} sources JARs", sources.len());
    }

    total_symbols
}

/// Split sources JARs into cache hits (returning ready-to-apply contributions
/// built from the cached `Arc<FileData>` — zero deep clones) and misses
/// (JAR path + pre-captured fingerprint, to be extracted + parsed).
/// Returns `(hit_count, cached_contributions, missed)`.
fn partition_sources_jars(
    cache: &std::collections::HashMap<String, super::sources_jar_cache::SourcesJarEntry>,
    sources: &[PathBuf],
) -> (
    usize,
    Vec<FileContributions>,
    Vec<(PathBuf, super::sources_jar_cache::JarFingerprint)>,
) {
    let mut cache_hits = 0usize;
    let mut cached_contributions: Vec<FileContributions> = Vec::new();
    let mut missed: Vec<(PathBuf, super::sources_jar_cache::JarFingerprint)> = Vec::new();

    for jar_path in sources {
        let Some(fingerprint) = super::sources_jar_cache::jar_fingerprint(jar_path) else {
            log::warn!("jar: cannot stat sources JAR {}", jar_path.display());
            continue;
        };
        let cache_key = jar_path.to_string_lossy().to_string();
        if let Some(entry) = cache.get(&cache_key) {
            if super::sources_jar_cache::entry_is_fresh(entry, &fingerprint) {
                for file_entry in &entry.files {
                    let Ok(uri) = Url::parse(&file_entry.uri) else {
                        continue;
                    };
                    let supertypes =
                        crate::indexer::apply::derive_supertypes(&uri, &file_entry.file_data);
                    cached_contributions.push(crate::indexer::apply::contributions_from_data(
                        &uri,
                        Arc::clone(&file_entry.file_data),
                        file_entry.content_hash,
                        &supertypes,
                    ));
                }
                cache_hits += 1;
                continue;
            }
        }
        missed.push((jar_path.clone(), fingerprint));
    }

    (cache_hits, cached_contributions, missed)
}

/// Extract + parse each missed JAR and insert its refreshed cache entry.
/// Per-JAR processing keeps the result→JAR association structural (no
/// URI-string reverse-mapping, which breaks under URL percent-encoding).
/// Parsing is parallel across each JAR's files.
///
/// A JAR is NOT cached when extraction fails (transient unreadability must not
/// hide symbols until the next mtime change) or when a parse thread panicked
/// (partial entry behind an immutable fingerprint would hide missing files
/// forever).  Empty entries for JARs with zero parseable files ARE cached.
fn parse_missed_sources_jars(
    cache: &mut std::collections::HashMap<String, super::sources_jar_cache::SourcesJarEntry>,
    missed: Vec<(PathBuf, super::sources_jar_cache::JarFingerprint)>,
) -> Vec<FileContributions> {
    let mut all_contributions: Vec<FileContributions> = Vec::new();

    for (jar_path, fingerprint) in missed {
        let entries = match extract_sources_jar_entries(&jar_path) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!(
                    "jar: failed to read sources JAR {}: {error}",
                    jar_path.display()
                );
                continue;
            }
        };

        let parsed = parse_jar_entries(entries);

        if parsed.complete {
            let files: Vec<super::sources_jar_cache::SourcesFileEntry> = parsed
                .results
                .iter()
                .map(|result| {
                    let mut data = result.data.clone();
                    data.source_set = SourceSet::Library;
                    super::sources_jar_cache::SourcesFileEntry {
                        uri: result.uri.to_string(),
                        content_hash: result.content_hash,
                        file_data: Arc::new(data),
                    }
                })
                .collect();
            cache.insert(
                jar_path.to_string_lossy().to_string(),
                super::sources_jar_cache::SourcesJarEntry {
                    mtime_secs: fingerprint.mtime_secs,
                    mtime_nanos: fingerprint.mtime_nanos,
                    file_size: fingerprint.file_size,
                    files,
                },
            );
        } else {
            log::warn!(
                "jar: parse incomplete for {} — not caching this JAR",
                jar_path.display()
            );
        }

        all_contributions.extend(
            parsed
                .results
                .iter()
                .map(crate::indexer::apply::file_contributions),
        );
    }

    all_contributions
}

/// Parse results from a batch of sources-JAR entries.
pub(crate) struct ParsedJarEntries {
    pub(crate) results: Vec<FileIndexResult>,
    /// `false` if any worker thread panicked — incomplete results must not be
    /// cached (immutable JAR fingerprints would hide gaps forever).
    pub(crate) complete: bool,
}

/// In-memory sources-JAR indexing path.  Takes pre-extracted `(uri, content)`
/// pairs and runs the parse + apply phase.  This is the function unit tests
/// call directly with mocked entries — no Gradle cache walk, no ZIP reading.
///
/// Returns the number of symbols indexed (sum of `result.data.symbols.len()`
/// across all entries).
#[cfg(test)]
pub(crate) fn index_jar_entries(
    indexer: &crate::indexer::Indexer,
    entries: Vec<(Url, String)>,
) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let parsed = parse_jar_entries(entries);
    let contributions = parsed
        .results
        .iter()
        .map(crate::indexer::apply::file_contributions)
        .collect();
    apply_sources_contributions(indexer, contributions)
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

/// Pure: parse a batch of (URI, content) pairs in parallel.
///
/// Returns a [`ParsedJarEntries`] whose `complete` flag is `false` when any
/// worker thread panicked — callers must not persist incomplete results to the
/// disk cache.
pub(crate) fn parse_jar_entries(entries: Vec<(Url, String)>) -> ParsedJarEntries {
    if entries.is_empty() {
        return ParsedJarEntries {
            results: Vec::new(),
            complete: true,
        };
    }
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunk_size = (entries.len() / num_threads).max(1);
    let mut complete = true;
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = entries
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        chunk
                            .iter()
                            .filter_map(|(uri, content)| {
                                let result = crate::indexer::Indexer::parse_file(uri, content);
                                if result.error.is_some() {
                                    None
                                } else {
                                    Some(result)
                                }
                            })
                            .collect::<Vec<_>>()
                    }))
                })
            })
            .collect();
        let mut all = Vec::with_capacity(entries.len());
        for handle in handles {
            match handle
                .join()
                .expect("scope thread cannot panic: caught by catch_unwind")
            {
                Ok(chunk) => all.extend(chunk),
                Err(_) => {
                    complete = false;
                    log::warn!("jar: parse worker thread panicked — results are incomplete");
                }
            }
        }
        all
    });
    ParsedJarEntries { results, complete }
}

/// Apply a batch of sources-JAR contributions to the indexer.
///
/// Removes stale per-file index entries for every URI in `contributions`,
/// inserts new contributions, marks the bare-name cache dirty, and returns
/// the total symbol count.
pub(crate) fn apply_sources_contributions(
    indexer: &crate::indexer::Indexer,
    contributions: Vec<FileContributions>,
) -> usize {
    if contributions.is_empty() {
        return 0;
    }
    for contrib in &contributions {
        indexer.remove_stale_for_uri(&contrib.file_data.0);
    }
    let mut total = 0usize;
    for contrib in contributions {
        indexer.library_uris.insert(contrib.file_data.0.clone());
        total += contrib.file_data.1.symbols.len();
        apply_contribution_to_index(indexer, contrib);
    }
    indexer
        .bare_names_dirty
        .store(true, std::sync::atomic::Ordering::Release);
    if let Ok(mut last) = indexer.last_completion.lock() {
        *last = None;
    }
    indexer
        .completion_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    total
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
    indexer.jar_symbol_packages.clear();

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

/// Parse the value-parameter text and `(required, total)` counts from a sidecar
/// signature `detail` (e.g. `fun WindowInsets(left: Int, top: Int = 0): WindowInsets`).
///
/// Required = params without a `=` default. Returns `("", (0, 0))` when there is
/// no value-parameter list. Matches the first balanced `(…)` after the name so a
/// function-type parameter like `block: () -> Unit` doesn't terminate early.
pub(crate) fn params_from_detail(detail: &str) -> (String, (u8, u8)) {
    let Some(open) = detail.find('(') else {
        return (String::new(), (0, 0));
    };
    let mut depth = 0i32;
    let mut close = None;
    for (offset, ch) in detail[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return (String::new(), (0, 0));
    };
    let inner = detail[open + 1..close].trim();
    if inner.is_empty() {
        return (String::new(), (0, 0));
    }
    let parts = crate::indexer::split_params_at_depth_zero(inner);
    let total = parts.len().min(u8::MAX as usize) as u8;
    let required = parts
        .iter()
        .filter(|p| !p.contains('='))
        .count()
        .min(u8::MAX as usize) as u8;
    (inner.to_owned(), (required, total))
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
    // (class_synthetic_line, supertype_simple_name, type_args) — lets the hierarchy
    // walker traverse inheritance through library types.
    let mut supers: Vec<(u32, String, Vec<String>)> = Vec::new();

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
        // The sidecar doesn't emit parameter counts, but its `detail` is the full
        // signature — parse counts from it so JAR functions get real arities.
        // Without this every JAR function looks 0-arg, producing call-arg false
        // positives (e.g. `WindowInsets(0,0,0,0)`) and breaking overload detection.
        let (params_text, param_counts) = params_from_detail(&sym.detail);
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
            params: params_text,
            param_counts,
            type_params: sym.type_params.clone(),
            extension_receiver,
            extension_receiver_type: sym.extension_receiver_type.clone(),
            doc: sym.doc.clone(),
            trailing_lambda: sym.trailing_lambda,
            deprecated: sym.deprecated,
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
        for super_name in &sym.supers {
            supers.push((line_idx as u32, super_name.clone(), Vec::new()));
        }
    }

    // Populate reverse index so removal can be O(symbols_in_jar).
    indexer
        .jar_uri_to_defs
        .insert(fake_uri_str.to_owned(), jar_names);

    // Per-symbol package side table, index-aligned with `symbols` (and the
    // synthetic line number == symbol index). Used by import resolution to
    // filter a JAR symbol by its real package.
    indexer.jar_symbol_packages.insert(
        fake_uri_str.to_owned(),
        sidecar_symbols.iter().map(|s| s.pkg.clone()).collect(),
    );

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

    // Add to qualified index so FQN resolution works for JAR symbols, using the
    // sidecar's *real* per-symbol package. Top-level declarations (a top-level
    // fun/val, or a class/interface/object itself) use `pkg.name`; class members
    // use `pkg.Container.name`. This is what makes an `import a.b.c.remember`
    // resolve to the public top-level `remember` rather than an unrelated
    // `SomeClass.remember` in another jar — the previous code keyed top-level
    // functions under their JVM facade (`pkg.ComposablesKt.remember`), so the
    // exact-FQN lookup missed and resolution fell back to an unfiltered scan.
    for (i, sym) in sidecar_symbols.iter().enumerate() {
        // Prefer the sidecar's real per-symbol package; fall back to the per-jar
        // inferred package for older sidecars that don't emit `pkg` (no regression).
        let effective_pkg = if !sym.pkg.is_empty() {
            sym.pkg.as_str()
        } else if let Some(ref p) = package {
            p.as_str()
        } else {
            continue;
        };
        let fqn = if sym.top_level || sym.container.is_empty() {
            format!("{}.{}", effective_pkg, sym.name)
        } else {
            format!("{}.{}.{}", effective_pkg, sym.container, sym.name)
        };
        indexer.qualified.insert(
            fqn,
            tower_lsp::lsp_types::Location {
                uri: fake_uri.clone(),
                range: symbols[i].range,
            },
        );
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
                deprecated: sym.deprecated,
            });
    }

    indexer.jar_files.insert(
        fake_uri_str.to_owned(),
        Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            lines: Arc::new(lines),
            package,
            supers,
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
