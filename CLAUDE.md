# kotlin-lsp — Claude Code Instructions

@AGENTS.md

## CRITICAL: LSP First, grep last

When investigating Kotlin/Java/Swift code, prefer the project's own CLI over raw grep:

1. `kotlin-lsp find <NAME>` — definition
2. `kotlin-lsp hover <FILE> <LINE> <COL>` — signature
3. `kotlin-lsp refs <NAME>` — references
4. `kotlin-lsp context <FILE> <LINE> <COL>` — one-stop context
5. `rg <pattern>` — last resort

## Source Layout

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry point, CLI dispatch |
| `src/backend/` | LSP handlers |
| `src/indexer/` | File discovery, parse, cache |
| `src/parser.rs` | Tree-sitter queries |
| `src/resolver/` | Cross-file resolution |
| `src/cli/` | Standalone CLI commands |
| `src/types.rs` | SymbolEntry, shared types |
