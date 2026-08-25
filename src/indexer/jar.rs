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
use crate::types::{
    pack_cold_fields, ExtensionEntry, FileData, FileIndexResult, SourceSet, SymbolEntry, Visibility,
};

// ── Gradle cache discovery ────────────────────────────────────────────────────

fn gradle_cache_root(gradle_home: Option<&Path>) -> PathBuf {
    gradle_home
        .map(|p| p.to_owned())
        .unwrap_or_else(default_gradle_home)
        .join("caches")
        .join("modules-2")
        .join("files-2.1")
}

/// Maven `groupId`s that host build-tooling, compiler, or IDE-platform
/// artifacts exclusively. Gradle's shared module cache holds every artifact
/// resolved anywhere in the build graph (buildscript classpath, plugin
/// classpath, lint/KSP tooling classpath), not just application runtime
/// dependencies, so these end up walked alongside real ones even though
/// application Kotlin/Java source never legitimately imports from them.
/// Verified against a real multi-module Android corpus: zero imports from
/// any package these groups ship.
const KNOWN_BUILD_TOOLING_GROUP_IDS: &[&str] = &[
    // Old `org.jetbrains.annotations`-shaded jar pulled in transitively by
    // IntelliJ-platform tooling; the real `org.jetbrains:annotations`
    // dependency covers any legitimate `@NotNull`/`@Nullable` usage.
    "com.intellij",
    // Android Gradle Plugin's embedded IntelliJ platform and Kotlin compiler,
    // used internally by AGP's lint/desugaring passes. `intellij-core` here
    // is a confirmed decoy source for both `Activity` and `CoroutineScope`.
    "com.android.tools.external.com-intellij",
    "com.android.tools.external.org-jetbrains",
    // AGP/Lint's own layout-rendering, static-analysis, device-bridge,
    // test-platform, telemetry, and Jetifier implementation packages.
    // NOTE: `com.android.tools.lint` is deliberately NOT listed here — see
    // `KNOWN_BUILD_TOOLING_ARTIFACTS` below, which excludes only that
    // group's genuinely tooling-only artifacts. `lint-api` and `lint-tests`
    // are real compile-time (respectively test-compile-time) dependencies
    // for any project that writes custom Android Lint rules.
    "com.android.tools.layoutlib",
    "com.android.tools.ddms",
    "com.android.tools.utp",
    "com.android.tools.analytics-library",
    "com.android.tools.build.jetifier",
    // Deprecated pre-AndroidX Data Binding compiler internals; app runtime
    // uses `androidx.databinding` instead.
    "com.android.databinding",
    // IntelliJ platform's own collections library (`gnu.trove`).
    "org.jetbrains.intellij.deps",
];

/// Specific `groupId`/`artifactId` pairs to exclude from groups that also
/// host genuine runtime or build-logic dependencies, so the whole group
/// cannot be dropped. `kotlin-compiler-embeddable` and
/// `symbol-processing-aa-embeddable` are confirmed decoy sources for
/// `Activity` (both ship a shaded `com.intellij.diagnostic.Activity`); the
/// rest are confirmed-unused siblings from the same compiler/AGP-internals
/// families.
const KNOWN_BUILD_TOOLING_ARTIFACTS: &[(&str, &str)] = &[
    ("org.jetbrains.kotlin", "kotlin-compiler-embeddable"),
    ("org.jetbrains.kotlin", "kotlin-daemon-embeddable"),
    (
        "org.jetbrains.kotlin",
        "kotlin-scripting-compiler-embeddable",
    ),
    (
        "org.jetbrains.kotlin",
        "kotlin-scripting-compiler-impl-embeddable",
    ),
    ("com.google.devtools.ksp", "symbol-processing-aa-embeddable"),
    ("com.android.tools.build", "bundletool"),
    ("com.android.tools.build", "aapt2-proto"),
    ("com.android.tools.build", "aaptcompiler"),
    ("com.android.tools.build", "apksig"),
    ("com.android.tools.build", "apkzlib"),
    ("com.android.tools.build", "builder"),
    ("com.android.tools.build", "builder-model"),
    ("com.android.tools.build", "builder-test-api"),
    ("com.android.tools.build", "manifest-merger"),
    // `com.android.tools.lint` group: only the genuinely tooling-only
    // artifacts, verified against a real Gradle cache. `lint` is the
    // standalone CLI (bundles UAST + the Gradle-plugin integration);
    // `lint-checks` is AOSP's own bundled built-in check implementations;
    // `lint-model`/`lint-typedef-remover` are internal AGP/Lint data models.
    // `lint-api` (the custom-Detector API) and `lint-tests` (the
    // `testImplementation` harness for custom-check unit tests) are left
    // out — both are real, documented compile-time dependencies.
    ("com.android.tools.lint", "lint"),
    ("com.android.tools.lint", "lint-checks"),
    ("com.android.tools.lint", "lint-model"),
    ("com.android.tools.lint", "lint-typedef-remover"),
];

/// True when a Gradle-cache JAR belongs to a known build-tooling/compiler/
/// IDE-platform artifact rather than an application dependency — see
/// [`KNOWN_BUILD_TOOLING_GROUP_IDS`] and [`KNOWN_BUILD_TOOLING_ARTIFACTS`].
fn is_known_build_tooling_jar(meta: &crate::cli::extract_sources::GradleMeta) -> bool {
    KNOWN_BUILD_TOOLING_GROUP_IDS.contains(&meta.group.as_str())
        || KNOWN_BUILD_TOOLING_ARTIFACTS
            .iter()
            .any(|(group, artifact)| *group == meta.group && *artifact == meta.artifact)
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
        if is_known_build_tooling_jar(&meta) {
            continue;
        }
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

/// True when the index contains at least one WORKSPACE Kotlin/Java source
/// file — the gate for the global `~/.gradle/caches` JAR pipeline. A
/// Swift-only workspace cannot reference JVM libraries, and unconditionally
/// running the pipeline there cost a 1.28M-name Tier-1 manifest over 755
/// JARs plus a 1.66M-symbol sources-JAR pass (observed live on an iOS repo).
///
/// Deliberately asks the INDEX instead of probing build files: a
/// build-marker heuristic wrongly excluded non-Gradle JVM builds
/// (Maven/Bazel/manual) and Gradle repos opened at a deep subdirectory
/// (markers only above the root), and duplicated the root-marker detection
/// that already lives in `document_handler`. Library source-set files
/// (sourcePaths, extracted jar sources) don't count — they are not the
/// workspace's own code.
///
/// The index answers reliably only once the workspace scan has had a chance
/// to populate it — consult this through [`wait_for_jvm_sources_gate`] on
/// the jar-scan path, which is spawned concurrently with (and typically
/// ahead of) the workspace scan.
pub(crate) fn workspace_has_jvm_sources(indexer: &crate::indexer::Indexer) -> bool {
    indexer.files.iter().any(|entry| {
        entry.value().source_set != SourceSet::Library
            && (entry.key().ends_with(".kt")
                || entry.key().ends_with(".kts")
                || entry.key().ends_with(".java"))
    })
}

/// Scan-race-safe wrapper around [`workspace_has_jvm_sources`], for the jar
/// pipeline task. That task is spawned right after the workspace scan is
/// ENQUEUED — evaluating the gate immediately read an EMPTY index and
/// skipped the whole Gradle pipeline on a Kotlin repo (observed live:
/// "jar: no Kotlin/Java sources in the workspace" on a Compose project →
/// Tier-1 manifests never populated → jar auto-import/doc/extension data
/// silently missing for the rest of the session).
///
/// Blocks (poll + sleep; callers run on a `spawn_blocking` thread) until one
/// of:
/// - a workspace JVM source shows up in the index → `Some(true)`. On a JVM
///   repo the scan indexes one within its first files, so the pipeline
///   still starts near-immediately and keeps its old concurrency with the
///   workspace scan;
/// - `is_scanning` reports the scan queue drained → final verdict from the
///   now-authoritative index (re-checked AFTER observing the drain, so a
///   source indexed between the two probes isn't missed);
/// - `generation_ok` fails (root changed mid-wait; this task is superseded)
///   → `None`, caller abandons like the other stale-generation checkpoints.
pub(crate) fn wait_for_jvm_sources_gate(
    indexer: &crate::indexer::Indexer,
    is_scanning: impl Fn() -> bool,
    generation_ok: impl Fn() -> bool,
    poll: std::time::Duration,
) -> Option<bool> {
    loop {
        if !generation_ok() {
            return None;
        }
        if workspace_has_jvm_sources(indexer) {
            return Some(true);
        }
        if !is_scanning() {
            return Some(workspace_has_jvm_sources(indexer));
        }
        std::thread::sleep(poll);
    }
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
    indexer.register_file_uri(&uri_str);
    indexer.files.insert(uri_str.clone(), file_data);

    for (name, locs) in contrib.definitions {
        let interned: Vec<crate::types::SymbolLoc> = locs
            .iter()
            .map(|loc| indexer.intern_location(loc))
            .collect();
        let mut entry = indexer.definitions.entry(name).or_default();
        entry.extend(interned);
    }
    for (key, loc) in contrib.qualified {
        indexer.qualified.insert(key, indexer.intern_location(&loc));
    }
    for (pkg, uris) in contrib.packages {
        // Intern before taking the shard guard: `file_table` is a separate lock.
        let file_ids: Vec<crate::types::FileId> = uris
            .iter()
            .filter_map(|uri| indexer.intern_uri_str(uri))
            .collect();
        let mut entry = indexer.packages.entry(pkg).or_default();
        entry.extend(file_ids);
    }
    for (super_name, locs) in contrib.subtypes {
        let interned: Vec<crate::types::SymbolLoc> = locs
            .iter()
            .map(|loc| indexer.intern_location(loc))
            .collect();
        let mut entry = indexer.subtypes.entry(super_name).or_default();
        entry.extend(interned);
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
    indexer.note_jar_symbols_populated();
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

/// Clear all compiled-JAR maps — used by callers that want a full reindex
/// (the startup crawl, `handle_reindex`). `index_jars` itself is additive
/// (see below) so on-demand per-JAR materialization never wipes unrelated
/// already-materialized JARs.
// NOTE: `Indexer::clear_jar_index` (src/indexer.rs) is a pre-existing,
// similarly-named method used on workspace-root change. It clears a
// different (overlapping but not identical) field set and additionally
// resets `jar_phase` — this function must NOT touch `jar_phase` since it
// runs mid-crawl. Deliberately not unified with `clear_jar_index` here;
// a future cleanup pass can decide whether to merge them.
pub(crate) fn clear_jar_maps(indexer: &crate::indexer::Indexer) {
    indexer.jar_files.clear();
    indexer.jar_definitions.clear();
    indexer.jar_uri_to_defs.clear();
    indexer.jar_symbol_packages.clear();
}

/// Index the given JAR/AAR files using the sidecar (with disk cache),
/// inserting results into the indexer's symbol maps. ADDITIVE: does not
/// clear existing entries for JARs not in `paths` — callers that want a full
/// reindex call `clear_jar_maps` first. The sidecar handle is borrowed
/// mutably so it can be set to `None` on crash.
pub(crate) fn index_jars(
    indexer: &crate::indexer::Indexer,
    paths: &[PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) -> usize {
    if paths.is_empty() {
        return 0;
    }

    // Memoized across calls on this `Indexer` (see the field doc comment on
    // `jar_symbol_cache`): decode the on-disk cache at most once per process
    // lifetime instead of once per `index_jars` call.
    // `materialize_jar_on_demand` calls this once per on-demand JAR, so
    // without memoization a burst of on-demand promotions (one completion
    // request touching many distinct not-yet-materialized JARs) paid a full
    // disk read + bincode deserialize of a potentially multi-hundred-MB blob
    // on every single promotion.
    let mut jar_symbol_cache_guard = indexer
        .jar_symbol_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if jar_symbol_cache_guard.is_none() {
        *jar_symbol_cache_guard = Some(super::jar_cache::load_jar_cache());
        super::jar_cache::note_jar_cache_loaded(indexer);
    }
    let Some(jar_cache) = jar_symbol_cache_guard.as_mut() else {
        // Unreachable: the branch above always populates `Some` when `None`.
        return 0;
    };

    let mut total = 0usize;
    let mut cache_hits = 0usize;
    let mut newly_cached_entries = 0usize;
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
                            newly_cached_entries += 1;
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

    if newly_cached_entries > 0 {
        // Throttled: on-demand promotion calls this once per cold JAR, and an
        // unthrottled whole-map save per JAR was the wave-7 completion-timeout
        // amplifier (see `maybe_save_jar_cache_throttled`).
        super::jar_cache::maybe_save_jar_cache_throttled(indexer, jar_cache, newly_cached_entries);
    }

    if total > 0 {
        log::info!(
            "jar: indexed {total} symbols from {} compiled JARs/AARs ({cache_hits} from cache)",
            paths.len()
        );
        indexer.note_jar_symbols_populated();
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
    // Build via `Url::from_file_path` (not `path.display()` interpolation) so
    // spaces and other reserved characters are percent-escaped — the same
    // pattern `jar_extract.rs` uses. A raw space (e.g. Windows' default
    // `C:\Program Files\Android\Sdk\...`) makes the resulting `Location.uri`
    // invalid per RFC 3986 and breaks go-to-definition/hover for every
    // symbol from that JAR. `Url::from_file_path` requires `path` to be
    // absolute per the CURRENT OS's own convention, which real production
    // paths (always sourced from this OS's own filesystem) satisfy; fall
    // back to the previous naive construction for the rare case where it
    // doesn't (e.g. a caller-supplied path with a foreign-OS shape) rather
    // than dropping the JAR's symbols entirely.
    let fake_uri_string = match Url::from_file_path(path) {
        Ok(file_url) => format!("jar:{file_url}"),
        Err(()) => format!("jar:file://{}", path.display()),
    };
    let fake_uri = match Url::parse(&fake_uri_string) {
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
            cold: pack_cold_fields(
                sym.type_params.clone(),
                extension_receiver,
                sym.extension_receiver_type.clone(),
                sym.doc.clone(),
            ),
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
            // ... and the candidate's LAST segment must start lowercase, the
            // package-name convention. A package-less NESTED class detail
            // ("class ContextualFlowColumnOverflow.Visible") otherwise
            // parses as `pkg.Type` and poisons the whole jar's fallback
            // package with an outer class name.
            let candidate_is_package_like = candidate
                .rsplit('.')
                .next()
                .and_then(|segment| segment.chars().next())
                .is_some_and(|c| c.is_ascii_lowercase());
            if is_type_name && candidate_is_package_like {
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
        // Parsed data (workspace or sources-JAR, in `files` with real
        // tree-sitter ranges) must win over this pipeline's synthetic
        // one-line-per-symbol locations REGARDLESS of execution order. The
        // crawl guarantees that by ordering (compiled first, sources last),
        // but on-demand materialization runs mid-session — after the
        // sources phase — so an unconditional insert here would invert the
        // invariant: completion's member enumeration range-nests inside the
        // class's span, and a one-line synthetic class has no interior, so
        // every inherited member would vanish from completion while
        // name-keyed hover kept working. Overwrite only entries that are
        // themselves synthetic (not backed by `files`), which also lets a
        // re-materialization of the same JAR refresh its own entries.
        // Copy the existing entry out and DROP the shard guard before touching
        // `file_table` or `files`: holding a `qualified` shard guard while
        // acquiring those locks inverts the documented lock order (see the
        // interning comment in `apply.rs`) against `remove_stale_for_uri`,
        // which holds a `files` guard across `qualified` writes — a real
        // deadlock cycle under shard collision. The check-then-insert gap this
        // opens (another thread inserting a parsed entry in between would be
        // overwritten) is the same order race the crawl already tolerates.
        let synthetic_loc =
            crate::types::SymbolLoc::new(indexer.file_table.intern(fake_uri), symbols[i].range);
        let existing_loc = indexer.qualified.get(&fqn).map(|entry| *entry.value());
        let existing_is_parsed = existing_loc.is_some_and(|existing_loc| {
            indexer
                .file_table
                .location(existing_loc)
                .is_some_and(|existing| indexer.files.contains_key(existing.uri.as_str()))
        });
        if !existing_is_parsed {
            indexer.qualified.insert(fqn, synthetic_loc);
        }
    }

    // Populate extension_by_receiver. `symbols` is index-aligned with
    // `sidecar_symbols` (the qualified loop above already relies on it).
    //
    // The entry's package MUST be the sidecar's real per-symbol `pkg`, not
    // the per-jar inferred `package`: a multi-package jar has no single
    // package, and the inference can be outright garbage — in the real
    // foundation-layout AAR its first dotted class-like detail is the
    // package-less nested `class ContextualFlowColumnOverflow.Visible`, so
    // every extension in the jar carried package
    // "ContextualFlowColumnOverflow" and `extension_is_in_scope` rejected
    // the user's explicitly imported `padding` — chained-call completion
    // (`Modifier.padding().padd…`) returned nothing. The per-jar package
    // remains only as a fallback for pre-v8 sidecars that emit no `pkg`.
    for (i, sym) in symbols.iter().enumerate() {
        if sym.extension_receiver().is_empty() {
            continue;
        }
        let symbol_package = sidecar_symbols
            .get(i)
            .filter(|sidecar_symbol| !sidecar_symbol.pkg.is_empty())
            .map(|sidecar_symbol| sidecar_symbol.pkg.clone())
            .or_else(|| package.clone());
        indexer
            .extension_by_receiver
            .entry(sym.extension_receiver().to_owned())
            .or_default()
            .push(ExtensionEntry {
                file_uri: fake_uri_str.to_owned(),
                name: sym.name.clone(),
                kind: sym.kind,
                detail: sym.detail.clone(),
                visibility: Visibility::Public,
                package: symbol_package,
                trailing_lambda: sym.trailing_lambda,
                deprecated: sym.deprecated,
            });
    }

    indexer.register_file_uri(fake_uri_str);
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

/// Materialize one JAR's full symbol data on demand (Tier 2). Checks
/// `materialized`/`materialization_failed` first; if neither, calls the
/// (now-additive) `index_jars` scoped to just this one JAR's path.
///
/// Returns `true` on success (including "already materialized" — idempotent
/// from the caller's point of view), `false` if materialization failed this
/// call or previously failed this session.
///
/// Callers MUST respect the sidecar-locking discipline in
/// `docs/superpowers/specs/2026-07-10-lazy-library-loading-design.md`
/// §Concurrency (Task 5) — this function does not itself implement the
/// bounded/non-blocking lock acquisition; that lives in the caller
/// (`ensure_jar_materialized`, Task 8), which passes in an already-locked
/// `sidecar` handle only when it got one within budget.
pub(crate) fn materialize_jar_on_demand(
    indexer: &crate::indexer::Indexer,
    jar_id: crate::types::JarId,
    sidecar: &mut Option<SidecarHandle>,
) -> bool {
    if indexer.materialized.contains(&jar_id) {
        return true;
    }
    if indexer.materialization_failed.contains(&jar_id) {
        return false;
    }
    let Some(path_str) = indexer.jar_table.path(jar_id) else {
        indexer.materialization_failed.insert(jar_id);
        return false;
    };
    let path = std::path::PathBuf::from(&path_str);
    let count = index_jars(indexer, std::slice::from_ref(&path), sidecar);
    if count > 0 {
        indexer.materialized.insert(jar_id);
        true
    } else {
        indexer.materialization_failed.insert(jar_id);
        false
    }
}

/// THE named gate+promote for every name-keyed Tier-2 consumer: checks
/// Tier 1 for candidates and promotes them within `sidecar_budget`, in one
/// call. Callers MUST run this before reading `jar_definitions`/`jar_files`
/// (directly or via `resolve_symbol_no_rg`-style helpers) — the check-then-
/// promote-then-read ordering was hand-rolled per site during the lazy-JAR
/// work and produced two shipped promote-AFTER-read bugs plus one silent
/// gate/promotion key mismatch; this function is the single place that
/// ordering and key discipline live now.
///
/// Handles the dotted-spelling mismatch the hand-rolled gates had (PR #215
/// follow-up #13): `jar_qualified_or_bare_has_candidate` passes for a
/// QUALIFIED-only spelling (`class X : com.lib.Base()`), but promotion is
/// keyed by BARE name — so the raw string found no candidates and the
/// promotion silently no-oped, dead-ending hierarchy walks over qualified
/// super types. A dotted `name` now falls back to its leaf segment.
pub(crate) fn ensure_jar_definitions_for(
    indexer: &crate::indexer::Indexer,
    name: &str,
    sidecar_budget: &mut usize,
) -> bool {
    if !indexer.jar_qualified_or_bare_has_candidate(name) {
        return false;
    }
    if ensure_jar_materialized_with_budget(indexer, name, sidecar_budget) {
        return true;
    }
    match name.rsplit('.').next() {
        Some(leaf) if leaf != name && !leaf.is_empty() => {
            ensure_jar_materialized_with_budget(indexer, leaf, sidecar_budget)
        }
        _ => false,
    }
}

/// Atomic gate + promote + READ for extension entries keyed by receiver
/// type: the one call completion/inference sites use instead of pairing a
/// promotion with a separate `extension_by_receiver.get` — pairing the two
/// by hand is exactly the ordering-bug shape that shipped twice during the
/// lazy-JAR work. Source-derived entries are returned even when no JAR
/// declares extensions on `receiver` (the promotion gate only guards the
/// Tier-2 promotion, never the read).
pub(crate) fn extension_entries_for<'indexer>(
    indexer: &'indexer crate::indexer::Indexer,
    receiver: &str,
    sidecar_budget: &mut usize,
) -> Option<dashmap::mapref::one::Ref<'indexer, String, Vec<crate::types::ExtensionEntry>>> {
    if indexer.jar_extension_receivers.contains_key(receiver) {
        ensure_jar_materialized_for_extension_receiver(indexer, receiver, sidecar_budget);
    }
    indexer.extension_by_receiver.get(receiver)
}

/// Shared promotion helper for every direct-read consumer of
/// `jar_definitions`/`jar_files`: if `name` has a Tier-1 candidate that
/// isn't materialized yet, attempt Tier-2 materialization via a bounded,
/// non-blocking sidecar lock attempt (never blocks the caller — see design
/// §Concurrency). Returns whether at least one candidate is now
/// materialized (either already was, or just got promoted).
///
/// Callers: `indexer/resolution.rs`, `indexer/lookup.rs`, `resolver/infer.rs`
/// (Task 8) — each calls this at its own read site rather than through a
/// central chokepoint (design §Consumer integration).
/// `resolver/resolve.rs`'s `importable_fqns` read site is deliberately
/// deferred to Task 9 (auto-import needs different promotion semantics).
pub(crate) fn ensure_jar_materialized(indexer: &crate::indexer::Indexer, name: &str) -> bool {
    let Some(candidates) = indexer.jar_bare_names.get(name) else {
        return false;
    };
    promote_candidates(indexer, candidates.iter().copied())
}

/// Budgeted variant of [`ensure_jar_materialized`] for callers on latency-
/// critical request paths (inlay-hint inference, per-import file-open
/// promotion): `budget` bounds BLOCKING SIDECAR IPC attempts across a whole
/// request, while fresh-cache-backed materializations stay free (pure
/// in-memory, see `promote_candidates_bounded`). A visible editor range can
/// need return-type inference for dozens of names — unbudgeted, that was
/// observed live as a 22s inlay compute (sequential sidecar round trips)
/// that timed out every queued request behind it.
pub(crate) fn ensure_jar_materialized_with_budget(
    indexer: &crate::indexer::Indexer,
    name: &str,
    budget: &mut usize,
) -> bool {
    let Some(candidates) = indexer.jar_bare_names.get(name) else {
        return false;
    };
    promote_candidates_bounded(indexer, candidates.iter().copied(), budget)
}

/// Extension-completion counterpart to `ensure_jar_materialized`: `name`
/// there is a symbol's own bare name, keying into `jar_bare_names`; here it's
/// an extension's receiver leaf type (e.g. "ViewModel"), keying into
/// `jar_extension_receivers`. Both funnel into the same promotion loop —
/// only the Tier-1 candidate lookup differs, since extension completion
/// (`extension_fn_completions`, `complete_bare`'s ancestor-extension loop)
/// doesn't know a specific symbol name in advance, only the receiver type
/// it's enumerating extensions for.
///
/// Unlike `ensure_jar_materialized` (bare names collide across few JARs in
/// practice), a common receiver type ("String", "Iterable") can be declared
/// on by dozens of library JARs — `jar_extension_receivers[receiver]` can be
/// large. `budget` caps how many of THIS call's candidates get a real
/// (blocking sidecar IPC) promotion attempt, decremented per attempt;
/// candidates beyond it are left unmaterialized this call (still offered by
/// name/stub via the existing Tier-1 merge, just without real detail) rather
/// than risking the multi-second cold-completion stall a review of this
/// design found without a cap (Task 12's own finding, same pathology).
pub(crate) fn ensure_jar_materialized_for_extension_receiver(
    indexer: &crate::indexer::Indexer,
    receiver: &str,
    budget: &mut usize,
) -> bool {
    let Some(candidates) = indexer.jar_extension_receivers.get(receiver) else {
        return false;
    };
    promote_candidates_bounded(indexer, candidates.iter().copied(), budget)
}

/// Shared promotion loop for a set of candidate `JarId`s: attempt Tier-2
/// materialization via a bounded, non-blocking sidecar lock attempt (never
/// blocks the caller — see design §Concurrency). Returns whether at least
/// one candidate is now materialized (either already was, or just got
/// promoted).
fn promote_candidates(
    indexer: &crate::indexer::Indexer,
    candidates: impl Iterator<Item = crate::types::JarId>,
) -> bool {
    let mut unbounded = usize::MAX;
    promote_candidates_bounded(indexer, candidates, &mut unbounded)
}

/// Same as `promote_candidates`, but stops attempting further promotions
/// once `budget` (attempts remaining) reaches zero.
///
/// The budget only exists to bound BLOCKING SIDECAR IPC per interactive
/// request, so it is spent only on candidates that genuinely need the
/// sidecar (no fresh entry in the memoized jar-symbol cache). A fresh-cache-
/// backed materialization is a pure in-memory `populate_from_symbols` —
/// milliseconds, no IPC — and throttling it starves completion of most of a
/// common receiver's extensions for no latency benefit (the "extension
/// methods on Modifier missing" regression: `jar_extension_receivers` for a
/// hot receiver type fans out to more JARs than the budget). Already-
/// materialized/already-failed candidates are also free to check.
fn promote_candidates_bounded(
    indexer: &crate::indexer::Indexer,
    candidates: impl Iterator<Item = crate::types::JarId>,
    budget: &mut usize,
) -> bool {
    let mut any_materialized = false;
    for jar_id in candidates {
        if indexer.materialized.contains(&jar_id) {
            any_materialized = true;
            continue;
        }
        if indexer.materialization_failed.contains(&jar_id) {
            continue;
        }
        // Probed BEFORE taking the sidecar lock (and released before
        // `materialize_jar_on_demand` re-locks the same non-reentrant cache
        // mutex inside `index_jars`). A rare freshness change between probe
        // and materialization only miscounts the budget by one — harmless.
        let cache_backed = jar_symbol_cache_is_fresh_for(indexer, jar_id);
        if !cache_backed && *budget == 0 {
            continue; // degrade gracefully — later calls/requests may promote the rest
        }
        let Some(mut sidecar_guard) =
            crate::workspace::scan_handler::try_lock_sidecar_bounded(indexer)
        else {
            continue; // degrade gracefully — a later call may succeed
        };
        if !cache_backed {
            *budget -= 1;
        }
        // `sidecar_guard` is `MutexGuard<Option<SidecarHandle>>`;
        // `materialize_jar_on_demand` takes `&mut Option<SidecarHandle>` — auto-deref
        // coercion handles the `MutexGuard` → `Option<SidecarHandle>` step here
        // (clippy: `&mut *sidecar_guard` is flagged as a redundant explicit deref).
        if materialize_jar_on_demand(indexer, jar_id, &mut sidecar_guard) {
            any_materialized = true;
        }
    }
    any_materialized
}

/// True when the memoized on-disk jar-symbol cache holds a FRESH entry for
/// `jar_id`'s path — i.e. materializing it is a pure in-memory
/// `populate_from_symbols` with no sidecar IPC. Lazily decodes the cache on
/// first use (the same one-time memoization `index_jars` performs; whichever
/// runs first pays it). The guard is dropped on return — callers re-lock the
/// same mutex inside `index_jars`, which is non-reentrant.
fn jar_symbol_cache_is_fresh_for(
    indexer: &crate::indexer::Indexer,
    jar_id: crate::types::JarId,
) -> bool {
    let Some(path_str) = indexer.jar_table.path(jar_id) else {
        return false;
    };
    let mut jar_symbol_cache_guard = indexer
        .jar_symbol_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if jar_symbol_cache_guard.is_none() {
        *jar_symbol_cache_guard = Some(super::jar_cache::load_jar_cache());
        super::jar_cache::note_jar_cache_loaded(indexer);
    }
    let Some(jar_cache) = jar_symbol_cache_guard.as_ref() else {
        return false;
    };
    jar_cache.get(&path_str).is_some_and(|entry| {
        super::jar_cache::cache_entry_is_fresh(entry, std::path::Path::new(&path_str))
    })
}

/// Tier 1: build the lightweight manifest (name+kind+container only) for
/// every JAR in `paths`. Cache-hit path reads `jar-manifest-v1.bin` directly
/// (cheap). Cache-miss path calls the sidecar (same cost as full
/// materialization on the sidecar side — see design §Tier 1 for why this is
/// a one-time cost, not a recurring one) but discards detail/params/doc
/// immediately rather than constructing a long-lived `SidecarSymbol`.
///
/// Does NOT touch `jar_definitions`/`jar_files`/`materialized` — Tier 1 and
/// Tier 2 are separate maps by design (§Tier 1); a consumer must call
/// `materialize_jar_on_demand` separately to get full data for a JAR this
/// function has manifested.
pub(crate) fn build_jar_manifest(
    indexer: &crate::indexer::Indexer,
    paths: &[PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) -> usize {
    if paths.is_empty() {
        return 0;
    }

    let mut manifest_cache = super::jar_manifest_cache::load_jar_manifest_cache();
    let mut total_names = 0usize;
    let mut cache_dirty = false;
    let mut missed: Vec<(PathBuf, String)> = Vec::new();

    for path in paths {
        let path_key = path.to_string_lossy().to_string();
        let jar_id = indexer.jar_table.intern(&path_key);

        if let Some(entry) = manifest_cache.get(&path_key) {
            if super::jar_manifest_cache::manifest_entry_is_fresh(entry, path) {
                total_names += populate_tier1_from_manifest(indexer, jar_id, &entry.names);
                continue;
            }
        }
        missed.push((path.clone(), path_key));
    }

    if !missed.is_empty() {
        if let Some(ref mut sidecar_guard) = sidecar {
            let sidecar_paths: Vec<&Path> = missed.iter().map(|(p, _)| p.as_path()).collect();
            match sidecar_guard.index_jars(&sidecar_paths) {
                Ok(results) => {
                    for ((path, path_key), symbols) in missed.into_iter().zip(results) {
                        let jar_id = indexer.jar_table.intern(&path_key);
                        let names: Vec<super::jar_manifest_cache::JarManifestName> = symbols
                            .iter()
                            .map(|s| super::jar_manifest_cache::JarManifestName {
                                name: s.name.clone(),
                                kind: s.kind.clone(),
                                container: (!s.container.is_empty()).then(|| s.container.clone()),
                                // `s.pkg` is the sidecar's real per-symbol
                                // package (same field `jar.rs`'s Tier-2 path
                                // already uses to build `indexer.qualified`
                                // FQNs) — carry it through so Tier 1 can build
                                // real FQNs too, not just short names.
                                package: (!s.pkg.is_empty()).then(|| s.pkg.clone()),
                                // Leaf-strip the same way Tier 2's
                                // `build_jar_file_data` derives its
                                // `extension_by_receiver` key (`sym
                                // .extension_receiver_type.split('<').next()`)
                                // — carrying this through lets Tier 1 know
                                // this JAR defines an extension on a given
                                // receiver type without materializing it.
                                extension_receiver: (!s.extension_receiver_type.is_empty()).then(
                                    || {
                                        s.extension_receiver_type
                                            .split('<')
                                            .next()
                                            .unwrap_or("")
                                            .to_owned()
                                    },
                                ),
                            })
                            .collect();
                        total_names += populate_tier1_from_manifest(indexer, jar_id, &names);
                        if let Some(entry) = make_manifest_entry(&path, names) {
                            manifest_cache.insert(path_key, entry);
                            cache_dirty = true;
                        }
                        // `symbols` (the full SidecarSymbol vec with
                        // detail/params/doc) is dropped here at the end of
                        // this iteration — never inserted into any long-lived
                        // map. This is the discard point the module doc
                        // promises.
                    }
                }
                Err(err) => {
                    log::warn!("jar_manifest: sidecar batch error: {err} — disabling sidecar");
                    *sidecar = None;
                }
            }
        }
    }

    if cache_dirty {
        super::jar_manifest_cache::save_jar_manifest_cache(&manifest_cache);
    }

    if total_names > 0 {
        // Mirror `index_jars`' invalidation (above): `jar_bare_names` was
        // just populated (via `populate_tier1_from_manifest`) with data that
        // `ensure_bare_names_fresh`'s consumers (`complete_bare` and
        // friends) don't see until `rebuild_bare_name_cache` runs, which
        // only happens when `bare_names_dirty` is set. Without this, a
        // completion request that races ahead of the crawl (normal timing —
        // the crawl runs on a background thread) consumes the
        // already-`true` initial value of `bare_names_dirty` against empty
        // pre-crawl data, and nothing ever flips it back to `true`
        // afterward — every Tier-1-only candidate this call just manifested
        // would stay permanently invisible to bare-word/auto-import
        // completion for the rest of the process's life.
        indexer.note_jar_symbols_populated();
    }
    total_names
}

/// Populate `jar_bare_names`/`jar_qualified` (Tier 1) for one JAR's manifest
/// names. Never touches `jar_definitions`/`jar_files`/`materialized` (Tier
/// 2). Returns the number of names populated.
pub(crate) fn populate_tier1_from_manifest(
    indexer: &crate::indexer::Indexer,
    jar_id: crate::types::JarId,
    names: &[super::jar_manifest_cache::JarManifestName],
) -> usize {
    for entry in names {
        indexer
            .jar_bare_names
            .entry(entry.name.clone())
            .or_default()
            .push(jar_id);
        // Build the real FQN straight from the manifest's own `package`
        // field (Task 3) — this is exactly what jar.rs's Tier-2 path already
        // does with `SidecarSymbol::pkg`/`sym.container` to populate
        // `indexer.qualified` (see `effective_pkg`/`sym.top_level` above),
        // so Tier 1 needs no separate FQN-construction mechanism, and
        // jar_qualified is never a dead map. Top-level symbols (no
        // container, or a manifest cached before `container` existed) get
        // `pkg.name`; class members (companion functions, nested classes,
        // enum entries) get `pkg.Container.name` — dropping `container`
        // here would collide same-named members from different classes in
        // the same package under one wrong FQN.
        if let Some(pkg) = entry.package.as_deref().filter(|p| !p.is_empty()) {
            let fqn = match entry.container.as_deref().filter(|c| !c.is_empty()) {
                Some(container) => format!("{pkg}.{container}.{}", entry.name),
                None => format!("{pkg}.{}", entry.name),
            };
            indexer.jar_qualified.entry(fqn).or_insert(jar_id);
        }
        // No package (default package, or a manifest cached before this
        // field existed): the symbol is still reachable via
        // `jar_bare_names` for completion/auto-import candidate listing —
        // just not by exact-FQN lookup until Tier 2 materializes it.

        // Tier 1 extension-receiver index: lets extension completion know
        // this JAR declares an extension on `receiver` without reading
        // `extension_by_receiver` (Tier 2, populated only by materialization).
        if let Some(receiver) = entry
            .extension_receiver
            .as_deref()
            .filter(|r| !r.is_empty())
        {
            let mut slot = indexer
                .jar_extension_receivers
                .entry(receiver.to_owned())
                .or_default();
            // Manifests are processed contiguously per JAR, so a run of
            // several extensions on the same receiver (common for a library
            // JAR with many extensions on e.g. "String") only needs a
            // same-as-last check, not a full scan, to avoid storing this
            // JarId once per extension.
            if slot.last() != Some(&jar_id) {
                slot.push(jar_id);
            }
        }
    }
    names.len()
}

/// Build a `JarManifestEntry` (mtime/size + names) for a JAR that was just
/// manifested via the sidecar. Returns `None` if the JAR's metadata can't be
/// read (e.g. removed between the sidecar call and here) — the manifest is
/// simply not cached in that case, and the next crawl will re-attempt it.
fn make_manifest_entry(
    jar: &Path,
    names: Vec<super::jar_manifest_cache::JarManifestName>,
) -> Option<super::jar_manifest_cache::JarManifestEntry> {
    let meta = std::fs::metadata(jar).ok()?;
    let mtime = meta.modified().ok()?;
    let duration = mtime
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Some(super::jar_manifest_cache::JarManifestEntry {
        mtime_secs: duration.as_secs(),
        mtime_nanos: duration.subsec_nanos(),
        file_size: meta.len(),
        names,
    })
}
