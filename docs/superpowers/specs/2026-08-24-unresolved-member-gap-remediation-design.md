# Unresolved-Member Gap Remediation — Design

Status: **proposed** (2026-08-24). Responds to `.superpowers/scout-unresolved-member-gaps-report.md`
(resolution-accuracy scan against a real 13,225-file Kotlin/Android codebase, run after PR #274/#275).
Builds on `docs/architecture/unified-resolution-strategy.md` (string-domain direction), the CST design
doc, and directly follows `docs/superpowers/specs/2026-08-24-qualified-resolution-unification-design.md`
(landed as PR #272 the same day) — several of that plan's own "coordination notes" turn out to matter
here. This document does not re-derive those three; it slots the scout report's six categories into the
architecture they already establish, correcting the scout report's own analysis where code-reading
during this pass found it wrong or incomplete.

## Context (why)

The scan found 19,968 member-ref Gaps, categorized by root cause into six buckets (A, A2, B, C, D, E, F)
with a proposed priority order (A → A2 → C → B → F → D/E). Each category's proposed fix was written
*before* checking it against this repo's established "which primitive already does this shape"
discipline — the same discipline the CST doc's reuse inventory and the qualified-resolution doc's
"two primitives, one shared pattern" section both apply. This document applies that check.

**Headline correction, found while verifying Category A:** the scout report's proposed fix (a bare
`find_field_type_via_supertypes` function, `.or_else()`-chained ad hoc) copies `method_return_type`'s
*shape* but not its *placement*. `Resolver::method_return_type` is a trait method — the single seam
both the string domain and the CST domain (`InferDeps::find_method_return_type_for_type` on `Indexer`)
call through. The field equivalent (`find_field_type_in_class`) is a bare free function called directly
by three unrelated sites (`Indexer::find_field_type`, `infer_field_chain_type`, and
`semantic_tokens/resolve.rs`'s bespoke walk) — none through a trait. That asymmetry, not just the missing
supertype walk, is the real gap. See Category A below.

**Second correction, found while verifying Category B:** `src/workspace_json.rs::detect_android_sdk_source_paths`
already exists, is already wired into both `Config::resolve_sources` (the live LSP indexing path) and
the CLI's `collect_cli_source_paths` — it detects `$ANDROID_HOME/sources/android-XX` (via
`local.properties`' `sdk.dir`, `ANDROID_HOME`, or `ANDROID_SDK_ROOT`) and indexes it as an ordinary
workspace source directory. The scout report's "neither is ever scanned" is too strong: for any
developer who has installed the optional "Sources for Android API XX" SDK component, `Activity`,
`Context`, `View`, etc. **already resolve** through this path today, with source-level fidelity (real
bodies, not stub signatures) — better than the JAR-sidecar path the scout report proposes building.
The real gap is narrower: this component is large and frequently *not* installed (very commonly absent
on CI runners and on developer machines that only pulled the SDK platform, not its sources), and there
is no fallback for that case, and no equivalent detection for `android.jar` itself (bytecode, no
matching source dir) or the JDK. See Category B below.

**Third finding, found while comparing A2 and C side by side:** both are the *same shape* of bug —
"the CST/type-inference engine already has the semantic knowledge to answer this correctly; the
string-domain symbol-identity path (`resolve_identity` → `find_definition_qualified` →
`resolver/resolve.rs`) never consults it." `chain.rs` already special-cases `SCOPE_FUNCTIONS` for type
inference; `resolve_qualified`'s uppercase branch never gets there for goto-def. `Resolver::method_return_type`
already walks supertypes; `resolve_qualified`'s uppercase branch never calls the equivalent
(`resolve_from_class_hierarchy`) either. This is not a coincidence — it is exactly the gap the CST design
doc's Goal 6 ("CST-aware navigation … the symbol-identity-at-cursor family … 'forgotten orphans'") already
named and scoped as a *future, post-catalogue phase*, not yet started. A2 and C are two concrete instances
of that predicted gap, arriving before Goal 6 does. See "A2/C: a shared root cause" below.

**Fourth finding, found while checking Category F's proposed mechanism against its own code:**
`LAMBDA_RESULT_FNS` does the *opposite* of what Category F needs. Its documented behavior — "the
inferred type is the lambda's last expression" — is an *unwrap*: `remember { Foo() }` → `Foo` (correct,
because `remember` backs a property-delegate that unwraps `.value`). `it.async { loadData() }` needs a
*wrap*: `Deferred<T>`, not `T` — `setupModelAsync.await()` must see a `Deferred`-shaped receiver, not the
loaded data's own type. Adding `async` to `LAMBDA_RESULT_FNS` as literally proposed would set
`setupModelAsync`'s type to the *loadData() return type*, and `.await()` would then fail to resolve on
*that* type too — the fix as specified would not close the gap it targets. See Category F below.

## Category-by-category primitive mapping

| Category | Domain | Primitive to extend | Verdict on scout's proposed fix |
|---|---|---|---|
| A — field supertype walk | Coordinated (one string-domain fix, reused by CST via the existing `InferDeps` seam) | `Resolver` trait (add `field_type`, mirroring `method_return_type`) | Right shape, wrong placement — promote to the trait, don't bolt on a free function |
| A2 — uppercase-qualifier hierarchy fallback | Pure string | `resolve_from_class_hierarchy` (already exists, already used by the `this`/`super` branches two cases up in the same function) | Correct as proposed — smallest possible diff |
| B — JDK/Android SDK indexing | Infra/indexing (neither domain — upstream of both) | `detect_android_sdk_source_paths` (exists, partially wired) | Root cause overstated; extend the *existing* mechanism, don't build a parallel JAR-sidecar path for the common case |
| C — universal scope functions | Pure string | `SCOPE_FUNCTIONS` (exists, CST-only today) | Correct root cause; needs a string-domain consumer, same shared-root-cause note as A2 |
| D — Compose dual-receiver scopes | CST (would need real modeling) | None — `LambdaScope` has no "enclosing composable scope" dimension | Scout's own "needs its own investigation" caveat holds; confirmed no shortcut exists |
| E — per-library JAR quirks | Unconfirmed | N/A | Not investigated further this pass — scout's own caveat holds |
| F — coroutine-builder return type | CST | Needs a **new** wrap-not-unwrap mechanism; `LAMBDA_RESULT_FNS` is the wrong list | Scout's named mechanism doesn't fix the bug it targets — see correction above |

## A2/C: a shared root cause, not two one-off patches

Both bugs are an instance of: *the symbol-identity-at-cursor path (goto-def, and via `resolve_identity`,
this benchmark) is purely string/name-based and does not consult the semantic knowledge the CST
type-inference engine already encodes.* The CST design doc's Goal 6 already named this exact family
("go-to-def / goto-impl … `CstResolve::receiver_type`/`expr_type` → resolve_member → Definitions, string
fallback") as future work, gated on Steps 1–5 of that doc's own sequencing, not yet started.

This has a real sequencing consequence: A2 and C should be landed as **narrow, deletable string-domain
patches** now (both are cheap, both close real Gap volume today), but **not** grown into a general
"teach the string domain more CST-shaped semantics" mechanism — that generalization is Goal 6's job, and
duplicating its scope piecemeal here would create exactly the kind of divergent-clone debt this repo's
past unification efforts (unified-resolution-handler, CST catalogue) were built to close. Each fix in
this plan is scoped narrowly enough that Goal 6, whenever it lands, can delete or absorb it without a
awkward migration — this is called out explicitly as a note for whoever picks up Goal 6 next, matching
the qualified-resolution doc's own "coordinate, don't execute" convention for its own Slice-4 note.

## Domain classification (answers the "does this change priority" question)

- **Pure string domain:** A2, C. Both touch only `resolver/*.rs`. Both are the cheapest, most
  isolated, most precedented fixes in this set — smaller than A even though A ranks first in the scout
  report's own ordering.
- **Coordinated, but thin:** A. The fix lands once in `resolver/infer.rs` (mirroring
  `find_method_return_type_via_supertypes`'s shape) and is exposed on the `Resolver` trait; the CST
  domain's `Indexer::find_field_type` (the `InferDeps` impl) picks it up with a one-line change —
  exactly the same "thin delegation" `Indexer::find_method_return_type_for_type` already demonstrates
  for methods. This is not a bigger lift than A2/C; it is a slightly wider one (touches `resolver/api.rs`,
  `resolver/infer.rs`, and `indexer.rs`'s two trait impls instead of one file), and the trait-placement
  decision itself is worth deciding correctly once rather than revisited later.
- **Pure CST (would need new modeling):** D, and — once correctly root-caused — F (a new wrap-shaped
  mechanism, not a `LAMBDA_RESULT_FNS` entry).
- **Infra/indexing, upstream of both domains:** B. Neither `resolver/` nor `indexer/infer/` needs to
  change; the fix is in what gets indexed at all (`workspace_json.rs`, `jar.rs`), which both domains
  then benefit from automatically once symbols exist.
- **Unconfirmed:** E — not enough evidence yet to classify.

**Priority re-ordering.** The scout report's order (A → A2 → C → B → F → D/E) ranks by estimated impact.
Re-ordering by architectural readiness + risk instead:

1. **C** before A2 and A — same root-cause family as A2, but even smaller: `SCOPE_FUNCTIONS` already
   exists and needs one new call site, zero new functions, zero new types. Best ratio of "prove the
   shared-root-cause pattern works" to actual code touched.
2. **A2** — same family, one more call added to an existing function
   (`resolve_from_class_hierarchy`) already used two branches above in the same function. No new types.
3. **A** — same conceptual fix as A2/C ("call the walk that already exists"), but requires the one
   real design decision in this plan (trait placement) plus touching two `impl` blocks and three call
   sites. Doing it third, after C and A2 have re-confirmed the "symbol-identity lags the type-inference
   engine" pattern twice on genuinely trivial diffs, de-risks the trait decision — if C/A2 surface any
   surprise (e.g. a reachability regression neither scout nor this doc anticipated), it's cheap to have
   found it before the wider A change.
4. **B** — largest total volume, but explicitly infra work with its own risk profile (filesystem
   detection, environment variables, no unit-testable "real Android SDK" fixture beyond the existing
   synthetic `local.properties`/`sources/android-XX` test doubles already in `workspace_json_tests.rs`).
   Independent of 1–3; could run in parallel by a different session, but sequenced fourth here because
   the *tightest, most-precedented* wins should land first and this genuinely needs its own dedicated
   design pass (see Non-goals).
5. **F** — re-scoped (see correction above) to "design a wrap-shaped mechanism" rather than "add a name
   to an existing list." Smaller total volume than B; deferred behind it because F's actual mechanism
   isn't designed yet, only mis-scoped.
6. **D, E** — unchanged from scout's own assessment: defer, need dedicated investigation first.

## Primitive A: `Resolver::field_type` (coordinated — string fix, CST reuse)

### WHY

`Resolver::method_return_type` (`resolver/api.rs:148`) is explicitly documented as "the single composite
for member resolution … checks extension functions and member functions … then walks the type's declared
supertypes," implemented as `find_method_return_type(...).or_else(|| find_method_return_type_via_supertypes(...))`
inside the trait impl. Nothing calls `find_method_return_type` directly except that impl and its own
`_via_class_hierarchy` helper — the composite is the *only* door.

`find_field_type_in_class` (`resolver/infer.rs:1124`) has no such composite and no such door: it is
called directly by `Indexer::find_field_type` (`indexer.rs:430`, the `InferDeps` seam consumed by
`chain.rs`/`expr_type.rs`), by `infer_field_chain_type` (`resolver/infer.rs:220`, the hover/goto-def
chain walker), and by `semantic_tokens/resolve.rs:405` (the bespoke walk the CST design doc's Slice 2
already schedules for deletion). Three call sites, no trait, no supertype walk. Adding a bare
`find_field_type_via_supertypes` and `.or_else()`-chaining it at each of those three sites (the scout
report's literal proposal) would work, but it re-creates the exact "same fix in 2–3 places" pattern this
whole line of unification work exists to close — one of those three sites is a function this repo has
already flagged for deletion.

### WHAT

Add `field_type` to the `Resolver` trait, same signature shape as `method_return_type` but carrying the
declaring `Url` (fields need it for chain re-anchoring the way methods currently don't — see the
qualified-resolution doc's own "method-return lookups are not yet Url-aware" coordination note, which
this addition does not change):

```rust
/// Resolve the declared type of `field_name` on a receiver whose type's base
/// name is `type_name`, together with the `Url` of the file where that field
/// is actually declared (the reachability anchor for the *next* hop in a
/// dotted chain — see `find_field_type_in_class`'s own doc comment for why
/// this differs from `method_return_type`, which does not yet carry one).
///
/// The single composite for field resolution: checks `type_name`'s own body
/// first, then walks its declared supertypes (with type-argument
/// substitution) — the field-typed sibling of `method_return_type`.
///
/// Returns `None` when no field (own or inherited) with a declared type is found.
fn field_type(
    &self,
    type_name: &str,
    field_name: &str,
    from_uri: &Url,
) -> Option<(String, Url)>;
```

Deliberately **not** wrapped in a new `FieldType` newtype: `ReturnType` already names a *method-return*
value specifically (its doc comment and every existing caller assume that), and the `(String, Url)` pair
is already the established shape for this exact value everywhere it flows today
(`find_field_type_in_class`, `InferDeps::find_field_type`, `ResolvedType`'s future `declaring_uri` field
per the qualified-resolution doc). Introducing a fourth name for the same shape would be new vocabulary
without new information — the tuple is already self-documenting at every call site via its parameter
names.

`find_field_type_via_supertypes`, the missing half, mirrors `find_method_return_type_via_supertypes`
exactly: `walk_hierarchy` over `class_base`'s ancestors (JAR-promotion-aware, cycle-safe, capped),
`substitute_direct_supertype_args` for the direct supertype's own generic parameters, returning as-is for
deeper ancestors (same documented limitation `find_method_return_type_via_supertypes` already accepts).
Reuses `find_field_type_in_class_impl`'s existing `MAX_RAW_TYPE_INFER_DEPTH` budget-threading — this
function sits in the exact mutual-recursion cycle PR #275 just bounded (`find_field_type_in_class` ↔
`infer_variable_type_raw`), so the new supertype-walking function must accept and pass through the same
shared depth counter from day one, not add an unguarded parallel recursion.

### HOW

```rust
impl Resolver for Indexer {
    fn field_type(&self, type_name: &str, field_name: &str, from_uri: &Url) -> Option<(String, Url)> {
        find_field_type_in_class(self, type_name, field_name, from_uri)
            .or_else(|| find_field_type_via_supertypes(self, type_name, field_name, from_uri))
    }
}
```

Then, mirroring `Indexer::find_method_return_type_for_type`'s existing one-line delegation into
`Resolver::method_return_type`:

```rust
// src/indexer.rs — InferDeps impl
fn find_field_type(&self, class_name: &str, field_name: &str, uri: &Url) -> Option<(String, Url)> {
    if let Some(type_name) = synthetic_enum_field(self, class_name, field_name) {
        return Some((type_name, uri.clone()));
    }
    crate::resolver::Resolver::field_type(self, class_name, field_name, uri)
}
```

This one change is what makes CST consumers (`chain.rs::resolve_member_type_on`,
`expr_type.rs::infer_navigation_expr_type`) pick up the supertype walk automatically — both already
thread the `(String, Url)` `InferDeps::find_field_type` returns (PR #272's own Primitive 2 work), so
nothing downstream of the trait needs to change.

`infer_field_chain_type` (`resolver/infer.rs:220`) switches its direct `find_field_type_in_class` call to
`Resolver::field_type` — a one-line swap, same return shape.

`semantic_tokens/resolve.rs:405`'s bespoke walk keeps calling `find_field_type_in_class` directly for now
(its own deletion is already scheduled by the CST doc's Slice 2, independent of this plan) — noted, not
touched, to avoid scope creep into a different unification effort's territory.

### Testing

- RED: a two-level generic-superclass fixture mirroring the scout report's `ContactAddressScreen`
  evidence — `abstract class MviViewModel<S, E> { val uiState: S; val effect: E }` /
  `class ContactAddressViewModel : MviViewModel<ContactState, ContactEffect>()` — assert
  `Resolver::field_type(idx, "ContactAddressViewModel", "uiState", uri)` returns `("ContactState", ...)`.
  Confirm it fails today (falls through to `None`, since `find_field_type_in_class` only reads the
  receiver's own body).
- GREEN after the trait method lands.
- A decoy: a field with the *same name* declared both on the subclass and an unrelated sibling class
  reachable from the same file, confirming the composite doesn't accidentally widen to an unscoped
  by-name scan (this is exactly the kind of regression `find_name_scoped_to_container`'s existence in
  this codebase already guards against elsewhere — same discipline, different function).
- One CST-domain regression test (hover or inlay on a chain through an inherited field) as the "fixed
  once, propagates automatically" proof — the whole point of routing through the trait instead of
  patching `chain.rs` separately.

## Primitive A2: `resolve_qualified`'s uppercase branch — add the hierarchy fallback

### WHY / WHAT

Confirmed by reading `resolve_qualified` (`resolver/resolve.rs:913-1003`) in full: after the per-`qual_loc`
candidate loop (companion lookup → nested-segment walk via `find_name_scoped_to_container` → final-segment
`find_name_scoped_to_container`) comes up empty for every candidate, the function falls straight to the
extension-entries check — `resolve_from_class_hierarchy` is never called in this branch, even though it
is already called two cases above (the `this`/`super` branches, lines 924-933) for exactly this situation.
This is scout's own root-cause finding, verified accurate as written — no correction needed here, only the
placement question (see below).

### HOW

Add the fallback after the candidate loop, before the extension-entries block:

```rust
for qual_loc in &qual_locs {
    // … existing companion / nested-segment / final-segment logic …
}
if let Some(location) = resolve_from_class_hierarchy(indexer, name, from_uri) {
    return vec![location];
}
// … existing extension-entries fallback …
```

Note this passes `from_uri` (the *caller's* file), not `qual_loc.uri` (the object's declaring file) —
matching `resolve_from_class_hierarchy`'s existing signature and its two existing call sites in this same
function. This is deliberately not "fixed" to re-anchor on `qual_loc.uri` as part of this step: that would
be importing Primitive 1's per-hop-reachability concern (the qualified-resolution doc's own territory)
into a fix that doesn't need it — `AccidentComponentManager` (the object) and the call site are typically
the same reachability context in this shape (module-local object + call), and over-scoping this fix risks
solving a problem A2's own evidence doesn't show. Flagged as a known simplification, not silently assumed.

### Testing

- RED: the scout report's own evidence shape — `object Manager : AbstractManager<T>()` with
  `AbstractManager` declaring `requireComponent()`/`bindInstanceToActivity()` — assert
  `resolve_symbol(idx, "requireComponent", Some("Manager"), uri)` finds `AbstractManager`'s declaration.
  Confirm RED against current code.
- GREEN after the one-line addition.
- Decoy: a same-named method on an *unrelated* class reachable from the same file (not a supertype of
  `Manager`) must not be picked up — proves the fallback is genuinely hierarchy-scoped, not a blanket
  by-name rescue.

## Primitive C: universal scope functions in the string-domain resolution path

### WHY / WHAT

Confirmed by reading `resolve_identity_with_io` (`indexer/infer/cst_symbol.rs:354`): the member-ref path
calls `find_definition_qualified(name, Some(receiver_type), uri)` unconditionally — no `SCOPE_FUNCTIONS`
check anywhere upstream. `SCOPE_FUNCTIONS` (`indexer/infer/lambda.rs:19`) is imported and consulted only
inside `chain.rs` (type inference), never from `resolver/`. Scout's root cause is accurate as written.

### HOW

The cleanest seam is `find_definition_qualified` itself (`indexer/lookup.rs:42`), since both goto-def and
this benchmark's `resolve_identity` funnel through it: before delegating to `resolve_symbol`, check
whether `name` is in `SCOPE_FUNCTIONS` (plus `takeIf`/`takeUnless`, already in the list) — if so, resolve
directly to the one well-known kotlin-stdlib declaration site instead of attempting a receiver-scoped
member search that can structurally never succeed. `SCOPE_FUNCTIONS` lives in `indexer/infer/lambda.rs`
(CST module) but is already `pub(crate)` and imported cross-module by `chain.rs`; importing it into
`indexer/lookup.rs` follows the same existing cross-module convention, not a new one.

The "one well-known declaration site" needs the kotlin-stdlib JAR's already-indexed location for e.g.
`kotlin.let`/`kotlin.apply` — reuse whatever lookup already answers "where is this stdlib top-level
function declared" for a *bare* (no-receiver) reference to the same name, since `let`/`apply`/etc. are
themselves indexed as ordinary top-level extension functions once the Kotlin stdlib JAR is promoted; the
fix is routing the *receiver-typed* lookup to fall back onto that *bare* lookup for exactly this closed
list of names, not building a new hardcoded location table.

### Testing

- RED: `build().apply { show() }`-shaped fixture (scout's own evidence) — assert
  `find_definition_qualified("apply", Some("SomeBuilderResult"), uri)` resolves to `kotlin.apply`'s
  declaration instead of returning empty.
- GREEN after the fix.
- Decoy: a **user-defined** function also named `apply` on the specific receiver type (a real, if
  unusual, shadowing case) must still resolve to the user's own declaration, not the stdlib one — proves
  the fallback only fires when the ordinary receiver-scoped search has already failed, not
  unconditionally for every name in the list.

## Non-goals

- **Category B is not fully designed here.** This document identifies that the fix should extend
  `detect_android_sdk_source_paths` (source-based, higher fidelity) with a JAR-sidecar fallback for
  `android.jar` when SDK sources aren't installed, and separately flags the JDK stdlib gap as real and
  currently wholly unaddressed by any existing mechanism — but sizing the JDK piece (full `jrt-fs.jar`
  indexing vs. a hardcoded stub table for the handful of dominant types, per scout's own tiered proposal)
  needs its own dedicated design pass, not a subsection of this one. This plan's migration sequence
  treats B as a later, separately-scoped unit of work.
- **Category F's actual fix is not designed here.** This document's contribution is the correction (the
  named mechanism is wrong) and the domain classification (CST, needs new "wrap the lambda's inferred
  type in a known generic template" logic — a real design decision: is this a new const list of
  `(name, wrapper_template)` pairs analogous to `NUMERIC_CONVERSION_FNS`'s shape, or something richer?).
  Designing that mechanism is out of scope here; it is not yet even confirmed which coroutine builders
  besides `async` matter enough to include.
- **Categories D and E are not investigated further.** Scout's own confidence levels (MEDIUM, LOW) and
  "needs its own investigation" framing are accepted as-is; nothing this pass found changes that
  assessment.
- **Not the CST design doc's Goal 6** (general CST-aware navigation for the symbol-identity family).
  A2 and C are narrow, deletable interim patches in the *shape* Goal 6 will eventually generalize, not an
  attempt to build Goal 6 piecemeal. See "A2/C: a shared root cause" above.
- **Not re-opening the qualified-resolution-unification plan.** That plan's own steps (Primitive 1/2,
  `find_name_scoped_to_container`, the `Url`-threading in `chain.rs`/`expr_type.rs`) are confirmed landed
  (PR #272) during this pass's verification and are treated as a stable foundation, not touched here.
- **Not adding `Vec<Location>`/overload-set semantics anywhere** — orthogonal, already tracked as its own
  follow-up per `unified-resolution-strategy.md`.
- **Not attempting full JDK/Android SDK indexing as part of Category B's slice in this plan** — explicitly
  deferred to Category B's own future design pass (see above), including whether it belongs in this
  server at all versus a separately-shipped stub table.

## Migration (incremental, test-anchored — mirrors the qualified-resolution doc's own sequencing style)

Each step lands independently, is green on `cargo test --bin kmp-lsp` before the next. Marked **(B)**
where a RED test must fail against current code first (migration-is-the-fix), or **(new)** for additive
work with no behavior to regress yet.

1. **(new) Category C.** Add the `SCOPE_FUNCTIONS` check to `find_definition_qualified`
   (`indexer/lookup.rs`). RED-then-GREEN per the Testing section above. Smallest, most isolated step in
   this plan — lands first to re-confirm the "symbol-identity lags type-inference" pattern cheaply before
   spending a design decision on Primitive A.

2. **(B) Category A2.** Add the `resolve_from_class_hierarchy` fallback to `resolve_qualified`'s
   uppercase branch. RED-then-GREEN per the Testing section above. Independent of step 1 (different
   function, same file) — could land in either order, sequenced second here only because it's the next
   smallest.

3. **(new) Primitive A, part 1 — land `Resolver::field_type` + `find_field_type_via_supertypes`
   additively.** Add the trait method and its supertype-walking implementation; wire `Indexer`'s
   `Resolver` impl. No caller changes yet — `find_field_type_in_class` keeps its existing direct callers
   for this step. Unit tests directly against `Resolver::field_type` (RED-then-GREEN, per the Testing
   section above), independent of any caller migration.

4. **(A, no intended behavior change) Primitive A, part 2 — route `Indexer::find_field_type` (the
   `InferDeps` seam) and `infer_field_chain_type` onto `Resolver::field_type`.** Two call-site swaps.
   Existing CST regression suites (hover/inlay/semantic-tokens on inherited-field chains) are the
   behavior net for the "own-class" case (must stay green — this step is additive for those, since
   `find_field_type_in_class`'s own-class behavior is unchanged); the new supertype-walk RED test from
   step 3, run again through the CST path this time (an inlay/hover fixture on
   `viewModel.uiState`-shaped code), is the **(B)** half of this step — confirms the fix that landed once
   in `resolver/` actually reaches the CST domain through the seam, not just the string-domain unit test.
   `semantic_tokens/resolve.rs` is explicitly left calling `find_field_type_in_class` directly (its
   deletion belongs to the CST doc's own Slice 2, not this plan).

5. **(deferred, coordinate not execute) Note for Category B's future design pass:** the field-type
   composite this step lands (`Resolver::field_type`) is exactly where a future JDK/Android-SDK stub
   table (Category B's tier-2 proposal) would plug in as another `.or_else()` — a fixed lookup table for
   `BigDecimal.ZERO`-shaped constants fits the same composite shape as the supertype walk. Left as a note
   for whoever designs Category B, not scheduled here.

## Testing & verification

- `cargo test --bin kmp-lsp` green after every step (binary-only crate; `--lib` runs 0 tests). Focused
  loops while iterating: `-- resolver`, `-- indexer_tests`, `-- chain`, `-- expr_type`.
- Every step gets a RED-before-GREEN decoy test in the same "same-named sibling/shadow" shape that
  motivated the qualified-resolution doc's own tests — this codebase's dominant real-world failure
  pattern (MVI sealed hierarchies, DI object managers) is collision-prone by construction, and a
  happy-path-only test (the qualified-resolution doc's own cited cautionary example,
  `resolve_dotted_name_traverses_deep_nesting`) proves nothing about it.
- `find_referencing_symbols` (Serena) on `find_field_type_in_class`, `find_method_return_type`, and
  `resolve_from_class_hierarchy` before and after steps 3–4 — confirms the exact blast radius claimed
  above (3 callers for fields, none for the member-only method half) and that no caller was missed.
- Ground-truth harness (per `unified-resolution-strategy.md`'s debugging recipe) or a re-run of the
  resolution-accuracy CLI benchmark itself against the real corpus — this is the one plan in this
  document's lineage that can *directly* re-measure its own impact: Gap counts for `uiState`/`effect`/
  `requireComponent`/`apply` should drop measurably after steps 1–4, the same before/after trend-metric
  framing the benchmark's own design doc establishes.

## Risks

- **Depth-budget reuse (Primitive A).** `find_field_type_via_supertypes` walks the same mutual-recursion
  territory PR #275 just bounded (`find_field_type_in_class` ↔ `infer_variable_type_raw`, shared
  `MAX_RAW_TYPE_INFER_DEPTH`). Adding a *third* participant in that cycle without threading the same
  shared counter through it would silently reopen the exact crash PR #275 closed — on the exact kind of
  large-real-codebase input (deep MVI generic hierarchies) this whole gap report came from. Mitigation:
  the WHAT/HOW sections above call this out explicitly as a hard requirement, not an afterthought; the
  RED test in step 3 should include a mutual-superclass-cycle fixture mirroring PR #275's own regression
  test (`find_field_type_in_class_terminates_on_a_mutual_field_reference_cycle`), not just a happy-path
  two-level hierarchy.
- **Trait-method placement (Primitive A) is a one-way door for its implementors.** Same risk class the
  qualified-resolution doc flagged for widening `InferDeps::find_field_type`'s signature: today there are
  exactly two `Resolver` implementors to update (`Indexer`; check whether a test double implements
  `Resolver` directly — `find_referencing_symbols` on the trait before landing, not assumed from this
  doc). Mitigation: land the trait method and its one real implementation together, not as a separate
  "add the trait, implement later" split.
- **CI stack-depth regression (Primitives A2/A), by analogy to PR #272's own landed history.** PR #272's
  `Option<(String, Url)>` widening in `expr_type.rs` deepened recursive call frames enough to overflow
  macOS CI's default test-thread stack, fixed by giving the affected tests an explicit 8 MiB stack
  (matching the existing `..._survives_a_pathologically_deep_file` convention). Any new deeply-recursive
  test fixture added for Primitive A's supertype walk (step 3/4) should default to that same explicit
  larger stack from the start rather than discover the macOS-only failure after the fact.
- **`find_name_scoped_to_container`'s degenerate-range fallback (PR #272's other landed regression) is a
  precedent, not a risk unique to this plan** — but Primitive A2's `resolve_from_class_hierarchy` call
  walks JAR-derived ancestor classes too (`walk_hierarchy` is explicitly "JAR-promotion-aware"), so a
  JAR-only stub class with a degenerate `.range` could hit an analogous edge in whatever range-based logic
  the hierarchy walk relies on downstream. Worth a specific JAR-superclass decoy test in step 2, not only
  a workspace-only fixture.
- **Category C's stdlib-location lookup could regress silently if the kotlin-stdlib JAR isn't yet
  promoted** at the time a scope-function reference is resolved (the same lazy-JAR-loading concern
  `unified-resolution-strategy.md` already tracks generally). Mitigation: the decoy test in Primitive C's
  Testing section (user-defined `apply` shadowing) also indirectly covers "stdlib JAR not yet loaded"
  falling through gracefully to the existing receiver-scoped search rather than crashing — but an explicit
  "JAR not yet promoted" unit test (mocking `InferDeps`/index state) would close this more directly than
  this plan currently specifies; flagged as a gap in this plan's own test coverage, not assumed safe.
- **Priority re-ordering risk.** This document ranks C/A2 ahead of A on architectural-readiness grounds;
  if the actual Gap-count impact of C/A2 turns out much smaller in practice than A once measured (the
  benchmark's own trend-metric framing is the way to check this concretely), the ordering should be
  revisited after step 2 lands and before committing further to step 3 — this plan's ordering is a
  reasoned starting point, not a commitment independent of what the benchmark shows once C/A2 are live.
