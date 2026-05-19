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
| `rg 'class MyViewModel'` returns every text match including doc comments and imports | `kotlin-lsp find MyViewModel --limit 5` returns only declaration sites |
| `rg 'MyViewModel'` to find usages, then manually filter | `kotlin-lsp refs MyViewModel --limit 20` returns real references |
| Open file, read 200 lines to figure out what `foo.bar(x)` returns | `kotlin-lsp hover Foo.kt 42 10` returns just the signature |

**Output is AI-tuned by default**:
- Text mode (default) for `find`/`refs` is **grouped by file** — path on its own line, then one `line:col[ kind]` per match, blank line between file groups. The query name is omitted (it's whatever you typed). Example:
  ```
  app/src/main/kotlin/com/example/Foo.kt
  4:9
  5:19

  shared/src/commonMain/kotlin/Bar.kt
  22:5
  ```
  This is the cheapest text format. For grep-style `path:line:col: name` (one record per line, for piping into `cut`), add `--flat`.
- `--json` emits **compact** JSON (no pretty-print whitespace). Use when you need structured fields like `module`, `sourceSet`, `signature`.
- For high-hit-rate queries, plain text + `--limit` is often cheaper than JSON. Reach for `--json` only when downstream parsing needs the field names.
- `--relative` (workspace-relative paths) is auto-enabled when the CLI's stdout is piped (i.e. always in agent context). Pass `--absolute` to opt out.

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

Same `--module` / `--source-set` filters apply. Add `--relative` to print workspace-relative paths (shorter — saves tokens). Add `--json` when you want `relativePath` / `module` / `sourceSet` as parseable fields.

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

**"What's the type of the variable on line 87 of `Repo.kt`, column 16?"**

```bash
kotlin-lsp hover shared/src/commonMain/kotlin/data/Repo.kt 87 16
```

**"What methods can I call on `userRepo.` here?"**

```bash
kotlin-lsp complete shared/src/commonMain/kotlin/data/Repo.kt 87 --dot
```

## When to reach for kotlin-lsp vs rg

The win is largest when the query crosses module boundaries or touches code rg can't see:

```
Query is about Kotlin/Java/Swift symbols?
├─ No → rg / Read
└─ Yes:
   ├─ Symbol name is unique AND in this repo → rg --type kotlin is fine (and faster)
   ├─ Symbol name is generic (handle, String, Event, …) → kotlin-lsp find/refs --module … --limit
   ├─ Symbol lives in library (Compose, AndroidX, 3rd-party) → kotlin-lsp find (rg cannot reach)
   ├─ Symbol lives in generated code (build/openapi/, build/i18n/) → kotlin-lsp find (rg blocked by .ignore)
   ├─ Need cross-module ref filtering (--module / --source-set) → kotlin-lsp refs
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
