# kotlin-lsp — Agent Instructions

## Rust Skills

This project uses [actionbook/rust-skills](https://github.com/actionbook/rust-skills) via CoWork. Install with:

```sh
cargo install cowork
cowork config install
```

See `.cowork/Skills.toml` for config.

This project also publishes its own agent skill at `skills/kotlin-lsp/SKILL.md`
for use in downstream Kotlin projects. See [`skills/README.md`](skills/README.md)
for how to install, use, and maintain skills.

## Local Planning Files

For multi-step work, keep local planning context in three root-level files:

| File | Purpose | Update when |
|------|---------|-------------|
| `task_plan.md` | Current roadmap, priorities, active phases, and scope decisions | The plan changes or a phase status changes |
| `findings.md` | Research findings and rationale that should survive context loss | You discover a fact that affects direction |
| `progress.md` | Session log: what changed, what tests ran, errors encountered | After meaningful actions or verification |

Why three files:

- `task_plan.md` says where the project is going, but not why every decision was made.
- `findings.md` preserves evidence and tradeoffs so future agents do not re-research the same question.
- `progress.md` records execution details, test results, and failed attempts so work can resume after context loss.

Rules:

1. Read `task_plan.md`, `findings.md`, and `progress.md` before changing roadmap or scope.
2. Update `findings.md` after research-heavy or architectural decisions.
3. Update `progress.md` after implementation, verification, or notable errors.
4. Keep these files local by default. They are intentionally gitignored (`TASK_PLAN.md`, `task_plan.md`, `findings.md`, `progress.md`) and should not be committed unless the user explicitly asks to publish planning artifacts.
5. If a plan decision should become public project policy, move the durable part into tracked docs such as `AGENTS.md`, `README.md`, or `docs/`.

## Quick Start

```sh
cargo build --release
cargo test
cargo clippy -- -D warnings
```

## Non-Negotiable Rules

1. **Zero warnings** — fix clippy/fmt, never `#[allow]` without a comment
2. **No hardcoded node kind strings** — use `KIND_*` constants from `src/queries.rs`
3. **Prefer generics over `Box<dyn Trait>`** — static dispatch, zero cost
4. **No bare `unwrap()`** — use `expect("reason")`
5. **Tests in `*_tests.rs` files** — not inline `mod tests {}`
6. **`#[serde(default)]` on new `SymbolEntry` fields** — bump `CACHE_VERSION`
7. **Always PR, never push to main** — `git push origin feat/<name>`, create PR, squash-merge on green CI
8. **Run locally before push** — `cargo fmt --all -- --check && cargo test && cargo clippy -- -D warnings`

   **With fmt proxy issue:** `find src tests -name '*.rs' | xargs rustfmt --edition 2021 --check`

9. **False-positive syntax error fix — test-first** — When fixing `check` false positives:
   - First write a `#[test] fn fp_*` regression test that parses the valid Kotlin and asserts `data.syntax_errors.is_empty()`
   - Verify the test fails before the fix (reproduces the issue)
   - Then add suppression logic in `collect_syntax_errors()` (in `src/parser.rs`)
   - Verify the test passes after the fix
   - Run `cargo test --bin kotlin-lsp 'parser::tests::'` to confirm no regressions
   - Group related tests under `// ── false positive syntax error regression tests ───────────`

10. **Always create a PR — never push to main directly** — Every change, no matter how small:
    - Cut a branch: `git checkout -b feat/xxx` or `fix/xxx`
    - Push: `git push origin feat/xxx`
    - Create PR: `gh pr create --base main --head feat/xxx --title "..." --body "..."`
    - Wait for CI green, then merge: `gh pr merge --squash`
    - NEVER use `git push origin main` or `git push origin master`
    - Exception: only for CHANGELOG.md / README.md / AGENTS.md doc fixes that don't touch code

## CLI Reference

| Need | Command |
|------|---------|
| Find definition | `kotlin-lsp find <NAME>` |
| Find references | `kotlin-lsp refs <NAME>` |
| Get signature | `kotlin-lsp hover <FILE> <LINE> <COL>` |
| Completions | `kotlin-lsp complete <FILE> <LINE> [COL]` |
| One-stop context | `kotlin-lsp context <FILE> <LINE> <COL>` |
| Syntax errors | `kotlin-lsp check <FILE>...` |
| Code actions | `kotlin-lsp code-action <FILE> <LINE> <COL>` |
| Call hierarchy | `kotlin-lsp call-hierarchy <FILE> <LINE> <COL>` |
| Type hierarchy | `kotlin-lsp type-hierarchy <NAME>` |
| Organize imports | `kotlin-lsp organize-imports <FILE>...` |
| Batch imports | `kotlin-lsp batch-imports <FILE>` |
| Index JAR sources | `kotlin-lsp index-jars [ROOT]` |
| Index workspace | `kotlin-lsp index [--root <DIR>]` |
| Cache stats | `kotlin-lsp cache stats` |
| Benchmark | `kotlin-lsp benchmark` |
| Filter by kind | `--kind class,fun,interface` |
| JSON output | `--json` |

## Merge Rules

**Never merge until CI is green on all 3 platforms.** Wait for `gh pr checks` to show all pass before merging.

## How to Release

When asked to "release" or "publish":

1. Bump version in `Cargo.toml` (line 6)
2. Add section to top of `CHANGELOG.md`
3. Commit, create PR, merge on green CI
4. `git tag vX.Y.Z && git push origin vX.Y.Z`
5. GitHub Actions builds release artifacts

## CLI Reference
