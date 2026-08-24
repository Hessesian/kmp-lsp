# Qualified-Name Resolution Unification — Design

Status: **proposed** (2026-08-24). Written after four independent bugs (three fixed, one
newly found by this audit) surfaced the same root shape across two days of work on
`fix/when-smart-cast-field-collision`. Builds directly on `docs/architecture/unified-resolution-strategy.md`
(string-domain direction, proposed) and `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`
(CST-domain direction, approved, partially implemented) — this document does not re-derive either;
it slots a missing piece into both.

## Context (why)

Four sites answer the same question — *"given a multi-segment qualified name (nested type,
chained field access), which specific declaration does the LAST segment refer to?"* — and three
of them got it wrong the same way: **only the first segment was resolved reachability-first
(imports/package-aware); every segment after it fell back to an unscoped or under-scoped
search**, landing on an arbitrary same-named sibling instead of the one actually meant.

This is not a hypothetical shape. The verification codebase is MVI-style Kotlin with hundreds of

```kotlin
sealed interface Event {
    data class Foo(val event: FooEvent) : Event
    data class Bar(val event: BarEvent) : Event
    // … dozens more variants, one file, all with a field literally named `event`
}
```

so "wrong sibling variant's same-named field" is the *default* failure mode for any lookup that
doesn't scope every hop, not an edge case.

### The five sites (four given, one found during this audit)

| # | Site | Domain | Status before this doc |
|---|---|---|---|
| 1 | `resolver/infer.rs::find_field_type_in_class` + `infer_field_chain_type` | string | **Fixed** (this session) — reference implementation |
| 2 | `resolver/resolve.rs::resolve_qualified` (uppercase-qualifier branch) | string | **Fixed** (this session) — ported #1's segment-walk shape in |
| 3 | `indexer/infer/chain.rs::resolve_member_type_on` / `forward_resolve_segments` | CST | **Not fixed** — confirmed live |
| 3b | `indexer/infer/expr_type.rs::infer_navigation_expr_type` | CST | **Not fixed — newly found by this audit**, same root cause as #3, independent function |
| 4 | `resolver/resolve.rs::resolve_symbol` (dotted-`name` branch, ~L139–171) | string | **Not fixed — audit confirms it is NOT clean** (see below) |

#### Site 1 — `find_field_type_in_class` / `infer_field_chain_type` (fixed, reference shape)

`find_field_type_in_class` resolves the *class* via `resolve::resolve_type_index_only` (reachability-first:
imports → package → hierarchy, per `ResolveIo`), then `infer_field_chain_type` threads a
`reachability_uri` through the whole chain walk — each resolved segment's own declaring file becomes
the anchor for the *next* segment's lookup, instead of reusing the caller's original file. This is the
correct shape and is what the new primitive below generalizes, not reinvents.

#### Site 2 — `resolve_qualified`'s uppercase branch (fixed, by porting #1's shape)

Was: resolved only the outer segment (`Event`) to a file + line, then called
`find_name_in_uri_after_line(member_name, that_line)` — which returns whichever same-named member is
*textually closest* to `Event`'s own declaration line, never actually narrowing to a specific nested
segment (`OverdraftInput`). Fixed by walking every remaining nested-type segment to its own location
first (`anchor`), then searching `name` after `anchor`'s line — mirroring the lowercase branch, which
already did per-segment field-type inference correctly.

**Residual gap the fix left standing:** the nested-*type*-segment walk itself (`segments[1..]`, i.e.
`OverdraftInput` in `Event.OverdraftInput.event`) still uses plain `find_name_in_uri` — whole-file,
first-match, no scoping at all. Only the *final* segment (`name`) got the closest-line treatment. A
3+-segment chain where a *middle* segment collides (two same-named nested types in the same file) is
still under-scoped. Not hypothetical-free, but narrower blast radius than the field-collision case
this session actually hit.

#### Site 3 / 3b — the CST engine's chain walk (confirmed live, not fixed)

`chain.rs::resolve_member_type_on` and `expr_type.rs::infer_navigation_expr_type` both call
`InferDeps::find_field_type` — which (on `Indexer`, `src/indexer.rs:420-426`) delegates to
`find_field_type_in_class` (site #1's now-correct implementation) and then **discards the declaring
`Url` it returns**:

```rust
// src/indexer.rs:420
fn find_field_type(&self, class_name: &str, field_name: &str, uri: &Url) -> Option<String> {
    ...
    crate::resolver::infer::find_field_type_in_class(self, class_name, field_name, uri)
        .map(|(field_type, _declaring_uri)| field_type)   // <-- reachability anchor thrown away
}
```

Every caller of `find_field_type` therefore anchors **every hop of a multi-segment chain on the
original caller's own `uri`**, never on the declaring file of the class actually being walked into —
even though the underlying per-class lookup is reachability-correct. Confirmed independently in two
functions:

- `chain.rs::forward_resolve_segments` (2 call sites of `resolve_member_type_on`, `src/indexer/infer/chain.rs:166,191`):
  the loop tracks `current_type: Option<String>` across segments but never a `current_uri`; `uri` is the
  single parameter passed in at the top and reused unchanged for every `resolve_member_type_on` call.
- `expr_type.rs::infer_navigation_expr_type` (`src/indexer/infer/expr_type.rs:198-227`): recursively
  resolves the receiver's type via `infer_expr_type_at_depth`, then calls
  `deps.find_field_type(&receiver_type, &member, uri)` — again the *original* `uri`, at every level of
  the recursion (a 3-segment chain `a.b.c` re-derives `a.b`'s type correctly but still resolves `.c` off
  the outermost caller's file). **This is a second, independent instance of the exact same bug**, not
  mentioned in the original brief — found by tracing `find_field_type`'s callers with
  `find_referencing_symbols`.

Both are downstream of `find_field_type`'s signature discarding information the implementation already
computes. This narrows the fix to one seam, not two hand-patches (see Primitive 2 below).

#### Site 4 — `resolve_symbol`'s dotted-`name` branch — **audit finding: not clean**

The brief flagged this as "closer to correct... but never audited." It is not clean. Read in full
(`src/resolver/resolve.rs:139-171`):

```rust
if name.contains('.') {
    let segments: Vec<&str> = name.split('.').collect();
    if let Some(start) = segments.iter().position(|s| s.starts_with_uppercase()) {
        let outer_locs = resolve_symbol_inner(indexer, segments[start], from_uri, true);
        if let Some(outer_loc) = outer_locs.first() {
            if start + 1 == segments.len() { return outer_locs; }
            let mut current_file = outer_loc.uri.to_string();
            let mut resolved: Option<Vec<Location>> = None;
            for seg in &segments[start + 1..] {
                let locs = find_name_in_uri(indexer, seg, &current_file);   // <-- whole-file, first match
                match locs.first() {
                    Some(loc) => { current_file = loc.uri.to_string(); resolved = Some(locs); }
                    None => { resolved = None; break; }
                }
            }
            if let Some(locs) = resolved { return locs; }
        }
    }
}
```

`find_name_in_uri` (`resolver/find.rs:13-31`) is confirmed (read in full) to be a **whole-file,
first-symbol-match scan** — `file_data.symbols.iter().find(|s| s.name == name)` — with *zero*
scoping, not even the line-proximity heuristic site #2's fix applies to its own final segment. Every
segment after the first is walked this way, including the last. This is the identical bug shape,
just with a narrower practical blast radius: this branch fires when the dotted string is itself the
`name` parameter (a type-reference chain like `Event.OverdraftInput` written directly, or a variable's
inferred dotted type), so its segments are typically nested *type* names rather than field names —
lower collision odds than the MVI field-name pattern that motivated sites #1/#2, but the exact same
structural gap. Concrete failure mode: two sibling top-level types in one file each declaring a
same-named nested member (`sealed interface Event { object Loading : Event }` and
`sealed interface UiEvent { object Loading : Event }` in one file) — resolving `Event.Loading` finds
whichever `Loading` symbol appears first in `file_data.symbols`' order, not necessarily `Event`'s own.

The one existing test on this path, `resolve_dotted_name_traverses_deep_nesting`
(`resolver/tests.rs:611-627`), has **no collision decoy** — `Bar.Baz.Foo` is the only three-level chain
in its fixture, so it passes today and would keep passing even with the bug fully intact. It does not
prove this path correct; it only proves the happy path with nothing to collide against.

## One primitive or two?

**Two.** Sites #1/#2/#4 live in `resolver/` — the string domain. Sites #3/#3b live in
`indexer/infer/` — the CST domain. Both governing docs already establish this split is deliberate and
the two domains "should stay separate" (`2026-06-30-cst-resolution-unification-design.md`, "Context"
section): the string domain is intentionally heuristic (works with no synced project, powers cold-start
navigation and agent find), the CST domain is authoritative (works from a real parsed tree, backs
diagnostics). That same document explicitly rejects "a shared cross-domain IR that both string and CST
paths lower into... added complexity / a new domain" as a non-goal. Unifying "resolve a multi-segment
qualified name" into one type/function spanning both domains would be exactly that rejected IR, just
narrower in scope. Nothing about this bug's shape overrides that prior, reasoned decision — the bug
*rhymes* across domains (both walk segments; both need reachability re-anchored per hop) but the two
implementations have no shared runtime state, no shared node/type representation, and answer to
different IO/authority contracts. Sharing *code* would mean threading `InferDeps` through string-domain
callers or `Location`/`ResolveIo` through CST-domain callers — neither belongs on the other side.

So: **two primitives, one shared pattern, documented once so neither is reinvented a third time.**

- **Primitive 1 (string domain):** a correctly-scoped per-hop lookup, replacing three different
  approximations of "find `name` declared specifically within/near a known anchor" that the four
  string-domain sites currently roll separately, at varying quality.
- **Primitive 2 (CST domain):** widen `InferDeps::find_field_type`'s signature to carry the declaring
  `Url` it already computes internally but currently discards, and thread it through the two chain
  walkers (#3, #3b) the same way site #1 already threads `reachability_uri`.

Neither primitive is itself new *code* in the sense of a from-scratch design — primitive 1 generalizes
site #1's already-correct shape; primitive 2 stops throwing away information site #1's own fix already
computes. This plan is closer to "finish applying a decision already made" than "design something new."

## Primitive 1 (string domain): scoped per-hop lookup

### WHY

Three different functions currently approximate "find `name` declared specifically within a known
container," each at different fidelity, none exact:

| Approximation | Used by | Precision |
|---|---|---|
| `find_name_in_uri` (`resolver/find.rs:13`) | site #4 (every hop), site #2 (mid-chain nested-type hops) | **None** — whole-file, first symbol-table match |
| `find_name_in_uri_after_line` (`resolver/find.rs:47`) | site #2 (final segment only) | **Approximate** — closest symbol at/after a line hint |
| `windowed_infer_type_raw` / `infer_field_type_raw`'s `near_line` (`resolver/infer.rs:1009-1071`) | site #1 (field lookup) | **Approximate** — ±20-line text window around a line hint, ordered by distance |

All three exist because the container's *exact* range usually isn't threaded to the call site — only a
`near_line` int or nothing at all. But the exact range is almost always available one hop back: every
`Location` these functions receive came from a `SymbolEntry` that has its own full `.range` (not just
`.selection_range`) sitting right there in the same file's symbol table — `resolve_companion_member`
(`resolve.rs:1248-1291`) already re-fetches it this way to scope a companion-object search, and
`enclosing_container_chain` (`resolve.rs:1207-1229`) already computes container nesting via
`range_encloses`. The infrastructure for *exact* scoping exists; these three call sites just don't use
it, because it wasn't factored out as a shared primitive when `resolve_companion_member` first needed
it.

### WHAT

One new function, additive (does not replace the three above for their *other* existing callers —
see Migration and Risks):

```rust
/// Find `name` declared specifically within `container`'s own body — not merely
/// somewhere in `container`'s file. Prefers exact range-containment (re-fetching
/// `container`'s full `SymbolEntry.range`, the way `resolve_companion_member`
/// already does for the companion-object case); falls back to the existing
/// closest-declaration-after-line heuristic only when `container`'s own
/// declaration range can't be found (an un-indexed / disk-read-only file).
///
/// Supersedes, for its callers, three prior approximations of the same
/// question: `find_name_in_uri` (no scoping at all), `find_name_in_uri_after_line`
/// (line-proximity only), and `infer_field_type_raw`'s `near_line` window
/// (text-window proximity only) — see this doc's Primitive 1 section for why
/// none of the three is exact and why this one can be.
pub(crate) fn find_name_scoped_to_container(
    indexer: &Indexer,
    name: &str,
    container: &Location,
) -> Option<Location>
```

Plus a trivial, explicitly-named loop shape — not because it's complex (it's ~10 lines) but because
three call sites currently hand-roll a slightly different version of it and a fourth (site #1) rolls
yet another variant over type-strings instead of `Location`s. Naming it stops a fifth reinvention:

```rust
/// Walk `segments` left to right, re-anchoring on each hop's own result — never
/// falling back to an unscoped or first-segment-only search. `hop` receives the
/// current anchor and the next segment name; returns `None` to abort the walk.
/// Generic over the anchor type `A` because sites #1 (walks type-strings +
/// reachability `Url`s) and sites #2/#4 (walk `Location`s) genuinely need
/// different anchor shapes — this is the shared *loop*, not a shared *type*.
fn walk_qualified_segments<A>(
    segments: &[&str],
    initial: A,
    hop: impl Fn(&A, &str) -> Option<A>,
) -> Option<A>
```

### HOW

`find_name_scoped_to_container`'s body: look up `container`'s own `SymbolEntry` (by matching
`selection_range` in its file's symbol table — the exact pattern `resolve_companion_member` already
uses for a companion object) to get its full `.range`; if found, filter that file's symbols to
`name`-matching entries with `range_encloses(container_full_range, candidate.range)` and return the
best (there should be exactly one for a well-formed nested declaration; multiple is a genuine
`Ambiguous`-shaped case the caller can choose to surface or pick-first, unchanged from today's
behavior). If `container`'s own `SymbolEntry` can't be found (un-indexed file, disk fallback), fall
back to today's `find_name_in_uri_after_line(name, container.uri, container.range.start.line)` — no
regression versus current behavior in that fallback case, just no improvement either; acceptable
because it's already the *best* of the three existing approximations.

Lives in `resolver/find.rs`, next to (and eventually superseding, per-caller, not globally) the two
heuristics it's built from — matching that file's existing convention of un-catalogued `pub(crate)` fns
consumed directly by `resolve.rs`/`infer.rs`.

## Primitive 2 (CST domain): reachability-carrying field lookup

### WHY

`InferDeps::find_field_type`'s signature (`Option<String>`) cannot represent "and here's the file this
field's type must be further resolved *from*" — so every caller is structurally forced to reuse its
own `uri`, which is exactly wrong past the first hop. `find_field_type_in_class`, the function
underneath it, already computes and returns the right answer (`(String, Url)`); `Indexer`'s
`InferDeps` impl throws the `Url` half away at the trait boundary (`src/indexer.rs:420-426`).

### WHAT

Widen the trait method to return what the implementation already has:

```rust
// src/indexer/infer/deps.rs — InferDeps trait
fn find_field_type(&self, class_name: &str, field_name: &str, uri: &Url) -> Option<(String, Url)>;
```

`Indexer`'s impl becomes a straight pass-through of `find_field_type_in_class`'s existing return value
(deletes the `.map(|(field_type, _declaring_uri)| field_type)` discard). `TestDeps`'s impl
(`deps.rs:451`) returns `(type, uri.clone())` using its own test-fixture `uri` parameter (already
available, currently ignored via `_uri`) unless a richer test fixture is worth adding later.

### HOW

Two call sites thread the returned `Url` forward as the next hop's anchor, mirroring
`infer_field_chain_type`'s existing `reachability_uri` pattern exactly (site #1 is the reference
implementation for this too):

- `chain.rs::forward_resolve_segments` — add `current_uri: Url` alongside `current_type: Option<String>`
  in the loop state (currently just `uri: &Url` reused unchanged); on each successful
  `resolve_member_type_on` call, take the returned `Url` and use it for the *next* iteration's lookups.
  `resolve_member_type_on` itself must also return `(String, Url)` instead of `Option<String>` to carry
  this forward — it is the direct caller of `deps.find_field_type`.
- `expr_type.rs::infer_navigation_expr_type` — same threading through its recursive
  `infer_expr_type_at_depth` call: the receiver's resolved type must come back paired with its
  declaring `Url`, used for the `deps.find_field_type(&receiver_type, &member, uri)` call instead of
  the outer `uri` parameter.

**Where this naturally lands architecturally:** the CST catalogue's `ResolvedType`
(`indexer/infer/mod.rs:107-134`) is the one type both of these functions' results already flow through
(`CstQuery::expr_type` wraps `infer_expr_type`'s `Option<String>` into `Resolution<ResolvedType>`).
Adding a `declaring_uri: Url` field to `ResolvedType` — populated from this same threading — is the
single place that fixes both #3 and #3b at once and gives every *future* `CstQuery` consumer the
reachability anchor for free, rather than three call sites each carrying their own parallel `Url`
alongside a `String`. This is also **exactly** the CST design doc's already-planned Slice 4 ("collapse
the chain walk — `chain.rs` becomes the chain step of `expr_type`; delete the echoed chain logic") —
see Non-goals for why this plan does not itself execute Slice 4, but the sequencing note below is
important: land the `Url`-threading described here *as part of* Slice 4's `chain.rs` absorption, not as
a throwaway patch to the soon-to-be-deleted standalone `chain.rs`/`expr_type.rs` functions first. If
Slice 4 isn't imminent, the standalone patch is still correct and independently testable — it just
duplicates work Slice 4 will otherwise redo.

## Reuse inventory (existing types — do not reinvent)

| Type / fn the design needs | Status | Location | Decision |
|---|---|---|---|
| `find_field_type_in_class` | exists, fixed | `resolver/infer.rs:1116` | Reference implementation for Primitive 1's shape; unchanged |
| `infer_field_chain_type` | exists, fixed | `resolver/infer.rs:161` | Reference implementation for `reachability_uri` threading; unchanged |
| `find_name_in_uri` | exists | `resolver/find.rs:13` | Superseded **for these 4 sites' per-hop lookups only** — stays for its other ~dozen callers (see Risks) |
| `find_name_in_uri_after_line` | exists | `resolver/find.rs:47` | Becomes Primitive 1's fallback path (un-indexed-container case) |
| `range_encloses` | exists | `resolve.rs:1236` | Reuse directly for range-containment scoping |
| `enclosing_container_chain` | exists | `resolve.rs:1207` | Pattern precedent (re-fetch full range by `selection_range` match) — Primitive 1 follows the same shape, doesn't call this directly |
| `resolve_companion_member` | exists | `resolve.rs:1248` | The existing precedent for "look up a container's full range, scope a name search to it"; Primitive 1 generalizes this one case into a shared fn |
| `ResolveIo` | exists | `resolve.rs:78` | Unrelated to this fix (already unified per `unified-resolution-handler.md`) — Primitive 1 does not need its own IO policy; it's a pure index read like `find_name_in_uri` |
| `Location` | exists (tower_lsp) | — | Reuse as Primitive 1's anchor/output type |
| `InferDeps` trait | exists | `indexer/infer/deps.rs` | Primitive 2 widens one existing method's signature; no new trait |
| `ResolvedType` | exists | `indexer/infer/mod.rs:109` | Natural home for Primitive 2's `declaring_uri` (see HOW) — extend, don't wrap |
| `CstQuery` | exists | `indexer/infer/mod.rs:149` | Consumer of the widened `ResolvedType`; not itself modified by this plan |
| `Resolver` trait | exists | `resolver/api.rs` | **Not extended by this plan** — Primitive 1 is an internal per-hop helper consumed by `resolver/*.rs` internals, not a feature-facing capability; doesn't belong on the feature catalogue |
| `unified-resolution-strategy.md`'s `ResolvedSymbol`/`resolve()` | proposed, not implemented | `docs/architecture/unified-resolution-strategy.md` | **Distinct, larger effort** — not this plan; see Non-goals |
| `SymbolEntry.range` vs `.selection_range` | exists | `types.rs` | Primitive 1's exact-scoping depends on this distinction (already used by `resolve_companion_member`) |

Net: **zero new types**, one new function (`find_name_scoped_to_container`), one new tiny generic
helper (`walk_qualified_segments`, and only if the migration step below finds real duplication worth
collapsing — see Step 3's note), one widened trait method signature, one widened existing struct
(`ResolvedType` +1 field).

## Non-goals

- **Not the full `unified-resolution-strategy.md` consumer-unification** (`ResolvedSymbol` + `resolve()`
  + `IoPolicy` spanning go-to-def/diagnostics/hover/completion). That is a separate, much larger,
  already-proposed effort targeting a different problem (candidate enumeration + overload policy across
  five *consumers*). This plan fixes a narrower, already-identified sub-bug (multi-segment qualifier
  walking); it does not block that effort and does not need to wait for it.
- **Not merging the string and CST domains**, and not building a shared cross-domain IR or shared
  walker type spanning both. The CST design doc already rejected this explicitly; nothing in this
  bug's shape gives new reason to revisit it. Two primitives, one documented pattern, zero shared code
  across the domain boundary.
- **Not executing CST unification Slice 4** (`chain.rs` → `expr_type` chain-step absorption, deleting
  echoed logic in `receiver.rs`). Primitive 2's `Url`-threading is scoped to be Slice-4-compatible (see
  HOW's sequencing note) but this plan does not take on Slice 4's full scope (receiver.rs dedup,
  `CstExpr` exhaustive dispatch, construction-sealing).
- **Not eliminating line-proximity/text-window heuristics everywhere in the codebase** — only replacing
  the three specific instances that feed these four/five call sites' per-hop disambiguation.
  `infer_variable_type`'s general line-scanning, `windowed_infer_type_raw`'s other callers, etc. are
  unaffected.
- **Not touching `ResolveIo`/the resolution-chain unification** (`resolve_chain`, `resolve_symbol_no_rg`,
  etc.) — already unified per `docs/architecture/unified-resolution-handler.md`; confirmed still in
  place reading `resolve.rs` today. Orthogonal to this bug.
- **Not adding `Vec<Location>`/overload-set semantics to `qualified`** — that's
  `unified-resolution-strategy.md`'s own tracked follow-up, unrelated to per-hop scoping.

## Migration (incremental, test-anchored — mirrors the CST doc's own sequencing style)

Each step lands independently, is green on `cargo test --bin kmp-lsp` before the next, and is marked by
which category it's in: **(A) refactor-onto-primitive** for sites already behaviorally correct (no
intended output change; a decoy regression test proves it), or **(B) migration-is-the-fix** for sites
still broken (a RED test first, green after).

1. **(new, additive) Land `find_name_scoped_to_container` in `resolver/find.rs`.** No caller changes
   yet. Unit tests directly against it: exact range-containment case, un-indexed-container fallback
   case, ambiguous-multiple-match case (document the pick-first behavior explicitly, matching today's
   convention elsewhere in this file).

2. **(B) Site #4 — `resolve_symbol`'s dotted-`name` branch.** Write the RED decoy test first: two
   sibling types in one file each declaring a same-named nested member (the `Event.Loading` /
   `UiEvent.Loading` shape from the audit above), assert `resolve_symbol(idx, "Event.Loading", None,
   uri)` returns `Event`'s own `Loading`, not `UiEvent`'s. Confirm it fails against current code. Then
   replace the `find_name_in_uri` calls in the segment loop with `find_name_scoped_to_container`, using
   the previous hop's `Location` as the container. Confirm the RED test goes green and
   `resolve_dotted_name_traverses_deep_nesting` (the existing happy-path test) stays green.

3. **(A) Site #2 — `resolve_qualified`'s uppercase branch.** Two sub-parts, each independently
   testable: (a) replace the mid-chain nested-type-segment walk's `find_name_in_uri` calls with
   `find_name_scoped_to_container` (closes the residual middle-segment gap the original fix left
   standing — write a decoy test for a 3+-segment chain with a colliding middle nested-type name first,
   confirm RED, then GREEN); (b) replace the final segment's `find_name_in_uri_after_line` call with
   `find_name_scoped_to_container` too, so the whole nested-segment walk uses one consistent primitive
   instead of two different ones. Existing sibling-field-collision regression tests from this session
   are the behavior net for (b) — must stay green (no intended change, since exact range-containment is
   a superset of what closest-line-after already got right for this case).

   At this point, evaluate whether `walk_qualified_segments` (the generic loop helper) is worth
   extracting: sites #2 and #4 now have near-identical "for each remaining segment, look up scoped-to-
   previous-anchor, bail on miss" loops. If the bodies are close enough after step 2/3's edits, factor
   the loop out; if the surrounding bail-out/fallback logic still differs enough to make the extraction
   net-negative in clarity, leave them as two short hand-written loops calling the same leaf primitive.
   Judge this from the actual diff, not in advance — the CST doc's own "move-don't-rewrite" discipline
   applies here too.

4. **(A, no-op refactor) Site #1 — `find_field_type_in_class`.** Already the reference shape; no
   behavior change. Optionally replace its `infer_field_type_raw`/`near_line` window-scan with
   `find_name_scoped_to_container` + a value-extraction step for full range-containment parity with
   sites #2/#4 — but note this function returns a *type string*, not a `Location`, so this is a genuine
   reshaping (find the `Location` via the new primitive, then read its declared type off that
   `Location`'s own range) rather than a drop-in swap. Land only if step 3's evaluation shows real
   value in a single consistent lookup path; otherwise this function is already correct and this step
   is deferrable.

5. **(B) Widen `InferDeps::find_field_type` → `Option<(String, Url)>`.** One commit: trait signature,
   `Indexer` impl (delete the discard), `TestDeps` impl. Enumerate every `find_field_type` caller with
   `find_referencing_symbols` first (this doc found 2: `chain.rs`, `expr_type.rs` — confirm no others
   were missed) before touching the trait, per the CST doc's own "anti-reinvention completeness check."
   This step alone does not fix anything yet — every caller must still be updated to consume the new
   `Url`. Compiles green with callers doing `.map(|(t, _)| t)` at each call site as a transitional no-op,
   OR land steps 5+6+7 as one commit if the blast radius is small enough to review as a unit (2
   call sites, confirmed above).

6. **(B) Thread the `Url` through `chain.rs::forward_resolve_segments` / `resolve_member_type_on`.**
   RED test first: a chain-typed hover/inlay scenario mirroring the sibling-field MVI shape but reached
   through the CST engine (e.g. `nullable_call_diagnostics` or an inlay-hints fixture with a 3-segment
   chain `a.b.c` where `b`'s type is a same-named-sibling-prone class). Confirm current `chain.rs`
   resolves the wrong sibling; fix by tracking `current_uri` in the loop; confirm GREEN.

7. **(B) Thread the `Url` through `expr_type.rs::infer_navigation_expr_type`.** Same shape as step 6,
   independently testable (different function, different fixture — likely a semantic-tokens or
   completion scenario, since that's this function's actual consumer per `find_referencing_symbols`).
   RED-then-GREEN.

8. **(deferred, coordinate not execute) Note for whoever picks up CST Slice 4 next:** when `chain.rs`
   is absorbed into `expr_type`/`CstQuery` per the existing roadmap, the `current_uri` state from steps
   6/7 should become a field on `ResolvedType` (`declaring_uri: Url`) rather than being re-derived a
   third time — this plan's Primitive 2 section spells out exactly where. Left as a note, not a step,
   because Slice 4 is not scheduled by this plan.

## Testing & verification

- `cargo test --bin kmp-lsp` green after every step (binary-only crate; `--lib` runs 0 tests). Focused
  loops while iterating: `-- resolver`, `-- indexer_tests`, `-- it_this`, `-- nullable_call`.
- Every **(B)** step gets a RED-before-GREEN decoy test in the sibling-collision shape that actually
  exposed these bugs — not a happy-path-only test (the existing
  `resolve_dotted_name_traverses_deep_nesting` is the cautionary example of what *not* to rely on).
- Every **(A)** step is behavior-preserving by construction; the existing regression suite from this
  session's site #1/#2 fixes is the net. No new decoy needed unless step 3's middle-segment gap is
  addressed, which does need its own new decoy (it's currently untested either way).
- `find_referencing_symbols` on `find_field_type`, `find_name_in_uri`, and `find_name_in_uri_after_line`
  before and after each migration step — confirms the primitive's callers were actually moved (not just
  added alongside) and that no other caller of the two heuristics was silently affected.
- Ground-truth harness (per `unified-resolution-strategy.md`'s own debugging recipe) against the real
  MVI-shaped sample project remains the way to confirm these fixes generalize beyond hand-written
  fixtures — unit tests alone can't reproduce the JAR/sources-jar-backed collision shapes reliably.

## Risks

- **`find_name_in_uri` has callers well beyond these four sites** (used throughout `resolve.rs`/
  `resolve_qualified`'s lowercase branch, extension resolution, etc.). This plan deliberately does
  **not** replace it globally — only the specific per-hop call sites in steps 2–4 move onto the new
  primitive. Mitigation: `find_referencing_symbols` before and after each step (see Testing) keeps the
  blast radius visible and bounded; no blanket rename.
- **`InferDeps::find_field_type`'s signature widening (step 5) touches every implementor in one commit**
  — smaller blast radius than it sounds (2 implementors: `Indexer`, `TestDeps`; 2 callers confirmed via
  `find_referencing_symbols`), but it's a trait change, so any *future* implementor added between now
  and this landing would need the same update. Mitigation: land steps 5–7 close together, not with a
  long gap.
- **Range-containment lookup cost.** `find_name_scoped_to_container`'s exact path re-scans a file's
  symbol table twice per hop (once to find the container's full range, once to find `name` within it) —
  same order of cost as the existing `resolve_companion_member` precedent, negligible for typical file
  sizes, but worth a spot-check on the Moneta-scale ground-truth harness this codebase already uses for
  perf validation, given these are hover/goto-def hot-ish paths.
- **Site #1's reshaping (step 4) is the least mechanical step** — it returns a type string, not a
  `Location`, so unlike steps 2/3/6/7 it can't be a pure "swap the lookup call" edit. Explicitly marked
  optional/deferrable in the migration plan rather than forced, to avoid destabilizing the one site that
  is already fully correct today for the sake of code-sharing purity.
- **Duplicating Slice 4's future work (Primitive 2).** If Slice 4 lands soon after this plan, steps 6–7
  land logic that Slice 4 then moves again. Mitigation: step 8 is written as an explicit coordination
  note in this doc precisely so that isn't silently rediscovered later; the `Url`-threading logic itself
  is small enough (mirroring an already-proven pattern from site #1) that redoing it once more inside
  Slice 4 is cheap even if the timing doesn't line up.
