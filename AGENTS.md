# AGENTS.md — kmp-lsp Project Instructions

kmp-lsp is a Kotlin Language Server Protocol implementation in Rust. The binary is `kmp-lsp`.

**Deep reference material** (source layout, architecture patterns, coding guidelines with examples, test conventions, Serena MCP workflow — ~800 lines) lives in `docs/agent-reference.md`. Load that on-demand when you need it. This file stays lean: behavioral rules only.

## Workflow
- **Never commit directly to `main`.** Always create a feature branch, push, and open a PR.
- **PR merge workflow:** commit → push → resolve ALL review threads via `gh api graphql resolveReviewThread` → re-request review → `gh pr merge --squash --delete-branch`.
- **Stacked PRs:** merge base first, rebase dependent, then `cargo build && cargo test` on main after.
- **After merging,** install the binary: `cargo install --path . --force`.
- Run tests after every change: `cargo test`. All tests must pass.
- Run clippy after every change: `cargo clippy -- -D warnings`. Must be clean.

## Code Quality
- **Rust types model behaviour** — code should be obvious in retrospect.
- **No abbreviated names.** Never use single-letter or short variable names like `s`, `c`, `ty`, `rt`, `sym`, `loc`, `p`, `diags`. Use full words: `string`, `char`, `type`, `receiver`, `symbol`, `location`, `package`, `diagnostics`.
- **Explicit over clever.** Split compound boolean expressions into named variables. Avoid chained combinators when a simple conditional is clearer.
- **Every fix must include a test.** If a bug slips through, the test should have caught it.
- **Write tests that prove correctness**, not just verify happy-path. Include competing/misleading definitions to catch regressions.
- **`and` in a function name means it's doing two things.** Split into two functions.
- **Section comments inside a function body signal a split.** Each phase becomes a named helper.

## Rust-Specific
- Read paths must be pure `&self` (no `&mut self`) — respect `GlobalState`/`Snapshot` split.
- No unsound lazy caching without snapshot isolation.
- Prefer `patch`/`write_file` over unreliable symbol-rewriting tools for large edits.
- Use tree-sitter cursor API (not index arithmetic) for node traversal. Use `NodeExt` helpers.
- Use `cargo fmt` before committing; pre-commit hooks enforce this.

## Key Files
- `src/indexer/infer/sig.rs` — signature inference and call-site resolution
- `src/resolver/resolve.rs` — the main resolution pipeline (go-to-definition, etc.)
- `src/features/call_arg_diagnostics.rs` — parameter count diagnostics
- `src/indexer/resolution.rs` — `IndexRead` trait and index read path
- `src/types.rs` — `SymbolEntry`, `ExtensionEntry`, core data types
- `docs/agent-reference.md` — full architecture, coding guidelines, test conventions, patterns

## Communication
- **Short, direct responses.** No fluff.
- **Understand root causes**, not just surface fixes.
- When rubber-ducking design, listen and respond with concrete trade-offs.
