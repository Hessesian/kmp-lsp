# Features

## CLI (primary surface)

All commands work standalone — no editor, no daemon, no JVM.

| Command | What it does |
|---------|-------------|
| `find` | Declaration search — qualified name, `--owner`, `--kind`, `--module`, `--source-set`, `--limit` |
| `refs` | All references — same filters, plus `--explain` for provenance |
| `hover` | Signature, visibility, KDoc, deprecated warning, data class properties |
| `complete` | Dot-completion, auto-import, scored ranking, stdlib entries |
| `context` | One-stop: definition + signature + doc + reference count; `--expand` for type chain |
| `check` | Syntax errors, unused/duplicate imports, deprecation warnings, redundant vals |
| `call-hierarchy` | Incoming (rg) + outgoing (CST walk) call chain |
| `type-hierarchy` | Supertype/subtype tree (BFS) |
| `organize-imports` | Sort, dedup, remove unused |
| `inject` | Batch-resolve all type signatures in a file |
| `code-action` | List/apply code actions from the command line |
| `batch-imports` | Scan file for import candidates |
| `new-file` | Generate file from template |
| `cache stats` | Index cache diagnostics |
| `benchmark` | Performance benchmarks |
| `sources` | List auto-discovered source roots; `--explain` for provenance |
| `extract-sources` | Unpack `*-sources.jar` from Gradle cache |
| `index-jars` | Index library symbols from extracted sources |

### Output conventions

- **Text mode (default)** — results grouped by file with structural annotation:
  path on its own line, then `line:col[ kind]` per match, blank line between
  file groups.
- **`--json`** — compact JSON. Use when structured data is needed downstream.
- **`--relative`** — workspace-relative paths, auto-enabled when stdout is
  piped (always true in agent context). Pass `--absolute` to opt out.
- **`--flat`** — legacy grep-style `<path>:<line>:<col>: <name>` output.

### Filters

| Flag | Effect |
|------|--------|
| `--module <frag>` | Keep results whose module path contains `<fragment>` |
| `--source-set <set>` | Keep results in the given source set(s); comma-separated for OR |
| `--owner <name>` | Keep results enclosed by class/interface/object `<name>` |
| `--kind class,fun` | Filter by symbol kind |
| `--limit <n>` | Cap result count after filtering |

---

## LSP (compatibility transport)

When `kotlin-lsp` is launched as a language server, all CLI commands are
available as LSP handlers. Some additional handlers exist for editor
compatibility.

### Document sync

| Capability | Notes |
|------------|-------|
| `textDocument/didOpen` | Indexes file, publishes initial diagnostics |
| `textDocument/didChange` | 300 ms debounce — reindexes and publishes diagnostics |
| `textDocument/didSave` | Re-indexes the saved file (external formatters/codegen) |
| `textDocument/didClose` | Clears diagnostics for the closed file |
| `workspace/didChangeWatchedFiles` | Re-indexes `.kt`/`.java` files changed on disk |

### Navigation and search

| Capability | Notes |
|------------|-------|
| `textDocument/definition` | Index lookup → superclass chain → `rg` fallback |
| `textDocument/typeDefinition` | Resolves `val x: Foo` → `Foo`, `fun foo(): Bar` → `Bar`, `it`/lambda params |
| `textDocument/declaration` | Delegates to `goto_definition` (no separate declaration/definition in Kotlin/Java) |
| `textDocument/implementation` | Transitive subtype lookup via BFS |
| `textDocument/references` | Project-wide `rg --word-regexp` + open-buffer scan |

### Hover and help

| Capability | Notes |
|------------|-------|
| `textDocument/hover` | Declaration kind, visibility, source line, lambda param types, KDoc, deprecated warnings, data class properties, Kotlin stdlib signatures |
| `textDocument/signatureHelp` | Active function signature + highlighted parameter — editor popup use only; prefer `hover` or `context` in CLI |
| `completionItem/resolve` | Lazy KDoc/Javadoc + signature on item selection |

### Completion

| Capability | Notes |
|------------|-------|
| `textDocument/completion` | Dot-completion, bare-word, deprecated tag, label_details (inline params + return type), stdlib entries. Triggers on `.`, `:`, `@`. |
| `completionItem/resolve` | Lazy KDoc/Javadoc + signature on item selection |

### Code actions and refactoring

| Capability | Notes |
|------------|-------|
| `textDocument/codeAction` | Available via CLI `kotlin-lsp code-action` |
| `textDocument/rename` | Project-wide rename via `WorkspaceEdit`; index updated via file watcher |
| `textDocument/formatting` | Delegates to `ktfmt`, `google-java-format`, or `swift-format` on `$PATH` |
| `textDocument/rangeFormatting` | Same external formatters, clipped to requested range |
| `executeCommand` | `kotlin-lsp/reindex`, `kotlin-lsp/clearCache` |

### Diagnostics

| Capability | Notes |
|------------|-------|
| `textDocument/publishDiagnostics` | Syntax errors from tree-sitter (ERROR/MISSING nodes); NOT type checking |
| `$/progress` | Spinner while workspace is indexed |

### Document structure

| Capability | Notes |
|------------|-------|
| `textDocument/documentSymbol` | All symbols in the current file (outline view) |
| `workspace/symbol` | Fuzzy substring search; supports dot-qualified queries for extension functions |
| `textDocument/foldingRange` | Brace regions, import blocks (`Imports`), comment blocks (`Comment`) — editor only |
| `textDocument/selectionRange` | Smart expand-selection via tree-sitter CST ancestor chain — editor only |

### Visual-only (editor compatibility)

| Capability | Notes |
|------------|-------|
| `textDocument/semanticTokens/full` | Two-phase: CST classification + cross-file resolution. Kotlin, Java, Swift |
| `textDocument/semanticTokens/range` | Same as full, clipped to range |
| `textDocument/documentHighlight` | All in-file occurrences; declaration sites marked WRITE, usages READ |
| `textDocument/inlayHint` | Type hints for lambda `it`, named lambda params, `this`, untyped `val`/`var` (configurable) |
| `textDocument/onTypeFormatting` | Auto de-indent on `}` |

### Call and type hierarchy

| Capability | Notes |
|------------|-------|
| `callHierarchy` | `prepareCallHierarchy` + `incomingCalls` (rg) + `outgoingCalls` (CST walk) |
| `textDocument/typeHierarchy` | (planned — blocked on lsp-types upstream feature availability) |

---

## Index

### What gets indexed

| Language | Symbols |
|----------|---------|
| **Kotlin** | `class`, `interface`, `object`, `fun`, `val`, `var`, `typealias`, constructor params, enum entries |
| **Java** | `class`, `interface`, `enum`, `method`, `field`, `enum_constant` |
| **Swift** | `class`, `struct`, `enum`, `protocol`, `func`, `let`, `var`, `typealias`, `extension`, `init`, enum cases |

### Source discovery

- **Gradle / KMP** — `src/*/kotlin/`, `build.gradle.kts` modules, source sets
- **Maven** — standard `src/main/java`, `src/main/kotlin` layouts
- **Android** — `AndroidManifest.xml` or `build.gradle.kts` namespace detection
- **IntelliJ** — `.idea` / `workspace.xml` / `*.iml` module roots
- **Swift** — `Package.swift` targets
- **Generic** — `src/` directories under the workspace root
