# Architecture

## Overview

`kotlin-lsp` is a no-JVM language server for Kotlin, Java, and Swift. It provides
LSP protocol features (goto-definition, hover, completion, diagnostics, etc.)
and a standalone CLI for agent/tooling integration.

## Hexagonal Architecture

```
┌──────────────────────────────────────────────────────────┐
│                      CLI Layer                           │
│  (args.rs → run.rs → batch.rs / templates.rs / etc.)    │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│                     LSP Layer                            │
│  (backend/mod.rs — LanguageServer trait impl)            │
│  ├── handlers.rs — feature implementations              │
│  ├── helpers.rs — diagnostics                            │
│  ├── actions.rs — code actions                           │
│  ├── nav.rs — goto-definition / type-definition         │
│  └── format.rs — hover markdown formatting              │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│                   Index Layer                            │
│  (indexer/)                                              │
│  ├── scope.rs — file/workspace queries                  │
│  ├── lookup.rs — definition/resolution                  │
│  ├── resolution.rs — symbol enrichment                  │
│  ├── cache.rs — on-disk serialization                   │
│  ├── scan.rs — workspace scanning                        │
│  ├── apply.rs — merging results                         │
│  ├── infer/ — type inference (cst_cursor, sig, subst)   │
│  └── live_tree.rs — live document tracking              │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│                   Parser Layer                           │
│  (parser.rs — tree-sitter dispatch)                     │
│  ├── queries.rs — node kind constants                   │
│  ├── str_ext.rs — string utilities                       │
│  └── lines_ext.rs — line-based parsing                  │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│                   Domain Types                           │
│  (types.rs — SymbolEntry, FileData, ImportEntry, etc.)  │
└──────────────────────────────────────────────────────────┘
```

## Key Data Structures

### `Indexer`
Central shared state holding parsed file data, symbol definitions, subtype mappings,
and live document trees. Thread-safe via `Arc<DashMap<K, V>>` and `RwLock`.

```rust
pub(crate) struct Indexer {
    pub files: Arc<DashMap<String, FileData>>,
    pub definitions: Arc<DashMap<String, Vec<Location>>>,
    pub subtypes: Arc<DashMap<String, Vec<Location>>>,
    pub live_lines: Arc<DashMap<String, Vec<String>>>,
    // ...
}
```

### `SymbolEntry`
Per-symbol cached data from tree-sitter parsing:

```rust
pub(crate) struct SymbolEntry {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range,
    pub selection_range: Range,
    pub detail: String,          // Full signature (e.g. "fun foo(x: Int): String")
    pub type_params: Vec<String>,
    pub extension_receiver: String,
    pub deprecated: bool,
}
```

### `FileData`
All data extracted from a single parsed file:

```rust
pub(crate) struct FileData {
    pub uri: String,
    pub lines: Vec<String>,
    pub symbols: Vec<SymbolEntry>,
    pub imports: Vec<ImportEntry>,
    pub syntax_errors: Vec<SyntaxError>,
    pub supers: Vec<(usize, String, Vec<String>)>,
    pub content_hash: u64,
}
```

## Resolution Pipeline

1. **CST parsing** — tree-sitter produces a concrete syntax tree
2. **Symbol extraction** — walk the CST to find declarations
3. **Cross-file resolution** — match symbol names across files via `definitions` map
4. **Type substitution** — resolve generic type parameters for subclass contexts
5. **Rg fallback** — `rg` (ripgrep) for cold-start / unindexed symbols

## Concurrency

- The `Backend` holds the `Indexer` behind `Arc`
- LSP event handlers run on Tokio (async)
- File indexing runs via `tokio::task::spawn_blocking`
- Live document trees updated synchronously (single-threaded LSP dispatch)

## Testing Strategy

- **Unit tests** alongside production code (e.g., `inlay_hints_tests.rs`)
- **Smoke tests** in `tests/lsp_smoke.rs` (end-to-end LSP via stdio)
- **CLI tests** in `tests/` (grammar validation)
- All tests run via `cargo test`
