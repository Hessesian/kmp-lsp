# Performance

## Benchmarks

Measured on a real Android codebase (12,142 Kotlin files, 51,281 symbols):

| Operation | Time | Notes |
|---|---|---|
| Cold index (parse all files) | ~3s | First run, no cache |
| Warm index (load from cache) | ~90ms | 70 MB bincode file |
| Semantic tokens (CST-only) | **19ms** | Default CLI mode |
| Semantic tokens (with resolve) | ~92ms | `--resolve` flag, loads index |
| `find`/`refs` (rg fallback) | ~50ms | `--fast` mode |
| `find`/`refs` (indexed) | ~100ms | Includes cache load |

**System:** Linux (CachyOS), Rust release build (`opt-level=3`, thin LTO).

## CLI Performance Notes

The `tokens` command defaults to CST-only mode (Phase 1 classification). This is instant
(~20ms) and sufficient for syntax highlighting preview. Use `--resolve` to opt-in to
Phase 2 cross-file resolution, which loads the full workspace index.

For `find`/`refs`, the `--fast` mode uses ripgrep directly and skips cache loading — this
is often faster than the indexed path for simple name lookups.

## Profiling

### Setup

Add the `[profile.profiling]` section to `Cargo.toml` (already present):

```toml
[profile.profiling]
inherits  = "release"
debug     = 2
strip     = false
lto       = false
```

### Using samply (recommended)

```bash
cargo install samply

# Ensure perf events are accessible
echo '1' | sudo tee /proc/sys/kernel/perf_event_paranoid
sudo sysctl kernel.perf_event_mlock_kb=8192

# Build with debug symbols
cargo build --profile profiling

# Profile indexing
rm -rf ~/.cache/kmp-lsp/
cd /path/to/kotlin/project
samply record -- /path/to/target/profiling/kmp-lsp index

# Profile semantic tokens
samply record -- /path/to/target/profiling/kmp-lsp tokens --resolve src/BigFile.kt

# Save profile without opening browser
samply record --save-only -o profile.json -- ...
samply load profile.json  # open later
```

Samply opens Firefox Profiler UI with interactive flamegraph, call tree, and timeline.

## Architecture Hotspots

Based on profiling the indexing pipeline:

1. **Cache deserialization** — bincode deserialization of the 70 MB index dominates startup
   for CLI commands that need the full index. Mitigated by defaulting `tokens` to CST-only.

2. **File discovery** — `fd` subprocess to enumerate workspace files. Fast (~200ms for 12K
   files) but could be skipped with warm-manifest mode when cache is fresh.

3. **Tree-sitter parsing** — parallelized across all cores. Individual file parse is ~0.3ms.
   Bulk parsing 12K files takes ~2.5s thanks to Rayon thread pool.

4. **Semantic token generation** — Phase 1 (CST walk) is O(nodes), ~10ms for 1700-line file.
   Phase 2 (index lookups) adds ~5ms per file when index is already in memory.

## Future Optimization Plan

### Short-term (planned)

- **zstd compression** for the on-disk cache. The 70 MB bincode compresses to ~15 MB with
  zstd level 3. Disk I/O reduction should halve load time on spinning disks and NVMe alike
  (CPU cost of decompression is negligible vs I/O savings).

### Medium-term (considered)

- **rkyv zero-copy deserialization** — replace bincode with [rkyv](https://rkyv.org). Memory-map
  the cache file and access data in-place without heap allocation. "Loading" becomes ~0ms (just
  an mmap syscall). Per-access cost is minimal. This would make indexed CLI commands instant.

- **Per-file cache sharding** — instead of one monolithic blob, store one cache file per source
  file (or per module). Only load what's needed. Benefits:
  - Incremental updates: edit one file → rewrite one entry (not 70 MB)
  - Partial loads: `tokens --resolve` only needs imports of the target file
  - Better filesystem caching: OS page cache can evict cold entries

### Long-term (speculative)

- **Incremental indexing** — on file-change notification, re-parse only the changed file and
  update the in-memory index. Avoids full workspace re-index on every save. Already partially
  implemented for `textDocument/didChange`.

- **Lazy resolution** — for semantic tokens, resolve type references on-demand using a
  lightweight import graph rather than loading the full symbol table. Would allow Phase 2
  without full index load.
