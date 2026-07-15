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

### Tier 1 — always eager, genuinely its own lightweight path (not Tier 2 + discard)

**Revised after critique.** The first draft proposed building Tier 1 by running the existing full
sidecar/parse pipeline and discarding most fields afterward. That does not work for compiled-JAR
symbols: `load_jar_cache()` (`jar_cache.rs:70-86`) `bincode::deserialize_from`s the **entire**
monolithic `jar-symbols-vN.bin` — one `HashMap<String, JarCacheEntry>` covering all 777 JARs — in
a single call. Discarding fields *after* that call reduces steady-state retention but not the
transient peak, since the whole 618 MB-equivalent structure must exist in memory during the decode
regardless of what's kept afterward. "Run Tier 2, then discard" is a steady-state optimization
only; it does not move peak, which is what the memory-reduction effort has been measuring.

Tier 1 must therefore be a genuinely separate, purpose-built lightweight path per subsystem:

- **Compiled-JAR symbols:** the on-disk cache format changes from one monolithic blob to
  independently-decodable per-JAR entries (a version bump, same precedent as the SymbolEntry
  diet's `CACHE_VERSION` 29→30). Tier-1 build reads only a cheap manifest section (name+kind+
  container) without decoding each entry's full `Vec<SidecarSymbol>` (with `detail`/`doc`/
  `params`). This is real cache-format work, not "reuse unchanged" — scoped explicitly into v1
  because without it the compiled-JAR side of this design does not reduce peak memory, only
  steady-state, which undercuts the design's primary goal for that subsystem.
- **Source-JAR text:** tree-sitter parsing the full file is the dominant cost, and it happens in
  the Rust process (unlike compiled-JAR symbols, whose expensive analysis happens sidecar-side
  regardless). A lightweight scan for `package`/top-level declaration names (skipping full AST
  construction) is a genuine wall-clock *and* memory win here, not just memory — this asymmetry
  between the two subsystems is real and should stay explicit rather than treating both
  identically.

Both feed the same shape, reusing the interning pattern already proven in PRs
#206/#208/#209/#210 (`FileId`/`FileTable`) applied to JAR paths instead of file URIs:

```rust
pub(crate) struct JarId(u32);
pub(crate) struct JarTable { /* identical shape to FileTable: RwLock<Vec<...>> + DashMap intern */ }

jar_qualified: DashMap<String /* FQN */, JarId>
jar_bare_names: DashMap<String /* short name */, Vec<JarId>>   // for imports/wildcards
materialized: DashSet<JarId>
```

### Tier 2 — the existing pipeline, retimed

`materialize_jar_on_demand(jar_id)`: checks `materialized`; if absent, calls **today's
`index_jars`/`index_sources_jars` unchanged**, scoped to that one JAR's path, merges via the
existing `LibraryBatch`/apply path (also unchanged), marks `materialized`. No new compute or merge
logic — only the trigger and its timing are new. See §Concurrency for the sidecar-access model
this now requires.

### Consumer integration — revised after critique

**The first draft's central claim — "one new rung in `resolve_symbol`'s `ResolveIo` ladder covers
all resolution" — is false.** An independent critique (grounded in the actual call sites, not the
design's description of them) found that the highest-value library consumers read
`jar_definitions`/`jar_files`/`qualified` **directly**, bypassing `resolve_symbol` entirely:
call-arg diagnostics and signature resolution (`sig.rs:909,994,1001`), type/receiver inference
(`resolver/infer.rs:778,933`; `indexer/resolution.rs:663,677`), import/hover scope
(`indexer/lookup.rs:69,83,99`), and — architecturally most significant — completion's candidate-
name enumeration (`apply.rs:1081` `rebuild_bare_name_cache`, `apply.rs:1109`
`rebuild_importable_fqns`). None of these route through `ResolveIo`; a single new rung there would
leave every one of them silently stuck on Tier-1-only (i.e. missing) results, permanently, even
after this ships.

Completion is a distinct case, not just another missed call site: its job is surfacing names the
user *hasn't typed yet* (`remem` → `remember`), which cannot be "lazy on lookup" — there is no
lookup to hook until the name is already known. The fix: completion's candidate lists read
`jar_bare_names`/`jar_qualified` **directly** (cheap, always available, no materialization
needed to list a name) for the enumeration step; Tier-2 materialization for a specific JAR
triggers when the user **selects** a completion item (LSP's `completionItem/resolve`, which
clients already call for additional detail on-demand) or when hover/goto-def is invoked on it —
not on every keystroke.

For the direct-read consumers (sig.rs, resolution.rs, infer.rs, lookup.rs), each calls a shared
`ensure_jar_materialized(jar_id, io_policy)` helper **at its own read site** rather than through a
central chokepoint: several integration points, not one. Each site already knows its own
`ResolveIo` policy from its caller context.

### Auto-import / missing-import completion — named explicitly, was previously implicit

**Caught in a second review pass, not the first critique — worth naming as its own scenario
rather than leaving it folded into "completion" generically.** Two rebuilt caches feed bare-word
and auto-import completion, both iterating only *currently-populated* maps rather than reading
live per-request, and both would silently lose coverage for unmaterialized JARs under this design
if left unaddressed — the same root cause, but they're not currently consistent with each other,
which is worth being precise about rather than assuming one fix covers both:

- `rebuild_bare_name_cache` (`apply.rs:1073-1090`) already includes JAR-only names *today*: it
  explicitly walks `jar_definitions.iter()` (line 1081-1085) in addition to `definitions`. Under
  this design, `jar_definitions` only contains materialized JARs, so this cache would silently
  shrink to Tier-2-only coverage unless it also merges in `jar_bare_names` (Tier 1).
- `importable_fqns` (`indexer.rs:274`, rebuilt by `rebuild_importable_fqns`, `apply.rs:1109-1134`,
  called from within `rebuild_bare_name_cache` itself at line 1090) has a **separate, pre-existing**
  gap: it iterates only `self.files`, never `jar_definitions` at all — meaning compiled-JAR-only
  symbols (no `-sources.jar` published, bytecode only) likely don't get auto-import suggestions
  *today*, before this design touches anything. Not a regression to fix as part of this work, but
  worth naming so it isn't mistaken for something this design broke.

`complete_bare`'s auto-import path (`resolver/complete.rs:1700`, `resolver/resolve.rs:65`) is the
worst case for the import-scoped-eager-promotion fix above regardless of which cache: auto-import
exists specifically to suggest symbols the file has **no** `ImportEntry` for, so "eagerly
materialize what a file already imports" categorically cannot help it.

**Fix:** both `rebuild_bare_name_cache` and `rebuild_importable_fqns` (or `complete_bare` at the
read site — an implementation choice for the plan) merge in `jar_qualified`/`jar_bare_names`
(Tier 1) alongside their existing materialized-data sources. This needs no new data: `jar_qualified`
's key is already the full FQN (`package.Name`), exactly what an import-insertion edit needs, and
Tier 1 already retains `kind` for correct completion-item presentation — both available *before*
Tier-2 materialization. Selecting a Tier-1-sourced candidate triggers `ensure_jar_materialized` for
that JAR, consistent with "completion promotes on selection, not on listing" already established
above — no new principle, just a site where it would be easy to fix one cache, test dot-completion
and hover, ship, and never notice the other cache silently kept its pre-design behavior by
coincidence until a never-imported library type stopped appearing as a suggestion.

## Data flow

1. **Scan time:** discover JARs (unchanged) → `build_jar_manifest` for all (its own lightweight
   path, §Tier 1) → Tier 1 populated.
2. **On file open / first diagnostics pass for a file:** eagerly materialize the JARs referenced
   by that file's own already-parsed `ImportEntry` list (§Import-scoped eager promotion) — before
   diagnostics runs, so the common case (a file using the types it imports) has full data
   promptly and the Tier-1-suppression path (§Error handling) only carries genuinely-unreferenced
   or not-yet-processed candidates.
3. **Resolution time (completion, hover, goto-def, diagnostics):** each consumer reads Tier 1
   directly for existence/enumeration; the direct-read consumers call `ensure_jar_materialized`
   at their own site when `ResolveIo` permits (§Consumer integration); completion promotes on
   item-selection, not on every keystroke; diagnostics never promotes, only suppresses on a
   Tier-1 existence check.
4. Materialized jars stay resident until the next reindex (§Reindex and freshness) — not
   literally forever, but for the practical duration of a session between reindexes.

## Import-scoped eager promotion — moved into v1 scope after critique

The first draft deferred this to "v2, optional." Combined with the diagnostics fix above, it's
load-bearing for v1, not an optimization: without it, a freshly opened file's own diagnostics pass
runs before any of its imported library types are materialized, so call-arg/nullable diagnostics
on those types would be degraded (falling back to the Tier-1 suppression path) on the default
interactive path — every file, every session — rather than only for the rare case of a symbol
reached outside the file's own declared imports. Proactively materializing a file's imports on
open turns "always degraded for libraries" into "briefly incomplete, then correct" — the gap the
mechanism should actually produce.

## Concurrency — revised after critique

The first draft cited `FileTable::intern`'s double-checked locking as sufficient precedent for
`materialize_jar_on_demand`. That analogy doesn't hold: `intern` is a pure in-memory append under
a `Vec` write lock with no IO. `materialize_jar_on_demand` calls the sidecar — a **single**
`SidecarHandle` behind `jar_sidecar: Mutex<Option<...>>` (`indexer.rs:296`), driven by blocking
`write_all`/`read_line` round-trips to a Java process. Two problems the `intern` precedent doesn't
model, both confirmed against the real code:

1. **The background startup crawl holds the sidecar mutex across its entire batch**
   (`scan_handler.rs:395` acquires it once for the whole `index_jars` + `index_sources_jars`
   pass), not per-JAR. An on-demand materialization request arriving during that window would
   block until the *entire* crawl finishes — potentially many seconds — not just until one JAR's
   worth of work completes.
2. **Blocking sidecar IO on an interactive path.** `Full` policy covers hover/goto-def; triggering
   a synchronous per-JAR sidecar RPC (bytecode + KDoc analysis, potentially hundreds of ms for a
   large JAR) from those call sites blocks whatever's serving that request for the duration.

**Fix, in scope for v1:**
- The startup crawl acquires and releases the sidecar lock **per JAR**, not once for the whole
  batch — a scoping change to the existing loop in `scan_handler.rs`, not new machinery.
- On-demand materialization attempts a bounded/non-blocking lock acquisition. If the sidecar is
  busy (crawl in progress, or another on-demand request in flight), the caller gets a Tier-1-only
  result for *this* request rather than blocking — degrading gracefully rather than stalling the
  hover/completion path. A later request (the crawl finishes, or the user re-hovers) succeeds
  normally.

## Error handling

- **Concurrent requests hitting the same un-materialized jar:** the double-checked-locking
  pattern remains correct for the in-memory bookkeeping (`materialized` set, `JarTable::intern`)
  — see §Concurrency for why the sidecar call itself needs a different treatment than a plain
  lock-and-proceed.
- **Sidecar crash during on-demand materialization:** matches existing behavior (`sidecar.rs`:
  "on crash, handle set to None, callers get no symbols"). Mark the jar failed-this-session (not
  materialized) so it isn't retried in a loop, but isn't permanently poisoned across sessions
  either — a third `DashSet` for "attempted and failed" is sufficient, no retry/backoff timer
  needed given a session is short-lived relative to a stuck sidecar being a rare failure mode.
- **`IndexOnly` (diagnostics) — revised after critique.** The first draft said this policy
  "degrades like a miss does today," which is wrong: today there is no partial-materialization
  state to compare against (everything is eager), so nothing has ever exercised this path. The
  real risk, confirmed against `call_arg_diagnostics.rs:32-39,171-173` and `sig.rs:955-1017`: that
  feature's *only* defense against false-positive warnings is seeing every overload candidate and
  suppressing (`SignatureResult::Overloaded`) when ambiguous. If one candidate's JAR is
  unmaterialized, diagnostics would see a partial set, wrongly conclude "unique," and emit a false
  "wrong parameter count" warning where today it correctly suppresses.

  **Fix:** `IndexOnly` never triggers Tier-2 materialization (unchanged — still no IO on the
  diagnostics path), but it **does** consult Tier 1 (`jar_qualified`/`jar_bare_names` — cheap,
  in-memory, no IO) to check whether another candidate for the same name exists in an
  unmaterialized JAR. If so, treat it the same as `Overloaded` and suppress — extending the
  existing suppression semantics with a cheap existence check, rather than ever needing to
  materialize from the diagnostics path. This keeps `IndexOnly`'s "no IO" contract intact while
  closing the false-positive gap: `jar_phase == Ready` no longer needs to imply "every candidate
  materialized," only "Tier 1 complete," and diagnostics correctness no longer depends on that
  distinction.

## Reindex and freshness — new, from critique

The on-disk caches already key freshness on `(mtime, size)` because most JARs are immutable
release artifacts — but not all: `-SNAPSHOT` dependencies, `mavenLocal()`/composite builds, and
user-supplied `workspace.json`/init-option JAR paths can point at a file that changes mid-session.
Today `handle_reindex` → `index_jars` clears and repopulates the JAR maps wholesale
(`jar.rs:526-529`), which already handles this. The design must preserve that: **on reindex, reset
`materialized` and rebuild Tier 1** — the same discipline the existing caches already apply, not a
new mechanism. A materialized `JarId` is not "forever" in the literal sense the first draft
implied; it's "until the next reindex," matching how the rest of the index already behaves.

## Testing

- **Decoy-first**, matching this effort's discipline throughout: a symbol resolvable only through
  a never-touched jar — assert it resolves correctly through the lazy trigger, verified RED-
  without/GREEN-with via `git stash` (fails without Tier 2 promotion wired up, passes with it).
  Include a decoy specifically for §Auto-import: a file with no import for a type that lives only
  in an unmaterialized JAR — bare-word completion on that type's name must offer it as an
  auto-import candidate, not silently omit it.
- **Probe-verified, not assumed**: extend `memory_retainer_profile` (or a sibling test) to show
  most jars sitting in Tier-1-only state after a small set of realistic feature invocations, with
  only touched jars carrying full Tier-2 data. This is the actual memory claim this design makes —
  it must be measured before being declared a win, exactly as the jar-cache-streaming fix's null
  result was measured and honestly reported rather than assumed.
- **Full existing suite stays green** — behavior-preserving for anything actually touched; the
  only observable difference is memory for what *isn't* touched. No existing test should need its
  assertion weakened; if one does, that's a signal the design has a real gap, not a test to fix.

## Explicitly out of scope for v1

- **Eviction of materialized (Tier-2) jars under memory pressure** — the whole premise is that most
  jars never get touched, so the materialized set should stay small in practice; add eviction only
  if measurement shows otherwise.
- **A live "memory usage" LSP command** (inspired by rust-analyzer's `MemoryUsageRequest`) —
  complementary idea, separate from this design, not required for it.
- **Wildcard imports resolving via `jar_bare_names` alone are approximate**: `import a.b.*` really
  needs package→JarId, which short-name lookup doesn't cleanly provide (noted by critique as a
  refinement, not a blocker). v1 can fall back to today's broader-but-slower resolution path for
  wildcard imports specifically; a package-keyed Tier-1 index is a natural v2 addition once the
  per-name version is proven.

(Import-scoped eager promotion and per-subsystem Tier-1 cost are now in v1 scope — see the
sections above; the critique found both load-bearing, not optional.)

## Rollout

Both subsystems (compiled-JAR, source-JAR) share the mechanism from day one in this design, but
implementation can still stage: land the `JarId`/`JarTable`/Tier-1-map foundation and wire
compiled-JAR symbols through it first (smaller, already measured, sidecar protocol is naturally
per-JAR-request-friendly already), then flip source-JAR text onto the same trigger once the
mechanism is validated end-to-end. This is a sequencing choice for the implementation plan, not a
second design.

### Relationship to the pending CST-navigation unification (slice 6) — resolved after discussion

`src/indexer/infer/sig.rs` (call-arg diagnostics, signature resolution) sits inside the CST-
unification work that's already complete and merged — safe, stable ground. `src/resolver/
resolve.rs`, `src/resolver/infer.rs`, and `src/indexer/lookup.rs` are the still-heuristic "string
path" (each's own doc comment confirms it: fallback chains through imports/package/`rg`) — exactly
what the separately-planned slice 6 ("CST-aware navigation") is scoped to unify for go-def/
find-refs/rename/highlight. Wiring `ensure_jar_materialized` into that layer's *current* shape now
means slice 6 will likely touch those call sites again later — that's an accepted, expected cost.

**What this does NOT mean: skip wiring those consumers.** Flipping eager materialization to lazy
while leaving any direct reader of `jar_definitions`/`files` unwired is a functional regression for
that consumer, not a neutral deferral — go-to-definition into an unmaterialized library symbol
would silently fail where it works today. `ensure_jar_materialized` must be wired into every
current direct-read consumer identified in §Consumer integration, including the string-path ones,
for this to be safe to ship. What's deferred to slice 6 is the *architectural cleanup* of how those
call sites integrate (slice 6's own goal — fewer, more uniform lookup paths, likely simplifying
this integration further) — not correctness now.
