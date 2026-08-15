# Resolution-Accuracy Benchmark — Design

Status: **proposed** (approved 2026-08-15, methodology independently reviewed by Fable before
write-up). CLI-only for this pass; positioned to grow a live diagnostic later, same dual-purpose
shape as `missing_import_diagnostics`/`unused_import_diagnostics` (a shared detection function
used by both a CLI benchmark and, eventually, a real LSP diagnostic).

## Context (why)

This codebase has no compiler or type-checker backing it — resolution is CST heuristics plus
cross-file/JAR symbol lookup. Two existing CLI benchmarks (`missing-imports`, `unused-imports`)
measure **precision**: run against a project known to compile cleanly, every flag is by
construction a false positive, so the flag count is a direct precision signal (ideal: zero).

There is no equivalent tool for the mirror statistic, **recall**: of every reference in a
workspace, what fraction can the resolver actually find? This matters directly to this session's
work — five separate self-shadow bugs were just fixed (an arity-incompatible same-file
declaration wrongly resurrected in place of the real target), each fixed by making the resolver
return *empty* rather than the wrong answer. That's the right per-call behavior, but it has no
observability: nothing currently measures whether "return empty" is trending down (gaps closing)
or just moving the failure from "wrong answer" to "no answer" without ever being followed up.

## Framing

No compiler ground-truth exists to validate recall against (an equivalent to "run against a
project you know compiles" doesn't exist for "run against a project you know resolves correctly"
— nothing computes that independently). This is explicitly a **trend metric**: run on the same
corpus before/after a resolver change and compare deltas ("member-ref recall went 71%→78% after
this fix"), not chase an absolute score. The tool's output should be framed the same way the
precision POCs frame "ideal is zero" — here, "ideal is monotonically up; watch the Gap bucket's
top names for regressions."

## Scope

Only identifiers `classify_symbol_at` classifies as `SymbolRole::Reference` — `Declaration` and
`ImportSegment` are excluded (declarations resolve by construction; imports are the existing
precision POCs' territory).

Reported in two separate lanes rather than filtering one out:
- **Member refs** (`receiver_type: Some(_)`, e.g. `x.foo()`, `viewModel.state`) — the harder,
  receiver-typed path, where resolver quality actually shows up.
- **Bare refs** (`receiver_type: None`, e.g. local vars, top-level unqualified calls) — expected
  to sit near 100%; reported as a sanity baseline, not filtered out as noise.

## Classification algorithm

Uses `classify_cursor` + `resolve_identity` directly — **not** `find_definition`'s full pipeline.
`find_definition` has an `rg`-grep text-search fallback and several contextual special cases
beyond what `resolve_identity` covers; going through it would let a lucky grep match mask a real
resolver regression, making the benchmark insensitive to exactly the class of bug it exists to
catch. This is a deliberate floor, not the full user-facing success rate — documented in the
tool's own output header.

For each `Reference`, four possible outcomes:

- **Member ref**: run the receiver-typed lookup (`find_definition_qualified(name,
  Some(receiver_type), uri)`), then the same shape filter `resolve_identity` already applies when
  a call shape is present.
  - Filtered result non-empty → **Success/CstResolved**.
  - Filtered result empty → probe the untyped lookup (`find_definition_qualified(name, None,
    uri)`):
    - Probe non-empty → **FilteredCandidate**. Ambiguous by design: could be a correct
      self-shadow-style suppression (the exact scenario last session's fixes address) or a
      genuine miss. Reported as its own bucket, not folded into a plain failure count.
    - Probe empty too → **Gap**. No candidate anywhere by that name — the actionable bucket.
- **Bare ref**: `resolve_identity`'s own result for a bare reference *is* the untyped lookup
  already (no receiver to filter by), so there is no separate probe:
  - Non-empty → **Success/NameScan**.
  - Empty → **Gap** directly.

`CstResolved` and `NameScan` successes are reported as separate tiers, not blended into one
"success" number — `CstResolved` is precise/receiver-verified; `NameScan` (only reachable for bare
refs here, since a member ref's `resolve_identity` arm never returns non-empty under the
`NameScan` label) is "found by name, possibly one of several same-named workspace symbols." Headline
recall is `CstResolved / total-member-refs` for the member lane.

## New infrastructure

One new piece of infrastructure: an identifier walker that finds every
`simple_identifier`/`type_identifier` position in a file's CST. Nothing currently does this
generically — `classify_symbol_at` operates on one cursor position at a time;
`missing_import_diagnostics.rs`'s walk (`collect_candidates`) is a narrower, bespoke pass (skips
import/package subtrees, tracks in-scope type params) not meant for general reuse.

Bounded recursion using the existing convention (`crate::util::MAX_CST_DESCENT_DEPTH` +
`crate::util::report_cst_depth_exceeded!`), matching `collect_candidates`'s own depth guard — the
prior stack-overflow-hardening pass (PRs #257–#259) bounded every hand-rolled recursive CST
descent in this codebase; this walker follows the same pattern from the start rather than needing
a later retrofit.

## Files

- `src/features/unresolved_symbol_diagnostics.rs` (new) — the identifier walker plus
  `collect_resolution_outcomes(indexer, uri, doc) -> Vec<ReferenceOutcome>`, the shared detection
  function. Named and positioned like its `missing_import_diagnostics`/`unused_import_diagnostics`
  siblings specifically so a later live diagnostic can reuse it without restructuring.
- `src/cli/resolution_accuracy_poc.rs` (new) — same skeleton as `missing_import_poc.rs`: build the
  index, warm the JAR/compiled-library index (member-ref recall depends on library-typed receivers
  resolving), walk every workspace `.kt`/`.java` file, aggregate.
- `src/cli/args.rs` — new `resolution-accuracy [root]` subcommand, following the existing
  `missing-imports`/`unused-imports` subcommand pattern (positional optional root, `--json` via the
  existing global flag).

## Output

Mirrors the existing POCs' "top flagged names" style:

- Files scanned, total references (member / bare split).
- Member-ref recall % (`CstResolved / total-member`), bare-ref recall % (`Success /
  total-bare`).
- `FilteredCandidate` count and top-N names by frequency (with one sample location each) —
  the bucket to eyeball for self-shadow-style suppressions.
- `Gap` count and top-N names by frequency (with one sample location each) — the actionable bucket.

## Testing

Unit tests for the classifier (mirroring `missing_import_diagnostics_tests.rs`'s structure) with
synthetic indexer setups:
- A JAR-backed target resolves → `Success/CstResolved`.
- A same-file, shape-mismatched self-declaration (the `triggers.collect { trigger -> }` shape
  from this session's fixes) → `FilteredCandidate`, not `Gap` and not counted as a plain miss.
- A genuinely undeclared name → `Gap`.
- A bare local-variable reference → `Success/NameScan`.

Identifier walker: a depth-bound test matching the existing `*_survives_a_pathologically_deep_*`
convention (`collect_candidates`'s own sibling tests), confirming a pathologically nested input
returns partial results rather than overflowing the stack.

## Risks / deferred

- `FilteredCandidate` is inherently ambiguous — this design deliberately does not try to further
  auto-classify it (e.g. distinguishing "shape-filtered" from "receiver-type mismatch"). Reported
  as one bucket for a human to spot-check; a finer split is a possible follow-up if the bucket
  proves too large to eyeball on a real corpus.
- Live-diagnostic wiring (an actual editor-visible "unresolved reference" squiggle) is explicitly
  out of scope for this pass — CLI benchmark only, per the user's stated sequencing ("cli first,
  then expand into lsp"). The shared function is positioned for that reuse, not implementing it.
- No compiler ground truth: a reference this tool calls a `Gap` may be genuinely unresolvable by
  design (external SDK symbol outside indexed JARs, generated code, DSL magic) — same caveat the
  precision POCs' "systematic FP source" framing already carries in the opposite direction.
