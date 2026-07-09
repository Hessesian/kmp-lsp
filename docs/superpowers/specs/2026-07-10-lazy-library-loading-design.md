# Lazy Library Loading — Design

2026-07-10. Follows the memory-reduction effort (docs/superpowers/plans/2026-07-04-memory-
reduction.md, 2026-07-05-post-fable-roadmap-refinements.md). That effort optimized workspace-side
memory (487 → 270.5 MB peak on a 117 MB reference corpus) before discovering, via real on-disk
caches from an actual session, that library data dwarfs it: the compiled-JAR symbol cache alone
measures 560 MB resident RSS (2.2× disk-to-RAM expansion, 1.45M symbols across 777 JARs) — more
than double the entire workspace-side optimization target — and the source-JAR text cache (1.06 GB
on disk, likely larger once decoded) remains unmeasured. See PR #213/#214 for the measurement.

## Problem

`workspace/scan_handler.rs` eagerly indexes **every** JAR discovered in the Gradle cache at scan
time — both compiled symbols (via the `kmp-jar-indexer` sidecar) and extracted source text (via
tree-sitter parsing) — regardless of whether the workspace actually references anything in a given
JAR. This is not scoped to imports; it processes the full transitive Gradle-cache JAR set. For a
real Android/KMP project this means materializing full `SymbolEntry`/`FileData` for symbols never
looked up in the session.

## Prior art consulted

- **Android Studio / IntelliJ platform:** project loaded eagerly; libraries outside declared
  imports resolve lazily when local resolution comes up short.
- **rust-analyzer** (cloned and read directly — `crates/hir-def/src/item_tree.rs`,
  `crates/hir-def/src/nameres.rs`, `crates/base-db/src/change.rs`): `file_item_tree` and
  `crate_def_map` are `#[salsa_macros::tracked]` queries, keyed per-file/per-crate — computed once,
  on first access, memoized; never touched if never queried. `SourceRoot.is_library` drives
  `Durability::HIGH` for library source roots in salsa's invalidation-tracking, but that's a
  separate concern from the laziness itself (salsa's core property: nothing is computed until a
  query asks for it).
- **The key simplification this codebase gets for free:** salsa's complexity is invalidation
  tracking (a library's source could theoretically change under path dependencies). kmp-lsp's JARs
  are immutable after download (`jar_cache.rs`'s own doc comment already asserts this and keys
  cache freshness on `(mtime, size)`). This means kmp-lsp only needs the simple half of what salsa
  does: **compute on first use, cache forever, never invalidate** — not a general incremental-
  computation engine.

## Design: two tiers, one mechanism, both existing subsystems

The compiled-JAR cache (`jar-symbols-vN.bin`, sidecar-driven) and the source-JAR cache
(`sources-jar-v2-cN.bin`, parse-driven) already share the same shape: `HashMap<jar_path, Entry>`,
freshness-checked by `(mtime, size)`, both merging into the same live-Indexer maps (`qualified`,
`definitions`, `jar_definitions`) via the same `LibraryBatch`-style apply path. The natural unit of
laziness is **per-JAR**, and both subsystems already key on it — one mechanism covers both; there
is no need to sequence "which cache first" as a design fork. Staging becomes an implementation
question (§Rollout), not an architectural one.

### Tier 1 — always eager, deliberately cheap

A coarse per-JAR manifest, reusing the interning pattern already proven in PRs #206/#208/#209/#210
(`FileId`/`FileTable`) applied to JAR paths instead of file URIs:

```rust
pub(crate) struct JarId(u32);
pub(crate) struct JarTable { /* identical shape to FileTable: RwLock<Vec<...>> + DashMap intern */ }
```

Two new maps, shaped exactly like the existing `qualified`/`definitions` so the resolver already
knows how to query them:

```rust
jar_qualified: DashMap<String /* FQN */, JarId>
jar_bare_names: DashMap<String /* short name */, Vec<JarId>>   // for imports/wildcards
materialized: DashSet<JarId>
```

`build_jar_manifest` runs at scan time for **every** discovered JAR path (replacing today's
eager full-materialization call): it runs the *existing* sidecar/parse output through unchanged,
but retains only `name`+`kind`+`container` — `detail`/`params`/`doc` are discarded immediately,
never allocated as `SymbolEntry`/`SidecarSymbol` at this stage. Cheap by construction; everyone
gets it, matching the "project loaded eagerly" half of the Android Studio model.

### Tier 2 — the existing pipeline, retimed

`materialize_jar_on_demand(jar_id)`: checks `materialized`; if absent, calls **today's
`index_jars`/`index_sources_jars` unchanged**, scoped to that one JAR's path, merges via the
existing `LibraryBatch`/apply path (also unchanged), marks `materialized`. No new compute or merge
logic — only the trigger and its timing are new.

### Resolver integration

Where a lookup misses the workspace maps today (`resolve.rs` and call sites using the existing
`ResolveIo` escalation ladder), add one more rung: check `jar_qualified`/`jar_bare_names`. On a
hit, if `ResolveIo` permits (`Full` or `NoRg` — not `IndexOnly`) and the jar isn't materialized,
call `materialize_jar_on_demand`, then retry the original lookup against the now-populated
`qualified`/`definitions`. This maps directly onto the existing typed policy without inventing a
new concept: `IndexOnly` (diagnostics, keystroke path) never triggers synchronous IO, matching its
current "strictly in-memory" contract exactly — a Tier-1-only hit there behaves like a miss does
today.

## Data flow

1. **Scan time:** discover JARs (unchanged) → `build_jar_manifest` for all → Tier 1 populated.
2. **Resolution time:** workspace lookup misses → Tier-1 hit on a `JarId` → not yet materialized
   → `ResolveIo` gate → `materialize_jar_on_demand` → retry → real answer.
3. Materialized jars stay resident for the session. No eviction, no invalidation — JARs are
   immutable, so once computed the data is correct until the process exits.

## Error handling

- **Concurrent requests hitting the same un-materialized jar:** reuse the exact double-checked-
  locking pattern `FileTable::intern` already uses (verified race-free in PR #208's review) — no
  new risk class introduced.
- **Sidecar crash during on-demand materialization:** matches existing behavior (`sidecar.rs`:
  "on crash, handle set to None, callers get no symbols"). Mark the jar failed-this-session (not
  materialized) so it isn't retried in a loop, but isn't permanently poisoned across sessions
  either — this is a new small state distinct from `materialized`/absent; a boolean flag or a
  third `DashSet` for "attempted and failed" is sufficient, no retry/backoff timer needed given a
  session is short-lived relative to a stuck sidecar being a rare failure mode.
- **`IndexOnly` (diagnostics)** never triggers materialization synchronously — see Resolver
  integration above.

## Testing

- **Decoy-first**, matching this effort's discipline throughout: a symbol resolvable only through
  a never-touched jar — assert it resolves correctly through the lazy trigger, verified RED-
  without/GREEN-with via `git stash` (fails without Tier 2 promotion wired up, passes with it).
- **Probe-verified, not assumed**: extend `memory_retainer_profile` (or a sibling test) to show
  most jars sitting in Tier-1-only state after a small set of realistic feature invocations, with
  only touched jars carrying full Tier-2 data. This is the actual memory claim this design makes —
  it must be measured before being declared a win, exactly as the jar-cache-streaming fix's null
  result was measured and honestly reported rather than assumed.
- **Full existing suite stays green** — behavior-preserving for anything actually touched; the
  only observable difference is memory for what *isn't* touched. No existing test should need its
  assertion weakened; if one does, that's a signal the design has a real gap, not a test to fix.

## Explicitly out of scope for v1

- **Import-scoped eager promotion** (proactively materializing JARs the workspace's own
  `ImportEntry` list references, ahead of first lookup) — a plausible v2 optimization once the
  two-tier mechanism is proven, not required for the core design to work or be measured.
- **Cheaper Tier-1 extraction** (a genuinely lighter sidecar protocol mode, vs. today's "run full
  extraction, discard most of it") — v1 accepts the discard-after-receipt cost to avoid touching
  the sidecar wire protocol; revisit only if Tier-1 build time itself becomes a measured problem.
- **Eviction of materialized (Tier-2) jars under memory pressure** — the whole premise is that most
  jars never get touched, so the materialized set should stay small in practice; add eviction only
  if measurement shows otherwise.
- **A live "memory usage" LSP command** (inspired by rust-analyzer's `MemoryUsageRequest`) —
  complementary idea, separate from this design, not required for it.

## Rollout

Both subsystems (compiled-JAR, source-JAR) share the mechanism from day one in this design, but
implementation can still stage: land the `JarId`/`JarTable`/Tier-1-map foundation and wire
compiled-JAR symbols through it first (smaller, already measured, sidecar protocol is naturally
per-JAR-request-friendly already), then flip source-JAR text onto the same trigger once the
mechanism is validated end-to-end. This is a sequencing choice for the implementation plan, not a
second design.
