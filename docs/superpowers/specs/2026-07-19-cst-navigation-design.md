# CST-Aware Navigation — Design (Slice 6)

Status: **approved design** (brainstormed with the user 2026-07-19; decisions: three sub-slices
under one spec; rename refuses unless CST-resolved). Slice 6 of
[CST resolution unification](2026-06-30-cst-resolution-unification-design.md), expanding the
sketch in `docs/superpowers/plans/2026-07-05-post-fable-roadmap-refinements.md` §6.
Branch: `refactor/cst-navigation` off `refactor/unified-resolution` (post-#224).
Sub-slices land as separate PRs: 6a → 6b → 6c, each live-verified before the next.

## Context (why)

The symbol-identity navigation family — go-to-definition, goto-implementation,
find-references, document-highlight, rename — answers "which symbol does this identifier
refer to?" name-based today. Same-named members on unrelated types collide, locals bleed
across scopes, and rename is a silent gamble in ambiguous states. The CST engine built up
through slices 1-4 (receiver typing via `CstQuery::expr_type`, uri-threaded member lookup,
repair-wired tree acquisition via `lambda_doc_at`) can now answer identity precisely — the
features just don't ask it.

Current surfaces: `features/definition.rs` (195 lines), `references.rs` (616),
`rename.rs` (476), `highlight.rs` (54), `implementation.rs` (219). Shared per-request context:
`backend/cursor.rs::CursorContext` — string-first (`word_and_qualifier_at` text scan) with a
CST bridge for contextual receivers.

## Decisions locked with the user

- **Decomposition:** one spec, three implementation cycles: 6a = shared core + read-only
  features (go-def, goto-impl, document-highlight); 6b = find-references; 6c = rename.
- **Rename policy:** rename is either right or refuses with a reason — never a silent gamble.
  The original wording locked here ("any `NameScan` residue ⇒ typed refusal") was explicitly
  conditioned on 6c's own live measurement (see 6c's "Policy gate"). Both live-measured members
  turned out to have real overrides and so refuse regardless of this question — the measurement
  mainly proved override detection was the load-bearing gap, not that the `NameScan` question
  itself is common. The spec's own documented fallback still applies on reasoning, not direct
  measurement: today's rename already ships zero verification, so `NameScan` residue (not
  proven wrong, not otherwise resolvable) is included at that same pre-existing trust level,
  while `rejected` (proven wrong) or structurally ambiguous cases (override participation in
  either direction, non-unique/library identity) still refuse with a typed LSP error. Never
  text-rename a candidate this pass proved is a different identity.
- **Approach:** new classification layer in the CST domain (not an extension of
  `CursorContext`, not a hover rewrite). `CursorContext` stays for hover; goto-def migrates
  off it in 6a. The string engine remains the guaranteed fallback for every feature — cold
  start, unsynced projects, and agent find must keep working exactly as today.

## Goals / non-goals

**Goals**

1. One classifier: `classify_symbol_at(indexer, uri, pos) -> Option<SymbolAtCursor>` in the
   CST domain, reusing `cursor_node_at`, `lambda_doc_at`, and `CstQuery` resolution.
2. Typed provenance: `NavigationSource::{CstResolved, NameScan}` on every navigation result
   path — the fallback is visible in code, rankable, and testable.
3. 6a: precise jumps + scope-correct highlight. 6b: receiver-verified references without
   losing recall. 6c: rename that is either right or refuses with a reason.
4. Existing navigation behavior is the floor: `NameScan` reproduces today's results wherever
   the CST cannot establish identity.

**Non-goals**

- Hover, document-symbol, workspace-symbols — not in the identity family.
- Deleting `CursorContext` (hover keeps it; shrinking it is post-6a cleanup, own change).
- Unifying the string engine's internals (locked non-goal of the parent design).
- Cross-file type-hierarchy-wide rename semantics (e.g. renaming an override renames the
  base): follow Kotlin LSP conventions later; 6c renames the exact identity under the
  cursor and its verified references only, refusing when override relationships make the
  edit set ambiguous (detected via the existing supertype machinery).

## Design

### Core (built in 6a): `SymbolAtCursor` + `NavigationSource`

```rust
// indexer/infer/symbol_at_cursor.rs (new module, catalogued in infer/mod.rs)
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
    /// Receiver TYPE for member references (`user.save` → "User"), resolved via
    /// `CstQuery::expr_type` on the receiver subtree. `None` for bare names.
    pub receiver_type: Option<String>,
}

pub(crate) enum SymbolRole {
    /// The name token of a declaration; `kind` from the declaration node.
    Declaration { kind: DeclarationKind },   // Class | Function | Property | LambdaParam | …
    /// An identifier in expression/type position.
    Reference,
    /// A segment of an import path (navigation targets the imported symbol).
    ImportSegment,
}

pub(crate) enum NavigationSource<T> {
    /// Identity established from the CST + index: precise, ranked first.
    CstResolved(T),
    /// Name-based scan (string engine / rg): today's behavior, visibly labeled.
    NameScan(T),
}

pub(crate) fn classify_symbol_at(
    indexer: &Indexer, uri: &Url, pos: CursorPos,
) -> Option<SymbolAtCursor>
```

**Reuse, not a fresh pass (independent critique finding):** `semantic_tokens/helpers.rs`
already classifies every token as declaration-vs-reference (`is_declaration_site`, keyed on
the identical parent-node-kind test this classifier needs) and already extracts + types
receivers (`resolve.rs::resolve_member_access` via `CstQuery::expr_type`,
`navigation_receiver_node`/`navigation_member_ident`). Promote these four helpers out of
`semantic_tokens` into a shared home (`indexer/infer` — they're already `CstQuery`-shaped)
and have `classify_symbol_at` call them instead of re-deriving the same walk. This also stops
semantic_tokens and the navigation classifier from silently drifting on what counts as a
declaration site.

Classification is one CST pass at the cursor node, built on the promoted helpers:
- **Declaration**: the identifier is the name child of a declaration node
  (`class_declaration`, `object_declaration`, `function_declaration`, `property_declaration`,
  parameter, lambda parameter).
- **Reference**: identifier in expression or type position; if its parent is a
  `navigation_suffix`, extract the receiver subtree and type it via `CstQuery::expr_type`
  (the #222/#224 machinery — uri-threaded member lookup included).
- **ImportSegment**: identifier inside an `import_header` path.
- **`None` (not a symbol)**: cursor in a string literal (non-interpolated part), comment, or
  whitespace — navigation features return no result instead of name-scanning strings.
- Acquisition through `lambda_doc_at`, so mid-typing states classify against the repaired
  tree; classification never triggers the marker-insertion transform (the cursor sits on an
  existing token, not a completion gap).

The classifier produces IDENTITY, not locations. A second shared function resolves identity
to a definition — named up front (independent critique finding) because all three
sub-slices need it and would otherwise each hand-roll it: 6a's go-def jump, 6b's per-candidate
declaring-type lookup, and 6c's "does this reference resolve uniquely" uniqueness test are the
same call:

```rust
pub(crate) fn resolve_identity<D: InferDeps>(
    sym: &SymbolAtCursor, deps: &D, uri: &Url,
) -> NavigationSource<Definitions>
```

Built on the existing `resolve_member`/`find_definition_qualified` family
(`resolver/api.rs`, `indexer/lookup.rs`) — this function does the IO-bounded work
(`ResolveIo`-gated), which is why it stays separate from the cheap pure-CST
`classify_symbol_at`.

### 6a — go-def, goto-impl, document-highlight

- **go-def**: `Reference` with `receiver_type` → receiver-typed member lookup (the
  `method_return_type`/`resolve_member` family) → `CstResolved` jump. Local/lambda-param
  references → the declaration node found by walking enclosing scopes in the CST.
  `Declaration` role → the symbol IS the definition (return self-location, matching LSP
  convention). Anything unresolvable → today's path (index by name + rg), wrapped `NameScan`.
  Ranking: `CstResolved` results first when both exist.
- **goto-impl**: same identity feeds the existing subtype lookup; receiver-typed identity
  filters same-named interfaces.
- **highlight** (54 lines): today highlights every text match across the WHOLE FILE — no
  scoping at all (confirmed: `word_byte_offsets` over the full document), so same-named
  locals in unrelated functions currently bleed together. Fixed via a new shared function,
  named up front because 6c needs the identical walk (independent critique finding):

  ```rust
  pub(crate) fn local_scope_occurrences(
      doc: &LiveDoc, decl_node: tree_sitter::Node,
  ) -> Vec<(Range, SymbolRole)>
  ```

  Pure CST subtree walk from the declaration node — no index, no rg. Highlight calls it for
  `Declaration`/local-`Reference` roles; everything else keeps today's whole-file behavior
  via `NameScan`.

### 6b — find-references

Recall engine unchanged: the name scan (index + rg) still FINDS candidate sites. The
classifier then VERIFIES each candidate: run `classify_symbol_at` at the candidate position;
a member reference matches only if its `receiver_type` agrees with the query identity (via
the existing supertype walk for inherited members); declarations match only their own scope.
Cost is bounded: one transient parse per candidate FILE (`live_doc_or_parse` reuses
live/indexed content), candidates only. Unverifiable candidates (parse failed, receiver
untypeable) are KEPT and labeled `NameScan` — recall never drops below today's. The response
concatenates `CstResolved` first, then surviving `NameScan` entries.

### 6b-hardening — prerequisite fixes before 6c

Found during 6c's live-data-gathering pass (real measurement against an 18k-file production
Kotlin monorepo, see "Policy gate" below), an independent critique of the first draft of this
section, and a PR-review-comment audit of #228 (four Copilot threads left unresolved at merge).
Land as their own small commit(s) on top of shipped 6b, before 6c's rename logic — 6c's
override-refusal step has no signal to consume until these fixes land.

1. **Declaration-arm agreement fix** (`references_verify.rs`, the `SymbolRole::Declaration`
   match arm). Currently uses exact string equality between the candidate's enclosing class and
   the query's declaring type — inconsistent with the `Reference` arm just above it, which uses
   `receiver_type_agreement`'s supertype walk. Effect measured live: a single-implementor
   interface member (`ICacheManager.clearAllCaches`, one real override) put the override's OWN
   declaration into `NameScan` — indistinguishable from "couldn't verify" — because
   `"CacheManager" != "ICacheManager"` as strings. Fix: call `receiver_type_agreement` here too.
   `Exact` → the query's own declaration; `Inherited` → a proven override; `Unrelated` → stays
   `NameScan`, unchanged (do not add new rejections to shipped 6b's output as a side effect of
   this fix — that's a separate, un-asked scope change). Only charge the IO-budget's
   agreement-walk unit here when a walk will actually run (see fix 2 below) — `Exact` short-
   circuits before any walk and must not be charged.
2. **IO-budget charged for an agreement check that didn't walk** (Copilot review on #228,
   thread still unresolved at merge, confirmed present in shipped code): `references_verify.rs`
   charges a budget unit unconditionally before calling `receiver_type_agreement`, but `Exact`
   (string equality) and `Unresolvable` (`has_type_definition` short-circuit) both return before
   any supertype walk runs — no IO was spent, yet the budget is charged as if it was. This
   genuinely accelerates exhaustion. (A second thread on the same PR, about `live_trees` vs.
   `files`/`live_lines` in the disk-read charge, turned out on inspection to be moot — the
   server only ever populates `live_trees` when `live_lines` is already present, so that branch
   already doesn't over-charge in practice. Don't touch it; the review comment doesn't hold up.)
   Fix: only decrement for the agreement check when the call is actually going to walk (i.e.
   `candidate_type != query_declaring_type` and `has_type_definition(candidate_type)` — the two
   conditions `receiver_type_agreement` itself checks before walking).
3. **`walk_hierarchy`'s sidecar-promotion cap becomes a parameter, not a hardcoded constant**
   (`hierarchy.rs`, `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK = 3`). The cap exists to protect
   *interactive, keystroke-latency-sensitive* callers (inlay hints, bare completion) from an
   unbounded blocking sidecar round trip on a cold JAR cache — it is not a claim that 3 is
   correct for every caller. Recursion depth is separately capped (`max_depth`) and each JAR
   promotion memoizes after its first attempt (`materialized`/`materialization_failed`), so
   raising or removing the promotion cap cannot make a walk infinite — it can only make a
   *rare, already-bounded-by-depth* walk take longer wall-clock time on a cold cache. Change
   `walk_hierarchy` (and the `supertype_chain_contains`/`receiver_type_agreement` wrappers) to
   take the sidecar budget as a parameter. Every existing caller passes
   `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` explicitly — behavior-preserving, zero change
   for 6a/6b or any other consumer. 6c's override-detection walk (below) is the first caller to
   pass something else. This directly replaces the "fail open vs. fail closed on budget
   exhaustion" question 6c would otherwise face: a rename-triggered walk simply runs to its
   real answer instead of guessing under a cap sized for a different use case.
4. **Naming cleanup** on the same unresolved review threads: `hierarchy_tests.rs` (`u`, `idx`)
   and the `references_verify.rs` test module (`src`, `u`, `idx`, `col`) use abbreviated
   identifiers AGENTS.md disallows. Sweep these into the same commit since the fix already
   touches both files.

### 6c — rename

**Local-variable fast path** (independent critique finding — also defuses the refusal-rate
risk below for the common case): when the symbol is a local/lambda-param whose declaration
and every reference live in one file, rename via `local_scope_occurrences` directly —
single-file CST subtree walk, no rg, no index, no cross-file verification. Every occurrence
from this walk is `CstResolved` by construction, so this path never refuses on receiver-type
gaps. This is also a strict improvement over today: today's local rename
(`rename.rs::enclosing_scope`) is a brace-depth **text** scan over lines, not a CST walk —
exactly the string-parsing class the parent design eliminates.

**Cross-file path — override detection (runs first, symmetric):**

A rename's cursor may land on either side of an override relationship — the interface/base
method, or a concrete override — and either side must detect the same relationship. Given the
query's declaring type `Q` (from `resolve_identity`) and each *Declaration*-role candidate's
enclosing type `C` in the recalled set (recall — rg/index — is unchanged from 6b, this classifies
it), check **both directions** with the uncapped walk from 6b-hardening fix 3:
`receiver_type_agreement(C, Q)` (C is a subtype of Q — C overrides Q) OR
`receiver_type_agreement(Q, C)` (Q is a subtype of C — the cursor is on the override, C is the
base). Either returning `Inherited` proves an override relationship exists in the candidate set.
`VerifiedReferences` gains a field carrying this: `proven_overrides: Vec<Location>` — the
Declaration-role candidates that resolved `Inherited` in either direction. (`kept`/`rejected`
keep their existing 6b meaning and shape unchanged; find-references ignores the new field. This
is computed once, inside `verify_candidates`, at the same point the Declaration-arm fix already
runs the agreement check — not a second scan.)

If `proven_overrides` is non-empty: **refuse**, reason = "renaming an override relationship
across `{Q}`/`{the override's enclosing type}` is not supported — rename the exact declaration
you need". This is a structural fact, proven by a completed walk (fix 3) — retrying will not
change it. This is the locked non-goal ("cross-file type-hierarchy-wide rename semantics...
follow Kotlin LSP conventions later"): 6c intentionally does not support renaming a member that
participates in an override relationship in either direction, because renaming only one side
produces non-compiling Kotlin and full hierarchy-wide rename is out of scope.

Because this check now runs the walk to actual completion (fix 3) rather than under the
interactive-feature cap, `Unresolvable` on a Declaration-role candidate is a genuine "this type
truly isn't known to the index" result, not a budget artifact — e.g. a raw external type with no
recoverable symbol info at all, where retrying later (once/if that type becomes indexable) could
change the answer. Treat it the same as today: the candidate stays `NameScan`, not a forced
refusal, since by construction the walk already tried as hard as it reasonably can right now.

**Cross-file path — edit-set assembly (only reached when no override was proven):**
1. Classify the cursor symbol via `resolve_identity`. Refuse unless it resolves to exactly one
   definition location (`resolve_identity`'s `locs.len() == 1` — `resolve_identity` itself does
   not enforce uniqueness, this check is 6c's own) that is not library-owned
   (`indexer.is_library_uri`) — reason "defined in a library" for the latter, "identity is
   ambiguous" for the former.
2. Collect 6b's verified reference set (`VerifiedReferences`). Drop `rejected` candidates from
   the edit set silently — they're proven a different identity, the same as any other candidate
   the recall scan never should have produced; excluding them is a strict improvement over
   today's zero-verification rename, not a refusal condition.
3. Remaining `NameScan` residue is **included** in the edit set as-is (see Policy gate below) —
   no additional filter is applied. (An earlier draft of this spec proposed reusing
   `references.rs`'s `has_wrong_qualifier_at_col` as a subtractive filter here; that function
   compares a call site's *receiver variable name* against a *type name* and is gated to
   uppercase names by its only real caller for exactly that reason — a variable is essentially
   never spelled like its type. Applied to lowercase member renames, the target population here,
   it would either no-op or wrongly exclude genuine references. No replacement filter is
   proposed; see Policy gate and Testing for what this residual risk actually is.)
4. Refuse only on: identity not unique or library-owned (step 1), an override relationship
   proven above, or zero candidates. Refusal = LSP request error with a human-readable reason
   string (Helix shows it in the status line). Success = `WorkspaceEdit` over the edit set from
   steps 2-3, logging CstResolved / NameScan-included / rejected-excluded counts for
   observability.

**Policy gate — resolved with live data (independent critique finding, was unresolved by
design; resolved here, before 6c implementation, per the spec's own condition: "if refusal is
common, that's a regression worth knowing about, not shipping silently. Before locking the
policy, 6c's live probe must measure the refusal rate on a real multi-call-site member rename
in the actual project").**

Measured against the real ~18k-file Moneta monorepo, full workspace indexing complete
(`$/progress` "kmp-lsp/indexing" end observed before querying — an unindexed workspace can't
resolve receiver types at all and would produce misleadingly pessimistic numbers):

- Single-implementor interface member (`ICacheManager.clearAllCaches`, one real override, 5
  candidates): 4 `CstResolved`, 1 `NameScan` (the override's own declaration — pre-hardening-fix
  artifact), 0 `rejected`.
- Genuine multi-implementor interface member (`IAppSettings.putString`, 278 real candidates
  across the codebase): 24 `CstResolved` (9%), 235 `NameScan` (85%), 19 `rejected` (7%).

**Both measured scenarios have real overrides**, so under the design above both refuse at the
override-detection step regardless of the `NameScan` policy — the softened policy below never
actually applies to either probed case. Read honestly, this data proves two different things:
(1) the verification/budget machinery holds up at real scale — 19 real decoys caught out of 278
candidates, not a synthetic-fixture-only result — and (2) override detection was the load-bearing
gap: before the Declaration-arm fix, neither scenario could even be correctly refused, because
the override declaration was indistinguishable from "couldn't verify." It does **not** directly
demonstrate the value of including `NameScan` residue, because the two members chosen for live
measurement both happen to participate in overrides. The `NameScan`-inclusion policy therefore
rests on the same reasoning the spec used before any measurement existed — today's rename already
ships zero-candidate-verification, so excluding `rejected` (proven-wrong) candidates is a strict
improvement with no new risk, while including `NameScan` (unverifiable, not disproven) candidates
carries exactly today's existing trust level forward, not a new one. This applies to the
population the live probe didn't happen to sample: multi-call-site members with **no** override
relationship (concrete-class methods, top-level functions, properties) — plausibly the more
common shape for a "many call sites" member outside an interface-heavy area of a codebase, but
this spec does not claim to have measured that population directly. 6c's own live-probe testing
step (below) should include one such non-override multi-call-site member specifically, to close
this gap with real data before merge.

### Error handling

- Classifier returns `None` → features fall back exactly as today (never an error).
- Repair-bounded acquisition inherited from `lambda_doc_at`; classification on a broken tree
  that repair can't fix degrades to `NameScan`, never wrong-identity.
- Rename refusals are errors BY DESIGN and carry reasons; all other features never error.

### Testing

House decoy for every sub-slice — two classes with identically-named members
(`User.save()` / `File.save()`):
- 6a: go-def from `user.save()` hits `User.save` only (decoy: `assert` the `File.save`
  location is absent); local highlight does not light the same name in another function.
- 6b: find-refs on `User.save` excludes `File.save` call sites; unverifiable site stays in
  the result as `NameScan` (recall pin).
- 6c: rename refuses when the cursor identity isn't unique or is library-owned (assert the
  error reason); refuses on a real override relationship from **both** directions — one test
  renaming from the interface/base method, one from the concrete override, same fixture,
  both must refuse with the override reason, neither silently renames one side only; renames
  the full edit set (`CstResolved` ∪ `NameScan`, `rejected` excluded) in a clean fixture with
  **no** override relationship (a top-level function or a concrete, non-overriding method with
  multiple call sites); a `NameScan` candidate that is a genuinely different symbol (the
  `File.save` decoy, receiver type unresolvable — e.g. behind a scope-function lambda) is the
  known residual risk this policy accepts: assert that it currently *does* get included in the
  edit set (a "this is the accepted gap" pin, not a "this is caught" claim — if a future fix
  narrows this, this test should start failing and be updated, not silently pass either way).
- Cursor in string/comment → no navigation result (noise-kill pin).
- Existing navigation suites are the floor — `NameScan` parity means they pass unchanged.
- Live probe per sub-slice on the real project before its merge (go-def on a Compose member
  chain; find-refs on a same-named workspace member; rename refusal on a jar symbol, and on a
  real override from **both** directions — base and concrete side; re-measure the
  6b-hardening budget-accounting fix against the `IAppSettings` scenario above and confirm
  `NameScan` share drops from the pre-fix 85%; **and** measure a genuine multi-call-site member
  with **no** override relationship — the Policy gate's `NameScan`-inclusion reasoning was not
  directly measured on this session's two probed members, both of which turned out to have
  overrides and refuse before that policy is ever reached).

### Known limitation / follow-up needed

`infer_type_in_lines_raw` (`src/resolver/infer_lines.rs`, reached via `find_var_type` /
`infer_type_in_lines_raw`'s doc comment) does a whole-file, cursor-position-blind scan for a
variable's type — unlike lambda parameters, which go through the properly scoped
`find_contextual_type` path. Two same-named parameters/locals with different types anywhere in
one file can cause wrong type inference at either site (confirmed, not theoretical). 6c's
CST-verified rename is the first consumer where this can turn into a silently wrong *edit written
to disk*, rather than only a wrong hover/inlay/chain-resolution display — all of which share this
same plumbing. Fixing the underlying scan to be scope-aware is a separate, properly-scoped
follow-up, not part of this slice; see the doc comment on `infer_type_in_lines_raw` for the
in-code pointer.
