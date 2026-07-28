# `diagnose --only <names>` — Design

Status: **implemented** (small, low-risk CLI addition; user requested "another flag which is a
specific diagnostic option and help menu" while reviewing `diagnose`'s current UX during the
duplicate-import brainstorm).

## Context (why)

`kmp-lsp diagnose <file>` always ran every wired-in diagnostic (`call_arg_diagnostics`,
`nullable_dot_call_diagnostics`, `when_diagnostics`) plus tree-sitter syntax errors, with no way
to isolate one. Reviewing this surfaced a real, disclosable gap: `diagnose` predates
`missing_import_diagnostics` and was never updated to include it — the live LSP
`publishDiagnostics` path is the only place that diagnostic runs today. Folding it in was a
natural, small addition alongside the filtering flag itself.

## Design

**New flag**: `--only <names>` — comma-separated diagnostic names to run. Omitted = run
everything (unchanged default behavior).

**Named diagnostics** (`DIAGNOSTIC_NAMES` in `cli/args.rs`, single source of truth for both
validation and help text): `syntax`, `call-arg`, `nullable`, `when`, `missing-import`.
`unused-import` is intentionally not yet listed — `features::unused_import_diagnostics` hasn't
merged to `main` (PR #239 open as of this writing); adding it is a one-line follow-up once that
lands (add the name to the list, add one `if` branch in `run_diagnose`, same pattern as
`missing-import` below).

**Validation**: an unknown name in `--only` fails immediately with a clear message listing the
valid names (`unknown diagnostic name 'X' — valid names: ...`), rather than silently running
everything or silently running nothing — both of those would be worse than a loud failure for a
flag whose entire purpose is precise control over what runs.

**Performance side effect, not just filtering**: `run_diagnose` now checks up front whether any
diagnostic that *needs* the index (`call-arg`/`nullable`/`when`/`missing-import`) is actually
requested. `--only syntax` skips building the workspace index and scanning Gradle JARs entirely —
the expensive part of this command — falling straight through to the already-index-free
tree-sitter syntax check, matching `check`'s existing no-index fast path. Verified directly: a
run with `--only syntax` against a real project never prints the `"Indexing..."`/`"Indexed:"`
lines the indexed path always emits.

**Help text**: the single flat `OPTIONS`/`EXAMPLES` block in `print_help()` (existing convention
— every other subcommand-specific flag is documented inline there, e.g. `--exclude-imports
(refs)`) gains a `--only <names>` line listing the valid names (interpolated from
`DIAGNOSTIC_NAMES`, so the list can't drift out of sync with what's actually accepted) and a new
example line.

## Testing

`tests/cli_diagnose.rs` (existing integration-test file, hermetic `GRADLE_USER_HOME`/
`XDG_CACHE_HOME` pattern already established there):
- `--only syntax` on a file with a real call-arg violation: the call-arg diagnostic does not
  appear, AND the `"Indexed:"` marker never appears (proves the fast no-index path, not just
  that the diagnostic was filtered post-hoc).
- `--only call-arg` on the exact same fixture: the diagnostic still appears — proves the filter
  is a real allow-list, not accidentally suppressing everything.
- An unknown `--only` name: non-zero exit, stderr names the bad value and lists valid names.

## Out of scope

- `unused-import` in `DIAGNOSTIC_NAMES` — blocked on PR #239 merging (disclosed above, not a
  design gap, just sequencing).
- A dedicated `diagnose --help` screen — explicitly decided against; stays in the one global
  help screen like every other subcommand flag.
