# Sidecar (kmp-jar-indexer) Performance Audit

## Summary
Audit of the Java sidecar integration across `src/sidecar.rs`, `src/indexer/jar.rs`, `src/indexer/jar_cache.rs`, `src/indexer.rs`, `src/workspace/scan_handler.rs`, and `src/indexer/infer/sig.rs`.  Nine issues found (3 high, 4 medium, 2 low).

---

## 1. src/sidecar.rs — Blocking I/O on Hot Path (HIGH)

**File + line**: `src/indexer/jar.rs:99-136` (caller loop), `src/sidecar.rs:103-132` (`index_jar`)

**Issue**: `index_jars()` in jar.rs processes JARs sequentially in a for-loop. Each iteration calls `index_jar()` which does `write_all` + `flush` + `read_line` — a full synchronous I/O round-trip. For a project with 100+ Gradle dependencies, this means 100 sequential RPC calls to the sidecar, each blocking the `spawn_blocking` thread. The sidecar is a single long-lived process capable of handling pipelined requests, but the protocol is strictly request-response (no pipelining).

**Severity**: HIGH — project startup time scales linearly with JAR count. A typical Android project has 80-200 Gradle libs.

**Fix**: Implement batching in the sidecar protocol. Change the JSON request format to accept an array:
```rust
// Current: {"jar":"/path/to/foo.jar"}
// Proposed: {"jars":["/path/to/foo.jar","/bar.jar"]}
```
The sidecar responds with a JSON array of arrays, one per JAR. Then `index_jars` sends up to N JARs per call (e.g., 16 at a time). Even without sidecar changes, send requests without waiting for responses (pipelining), then read responses in order. Change `SidecarHandle::index_jar` to `index_jars(&mut self, paths: &[&Path])` or add a `flush_batch` method. The `BufReader` on stdout already buffers responses — just don't block reading after each write.

---

## 2. src/indexer/jar.rs — Redundant `rebuild_bare_name_cache` (HIGH)

**File + line**: `src/indexer/jar.rs:135` and `src/indexer/apply.rs:866-885`

**Issue**: `index_jars()` calls `indexer.rebuild_bare_name_cache()` once at the end (line 135). However, `rebuild_bare_name_cache()` in apply.rs:866-885 rebuilds the entire bare-name list from scratch: it iterates all `definitions` entries AND all `jar_definitions` entries, clones every key, sorts, and deduplicates. It also calls `rebuild_importable_fqns()` which iterates *every file* and *every top-level symbol*. This is O(definitions + jar_definitions + files × symbols_per_file).

For ~50,000 symbols across 200 JARs, this is significant CPU time. Worse, `index_content()` (called on every file save) also calls `rebuild_bare_name_cache()`. When a user saves a single file, the entire cache is rebuilt.

**Severity**: HIGH — rebuild is called on every file save AND after JAR indexing. In a large project this becomes a noticeable pause on every keystroke.

**Fix**: Make the rebuild incremental. Instead of full rebuilds, maintain a sorted `Vec<String>` with a dirty flag:
```rust
pub(crate) fn rebuild_bare_name_cache(&self) {
    let mut cache = self.bare_name_cache.write().unwrap();
    // Use a dirty flag — only rebuild when actually needed
    if !self.bare_names_dirty.swap(false, Ordering::AcqRel) {
        return;
    }
    // ... existing rebuild logic
}
```
Set `bare_names_dirty` to `true` only in `reset_index_state`, `apply_file_result`, `clear_jar_index`, and after `index_jars()`.  Do NOT set it in `index_content()` — instead, just append/remove the single file's names from the sorted Vec (binary search + insert/remove) and defer a full sort-dedup to when enough mutations accumulate.  Additionally, `rebuild_importable_fqns` should be called only on full resets, not per-file.

---

## 3. src/indexer/jar.rs — O(n²) `retain` on `jar_definitions` (HIGH)

**File + line**: `src/indexer/jar.rs:162-166` (`populate_from_symbols`)

**Issue**: For each JAR being re-indexed (cache miss path), `populate_from_symbols` does:
```rust
indexer.jar_definitions.retain(|_, locs| {
    locs.retain(|l| l.uri != fake_uri);
    !locs.is_empty()
});
```
This iterates **every entry** in `jar_definitions` (potentially 50K+ symbols) and for each, iterates the inner `Vec<Location>` to remove locations matching this JAR's URI. For the N-th JAR processed, this is O(total_symbols × avg_locations_per_symbol). Across 100 JARs, this approaches O(n²).

**Severity**: HIGH — quadratic behavior on JAR count.

**Fix**: Build a reverse index `jar_uri -> Vec<symbol_name>` during population. Then removal is O(symbols_in_this_jar) instead of O(all_symbols):
```rust
// At Indexer struct level, add:
// jar_uri_to_def_names: DashMap<String, Vec<String>>
//
// In populate_from_symbols:
if let Some(names) = indexer.jar_uri_to_def_names.remove(&fake_uri_str) {
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
```

---

## 4. src/indexer/jar_cache.rs — Full Cache Load on Every JAR Index (MEDIUM)

**File + line**: `src/indexer/jar_cache.rs:49-62` (`load_jar_cache`), `src/indexer/jar.rs:95` (call site)

**Issue**: `index_jars()` calls `load_jar_cache()` which reads the entire bincode file, deserializes all entries, and builds a `HashMap<String, JarCacheEntry>`. The cache file contains entries for ALL projects the user has ever opened. For a developer working on multiple Gradle projects, this could be thousands of entries. This happens on every workspace open/reindex.

**Severity**: MEDIUM — grows with aggregate user history, not just the current project. A user with 20 Gradle projects could have 2,000+ cache entries, all deserialized on every open.

**Fix**: Load the cache lazily, or use a more efficient on-disk format. Options:
1. Split the cache into per-project files keyed by Gradle cache hash (but JARs may be shared across projects).
2. Use LMDB/sled for mmap-based access instead of full deserialization. This allows O(1) lookups without loading the entire file.
3. At minimum, load the cache only when sidecar is available (already done implicitly) and use `Arc<HashMap<...>>` + `OnceCell` to share across reindex calls.

---

## 5. src/indexer/jar_cache.rs — Unnecessary `metadata()` for Immutable JARs (MEDIUM)

**File + line**: `src/indexer/jar_cache.rs:103-118` (`cache_entry_is_fresh`), `src/indexer/jar.rs:104`

**Issue**: `cache_entry_is_fresh()` calls `std::fs::metadata()` for every JAR to check mtime and size. Gradle-cached JARs are immutable after download (the filename includes a content hash). The freshness check is always true for existing JARs. Each `metadata()` call is a syscall that adds latency.

For a cold-start with 150 cached JARs, this is 150 unnecessary syscalls.

**Severity**: MEDIUM — 150 syscalls is ~1-5ms on an SSD, but it's wasted work on every open.

**Fix**: Since Gradle cache JAR filenames contain SHA hashes and the files are immutable, consider checking only the filename key instead of metadata. Alternatively, cache `metadata()` results with a short-lived (`Instant::now()` based) cache within the `index_jars` loop so repeated calls for the same path (unlikely but defensive) are avoided. Better: just check that the file *exists* (using the HashMap key as evidence) and skip the metadata call entirely, trusting the filename-based identity. The mtime check was designed for JARs that might be replaced, but Gradle JARs with hashed paths never change. Add a comment documenting this invariant.

---

## 6. src/indexer/infer/sig.rs — O(files) Linear Scan in `find_fun_signature` (MEDIUM)

**File + line**: `src/indexer/infer/sig.rs:169-208` (`find_fun_signature`)

**Issue**: After `resolve_symbol_no_rg` fails, `find_fun_signature` does a full linear scan through ALL files:
```rust
for entry in idx.files.iter() {
    if entry.key() == uri.as_str() { continue; }
    if let Some(sig) = collect_fun_params_text(fn_name, entry.key(), idx) {
        return Some(sig);
    }
}
```
This is O(number_of_files × symbols_per_file). Called on the signature-help hot path (every keystroke when inside parentheses). The `idx.files` DashMap iteration is lock-free but still traverses all entries.

**Severity**: MEDIUM — fires on signature help (every keystroke). For 5,000+ files, this linear scan runs in ~1-3ms which adds up on every keystroke.

**Fix**: Build a `name -> [uri]` reverse index specifically for function/method signatures. When a file is indexed, populate a `DashMap<String, Vec<String>>` mapping function name to URIs containing that function. Then the fallback becomes O(matching_URIs) instead of O(all_URIs):
```rust
// In Indexer:
// pub(crate) fun_uris: DashMap<String, Vec<String>>
// Populated in apply_file_result when a FUNCTION/METHOD symbol is indexed.

// In find_fun_signature:
if let Some(uris) = idx.fun_uris.get(fn_name) {
    for file_uri in uris.iter() {
        if let Some(sig) = collect_fun_params_text(fn_name, file_uri, idx) {
            return Some(sig);
        }
    }
}
```

---

## 7. src/indexer/infer/sig.rs — Double Full-File Symbol Scan in `find_method_params_in_class` (MEDIUM)

**File + line**: `src/indexer/infer/sig.rs:405-444` (`find_method_params_in_class`)

**Issue**: For each definition location of `type_base`, the function first iterates ALL symbols to verify the class exists (`file_data.symbols.iter().any(...)`) and then iterates ALL symbols AGAIN to find matching methods (`for sym in &file_data.symbols { ... }`). Two full passes over potentially thousands of symbols per file.

**Severity**: MEDIUM — called from `InferDeps::find_method_params_text` during type inference. For type-heavy codebases this adds up.

**Fix**: Combine the two passes into one. Since the method search already checks `sym.container.as_deref() != Some(type_base)`, the class-existence check is redundant for files that have the type_base as a definition location. Remove the `has_class` pre-check entirely (the method loop already verifies container membership):
```rust
for loc in locations.iter() {
    let Some(file_data) = idx.files.get(loc.uri.as_str()) else { continue; };
    // The definition map already guarantees type_base exists in this file.
    // Skip the has_class pre-check iteration.
    for sym in &file_data.symbols {
        if sym.name != method_name { continue; }
        // ... rest unchanged
    }
}
```

---

## 8. src/sidecar.rs — `find_java()` Spawns Subprocess at Startup (LOW)

**File + line**: `src/sidecar.rs:143-158` (`find_java`)

**Issue**: `find_java()` calls `Command::new("java").arg("-version").status()` to verify Java is on PATH. This spawns a subprocess (fork+exec) and waits for JVM startup just to check existence. Takes ~30-100ms on a typical system.

**Severity**: LOW — only happens once during `Indexer::new()`. But it's called regardless of whether a JAR launch path is needed (the native binary path is tried first). If the native binary exists, the `java -version` call is wasted.

**Fix**: Check PATH using `which` crate or iterate `$PATH` manually instead of spawning `java -version`. Rust's `std::env::split_paths` can enumerate PATH entries; just check if `$PATH_entry/java` exists:
```rust
fn find_java() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(home).join("bin").join("java");
        if candidate.exists() { return Some(candidate); }
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join("java");
                candidate.exists().then_some(candidate)
            })
        })
}
```
This avoids subprocess overhead entirely. Also, only call `find_java()` when actually falling through to `launch_jar`.

---

## 9. src/workspace/scan_handler.rs — Eager JAR Re-indexing on Reindex (LOW)

**File + line**: `src/workspace/scan_handler.rs:92-94` (`handle_reindex`), `scan_handler.rs:230-248` (`spawn_jar_indexing`)

**Issue**: `handle_reindex` unconditionally calls `spawn_jar_indexing()` which re-scans the entire Gradle cache and re-indexes all JARs. JAR symbols survive `reset_index_state()` (they're in separate maps). The re-indexing will:
1. Scan the Gradle cache filesystem again (directory walk).
2. For each JAR, check the disk cache, then call the sidecar if stale.
3. Call `rebuild_bare_name_cache()` at the end.

Unless the user has actually added/removed dependencies, this entire process is redundant — the JAR index is still valid.

**Severity**: LOW — reindex is user-initiated and infrequent. But for a large project, re-scanning Gradle cache takes ~500ms-2s of unnecessary work.

**Fix**: Track a "JAR index generation" alongside the workspace generation. Only re-index JARs when the generation has changed (i.e., when `handle_initialize` or `handle_change_root` fires, not `handle_reindex`). The `handle_reindex` call already has a comment acknowledging this: "JAR symbols survive reindex (separate maps), but re-scan in case new dependencies were added". Add a lightweight check: hash the directory listing of the Gradle cache root and only re-scan if it changed.

---

## Appendix: Verified Non-Issues

1. **Sidecar handle is correctly shared**: `jar_sidecar` lives in `Indexer` as `Mutex<Option<SidecarHandle>>` — one per Indexer, shared across the session. Not recreated per request. ✓
2. **Cache TTL is appropriate**: The on-disk cache in `jar_cache.rs` uses mtime+size validation. Since Gradle JARs are content-addressed, this is effectively permanent. No TTL issue. ✓
3. **`split_params_at_depth_zero` is efficient**: Uses byte-index iteration with O(n) time and zero allocations beyond the output Vec. ✓
4. **`SidecarHandle::try_launch()` is called once**: Called from `Indexer::new()`, which is called once per LSP session. Not per-request. ✓
5. **`scan_gradle_jars` is correctly deduplicated**: Uses `(group, artifact, latest-version)` to avoid indexing multiple versions. ✓
