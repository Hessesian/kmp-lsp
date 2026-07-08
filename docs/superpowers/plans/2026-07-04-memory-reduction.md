# Memory Reduction Plan — retainer-ranked, probe-verified

2026-07-04. Fable-authored analysis; execution designed for post-Fable (Opus) sessions.
Companion evidence: the `memory_retainer_profile` probe (branch `perf/memory-probe`, an
`#[ignore]`d test that walks the loaded `Indexer` and prints an attribution table). The probe is
BOTH the ranking evidence and the permanent before/after meter — every fix below re-runs it and
records the delta. Numbers section at the bottom gets filled from the probe report.

## Problem

Excessive RAM after indexing a real KMP workspace (eager sources-JAR pipeline indexes every
library source file into the same maps as workspace files). On-disk cache ~70 MB bincode expands
multi-fold in RAM; ~/.cache/kmp-lsp holds 1.4 GB across workspaces on this machine.

## CORRECTION (2026-07-08, Task F1a) — the probe has never measured library memory

Investigating the "library_uris empty after warm load" finding (originally read as a bug) found
**no bug**: `LibraryBatch`/`restore_library_chunk` correctly populates `library_uris` and has since
commit e46e4b9, well before this plan. The real issue is probe SCOPE — `memory_retainer_profile`
loads only `index.bin`, and `save_cache` deliberately EXCLUDES library files from it
(cache.rs:225-228, "Skip library-source files — re-indexed from sourcePaths on each startup").
Library sources live in a SEPARATE on-disk cache, `~/.cache/kmp-lsp/library-<hash>-chunks/`
(confirmed present on this machine), loaded only via `restore_library_chunk` — a path the probe
never drives.

**Consequence:** every number in the baseline table below is 100% WORKSPACE data. The "R1: files:
line text 48.5 MB" retainer is NOT library source — it's the lines of the 12,621 workspace files
actually open/editable in this corpus, which is NOT an eviction candidate (editing needs the
lines). **F1 as originally scoped (evict library lines) has not yet been measured against real
library data and its target size is currently unknown** — do not execute F1 as written below
until the probe is extended to also load a real `library-*-chunks/` directory (env override
`KMP_LSP_PROFILE_LIBRARY_CACHE=<dir>` or similar) and produces a workspace-vs-library split with
real library entries. That extension is the correct next task, not F1's eviction itself.
The F3 interning work (peak 487→274 MB) is UNAFFECTED by this correction — it measured real
per-entry `Location`/`Url` duplication in `qualified`/`definitions`/`subtypes`, which holds
regardless of workspace-vs-library composition.

## Retainer inventory (code-verified, post-triad-collapse @ cc583eb)

| # | Retainer | Shape | Mechanism |
|---|----------|-------|-----------|
| R1 | `Indexer.files` → `FileData.lines` | `Arc<Vec<String>>` per file (types.rs:296) | FULL source text resident for every indexed file — workspace AND library (`library_uris` marks the split). One heap alloc per line + 24 B String header + 24 B Vec slot. |
| R2 | `FileData.symbols` → `SymbolEntry` | `name`, `detail`, `params` Strings per symbol (types.rs:147-167) | Library symbols (Compose!) carry huge `detail`/`params` signatures; retained even though library symbols are mostly touched via completion/hover paths that could re-derive. |
| R3 | `definitions: DashMap<String, Vec<Location>>` | tower_lsp `Location` = parsed `Url` + Range | The file URI string is duplicated ONCE PER SYMBOL (a 5 000-file library corpus with 40 symbols/file = 200k Url allocations of ~100+ B each). Same pattern in `qualified`, `subtypes`, `packages`. |
| R4 | jar side: `jar_definitions` / `jar_files` / `jar_symbol_packages` | compiled-JAR symbol tables + side tables | Sidecar-enriched entries (v8 cache) now carry pkg/params/detail — more strings than pre-enrichment. |
| R5 | derived caches: `importable_fqns`, `bare_name_cache`, `completion_cache`, `this_ext_ancestor_cache` | rebuilt copies of names already in R2/R3 | Third copy of every public name. |

## Fixes, ranked (re-rank after probe numbers land)

### F1 — Evict library `lines` from RAM (predicted top win; LOW risk)
The triad collapse made this possible: every consumer now reaches text through
`live_doc_or_parse`, which already falls back `live_lines → files.lines → std::fs::read`
(live_tree_impl.rs:39). For `library_uris` files (never open in the editor, on disk under
~/.cache/kmp-lsp extraction dirs), drop the resident lines after symbol extraction.
**Type-driven shape:** `FileData.lines` becomes
`enum SourceText { Resident(Arc<Vec<String>>), Evicted }` (or `Evicted { path }`), so every
consumer is forced by the compiler to decide, instead of silently getting an empty Vec.
`mem_lines_for` returns `None` for evicted → CST consumers transparently disk-fallback (the
exact mechanism Task 5/6 proved); hover `word_at()` on a library file does one on-demand read
(AllowIo path — hover already may block per the unified-resolution IoPolicy design).
Decoy-first: hover + go-def on a library symbol whose lines are evicted must still work.
Meter: probe `files/library lines` row → target ≈ 0.

### F2 — Strip/defer library `detail`+`params` (MED risk)
Options in order of preference: (a) keep only for `top_level` public symbols, re-derive members
on demand from the extracted source via the CST (`live_doc_or_parse` again); (b) store interned/
compressed. Watch the call-arg-diagnostics dependency on `params_from_detail` counts — counts
`(u8,u8)` stay resident (they're 2 bytes), only the STRINGS go.
Meter: probe `symbols detail/params` rows.

### F3 — URI interning / file-ids (HIGH yield, HIGH ripple — design task)
Replace per-symbol `Location` with `{ file_id: u32, range }` + a `Vec<Arc<Url>>` side table;
`definitions`/`qualified`/`subtypes`/`packages` all shrink. This is catalogue territory — do it
as part of unified-resolution slice 5 (type-driven sweep) where `ResolvedSymbol` already wants a
richer shape, NOT as a standalone find-replace. Requires the probe's R3 number to justify.

### F4 — rkyv/mmap library chunks (already on performance.md roadmap)
Orthogonal to F1-F3; turns resident jar/library symbol data into mmap-backed pages the OS can
evict. Bigger infrastructure change; only if R4 dominates after F1/F2.

### F5 — derived-cache dedup
Only worth it after F3 (interning makes these cheap automatically). Do not start here.

## Execution protocol (post-Fable sessions)

1. Merge/keep the probe branch; run baseline, paste table below.
2. F1 as one task (worktree, decoy-first, probe delta in the PR body). Expected to be
   Task-5-shaped: re-route → verify → evict → suite green.
3. F2 second, F3 folded into slice 5's design, F4/F5 re-evaluated on the new baseline.
4. Every memory PR pastes the probe table before/after — no "should be smaller" claims.
5. GOTCHA from the triad collapse: text you delete may have a SECOND job (broken-syntax
   resilience taught us). For F1: `lines` also feed `word_at()`, identifier sets, and the
   resolver's `infer_lines` — grep every `FileData.lines` / `mem_lines_for` consumer and
   classify BEFORE evicting; the probe's library/workspace split tells you which consumers
   ever see library URIs.

## Baseline numbers (probe run 2026-07-05, branch `perf/memory-probe`)

Corpus `~/.cache/kmp-lsp/7b61330400b927bb`, index.bin = 117 MB on disk. 12,621 files,
155,909 symbols, 242,467 `qualified` entries. Struct sizes: FileData=256 B, SymbolEntry=240 B,
Location=104 B.

| retainer | entries | MB | % acct |
|---|---|---|---|
| files: line text | 12,621 | 48.5 | 18.0% |
| qualified: Location URIs | 242,467 | 36.0 | 13.4% |
| files: symbol structs (fixed 240 B) | 155,909 | 36.0 | 13.3% |
| definitions: Vec buffers | 52,868 | 33.6 | 12.5% |
| qualified: keys (FQN strings) | 242,467 | 24.6 | 9.1% |
| definitions: Location URIs | 155,909 | 23.1 | 8.6% |
| files: imports+package | 12,621 | 17.9 | 6.6% |
| files: symbol .detail | 155,909 | 9.1 | 3.4% |
| (long tail) | | ~40.7 | 15.1% |
| **TOTAL accounted** | | **269.6** | 100% |

RSS: 9.6 MB before → **486.9 MB warm-load PEAK** (apply holds result vec + full Indexer copy
simultaneously; jemalloc retains the freed transients) → steady accounted 269.6 MB.
Unaccounted 207.7 MB ≈ that transient retention + DashMap shard overhead.

## DATA-DRIVEN RE-RANKING (supersedes the predicted order above)

1. **F0 (NEW): kill the 2× warm-load peak.** The apply path clones instead of moving — result vec
   + Indexer copy live at once; jemalloc keeps the slack. Cheap, halves perceived RSS on startup.
   Meter: "RSS apply peak" line.
2. **F3 is #1 structural** (was ranked third): the URI/FQN duplication cluster —
   qualified URIs 36.0 + qualified keys 24.6 + definitions Vec buffers 33.6 + definitions URIs
   23.1 + subtypes 5.1 + definitions keys 2.2 ≈ **125 MB ≈ 46% of accounted**. `qualified` alone
   holds 242k parsed `Url`s + 242k FQN Strings. File-id interning + compact `{file_id, range}`
   locations + FQN prefix-sharing land here. Still slice-5-shaped; now the top prize.
3. **F1 (evict lines): 48.5 MB**, still worthwhile and lowest-risk — BUT the probe found
   **`library_uris` is EMPTY after a warm cache load** (12,621/12,621 classified workspace, which
   is wrong for this corpus): library classification is not persisted/restored. F1 must first fix
   that (persist the flag in the cache, or gate eviction by path: outside workspace root / under
   the extraction dirs).
4. **SymbolEntry diet (NEW, part of F2)**: 240 B fixed × 156k = 36 MB before any string content;
   several always-present String headers + two Ranges. Boxing rare fields / narrowing spans is
   worth a look inside the slice-5 sweep.
5. F2 strings (.detail 9.1 + .params 4.0 + other 5.9 ≈ 19 MB) and F4/F5: after the above.
