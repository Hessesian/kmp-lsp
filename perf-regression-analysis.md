# CLI Completion Regression Analysis: `source_paths_outside_workspace_appear_in_completion`

## Summary

The regression is caused by the "Incremental rebuild_bare_name_cache" optimization. The `LibraryBatch::flush_into` method (used in the library cache fast path) inserts library definitions directly into the Indexer's DashMaps without setting the `bare_names_dirty` flag. This causes `rebuild_bare_name_cache()` to be a no-op when called immediately after, leaving library symbols out of `bare_name_cache`.

## Exact Code Path

### Test scenario

The test `source_paths_outside_workspace_appear_in_completion` (tests/cli_complete.rs:188-236):

1. Creates a temp workspace with `src/Screen.kt` referencing `LibraryTestClass`
2. Creates a temp library dir with `testlib/LibraryTestClass.kt` (package `com.kotlinlsp.testlib`)
3. Writes `workspace.json` with `sourcePaths` pointing to the external lib dir
4. Runs `kotlin-lsp index --root <workspace>` (process 1)
5. Runs `kotlin-lsp complete <Screen.kt> 3 16 --json --root <workspace>` (process 2)
6. Gets `[]` instead of completions including "LibraryTestClass"

### Process 1: `kotlin-lsp index` (works correctly)

1. `build_index_inner` (run.rs:151) → `index_workspace_full` (scan.rs:679)
2. `index_workspace_impl` → `finalize_workspace_scan` (scan.rs:891)
3. `apply_workspace_result` (apply.rs:526):
   - `reset_index_state()` sets `bare_names_dirty = true` (apply.rs:537)
   - Applies workspace file contributions
   - `rebuild_bare_name_cache()` runs successfully (dirty was true), includes workspace-only symbols (line 541)
4. `index_source_paths` (apply.rs:560):
   - No library manifest exists yet → slow path → `scan_source_paths_slow` → `apply_source_path_scan` (line 777)
   - `apply_contributions` (line 813) sets `bare_names_dirty = true` (line 865) for each file
   - `rebuild_bare_name_cache()` runs successfully (line 790), includes library symbols ✓
   - `save_library_cache` persists library to disk (line 798)

### Process 2: `kotlin-lsp complete` (BROKEN)

1. `build_index_inner` → `index_workspace_full` → `index_workspace_impl`
2. `prepare_scan` (scan.rs:276) → `partition_cache_hits` → all workspace files are cache hits
3. `finalize_workspace_scan`:
   - `apply_workspace_result` (apply.rs:526):
     - `reset_index_state()` sets `bare_names_dirty = true` (line 537)
     - Applies workspace file contributions (from cache)
     - `rebuild_bare_name_cache()` runs, sets `bare_names_dirty = false` (line 541)
     - `bare_name_cache` now has **workspace-only** symbols
4. `index_source_paths` (apply.rs:560):
   - Library manifest exists and is fresh → **fast path** (line 666-672)
   - `restore_library_chunk` → `LibraryBatch::flush_into` (line 390):
     - Directly inserts into `definitions`, `files`, `packages`, etc. DashMaps
     - **Does NOT set `bare_names_dirty = true`** ← THE BUG
   - `rebuild_bare_name_cache()` called at line 671
   - `bare_names_dirty` is `false` → **returns early without rebuilding** ← BUG
   - Library symbols are **NOT** in `bare_name_cache`

5. `completions_at` (complete.rs:12) → `indexer.completions` → `run_completions` (features/completion.rs:116) → `complete_symbol_with_context` → `complete_bare` (resolver/complete.rs:1292) → `BareCompletionWalk::new` → `collect_cross_package` (resolver/complete.rs:1040):
   - `ensure_bare_names_fresh()` checks `bare_names_dirty` → false → no-op (line 1052)
   - Reads `bare_name_cache` → only workspace symbols, no "LibraryTestClass"
   - Returns empty result

## Root Cause

**File:** `src/indexer/apply.rs`, line 668-672

```rust
for chunk in all_chunks {
    self.restore_library_chunk(chunk, &workspace_root);
}
self.rebuild_bare_name_cache();  // <-- no-op because bare_names_dirty is false
```

`LibraryBatch::flush_into` (apply.rs:390-420) directly manipulates DashMaps (`definitions`, `files`, `packages`, `qualified`, `subtypes`, `extension_by_receiver`, `library_uris`) without calling `apply_contributions`. It therefore **omits** the dirty flag update that `apply_contributions` performs at line 865:

```rust
// Line 864-865 in apply.rs (inside apply_contributions):
// Definitions were modified — mark bare_name_cache as stale.
self.bare_names_dirty.store(true, Ordering::Release);
```

Since the previous `apply_workspace_result` call just ran `rebuild_bare_name_cache()` (which does `bare_names_dirty.swap(false, ...)`), the dirty flag is `false` when the fast path reaches `rebuild_bare_name_cache()` at line 671. The rebuild is skipped entirely.

The slow path (`apply_source_path_scan`, line 777) is NOT affected because it calls `apply_contributions` (which sets the dirty flag) before calling `rebuild_bare_name_cache`.

## Why the jar path is NOT affected

`index_jars` (jar.rs:96) calls `populate_from_symbols` for each jar and then explicitly sets the dirty flag at line 165:

```rust
indexer.bare_names_dirty.store(true, std::sync::atomic::Ordering::Release);
```

## Fix

Add `self.bare_names_dirty.store(true, Ordering::Release)` before the `rebuild_bare_name_cache()` call in the fast path:

**File:** `src/indexer/apply.rs`, line 670 (insert before `self.rebuild_bare_name_cache();`)

```rust
// In index_source_paths, after the fast-path loop (line 666-672):
let loaded_chunks = all_chunks.len();
log::debug!("Library cache fresh: restoring {loaded_chunks} chunks without re-scanning");
for chunk in all_chunks {
    self.restore_library_chunk(chunk, &workspace_root);
}
self.bare_names_dirty.store(true, Ordering::Release);  // <-- ADD THIS LINE
self.rebuild_bare_name_cache();
```

Alternative fix: Add `indexer.bare_names_dirty.store(true, Ordering::Release)` at the end of `LibraryBatch::flush_into` (line 419), which would cover any future caller of `flush_into` as well. Either fix is correct; the first is more localized to the regression.

## Verification

After applying the fix:
1. Process 1 (`index`): slow path → `apply_contributions` sets dirty → rebuild works ✓ (unchanged)
2. Process 2 (`complete`): fast path → dirty flag explicitly set → rebuild works ✓ (fixed)
3. `bare_name_cache` now includes library symbols → `collect_cross_package` finds "LibraryTestClass" → test passes
