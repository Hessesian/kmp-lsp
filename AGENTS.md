# kotlin-lsp — Agent Instructions

## Rust Skills

This project uses [actionbook/rust-skills](https://github.com/actionbook/rust-skills) via CoWork. Install with:

```sh
cargo install cowork
cowork config install
```

See `.cowork/Skills.toml` for config.

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

## CLI Reference

| Need | Command |
|------|---------|
| Find definition | `kotlin-lsp find <NAME>` |
| Find references | `kotlin-lsp refs <NAME>` |
| Get signature | `kotlin-lsp hover <FILE> <LINE> <COL>` |
| Completions | `kotlin-lsp complete <FILE> <LINE> [COL]` |
| One-stop context | `kotlin-lsp context <FILE> <LINE> <COL>` |
| Syntax errors | `kotlin-lsp check <FILE>...` |
| Call hierarchy | `kotlin-lsp call-hierarchy <FILE> <LINE> <COL>` |
| Type hierarchy | `kotlin-lsp type-hierarchy <NAME>` |
| Organize imports | `kotlin-lsp organize-imports <FILE>...` |
