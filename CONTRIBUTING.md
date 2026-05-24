# Contributing to kotlin-lsp

## Setup

```sh
git clone git@github.com:qdsfdhvh/kotlin-lsp.git
cd kotlin-lsp
cargo build
```

## Pre-commit checks

Every commit must pass:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --no-fail-fast
```

## Coding standards

See [AGENTS.md](AGENTS.md) for the full list. Key rules:

1. Zero warnings — fix clippy/fmt, never `#[allow]` without a comment
2. No hardcoded node kind strings — use `KIND_*` constants from `src/queries.rs`
3. `expect("reason")` over `unwrap()`
4. Tests in `*_tests.rs` files, not inline `mod tests {}`
5. `#[serde(default)]` on new `SymbolEntry` fields, bump `CACHE_VERSION`

## PR workflow

1. Branch from `main`
2. Make changes + tests
3. Run pre-commit checks locally
4. Push and create PR
5. CI runs on ubuntu, macos, windows
6. Squash-merge on green

## Source layout

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry point, CLI dispatch |
| `src/backend/` | LSP request handlers |
| `src/indexer/` | File discovery, tree-sitter parsing, disk cache |
| `src/parser.rs` | Tree-sitter queries, `SymbolEntry` extraction |
| `src/resolver/` | Cross-file resolution, imports, type inference |
| `src/cli/` | Standalone CLI subcommands |
| `src/types.rs` | `SymbolEntry`, `FileData`, shared types |

## Tests

- Unit tests: `src/*_tests.rs` (alongside source)
- Integration tests: `tests/*.rs`
- Run specific test: `cargo test --test cli_commands`
- Run single test: `cargo test parser::tests::deprecated_annotation_on_class`

## Release

```sh
# Bump version in Cargo.toml and CHANGELOG.md
git tag vX.Y.Z
git push --tags
# GitHub Actions auto-builds release artifacts
```
