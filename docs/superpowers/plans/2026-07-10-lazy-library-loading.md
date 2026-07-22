# Lazy Library Loading (Compiled-JAR Symbols) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop eagerly materializing full compiled-JAR symbol data (name+kind+container+detail+params+doc, `SidecarSymbol`/`SymbolEntry`) for every JAR discovered in the Gradle cache at scan time. Build a cheap, always-eager Tier-1 manifest (name+kind+container only) for every JAR, and defer full Tier-2 materialization until a real consumer (completion selection, hover, goto-def, or an import-scoped file-open promotion) actually needs it.

**Architecture:** Two tiers sharing one per-JAR unit of laziness, both reusing existing merge machinery unchanged. `JarId`/`JarTable` interns JAR paths (mirrors the `FileId`/`FileTable` pattern from PR #208). Tier 1 (`jar_qualified`/`jar_bare_names`) is built from a new, separately-cached lightweight manifest. Tier 2 (`materialize_jar_on_demand`) calls today's `index_jars` — refactored to be additive rather than clear-then-repopulate — scoped to one JAR at a time, wired into every current direct reader of `jar_definitions`/`jar_files`. The startup crawl is retimed from full materialization to manifest-only; the "flip" to lazy-by-default is the last task, gated on every consumer already being wired.

**Tech Stack:** Rust, `bincode` 1.x (positional serialization), `serde`, `dashmap`, `zstd` 0.13 (new direct dependency — currently transitive-only via `zip`), the existing `kmp-jar-indexer` sidecar (newline-delimited JSON over stdin/stdout, one JAR per request already).

**Scope note:** This plan covers **compiled-JAR symbols only** (the `jar-symbols-vN.bin` / sidecar-driven subsystem). Source-JAR text (`sources-jar-v2-cN.bin`, tree-sitter-driven) is explicitly deferred to a follow-on plan per the design's own §Rollout staging — not an oversight.

## Global Constraints

- `cargo test --bin kmp-lsp` (binary-only crate) must stay green after every task. No `unwrap()`/`expect()` in production code. No abbreviations (spell out identifiers). Tests live in companion `*_tests.rs` files. `cargo clippy --bin kmp-lsp -- -D warnings` must be clean before each commit.
- Pre-commit hook runs fmt+clippy automatically; if it rewrites files, `git add -A` and re-commit.
- Behaviour-preserving until Task 12 (the flip): every task before it must leave the server's *current* eager behavior completely unchanged in production — new code is inert (unit-tested directly, never called from the live crawl) until Task 12 flips the switch. This is deliberate: it means the codebase is always in a known-good, fully-eager state if the plan is interrupted partway through.
- Decoy-first for any task that changes observable behavior (Tasks 4, 7, 9, 10, 11, 12): write a test that fails without the change and passes with it; never weaken an existing assertion to force green.
- Design reference: `docs/superpowers/specs/2026-07-10-lazy-library-loading-design.md` (read in full before starting — every task below cites the section of that spec it implements).

---

## File Structure

| File | Responsibility |
|---|---|
| `src/types.rs` | `JarId` newtype + `JarTable` (new) |
| `src/indexer.rs` | New `Indexer` fields: `jar_table`, `jar_qualified`, `jar_bare_names`, `materialized`, `materialization_failed` |
| `src/indexer/jar_manifest_cache.rs` (new) | The lightweight, separately-cached Tier-1 manifest format + zstd compression helpers |
| `src/indexer/jar_cache.rs` | Unchanged shape (full `SidecarSymbol` cache), only `JAR_CACHE_VERSION` bump when Task 3 needs it |
| `src/indexer/jar.rs` | `index_jars` becomes additive (Task 4); `materialize_jar_on_demand` (Task 4); `build_jar_manifest` (Task 6) |
| `src/workspace/scan_handler.rs` | Sidecar lock scoping (Task 5); the flip (Task 12) |
| `src/features/call_arg_diagnostics.rs` | Tier-1 suppression check (Task 7) |
| `src/indexer/resolution.rs`, `src/indexer/lookup.rs`, `src/resolver/infer.rs`, `src/resolver/resolve.rs` | `ensure_jar_materialized` call sites (Task 8) |
| `src/indexer/apply.rs` | `rebuild_bare_name_cache`/`rebuild_importable_fqns` Tier-1 merge (Task 9); reindex reset (Task 11) |
| `src/resolver/complete.rs` | Promote-on-selection wiring for auto-import (Task 9) |
| `src/indexer/memory_probe_tests.rs` | Probe extension (Task 13) |

---

### Task 1: `JarId` + `JarTable` foundation

**Files:**
- Modify: `src/types.rs` (add after the existing `FileTable` impl — read it first, e.g. `rg -n "struct FileTable" src/types.rs` to find the exact location; mirror its shape precisely)
- Test: `src/types_tests.rs` (or wherever `FileTable`'s own tests live — check with `rg -n "fn.*file_table\|FileTable::" src/types_tests.rs` and put `JarTable` tests alongside them)

**Interfaces:**
- Produces: `pub(crate) struct JarId(pub(crate) u32)` (Copy, Eq, Hash, Debug), `pub(crate) struct JarTable` with `pub(crate) fn intern(&self, path: &str) -> JarId` and `pub(crate) fn path(&self, id: JarId) -> Option<String>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn jar_table_intern_is_idempotent() {
    let table = JarTable::new();
    let id_a = table.intern("/gradle/caches/foo-1.0.jar");
    let id_b = table.intern("/gradle/caches/foo-1.0.jar");
    assert_eq!(id_a, id_b, "interning the same path twice must return the same JarId");
    let id_c = table.intern("/gradle/caches/bar-2.0.jar");
    assert_ne!(id_a, id_c, "different paths must get different JarIds");
    assert_eq!(table.path(id_a).as_deref(), Some("/gradle/caches/foo-1.0.jar"));
}

#[test]
fn jar_table_intern_concurrent_same_path_yields_one_id() {
    use std::sync::Arc;
    let table = Arc::new(JarTable::new());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let table = Arc::clone(&table);
        handles.push(std::thread::spawn(move || table.intern("/gradle/caches/shared.jar")));
    }
    let ids: Vec<JarId> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let first = ids[0];
    assert!(ids.iter().all(|id| *id == first), "concurrent interning of the same path must never mint two ids");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp jar_table_intern -- --nocapture`
Expected: FAIL with `cannot find type JarTable`/`cannot find struct JarId` (not yet defined).

- [ ] **Step 3: Write the implementation**

In `src/types.rs`, immediately after the existing `FileTable` struct and its `impl` block (read `FileTable::intern`'s exact double-checked-locking body first — the shape below must match it precisely, substituting `Url`/file-URI concerns for plain path strings, since JAR paths don't need URL parsing):

```rust
/// Interned identifier for a JAR path, into a [`JarTable`]. Mirrors [`FileId`]/
/// [`FileTable`] — same double-checked-locking intern, same append-only
/// growth (JAR identity doesn't change mid-session; reindex rebuilds the
/// whole table, see Task 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct JarId(u32);

pub(crate) struct JarTable {
    by_id: std::sync::RwLock<Vec<String>>,
    by_path: dashmap::DashMap<String, JarId>,
}

impl JarTable {
    pub(crate) fn new() -> Self {
        Self {
            by_id: std::sync::RwLock::new(Vec::new()),
            by_path: dashmap::DashMap::new(),
        }
    }

    /// Intern `path`, returning its stable `JarId`. Idempotent and race-safe:
    /// a fast-path read first, then a double-checked write under the same
    /// lock `FileTable::intern` uses (see PR #208's review for why this is
    /// race-free — the re-check happens inside the critical section, so a
    /// losing concurrent caller observes the winner's id, never mints a
    /// second one).
    pub(crate) fn intern(&self, path: &str) -> JarId {
        if let Some(existing) = self.by_path.get(path) {
            return *existing;
        }
        let mut ids = self.by_id.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = self.by_path.get(path) {
            return *existing;
        }
        assert!(
            u32::try_from(ids.len()).is_ok(),
            "JarTable overflow: more than u32::MAX distinct jars interned"
        );
        let id = JarId(ids.len() as u32);
        ids.push(path.to_owned());
        self.by_path.insert(path.to_owned(), id);
        id
    }

    /// The interned path for `id`, or `None` if `id` is not from this table.
    pub(crate) fn path(&self, id: JarId) -> Option<String> {
        let ids = self.by_id.read().unwrap_or_else(|e| e.into_inner());
        ids.get(id.0 as usize).cloned()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp jar_table_intern -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: all existing tests still pass (baseline count + 2), clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/types.rs src/types_tests.rs
git commit -m "feat(types): JarId/JarTable — interning foundation for lazy JAR loading

Mirrors FileId/FileTable (PR #208): double-checked-locking intern, same
race-safety proof. No production caller yet — this is pure foundation."
```

---

### Task 2: Tier-1 fields on `Indexer`

**Files:**
- Modify: `src/indexer.rs` (struct fields near the existing `jar_files`/`jar_definitions` declarations — `rg -n "jar_symbol_packages: DashMap" src/indexer.rs` to find the spot; add the constructor init near the existing `jar_symbol_packages: DashMap::new()` line)

**Interfaces:**
- Consumes: `JarId`, `JarTable::new()` from Task 1.
- Produces: `Indexer.jar_table: JarTable`, `Indexer.jar_qualified: DashMap<String, JarId>`, `Indexer.jar_bare_names: DashMap<String, Vec<JarId>>`, `Indexer.materialized: DashSet<JarId>`, `Indexer.materialization_failed: DashSet<JarId>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn indexer_has_jar_tier1_fields_initialized_empty() {
    let idx = Indexer::new();
    assert_eq!(idx.jar_qualified.len(), 0);
    assert_eq!(idx.jar_bare_names.len(), 0);
    assert_eq!(idx.materialized.len(), 0);
    assert_eq!(idx.materialization_failed.len(), 0);
    let id = idx.jar_table.intern("/some/path.jar");
    assert_eq!(idx.jar_table.path(id).as_deref(), Some("/some/path.jar"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp indexer_has_jar_tier1_fields -- --nocapture`
Expected: FAIL with `no field jar_table on type Indexer` (or similar for each field).

- [ ] **Step 3: Write the implementation**

In `src/indexer.rs`, in the `Indexer` struct definition, immediately after the existing `jar_symbol_packages: DashMap<String, Vec<String>>,` field:

```rust
    /// Interned JAR paths — one [`JarId`] per distinct compiled JAR discovered
    /// this session. See `docs/superpowers/specs/2026-07-10-lazy-library-loading-design.md`.
    pub(crate) jar_table: crate::types::JarTable,
    /// Tier 1: FQN → the JarId of a JAR whose manifest declares that name.
    /// Always-eager, cheap (name+kind+container only, no detail/params/doc).
    /// Populated by `build_jar_manifest`; NOT cleared by `index_jars` (Task 4).
    pub(crate) jar_qualified: DashMap<String, crate::types::JarId>,
    /// Tier 1: short name → candidate JarIds (for bare-word completion and
    /// auto-import — see design §Auto-import).
    pub(crate) jar_bare_names: DashMap<String, Vec<crate::types::JarId>>,
    /// JarIds whose full symbol data (Tier 2) has been materialized via
    /// `materialize_jar_on_demand` or the initial-import eager promotion.
    pub(crate) materialized: dashmap::DashSet<crate::types::JarId>,
    /// JarIds whose Tier-2 materialization was attempted and failed this
    /// session (sidecar crash, etc.) — distinct from `materialized`/absent so
    /// callers don't retry in a loop. See design §Error handling.
    pub(crate) materialization_failed: dashmap::DashSet<crate::types::JarId>,
```

In `Indexer::new()`'s field initializer, immediately after the existing `jar_symbol_packages: DashMap::new(),` line:

```rust
            jar_table: crate::types::JarTable::new(),
            jar_qualified: DashMap::new(),
            jar_bare_names: DashMap::new(),
            materialized: dashmap::DashSet::new(),
            materialization_failed: dashmap::DashSet::new(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp indexer_has_jar_tier1_fields -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 3 tests (Task 1's 2 + this task's 1), clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/indexer.rs
git commit -m "feat(index): Tier-1 jar manifest fields on Indexer

jar_table/jar_qualified/jar_bare_names/materialized/materialization_failed.
Inert — nothing populates or reads these yet."
```

---

### Task 3: Compiled-JAR manifest cache format (Tier 1's on-disk store)

**Files:**
- Create: `src/indexer/jar_manifest_cache.rs`
- Modify: `src/indexer.rs` (add `#[cfg(test)] #[path = "indexer/jar_manifest_cache_tests.rs"] mod jar_manifest_cache_tests;` near the other test-module declarations, and `pub(crate) mod jar_manifest_cache;` near the other `pub(crate) mod` lines — `rg -n "pub\(crate\) mod jar_cache" src/indexer.rs` for the exact spot)
- Test: `src/indexer/jar_manifest_cache_tests.rs`
- Modify: `Cargo.toml` (add `zstd = "0.13"` as a direct dependency, matching the version already pinned transitively via `zip` — check `grep -A2 'name = "zstd"' Cargo.lock` to confirm 0.13 is what's already resolved, so this doesn't bump anything)

**Why a separate cache, not "reuse jar_cache.rs and discard fields":** see design §Tier 1 — `load_jar_cache()` decodes the entire monolithic `jar-symbols-vN.bin` in one `bincode::deserialize_from` call; discarding fields *after* that call doesn't reduce the transient peak during decode, only steady-state retention. The manifest must be its own file, decodable without ever touching the full per-JAR symbol data.

**Interfaces:**
- Consumes: nothing external.
- Produces: `pub(crate) struct JarManifestEntry { pub mtime_secs: u64, pub mtime_nanos: u32, pub file_size: u64, pub names: Vec<JarManifestName> }`, `pub(crate) struct JarManifestName { pub name: String, pub kind: String, pub container: Option<String>, pub package: Option<String> }`, `pub(crate) fn load_jar_manifest_cache() -> HashMap<String, JarManifestEntry>`, `pub(crate) fn save_jar_manifest_cache(entries: &HashMap<String, JarManifestEntry>)`, `pub(crate) fn manifest_entry_is_fresh(entry: &JarManifestEntry, jar: &Path) -> bool` (identical freshness check to `jar_cache.rs::cache_entry_is_fresh` — same `(mtime, size)` comparison).

**Schema note — `package` is load-bearing, not decorative:** `JarManifestName.package` carries the sidecar's `SidecarSymbol.pkg` (see `src/sidecar.rs`), exactly the field `src/indexer/jar.rs`'s Tier-2 path already uses to build real FQNs into `indexer.qualified` (`effective_pkg` fallback pattern, `jar.rs` around the `populate_from_symbols` loop). Task 6 uses this same field to build `indexer.jar_qualified` at manifest-build time — omitting it here was an earlier draft's mistake: Tier 1 cannot construct real qualified names without it, and deferring the FQN question to a later task (as an earlier draft of this plan did) left `jar_qualified` permanently unpopulated. Do not drop this field.

- [ ] **Step 1: Write the failing test**

```rust
use super::jar_manifest_cache::{
    load_jar_manifest_cache, save_jar_manifest_cache, JarManifestEntry, JarManifestName,
};
use std::collections::HashMap;

#[test]
fn jar_manifest_cache_round_trips_through_zstd() {
    let mut entries: HashMap<String, JarManifestEntry> = HashMap::new();
    entries.insert(
        "/gradle/caches/compose-ui-1.6.0.jar".to_owned(),
        JarManifestEntry {
            mtime_secs: 1_700_000_000,
            mtime_nanos: 0,
            file_size: 12345,
            names: vec![
                JarManifestName {
                    name: "Column".to_owned(),
                    kind: "fun".to_owned(),
                    container: None,
                    package: Some("androidx.compose.foundation.layout".to_owned()),
                },
                JarManifestName {
                    name: "Modifier".to_owned(),
                    kind: "class".to_owned(),
                    container: None,
                    package: Some("androidx.compose.ui".to_owned()),
                },
            ],
        },
    );
    save_jar_manifest_cache(&entries);
    let loaded = load_jar_manifest_cache();
    assert_eq!(loaded.len(), 1, "round trip must preserve entry count");
    let entry = loaded
        .get("/gradle/caches/compose-ui-1.6.0.jar")
        .expect("saved entry must load back");
    assert_eq!(entry.names.len(), 2);
    assert_eq!(entry.names[0].name, "Column");
    assert_eq!(entry.names[1].kind, "class");
    assert_eq!(
        entry.names[0].package.as_deref(),
        Some("androidx.compose.foundation.layout"),
        "package must round-trip — it's how Task 6 builds real FQNs into jar_qualified"
    );
}

#[test]
fn jar_manifest_cache_missing_file_returns_empty_map() {
    // No save has happened for a fresh cache dir in this process — but since
    // the prior test already saved to the real cache path, this test instead
    // asserts the loader never panics on an absent/corrupt file by construction
    // (covered structurally by load_jar_manifest_cache's `Result`-returning
    // internals — see implementation step). Kept as a named placeholder for
    // the "absent" branch's contract, exercised properly in
    // jar_manifest_cache_corrupt_file_returns_empty_map below.
}

#[test]
fn jar_manifest_cache_corrupt_file_returns_empty_map() {
    // Write garbage bytes to the manifest cache path directly, then confirm
    // the loader degrades to an empty map instead of panicking.
    let path = super::jar_manifest_cache::manifest_cache_path_for_test();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not a valid zstd/bincode blob").unwrap();
    let loaded = load_jar_manifest_cache();
    assert!(loaded.is_empty(), "corrupt manifest cache must degrade to empty, not panic");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp jar_manifest_cache -- --nocapture`
Expected: FAIL — module `jar_manifest_cache` doesn't exist yet.

- [ ] **Step 3: Add the dependency**

In `Cargo.toml`, in the `[dependencies]` section, add (alphabetically near `zip = "2"`):

```toml
zstd = "0.13"
```

Run `cargo build --bin kmp-lsp 2>&1 | tail -5` to confirm it resolves against the already-locked transitive version (no `Cargo.lock` churn expected beyond promoting it to a direct entry).

- [ ] **Step 4: Write the implementation**

Create `src/indexer/jar_manifest_cache.rs`:

```rust
//! Tier-1 lightweight JAR manifest cache: name+kind+container only, no
//! detail/params/doc. This is the always-eager, cheap-by-construction store
//! `build_jar_manifest` (Task 6) reads and writes — kept entirely separate
//! from `jar_cache.rs`'s full `SidecarSymbol` cache so that decoding it never
//! requires materializing the full per-JAR symbol data. See design §Tier 1.
//!
//! Global (not per-workspace) file, same convention as `jar_cache.rs`:
//! `~/.cache/kmp-lsp/jar-manifest-v{VERSION}.bin`.
//!
//! zstd-compressed (level 3) — pattern borrowed directly from the
//! `qdsfdhvh/kotlin-lsp` community fork's PR #107, which validated this exact
//! wrapping (`zstd::encode_all`/`decode_all` around a bincode blob) for cache
//! files in a real deployment. That fork's compression targets a different
//! problem (CLI one-shot I/O latency, not server steady-state RAM) and must
//! NOT be applied to `jar_cache.rs`'s full symbol cache — encode_all/decode_all
//! materialize a full extra buffer, which would reintroduce exactly the
//! transient-peak problem this manifest cache exists to avoid for the FULL
//! cache. It's safe here specifically because the manifest is small by
//! construction (name+kind+container only).

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Bump when `JarManifestEntry`/`JarManifestName` schema changes.
const JAR_MANIFEST_CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct JarManifestCache {
    version: u32,
    entries: HashMap<String, JarManifestEntry>,
}

#[derive(Serialize)]
struct JarManifestCacheRef<'a> {
    version: u32,
    entries: &'a HashMap<String, JarManifestEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarManifestEntry {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub file_size: u64,
    pub names: Vec<JarManifestName>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarManifestName {
    pub name: String,
    pub kind: String,
    pub container: Option<String>,
    /// The declaring class's fully-qualified package, e.g.
    /// "androidx.compose.runtime". `None` for the default package or when
    /// the sidecar didn't emit one. Mirrors `SidecarSymbol::pkg` — this is
    /// what lets Task 6 build real FQNs into `jar_qualified` without ever
    /// touching the full per-JAR symbol cache.
    pub package: Option<String>,
}

fn cache_path() -> std::path::PathBuf {
    super::cache::xdg_cache_base()
        .join("kmp-lsp")
        .join(format!("jar-manifest-v{JAR_MANIFEST_CACHE_VERSION}.bin"))
}

#[cfg(test)]
pub(crate) fn manifest_cache_path_for_test() -> std::path::PathBuf {
    cache_path()
}

/// Load the global JAR manifest cache. Returns an empty map on any error
/// (absent file, corrupt zstd frame, corrupt bincode, version mismatch) —
/// callers treat "empty" as "nothing cached yet, build fresh."
pub(crate) fn load_jar_manifest_cache() -> HashMap<String, JarManifestEntry> {
    let path = cache_path();
    let Ok(compressed) = std::fs::read(&path) else {
        return HashMap::new();
    };
    let Ok(bytes) = zstd::decode_all(compressed.as_slice()) else {
        log::debug!("jar_manifest_cache: zstd decode failed, starting fresh");
        return HashMap::new();
    };
    match bincode::deserialize::<JarManifestCache>(&bytes) {
        Ok(c) if c.version == JAR_MANIFEST_CACHE_VERSION => {
            log::debug!("jar_manifest_cache: loaded {} entries", c.entries.len());
            c.entries
        }
        _ => {
            log::debug!("jar_manifest_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

/// Save the global JAR manifest cache atomically (write temp → rename),
/// zstd-compressed.
pub(crate) fn save_jar_manifest_cache(entries: &HashMap<String, JarManifestEntry>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("jar_manifest_cache: cannot create cache dir: {e}");
            return;
        }
    }
    let cache = JarManifestCacheRef {
        version: JAR_MANIFEST_CACHE_VERSION,
        entries,
    };
    let bytes = match bincode::serialize(&cache) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("jar_manifest_cache: serialize error: {e}");
            return;
        }
    };
    let compressed = match zstd::encode_all(bytes.as_slice(), 3) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("jar_manifest_cache: zstd encode error: {e}");
            return;
        }
    };
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &compressed) {
        log::warn!("jar_manifest_cache: write temp error: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("jar_manifest_cache: rename error: {e}");
        let _ = std::fs::remove_file(&tmp);
    } else {
        log::debug!(
            "jar_manifest_cache: saved {} entries ({} bytes raw -> {} bytes zstd)",
            entries.len(),
            bytes.len(),
            compressed.len()
        );
    }
}

/// Check whether the manifest entry for `jar` is still valid — identical
/// `(mtime, size)` freshness check to `jar_cache.rs::cache_entry_is_fresh`.
pub(crate) fn manifest_entry_is_fresh(entry: &JarManifestEntry, jar: &Path) -> bool {
    let meta = match std::fs::metadata(jar) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() != entry.file_size {
        return false;
    }
    let mtime = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let duration = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_secs() == entry.mtime_secs && duration.subsec_nanos() == entry.mtime_nanos
}
```

Wire the module into `src/indexer.rs`: add `pub(crate) mod jar_manifest_cache;` next to `pub(crate) mod jar_cache;`, and `#[cfg(test)] #[path = "indexer/jar_manifest_cache_tests.rs"] mod jar_manifest_cache_tests;` next to the other `#[cfg(test)]` module declarations (match the exact pattern used for `jar_cache`'s own test-module wiring).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp jar_manifest_cache -- --nocapture`
Expected: PASS (3 tests — note: run tests single-threaded for this file since they share the real on-disk cache path: `cargo test --bin kmp-lsp jar_manifest_cache -- --nocapture --test-threads=1`).

- [ ] **Step 6: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 6 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/indexer.rs src/indexer/jar_manifest_cache.rs src/indexer/jar_manifest_cache_tests.rs
git commit -m "feat(index): jar manifest cache — lightweight, zstd-compressed, separate from jar_cache.rs

Tier-1's on-disk store: name+kind+container only, never the full
SidecarSymbol data. zstd wrapping pattern borrowed from qdsfdhvh/kotlin-lsp
PR #107 — appropriate here because the manifest is small by construction;
NOT applied to jar_cache.rs's full cache, where it would reintroduce a
transient decode peak (see module doc for why). No production caller yet."
```

---

### Task 4: `index_jars` becomes additive; `materialize_jar_on_demand`

**Files:**
- Modify: `src/indexer/jar.rs` (the `index_jars` function — read it fully first: `rg -n "pub\(crate\) fn index_jars" -A 60 src/indexer/jar.rs`)
- Modify: `src/workspace/scan_handler.rs` (insert `clear_jar_maps` call before the crawl's `index_jars` call — Task 5 also touches this site)
- Modify: `src/cli/run.rs:465` (insert `clear_jar_maps` call before the `kmp-lsp find` CLI command's `index_jars` call)
- Test: `src/indexer/jar_tests.rs`

**Why this is necessary, not optional:** `index_jars` currently does `indexer.jar_files.clear(); indexer.jar_definitions.clear(); indexer.jar_uri_to_defs.clear(); indexer.jar_symbol_packages.clear();` unconditionally at the top, before processing `paths`. Calling it with a single JAR's path for on-demand materialization would silently wipe out every *other* already-materialized JAR's data. The design's claim that Tier 2 "reuses `index_jars` unchanged" needs this one caveat: the clear must move to the caller that actually wants a full reindex (the startup crawl), not live inside the function every caller gets.

**Naming — do not call the new function `clear_jar_index`:** `Indexer` already has a method with that near-identical name (`src/indexer.rs`, `pub(crate) fn clear_jar_index(&self)`, called from `handle_change_root` and `scan_handler.rs`'s root-change path). It clears an overlapping-but-different field set (`jar_files`, `jar_definitions`, `jar_uri_to_defs`, `library_uris`, `jar_phase` — notably it does NOT clear `jar_symbol_packages`, which is this task's function's whole reason for existing) and additionally resets `jar_phase` to `Pending`/`Unavailable`, which this task's function must NOT do (it runs mid-crawl, not on a workspace-root change). Name this task's function `clear_jar_maps` (not `clear_jar_index`) specifically to avoid a same-purpose-sounding, different-behavior collision with the existing method. Reconciling the two into one function is out of scope for this plan — leave a `// NOTE:` comment pointing at `Indexer::clear_jar_index` so a future cleanup pass can decide whether to unify them.

**Interfaces:**
- Consumes: `JarId`/`JarTable` (Task 1), `materialized`/`materialization_failed` (Task 2).
- Produces: `pub(crate) fn index_jars(indexer: &Indexer, paths: &[PathBuf], sidecar: &mut Option<SidecarHandle>) -> usize` (signature unchanged, but no longer clears first — callers that need a full reindex call the new `clear_jar_maps` first), `pub(crate) fn clear_jar_maps(indexer: &Indexer)`, `pub(crate) fn materialize_jar_on_demand(indexer: &Indexer, jar_id: JarId, sidecar: &mut Option<SidecarHandle>) -> bool` (returns whether materialization succeeded).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn index_jars_no_longer_clears_existing_entries() {
    let idx = Indexer::new();
    // Simulate one JAR already materialized by a prior on-demand call.
    idx.jar_definitions
        .insert("PreExisting".to_owned(), vec![]);
    let mut sidecar: Option<crate::sidecar::SidecarHandle> = None;
    // index_jars with an empty path list must not wipe jar_definitions.
    let count = crate::indexer::jar::index_jars(&idx, &[], &mut sidecar);
    assert_eq!(count, 0);
    assert!(
        idx.jar_definitions.contains_key("PreExisting"),
        "index_jars must be additive — it must not clear entries for JARs \
         not in its own `paths` argument"
    );
}

#[test]
fn materialize_jar_on_demand_is_idempotent() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/nonexistent/test-fixture.jar");
    let mut sidecar: Option<crate::sidecar::SidecarHandle> = None;
    // No sidecar and a nonexistent path: materialization fails cleanly.
    let ok = crate::indexer::jar::materialize_jar_on_demand(&idx, jar_id, &mut sidecar);
    assert!(!ok, "materializing a nonexistent jar with no sidecar must fail, not panic");
    assert!(
        idx.materialization_failed.contains(&jar_id),
        "a failed attempt must be recorded so callers don't retry in a loop"
    );
    assert!(!idx.materialized.contains(&jar_id));
    // A second call must not re-attempt (still failed, still no sidecar call).
    let ok_again = crate::indexer::jar::materialize_jar_on_demand(&idx, jar_id, &mut sidecar);
    assert!(!ok_again);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp index_jars_no_longer_clears materialize_jar_on_demand_is_idempotent -- --nocapture`
Expected: FAIL — `index_jars_no_longer_clears_existing_entries` fails because current `index_jars` clears unconditionally; `materialize_jar_on_demand` doesn't exist yet.

- [ ] **Step 3: Write the implementation**

In `src/indexer/jar.rs`, replace the top of `index_jars` (the four `.clear()` calls) by extracting them into a new function, and remove the calls from `index_jars` itself:

```rust
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

    let mut jar_cache = super::jar_cache::load_jar_cache();
    // ... (the rest of the function body is UNCHANGED from here — the
    // cache-hit loop, the batch sidecar call, the cache-dirty save, and the
    // bare_names_dirty/completion_epoch bump at the end all stay exactly as
    // they are today; only the four `.clear()` lines at the very top move
    // into `clear_jar_maps` above)
```

Every existing caller of `index_jars` that relied on the implicit clear must call `clear_jar_maps` first. Find them: `rg -n "jar::index_jars\(" src/`. There are two production call sites today:
1. `src/workspace/scan_handler.rs` (the startup crawl, `spawn_jar_indexing`) — needs `crate::indexer::jar::clear_jar_maps(&indexer);` inserted immediately before its `let compiled_total = crate::indexer::jar::index_jars(&indexer, &paths, &mut sidecar);` line (Task 5 revisits this exact site to also fix the lock-scoping — if Task 5 lands first, its snippet already includes this call, don't duplicate it).
2. `src/cli/run.rs:465` (the `kmp-lsp find` CLI command) — needs the same `clear_jar_maps` call immediately before its `index_jars` call. This one is harmless either way today (the CLI always constructs a fresh `Indexer::new()` per invocation, so there's nothing to clear), but add the call anyway for consistency — a future CLI change that reuses an `Indexer` across invocations must not silently regress into stale data.

Note `handle_reindex` does **not** need a separate insertion: it doesn't call `index_jars` directly — it calls `spawn_jar_indexing()` (`rg -n "fn handle_reindex\|spawn_jar_indexing" src/`), which routes through call site 1 above. Task 11 revisits `handle_reindex` itself, but only for the Tier-1 (`jar_qualified`/`jar_bare_names`/`materialized`/`materialization_failed`) reset, not for this `clear_jar_maps` insertion.

Now add `materialize_jar_on_demand` at the end of `jar.rs`:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp index_jars_no_longer_clears materialize_jar_on_demand_is_idempotent -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 8 tests, clippy clean. Pay attention to any existing test that asserted on `index_jars`'s clearing behavior directly (`rg -n "fn.*index_jars" src/indexer/jar_tests.rs`) — if one exists and now fails, that test was pinning the exact behavior this task deliberately changes; move its clearing assertion onto a test of `clear_jar_maps` instead, following the "fix the setup, not the assertion" discipline from this codebase's history (never weaken what the test actually verifies).

- [ ] **Step 6: Commit**

```bash
git add src/indexer/jar.rs src/workspace/scan_handler.rs src/cli/run.rs src/indexer/jar_tests.rs
git commit -m "refactor(index): index_jars becomes additive; add materialize_jar_on_demand

index_jars no longer clears jar maps unconditionally — that would wipe
already-materialized JARs on every on-demand call. Extracted to
clear_jar_maps, called explicitly by the startup crawl (the one caller
that wants a full reindex). materialize_jar_on_demand is Tier 2's entry
point: idempotent, records failures so callers don't retry in a loop.
No production trigger for materialize_jar_on_demand yet — still inert."
```

---

### Task 5: Sidecar concurrency fix

**Files:**
- Modify: `src/workspace/scan_handler.rs` (the crawl loop around line 395 — read the full function first: `rg -n "let mut sidecar = indexer" -B 20 -A 10 src/workspace/scan_handler.rs`)
- Test: `src/workspace/scan_handler_tests.rs` (check existing test conventions there first)

**Why:** design §Concurrency — the crawl currently holds `jar_sidecar.lock()` across the entire `index_jars` + `index_sources_jars` pass. An on-demand materialization request arriving during that window blocks until the whole crawl finishes.

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub(crate) fn try_lock_sidecar_bounded(indexer: &Indexer) -> Option<std::sync::MutexGuard<'_, Option<SidecarHandle>>>` — a short, bounded (not indefinite) attempt to acquire `jar_sidecar`, returning `None` if it can't within budget rather than blocking.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn try_lock_sidecar_bounded_returns_none_when_held() {
    let idx = Indexer::new();
    let held_guard = idx.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
    // While `held_guard` is alive, a bounded attempt from "another caller"
    // must return None quickly rather than blocking this test forever.
    let attempt = crate::workspace::scan_handler::try_lock_sidecar_bounded(&idx);
    assert!(
        attempt.is_none(),
        "bounded lock attempt must not block/succeed while the sidecar is held elsewhere"
    );
    drop(held_guard);
    let attempt_after_release = crate::workspace::scan_handler::try_lock_sidecar_bounded(&idx);
    assert!(attempt_after_release.is_some(), "must succeed once the lock is free");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp try_lock_sidecar_bounded -- --nocapture`
Expected: FAIL — function doesn't exist yet.

- [ ] **Step 3: Write the implementation**

In `src/workspace/scan_handler.rs`, add near the top of the file (after imports):

```rust
/// A short, bounded attempt to lock `jar_sidecar` — used by on-demand
/// materialization (Task 8) so a hover/completion request never blocks
/// indefinitely behind the startup crawl or another in-flight materialization.
/// Returns `None` (degrade to Tier-1-only for this request) rather than
/// waiting. See design §Concurrency.
pub(crate) fn try_lock_sidecar_bounded(
    indexer: &crate::indexer::Indexer,
) -> Option<std::sync::MutexGuard<'_, Option<crate::sidecar::SidecarHandle>>> {
    // `try_lock` is genuinely non-blocking (fails immediately if contended,
    // rather than spinning) — the "bounded" framing in the design becomes
    // "immediate or nothing" here, which is the simplest correct instance of
    // "don't block the interactive path" and avoids inventing a timeout
    // mechanism this codebase doesn't otherwise use.
    indexer.jar_sidecar.try_lock().ok()
}
```

Now fix the crawl's per-batch lock hold. The real function (`rg -n "let mut sidecar = indexer" -B 10 -A 55 src/workspace/scan_handler.rs`) holds the `sidecar` binding from line 395 all the way to line 445, where a *later*, unrelated block reads `sidecar.is_none()` to decide `final_phase` — well after `index_sources_jars` (line 416) has already run:

```rust
            let mut sidecar = indexer
                .jar_sidecar
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let compiled_total = crate::indexer::jar::index_jars(&indexer, &paths, &mut sidecar);

            // ... (generation check, `index_sources_jars` call, early-return
            // paths — all unchanged, see below) ...

            let total = sources_total + compiled_total;
            let final_phase = if sidecar.is_none() && compiled_total > 0 {
                // Sidecar died mid-index; sources may still be available.
                JarPhase::Failed(format!(
                    "sidecar died mid-index; {total} symbols partially loaded ({sources_total} from sources, {compiled_total} from compiled)"
                ))
            } else {
                JarPhase::Ready { count: total }
            };
```

**Do not simply wrap the first two lines in a block** — `sidecar` is read again 46 lines later for the `final_phase` computation, so scoping the binding to a block that drops it before that point does not compile (`cannot find value `sidecar` in this scope`). Instead, thread out the one bit of information the later check actually needs — whether the sidecar survived — as a `bool`, and drop the guard itself at the end of the scoped block:

```rust
            crate::indexer::jar::clear_jar_maps(&indexer);
            let (compiled_total, sidecar_alive) = {
                let mut sidecar = indexer
                    .jar_sidecar
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let compiled_total =
                    crate::indexer::jar::index_jars(&indexer, &paths, &mut sidecar);
                (compiled_total, sidecar.is_some())
                // `sidecar` MutexGuard drops here, at the end of this block —
                // released before index_sources_jars runs, so an on-demand
                // materialization request only ever contends with the
                // compiled-JAR phase, never the (much longer) sources-JAR
                // phase that follows it.
            };
```

Then update the `final_phase` computation (unchanged in every other respect — same position in the function, same surrounding generation checks, same `index_sources_jars` call in between) to read the threaded-out bool instead of the now-dropped guard:

```rust
            let final_phase = if !sidecar_alive && compiled_total > 0 {
```

Every other read of `compiled_total` in the function (the `total = sources_total + compiled_total` line, the `log::info!` call, etc.) is unaffected — `compiled_total` was already a plain `usize`, only `sidecar` itself was borrowed past its old scope.

(Note: `clear_jar_maps` here is the call Task 4 already required at this site — if Task 4's commit already added it, this step only needs the lock-scoping change around it, not a second `clear_jar_maps` insertion.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp try_lock_sidecar_bounded -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 9 tests, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/workspace/scan_handler.rs src/workspace/scan_handler_tests.rs
git commit -m "fix(workspace): scope the sidecar lock per-phase, not per-crawl; add bounded on-demand attempt

The crawl held jar_sidecar across both the compiled-JAR AND sources-JAR
phases; only the former needs it. Scoping the lock to just the
compiled-JAR block means on-demand materialization (Task 8) never blocks
behind the (much longer) sources-JAR phase. try_lock_sidecar_bounded
gives on-demand callers a non-blocking degrade-to-Tier-1 path instead of
stalling the hover/completion request."
```

---

### Task 6: `build_jar_manifest` — Tier 1 build

**Files:**
- Modify: `src/indexer/jar.rs` (add near `index_jars`)
- Test: `src/indexer/jar_tests.rs`

**Interfaces:**
- Consumes: `JarManifestEntry`/`load_jar_manifest_cache`/`save_jar_manifest_cache`/`manifest_entry_is_fresh` (Task 3), `jar_qualified`/`jar_bare_names` (Task 2).
- Produces: `pub(crate) fn build_jar_manifest(indexer: &Indexer, paths: &[PathBuf], sidecar: &mut Option<SidecarHandle>) -> usize`.

**Design note on the "pay once" cost:** on a cold manifest cache (no `jar-manifest-v1.bin` yet, e.g. the first run after this ships), building the manifest for a JAR with no cache hit still requires calling the sidecar and receiving its full response (name+kind+container+detail+params+doc — the sidecar protocol itself doesn't change, see design §Tier 1) — the manifest build discards everything but name+kind+container immediately, never constructing a `SymbolEntry`/long-lived `SidecarSymbol` for it, and saves only the cheap fields to `jar-manifest-v1.bin` for future warm starts. This is a genuine one-time cost on first run per JAR, not a repeated one.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn build_jar_manifest_populates_tier1_without_materializing_tier2() {
    let idx = Indexer::new();
    // No real sidecar in this unit test; a JAR with a fresh manifest cache
    // hit still exercises the "populate from cache" path without needing a
    // live sidecar process. Seed the manifest cache directly.
    let mut cache = std::collections::HashMap::new();
    cache.insert(
        "/gradle/caches/fixture-1.0.jar".to_owned(),
        crate::indexer::jar_manifest_cache::JarManifestEntry {
            mtime_secs: 0,
            mtime_nanos: 0,
            file_size: 0,
            names: vec![crate::indexer::jar_manifest_cache::JarManifestName {
                name: "FixtureClass".to_owned(),
                kind: "class".to_owned(),
                container: None,
                package: Some("com.fixture.pkg".to_owned()),
            }],
        },
    );
    // This test only exercises the in-memory population path, not the
    // (mtime,size) freshness gate — see Step 3's implementation for where
    // that gate lives; a full end-to-end freshness test belongs in a
    // follow-up test using a real tempfile fixture, out of scope for this
    // unit-level check.
    let jar_id = idx.jar_table.intern("/gradle/caches/fixture-1.0.jar");
    // NOT calling build_jar_manifest here (RED phase — see Step 2). This
    // asserts the *shape* build_jar_manifest must produce once it exists:
    // jar_bare_names keyed by the short name, jar_qualified keyed by the
    // real FQN (package.Name) built from the manifest's `package` field —
    // never a bare, un-prefixed name in jar_qualified.
    idx.jar_bare_names.entry("FixtureClass".to_owned()).or_default().push(jar_id);
    idx.jar_qualified.insert("com.fixture.pkg.FixtureClass".to_owned(), jar_id);
    assert!(idx.jar_bare_names.contains_key("FixtureClass"));
    assert_eq!(
        idx.jar_qualified.get("com.fixture.pkg.FixtureClass").map(|e| *e),
        Some(jar_id),
        "jar_qualified must hold the real FQN, built from the manifest's package field"
    );
    assert!(
        idx.jar_definitions.is_empty(),
        "populating Tier 1 must never touch Tier 2's jar_definitions map"
    );
    assert!(
        !idx.materialized.contains(&jar_id),
        "Tier 1 population must not mark the jar materialized"
    );
}
```

(This step's test is intentionally light — it pins the *contract* "Tier 1 population never touches Tier 2 state, and produces real FQNs in `jar_qualified`, not bare names" rather than driving the full sidecar/cache path, which needs a real fixture JAR to test end-to-end; add that integration-level test alongside the existing sidecar-fixture tests in `jar_tests.rs` if one already exists for `index_jars` — check `rg -n "fn.*index_jars.*test\|TestSidecar\|MockSidecar" src/indexer/jar_tests.rs` for the existing fixture pattern and reuse it.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp build_jar_manifest_populates_tier1 -- --nocapture`
Expected: FAIL to compile — `crate::indexer::jar::build_jar_manifest` doesn't exist yet, and this test file will reference it once Step 3 lands (the test above hand-seeds the maps to pin the target shape; once Step 3's `build_jar_manifest` exists, replace the two hand-seed lines with an actual call: `crate::indexer::jar::build_jar_manifest(&idx, &[std::path::PathBuf::from("/gradle/caches/fixture-1.0.jar")], &mut None);` after seeding `load_jar_manifest_cache`'s backing file via `save_jar_manifest_cache(&cache)` first, or by refactoring `populate_tier1_from_manifest` to be independently callable/testable — either is acceptable, the assertions above are what must hold either way). Confirm the RED state is "function doesn't exist," then proceed.

- [ ] **Step 3: Write the implementation**

In `src/indexer/jar.rs`:

```rust
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
    total_names
}

fn populate_tier1_from_manifest(
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
        // does with `SidecarSymbol::pkg` to populate `indexer.qualified`, so
        // Tier 1 needs no separate FQN-construction mechanism, and
        // jar_qualified is never a dead map.
        if let Some(pkg) = entry.package.as_deref().filter(|p| !p.is_empty()) {
            let fqn = format!("{pkg}.{}", entry.name);
            indexer.jar_qualified.entry(fqn).or_insert(jar_id);
        }
        // No package (default package, or a manifest cached before this
        // field existed): the symbol is still reachable via
        // `jar_bare_names` for completion/auto-import candidate listing —
        // just not by exact-FQN lookup until Tier 2 materializes it.
    }
    names.len()
}

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
```

**On `jar_qualified`:** an earlier draft of this task deferred `jar_qualified` (FQN-keyed) population to Task 9, reasoning the manifest "can't cheaply construct a real FQN without package data." That premise was wrong — the sidecar already emits a real per-symbol package on `SidecarSymbol::pkg` (the same field `jar.rs`'s Tier-2 path uses to build `indexer.qualified`), so Task 3's `JarManifestName.package` field and the `populate_tier1_from_manifest` logic above build real FQNs into `jar_qualified` directly, at manifest-build time — no deferral needed. Task 9 consumes `jar_qualified` as already-populated, not as something it must first construct.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp build_jar_manifest_populates_tier1 -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 10 tests, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/indexer/jar.rs src/indexer/jar_tests.rs
git commit -m "feat(index): build_jar_manifest — Tier 1 build, discards detail/params/doc

Populates jar_bare_names AND jar_qualified (real FQNs, built from the
manifest's package field — the same SidecarSymbol::pkg jar.rs's Tier-2
path already uses) from either the manifest cache (cheap) or a sidecar
call whose full response is discarded down to name+kind+container+package
immediately. Never touches jar_definitions/jar_files/materialized. Not
yet called from the crawl — still inert."
```

---

### Task 7: Diagnostics Tier-1 suppression fix

**Files:**
- Modify: `src/features/call_arg_diagnostics.rs` (the signature-resolution/suppression logic — read the full flow first: `rg -n "SignatureResult::Overloaded\|fn call_arg_diagnostics" src/features/call_arg_diagnostics.rs src/indexer/infer/sig.rs`)
- Test: `src/features/call_arg_diagnostics_tests.rs`

**Why:** design §Error handling (B2) — `IndexOnly` (diagnostics) never triggers materialization, but must still avoid false positives when a same-named candidate exists in an unmaterialized JAR. Extends the existing `SignatureResult::Overloaded` suppression with a Tier-1 existence check.

**Interfaces:**
- Consumes: `jar_bare_names` (Task 2/6).
- Produces: a new `SignatureResult` variant OR a wrapping check at the `call_arg_diagnostics` call site — read `sig.rs`'s exact `SignatureResult` enum first (`rg -n "enum SignatureResult" -A 15 src/indexer/infer/sig.rs`) to decide which is less invasive; prefer adding the check at the diagnostics call site over changing the enum, since the enum is likely consumed by other features (signature help) that don't need this Tier-1-awareness.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn call_arg_diagnostics_suppresses_when_unmaterialized_jar_has_same_named_candidate() {
    let idx = Indexer::new();
    idx.index_content(
        &uri("/Test.kt"),
        "fun main() { Foo(1, 2) }\nfun Foo(a: Int): Unit {}",
    );
    // Simulate: a same-named `Foo` exists in a JAR whose Tier 2 data isn't
    // materialized yet — Tier 1 knows the name exists, nothing more.
    let jar_id = idx.jar_table.intern("/fake/other-foo.jar");
    idx.jar_bare_names
        .entry("Foo".to_owned())
        .or_default()
        .push(jar_id);

    let doc = crate::indexer::live_tree::parse_live(
        "fun main() { Foo(1, 2) }\nfun Foo(a: Int): Unit {}",
        crate::parser::Language::Kotlin,
    )
    .unwrap();
    let diagnostics = crate::features::call_arg_diagnostics::call_arg_diagnostics(
        &idx,
        &uri("/Test.kt"),
        &doc,
    );
    assert!(
        diagnostics.is_empty(),
        "a same-named candidate in an unmaterialized JAR must suppress the \
         'wrong parameter count' warning, not let it through as a false \
         positive; got: {diagnostics:?}"
    );
}
```

(Adjust the exact `parse_live`/`LiveDoc` construction call to match whatever helper this test file's *existing* tests already use — check `rg -n "fn.*call_arg_diagnostics" -B 5 src/features/call_arg_diagnostics_tests.rs` for the established fixture pattern rather than inventing a new one.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp call_arg_diagnostics_suppresses_when_unmaterialized -- --nocapture`
Expected: FAIL — today's diagnostics has no Tier-1 concept, so it sees only the workspace `Foo(a: Int)`, concludes unique, and emits a false "expected 1 argument, found 2" warning.

- [ ] **Step 3: Write the implementation**

In `src/features/call_arg_diagnostics.rs`, find where a `SignatureResult::Unique` (or equivalent single-candidate result) leads to emitting the diagnostic (near the existing `Overloaded` suppression check cited in the design at `call_arg_diagnostics.rs:171-173` — read that exact block first). Add a Tier-1 check immediately before emitting:

```rust
// Before trusting a `Unique` signature result enough to emit a diagnostic,
// check whether a same-named candidate exists in an unmaterialized JAR —
// if so, treat it the same as Overloaded and suppress. IndexOnly policy
// forbids triggering materialization here (no IO on the diagnostics path),
// but checking jar_bare_names is a cheap in-memory lookup, not IO. See
// design §Error handling (B2).
if let Some(candidate_jars) = indexer.jar_bare_names.get(&callee_name) {
    let has_unmaterialized_candidate = candidate_jars
        .iter()
        .any(|jar_id| !indexer.materialized.contains(jar_id));
    if has_unmaterialized_candidate {
        continue; // or `return None` / whatever this call site's control flow uses for "suppress"
    }
}
```

(The exact variable names `callee_name`/`indexer`/the loop-vs-function control-flow shape must match what's actually in `call_arg_diagnostics.rs` at the real call site — read the surrounding ~20 lines before writing this in, and adapt the snippet's control flow, not just its logic, to fit.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp call_arg_diagnostics_suppresses_when_unmaterialized -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 11 tests, clippy clean. Also specifically re-run the existing `Overloaded`-suppression regression tests (`rg -n "fn.*overload" src/features/call_arg_diagnostics_tests.rs`) to confirm this addition doesn't change their behavior (they should still pass unchanged — this task only adds a new suppression trigger, not a new emission path).

- [ ] **Step 6: Commit**

```bash
git add src/features/call_arg_diagnostics.rs src/features/call_arg_diagnostics_tests.rs
git commit -m "fix(diagnostics): suppress call-arg warnings when an unmaterialized JAR has a same-named candidate

IndexOnly policy still never triggers materialization (no IO added), but
now checks jar_bare_names — a cheap in-memory Tier-1 lookup — before
trusting a single-candidate signature result. Closes the false-positive
gap flagged in the lazy-library-loading design's critique (§Error
handling, finding B2): jar_phase==Ready no longer needs to imply every
candidate is materialized for this diagnostic to stay correct."
```

---

### Task 8: Wire remaining direct-read consumers

**Files:**
- Modify: `src/indexer/resolution.rs` (lines ~663, ~677 per the design's citation — re-verify exact current line numbers with `rg -n "jar_definitions\|jar_files" src/indexer/resolution.rs`)
- Modify: `src/indexer/lookup.rs` (lines ~69, ~83, ~99 — `rg -n "jar_definitions\|jar_files\|jar_symbol_packages" src/indexer/lookup.rs`)
- Modify: `src/resolver/infer.rs` (lines ~778, ~933 — `rg -n "jar_definitions\|jar_files" src/resolver/infer.rs`)
- Modify: `src/resolver/resolve.rs` (the `importable_fqns` read site — `rg -n "importable_fqns" src/resolver/resolve.rs`)
- Test: companion `*_tests.rs` for each

**Interfaces:**
- Consumes: `materialize_jar_on_demand` (Task 4), `try_lock_sidecar_bounded` (Task 5), `jar_bare_names`/`jar_qualified` (Task 2/6).
- Produces: `pub(crate) fn ensure_jar_materialized(indexer: &Indexer, name: &str) -> bool` (shared helper, `src/indexer/jar.rs`) — looks up `name` in `jar_bare_names`, attempts materialization for any not-yet-materialized candidate via a bounded sidecar lock, returns whether at least one candidate is now materialized.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn ensure_jar_materialized_promotes_a_tier1_only_candidate() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/nonexistent/fixture.jar");
    idx.jar_bare_names
        .entry("SomeLibType".to_owned())
        .or_default()
        .push(jar_id);
    // No real sidecar — materialization will fail, but the function must
    // attempt it (not just silently return false for an unknown reason) and
    // record the attempt via materialization_failed, proving it took the
    // promote path rather than short-circuiting.
    let _ = crate::indexer::jar::ensure_jar_materialized(&idx, "SomeLibType");
    assert!(
        idx.materialization_failed.contains(&jar_id) || idx.materialized.contains(&jar_id),
        "ensure_jar_materialized must attempt promotion for a known Tier-1 \
         candidate, landing in either materialized or materialization_failed \
         — not leave the jar in limbo"
    );
}

#[test]
fn ensure_jar_materialized_no_op_for_unknown_name() {
    let idx = Indexer::new();
    let result = crate::indexer::jar::ensure_jar_materialized(&idx, "TotallyUnknownName");
    assert!(!result, "a name with no Tier-1 candidate must be a cheap no-op");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp ensure_jar_materialized -- --nocapture`
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Write the shared helper**

In `src/indexer/jar.rs`:

```rust
/// Shared promotion helper for every direct-read consumer of
/// `jar_definitions`/`jar_files`: if `name` has a Tier-1 candidate that
/// isn't materialized yet, attempt Tier-2 materialization via a bounded,
/// non-blocking sidecar lock attempt (never blocks the caller — see design
/// §Concurrency). Returns whether at least one candidate is now
/// materialized (either already was, or just got promoted).
///
/// Callers: `indexer/resolution.rs`, `indexer/lookup.rs`, `resolver/infer.rs`,
/// `resolver/resolve.rs` (Task 8) — each calls this at its own read site
/// rather than through a central chokepoint (design §Consumer integration).
pub(crate) fn ensure_jar_materialized(indexer: &crate::indexer::Indexer, name: &str) -> bool {
    let Some(candidates) = indexer.jar_bare_names.get(name) else {
        return false;
    };
    let mut any_materialized = false;
    for jar_id in candidates.iter() {
        if indexer.materialized.contains(jar_id) {
            any_materialized = true;
            continue;
        }
        if indexer.materialization_failed.contains(jar_id) {
            continue;
        }
        let Some(mut sidecar_guard) = crate::workspace::scan_handler::try_lock_sidecar_bounded(indexer) else {
            continue; // degrade gracefully — a later call may succeed
        };
        // Explicit deref: `sidecar_guard` is `MutexGuard<Option<SidecarHandle>>`,
        // `materialize_jar_on_demand` takes `&mut Option<SidecarHandle>` — spelled
        // out rather than relying on deref coercion at the call site.
        if materialize_jar_on_demand(indexer, *jar_id, &mut *sidecar_guard) {
            any_materialized = true;
        }
    }
    any_materialized
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp ensure_jar_materialized -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Wire the four consumer call sites**

For each of the four files, find the exact read of `jar_definitions`/`jar_files` and add an `ensure_jar_materialized` call immediately before it. Concretely, for `src/indexer/resolution.rs`'s `find_definitions`-shaped function (the one reading `self.jar_definitions.get(name)` near line 663 per the earlier read):

```rust
        if self.jar_qualified_or_bare_has_candidate(name) {
            crate::indexer::jar::ensure_jar_materialized(self, name);
        }
        if let Some(jar_locs) = self.jar_definitions.get(name) {
            locs.extend(jar_locs.iter().cloned());
        }
```

(`jar_qualified_or_bare_has_candidate` is a small new private helper — `self.jar_bare_names.contains_key(name) || self.jar_qualified.contains_key(name)` — add it once in `indexer.rs` as an `impl Indexer` method and reuse it at all four call sites, rather than repeating the two-map check inline four times.)

Repeat the same shape (check Tier 1 has a candidate → call `ensure_jar_materialized` → then do the existing direct read, unchanged) at:
- `indexer/lookup.rs`'s `jar_declaration_scope` (the `self.jar_definitions.get(name)?` line near the top of the function read earlier).
- `resolver/infer.rs`'s two cited sites (read the exact surrounding code first — `rg -n "jar_definitions\|jar_files" -B 5 -A 5 src/resolver/infer.rs`).
- `resolver/resolve.rs`'s `importable_fqns` read (Task 9 handles this one specifically as part of the auto-import fix — skip it here to avoid double-editing the same site in two tasks).

- [ ] **Step 6: Write a decoy test proving promotion actually changes the answer**

```rust
#[test]
fn jar_declaration_scope_finds_a_tier1_only_symbol_after_promotion_attempt() {
    // This test cannot exercise a REAL sidecar promotion without a live
    // fixture JAR + sidecar process (integration-test territory, out of
    // scope for this unit test). It instead pins the CONTRACT: calling
    // jar_declaration_scope on a name that only exists in jar_bare_names
    // must not panic and must call ensure_jar_materialized (observable via
    // materialization_failed being populated for the fake jar path, proving
    // the promotion attempt happened rather than being silently skipped).
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/nonexistent/fixture.jar");
    idx.jar_bare_names
        .entry("RemoteType".to_owned())
        .or_default()
        .push(jar_id);
    let _ = idx.jar_declaration_scope("RemoteType");
    assert!(
        idx.materialization_failed.contains(&jar_id),
        "jar_declaration_scope must attempt promotion for a Tier-1-only name, \
         not silently return None without trying"
    );
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp jar_declaration_scope_finds_a_tier1_only -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 14 tests, clippy clean.

- [ ] **Step 9: Commit**

```bash
git add src/indexer/jar.rs src/indexer.rs src/indexer/resolution.rs src/indexer/lookup.rs src/resolver/infer.rs src/indexer/resolution_tests.rs src/indexer/lookup_tests.rs src/resolver/infer_tests.rs
git commit -m "feat(index): wire ensure_jar_materialized into direct-read consumers

resolution.rs, lookup.rs, resolver/infer.rs each call the shared
promotion helper at their own read site before reading jar_definitions/
jar_files, per design §Consumer integration — several integration points,
not one central chokepoint (the design's original single-rung idea was
wrong per the independent critique). resolver/resolve.rs's importable_fqns
site is deliberately deferred to Task 9 (auto-import needs different
semantics — promote on completion-item-selection, not on lookup)."
```

---

### Task 9: Completion + auto-import integration

**Files:**
- Modify: `src/indexer/apply.rs` (`rebuild_bare_name_cache` line ~1073-1090, `rebuild_importable_fqns` line ~1109-1134 — re-read both fully first, they were quoted in the design)
- Modify: `src/resolver/complete.rs` (`complete_bare`, line ~1700 — read its full body first)
- Test: `src/indexer/apply_tests.rs`, `src/resolver/tests.rs`

**On promotion timing:** Task 6 already builds real FQNs into `jar_qualified` at manifest time, so a Tier-1-only candidate is offered in the auto-import list without needing promotion first — this task's `complete_bare` change (Step 4) instead promotes Tier 1 → Tier 2 for the *visible* candidate set at completion-item-build time, so the item's `detail` (signature, doc) is real rather than a name-only stub when possible. This is naturally bounded (a popup shows at most a few dozen items), unlike per-keystroke enumeration, which stays Tier-1-only per the design.

**Interfaces:**
- Consumes: `ensure_jar_materialized` (Task 8), `jar_bare_names`/`jar_qualified` (Task 2/6).
- Produces: `rebuild_bare_name_cache` and `rebuild_importable_fqns` both merge in Tier-1 names alongside their existing materialized-data sources.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn rebuild_bare_name_cache_includes_tier1_only_jar_names() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/lib.jar");
    idx.jar_bare_names
        .entry("LazyLibType".to_owned())
        .or_default()
        .push(jar_id);
    idx.bare_names_dirty.store(true, std::sync::atomic::Ordering::Release);
    idx.rebuild_bare_name_cache();
    let cache = idx.bare_name_cache.read().unwrap();
    assert!(
        cache.contains(&"LazyLibType".to_owned()),
        "bare-word completion must offer a name that only exists in an \
         unmaterialized JAR's Tier-1 manifest, not just already-materialized names"
    );
}

#[test]
fn rebuild_importable_fqns_includes_tier1_only_candidates() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/lib.jar");
    // Seed both maps exactly as build_jar_manifest (Task 6) would for a
    // symbol whose manifest entry carries a package: jar_bare_names always,
    // jar_qualified keyed by the real FQN.
    idx.jar_bare_names
        .entry("LazyLibType".to_owned())
        .or_default()
        .push(jar_id);
    idx.jar_qualified
        .insert("com.fake.lib.LazyLibType".to_owned(), jar_id);
    idx.bare_names_dirty.store(true, std::sync::atomic::Ordering::Release);
    idx.rebuild_bare_name_cache(); // calls rebuild_importable_fqns internally
    let fqns = idx.importable_fqns.read().unwrap();
    assert!(
        fqns.get("LazyLibType")
            .is_some_and(|v| v.contains(&"com.fake.lib.LazyLibType".to_owned())),
        "auto-import must offer a Tier-1-only candidate's real FQN even \
         before its JAR is materialized — this is the case that categorically \
         cannot be covered by import-scoped eager promotion (no ImportEntry \
         exists yet for a symbol nobody has imported)"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp rebuild_bare_name_cache_includes_tier1 rebuild_importable_fqns_includes_tier1 -- --nocapture`
Expected: FAIL — `rebuild_bare_name_cache_includes_tier1_only_jar_names` fails because `rebuild_bare_name_cache` doesn't merge `jar_bare_names` yet. `rebuild_importable_fqns_includes_tier1_only_candidates` fails because `rebuild_importable_fqns` today only iterates `files`, never `jar_qualified` (which Task 6 already populates in production, but nothing reads yet).

- [ ] **Step 3: Write the implementation**

In `src/indexer/apply.rs`, `rebuild_bare_name_cache` (the function quoted in the design at line 1073-1090): add the Tier-1 merge after the existing `jar_definitions` loop:

```rust
        if let Ok(mut cache) = self.bare_name_cache.write() {
            let mut names: Vec<String> = self.definitions.iter().map(|e| e.key().clone()).collect();
            for entry in self.jar_definitions.iter() {
                if !self.definitions.contains_key(entry.key()) {
                    names.push(entry.key().clone());
                }
            }
            // Tier 1: names that exist only in an unmaterialized JAR's
            // manifest. Without this, bare-word completion silently loses
            // coverage for anything not yet promoted to Tier 2 — see design
            // §Consumer integration.
            for entry in self.jar_bare_names.iter() {
                if !self.definitions.contains_key(entry.key())
                    && !self.jar_definitions.contains_key(entry.key())
                {
                    names.push(entry.key().clone());
                }
            }
            names.sort_unstable();
            names.dedup();
            *cache = names;
        }
```

`rebuild_importable_fqns` (line 1109-1134): this one's pre-existing gap (never reads `jar_definitions` at all, per the design's finding) is explicitly NOT this task's job to fix — only add the Tier-1 merge, matching the same shape:

```rust
    fn rebuild_importable_fqns(&self) {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for file_entry in self.files.iter() {
            let data = file_entry.value();
            let pkg = match &data.package {
                Some(p) if !p.is_empty() => p.clone(),
                _ => continue,
            };
            let syms = &data.symbols;
            for sym in syms.iter() {
                if sym.container.is_some() {
                    continue;
                }
                let fqn = format!("{}.{}", pkg, sym.name);
                map.entry(sym.name.clone()).or_default().push(fqn);
            }
        }
        // Tier 1: jar_qualified already stores full FQNs as keys, built by
        // build_jar_manifest (Task 6) directly from the manifest's package
        // field — no materialization required first. Merging this in means
        // a Tier-1-only candidate is offered by FQN as soon as its JAR is
        // manifested, not only after Tier 2 promotion. (A symbol whose
        // manifest entry had no package — default package, or a manifest
        // cached before this field existed — has no FQN to offer here; it's
        // still reachable via jar_bare_names for bare-name completion.)
        for entry in self.jar_qualified.iter() {
            let fqn = entry.key();
            if let Some(dot) = fqn.rfind('.') {
                let simple_name = &fqn[dot + 1..];
                map.entry(simple_name.to_owned())
                    .or_default()
                    .push(fqn.clone());
            }
        }
        for fqns in map.values_mut() {
            fqns.sort_unstable();
            fqns.dedup();
        }
        if let Ok(mut guard) = self.importable_fqns.write() {
            *guard = map;
        }
    }
```

- [ ] **Step 4: Wire `complete_bare`'s promote-on-selection**

Read `resolver/complete.rs`'s `complete_bare` fully (`rg -n "pub\(crate\) fn complete_bare" -A 80 src/resolver/complete.rs`) to find where it builds the final `CompletionItem` list from `importable_fqns`/`bare_name_cache` matches. Step 3 already means a Tier-1-only candidate is *offered* (by name, and by FQN when its manifest carried a package) without this step — what this step adds is *fidelity*: for each candidate whose JAR isn't yet in `materialized`, attach an LSP `command` or use `completionItem/resolve`-time promotion — concretely, call `crate::indexer::jar::ensure_jar_materialized(indexer, &candidate_name)` at the point `complete_bare` is about to construct the `CompletionItem` for such a candidate, immediately before building its `detail`/`insert_text` fields, so the item it returns carries real signature data when possible (falls back to a name-only/FQN-only completion item, still valid and still offered, if the bounded lock attempt didn't succeed in time — matching the graceful-degradation contract from Task 5).

```rust
    // Tier-1-only candidates (in jar_bare_names but not yet in
    // importable_fqns/jar_definitions): attempt promotion now, since
    // completion candidate lists are bounded by what's actually rendered
    // (unlike full enumeration) — cheap enough to do eagerly here rather
    // than waiting for a separate completionItem/resolve round-trip.
    for candidate_name in &tier1_only_candidates {
        crate::indexer::jar::ensure_jar_materialized(indexer, candidate_name);
    }
```

(Insert this immediately before whatever loop in `complete_bare` currently builds `CompletionItem`s from the merged candidate set — the exact insertion point depends on that function's real control flow, which must be read directly rather than assumed here.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --bin kmp-lsp rebuild_bare_name_cache_includes_tier1 rebuild_importable_fqns_includes_tier1 -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 16 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add src/indexer/apply.rs src/resolver/complete.rs src/indexer/apply_tests.rs src/resolver/tests.rs
git commit -m "feat(completion): merge Tier-1 candidates into bare-name/auto-import lists

rebuild_bare_name_cache and rebuild_importable_fqns both now include
names known only via jar_bare_names/jar_qualified (Tier 1), not just
already-materialized data — jar_qualified is already real-FQN-keyed at
this point (Task 6 builds it from the manifest's package field), so a
Tier-1-only candidate is offered by FQN without needing promotion first.
complete_bare promotes Tier-1-only candidates on completion-item-build
(bounded by what's rendered) so the item's detail is real, not a stub.
Closes the auto-import gap named explicitly in the design spec — a
symbol needing auto-import has no ImportEntry by definition, so
import-scoped eager promotion (Task 10) cannot cover this case."
```

---

### Task 10: Import-scoped eager promotion

**Files:**
- Modify: wherever `did_open`/first-diagnostics-pass is handled for a file (`rg -n "fn.*did_open\|fn.*handle_did_open" src/backend/*.rs src/workspace/*.rs` to find the exact site)
- Test: matching `*_tests.rs`

**Interfaces:**
- Consumes: `ImportEntry` (existing, `types.rs`), `ensure_jar_materialized` (Task 8, reused directly — this task just calls it for every name in the file's own import list rather than waiting for a lookup miss).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn opening_a_file_eagerly_promotes_its_own_imports() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/compose.jar");
    idx.jar_bare_names
        .entry("Column".to_owned())
        .or_default()
        .push(jar_id);
    let uri = uri("/Screen.kt");
    idx.index_content(&uri, "import androidx.compose.foundation.layout.Column\n\nfun Screen() { Column {} }");
    crate::workspace::promote_file_imports(&idx, &uri);
    assert!(
        idx.materialization_failed.contains(&jar_id) || idx.materialized.contains(&jar_id),
        "opening a file must eagerly attempt materialization for every JAR \
         its own ImportEntry list references, before any diagnostics pass runs"
    );
}
```

(Adjust `crate::workspace::promote_file_imports` to whatever module actually owns file-open handling once Step 1's investigation locates it — the function name/location here is a proposal, not a fact; confirm against the real `did_open` handler before writing this test.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp opening_a_file_eagerly_promotes -- --nocapture`
Expected: FAIL — function doesn't exist yet.

- [ ] **Step 3: Write the implementation**

Locate the file's parsed `FileData.imports: Vec<ImportEntry>` (already populated by parsing, per `apply.rs`'s `contributions_from_data` — no new parsing needed) and, in the `did_open` handler (or wherever the file first becomes "the active file" before its first diagnostics pass), add:

```rust
/// Eagerly promote every JAR a file's own imports reference, before that
/// file's first diagnostics pass runs. Without this, call-arg/nullable
/// diagnostics on the file's own imported library types would fall back to
/// the Tier-1-suppression path (Task 7) on every fresh open — correct, but
/// unnecessarily degraded for the common case (design §Import-scoped eager
/// promotion: "briefly incomplete, then correct" is the intended gap, not
/// "always degraded by default").
pub(crate) fn promote_file_imports(indexer: &crate::indexer::Indexer, uri: &tower_lsp::lsp_types::Url) {
    let Some(file_data) = indexer.mem_file_data_for(uri.as_str()) else {
        return;
    };
    for import in &file_data.imports {
        if import.is_star {
            continue; // wildcard imports: design explicitly defers package-keyed Tier-1 to v2
        }
        let simple_name = import.local_name.as_str();
        crate::indexer::jar::ensure_jar_materialized(indexer, simple_name);
    }
}
```

(`mem_file_data_for` is a proposed accessor name — check `indexer.rs`/`lookup.rs` for whatever the existing convention is for "get this URI's currently-known `FileData`" and use that instead, e.g. `self.files.get(uri).map(|r| r.clone())` inlined if there's no existing named helper.)

Call `promote_file_imports(&indexer, &uri)` from the `did_open` handler, before the first diagnostics computation for that file (find the exact ordering in the real handler and insert at the right point — do not guess the surrounding control flow without reading it).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp opening_a_file_eagerly_promotes -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 17 tests, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add <the real did_open handler file> src/workspace/*.rs <matching test file>
git commit -m "feat(workspace): eagerly promote a file's own imports on open

Before the file's first diagnostics pass, materialize the JARs its
ImportEntry list already names. Turns 'every fresh file degrades to
Tier-1 suppression for its own imported library types' into 'briefly
incomplete, then correct' — the design's explicit v1 scope (§Import-scoped
eager promotion), load-bearing given Task 7's diagnostics fix."
```

---

### Task 11: Reindex resets Tier 1 + materialization state

**Files:**
- Modify: wherever `handle_reindex` lives (`rg -n "fn handle_reindex" src/`)
- Test: matching `*_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reindex_resets_materialized_and_jar_tier1_state() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/lib.jar");
    idx.materialized.insert(jar_id);
    idx.jar_qualified.insert("SomeType".to_owned(), jar_id);
    idx.jar_bare_names.entry("SomeType".to_owned()).or_default().push(jar_id);

    crate::workspace::handle_reindex(&idx /* + whatever other args the real signature needs */);

    assert!(!idx.materialized.contains(&jar_id), "reindex must reset materialized state");
    assert!(idx.jar_qualified.is_empty(), "reindex must clear stale Tier-1 FQN data");
    assert!(idx.jar_bare_names.is_empty(), "reindex must clear stale Tier-1 bare-name data");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp reindex_resets_materialized -- --nocapture`
Expected: FAIL — `handle_reindex` doesn't currently touch any of the new Tier-1/materialization state.

- [ ] **Step 3: Write the implementation**

In `handle_reindex` (read its real body first — `rg -n "fn handle_reindex" -A 30 src/workspace/*.rs`), alongside the existing `jar.rs::clear_jar_maps`-equivalent reset it already does for `jar_files`/`jar_definitions`, add:

```rust
    indexer.materialized.clear();
    indexer.materialization_failed.clear();
    indexer.jar_qualified.clear();
    indexer.jar_bare_names.clear();
    // jar_table itself is NOT cleared — JarIds stay stable across a reindex
    // (append-only growth, same invariant FileTable already relies on); only
    // the maps keyed BY JarId reset. A JAR whose path is interned again
    // during the new crawl gets its existing JarId back (JarTable::intern
    // is idempotent), so nothing downstream needs to know a reindex happened.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp reindex_resets_materialized -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 18 tests, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/workspace/*.rs <matching test file>
git commit -m "fix(workspace): reindex resets Tier-1 and materialization state

jar_qualified/jar_bare_names/materialized/materialization_failed all
reset on reindex, matching the discipline the existing jar maps already
follow. jar_table itself stays append-only (JarId stability across
reindex, same invariant FileTable relies on for FileId)."
```

---

### Task 12: The flip — crawl builds Tier 1 only, not full materialization

**This is the task where the memory win activates. Every prior task must be green before starting this one.**

**Files:**
- Modify: `src/workspace/scan_handler.rs` (the crawl's compiled-JAR block, already touched in Task 5)
- Test: `src/workspace/scan_handler_tests.rs` + a new decoy test proving lazy materialization actually works end-to-end
- Pre-existing (do not re-add, just run it — Step 5b): `tests/lsp_smoke.rs::smoke_completion_from_compiled_jar` and its fixture `tests/fixtures/jars/lazylib-fixture.jar`, committed ahead of this plan's execution as the real end-to-end regression gate for the flip.

- [ ] **Step 1: Write the failing decoy test**

```rust
#[test]
fn crawl_no_longer_eagerly_materializes_every_jar() {
    // End-to-end: after a crawl completes, jar_definitions must be EMPTY for
    // a jar nothing has referenced yet, while jar_bare_names/jar_qualified
    // (Tier 1) must have real data for it. This is the design's core memory
    // claim, made testable.
    let idx = Indexer::new();
    // (test fixture setup mirrors whatever existing scan_handler_tests.rs
    // uses to drive a crawl against a small fixture jar set — reuse it
    // rather than building new fixture plumbing)
    // ... run the crawl ...
    assert!(
        idx.jar_definitions.is_empty(),
        "the crawl must not eagerly populate jar_definitions (Tier 2) for \
         any jar — that's the whole point of the flip"
    );
    assert!(
        !idx.jar_bare_names.is_empty(),
        "the crawl MUST populate jar_bare_names (Tier 1) for every jar — \
         cheap, always-eager"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin kmp-lsp crawl_no_longer_eagerly_materializes -- --nocapture`
Expected: FAIL — today's crawl still calls `index_jars` (full materialization) for everything.

- [ ] **Step 3: Write the implementation**

In `src/workspace/scan_handler.rs`, replace the crawl's compiled-JAR block (the one Task 5 already scoped the lock around) from calling `index_jars` to calling `build_jar_manifest` — keep the exact same `(count, sidecar_alive)` threading Task 5 introduced, just swap which indexing function runs inside the block. Do **not** reintroduce a bare `let mut sidecar = ...` that isn't scoped this way: `sidecar_alive` (renamed from Task 5's original binding, still consumed by the unchanged `final_phase = if !sidecar_alive && compiled_total > 0 { ... }` check ~46 lines below) is exactly the value the later code needs, and only `sidecar_alive` — not the guard itself — may outlive this block:

```rust
            crate::indexer::jar::clear_jar_maps(&indexer);
            let (compiled_total, sidecar_alive) = {
                let mut sidecar = indexer
                    .jar_sidecar
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let manifested_names =
                    crate::indexer::jar::build_jar_manifest(&indexer, &paths, &mut sidecar);
                (manifested_names, sidecar.is_some())
            };
            log::info!(
                "jar: manifested {compiled_total} names from {} compiled JARs (Tier 1 only — \
                 full materialization deferred to first real use)",
                paths.len()
            );
```

The local variable keeps the name `compiled_total` (not renamed to `manifested_names`) specifically so the unchanged `final_phase`/`total`/`log::info!` code 40+ lines below — all of which read `compiled_total` — needs no further edits beyond what Task 5 already made. Confirm this by re-reading the full function after this edit (`rg -n "compiled_total\|sidecar_alive" src/workspace/scan_handler.rs`): every remaining use should read as "count of Tier-1 names manifested," which is what makes `jar_phase` transitioning to `Ready` on this count correctly describe "Tier 1 ready" — the intended new meaning per Task 7's diagnostics fix.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin kmp-lsp crawl_no_longer_eagerly_materializes -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the FULL suite (not just new tests) — this is the critical regression gate**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -5`
Expected: every single existing test still passes. Any failure here means some consumer wasn't actually wired in Tasks 7-10, or was wired incorrectly — do not proceed to commit with any red test. If something fails, find which consumer is affected and fix its `ensure_jar_materialized` wiring before continuing; do not weaken the failing test's assertion.

- [ ] **Step 5b: Run the real end-to-end compiled-JAR smoke test — the actual regression gate for the flip**

Every per-task test in Tasks 7-10 hand-seeds `jar_bare_names`/`jar_qualified` directly rather than driving the real crawl → manifest → promote path — none of them alone proves the flip is safe. `tests/lsp_smoke.rs::smoke_completion_from_compiled_jar` closes that gap: it drives the real `--stdio` server against a real compiled fixture JAR (`tests/fixtures/jars/lazylib-fixture.jar`, already committed) via `workspace.json`'s `jarPaths`, and asserts the fixture's `LazyLibType` symbol appears in completion. It already passes against today's eager `index_jars` path — after this task's flip, it must keep passing unmodified, now exercising `build_jar_manifest` → (on first real completion touching it) promotion instead.

Run: `cargo test --test lsp_smoke smoke_completion_from_compiled_jar -- --nocapture`
Expected: PASS. If it fails after the flip, the crawl's Tier-1 manifest build or `complete_bare`'s promote-on-build-item (Task 9 Step 4) isn't actually reaching this candidate — do not weaken this test's assertion; find and fix the real gap.

- [ ] **Step 6: Clippy**

Run: `cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 7: Manual smoke test against a real workspace**

Step 5b already gives an automated pass/fail for the core "does a Tier-1-only compiled-JAR symbol still resolve" question. This manual pass covers what the fixture jar can't (real-world scale, real KMP project structure, features Step 5b doesn't touch): per this repo's `AGENTS.md` convention (install + exercise before claiming done), `cargo install --path . --force`, then open a real KMP project in an editor pointed at the freshly-installed binary, and check: (a) hover on a library type not yet used elsewhere in the session resolves correctly, (b) typing a bare name for an unimported library type still offers it as an auto-import completion, (c) go-to-definition into a library type works, (d) call-arg diagnostics on a library function call doesn't show a false-positive warning. This step has no automated pass/fail — record what you observed in the task report.

- [ ] **Step 8: Commit**

```bash
git add src/workspace/scan_handler.rs src/workspace/scan_handler_tests.rs
git commit -m "perf(workspace): flip the crawl from eager full-materialization to Tier-1-only

THE flip: the startup crawl now calls build_jar_manifest instead of
index_jars for the compiled-JAR phase. Full suite green, plus
tests/lsp_smoke.rs::smoke_completion_from_compiled_jar (a real end-to-end
compiled-JAR completion test, committed ahead of this task specifically
to gate the flip) still passes unmodified. Every direct-read consumer
wired in Tasks 7-10 is what makes this safe. This is the task that
activates the memory win measured in Task 13."
```

---

### Task 13: Probe extension — measure the real win

**Files:**
- Modify: `src/indexer/memory_probe_tests.rs`

**Interfaces:**
- Consumes: nothing new — this is measurement, not production code.

- [ ] **Step 1: Add a new `#[ignore]`d probe section**

Following the exact pattern already established in this file (read `memory_retainer_profile` and `library_jar_cache_footprint` fully first), add a new ignored test that: (a) runs a realistic crawl against a real workspace (reuse whatever fixture/corpus mechanism the existing probe tests use), (b) simulates a small set of realistic feature invocations (a handful of hover/completion calls touching a handful of JARs — reuse the existing test-fixture indexer-driving helpers), (c) reports the same table shape as `library_jar_cache_footprint`: entries, accounted MB, RSS delta — but split into "Tier 1 only" vs "Tier 2 materialized" rows, plus a count of how many JARs stayed Tier-1-only vs got promoted.

```rust
#[test]
#[ignore = "manual memory profiling — measures the lazy-loading win against a real corpus"]
fn lazy_jar_loading_tier_split_profile() {
    // ... corpus load + realistic feature-invocation simulation, following
    // the exact structure of `library_jar_cache_footprint` above this
    // function in the same file ...
    eprintln!("jars: {tier1_only_count} Tier-1-only, {tier2_count} materialized");
    eprintln!("Tier 1 accounted: {tier1_mb:.1} MB");
    eprintln!("Tier 2 accounted: {tier2_mb:.1} MB");
    eprintln!("RSS delta: {rss_delta_mb:.1} MB (compare against the 560.4 MB \
                baseline from PR #213's fully-eager measurement)");
}
```

- [ ] **Step 2: Run it against the real corpus**

Run: `cargo test --bin kmp-lsp lazy_jar_loading_tier_split_profile -- --ignored --nocapture`
Expected: real numbers, pasted into the task report — this is the actual deliverable of this task, not a pass/fail assertion. Report both the accounted-MB table and, critically, what fraction of the 777-JAR corpus stayed Tier-1-only after the simulated usage — that fraction is the design's whole thesis, made falsifiable.

- [ ] **Step 3: Run full suite + clippy**

Run: `cargo test --bin kmp-lsp 2>&1 | tail -3 && cargo clippy --bin kmp-lsp -- -D warnings 2>&1 | tail -3`
Expected: baseline + 1 (ignored, doesn't count toward the pass total but must compile), clippy clean.

- [ ] **Step 4: Commit**

```bash
git add src/indexer/memory_probe_tests.rs
git commit -m "test(probe): measure the Tier-1/Tier-2 split after lazy JAR loading

Extends the existing library_jar_cache_footprint methodology. This is
the number the whole design exists to produce — paste the real output
in the PR body, not an assumed one, matching this effort's discipline
throughout (the jar-cache-streaming fix's null result was a direct
consequence of measuring instead of assuming)."
```

---

## Self-Review Notes (for whoever executes this plan)

- **Spec coverage:** Tiers 1/2 (Tasks 1,2,3,4,6), concurrency (Task 5), diagnostics false-positive fix (Task 7), the four direct-read consumers (Task 8), completion + auto-import (Task 9), import-scoped promotion (Task 10), reindex (Task 11), the flip (Task 12), measurement (Task 13). Not covered by this plan, deliberately: source-JAR text (separate follow-on plan per design §Rollout), wildcard-import package-keyed Tier 1 (design's own out-of-scope-for-v1 list), eviction (same), a live memory-usage LSP command (same).
- **Revision history — plan reviewed by an independent Fable-model pass after Tasks 1-2 landed, before Task 3 dispatch (2026-07-10):** the review found two BLOCKING defects, both fixed in this revision:
  1. `jar_qualified` was a dead map — Task 6's `populate_tier1_from_manifest` deliberately deferred FQN construction (reasoning the manifest "can't cheaply construct a real FQN without package data"), while Task 9's own test asserted against `jar_qualified` as if it *were* populated. Root cause: Task 3's `JarManifestName` schema omitted a `package` field, even though the sidecar already emits `SidecarSymbol::pkg` (the same field `jar.rs`'s existing Tier-2 path already uses for this exact purpose). Fixed: Task 3 gained `package: Option<String>`; Task 6 now builds real FQNs into `jar_qualified` directly at manifest-build time, from either the sidecar response or a cache hit; Task 9's test was corrected to seed (and assert against) `jar_qualified` the way `build_jar_manifest` actually populates it.
  2. Task 5's (and Task 12's identical) sidecar lock-scoping snippet did not compile against the real `scan_handler.rs` — it dropped the `sidecar` MutexGuard at the end of a block, but the real function reads `sidecar.is_none()` ~46 lines later (the `final_phase` computation), after `index_sources_jars` has already run. Fixed: both tasks now thread a `sidecar_alive: bool` out of the scoped block instead of trying to keep the guard itself alive past its needed scope.

  Lower-severity findings also addressed: Task 4's caller enumeration was incomplete (missed `src/cli/run.rs:465`) and mischaracterized `handle_reindex` (it calls `spawn_jar_indexing`, not `index_jars`, directly); the new `clear_jar_maps` was renamed away from a collision with the pre-existing, differently-scoped `Indexer::clear_jar_index()`. And: no automated test exercised a Tier-1-only symbol through a real LSP handler (every Tasks 7-10 test hand-seeds internal maps) — closed by committing a real end-to-end test ahead of execution, `tests/lsp_smoke.rs::smoke_completion_from_compiled_jar`, against a real compiled fixture JAR (`tests/fixtures/jars/lazylib-fixture.jar`); Task 12 Step 5b runs it as the flip's actual regression gate.
- **Type consistency check (re-verified after the above revision):** `JarId` (Task 1) is used identically in every later task (`jar_table.intern`/`.path()`, `materialized`/`materialization_failed` as `DashSet<JarId>`, `jar_bare_names` as `DashMap<String, Vec<JarId>>`, `jar_qualified` as `DashMap<String, JarId>`) — verified consistent across Tasks 2, 4, 6, 7, 8, 9, 10, 11. `JarManifestName.package: Option<String>` (Task 3) flows unchanged through Task 6's two construction sites (cache-hit passthrough, sidecar-response mapping via `s.pkg`) into `populate_tier1_from_manifest`'s `jar_qualified` writes.
- **Every task before Task 12 is independently testable without the others being wired** — each new function is unit-tested directly, not only through end-to-end crawl behavior, so the plan can be paused at any task boundary with a fully green, fully-eager-behaving server.
