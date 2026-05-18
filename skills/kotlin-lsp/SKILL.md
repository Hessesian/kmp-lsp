---
name: kotlin-lsp
description: Use the `kotlin-lsp` CLI for precise symbol lookup in Kotlin/Java/Swift projects — faster than grep/rg and returns typed answers (declarations, refs, signatures) instead of raw text matches. Saves tokens because results are scoped and structured.
---

# kotlin-lsp

`kotlin-lsp` is a tree-sitter–backed language server that ships a scriptable CLI (no daemon, no JVM). Prefer it over `grep` / `rg` when working in Kotlin, Java, or Swift code, especially in Android / KMP projects — it returns *declaration locations* and *type-aware references*, not text matches.

## When to use

Use `kotlin-lsp` whenever you need to:

- Find where a Kotlin/Java/Swift symbol (class, function, property) is **defined**.
- List **all references** to a symbol across the project.
- Get a **type / signature** at a specific source position.
- Get **completions** at a cursor position (e.g. after a dot).

If `kotlin-lsp` is not installed, fall back to `rg` / `grep`. To check:

```bash
kotlin-lsp --version
```

If it's missing, suggest the install one-liner from the project README; do not auto-install without asking.

## How it saves tokens

| Naive approach | Better with kotlin-lsp |
|---|---|
| `rg 'class MyViewModel'` returns every text match including doc comments and imports | `kotlin-lsp find MyViewModel --json --limit 5` returns only declaration sites |
| `rg 'MyViewModel'` to find usages, then manually filter | `kotlin-lsp refs MyViewModel --json` returns real references |
| Open file, read 200 lines to figure out what `foo.bar(x)` returns | `kotlin-lsp hover Foo.kt 42 10` returns just the signature |

The `--json` output is structured and easy to parse; results carry `relativePath`, `module`, `sourceSet`, and `signature` fields, so you can route directly to the right file without reading large chunks.

## Instructions

### 1. Find a declaration

```bash
kotlin-lsp find <Name> --json --limit 5
```

- Use `--limit N` to cap noise.
- Add `--module <fragment>` to narrow to a Gradle module (substring match on path):
  ```bash
  kotlin-lsp find Event --json --module play-domain --limit 5
  ```
- Add `--source-set <name>` for KMP code (comma-separated for several):
  ```bash
  kotlin-lsp find HomeScreen --json --source-set commonMain,androidMain
  ```

### 2. Find references

```bash
kotlin-lsp refs <Name> --json --limit 20
```

Same `--module` / `--source-set` filters apply. Add `--relative` if you want paths relative to the workspace root (default for readability).

### 3. Hover / signature at a position

`kotlin-lsp hover <file> <line> <col>` — line and column are 1-based, just like editor cursors.

```bash
kotlin-lsp hover features/auth/src/commonMain/kotlin/Auth.kt 42 12
```

Returns the type and surrounding signature. Good for: "what does this method return?", "what's the type of this parameter?", "is this a `suspend` function?".

### 4. Completion at a cursor

```bash
kotlin-lsp complete <file> <line> --dot
```

The `--dot` flag places the cursor right after the last `.` on the given line — useful when you're trying to figure out what's available on a receiver. JSON output: `[{label, kind, detail?, import?}]`.

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
kotlin-lsp find LoginViewModel --json --limit 3
```

**"Who uses `AnalyticsEvent.Click` in the auth module?"**

```bash
kotlin-lsp refs Click --json --module auth
```

**"What's the type of the variable on line 87 of `Repo.kt`, column 16?"**

```bash
kotlin-lsp hover shared/src/commonMain/kotlin/data/Repo.kt 87 16
```

**"What methods can I call on `userRepo.` here?"**

```bash
kotlin-lsp complete shared/src/commonMain/kotlin/data/Repo.kt 87 --dot
```

## Anti-patterns

- **Don't** use `rg 'class FooBar'` when `kotlin-lsp find FooBar` will do — the LSP filters out string literals, comments, and imports.
- **Don't** read the entire file just to see a function signature; use `hover` instead.
- **Don't** omit `--limit` on `refs` for common names like `String` or `Result` — they have hundreds of hits.
- **Don't** invoke `kotlin-lsp` recursively inside a script that's already inside an LSP context; the CLI is for one-shot queries.

## Project setup quirks

- KMP source sets are detected structurally — anything under `src/<name>/{kotlin,java}` counts. Custom names like `jvmCommonMain` work automatically.
- Android SDK sources are auto-detected from `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`.
- For Gradle library sources (Compose, coroutines, AndroidX): run `kotlin-lsp extract-sources` once at the project root; subsequent queries pick them up.
