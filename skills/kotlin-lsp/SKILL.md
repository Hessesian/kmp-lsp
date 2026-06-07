---
name: kotlin-lsp
description: Use the `kotlin-lsp` CLI for precise symbol lookup in Kotlin/Java/Swift projects — faster than grep/rg and returns typed answers (declarations, refs, signatures) instead of raw text matches. Saves tokens because results are scoped and structured.
---

# kotlin-lsp

`kotlin-lsp` is a tree-sitter–backed language server that ships a scriptable CLI (no daemon, no JVM). Reach for it when working with Kotlin, Java, or Swift symbols, especially in Android / KMP projects — it returns *declaration locations* and *type-aware references*, not text matches.

Check installation:

```bash
kotlin-lsp --version
```

If missing, suggest the install one-liner from the project README; do not auto-install without asking.

## When to use kotlin-lsp vs rg

```
Query is about Kotlin/Java/Swift symbols?
├─ No → rg / Read
└─ Yes:
   ├─ Symbol name is unique AND in this repo → rg --type kotlin is fine (and faster)
   ├─ Symbol name is generic (handle, String, Event, …) → kotlin-lsp find/refs --module … --limit
   ├─ Symbol lives in library (Compose, AndroidX, 3rd-party) → kotlin-lsp find (rg cannot reach)
   ├─ Symbol lives in generated or ignored code → kotlin-lsp find (plain rg may miss it)
   ├─ Need cross-module ref filtering (--module / --source-set / --owner) → kotlin-lsp refs
   ├─ Need one-stop symbol info (def + sig + doc) → kotlin-lsp context <file> <line> <col>
   ├─ Need syntax check on edited files → kotlin-lsp check <file>
   ├─ Need call hierarchy → kotlin-lsp call-hierarchy <file> <line> <col>
   ├─ Need class hierarchy → kotlin-lsp type-hierarchy <Name>
   ├─ Imports are messy → kotlin-lsp organize-imports <file>
   ├─ Need batch type injection for a file → kotlin-lsp inject <file>
   ├─ Need signature/type at a declaration → kotlin-lsp hover <file> <line> <col>
   └─ Need signature at a call site → kotlin-lsp find <name> (jump to decl), then hover the decl
```

## How it saves tokens

| Naive approach | Better with kotlin-lsp |
|---|---|
| `rg 'class MyViewModel'` returns every text match including doc comments and imports | `kotlin-lsp find MyViewModel --limit 5` returns only declaration sites |
| `rg 'MyViewModel'` to find usages, then manually filter | `kotlin-lsp refs MyViewModel --limit 20` returns real references |
| Open file, read 200 lines to figure out what `foo.bar(x)` returns | `kotlin-lsp hover Foo.kt 42 10` returns just the signature |

**Output is AI-tuned by default:**
- Text mode (default) for `find`/`refs` is **grouped by file with structural annotation** — path on its own line followed by `[<module> <sourceSet>]` when known, then one `line:col[ kind]` per match, blank line between file groups.
- `--json` emits **compact** JSON. Use when you need structured data for `jq` or when `signature` / `relativePath` matter downstream.
- `--relative` (workspace-relative paths) is **auto-enabled when stdout is piped** (always true in agent context). Pass `--absolute` to opt out.

## Commands

### Symbol lookup

#### find — declaration search

```bash
kotlin-lsp find <Name> [--limit N] [--module <fragment>] [--source-set <set>] [--kind class,fun] [--owner <name>]
```

- `--limit N` caps noise.
- `--module <frag>` narrows to a Gradle module (substring match on path).
- `--source-set <name>` filters KMP code (comma-separated for OR).
- `--kind class,fun,interface` filters by symbol kind.
- `--owner <name>` filters by enclosing class/interface/object.

```bash
kotlin-lsp find LoginViewModel --limit 3
kotlin-lsp find Event --module play-domain --limit 5
kotlin-lsp find HomeScreen --source-set commonMain,androidMain
```

#### refs — reference search

```bash
kotlin-lsp refs <Name> [--limit N] [--module <fragment>] [--source-set <set>] [--owner <name>]
```

Same filters as `find`. Add `--json` when you need `relativePath` / `module` / `sourceSet` / `owner` as parseable fields.

```bash
kotlin-lsp refs Click --module auth --limit 20
kotlin-lsp refs Refresh --owner ScreenAction --limit 20
kotlin-lsp refs ScreenAction.Refresh              # auto-detect owner
```

#### hover — signature at position

```bash
kotlin-lsp hover <file> <line> <col>
```

Line and column are 1-based, like editor cursors. Returns the type and surrounding signature.

**Important limitation**: hover only resolves at the **declaration site**, not at call sites. Two-step workaround:

```bash
# Step 1: locate the declaration
kotlin-lsp find login --limit 1
#   features/auth/src/commonMain/kotlin/Auth.kt:42:5: fun login

# Step 2: hover the declaration
kotlin-lsp hover features/auth/src/commonMain/kotlin/Auth.kt 42 5
```

Two calls are still cheaper than reading the file.

### Navigation & hierarchy

#### complete — cursor completions

```bash
kotlin-lsp complete <file> <line> [col] [--dot|--eol] [--no-stdlib]
```

Pass `col`, or use `--dot` to place the cursor after the last `.` on the line, or `--eol` to use the end of trimmed line content. Text output is tab-separated: `label\tkind\tdetail\timport`. JSON output: `[{label, kind, detail?, import?}]`.

```bash
kotlin-lsp complete shared/src/commonMain/kotlin/data/Repo.kt 87 --dot
```

#### context — one-stop symbol info

```bash
kotlin-lsp context <file> <line> <col>
```

Returns definition + signature + doc comment in a single call. Good for: "tell me everything about this symbol".

#### call-hierarchy — caller lookup

```bash
kotlin-lsp call-hierarchy <file> <line> <col>
```

Finds callers of a function via `rg` across the workspace.

#### type-hierarchy — class hierarchy

```bash
kotlin-lsp type-hierarchy <Name> [--subtypes] [--supertypes]
```

Shows subtypes (classes implementing/extending) and/or supertypes. Default: subtypes only.

### Project analysis

#### check — syntax validation

```bash
kotlin-lsp check <file> [file...]
```

Parses files with tree-sitter and reports syntax errors. No index needed. Exit code 1 if errors found.

#### inject — batch type injection

```bash
kotlin-lsp inject <file>
```

Reads a file, extracts all referenced type names, and returns their signatures in one batch. One call replaces N context calls.

### Code manipulation

#### code-action — inspect or apply quick fixes

```bash
kotlin-lsp code-action <file> <line> <col> [--apply]
```

List available code actions first. Use `--apply` only when the intended edit is obvious.

#### organize-imports — import cleanup

```bash
kotlin-lsp organize-imports <file> [file...]
```

Sorts, deduplicates, and removes unused imports from Kotlin/Java files.

#### insert — code insertion

```bash
kotlin-lsp insert <file> <line> (--before|--after) --content "..." [--in-place]
```

Inserts text before or after a given line. With `--in-place`, writes back to file.

#### batch — cross-file batch modifications

```bash
kotlin-lsp batch <rule.json> [--dry-run]
```

Applies find-replace and insert operations across multiple files atomically via JSON rules. Use `--dry-run` to preview.

## Performance flags

| Flag | When |
|---|---|
| _(none)_ | Auto — use cached index if available, else fast `rg`/`fd` fallback |
| `--fast` | Always use `rg`/`fd`; instant, no index needed |
| `--smart` | Require a pre-built index; run `kotlin-lsp index` first |
| `--root <dir>` | Override workspace root (default: nearest `.git` directory) |
| `--no-stdlib` | For `complete`: skip extracted stdlib/library sources for faster workspace-only suggestions |

## Indexing & library sources

- **KMP source sets** are detected structurally — anything under `src/<name>/{kotlin,java}` counts. Custom names like `jvmCommonMain` work automatically.
- **Android SDK sources** are auto-detected from `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`.
- **Gradle library sources** (Compose, coroutines, AndroidX): run once:
  ```bash
  kotlin-lsp extract-sources
  ```
  Subsequent queries pick them up.
- **Pre-build index** for faster first-lookup:
  ```bash
  kotlin-lsp index --root ./android
  ```
- **Cache diagnostics**:
  ```bash
  kotlin-lsp cache stats
  ```

## Anti-patterns

- **Don't** use `rg 'class FooBar'` when `kotlin-lsp find FooBar` will do — the LSP filters out string literals, comments, and imports.
- **Don't** read the entire file just to see a function signature; use `hover` instead.
- **Don't** omit `--limit` on `refs` for common names like `String` or `Result` — they have hundreds of hits.
- **Don't** invoke `kotlin-lsp` recursively inside a script that's already inside an LSP context; the CLI is for one-shot queries.

## Reporting pain points

When using `kotlin-lsp` on a project task, keep track of concrete tool pain points such as false-positive `check` output, overly broad `refs`, missing narrowing flags, slow commands, or confusing output. If the user allows or requests upstream feedback, file an issue against `qdsfdhvh/kotlin-lsp` with a small sanitized repro and the expected/actual behavior.

**Privacy rule**: never include the current project's repo name, file paths, package names, class/function names, logs, business terminology, screenshots, or source snippets in upstream issues unless the user explicitly provides public repro material. Rewrite examples as generic placeholders like `example-domain`, `FeatureViewModel`, `ScreenState`, and `path/to/File.kt`; prefer minimal synthetic code that reproduces the tool behavior.

## Help and debug commands

Run `kotlin-lsp --help` for the exhaustive command list. Keep routine agent output focused on the task; do not paste full help text unless the user asks. Debug commands such as `tokens`, `tree`, and `benchmark` are primarily for kotlin-lsp development, not ordinary project navigation.
