# kotlin-lsp Architecture

## Overview

```
                 LSP Client (VS Code / Zed / Neovim)
                          |
                    tower-lsp (JSON-RPC)
                          |
                    ┌─────────────────┐
                    │   src/backend/   │
                    │  handlers + mod  │
                    └────────┬────────┘
                             |
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
        src/indexer/   src/resolver/   src/parser.rs
        scan, apply,   find, complete, tree-sitter
        cache, infer   infer, mod      queries
              │              │              │
              └──────────────┼──────────────┘
                             ▼
                       src/types.rs
                       SymbolEntry, FileData
```

## Key components

### src/backend/ — LSP protocol layer
- `mod.rs` — `LanguageServer` trait impl, `Backend` struct, capabilities
- `handlers.rs` — hover, completion, definition, references, folding, inlay hints
- `actions.rs` — code action quick-fixes
- `format.rs` — hover Markdown formatting

### src/indexer/ — Indexing & parsing
- Tree-sitter parsing for Kotlin/Java/Swift
- In-memory `DashMap`-based index
- Disk cache via bincode
- File discovery via fd/walkdir

### src/resolver/ — Symbol resolution
- Multi-tier resolution: local → import → same package → star import → rg fallback
- Type inference for lambda params, `it`/`this`
- Completion scoring + auto-import
- Supertype hierarchy walking

### src/cli/ — Standalone CLI
- `find`, `refs`, `hover`, `complete` — one-shot queries
- `inject`, `list-types`, `context` — AI agent tools
- `check`, `organize-imports` — code quality
- `call-hierarchy`, `type-hierarchy` — navigation

### src/parser.rs — Tree-sitter integration
- Query execution for Kotlin/Java/Swift grammars
- Symbol extraction: classes, functions, properties, imports
- Deprecated annotation detection

## Data flow

1. **Startup**: `index_workspace()` → discover files → parse → build index
2. **DidOpen**: `store_live_document_state()` → `index_content()` → publish diagnostics
3. **Completion**: `completions()` → line-scan → index lookup → score → return items
4. **Hover**: `hover_impl()` → resolve symbol → enrich → format Markdown
5. **CLI**: build index → execute command → output text/JSON
