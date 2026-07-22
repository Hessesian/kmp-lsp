# Sources-JAR Parse Cache Implementation Plan (v2 — post-critique)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Skip tree-sitter re-parsing of unchanged `*-sources.jar` files on startup by caching parse results to disk, cutting sources indexing from ~18s to ~3-5s (Phase 1 of `docs/startup-speed-plan.md`).

**Architecture:** Mirror the proven `jar_cache.rs` pattern (versioned bincode map keyed by JAR path, mtime+size fingerprint freshness, atomic rename) but store per-file `(uri, content_hash, Arc<FileData>)` — preserving per-file go-to-definition granularity. Cache hits build `FileContributions` directly from the deserialized `Arc<FileData>` (zero deep clones) and flow through the same `apply_contribution_to_index` as fresh parses.

**Critique fixes incorporated (v2):** structural per-JAR grouping instead of URI-string reverse-mapping (URL percent-encoding made it silently drop files); parse-thread panics block caching (immutable fingerprints would hide the gap forever); Arc-based zero-clone hit path (naive path was 3 deep clones per file); cache filename couples to `cache::CACHE_VERSION` (bincode positional decode can silently mis-decode after schema changes); pruning of entries for deleted JARs + stale version files (unbounded growth of full source text); exact hit counting; multi-process last-writer-wins documented.

**Tech Stack:** Rust, serde + bincode 1.x (positional — schema changes REQUIRE version bump), tree-sitter via existing `parse_file`, `std::thread::scope` parallelism.

**Constraints recap (from AGENTS.md / mem:core):**
- No abbreviated names, no `unwrap()`/`expect()` in production code, `pub(crate)` not `pub`.
- Tests in companion `*_tests.rs` wired via `#[cfg(test)] #[path = ...]`.
- `cargo test` + `cargo clippy -- -D warnings` + `cargo fmt` after every task.
- Commit per task, staging ONLY the files listed in that task (the working tree has unrelated in-flight changes — never `git add -A`).

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/indexer/sources_jar_cache.rs` | Create | On-disk cache: types, fingerprint, load/save/prune. NO Indexer knowledge — pure data + IO. |
| `src/indexer/sources_jar_cache_tests.rs` | Create | Roundtrip + freshness + pruning tests. |
| `src/indexer/apply.rs` | Modify | Extract `derive_supertypes` + `contributions_from_data` (Arc-based) from existing code. |
| `src/indexer/cache.rs` | Modify | `cache_entry_to_file_result` delegates supertypes derivation. |
| `src/indexer/jar.rs` | Modify | Split `index_jar_entries`; rewire `index_sources_jars` around the cache. |
| `src/indexer/jar_tests.rs` | Modify | New cache-integration tests; update the ONE existing `index_sources_jars` call site (line ~602) to 3-arg. |
| `src/indexer.rs` | Modify | Add `pub(crate) mod sources_jar_cache;` + test module wiring. |
| `src/workspace/scan_handler.rs` | Modify | Pass `None` cache-dir at the production call site (~line 355). |
| `src/indexer/sources_cache.rs` | Delete | Orphaned lazy experiment (backed up in `debug-scripts/lazy-experiment-backup/`). |

**Key existing APIs (verified against source — do not reimplement):**
- `Indexer::parse_file(uri: &Url, content: &str) -> FileIndexResult` — pure, returns `content_hash` (apply.rs:437).
- `file_contributions(&FileIndexResult) -> FileContributions` — `pub(crate)`, apply.rs:132; its only `FileData` clone is `Arc::new(result.data.clone())` at the end.
- `FileContributions.file_data: (String, Arc<FileData>)` (indexer.rs:127) — the Arc is what lands in the index.
- `Indexer::remove_stale_for_uri(&self, uri_str: &str)` (apply.rs:495).
- `cache::xdg_cache_base()`, `cache::CACHE_VERSION: u32` — both `pub(crate)` (cache.rs:24).
- `FileData` is `Serialize + Deserialize` (`serde/rc` enabled; `#[serde(skip)] syntax_errors` deserializes to empty Vec — harmless).
- `SourceSet::Library` — jar.rs already imports `SourceSet`.

---

### Task 1: `sources_jar_cache.rs` — types, fingerprint, load/save/prune

**Files:**
- Create: `src/indexer/sources_jar_cache.rs`
- Create: `src/indexer/sources_jar_cache_tests.rs`
- Modify: `src/indexer.rs` (module declarations, next to `pub(crate) mod jar_cache;`)

- [ ] **Step 1: Write the failing tests**

`src/indexer/sources_jar_cache_tests.rs`:

```rust
//! Tests for the sources-JAR parse cache (disk roundtrip, freshness, pruning).

use std::sync::Arc;

use super::sources_jar_cache::{
    entry_is_fresh, jar_fingerprint, load_sources_jar_cache, prune_deleted_jars,
    save_sources_jar_cache, SourcesFileEntry, SourcesJarEntry,
};

/// Parse a tiny Kotlin snippet to get a realistic FileData for fixtures.
fn parsed_file_entry(uri_text: &str, source: &str) -> SourcesFileEntry {
    let uri = tower_lsp::lsp_types::Url::parse(uri_text).expect("test uri");
    let result = crate::indexer::Indexer::parse_file(&uri, source);
    SourcesFileEntry {
        uri: uri_text.to_owned(),
        content_hash: result.content_hash,
        file_data: Arc::new(result.data),
    }
}

fn entry_for(fingerprint: &super::sources_jar_cache::JarFingerprint, files: Vec<SourcesFileEntry>) -> SourcesJarEntry {
    SourcesJarEntry {
        mtime_secs: fingerprint.mtime_secs,
        mtime_nanos: fingerprint.mtime_nanos,
        file_size: fingerprint.file_size,
        files,
    }
}

#[test]
fn roundtrip_preserves_entries() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"not a real zip, fingerprint only").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");

    let file_entry = parsed_file_entry(
        "jar:file:///fake/lib-1.0-sources.jar!/com/example/Core.kt",
        "package com.example\n\nclass Core\n",
    );
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        entry_for(&fingerprint, vec![file_entry]),
    );

    save_sources_jar_cache(Some(tmpdir.path()), &entries);
    let loaded = load_sources_jar_cache(Some(tmpdir.path()));

    let entry = loaded
        .get(jar_path.to_string_lossy().as_ref())
        .expect("entry survives roundtrip");
    assert_eq!(entry.files.len(), 1);
    assert_eq!(entry.files[0].file_data.symbols.len(), 1);
    assert_eq!(entry.files[0].file_data.symbols[0].name, "Core");
    assert_eq!(
        entry.files[0].file_data.package.as_deref(),
        Some("com.example")
    );
}

#[test]
fn load_from_missing_dir_returns_empty() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let loaded = load_sources_jar_cache(Some(&tmpdir.path().join("does-not-exist")));
    assert!(loaded.is_empty());
}

#[test]
fn entry_is_fresh_matches_unchanged_file() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");
    let entry = entry_for(&fingerprint, Vec::new());
    let current = jar_fingerprint(&jar_path).expect("fingerprint again");
    assert!(entry_is_fresh(&entry, &current));
}

#[test]
fn entry_is_stale_after_size_change() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");
    let entry = entry_for(&fingerprint, Vec::new());
    std::fs::write(&jar_path, b"content grew larger").expect("rewrite jar");
    let current = jar_fingerprint(&jar_path).expect("fingerprint after change");
    assert!(!entry_is_fresh(&entry, &current));
}

#[test]
fn prune_drops_entries_for_deleted_jars() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let live_jar = tmpdir.path().join("live-1.0-sources.jar");
    std::fs::write(&live_jar, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&live_jar).expect("fingerprint");

    let mut entries = std::collections::HashMap::new();
    entries.insert(
        live_jar.to_string_lossy().to_string(),
        entry_for(&fingerprint, Vec::new()),
    );
    entries.insert(
        tmpdir
            .path()
            .join("deleted-0.9-sources.jar")
            .to_string_lossy()
            .to_string(),
        entry_for(&fingerprint, Vec::new()),
    );

    let pruned = prune_deleted_jars(&mut entries);
    assert!(pruned, "pruning removed something");
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(live_jar.to_string_lossy().as_ref()));

    let pruned_again = prune_deleted_jars(&mut entries);
    assert!(!pruned_again, "second prune is a no-op");
}
```

- [ ] **Step 2: Wire test module and confirm compile failure**

In `src/indexer.rs`, directly after `pub(crate) mod jar_cache;` add:

```rust
pub(crate) mod sources_jar_cache;

#[cfg(test)]
#[path = "indexer/sources_jar_cache_tests.rs"]
mod sources_jar_cache_tests;
```

Run: `cargo test sources_jar_cache 2>&1 | head -20`
Expected: FAIL — `sources_jar_cache.rs` not found / unresolved imports.

- [ ] **Step 3: Implement the cache module**

`src/indexer/sources_jar_cache.rs`:

```rust
//! Disk cache for sources-JAR parse results.
//!
//! Mirrors `jar_cache.rs` (compiled-JAR sidecar cache) but stores tree-sitter
//! parse output per source file: `(uri, content_hash, FileData)`.  Keeping
//! per-file granularity (instead of flattening symbols per JAR) preserves
//! go-to-definition into individual `jar:file://…!/path/File.kt` entries and
//! lets cache hits reuse the exact same apply path as fresh parses.
//!
//! JARs in the Gradle cache are immutable after download, so `(mtime, size)`
//! fingerprinting is safe.  Entries whose JAR no longer exists on disk are
//! pruned (`prune_deleted_jars`) — unlike the compiled-JAR cache, entries here
//! hold full source text, so unbounded growth would reach GBs.
//!
//! Writers use an atomic rename (write temp → rename) to avoid corruption.
//! Concurrent kmp-lsp processes race load→modify→save; the last writer wins
//! and the loser re-parses on its next start.  Accepted trade-off (same as
//! `jar_cache.rs`) — no lock file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::types::FileData;

/// Bump when the on-disk schema of THIS module changes.  Changes to `FileData`
/// or `SymbolEntry` are covered automatically: the cache filename embeds
/// `cache::CACHE_VERSION`, which the project rule (mem:core) already bumps on
/// every such change — bincode 1.x is positional and can silently mis-decode
/// reordered same-shaped fields, so filename coupling is load-bearing.
const SOURCES_JAR_CACHE_VERSION: u32 = 1;

#[derive(Deserialize)]
struct SourcesJarCacheDisk {
    version: u32,
    entries: HashMap<String, SourcesJarEntry>,
}

/// Borrow-only view used for serialization — avoids cloning the entries map.
#[derive(Serialize)]
struct SourcesJarCacheDiskRef<'a> {
    version: u32,
    entries: &'a HashMap<String, SourcesJarEntry>,
}

/// Cached parse results for one sources JAR.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct SourcesJarEntry {
    pub(crate) mtime_secs: u64,
    pub(crate) mtime_nanos: u32,
    pub(crate) file_size: u64,
    /// One entry per successfully parsed `.kt`/`.java` file in the JAR.
    pub(crate) files: Vec<SourcesFileEntry>,
}

/// Cached parse result for one source file inside a JAR.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct SourcesFileEntry {
    /// Full synthetic URI, e.g. `jar:file:///…/lib-sources.jar!/com/example/Core.kt`.
    pub(crate) uri: String,
    pub(crate) content_hash: u64,
    /// Stored with `source_set` already set to `Library` so the apply path
    /// never needs to clone-and-override.
    pub(crate) file_data: Arc<FileData>,
}

/// `(mtime, size)` identity of a JAR file, captured BEFORE extraction so a
/// concurrent JAR replacement cannot pair new metadata with old parse results.
pub(crate) struct JarFingerprint {
    pub(crate) mtime_secs: u64,
    pub(crate) mtime_nanos: u32,
    pub(crate) file_size: u64,
}

/// Read the current fingerprint of a JAR file. `None` if unreadable.
pub(crate) fn jar_fingerprint(jar_path: &Path) -> Option<JarFingerprint> {
    let metadata = std::fs::metadata(jar_path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Some(JarFingerprint {
        mtime_secs: duration.as_secs(),
        mtime_nanos: duration.subsec_nanos(),
        file_size: metadata.len(),
    })
}

/// Check whether a cache entry still matches the JAR's current fingerprint.
pub(crate) fn entry_is_fresh(entry: &SourcesJarEntry, current: &JarFingerprint) -> bool {
    entry.file_size == current.file_size
        && entry.mtime_secs == current.mtime_secs
        && entry.mtime_nanos == current.mtime_nanos
}

/// Drop entries whose JAR no longer exists on disk.  Returns true if anything
/// was removed.  Keeps cross-workspace sharing intact: live JARs in
/// `~/.gradle` exist regardless of which workspace is open.
pub(crate) fn prune_deleted_jars(entries: &mut HashMap<String, SourcesJarEntry>) -> bool {
    let count_before = entries.len();
    entries.retain(|jar_path, _| Path::new(jar_path).exists());
    entries.len() != count_before
}

/// Cache file location.  `cache_dir` overrides the default
/// `~/.cache/kmp-lsp/` base — used by tests for isolation.
fn cache_file_path(cache_dir: Option<&Path>) -> PathBuf {
    let base = match cache_dir {
        Some(dir) => dir.to_owned(),
        None => super::cache::xdg_cache_base().join("kmp-lsp"),
    };
    base.join(format!(
        "sources-jar-v{SOURCES_JAR_CACHE_VERSION}-c{}.bin",
        super::cache::CACHE_VERSION
    ))
}

/// Load the global sources-JAR parse cache.  Empty map on any error.
pub(crate) fn load_sources_jar_cache(
    cache_dir: Option<&Path>,
) -> HashMap<String, SourcesJarEntry> {
    let path = cache_file_path(cache_dir);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return HashMap::new(),
    };
    let reader = std::io::BufReader::new(file);
    match bincode::deserialize_from::<_, SourcesJarCacheDisk>(reader) {
        Ok(disk) if disk.version == SOURCES_JAR_CACHE_VERSION => {
            log::debug!(
                "sources_jar_cache: loaded {} JAR entries",
                disk.entries.len()
            );
            disk.entries
        }
        _ => {
            log::debug!("sources_jar_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

/// Save the cache atomically (write temp → rename), then delete stale
/// `sources-jar-*.bin` files from older versions (each can be hundreds of MB).
/// Streams via `BufWriter` — the payload is far larger than the compiled-JAR
/// cache.
pub(crate) fn save_sources_jar_cache(
    cache_dir: Option<&Path>,
    entries: &HashMap<String, SourcesJarEntry>,
) {
    let path = cache_file_path(cache_dir);
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("sources_jar_cache: cannot create cache dir: {error}");
            return;
        }
    }
    let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    let file = match std::fs::File::create(&temp_path) {
        Ok(f) => f,
        Err(error) => {
            log::warn!("sources_jar_cache: create temp error: {error}");
            return;
        }
    };
    let writer = std::io::BufWriter::new(file);
    let disk = SourcesJarCacheDiskRef {
        version: SOURCES_JAR_CACHE_VERSION,
        entries,
    };
    if let Err(error) = bincode::serialize_into(writer, &disk) {
        log::warn!("sources_jar_cache: serialize error: {error}");
        let _ = std::fs::remove_file(&temp_path);
        return;
    }
    if let Err(error) = std::fs::rename(&temp_path, &path) {
        log::warn!("sources_jar_cache: rename error: {error}");
        let _ = std::fs::remove_file(&temp_path);
        return;
    }
    log::debug!("sources_jar_cache: saved {} JAR entries", entries.len());
    remove_stale_version_files(&path);
}

/// Delete sibling `sources-jar-*.bin` files that are not the current cache
/// file.  In-flight temp files (`.tmp.<pid>` extension) are not matched.
fn remove_stale_version_files(current: &Path) {
    let Some(parent) = current.parent() else {
        return;
    };
    let Ok(dir_entries) = std::fs::read_dir(parent) else {
        return;
    };
    for dir_entry in dir_entries.flatten() {
        let entry_path = dir_entry.path();
        if entry_path == current {
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("sources-jar-") && name.ends_with(".bin") {
            let _ = std::fs::remove_file(&entry_path);
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test sources_jar_cache -- --nocapture 2>&1 | tail -5`
Expected: 5 passed, 0 failed.

- [ ] **Step 5: Lint + format + commit**

```bash
cargo clippy --quiet -- -D warnings && cargo fmt
git add src/indexer/sources_jar_cache.rs src/indexer/sources_jar_cache_tests.rs src/indexer.rs
git commit -m "feat(sources-jar): add disk parse cache module (load/save/fingerprint/prune)"
```

---

### Task 2: Arc-based contribution path in `apply.rs` (pure refactor + one new entry point)

**Files:**
- Modify: `src/indexer/apply.rs` (`file_contributions` at :132, `parse_file` at :437)
- Modify: `src/indexer/cache.rs` (`cache_entry_to_file_result` at :154)

The supertypes derivation currently exists in BOTH `parse_file` and `cache_entry_to_file_result`; `file_contributions` always deep-clones `FileData`. Extract one derivation and one Arc-based constructor so cache hits pay zero deep clones.

- [ ] **Step 1: Refactor `apply.rs`**

Add (near `file_contributions`):

```rust
/// Derive supertype relationships from parsed data — the same logic
/// `parse_file` runs at parse time.  Shared by the workspace cache and the
/// sources-JAR parse cache, which rebuild results from persisted `FileData`.
pub(crate) fn derive_supertypes(
    uri: &Url,
    data: &FileData,
) -> Vec<(String, Location)> {
    let class_kinds = [
        SymbolKind::CLASS,
        SymbolKind::INTERFACE,
        SymbolKind::STRUCT,
        SymbolKind::ENUM,
        SymbolKind::OBJECT,
    ];
    let mut supertypes: Vec<(String, Location)> = Vec::new();
    for symbol in &data.symbols {
        if !class_kinds.contains(&symbol.kind) {
            continue;
        }
        let start_line = symbol.selection_start();
        let class_location = Location {
            uri: uri.clone(),
            range: symbol.selection_range,
        };
        for (_, super_name, _) in data.supers.iter().filter(|(line, _, _)| *line == start_line) {
            supertypes.push((super_name.clone(), class_location.clone()));
        }
    }
    supertypes
}

/// Build `FileContributions` from an `Arc<FileData>` WITHOUT cloning the data.
/// This is the cache-hit fast path: the deserialized Arc flows straight into
/// the index.
pub(crate) fn contributions_from_data(
    uri: &Url,
    file_data: Arc<FileData>,
    content_hash: u64,
    supertypes: &[(String, Location)],
) -> FileContributions {
    let uri_str = uri.to_string();
    let file_stem = crate::path_util::file_stem_from_uri(uri);

    let mut definitions: HashMap<String, Vec<Location>> = HashMap::new();
    let mut qualified: HashMap<String, Location> = HashMap::new();

    for symbol in &file_data.symbols {
        let location = Location {
            uri: uri.clone(),
            range: symbol.selection_range,
        };
        definitions
            .entry(symbol.name.clone())
            .or_default()
            .push(location.clone());
        if let Some(ref package) = file_data.package {
            qualified.insert(format!("{package}.{}", symbol.name), location.clone());
            if let Some(ref stem) = file_stem {
                if *stem != symbol.name {
                    qualified.insert(format!("{package}.{stem}.{}", symbol.name), location);
                }
            }
        }
    }

    let mut packages: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(ref package) = file_data.package {
        packages
            .entry(package.clone())
            .or_default()
            .push(uri_str.clone());
    }

    let mut subtypes: HashMap<String, Vec<Location>> = HashMap::new();
    for (super_name, class_location) in supertypes {
        subtypes
            .entry(super_name.clone())
            .or_default()
            .push(class_location.clone());
    }

    let mut extensions: HashMap<String, Vec<ExtensionEntry>> = HashMap::new();
    for symbol in &file_data.symbols {
        if symbol.extension_receiver.is_empty() {
            continue;
        }
        extensions
            .entry(symbol.extension_receiver.clone())
            .or_default()
            .push(ExtensionEntry {
                file_uri: uri_str.clone(),
                name: symbol.name.clone(),
                kind: symbol.kind,
                detail: symbol.detail.clone(),
                visibility: symbol.visibility,
                package: file_data.package.clone(),
                trailing_lambda: symbol.trailing_lambda,
            });
    }

    let content_hash_entry = (uri_str.clone(), content_hash);
    FileContributions {
        definitions,
        qualified,
        packages,
        subtypes,
        extensions,
        file_data: (uri_str, file_data),
        content_hash: content_hash_entry,
    }
}
```

Then shrink `file_contributions` to a delegating wrapper (its observable behavior is identical — the body above is its current body, parameterized):

```rust
pub(crate) fn file_contributions(result: &FileIndexResult) -> FileContributions {
    contributions_from_data(
        &result.uri,
        Arc::new(result.data.clone()),
        result.content_hash,
        &result.supertypes,
    )
}
```

In `parse_file` (apply.rs:437), replace the inline supertypes block (the `class_kinds` array + `for sym in &data.symbols` loop) with:

```rust
let supertypes = derive_supertypes(uri, &data);
```

- [ ] **Step 2: Delegate in `cache.rs`**

In `cache_entry_to_file_result` (cache.rs:154), replace the inline `class_kinds` + loop with:

```rust
let supertypes = crate::indexer::apply::derive_supertypes(uri, data);
```

(keeping the existing `FileIndexResult { ... }` construction; `data` there is `&entry.file_data`).

- [ ] **Step 3: Verify behavior unchanged**

Run: `cargo test --quiet 2>&1 | rg "test result" | rg -v "0 failed" ; echo "exit=$?"`
Expected: `exit=1` (no line had failures).

- [ ] **Step 4: Lint + format + commit**

```bash
cargo clippy --quiet -- -D warnings && cargo fmt
git add src/indexer/apply.rs src/indexer/cache.rs
git commit -m "refactor(apply): extract derive_supertypes + Arc-based contributions_from_data"
```

---

### Task 3: Split `index_jar_entries` into parse + apply halves

**Files:**
- Modify: `src/indexer/jar.rs` (`index_jar_entries`, ~line 201; `apply_contribution_to_index` ~line 274)

Cache misses need parse results both applied AND captured per-JAR for the cache; cache hits need apply-only from `FileContributions`. `index_jar_entries` keeps its signature (8 direct test callers in jar_tests.rs).

- [ ] **Step 1: Refactor**

Replace `index_jar_entries` with:

```rust
/// In-memory sources-JAR indexing path.  Takes pre-extracted `(uri, content)`
/// pairs and runs the parse + apply phase.  This is the function unit tests
/// call directly with mocked entries — no Gradle cache walk, no ZIP reading.
pub(crate) fn index_jar_entries(
    indexer: &crate::indexer::Indexer,
    entries: Vec<(Url, String)>,
) -> usize {
    let parsed = parse_jar_entries(entries);
    if !parsed.complete {
        log::error!("jar: a parse thread panicked — index may be missing files");
    }
    let contributions: Vec<crate::indexer::FileContributions> = parsed
        .results
        .iter()
        .map(crate::indexer::apply::file_contributions)
        .collect();
    apply_sources_contributions(indexer, contributions)
}

/// Result of parallel-parsing JAR entries.
struct ParsedJarEntries {
    results: Vec<crate::types::FileIndexResult>,
    /// False if a parse thread panicked — `results` are partial.  Callers
    /// must NOT cache partial results: JAR fingerprints never change (Gradle
    /// cache files are immutable), so a partial entry would hide the missing
    /// files forever.
    complete: bool,
}

/// Parse `(uri, content)` pairs in parallel with tree-sitter.  Pure — no
/// index writes.  Files that fail to parse are dropped.  Every result's
/// `source_set` is forced to `Library` (these are always library sources).
fn parse_jar_entries(entries: Vec<(Url, String)>) -> ParsedJarEntries {
    if entries.is_empty() {
        return ParsedJarEntries {
            results: Vec::new(),
            complete: true,
        };
    }
    let mut complete = true;
    let results = std::thread::scope(|scope| {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let chunk_size = (entries.len() / num_threads).max(1);

        let handles: Vec<_> = entries
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local_results = Vec::with_capacity(chunk.len());
                    for (uri, content) in chunk {
                        let mut result = crate::indexer::Indexer::parse_file(uri, content);
                        if result.error.is_some() {
                            continue;
                        }
                        result.data.source_set = SourceSet::Library;
                        local_results.push(result);
                    }
                    local_results
                })
            })
            .collect();

        let mut all_results = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(mut chunk_results) => all_results.append(&mut chunk_results),
                Err(_) => complete = false,
            }
        }
        all_results
    });
    ParsedJarEntries { results, complete }
}

/// Apply sources-JAR contributions to the index: stale-removal pre-pass,
/// parallel inserts, then derived-cache invalidation.  Returns the number of
/// symbols applied.  Used by both the fresh-parse and cache-hit paths.
fn apply_sources_contributions(
    indexer: &crate::indexer::Indexer,
    mut contributions: Vec<crate::indexer::FileContributions>,
) -> usize {
    if contributions.is_empty() {
        return 0;
    }

    // Pre-pass: remove stale entries for all URIs we're about to insert.
    // Without this, re-running on the same set would double-count symbols in
    // `definitions` / `packages` / `subtypes` / `extension_by_receiver` since
    // the parallel insert below uses `entry().or_default().extend(...)`.
    for contribution in &contributions {
        indexer.remove_stale_for_uri(&contribution.file_data.0);
    }

    // Chunk by value so worker threads own their contributions (DashMap is
    // thread-safe; concurrent inserts from multiple threads are safe).
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunk_size = (contributions.len() / num_threads).max(1);
    let mut owned_chunks: Vec<Vec<crate::indexer::FileContributions>> = Vec::new();
    while !contributions.is_empty() {
        let split_at = chunk_size.min(contributions.len());
        let tail = contributions.split_off(split_at);
        owned_chunks.push(std::mem::replace(&mut contributions, tail));
    }

    let total_symbols: usize = std::thread::scope(|scope| {
        let handles: Vec<_> = owned_chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local_symbols = 0usize;
                    for contribution in chunk {
                        indexer.library_uris.insert(contribution.file_data.0.clone());
                        local_symbols += contribution.file_data.1.symbols.len();
                        apply_contribution_to_index(indexer, contribution);
                    }
                    local_symbols
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or(0))
            .sum()
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
```

NOTES:
- The old `index_jar_entries` did the parse and apply in ONE parallel pass; this splits them (parse barrier, then apply). The parse phase dominates; the extra pass over already-materialized contributions is cheap.
- `apply_contribution_to_index` keeps its source_set-override branch (now a no-op since `parse_jar_entries` sets Library up front — but it stays as a guard for other callers).
- The old `handle.join().unwrap()` is gone (no-`unwrap` rule); panics now degrade to partial results + `complete: false` + error log instead of a process abort.
- Check the existing imports in jar.rs: `Url`, `Arc`, `SourceSet` are already imported; `FileContributions` may be referenced via `crate::indexer::FileContributions` (as the existing `apply_contribution_to_index` signature does).

- [ ] **Step 2: Verify behavior unchanged**

Run: `cargo test --quiet jar 2>&1 | rg "test result" | rg -v " 0 failed" ; echo "exit=$?"`
Expected: `exit=1` (no failures anywhere).

- [ ] **Step 3: Lint + format + commit**

```bash
cargo clippy --quiet -- -D warnings && cargo fmt
git add src/indexer/jar.rs
git commit -m "refactor(jar): split index_jar_entries into parse and contribution-apply phases"
```

---

### Task 4: Wire the cache into `index_sources_jars`

**Files:**
- Modify: `src/indexer/jar.rs` (`index_sources_jars`, ~line 143)
- Modify: `src/workspace/scan_handler.rs` (~line 355)
- Modify: `src/indexer/jar_tests.rs` (ONE existing call site at ~line 602 + new tests)

- [ ] **Step 1: Write the failing tests**

Append to `src/indexer/jar_tests.rs`:

```rust
// ============================================================================
// Sources-JAR parse cache integration
// ============================================================================

/// First run writes the cache file; the entry fingerprint matches the JAR.
#[test]
fn index_sources_jars_writes_parse_cache() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let jar_path = write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "cached",
        "1.0.0",
        &[(
            "com/example/cached/CachedCore.kt",
            "package com.example.cached\n\nclass CachedCore\n",
        )],
    );

    let indexer = idx();
    let total = crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );
    assert!(total > 0, "first run should parse and index");

    let cache = crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    let entry = cache
        .get(jar_path.to_string_lossy().as_ref())
        .expect("cache entry written for the JAR");
    assert!(!entry.files.is_empty(), "entry holds parsed files");
    assert_eq!(
        entry.files[0].file_data.source_set,
        crate::types::SourceSet::Library,
        "cached file data is pre-marked Library"
    );
    let current = crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path)
        .expect("fingerprint");
    assert!(
        crate::indexer::sources_jar_cache::entry_is_fresh(entry, &current),
        "entry fingerprint matches the JAR on disk"
    );
}

/// A fresh cache entry is served WITHOUT touching the JAR contents: the file
/// on disk is garbage (not a ZIP), so any symbols in the index must have come
/// from the cache.
#[test]
fn index_sources_jars_serves_fresh_entry_from_cache_without_extraction() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");

    // Lay out a Gradle-cache-shaped path with a GARBAGE jar (unzippable).
    let jar_dir = gradle_dir
        .path()
        .join("caches/modules-2/files-2.1/com.example/garbage/1.0.0/abc123");
    std::fs::create_dir_all(&jar_dir).expect("mkdir");
    let jar_path = jar_dir.join("garbage-1.0.0-sources.jar");
    std::fs::write(&jar_path, b"definitely not a zip archive").expect("write garbage jar");

    // Pre-populate the cache with a fingerprint-fresh entry for it.
    let uri_text = format!(
        "jar:file://{}!/com/example/garbage/FromCache.kt",
        jar_path.display()
    );
    let uri = tower_lsp::lsp_types::Url::parse(&uri_text).expect("uri");
    let mut parsed = crate::indexer::Indexer::parse_file(
        &uri,
        "package com.example.garbage\n\nclass FromCache\n",
    );
    parsed.data.source_set = crate::types::SourceSet::Library;
    let fingerprint =
        crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path).expect("fingerprint");
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: fingerprint.mtime_secs,
            mtime_nanos: fingerprint.mtime_nanos,
            file_size: fingerprint.file_size,
            files: vec![crate::indexer::sources_jar_cache::SourcesFileEntry {
                uri: uri_text.clone(),
                content_hash: parsed.content_hash,
                file_data: std::sync::Arc::new(parsed.data),
            }],
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    let total = crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    assert!(total > 0, "cache hit should contribute symbols");
    assert!(
        indexer.definitions.get("FromCache").is_some(),
        "FromCache must be resolvable purely from the cache (the JAR is garbage)"
    );
    assert!(
        indexer.library_uris.contains(&uri_text),
        "cache-hit files are registered as library URIs"
    );
}

/// A stale entry (fingerprint mismatch) is re-parsed from the real JAR and
/// the cache is updated with the new content.
#[test]
fn index_sources_jars_stale_entry_reparses_and_updates_cache() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let jar_path = write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "staleness",
        "1.0.0",
        &[(
            "com/example/staleness/NewSymbol.kt",
            "package com.example.staleness\n\nclass NewSymbol\n",
        )],
    );

    // Cache entry with a deliberately wrong fingerprint holding an old symbol.
    let stale_uri_text = format!(
        "jar:file://{}!/com/example/staleness/OldSymbol.kt",
        jar_path.display()
    );
    let stale_uri = tower_lsp::lsp_types::Url::parse(&stale_uri_text).expect("uri");
    let stale_parsed = crate::indexer::Indexer::parse_file(
        &stale_uri,
        "package com.example.staleness\n\nclass OldSymbol\n",
    );
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: 1,
            mtime_nanos: 2,
            file_size: 3,
            files: vec![crate::indexer::sources_jar_cache::SourcesFileEntry {
                uri: stale_uri_text,
                content_hash: stale_parsed.content_hash,
                file_data: std::sync::Arc::new(stale_parsed.data),
            }],
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    assert!(
        indexer.definitions.get("NewSymbol").is_some(),
        "stale entry must be re-parsed from the real JAR"
    );
    assert!(
        indexer.definitions.get("OldSymbol").is_none(),
        "stale cached symbols must not leak into the index"
    );

    let reloaded =
        crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    let entry = reloaded
        .get(jar_path.to_string_lossy().as_ref())
        .expect("cache entry refreshed");
    let current =
        crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path).expect("fingerprint");
    assert!(
        crate::indexer::sources_jar_cache::entry_is_fresh(entry, &current),
        "refreshed entry matches the real JAR"
    );
}

/// Entries for JARs that no longer exist are pruned from the cache on save.
#[test]
fn index_sources_jars_prunes_deleted_jar_entries() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "alive",
        "1.0.0",
        &[(
            "com/example/alive/Alive.kt",
            "package com.example.alive\n\nclass Alive\n",
        )],
    );

    // Seed the cache with an entry for a JAR path that does not exist.
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        "/nonexistent/path/gone-0.1-sources.jar".to_owned(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: 1,
            mtime_nanos: 2,
            file_size: 3,
            files: Vec::new(),
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    let reloaded =
        crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    assert!(
        !reloaded.contains_key("/nonexistent/path/gone-0.1-sources.jar"),
        "entry for the deleted JAR is pruned"
    );
    assert_eq!(reloaded.len(), 1, "only the live JAR remains");
}
```

Also update the ONE existing `index_sources_jars` call site at `jar_tests.rs:602` (test `index_sources_jars_end_to_end_with_real_jar`) from 2-arg to 3-arg with an isolated cache dir:

```rust
let cache_dir = tempfile::tempdir().expect("cache dir");
let total = crate::indexer::jar::index_sources_jars(
    &indexer,
    Some(tmpdir.path()),
    Some(cache_dir.path()),
);
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --quiet jar 2>&1 | head -20`
Expected: compile FAILURE — `index_sources_jars` takes 2 arguments.

- [ ] **Step 3: Implement the cache-aware `index_sources_jars`**

Replace `index_sources_jars` in `src/indexer/jar.rs`:

```rust
/// Index *-sources.jar files from the Gradle cache by unpacking them
/// in-memory and parsing each `.kt` / `.java` entry with tree-sitter.
///
/// Results go into the main `files` / `definitions` / `qualified` maps
/// (via the shared apply path), marked `SourceSet::Library`, so they are
/// visible to go-to-definition / hover / completion.
///
/// Parse results are cached to disk per JAR (`sources_jar_cache`), keyed by
/// `(mtime, size)`.  Unchanged JARs skip extraction AND parsing on subsequent
/// startups — the dominant startup cost (see docs/startup-speed-plan.md).
///
/// `cache_dir` overrides the parse-cache location (tests); production passes
/// `None` for the default `~/.cache/kmp-lsp/`.
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

    let (cache_hits, cached_contributions, missed) = partition_sources_jars(&cache, &sources);
    // Save whenever pruning changed the map or any JAR was (re-)parsed.  A
    // miss whose extraction fails skips its insert, making the save a no-op
    // for it — harmless.
    let cache_dirty = pruned || !missed.is_empty();

    let mut contributions = cached_contributions;
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
/// built from the cached `Arc<FileData>` — no deep clones) and misses (JAR
/// path + pre-captured fingerprint, to be extracted + parsed).
/// Returns `(hit_count, cached_contributions, missed)`.
fn partition_sources_jars(
    cache: &std::collections::HashMap<String, super::sources_jar_cache::SourcesJarEntry>,
    sources: &[PathBuf],
) -> (
    usize,
    Vec<crate::indexer::FileContributions>,
    Vec<(PathBuf, super::sources_jar_cache::JarFingerprint)>,
) {
    let mut cache_hits = 0usize;
    let mut cached_contributions: Vec<crate::indexer::FileContributions> = Vec::new();
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
/// URI-string reverse-mapping, which breaks under URL percent-encoding);
/// parsing is parallel across each JAR's files.
///
/// A JAR is NOT cached when its extraction fails (transient unreadability
/// must not hide its symbols until the next mtime change) or when a parse
/// thread panicked (partial entry behind an immutable fingerprint would hide
/// the missing files forever).  Empty entries for JARs with zero parseable
/// files ARE cached — re-extracting them every startup is pure waste.
fn parse_missed_sources_jars(
    cache: &mut std::collections::HashMap<String, super::sources_jar_cache::SourcesJarEntry>,
    missed: Vec<(PathBuf, super::sources_jar_cache::JarFingerprint)>,
) -> Vec<crate::indexer::FileContributions> {
    let mut all_contributions: Vec<crate::indexer::FileContributions> = Vec::new();

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
                .map(|result| super::sources_jar_cache::SourcesFileEntry {
                    uri: result.uri.to_string(),
                    content_hash: result.content_hash,
                    file_data: Arc::new(result.data.clone()),
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
```

Update the production call site in `src/workspace/scan_handler.rs` (~line 355):

```rust
let sources_total = crate::indexer::jar::index_sources_jars(&indexer, None, None);
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --quiet jar 2>&1 | rg "test result" | rg -v " 0 failed" ; echo "exit=$?"`
Expected: `exit=1` — zero failures (4 new tests + all existing).

- [ ] **Step 5: Full suite + lint + format + commit**

```bash
cargo test --quiet 2>&1 | rg "test result" | rg -v " 0 failed" ; cargo clippy --quiet -- -D warnings && cargo fmt
git add src/indexer/jar.rs src/indexer/jar_tests.rs src/workspace/scan_handler.rs
git commit -m "feat(sources-jar): parse cache — skip re-parsing unchanged sources JARs on startup"
```

---

### Task 5: Remove the orphaned lazy experiment + measure

**Files:**
- Delete: `src/indexer/sources_cache.rs` (untracked orphan; backup exists in `debug-scripts/lazy-experiment-backup/sources_cache.rs`)

- [ ] **Step 1: Delete the orphan**

```bash
rm src/indexer/sources_cache.rs
```

(It is not in the module tree, so no code changes needed. It is untracked, so no `git rm`.)

- [ ] **Step 2: Full verification**

```bash
cargo test --quiet 2>&1 | rg "test result" | rg -v " 0 failed" ; echo "exit=$?"
cargo clippy --quiet -- -D warnings && echo CLIPPY CLEAN
```

Expected: `exit=1` and `CLIPPY CLEAN`.

- [ ] **Step 3: Measure (manual, real Gradle cache)**

```bash
cargo build --release
rm -f ~/.cache/kmp-lsp/sources-jar-*.bin
time ./target/release/kmp-lsp index .   # cold: writes cache (expect ≈ old timing + cache write)
time ./target/release/kmp-lsp index .   # warm: expect sources phase ~3-5s (decode + Arc apply)
```

Record both timings in the final report. If the warm run does not improve materially, STOP and investigate before committing anything further.

---

## Risks / Known Trade-offs

1. **Cache size:** `FileData.lines` holds full source text, so the cache file will be large (likely hundreds of MB for 41k files — roughly the in-memory footprint the index already pays). Pruning (deleted JARs + stale version files) bounds growth. If cold-run serialize or warm-run decode disappoints, zstd-compress the stream — YAGNI until Task 5 measures.
2. **bincode 1.x is positional:** mis-decodes of same-shaped reordered fields are silent. Mitigated structurally: the filename embeds `cache::CACHE_VERSION`, which the existing project rule bumps on every `FileData`/`SymbolEntry` change.
3. **Multi-process race:** two kmp-lsp instances save last-writer-wins; the loser re-parses next start. Documented in the module; same trade-off as `jar_cache.rs`.
4. **Version-switch stale URIs:** when a library version changes, old entry URIs differ from new ones, so `remove_stale_for_uri` won't see them — identical to the existing eager path's behavior (old version's JAR drops out of the scan; entries evicted on next `reset_index_state`). Not a regression; out of scope.

## Out of Scope (later phases of docs/startup-speed-plan.md)

- Phase 2 (parallel ZIP extraction — note Task 4 serializes extraction per JAR; ZIP read was ~3.5s/20% in the baseline and is now paid only on cache misses), Phase 3 (skip private symbols — changes resolver behavior, needs own tests), Phase 4 (skip trivial files).
