---
name: kotlin-lsp
description: Use the `kotlin-lsp` CLI for precise symbol lookup in Kotlin/Java/Swift projects — faster than grep/rg and returns typed answers (declarations, refs, signatures) instead of raw text matches. Saves tokens because results are scoped and structured.
---

# kotlin-lsp

`kotlin-lsp` is a tree-sitter–backed language server that ships a scriptable CLI (no daemon, no JVM). Prefer it over `grep` / `rg` when working in Kotlin, Java, or Swift code, especially in Android / KMP projects — it returns *declaration locations* and *type-aware references*, not text matches.

## When to use

Use `kotlin-lsp` whenever you need to:

- Get **one-stop context** (definition + signature + docs) for a symbol.
- Check files for **syntax errors** without starting LSP.
- Find **callers / callees** of a function.
- Find **subtypes / supertypes** of a class.
- **Organize imports** (sort, dedup, remove unused).
- **Inject types** in batch for a file.
- **List all types** in the project.
- **Insert code** at a specific line.
- **Batch modify** multiple files atomically via rules.
- **Inject types** in batch for a file (one call = N context calls).
- **List all types** in the project with optional filters.
- Find where a Kotlin/Java/Swift symbol (class, function, property) is **defined**.
- List **all references** to a symbol across the project.
- Get a **type / signature** at a specific source position.
- Get **completions** at a cursor position (e.g. after a dot).

If `kotlin-lsp` is not installed, fall back to `rg` / `grep`. To check:

```bash
kotlin-lsp --version
```

If it's missing, suggest the install one-liner from the project README; do not auto-install without asking.

## Reporting pain points

When using `kotlin-lsp` on a project task, keep track of concrete tool pain points such as false-positive `check` output, overly broad `refs`, missing narrowing flags, slow commands, or confusing output. If the user allows or requests upstream feedback, file an issue against `qdsfdhvh/kotlin-lsp` with a small sanitized repro and the expected/actual behavior.

Privacy rule: never include the current project's repo name, file paths, package names, class/function names, logs, business terminology, screenshots, or source snippets in upstream issues unless the user explicitly provides public repro material. Rewrite examples as generic placeholders like `example-domain`, `FeatureViewModel`, `ScreenState`, and `path/to/File.kt`; prefer minimal synthetic code that reproduces the tool behavior.

## How it saves tokens

| Naive approach | Better with kotlin-lsp |
|---|---|
| `rg 'class MyViewModel'` returns every text match including doc comments and imports | `kotlin-lsp find MyViewModel --limit 5` returns only declaration sites |
| `rg 'MyViewModel'` to find usages, then manually filter | `kotlin-lsp refs MyViewModel --limit 20` returns real references |
| Open file, read 200 lines to figure out what `foo.bar(x)` returns | `kotlin-lsp hover Foo.kt 42 10` returns just the signature |

**Output is AI-tuned by default**:
- Text mode (default) for `find`/`refs` is **grouped by file with structural annotation** — path on its own line followed by `[<module> <sourceSet>]` when known, then one `line:col[ kind]` per match, blank line between file groups. The query name is omitted (it's whatever you typed). Example:
  ```
  features/auth-domain/src/commonMain/kotlin/.../SessionRefresherImpl.kt [features/auth-domain commonMain]
  37:14

  features/play-export/src/commonMain/kotlin/.../ChatArchiveViewModel.kt [features/play-export commonMain]
  16:16
  ```
  The `[module sourceSet]` tail lets you filter results by Gradle module or KMP source-set in a second pass without re-parsing the path string — same semantic info `--json` carries as `module` / `sourceSet` fields, but cheaper. The annotation is empty for files outside any module (top-level scripts, `build.gradle.kts`, etc.). For grep-style `path:line:col: name`, add `--flat`.
- `--json` emits **compact** JSON. Reach for it when you need the data as a structured object — e.g. piping through `jq` for complex filtering, or when `signature` / `relativePath` matter to downstream code.
- Plain text + `--limit` is the default for typical "where is X" / "who calls Y" queries — cheaper, and the `[module sourceSet]` annotation already carries the structural info you'd reach for `--json` to get.
- `--relative` (workspace-relative paths) is auto-enabled when the CLI's stdout is piped (always true in agent context). Pass `--absolute` to opt out.

## Instructions

### 1. Find a declaration

```bash
kotlin-lsp find <Name> --limit 5
```

- Drop `--json` unless you need the structured fields — plain text is cheaper.
- `--limit N` caps noise.
- `--module <fragment>` narrows to a Gradle module (substring match on path):
  ```bash
  kotlin-lsp find Event --module play-domain --limit 5
  ```
- `--source-set <name>` filters KMP code (comma-separated for OR):
  ```bash
  kotlin-lsp find HomeScreen --source-set commonMain,androidMain
  ```

### 2. Find references

```bash
kotlin-lsp refs <Name> --limit 20
```

Same `--module` / `--source-set` / `--owner` filters apply. Add `--relative` to print workspace-relative paths (shorter — saves tokens). Add `--json` when you want `relativePath` / `module` / `sourceSet` / `owner` as parseable fields.

### 3. Hover / signature at a position

`kotlin-lsp hover <file> <line> <col>` — line and column are 1-based, just like editor cursors.

```bash
kotlin-lsp hover features/auth/src/commonMain/kotlin/Auth.kt 42 12
```

Returns the type and surrounding signature. Good for: "what does this method return?", "what's the type of this parameter?", "is this a `suspend` function?".

**Important limitation**: hover only resolves at the **declaration site**, not at call sites. If you point at a call (e.g. `repo.login()` inside another function) hover returns nothing. The two-call workaround:

```bash
# Step 1: locate the declaration
kotlin-lsp find login --limit 1
#   features/auth/src/commonMain/kotlin/Auth.kt:42:5: fun login

# Step 2: hover the declaration
kotlin-lsp hover features/auth/src/commonMain/kotlin/Auth.kt 42 5
```

Two calls are still cheaper than `Read`-ing the file, but skip the dance for hot paths where you already know the signature.

### 4. Completion at a cursor

```bash
kotlin-lsp complete <file> <line> --dot
```

The `--dot` flag places the cursor right after the last `.` on the given line — useful when figuring out what's available on a receiver. Text output is tab-separated: `label\tkind\tdetail\timport`. JSON output: `[{label, kind, detail?, import?}]`.

### 5. Performance flags

| Flag | When |
|---|---|
| _(none)_ | Auto — use cached index if available, else fast `rg`/`fd` fallback |
| `--fast` | Always use `rg`/`fd`; instant, no index needed |
| `--smart` | Require index; build it if missing (slower first call, accurate after) |
| `--root <dir>` | Override workspace root (default: nearest `.git` directory) |
| `--no-stdlib` | Skip library sources; ~5× faster for project-only queries |

## Examples

**"Where is `LoginViewModel` defined?"**

```bash
kotlin-lsp find LoginViewModel --limit 3
```

**"Who uses `AnalyticsEvent.Click` in the auth module?"**

```bash
kotlin-lsp refs Click --module auth --limit 20
```

**"Who uses `Refresh` in `ScreenAction`?"**

```bash
kotlin-lsp refs Refresh --owner ScreenAction --limit 20
```

**"What's the type of the variable on line 87 of `Repo.kt`, column 16?"**

```bash
kotlin-lsp hover shared/src/commonMain/kotlin/data/Repo.kt 87 16
```

**"What methods can I call on `userRepo.` here?"**

```bash
kotlin-lsp complete shared/src/commonMain/kotlin/data/Repo.kt 87 --dot
```


### 5. One-stop context

```bash
kotlin-lsp context <file> <line> <col>
```

Returns definition + signature + doc comment in a single call. Good for: "tell me everything about this symbol".

### 6. Check syntax errors

```bash
kotlin-lsp check <file> [file...]
```

Parses files with tree-sitter and reports syntax errors. No index needed. Exit code 1 if errors found.

### 7. Call hierarchy

```bash
kotlin-lsp call-hierarchy <file> <line> <col>
```

Finds callers of a function via `rg` across the workspace.

### 8. Type hierarchy

```bash
kotlin-lsp type-hierarchy <Name> [--subtypes] [--supertypes]
```

Shows subtypes (classes implementing/extending) and/or supertypes. Default: subtypes only.

### 9. Organize imports

```bash
kotlin-lsp organize-imports <file> [file...]
```

Sorts, deduplicates, and removes unused imports from Kotlin/Java files.



### 10. Batch type injection

```bash
kotlin-lsp inject <file>
```

Reads a file, extracts all referenced type names, and returns their signatures in one batch. One call replaces N context calls. Ideal for AI-agent Read Hooks.

### 11. List all types

```bash
kotlin-lsp list-types [--limit N]
```

Lists all known types in the workspace index, grouped by module.


### 12. Insert code at a line

```bash
kotlin-lsp insert <file> <line> --after --content "..." [--in-place]
```

Inserts text before or after a given line. With --in-place, writes back to file.

### 13. Cross-file batch modifications

```bash
kotlin-lsp batch <rule.json> [--dry-run]
```

Applies find-replace and insert operations across multiple files atomically via JSON rules. Use --dry-run to preview.

## When to reach for kotlin-lsp vs rg


Reads a file, extracts referenced type names and returns signatures in one batch.

### 11. List all types


The win is largest when the query crosses module boundaries or touches code rg can't see:


Lists all known types in the workspace index.
```
Query is about Kotlin/Java/Swift symbols?
├─ No → rg / Read
└─ Yes:
   ├─ Symbol name is unique AND in this repo → rg --type kotlin is fine (and faster)
   ├─ Symbol name is generic (handle, String, Event, …) → kotlin-lsp find/refs --module … --limit
   ├─ Symbol lives in library (Compose, AndroidX, 3rd-party) → kotlin-lsp find (rg cannot reach)
   ├─ Symbol lives in generated code (build/openapi/, build/i18n/) → kotlin-lsp find (rg blocked by .ignore)
   ├─ Need cross-module ref filtering (--module / --source-set / --owner) → kotlin-lsp refs
   ├─ Need one-stop symbol info (def + sig + doc) → kotlin-lsp context <file> <line> <col>
   ├─ Need syntax check on edited files → kotlin-lsp check <file>
   ├─ Need call hierarchy → kotlin-lsp call-hierarchy <file> <line> <col>
   ├─ Need class hierarchy → kotlin-lsp type-hierarchy <Name>
   ├─ Imports are messy → kotlin-lsp organize-imports <file>
   ├─ Need batch type injection for a file → kotlin-lsp inject <file>
   ├─ Need project-wide type listing → kotlin-lsp list-types
   ├─ Need signature/type at a declaration → kotlin-lsp hover <file> <line> <col>
   └─ Need signature at a call site → kotlin-lsp find <name> (jump to decl), then hover the decl
```

Rough byte savings on a real KMP monorepo (kataris):

| Scenario | rg | kotlin-lsp (plain+relative) | Saving |
|---|---|---|---|
| Generic name like `handle` across exports | 7.7 KB | 2.9 KB | ~60% |
| Library symbol like `LazyColumn` | impossible | 1.1 KB | n/a (rg can't reach) |
| Hover at declaration | 0.5 KB | 32 B | ~94% |

## Anti-patterns

- **Don't** use `rg 'class FooBar'` when `kotlin-lsp find FooBar` will do — the LSP filters out string literals, comments, and imports.
- **Don't** read the entire file just to see a function signature; use `hover` instead.
- **Don't** omit `--limit` on `refs` for common names like `String` or `Result` — they have hundreds of hits.
- **Don't** invoke `kotlin-lsp` recursively inside a script that's already inside an LSP context; the CLI is for one-shot queries.

## Project setup quirks

- KMP source sets are detected structurally — anything under `src/<name>/{kotlin,java}` counts. Custom names like `jvmCommonMain` work automatically.
- Android SDK sources are auto-detected from `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`.
- For Gradle library sources (Compose, coroutines, AndroidX): run `kotlin-lsp extract-sources` once at the project root; subsequent queries pick them up.
