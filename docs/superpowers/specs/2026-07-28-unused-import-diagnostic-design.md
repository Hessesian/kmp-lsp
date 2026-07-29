# Unused-Import Diagnostic — Design

Status: **implemented and shipped** (approved design 2026-07-28, immediately following the
missing-import diagnostic's live-wiring work; two exemptions folded in after the benchmark run
against nowInAndroid surfaced real systematic false positives the original design didn't
anticipate — see "Detection algorithm" and "Benchmark results" below). Sibling feature to
`missing_import_diagnostics` — same dual property (a real LSP feature that also functions as a
precision benchmark for the resolution pipeline, measurable via a CLI harness against real
projects), deliberately narrower in scope than that feature's own detection logic in the
opposite direction (see "Detection algorithm" below for why).

## Context (why)

`missing_import_diagnostics` (this session) flags a bare reference that's importable elsewhere
but not reachable from the file's own scope. Its mirror image — an `import` statement whose
name is never used anywhere in the file — has never been implemented in this codebase at all.
Editors without a full IntelliJ/compiler backend (Helix, plain-LSP VS Code/Neovim setups) have
no way to catch this today. Like missing-import, it doubles as a benchmark: run over a
compiling real project, every flag is by construction either a genuine unused import (real
value) or a false positive (a gap in what counts as "used" — precision signal).

**Known and expected going in**: nowInAndroid is expected to be near-clean (small, actively
maintained sample app). Moneta (the ~12k-file real monorepo used for the missing-import
benchmark) is **not** expected to be clean here — the user has flagged upfront that it carries
genuine unused imports. Its flag count is not a pure false-positive signal the way nowInAndroid's
is; each flag needs eyeballing before being attributed to either "real dead import" or "detector
gap."

## Detection algorithm

Missing-import's `collect_candidates` deliberately narrows to *bare* calls and *bare* type
identifiers, to stay high-confidence about what counts as a genuine unresolved reference.
Unused-import needs the opposite bias: it must catch **every** way an import's name could be
used (bare call, bare type, member/extension-function call name, annotation, generic argument,
supertype list, …) — missing one shape means suggesting the deletion of a still-needed import,
which is worse than a missed detection.

Rather than enumerating shapes (real risk of missing one), the detector collects the **flat set
of every `simple_identifier`/`type_identifier` node's text anywhere in the file**, skipping only
the `import_header`/`package_header` subtrees, then checks each import's `local_name` against
that set. Verified this handles annotations for free: `@Foo` still has a `type_identifier` node
for `Foo` somewhere in the tree (`annotation → constructor_invocation → user_type →
type_identifier`, or the shorter `annotation → user_type → type_identifier` form), regardless of
nesting depth — no shape-specific special-casing needed, unlike missing-import's receiver/type-
param scoping. A local variable that happens to shadow an import's name makes the import look
"used" when it technically isn't — a false *negative*, the safe direction of error for a
delete-this suggestion.

**Scope decision**: star imports (`import com.foo.*`, `ImportEntry.is_star == true`) are never
flagged — there is no single name to check usage of, and neither ktlint nor IntelliJ attempt
this either. Only single-symbol imports are in scope, including aliased ones
(`ImportEntry.local_name` already reports the alias, not the original name, so no special
handling is needed there).

**Three fixes, all found (not guessed at) by running the benchmark and then, on the user's
request, actually deleting every flagged import across a real project and compiling it**, folded
in before merge rather than shipped as disclosed-but-unfixed gaps:

1. **Operator-convention imports** (dominant source — 32 of 62 flags). Kotlin property-delegate
   syntax (`val x by lazy { }`) and Gradle's Kotlin-DSL analogues (`=`/`()`/`[]` sugar in
   convention plugins, e.g. `android.namespace = "..."`) desugar to a call
   (`getValue`/`setValue`/`assign`/`invoke`/`get`/…) the compiler synthesizes from the special
   syntax — the name never appears as literal identifier text anywhere in the file. Verified via
   parse tree: `val x by lazy { }` produces a `property_delegate` node whose only child text is
   the `by` keyword itself; there is no node bearing `getValue` anywhere. No amount of widening
   the identifier-collection walk can catch this — there is nothing to widen to. Fixed with a
   static allowlist (`OPERATOR_CONVENTION_NAMES`) of Kotlin's ~30 operator-convention function
   names (`getValue`, `setValue`, `provideDelegate`, `invoke`, `get`, `set`, `assign`, `plus`,
   …, `component1`..`component5`) — an import for one of these is never flagged, full stop,
   regardless of whether this specific file's own syntax happens to use the corresponding sugar.
2. **KDoc `[Reference]` links** (remaining ~8 flags, all of the residual noise). Confirmed via
   parse tree: `multiline_comment` is one opaque leaf — tree-sitter-kotlin does not parse KDoc
   bodies into structured sub-nodes at all, so there is no CST node for `[Foo]` to widen the walk
   to. Fixed with a light text scan over comment-leaf content for `[Identifier`-shaped tokens,
   adding every match to the used-names set. This is **not** "recovering structure the CST
   already has" (there is none to recover for comment prose) — it is the same "genuine
   heuristic, no precise CST answer exists" carve-out the parent design already grants stdlib
   scope-function receiver inference. **CST-shape gotcha found while implementing this**: a
   trailing comment immediately after an import is attached as the LAST CHILD of that import's
   own `import_header` node (not a leading child of the next declaration) — an early return that
   skips the whole `import_header` subtree (needed so an import's own path segments don't count
   as "using" themselves) silently ate this comment too on the first attempt. Fixed by threading
   an `in_declaration_header` flag through the walk instead of returning early, so comment
   scanning still happens inside an import header's subtree even though identifier-text
   collection is suppressed there.
3. **Bare `$identifier` string-template interpolation** — found the hard way, past what the
   benchmark alone surfaced: after both exemptions above brought nowInAndroid to 0 flags, the
   user asked to actually delete every flagged import across Moneta (392 flags, 253 files) and
   compile the result. This produced exactly one real build break:
   `Regex("^$REGEX_PHONE_WITH_OPTIONAL_PREFIX$")` in
   `core/commonui/.../SimpleCzechPhoneNumberLengthValidator.kt` — `Unresolved reference
   'REGEX_PHONE_WITH_OPTIONAL_PREFIX'`. Unlike the two exemptions above, this **is** a real
   CST-shape gap, not a "nothing to widen to" case: dumping the parse tree for
   `Regex("^$FOO$")` shows bare `$identifier` interpolation produces its own dedicated
   `interpolated_identifier` leaf node — distinct from `simple_identifier`/`type_identifier` —
   that the walk's kind filter simply didn't include. Braced `${identifier}`/`${expr.member}`
   interpolation was already handled correctly (verified separately): it wraps a real
   `navigation_expression`/`call_expression` whose own leaves are ordinary `simple_identifier`
   nodes the walk already collects. Fixed by adding `KIND_INTERPOLATED_IDENT` to the kind match.

Every other case not covered by the three fixes above is disclosed but not fixed: any other
implicit/reflection-based use (e.g. `kotlinx.serialization`/data-binding fields, DI framework
magic) that doesn't manifest as a literal identifier, a KDoc reference, string-template
interpolation, or a known operator convention will still be flagged. Not attempted, since the
delete-and-compile validation (below) found no further evidence of this category.

## Diagnostic and code action

- `Diagnostic { severity: Some(DiagnosticSeverity::HINT), tags: Some(vec![DiagnosticTag::UNNECESSARY]), source: Some("kmp-lsp"), message: format!("Unused import '{}'", entry.full_path), .. }` — `HINT` + `UNNECESSARY` is the standard LSP convention for "unused" (renders as grayed-out/faded rather than a squiggle in most editors), distinct from missing-import's `WARNING`.
- Quick-fix "Remove unused import": a single `TextEdit` deleting the exact import line (including its trailing newline), range computed directly from the import statement's own line — no position-search logic needed (unlike missing-import's insertion-point computation).
- Wired into **both** `document_handler.rs`'s open/republish diagnostic block **and**
  `file_change_handler.rs`'s debounced `didChange` block from the start — the missing-import
  diagnostic was wired into only the former initially and needed a follow-up fix once live
  testing surfaced that ongoing edits never re-triggered it. Not repeating that here.

## CLI precision benchmark

New `unused-imports` subcommand (`cli/unused_import_poc.rs`), same shape as
`cli/missing_import_poc.rs`: index the workspace, run the shared detection function
(`collect_unused_import_flags`, shared with the live diagnostic, same pattern as
`collect_missing_import_flags`) over every source file, print per-file flags plus an aggregate
summary. Run against nowInAndroid and Moneta before merging — nowInAndroid's count is treated as
a pure false-positive signal; Moneta's count is **not** — each flag needs corroboration before
being attributed to "real unused import" vs. "detector gap." Two levels of corroboration were
used, in increasing order of strength: spot-checking individual flags by hand (see "Benchmark
results"), then — because the detector's whole value proposition is precision at scale — actually
deleting every flagged import across the full Moneta checkout and running its real Kotlin
compiler (see "Delete-and-compile validation"), the strongest ground truth available for a
diagnostic whose entire job is knowing what's genuinely unused.

## Testing

- Unit tests mirroring `missing_import_diagnostics_tests.rs`'s structure: a genuinely-unused
  import is flagged; a bare-call use, a bare-type use, a member/extension-call use, and an
  annotation use of the same name all suppress the flag; a star import is never flagged; an
  aliased import checks the alias, not the original name.
- `tests/lsp_smoke.rs`: a `smoke_unused_import_diagnostic_on_edit` test mirroring
  `smoke_missing_import_diagnostic_on_edit` — spawns the real binary, opens a file with a used
  import, removes the only use via a live `textDocument/didChange`, and asserts the diagnostic
  appears. This is the regression test for the exact wiring gap missing-import already hit once.
- Code-action test: requesting `textDocument/codeAction` at a flagged diagnostic's range returns
  a `QUICKFIX` deleting exactly that import line.

## Benchmark results

- **nowInAndroid, before the two exemptions**: 62 flags across 46 files. 32 were operator-
  convention imports (`androidx.compose.runtime.getValue`/`setValue` — Compose's `by remember`/
  `by lazy` idiom — plus Gradle Kotlin DSL's `assign`/`invoke`/`get` in `build-logic/convention`).
  The remaining ~8 were all KDoc `[Reference]`-only uses (`[NewsResource]`, `[LazyListScope]`,
  `[AndroidBasePlugin]`, `[IconButton]`, `[ImageVector]`, `[NetworkRequest]`, `[AsyncImage]`,
  `[TestRule]` — each spot-checked directly against the source file and confirmed as exactly
  that shape, no other use anywhere).
- **nowInAndroid, after both exemptions**: **0 flags** across all 338 files — matches the design's
  original expectation for this project exactly.
- **Moneta, after both exemptions (before the interpolation fix)**: 403 flags across 264 files
  (of 12,856 scanned). Per the user's explicit warning going in, this is **not** treated as a
  pure false-positive signal. Spot-checked three, chosen for variety (highest-frequency import,
  a same-package-cluster pattern, an Android-resource-class import): `okhttp3.MultipartBody` in
  a Retrofit interface (`ActivationApi.kt`) — genuinely never referenced beyond
  `@Multipart`/`@Part` annotations that don't need the type itself; `cz.moneta.smartbanka.mobile.R`
  in `TaxResidenceItemView.kt` — genuinely never referenced; and a cluster of four imports
  (`ResultState`/`loadResultState`/`safeFlowResultState`/`ISimpleLoadDataFlowInteractor`) in
  `MaintenanceInteractor.kt`, all flagged together — read the whole file and confirmed it's a
  real refactor leftover: an old error-handling pattern was replaced but its imports were never
  cleaned up. All three spot-checks are genuine dead imports, not detector gaps — consistent
  with the user's prediction that Moneta carries real unused-import debt.

### Delete-and-compile validation (beyond spot-checking)

Spot-checking a handful of flags is necessarily a sample. The user asked for the strongest
possible validation instead: **delete every flagged import in Moneta and try to build it.** On a
clean checkout (`git status` clean beforehand, working-tree-only edit, no commit — trivially
reversible via `git checkout --`), all 403 flagged import lines were deleted by an exact
file:line match (each line verified to actually start with `import` and contain the expected FQN
before deletion — zero mismatches), then `./gradlew compileDebugKotlin compileKotlin --continue`
was run across the whole multi-module project.

The deletion step is `scripts/delete-flagged-imports.py` (checked into this repo): given a
project root and the `unused-imports` POC's stdout, it groups flags by file, deletes bottom-to-top
so line numbers don't shift, and verifies each targeted line actually starts with `import` and
contains the expected FQN before touching it.

```
kmp-lsp unused-imports --root /path/to/project > flags.txt
scripts/delete-flagged-imports.py /path/to/project flags.txt
```

**First run**: one real build failure — `core/commonui/.../SimpleCzechPhoneNumberLengthValidator.kt`,
`Unresolved reference 'REGEX_PHONE_WITH_OPTIONAL_PREFIX'` (the bare-`$identifier`-interpolation
gap described above). Everything else compiled clean. The broken file was restored
(`git checkout --` on that one file) and the interpolation fix (`KIND_INTERPOLATED_IDENT`) was
added.

**Second run, from a clean checkout again, after the fix**: the benchmark's own count first
dropped to 392 flags (11 fewer — the interpolation fix correctly stopped flagging those real
uses). All 392 were deleted the same way and the build was re-run — one more real build failure
surfaced, `KspAAWorkerAction`/`@AssistedFactory` errors in `feature/loan_repayment/.../retention/`
— `Expected exactly one factory method for RetentionViewModel.Factory but found: []`, and two
sibling files with the same shape. Root cause:
`fun interface Factory : AssistedViewModelFactory<RepaymentInitialData, RetentionViewModel>` — a
bodyless generic `fun interface` supertype, not the last member of its enclosing class.
tree-sitter-kotlin has no grammar rule for this shape: dumping the parse tree (`kmp-lsp tree`)
showed `AssistedViewModelFactory` does not appear *anywhere* in the CST — not even as an
uncollected node kind (the interpolation case's problem) — error recovery silently drops it. The
one saving grace: the surrounding `ERROR` node's own byte range still covers the dropped text, it
just has no child token representing it. Fixed by desugaring the shape directly
([`error_node_desugared_supertype_name`](../../../src/features/unused_import_diagnostics.rs)):
an `ERROR` node ending in a bare `:` means the bytes from right after it to the node's own end are
the malformed supertype clause, so its leading identifier run is the supertype name — a targeted
extraction of exactly one identifier per match, not a blanket scan of the `ERROR` node's text for
arbitrary identifier-shaped noise (rejected as an initial draft in favor of this more precise
approach). The checkout was restored, the fix added with a regression test, and the benchmark
re-run: 387 flags (3 fewer, matching the 3 `AssistedViewModelFactory` occurrences exactly), zero
remaining occurrences of that FQN in the output.

**Third run, from a clean checkout again, after this fix**: all 387 flagged imports deleted
(zero mismatches), `./gradlew compileDebugKotlin` run again.

**`BUILD SUCCESSFUL in 31s`, 0 compiler errors** (mostly Gradle build-cache hits from the prior
run's unaffected modules, with the previously-failing module and its dependents re-executed and
passing). Every one of the 387 flagged imports across 248 files was genuinely dead,
workspace-wide, confirmed by the strongest test available (a real compile, not a sample). This
was still not the end of it (see below) — but at the time, it was the actual precision result
this feature's benchmark property was built to produce: not a spot-checked sample, but a
whole-project ground truth.

### Fourth run: a second, more severe parser-corruption class

This time the deletions were carried further: a different agent picked up the delete-and-compile
Moneta branch, ran `./gradlew compileDebugKotlin` and `spotlessApply`, and reported its findings
in `lsp_tasks/2026-07-29-broken-imports-from-unused-import-removal.md`. 5 files had a genuinely
still-used import removed. That report's own root-cause guess (a naive `ImportedName(`-followed-by-
`(` regex) didn't match this tool's actual implementation (a flat CST identifier-text walk with no
regard for call shape) — spot-checking the guess against the actual identifier-collection code
would have caught the mismatch, a reminder to verify a hypothesis against the real implementation
before trusting it, not just against how the *symptom* looks. Reproducing directly (`kmp-lsp tree`)
found the real cause: `expr(...).prop = value` — an assignment to a property accessed through a
call result, e.g. `view.findViewById(id).text = "hello"` — breaks tree-sitter-kotlin's assignment
grammar. Depending on what follows, this manifests two ways, both confirmed via minimal
reproductions:

1. The very next statement can collapse entirely into one opaque `ERROR` leaf with **no
   children at all** (e.g. `if (mortgage is WustenrotMortgage)` became a single `ERROR` node whose
   own text was `"is WustenrotMortgage"`, with the whole type name unreachable by walking
   children). Unlike the `fun interface` case, this shape has no known single identifier to
   desugar precisely, so `error_node_desugared_supertype_name` falls back to a general
   identifier-token scan (`collect_identifier_like_tokens`) over the `ERROR` node's own text when
   the precise desugar doesn't match.
2. Worse, tree-sitter can lose track of where a *later* string literal ends, so a subsequent,
   structurally normal-looking `string_content` node ends up holding real code (`WustenrotProductUtils.formatUpdatedDate(...)`) as its raw text — potentially swallowing an
   arbitrary number of following lines, not just one declaration. This node isn't itself an error
   (its own `kind()` is the ordinary `string_content`), so it's found by threading a
   `parent_has_error` flag through the walk (its *parent* contains the triggering `ERROR`
   children as siblings) and running the same general identifier-token scan whenever that flag is
   set.

Re-running the benchmark after this fix: 387 → 381 flags (6 fewer, exactly the 6 imports across
the 5 reported files — `WustenrotMortgage` and `WustenrotProductUtils` both in one file, one each
in the other four). All 5 reported files independently re-verified at 0 flags. Deleting all 381
from a clean checkout and running `./gradlew compileDebugKotlin` again: **`BUILD SUCCESSFUL in
2m 19s`**, 0 compiler errors.

## Risks

- Any use that's neither a literal identifier, a KDoc reference, string-template interpolation,
  a known operator-convention name, nor one of the disclosed CST-error-recovery shapes
  (reflection-based frameworks, generated-code magic, or a *different* tree-sitter-kotlin parse
  gap than the ones found and fixed here) is still a possible false positive — disclosed, not
  fixed, since the delete-and-compile validation found no further evidence of this category.
- The `expr(...).prop = value` corruption class is a genuine tree-sitter-kotlin grammar gap, not
  specific to this diagnostic — it can in principle affect any feature that reads the CST around
  such an assignment (this diagnostic's fallback only recovers identifiers for its own purposes,
  it doesn't fix the underlying parse).
