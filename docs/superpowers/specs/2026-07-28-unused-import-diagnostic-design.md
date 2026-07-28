# Unused-Import Diagnostic — Design

Status: **approved design** (brainstormed with the user 2026-07-28, immediately following the
missing-import diagnostic's live-wiring work). Sibling feature to
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

**Disclosed false-positive risk**: an import referenced only inside a KDoc `[Reference]` link is
not caught — tree-sitter-kotlin does not tokenize comment content into identifier nodes. It will
be flagged as unused even though IntelliJ special-cases this. Not fixed preemptively; the
benchmark run is what determines whether this is worth addressing.

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
a pure false-positive signal; Moneta's count is **not** — each flag gets spot-checked against
the actual file content before being attributed to "real unused import" vs. "detector gap."

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

## Risks

- KDoc-only references (disclosed above) — accepted, not fixed preemptively.
- Moneta's benchmark run will show non-zero flags by the user's own account; the review step
  (spot-checking a sample against real file content) is required before drawing any precision
  conclusion from that number, unlike nowInAndroid's number which can be trusted directly.
