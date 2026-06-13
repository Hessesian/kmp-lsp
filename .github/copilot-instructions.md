# kmp-lsp — Agent & Copilot Coding Instructions

**Deep reference material** (source layout, architecture patterns, coding guidelines with examples, test conventions, Serena MCP workflow) lives in `docs/agent-reference.md`. Load that on-demand. This file is behavioral rules only.

## Workflow
- **Never commit directly to `main`.** Always create a feature branch, push, and open a PR.
- **PR merge workflow:** commit → push → resolve ALL review threads via `gh api graphql resolveReviewThread` → re-request review → `gh pr merge --squash --delete-branch`.
- **Stacked PRs:** merge base first, rebase dependent, then `cargo build && cargo test` on main after.
- **After merging,** install the binary: `cargo install --path . --force`.

## Code Quality
- **Rust types model behaviour** — code should be obvious in retrospect.
- **No abbreviated names.** Never use single-letter or short variable names like `s`, `c`, `ty`, `rt`, `sym`, `loc`, `p`, `diags`. Use full words: `string`, `char`, `type`, `receiver`, `symbol`, `location`, `package`, `diagnostics`.
- **Explicit over clever.** Split compound boolean expressions into named variables. Avoid chained combinators when a simple conditional is clearer.
- **Every fix must include a test.** If a bug slips through, the test should have caught it.
- **Write tests that prove correctness**, not just verify happy-path. Include competing/misleading definitions to catch regressions.

## Rust-Specific
- Read paths must be pure `&self` (no `&mut self`) — respect `GlobalState`/`Snapshot` split.
- No unsound lazy caching without snapshot isolation.
- Avoid helper functions that obscure intent — the code should read like a spec.

## Communication
- Short, direct responses. No fluff.
- Understand root causes, not just surface fixes.
- When rubber-ducking design, listen and respond with concrete trade-offs.
