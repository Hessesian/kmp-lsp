# Inferred-receiver find-references discovery gap — design and implementation plan

> **2026-09-22.** Drafted by an Opus planning pass over three scout reports (rg.rs/references.rs
> architecture, AGENTS.md + resolution design docs, recent-PR/inference precedent), against `main`
> tip `6a6cf5b6`. Scouting and this plan were produced by fresh sub-agents with no access to the
> reporting session's earlier back-and-forth — every claim below was re-derived from current code,
> not carried over from stale memory.

## The bug

`textDocument/references` on a data-class field/property returns zero results when invoked from the
field's own declaration line, if every real usage site accesses the field through a variable whose
type is only known via transitive inference — e.g.

```kotlin
val response = repository.openBody()   // Body declared as the return type of an interface method,
response.isOnline                       // never named/imported directly in this file
```

Real-world repro (external project, not in this repo, evidence only): `ApplicationOpenResponseBody`
(fields `isOnline`, `ntcLoanEnabled`, `ntcDepositEnabled`) declared in
`core/data/.../ApplicationOpenResponseBody.kt`; real usages exist only in `ScreenFlowInteractor.kt`
across ~6 unrelated feature modules, none of which import or textually mention the class.

## Root cause

`field_scoped_reference_locations` (`src/rg.rs:1160-1229`) is the candidate-file-discovery stage for
member references at their declaration site. It:

1. Builds `candidate_files` by rg-scanning the workspace for files that **textually match
   `\b{owner_class}\b`** (rg.rs `rg_files_with_matches_scoped`).
2. Always merges in the declaring file.
3. **If `candidate_files` is empty, returns `vec![]` immediately (rg.rs:1185) — no fallback.**
4. Only then searches those candidate files for the field name.

Files that reach the field purely through an inferred-type receiver never mention the class name, so
they're excluded at step 1, before any real type-checking runs.

## Corrections to the initial brief (verified against current code)

1. **`field_scoped` never fires for methods.** `field_owner_for_decl` (`references.rs:812-830`)
   filters on `SymbolKind::PROPERTY | VARIABLE | FIELD` only. Interface *methods* on-decl get
   `parent_class = enclosing_class` (`references.rs:392`) and route through
   `parent_scoped_reference_locations` instead.
2. **`package_scoped_reference_locations` is unreachable for members at all** — it requires
   `parent_class.is_none() && declared_pkg.is_some()`, i.e. a **top-level** lowercase declaration.
   Top-level functions/extensions are unreachable without an import or same-package in the first
   place; there is no inferred-receiver path to them. **Ruled out of this fix, with a real reason**
   (see Task/Test C below, which pins this as a deliberate non-goal).
3. **`parent_scoped_reference_locations` has the same conceptual gap through a different mechanism**
   — not the empty-guard, but its bare-name pass (rg.rs:1016-1019) is restricted to
   `import[^\n]*\bParent\b` files. A caller holding the receiver via inference, without importing the
   interface, is missed identically. **In scope.**

## Scope confirmed by scouting

- `owner_scoped_reference_locations` (rg.rs:1085-1145, lowercase methods declared inside a
  doubly-nested class, e.g. `create` inside `Factory` inside `Reducer`) has the **identical**
  text-scan-then-empty-guard shape. Second call site with the same bug class.
- The gap is **gated to declaration-site queries only** (`resolve_scope_with_qualifier`,
  references.rs:339-450, the `on_decl` check at 401-410/429). Find-references invoked from a *usage*
  site instead takes an unscoped `run_rg_search` full-workspace path, filtered post-hoc by
  `verify_candidates`. Whether that path already succeeds for this shape is architecturally plausible
  (see below) but **unconfirmed by any existing test** — treated as a separate open question, not
  silently assumed fixed by this plan.
- `verify_candidates` (`src/features/references_verify.rs`, receiver-type-agreement via
  `walk_hierarchy`) is **fully receiver-type-based and independent of file text content**. Downstream
  verification needs **no changes** — only the discovery/candidate-file stage does. This is also why
  the chosen mechanism (below) is safe: verify is the correctness backstop over anything discovery
  proposes.
- No recent PR (#272, #286-290, #298-322, including the extension-registry family #321/#322 and
  hierarchy-walk/workspace.json #286-290) touches `src/rg.rs`, `src/features/references.rs`, or
  `src/features/references_verify.rs`. Genuinely untouched gap, not a rediscovery — and distinct from
  the memory-tracked "field supertype-walk gap" (`find_field_type_in_class` not walking supertypes,
  `resolver/infer.rs:534`), whose own audit pass explicitly checked `features/references.rs` and
  found nothing at the time.
- Type inference itself (`Indexer::find_var_type` → `infer_variable_type_from_cst` →
  `resolve_call_expr_type` → `method_return_type`/`find_fun_return_type_reachable`) is import-aware
  and cross-file by design (PR #192) — plausibly already resolves `response`'s type as `Body` from
  `repository.openBody()`'s declared return type, for hover/goto-def. No existing test covers this
  exact shape, so don't rely on it without adding one.
- Neither the `Resolver` trait (`src/resolver/api.rs`) nor `CstQuery` (`src/indexer/infer/mod.rs`)
  exposes corpus-wide "which files have a variable typed as X" — both are per-position. Any fix
  either needs a bounded rg-based broadening heuristic (reusing `verify_candidates` as the backstop)
  or new corpus-scanning machinery. The former is recommended; see below for why the latter is
  rejected.

## Departure from the 6b design's locked non-goal — stated explicitly

`docs/superpowers/specs/2026-07-20-cst-find-references-design.md` (Slice 6b, shipped as PR #228,
the design doc for the current `verify_candidates` machinery) locks *"Recall is untouched — the
existing rg + index candidate search stays exactly as today"* with an explicit non-goal of
*"changing rg's scope-narrowing inputs (`parent_class`/`declared_pkg`/`owner_class`)."*

**This plan deliberately breaks that.** 6b was a *verification-layer* slice; the discovery-layer
boundary was scoped out for that one slice, never declared permanent, and no slice since has
revisited it. This is exactly the lesson in `docs/architecture/unified-resolution-strategy.md`'s
2026-09-14 addendum: *"a 'scoped out for now' boundary needs an expiry, not just a label."* The gap
found here lives precisely on the boundary 6b routed around. 6b's own machinery — a receiver-type
verify pass independent of file text — is what makes widening discovery safe now: it needs zero
changes to absorb a wider candidate set.

That same addendum's other lesson — *"a root cause named once is not fixed once it's fixed in one
place; grep every sibling function solving the same kind of problem"* — is why this plan covers
`field_scoped_reference_locations` **and** `owner_scoped_reference_locations` together, not just the
reported instance.

## Recommended mechanism: one bounded producer hop, reusing rg + `verify_candidates`

**Rejected alternative — widen to an unscoped `--word-regexp` scan whenever the candidate set is
empty/thin.** `verify_candidates` caps IO at `MAX_VERIFICATION_IO_OPERATIONS = 48`
(`references_verify.rs:13,60`); budget exhaustion **keeps** unresolved candidates as `NameScan`
rather than rejecting them. On an ~18k-file monorepo, a common name like `isOnline` would return
hundreds of unverified false positives once past that budget. Widening recall past what verify can
actually check converts a miss into noise, not a fix.

**The fix — bounded depth-1 transitive hop.** Usage files never mention `Body`, but they do mention
the *producer* — `repository.openBody()`. The producer's own declaration
(`fun openBody(): Body`) lives in a file the **existing hop-1 scan already returns**, because that
file names `Body`. So:

- **hop 1 (unchanged):** `rg_files_with_matches_scoped(r"\bOwner\b", …)`.
- **hop 1.5 (new):** `rg_pattern_in_files(owner_pattern, &hop1_files)` (existing helper, rg.rs:1631,
  already returns `(Location, String)`). Parse each line with a new
  `declared_member_name_returning(content, owner_class) -> Option<String>` (sibling of
  `is_declaration_of` / `is_java_field_declaration_at`): matches `fun NAME(…): …Owner…`,
  `val/var NAME: …Owner…`, and the Java `Owner NAME(` shape. Generic wrappers (`Flow<Owner>`,
  `List<Owner>`) fall out for free from the substring match.
- **hop 2 (new):** one more `rg_files_with_matches_scoped` over an alternation of those producer
  names, merged into `candidate_files` via the existing `extend_unique_files`.
- Then the **existing, unmodified** `rg_word_in_files(field_name, candidates)` and the **unchanged**
  `verify_candidates` backstop.

A hop-2 file must contain *both* a producer name *and* the member name by construction — narrow
enough that verify's budget is not stressed.

**Bounded and typed, per AGENTS.md's "reach for a type before a comment":**

```rust
const MAX_OWNER_PRODUCING_MEMBER_NAMES: usize = 8;
const MAX_PRODUCER_CANDIDATE_FILES: usize = 256;

enum ProducerExpansion { Expanded(Vec<String>), SkippedTooBroad }
```

`SkippedTooBroad` degrades to today's exact behaviour, so the common narrow case cannot regress.
Delete the `if candidate_files.is_empty() { return vec![] }` guards at rg.rs:1104 and rg.rs:1185 —
after the hop they're the wrong question; an empty hop-1 with a non-empty hop-2 is a legitimate,
productive state.

One shared helper, called from all three affected sites — "fix it once where all callers route
through," per this repo's own root-cause discipline.

## Tasks

**PR 1 — `fix(references): discover field references through inferred receiver types`**
(~300 lines, base of the stack)

1. `declared_member_name_returning` in `rg.rs` + unit tests (Kotlin `fun`/`val`, Java, generic
   wrapper, negative case: parameter type, negative case: local `val` with an initializer expression
   rather than a type annotation).
2. `producer_scoped_candidate_files(request, matcher, owner_class, hop1_files) -> ProducerExpansion`
   with the two caps above.
3. Wire into `field_scoped_reference_locations`; drop its empty-guard. No new provenance type needed
   here — the field path applies no owner-name-dependent filter downstream (rg.rs:1189-1227 only
   checks `is_declaration_of` / the Java-specific shape checks).
4. Tests A, C, D (below).

**PR 2 — `fix(references): sweep the inferred-receiver gap to owner- and parent-scoped discovery`**
(stacked on PR 1, ~250 lines)

5. `owner_scoped_reference_locations`: same hop, drop its empty-guard. **Needs provenance** —
   `qualifier_hints_owner` (rg.rs:1139) would reject every hop-2 hit, since the receiver there is
   named after the producer, not the owner class. Introduce `ProducerDiscoveredFiles(HashSet<String>)`
   with a `contains` check, and skip `qualifier_hints_owner` only for members reached through it;
   `verify_candidates` remains the backstop.
6. `parent_scoped_reference_locations`: merge the hop into `candidate_files` before the bare-name
   pass at rg.rs:1016.
7. Tests B, E (below).

Splitting the stack this way keeps the reported bug and its regression test in a reviewable base PR,
and isolates the widening of the two more heavily-relied-on paths — matching this repo's precedent
of preparatory/adjacent work stacked around a main slice.

## Test plan

All via `tempfile::tempdir` + real `rg`, per the existing pattern at `references_tests.rs:85-143`.
Every AGENTS.md-required test includes a competing/misleading decoy, not just the happy path.

**A. `field_reference_found_through_inferred_receiver_type`** — the repro.
- `a/Body.kt`: `data class Body(val isOnline: Boolean)`
- `a/Repo.kt`: `interface Repo { fun openBody(): Body }`
- `b/Caller.kt`: imports `a.Repo` **only**; `val response = repository.openBody()` then
  `response.isOnline`
- Decoy 1, `b/MentionsBody.kt`: names `Body` textually (`fun consume(body: Body) {}`) and declares its
  own unrelated `class Other(val isOnline: Boolean)` — must be absent (hop-1 file, dropped by
  `is_declaration_of`).
- Decoy 2, `c/Session.kt`: `class Session(val isOnline: Boolean)` plus `session.isOnline`, mentioning
  neither `Body` nor `openBody` — must be absent. This is the anti-regression assertion: it fails
  loudly if this fix is later "simplified" into an unscoped scan.
- Decoy 3, `c/FakeProducer.kt`: declares an unrelated `fun openBody(): Session` and calls
  `openBody().isOnline`. Reaches the candidate set via hop 2, and must land in
  `VerifiedReferences::rejected` — asserted directly, per the 6b design's own rule that a proven
  exclusion is an assertable fact, not a silent absence.

**B. `owner_scoped_method_reference_found_through_inferred_receiver_type`** — doubly-nested
`Reducer.Factory.create`; caller does `val factory = module.provideFactory()` then `factory.create()`,
never naming `Reducer`. Decoy: `overviewMapperFactory.create()` in a hop-1 file — must still be
excluded by `qualifier_hints_owner`, proving the bypass is scoped to hop-2 files only.

**C. `top_level_function_reference_stays_package_scoped`** — locks the ruled-out `package_scoped`
decision in a test: top-level `fun formatPrice()` in package `a`, importing caller in `b` (found),
uncalled same-named top-level `fun formatPrice()` in `c` (absent). Fails if `package_scoped` is later
widened without cause.

**D. `usage_site_field_reference_finds_sibling_usages`** — characterizes the architecturally
different unscoped usage-site path: cursor on `response.isOnline` in `Caller.kt`; assert the
declaration and all usages are returned and `c/Session.kt` lands in `rejected`. Expected outcome is
genuinely unknown going in — if red, that's a separate, distinct finding and gets its own follow-up,
not a silent scope expansion of this plan.

**E. `interface_method_reference_found_without_importing_the_interface`** — the `parent_scoped`
analogue of test A.

**Floor:** all existing `references_tests.rs` and `references_verify.rs` tests stay green unchanged —
this only widens recall; nothing already-found may be lost.

## Performance

Added cost per declaration-site member query: one `rg_pattern_in_files` over the already-small hop-1
set (a class name is rare), plus one scoped rg over an alternation of ≤8 producer names. Both caps
degrade to today's exact behaviour, so the common case — where the narrow scan is already correct —
pays one extra bounded rg call over a handful of files. No unscoped workspace scan is ever
introduced. Worth measuring the `isOnline` query against `/home/ocel/Work/Moneta/android` before
merge, using the ground-truth harness recipe in `docs/architecture/unified-resolution-strategy.md`.

## Critical files

- `src/rg.rs` — `field_scoped_reference_locations` (1160-1229), `owner_scoped_reference_locations`
  (1085-1145), `parent_scoped_reference_locations` (928-1021), `rg_pattern_in_files` (1631)
- `src/features/references.rs` — `field_owner_for_decl` (812-859), `resolve_scope_with_qualifier`
  (339-450)
- `src/features/references_verify.rs` — backstop, read-only, no changes needed
- `src/features/references_tests.rs` — tests A-E
- `docs/superpowers/specs/2026-07-20-cst-find-references-design.md` — the locked decision this plan
  deliberately departs from
