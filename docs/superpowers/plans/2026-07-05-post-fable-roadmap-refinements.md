# Post-Fable Roadmap Refinements

2026-07-05, final Fable pass. Upgrades every remaining planned item with the design decisions and
trap lists that are expensive to re-derive. Execution: Opus sessions, subagent-driven, probe-metered
(memory items) and decoy-gated (behavior items). Ordering + dependency graph at the bottom.

## 1. F3 — file-id interning (the 125 MB / 46% prize) — TARGET DESIGN

**Target types** (new, in types.rs; names final unless the code disagrees):
```rust
pub(crate) struct FileId(u32);                    // index into FileTable
pub(crate) struct FileTable {                     // append-only; RwLock<Vec<Arc<Url>>> + DashMap<String, FileId>
    by_id: RwLock<Vec<Arc<Url>>>,
    by_uri: DashMap<String, FileId>,
}
pub(crate) struct SymbolLoc { file: FileId, range: Range }   // 4 + 16 = 20 B vs Location's 104 B + Url heap
```
- `definitions: DashMap<String, Vec<SymbolLoc>>`, same for `qualified`, `subtypes`, `packages`
  (packages values become `Vec<FileId>`).
- Conversion to `tower_lsp::Location` happens ONLY at the LSP boundary (one helper:
  `FileTable::location(&self, SymbolLoc) -> Location`); grep every `.uri` touch on map values and
  push the conversion outward until the maps never surface `Location`.
- **Do it as slice 5's first task**, not standalone: the catalogue's `ResolvedSymbol` should carry
  `SymbolLoc` natively so the sweep and the interning are one migration, not two.

**Migration order** (each step suite-green): (1) introduce FileTable + FileId, populate during
index_content, no consumers; (2) migrate `qualified` (biggest single win, 60.6 MB keys+URIs, and it
has the FEWEST consumers — verify with find_referencing_symbols); (3) `definitions` + the
`Vec<Location>` buffers; (4) `subtypes`/`packages`; (5) cache format bump (see below); (6) delete
the last map-side `Location` uses.

**Traps:**
- **Cache format**: index.bin serializes these maps. Bump to v9; serialize FileTable once + numeric
  SymbolLocs (this SHRINKS the 117 MB index.bin substantially — expect faster warm load too, which
  interacts with F0's peak fix: do F0 FIRST so before/after attribution stays clean).
- **reset_index_state / reindex**: FileTable is append-only per session; on workspace reindex either
  rebuild it or accept growth (bounded by file count — fine). Decide in-code with a comment-free
  typed choice: `FileTable::rebuild()` called exactly where files.clear() happens.
- **FQN keys (24.6 MB)**: do NOT attempt prefix-compression in the same pass (rope/trie is a
  different risk class). A cheap follow-on: `qualified` keys share package prefixes with
  `packages` keys — measure again after interning before deciding.
- **library_uris**: becomes `DashSet<FileId>` in step 4 — which also forces fixing its warm-load
  restoration (see item 3).

## 2. F0 — kill the 2× warm-load peak

The probe proved apply holds the deserialized result AND an Indexer copy simultaneously
(RSS 487 MB vs 270 MB steady). Refinements:
- Fix shape: make the apply path consume `self`/move the loaded maps INTO the live Indexer
  (mem::replace per-field, or build-then-swap the Arc'd Indexer if the server holds `Arc<Indexer>`
  — check how backend holds it; if `Arc<Indexer>` with interior DashMaps, per-field
  `clear()+extend(drain)` avoids the second copy without swapping the Arc).
- jemalloc retention makes RSS a bad meter for THIS fix — the probe's "apply peak" checkpoint is
  the meter, not steady RSS.
- Task-shape: single Opus task, LOW risk, do before F3 (clean attribution) and ship with probe
  before/after in the PR body.

## 3. F1 — evict library lines (48.5 MB) + the library_uris warm-load bug

- FIRST fix classification: probe showed `library_uris` EMPTY after warm cache load → either
  persist it in the cache (v9 rides with F3's bump — coordinate!) or re-derive on load
  (path outside workspace_root || under jar-extraction dirs). Prefer re-derive: no format
  coupling, one function, testable without a cache fixture.
- Eviction gate: `SourceText::{Resident(Arc<Vec<String>>), Evicted}` on FileData — compiler forces
  every consumer to decide. Consumers audit BEFORE evicting (triad-collapse lesson: deleted text
  can have a second job): `word_at`, identifier sets (built at index time — keep, they're already
  extracted), `mem_lines_for` (None for evicted → live_doc_or_parse disk fallback, proven
  mechanism), resolver `infer_lines` (string path — library URIs reachable? classify).
- Decoys: hover + go-def + completion on a library symbol with evicted lines; and the
  broken-syntax question does NOT apply (library files aren't being edited).

## 4. Slice 4 — chain-walk collapse (chain.rs) — DECISION RULES

- Scope: collapse the remaining text-shaped chain inference onto the CST walk; entry points
  `resolve_callee_chain` / `cst_forward_resolve_receiver_type` are already node-native after
  Task 8 — the remaining text is the string-path side (`resolver/infer_lines.rs`,
  `uppercase_dotted_type_prefix` heuristics). RULE: the string path (resolver/) stays heuristic
  BY DESIGN (locked decision) — slice 4 collapses only `indexer/infer/chain.rs` internals. If a
  chain.rs function's only callers are resolver-side, it MOVES there, not deletes.
- Carry-ins from the wave follow-ups: hoist brace repair to the shared tree-acquisition seam
  (scope walk + named-param path still unrepaired — `lambda_resolution_doc_at` is the house
  mechanism, generalize to `CstQuery` level so ALL features get mid-typing resilience); drop the
  vestigial `_lines` param (~40 sites, mechanical); stale "text path's" comment chain.rs:278;
  nested-generic-`it` completion test; EOF remap unbalanced-brace gate refinement.
- Trap: generics substitution (`type_subst`) is the wrong-answer factory — every step needs a
  decoy asserting a CONCRETE type, and `assert_ne!` against the bare parameter name (house
  pattern from the `regression_*_not_t` family).

## 5. Slice 5 — type-driven sweep — now has three tenants

Order inside the slice: (a) F3 interning (item 1 — it IS the data-model sweep), (b) SymbolEntry
diet (240 B × 156k: Box rare fields — `type_params: Vec<String>` and `detail`/`params` are
empty for most locals; consider `Option<Box<SymbolDetail>>` for the cold half; re-measure before
committing to layout churn), (c) exhaustive `CstExpr` dispatch from the original design doc.
RULE: (b) only after (a) — interning changes what's hot.

## 6. Slice 6 — CST-aware navigation — DESIGN SKETCH (was: "write design while Fable available")

The budget went to memory instead; here is the compressed sketch to expand later:
- Generalize the existing `CursorContext`→CST bridge (used by completion) into
  `SymbolAtCursor { name, kind_hint, receiver_chain, definition_site_hint }` produced by one CST
  classification of the cursor node (declaration? reference? import segment? string?).
- go-def/find-refs/rename/highlight consume it and fall back to today's name-based scan ONLY via
  a typed `NavigationSource::{CstResolved, NameScan}` so results can be ranked and the fallback
  is visible in code, not implied.
- Rename is the risk king: it must REFUSE (typed refusal, not empty result) when the CST
  classification is ambiguous — never text-rename on a NameScan source.
- Precondition: none on memory work; can run parallel to slices 4/5 in a separate worktree.

## 7. Small debts (single-task, any session)

- `bare_this_type.or_else(enclosing_class)` can undo InsideReceiver's intentional None
  (Copilot on #203, verified plausible): fix = only fall back when ThisContext::NotFound;
  decoy: `unknown.apply { this. }` must NOT complete enclosing-class members.
- `scan_handler.rs` 3× `lock().unwrap()` + `source_paths_raw.read().unwrap()` + semaphore
  `.expect` (pre-existing, hook-flagged twice): one mechanical pass; poisoned-lock policy =
  match the repo's existing non-unwrap Mutex handling (grep first — if none exists, `.lock()
  .unwrap_or_else(|poisoned| poisoned.into_inner())` with a one-line WHY).
- GH issue #147 (args.rs text-scan consolidation onto CST) — same shape as Tasks 5/6; check the
  broken-syntax second-job question there too.

## Dependency graph / suggested order

```
F0 (peak) ──► F3+slice5(a) interning ──► cache v9 ──► SymbolEntry diet (5b)
library_uris re-derive ──► F1 evict lines          (independent of F0/F3, coordinate cache bump)
slice 4 (chain + repair-seam hoist + small carry-ins)   — independent track
slice 6 (navigation)                                     — independent track
small debts (item 7)                                     — fillers between tasks
```
Every memory PR: probe table before/after in the body. Every behavior PR: decoy-first, never
weaken an assertion, Serena directive in every dispatch (worktree activate_project caveat).
