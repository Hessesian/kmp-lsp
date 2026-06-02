# Features

## LSP capabilities

| LSP capability | Notes |
|---|---|
| `textDocument/definition` | Index lookup → superclass hierarchy → `rg` fallback |
| `textDocument/hover` | Declaration kind, visibility, source line, lambda param types, KDoc, deprecated warnings, data class properties, Kotlin stdlib signatures |
| `textDocument/documentSymbol` | All symbols in the current file (outline view) |
| `textDocument/completion` | Dot-completion, bare-word, deprecated tag, label_details (inline params + return type), stdlib entries |
| `completionItem/resolve` | Lazy KDoc/Javadoc + signature on item selection; keeps initial list fast |
| `textDocument/references` | Project-wide `rg --word-regexp` + in-memory scan of open buffers |
| `textDocument/signatureHelp` | Active function signature + highlighted parameter as you type |
| `textDocument/rename` | Renames symbol across all files via `WorkspaceEdit`; index updated via file watcher |
| `textDocument/foldingRange` | Brace regions, import blocks (`Imports` kind), `//` and `/*` comment blocks (`Comment` kind), all fold types include `collapsed_text` |
| `textDocument/onTypeFormatting` | Auto de-indent on `}` — aligns closing brace with matching opening brace indentation |
| `callHierarchy` | prepareCallHierarchy + incomingCalls (rg) + outgoingCalls (CST call-expr walk) |
| `textDocument/selectionRange` | Smart expand-selection via tree-sitter CST ancestor chain |
| `textDocument/inlayHint` | Type hints for lambda `it`, named lambda params, `this`, untyped `val`/`var` |
| `textDocument/semanticTokens/full` | Two-phase: Phase 1 CST classification + Phase 2 cross-file resolution. Kotlin, Java, Swift |
| `textDocument/publishDiagnostics` | Syntax errors from tree-sitter (ERROR/MISSING nodes) — not type checking |
| `textDocument/implementation` | Transitive subtype lookup (interface → all implementing classes, BFS) |
| `textDocument/gotoDeclaration` | Delegates to `goto_definition` (no separate declaration/definition concept in Kotlin/Java) |
| `textDocument/documentHighlight` | Highlights all in-file occurrences; declaration sites marked WRITE, usages READ |
| `workspace/symbol` | Fuzzy substring search; supports dot-qualified queries for extension functions |
| `$/progress` | Spinner while workspace is indexed; non-blocking |
| `textDocument/didSave` | Re-indexes the saved file so external formatters/codegen are picked up |
| `textDocument/formatting` | Delegates to `ktfmt` (Kotlin), `google-java-format` (Java), or `swift-format` (Swift) on `$PATH` |
| `textDocument/rangeFormatting` | Reuses the same external formatters; returns an edit clipped to the requested range |

## Not yet implemented

The following LSP features are commonly supported by full language servers and
are candidates for future work. Rough effort estimates assume tree-sitter
parsing only (no type resolution):

| LSP capability | Effort | Notes |
|---|---|---|
| ~~`textDocument/semanticTokens/full`~~ | ~~High~~ | ✅ **Implemented in 0.11.0.** Two-phase pipeline: Phase 1 (CST classification) + Phase 2 (cross-file index resolution). Kotlin, Java, Swift. |
| `textDocument/typeDefinition` | Medium | Jump to the type of a variable. Requires type inference beyond what tree-sitter provides without the compiler. |

## CLI subcommands

`kotlin-lsp` ships with a standalone CLI in addition to the LSP server. Output is **tuned for AI agents**: text is one record per line (grep-style), `--json` is compact (no whitespace), and `--relative` is auto-enabled when stdout isn't a TTY so the absolute workspace prefix doesn't bloat the token bill on every call.

### Global output flags

| Flag | Behaviour |
|---|---|
| _(none)_ | Plain text, **grouped by file** for find/refs. Auto-relative when piped. |
| `--json` | Compact JSON (no whitespace). Pipe to `jq` if a human needs to read it. |
| `--relative` | Force workspace-relative paths. With `--json`, the `file` field carries the relative path and `relativePath` is omitted (no duplication). |
| `--absolute` | Force absolute paths. Useful for shell scripts that feed paths back to other tools. Overrides the non-TTY auto-relative default. |
| `--flat` | (find/refs) Restore the legacy `<path>:<line>:<col>: <name>` one-line-per-match format. Use this when piping into grep-style tools like `cut -d: -f1`. |

### `find` / `refs`

```
kotlin-lsp find <NAME> [--module <substring>] [--source-set <a,b,c>] [--limit <n>] [...]
kotlin-lsp refs <NAME> [--module <substring>] [--source-set <a,b,c>] [--limit <n>] [...]
```

**Text (default, grouped)** — file path on its own line, followed by `[<module> <sourceSet>]` when known, then one `<line>:<col>[ <kind>]` per match. Blank line between file groups. The query name is omitted (it's whatever you typed on the command line):

```text
features/auth-domain/src/commonMain/kotlin/.../SessionRefresherImpl.kt [features/auth-domain commonMain]
37:14

features/play-export/src/commonMain/kotlin/.../ChatArchiveViewModel.kt [features/play-export commonMain]
16:16
```

Format rationale: in `refs` results, the same file usually carries many matches; repeating its (often 60+ char) workspace path on every line is the biggest token cost. Grouping cuts ~70–80% of bytes on typical refs queries. The `[module sourceSet]` annotation lets agents filter by Gradle module or KMP source-set on the second pass without re-parsing the path string — same semantic info the `--json` output carries as `module` / `sourceSet` fields, but cheaper in bytes. The annotation collapses to empty when neither field is populated (top-level scripts, etc.):

```text
build.gradle.kts
12:5
```

**Text with `--flat`** — legacy grep-style, one full record per line. Use when feeding `cut`/`awk` or other line-oriented tools:

```text
app/src/main/kotlin/com/example/Foo.kt:4:9: greet
app/src/main/kotlin/com/example/Foo.kt:5:19: greet
```

Pipe-friendly: `kotlin-lsp refs greet --flat | cut -d: -f1 | sort -u`.

**JSON (default, with auto-relative)** — `file` holds the relative path; `relativePath` is omitted because it would duplicate `file`:

```json
[
  {"file":"app/src/main/kotlin/com/example/Foo.kt","line":3,"col":1,"name":"Foo","module":"app","sourceSet":"main"}
]
```

**JSON with `--absolute`** — `file` is absolute and the `relativePath` / `module` / `sourceSet` fields are kept verbatim:

```json
[
  {"file":"/Users/me/proj/app/src/main/kotlin/com/example/Foo.kt","line":3,"col":1,"name":"Foo","relativePath":"app/src/main/kotlin/com/example/Foo.kt","module":"app","sourceSet":"main"}
]
```

Optional JSON fields (`kind`, `signature`, `module`, `sourceSet`, `relativePath`) are omitted when empty.

### `hover`

```
kotlin-lsp hover <FILE> <LINE> <COL>
```

**Text** — signature on the first line, optional KDoc/Javadoc after a blank line:

```text
fun greet(other: String): String = "$name says hi to $other"
```

**JSON** — `{"signature": "<text>"}` (the signature value may contain `\n`).

> **Limitation**: hover only resolves at the **declaration site**, not at call sites (tree-sitter cannot do general type inference). To get the signature at a call site, run `find <name>` first to locate the declaration, then hover that position. The agent skill documents this two-call pattern.

### `complete`

```
kotlin-lsp complete <FILE> <LINE> [COL] [--dot] [--eol] [--no-stdlib]
```

Returns completion candidates for the cursor position without starting the LSP daemon — useful for shell integrations, editor plugins, and testing.

| Flag | Description |
|---|---|
| `--dot` | Auto-place cursor immediately after the last `.` on the line |
| `--eol` | Auto-place cursor at the end of the line |
| `--no-stdlib` | Skip `~/.kotlin-lsp/sources` for project-only results (~5× faster) |

**Text** — tab-separated `label\tkind\tdetail\timport`. Empty trailing columns are kept so `cut -f1` is column-stable:

```text
greet	fun	fun greet(other: String): String	
shout	fun	fun shout()	
let	method	inline fun <T, R> T.let(block: (T) -> R): R	
```

**JSON** — `[{"label","kind","detail"?,"import"?}]`. `detail` and `import` are omitted when empty.

The item count is printed to stderr (`(N items)`), not stdout, so it never pollutes piped output.

### `sources`

```
kotlin-lsp sources [--root <dir>]
```

Lists every source root that would be auto-discovered for a project (from `workspace.json` or standard Gradle/Maven build layout). Run this to verify the indexer will find your sources without starting the server.

**Text** — one **existing** path per line, no decoration:

```text
/Users/me/proj/app/src/main/kotlin
/Users/me/.kotlin-lsp/sources
```

Configured-but-missing paths and tips (e.g. "run `extract-sources`") go to **stderr** so stdout stays parseable.

**JSON** — `[{"path","origin","exists"}]`; `origin` is `"workspace.json"` or `"build-layout"`. Includes paths that don't exist on disk so tooling can flag misconfiguration:

```json
[{"path":"/Users/me/proj/app/src/main/kotlin","origin":"build-layout","exists":true}]
```

```
kotlin-lsp extract-sources [PATTERN…] [OPTIONS]
```
Unpacks `*-sources.jar` files from the Gradle module cache so the LSP server can serve hover docs and go-to-definition for library code.

| Option | Default | Description |
|---|---|---|
| `PATTERN…` | (all) | Substring filter on artifact path, e.g. `androidx.compose` `org.jetbrains.kotlin` |
| `--gradle-home <dir>` | `$GRADLE_USER_HOME` or `~/.gradle` | Gradle home directory |
| `--output <dir>` | `~/.kotlin-lsp/sources` | Extraction root |
| `--dry-run` | off | Print what would be extracted; write nothing |

**Typical workflow:**

```sh
# 1. Check what source roots are auto-detected
kotlin-lsp sources --root ./android

# 2. Extract library sources (first time, or after a Gradle sync)
kotlin-lsp extract-sources androidx.compose org.jetbrains.kotlin

# Android SDK, workspace.json, and ~/.kotlin-lsp/sources are picked up automatically.
# 3. Re-index (or restart the server) to pick up new sources
kotlin-lsp index --root ./android
```

The extractor deduplicates by artifact — when multiple versions are cached, only the latest is extracted.


### `check`

```
kotlin-lsp check <FILE> [FILE...]
```

Parses each file with tree-sitter and prints syntax errors. No index or LSP session needed. Exits with code 1 when errors are found.

### `organize-imports`

```
kotlin-lsp organize-imports <FILE> [FILE...]
```

Sorts, deduplicates, and removes unused imports from Kotlin/Java files. Detects which symbols are actually used in the file (excluding import lines) and removes the rest.

### `context`

```
kotlin-lsp context <FILE> <LINE> <COL>
```

One-stop symbol context: definition location, signature (with type substitution), and doc comment in a single call.

### `call-hierarchy`

```
kotlin-lsp call-hierarchy <FILE> <LINE> <COL>
```

Finds callers of a function/method via `rg` pattern search across the workspace.

### `type-hierarchy`

```
kotlin-lsp type-hierarchy <NAME> [--subtypes] [--supertypes]
```

Shows subtypes (classes that extend/implement NAME) and/or supertypes. Default: subtypes only.

## What gets indexed

**Kotlin:** `class`, `interface`, `object`, `fun`, `val`, `var`, `typealias`, constructor parameters, enum entries  
**Java:** `class`, `interface`, `enum`, `method`, `field`, `enum_constant`  
**Swift:** `class`, `struct`, `enum`, `protocol`, `func`, `let`, `var`, `typealias`, `extension`, `init`, enum cases

## Resolution chain

Go-to-definition resolves symbols in this order:

1. **Local file** — indexed symbols in the same file
2. **Local variables / parameters** — line-scanned, catches un-annotated `fun` params
3. **Explicit imports** — exact FQN lookup, then package-filtered index, then `fd` on-demand
4. **Same package** — symbols in files sharing the same `package` declaration
5. **Star imports** — `import com.example.*` checked in the package dir
6. **Superclass hierarchy** — inherited methods from `extends`/`implements`/Kotlin delegation specifiers, up to 4 levels deep, cycle-safe
7. **Project-wide `rg`** — last resort; always finds symbols not yet indexed

`this.member` searches the current class + its supers.  
`super.member` skips the current class and walks the hierarchy directly.

## Completion details

- **Dot-completion** (`repo.`) — resolves the variable's declared type, finds the matching file, returns its public members. Private members are hidden.
- **Bare-word completion** — matches symbols from the current file and the workspace index by prefix (case-aware: lowercase prefix → lowercase suggestions first).
- **Kotlin stdlib** — scope functions (`run`, `apply`, `let`, `also`, `with`), collection extensions (`map`, `filter`, `find`, …), string extensions, and nullable helpers all appear in completion with proper signatures. They sort after project symbols.
- **Lazy loading** — files beyond the initial index limit are parsed on-demand the first time you trigger completion on one of their types.
- **Pre-warming** — when you open a file, its injected/constructor types are pre-warmed in the background so the first dot-completion is instant.
- **Live line scanning** — dot-detection uses the current document text (not the debounced index) so typing `.`, deleting it, and re-typing it always works correctly.
- **Visibility filtering** — `private` members are hidden from dot-completion; `protected`/`internal` members are shown.

## Completion ranking

Completions are scored by match quality:

| Score | Match type | Example |
|---|---|---|
| 0 | Exact prefix (case-insensitive) | `Col` → **Col**umn |
| 1 | CamelCase acronym | `CB` → **C**olumn**B**utton |
| 2 | Substring (same-file/package only) | `View` → RecyclerView |

Results are capped at 150 items; `isIncomplete: true` is returned so the client re-queries as you type.

**Context-aware filtering:**
- Lowercase prefix → only functions, vars, params
- Uppercase prefix → only classes, objects, types
- `@` prefix → only annotation/class kinds
- Cross-package symbols require prefix ≥ 2 characters

## Auto-import

When completing an unimported symbol:

- Start typing a class name (uppercase, ≥ 2 chars) → candidates appear from all indexed files including `sourcePaths`
- Select a candidate → symbol inserted **and** `import pkg.ClassName` added at the correct position
- Same-named classes from different packages appear as separate items with the package in the detail column
- Already-imported symbols appear without a duplicate edit
- Star imports (`import pkg.*`) are respected — no redundant explicit import added

## Ignore pattern semantics

| Pattern | Matches |
|---|---|
| `bazel-*` | Any dir/file named `bazel-*` at **any depth** |
| `third-party/**` | Everything inside `third-party/` relative to workspace root |
| `/abs/path/**` | Absolute path — normalized to relative before matching |

Patterns apply to both `fd` (fast path) and the `walkdir` fallback, and filter the warm-start cached manifest so newly added patterns take effect without clearing the cache.

## Source path behaviour

| Behaviour | `sourcePaths` files |
|---|---|
| Hover / go-to-definition | ✓ |
| Autocomplete | ✓ |
| `findReferences` | ✗ (excluded) |
| `rename` | ✗ (excluded) |

Paths can be absolute (including `~/…`) or relative to the workspace root. The full path is trusted — standard excludes (`.gradle`, `build`, `target`) are not applied.

- Single-hop: `ClassName`, `functionName`, `CONSTANT`
- Multi-hop field chains: `account.profile.email`
- Constructor parameter declarations (without `val`/`var`)
- Lambda parameters: `{ account -> account.name }` jumps to the `account ->` binding
- `this.method()` and `super.method()` qualifier handling
- Precise `fd --full-path` search uses the full package path from the import, not just the filename — dramatically faster in multi-module projects
- Cross-file fallback via `rg` for symbols not yet in the index
