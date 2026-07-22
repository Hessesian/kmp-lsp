# Startup Speed Optimization Plan

## Current State
- 41,807 source files across 428 sources JARs
- Total sources indexing: ~18s
- Compiled JARs (sidecar, cached): ~2s
- Workspace scan: ~1s
- **Total startup: ~21s**

## Timing Breakdown
| Phase | Time | % of total |
|-------|------|------------|
| Sequential ZIP extraction | 3.5s | 20% |
| Tree-sitter parsing (4 threads) | 13s | 72% |
| Sequential merge (DashMap inserts) | 1.5s | 8% |
| Stale removal | 21ms | <1% |

## Bottleneck Analysis
The tree-sitter parsing of 41,807 files is the dominant bottleneck at 72% of total time.
This is already parallelized across 4 threads. Per-file parsing takes ~0.3ms which is
near the theoretical minimum for tree-sitter + symbol extraction.

## Optimization Plan

### Phase 1: Sources-JAR Parse Cache (HIGH IMPACT — estimated 10-13s savings)
**Problem**: Sources JARs are re-parsed on every startup, even though they rarely change.
**Solution**: Cache parsed results to disk, keyed by JAR path + mtime + size.
**Implementation**:
- Add `sources_cache.bin` alongside existing `jar_cache.bin`
- Key: `(jar_path, mtime, file_count)` → `Vec<FileContributions>`
- On cache hit: skip extraction + parsing, load contributions directly
- On cache miss: parse normally, save to cache
**Expected speedup**: ~13s (skip parsing entirely for unchanged JARs)

### Phase 2: Parallel ZIP Extraction (MEDIUM IMPACT — estimated 2-3s savings)
**Problem**: ZIP extraction is sequential (428 JARs read one by one).
**Solution**: Extract JARs in parallel using `std::thread::scope`.
**Implementation**:
- Split JAR paths into chunks, one per thread
- Each thread extracts its JARs and produces `(Url, String)` entries
- Collect all entries, then parse in parallel as before
**Expected speedup**: ~2s (extraction is I/O bound, parallelizing helps)

### Phase 3: Skip Private/Internal Symbols (MEDIUM IMPACT — estimated 1-2s savings)
**Problem**: We parse and index all symbols, but only public API is needed for sources JARs.
**Solution**: After parsing, filter out private/internal symbols before indexing.
**Implementation**:
- Add `retain(|s| s.visibility == Visibility::Public)` after parsing
- This reduces symbol count, DashMap insertions, and memory usage
**Expected speedup**: ~1-2s (fewer symbols to process and insert)

### Phase 4: Skip Trivial Files (LOW IMPACT — estimated 0.5s savings)
**Problem**: Many source files contain only private helpers or are empty.
**Solution**: Skip files with no public symbols after filtering.
**Implementation**:
- After filtering, if a file has no public symbols, skip it entirely
**Expected speedup**: ~0.5s

## Combined Expected Result
| Optimization | Savings | New Total |
|-------------|---------|-----------|
| Parse cache (Phase 1) | -13s | ~5s |
| Parallel extraction (Phase 2) | -2s | ~3s |
| Skip private symbols (Phase 3) | -1s | ~2s |
| Skip trivial files (Phase 4) | -0.5s | ~1.5s |

**Target startup time: ~3-5s** (down from ~18s for sources indexing)

## Implementation Order
1. Phase 1 (parse cache) — biggest impact, most complex
2. Phase 2 (parallel extraction) — straightforward
3. Phase 3 (skip private symbols) — easy, builds on Phase 1
4. Phase 4 (skip trivial files) — easy, builds on Phase 3
