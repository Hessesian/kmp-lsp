# kotlin-lsp

[![crates.io](https://img.shields.io/crates/v/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![downloads](https://img.shields.io/crates/d/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![release](https://img.shields.io/github/v/release/Hessesian/kotlin-lsp)](https://github.com/Hessesian/kotlin-lsp/releases/latest)
[![build](https://img.shields.io/github/actions/workflow/status/Hessesian/kotlin-lsp/ci.yml)](https://github.com/Hessesian/kotlin-lsp/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/kotlin-lsp)](LICENSE)

A fast, low-memory LSP server for **Kotlin**, **Java**, and **Swift**, written in Rust.  
Built with [tree-sitter](https://tree-sitter.github.io/) — instant startup, no JVM.

![kotlin-lsp demo](demo/demo.gif)

## Install

**macOS / Linux** — one-liner (downloads a prebuilt binary from the latest release into `~/.local/bin`):

```bash
curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh | bash
```

**Windows (PowerShell)** — one-liner (drops `kotlin-lsp.exe` into `%USERPROFILE%\.kotlin-lsp\bin` and adds it to user PATH):

```powershell
iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex
```

Both scripts support `KOTLIN_LSP_VERSION` (pin a tag like `v0.14.0`) and `KOTLIN_LSP_PREFIX` (override the install dir).

**Optional:** Install `fd` and `rg` (ripgrep) for faster file discovery and cross-file search.

## For AI agents (Claude Code, Cursor, Codex, …)

Once `kotlin-lsp` is on your PATH, install the bundled agent skill so your AI tool knows when and how to call it (saves tokens vs. blind `grep`/`rg`):

```bash
npx skills add https://github.com/qdsfdhvh/kotlin-lsp
```

This drops [`skills/kotlin-lsp/SKILL.md`](skills/kotlin-lsp/SKILL.md) into your project's agent directory. The skill teaches the agent to prefer `kotlin-lsp find` / `refs` / `hover` over text-grep for Kotlin/Java/Swift, and how to use the `--module`, `--source-set`, and `--json` filters introduced for agent workflows.

Project-scoped by default; pass `-g` to install globally for all your projects, or `-a <agent>` to target a specific agent (`claude-code`, `cursor`, `codex`, etc.).

## Quick start

**VS Code** — download and install the `.vsix` from the [latest release](https://github.com/Hessesian/kotlin-lsp/releases/latest):

```bash
code --install-extension kotlin-lsp-linux-x64-vX.Y.Z.vsix   # Linux
code --install-extension kotlin-lsp-darwin-arm64-vX.Y.Z.vsix # macOS Apple Silicon
```

The extension bundles syntax highlighting and launches `kotlin-lsp` automatically.

**Zed** — install the bundled extension (registers `kotlin-lsp` from `$PATH`, no manual wiring):

```bash
zed --install-dev-extension contrib/zed-extension
```

Then suppress the default JVM server in `~/.config/zed/settings.json`:

```json
{
  "languages": {
    "Kotlin": { "language_servers": ["kotlin-lsp", "!kotlin-language-server"] },
    "Java":   { "language_servers": ["kotlin-lsp"] },
    "Swift":  { "language_servers": ["kotlin-lsp"] }
  }
}
```

[Full Zed setup + manual wiring option →](docs/editors.md#zed)

**Helix** — add to `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "kotlin"
language-servers = ["kotlin-lsp"]

[[language]]
name = "java"
language-servers = ["kotlin-lsp"]

[language-server.kotlin-lsp]
command = "kotlin-lsp"
```

[Neovim, Zed setup →](docs/editors.md)

**Once your editor is wired up:**

1. Open a Kotlin/Java file — hover, go-to-definition, and completions work immediately via `rg` fallback while the index builds in the background.
2. Library sources are discovered automatically — no configuration needed in most cases:
   - **Android SDK** (`Activity`, `Context`, `View`, …) — detected from `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`
   - **Gradle library sources** (Compose, coroutines, AndroidX, …) — run once to unpack `*-sources.jar` from the Gradle cache:

```bash
kotlin-lsp extract-sources   # one-time; restart editor after
```

   - **IntelliJ/Android Studio projects** — `workspace.json` source roots are picked up automatically, including any `sourcePaths` you've configured there.

---

## Features

| Capability | Notes |
|---|---|
| **Go-to-definition** | Index → superclass hierarchy → `rg` fallback. Multi-hop chains, lambda params, `this`/`super` |
| **Hover** | Declaration signature, lambda param types, Kotlin stdlib docs |
| **Completion** | Dot-completion with type resolution, auto-import, scored ranking, stdlib entries |
| **References** | Project-wide `rg --word-regexp` + open buffers |
| **Document/workspace symbol** | Outline view, fuzzy search, dot-qualified extension function queries |
| **Rename** | Project-wide via `WorkspaceEdit` |
| **Inlay hints** | Lambda `it`, named params, `this`, untyped `val`/`var` |
| **Semantic tokens** | Full syntax highlighting via tree-sitter CST + cross-file resolution |
| **Diagnostics** | Syntax errors from tree-sitter (not type checking) |
| **Go-to-implementation** | Transitive subtype lookup (BFS) |
| **Signature help** | Active parameter highlighting |
| **Folding** | Brace regions + consecutive comment blocks |
| **CLI mode** | `find`, `refs`, `hover`, `index`, `complete`, `tokens`, `tree`, `sources`, `extract-sources` — scriptable, no daemon |

All features work immediately — `rg` fallback handles symbols before indexing finishes.

### What gets indexed

| Language | Symbols |
|---|---|
| **Kotlin** | `class`, `interface`, `object`, `fun`, `val`, `var`, `typealias`, constructor params, enum entries |
| **Java** | `class`, `interface`, `enum`, `method`, `field`, `enum_constant` |
| **Swift** | `class`, `struct`, `enum`, `protocol`, `func`, `let`, `var`, `typealias`, `extension`, `init`, enum cases |

---

## CLI

`kotlin-lsp` works standalone — no editor, no daemon.

![kotlin-lsp CLI demo](demo/cli.gif)

```bash
kotlin-lsp find MyViewModel              # search declarations
kotlin-lsp refs MyViewModel              # find all references
kotlin-lsp hover src/Foo.kt 42 10        # hover info at line 42, col 10
kotlin-lsp complete src/Foo.kt 42 --dot  # completions after last '.' on line 42
kotlin-lsp index --root ./android        # pre-build cache
kotlin-lsp sources --root ./android      # list detected source roots
kotlin-lsp extract-sources               # unpack library sources from Gradle cache
```

| Flag | Behaviour |
|---|---|
| _(none)_ | Auto: use cached index if available, fall back to fast `rg`/`fd` |
| `--fast` | Always use `rg`/`fd`; instant, no index needed |
| `--smart` | Require index; build it if missing |
| `--json` | Compact JSON output (no whitespace); pipe to `jq` for human reading |
| `--relative` | Print workspace-relative paths. **Auto-enabled when stdout isn't a TTY** (typical AI agent invocation) |
| `--absolute` | Force absolute paths; opt out of the non-TTY auto-relative default |
| `--flat` | Use legacy grep-style `<path>:<line>:<col>: <name>` format (one full path per line) |
| `--root <dir>` | Workspace root (default: nearest `.git` dir) |

### Output format

`kotlin-lsp` is tuned for AI agents: text is grouped to avoid repeating the path on every line, JSON is compact, and absolute paths are stripped when piped.

**`find` / `refs`** — file path on its own line, then one `<line>:<col>[ <kind>]` per match. Blank line between file groups. The query symbol's name is omitted because it's whatever you typed on the command line (`find <NAME>` / `refs <NAME>`) — repeating it on every row is pure waste.

```text
app/src/main/kotlin/com/example/Foo.kt
4:9
5:19
11:19

app/src/main/kotlin/com/example/Bar.kt
22:5
```

For grep-pipe compatibility (`cut -d: -f1` etc.) pass `--flat`:

```text
app/src/main/kotlin/com/example/Foo.kt:4:9: greet
app/src/main/kotlin/com/example/Foo.kt:5:19: greet
```

With `--json` (compact; `relativePath` is folded into `file` because `--relative` is on by default for non-TTY callers):

```json
[{"file":"app/src/main/kotlin/com/example/Foo.kt","line":4,"col":9,"name":"greet","module":"app","sourceSet":"main"}]
```

Pass `--absolute` to get an absolute `file` plus the original `relativePath`/`module`/`sourceSet` fields.

**`hover`** — signature on the first line, docs (if any) after a blank line. With `--json`: `{"signature":"…"}`.

**`complete`** — tab-separated `label\tkind\tdetail\timport` (empty trailing columns kept so `cut -f1` is stable). With `--json`: `[{"label","kind","detail"?,"import"?}]`.

**`sources`** — one existing path per line (text); `[{"path","origin","exists"}]` with `--json`.

[Full CLI reference + per-flag examples →](docs/features.md#cli-subcommands)

---

## Configuration

### Workspace root

Resolved in order:

1. `KOTLIN_LSP_WORKSPACE_ROOT` env var
2. LSP client `rootUri` / `workspaceFolders`
3. `~/.config/kotlin-lsp/workspace` file (for clients that send no root)

### Ignore patterns

```toml
# ~/.config/helix/languages.toml
[language-server.kotlin-lsp.config.indexingOptions]
ignorePatterns = ["bazel-*", "build/**", "third-party/**"]
```

Patterns follow gitignore glob rules and apply to both `fd` and `walkdir` fallback.

### Source paths

Library sources are resolved automatically — no manual config needed in most cases:

| Source | How it's discovered |
|---|---|
| Android SDK (`Activity`, `Context`, …) | `sdk.dir` in `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT` |
| Gradle library sources (Compose, coroutines, …) | `~/.kotlin-lsp/sources` after running `kotlin-lsp extract-sources` |
| IntelliJ/Android Studio project roots | `workspace.json` at project root (exported by IDE) |
| Standard Gradle/Maven layouts | `src/main/kotlin`, `src/test/kotlin`, per-module subprojects |

**`workspace.json`** — JetBrains IDEs export this file to the project root. It describes every module's source roots and lets you override library source directories:

```json
{
  "sourcePaths": [
    "<WORKSPACE>/custom-stubs",
    "/absolute/path/to/generated-sources"
  ]
}
```

When `sourcePaths` is present (even as `[]`), it overrides the `~/.kotlin-lsp/sources` default. Use `[]` to disable all library sources for a specific project.

**Manual override** via LSP config (for custom stubs or generated code):

```toml
# ~/.config/helix/languages.toml
[language-server.kotlin-lsp.config.indexingOptions]
sourcePaths = ["buildSrc/src", "/path/to/generated-stubs"]
```

Source path files are indexed for hover and completions but excluded from `findReferences` and `rename`.

[Full configuration reference →](docs/features.md)

---

## Limitations

- **No type inference** for generic lambda parameters — use explicit annotations for unresolvable cases
- **No type checking** — syntax errors only; use Gradle/Xcode/CI for semantic diagnostics
- **Swift support is structural** — all symbols indexed; no module boundaries or closure type inference
- **Java completion** is less refined than Kotlin
- **`findReferences` on common names** returns noise — name-based search via `rg`, no import filtering yet
- **Binary `.aar`/`.jar`** — only the public API surface is available; full source navigation requires a `*-sources.jar` (use `kotlin-lsp extract-sources`). Direct class-file indexing is [planned](https://github.com/Hessesian/kotlin-lsp/issues/79).

---

## vs. Official Kotlin LSP

| | **kotlin-lsp** | **[Kotlin/kotlin-lsp](https://github.com/Kotlin/kotlin-lsp)** (JetBrains) |
|---|---|---|
| **Runtime** | Native Rust, no JVM | JVM 17+, ~500 MB |
| **Startup** | Instant | Gradle import (slow) |
| **Memory** | < 200 MB | 1+ GB |
| **Accuracy** | Syntactic (tree-sitter) | Full IntelliJ Analysis API |
| **Editor support** | Any LSP editor | VS Code (official) |
| **Swift** | ✓ | ✗ |

They can coexist — use kotlin-lsp for fast navigation, the official one for type-checked diagnostics.

---

## Learn more

- [Feature details](docs/features.md) — resolution chain, completion, CLI reference
- [Editor setup](docs/editors.md) — Helix, Neovim, VS Code, Zed
- [GitHub Copilot CLI](docs/copilot.md) — agent integration, skill extension
- [Architecture & performance](docs/architecture.md) — source layout, memory model
- [Performance & profiling](docs/performance.md) — benchmarks, flamegraph setup
- [Changelog](CHANGELOG.md)

---

## Acknowledgements

Superclass hierarchy resolution, `this`/`super` qualifier handling, and lambda parameter recognition were inspired by [**code-compass.nvim**](https://github.com/emmanueltouzery/code-compass.nvim) by Emmanuel Touzery.
