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


**Optional:** Install `fd` and `rg` (ripgrep) for faster file discovery and cross-file search.

## For AI agents (Claude Code, Cursor, Codex, …)

Once `kotlin-lsp` is on your PATH, install the bundled agent skill so your AI tool knows when and how to call it (saves tokens vs. blind `grep`/`rg`):

```bash
npx skills add https://github.com/qdsfdhvh/kotlin-lsp
```

This drops [`skills/kotlin-lsp/SKILL.md`](skills/kotlin-lsp/SKILL.md) into your project's agent directory. The skill teaches the agent to prefer `kotlin-lsp find` / `refs` / `hover` over text-grep for Kotlin/Java/Swift, and how to use the `--module`, `--source-set`, and `--json` filters introduced for agent workflows.


```





**Once your editor is wired up:**

1. Open a Kotlin/Java file — hover, go-to-definition, and completions work immediately via `rg` fallback while the index builds in the background.
2. Library sources are discovered automatically — no configuration needed in most cases:
   - **Android SDK** (`Activity`, `Context`, `View`, …) — detected from `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`
   - **Gradle library sources** (Compose, coroutines, AndroidX, …) — run once to unpack `*-sources.jar` from the Gradle cache:

```bash
kotlin-lsp extract-sources   # one-time
```

   - **IntelliJ/Android Studio projects** — `workspace.json` source roots are picked up automatically, including any `sourcePaths` you've configured there.

---

## Features

| Capability | Notes |
|---|---|
| **Go-to-definition** | Index → superclass hierarchy → `rg` fallback. Multi-hop chains, lambda params, `this`/`super` |
| **Hover** | Signature, visibility, KDoc, deprecated warning, data class props |
| **Completion** | Dot-completion, auto-import, deprecated tag, label_details, scored ranking, stdlib |
| **References** | Project-wide `rg --word-regexp` + open buffers |
| **Document/workspace symbol** | Outline view, fuzzy search, dot-qualified extension function queries |
| **Rename** | Project-wide via `WorkspaceEdit` |
| **Inlay hints** | Lambda `it`, named params, `this`, untyped `val`/`var` |
| **Semantic tokens** | Full syntax highlighting via tree-sitter CST + cross-file resolution |
| **Diagnostics** | Syntax errors from tree-sitter (not type checking) |
| **Folding range** | Brace, import, comment blocks with collapsed text |
| **Selection range** | Smart expand via tree-sitter CST |
| **Call hierarchy** | Incoming (rg) + outgoing (CST walk) |
| **On-type formatting** | Auto de-indent on `}` |
| **Document formatting** | ktfmt / google-java-format / swift-format |
| **Code action** | Introduce variable, add import, suppress warning, generate overrides |

| **Go-to-implementation** | Transitive subtype lookup (BFS) |
| **Signature help** | Active parameter highlighting |
| **CLI mode** | `find`, `refs`, `hover`, `complete`, `index`, `check`, `context`, `call-hierarchy`, `type-hierarchy`, `organize-imports`, `tokens`, `tree`, `sources`, `extract-sources`, `inject`, `list-types`, `insert`, `batch — scriptable, no daemon |

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

