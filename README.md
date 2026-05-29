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

```bash
cargo install kotlin-lsp
```

> No Cargo? Get it at [rustup.rs](https://rustup.rs). After install, `kotlin-lsp` is at `~/.cargo/bin/` — make sure it's on your `PATH`.

**Optional:** Install `fd` and `rg` (ripgrep) for faster file discovery and cross-file search.

### JAR indexer sidecar

For full JAR/library type information (Compose, AndroidX, Kotlin stdlib docs), install the native sidecar alongside `kotlin-lsp`. Download the matching `kotlin-jar-indexer-*` binary from the [latest release](https://github.com/Hessesian/kotlin-lsp/releases/latest) and place it next to `kotlin-lsp`:

```bash
# Linux x86_64 example
tar -xzf kotlin-jar-indexer-linux-x86_64.tar.gz
mv kotlin-jar-indexer-linux-x86_64 ~/.cargo/bin/kotlin-jar-indexer
chmod +x ~/.cargo/bin/kotlin-jar-indexer
```

The sidecar is a self-contained native binary — **no JVM required**. It starts in ~4 ms and uses ~10 MB RAM.

> If the native sidecar is not installed but `java` is on your PATH, `kotlin-lsp` automatically falls back to the JAR version. VS Code and Neovim platform extensions bundle the sidecar automatically.

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

Then add to `~/.config/zed/settings.json`:

```json
{
  "languages": {
    "Kotlin": {
      "language_servers": ["kotlin-lsp", "!kotlin-language-server"],
      "format_on_save": "off",
      "show_completions_on_input": true
    },
    "Java":  { "language_servers": ["kotlin-lsp"], "format_on_save": "off" },
    "Swift": { "language_servers": ["kotlin-lsp"], "format_on_save": "off" }
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
| **References** | Project-wide `rg --word-regexp` + open buffers; scoped to declaring class for fields/properties |
| **Document/workspace symbol** | Outline view, fuzzy search, dot-qualified extension function queries |
| **Rename** | Project-wide via `WorkspaceEdit` |
| **Inlay hints** | Lambda `it`, named params, `this`, untyped `val`/`var`; enriched async via background `rg` pass |
| **Semantic tokens** | Full syntax highlighting via tree-sitter CST + cross-file resolution |
| **Diagnostics** | Syntax errors (tree-sitter), missing `when` branches (sealed/enum), missing call arguments |
| **Go-to-implementation** | Interface methods and abstract functions; transitive subtype BFS; scoped by declaring class |
| **Signature help** | Active parameter highlighting |
| **Code actions** | Fill missing `when` branches for sealed classes and enums |
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
| `--json` | Machine-readable output |
| `--root <dir>` | Workspace root (default: nearest `.git` dir) |

`complete` returns JSON `[{label, kind, detail?, import?}]`. Use `--dot` / `--eol` to auto-place the cursor; `--no-stdlib` skips `~/.kotlin-lsp/sources` for ~5× faster project-only results.

[Full CLI reference →](docs/features.md#cli-subcommands)

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
- [GitHub Copilot CLI](docs/copilot.md) — agent integration, skill extension, Serena MCP setup
- [Architecture & performance](docs/architecture.md) — source layout, memory model
- [Performance & profiling](docs/performance.md) — benchmarks, flamegraph setup
- [Changelog](CHANGELOG.md)

---

## Acknowledgements

Superclass hierarchy resolution, `this`/`super` qualifier handling, and lambda parameter recognition were inspired by [**code-compass.nvim**](https://github.com/emmanueltouzery/code-compass.nvim) by Emmanuel Touzery.
