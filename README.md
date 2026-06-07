# kotlin-lsp

[![crates.io](https://img.shields.io/crates/v/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![downloads](https://img.shields.io/crates/d/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![release](https://img.shields.io/github/v/release/qdsfdhvh/kotlin-lsp)](https://github.com/qdsfdhvh/kotlin-lsp/releases/latest)
[![build](https://img.shields.io/github/actions/workflow/status/qdsfdhvh/kotlin-lsp/ci.yml)](https://github.com/qdsfdhvh/kotlin-lsp/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/kotlin-lsp)](LICENSE)

A fast, low-memory LSP server for **Kotlin**, **Java**, and **Swift**, written in Rust.  
Built with [tree-sitter](https://tree-sitter.github.io/) — instant startup, no JVM.

![kotlin-lsp demo](demo/demo.gif)

## Install

### macOS / Linux

Install the latest prebuilt binary:

```bash
curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh | bash
kotlin-lsp --version
```

The installer detects your OS and CPU architecture, downloads the matching
release asset, and writes `kotlin-lsp` to `~/.local/bin` by default.

Pin a specific release:

```bash
curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh \
  | KOTLIN_LSP_VERSION=v0.20.0 bash
```

Install into another directory:

```bash
curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh \
  | KOTLIN_LSP_PREFIX=/usr/local/bin bash
```

If the install directory is not on `PATH`, add it and open a new shell:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
```

### Windows

Install the latest prebuilt binary from PowerShell:

```powershell
iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex
kotlin-lsp --version
```

The installer writes `kotlin-lsp.exe` to
`%USERPROFILE%\.kotlin-lsp\bin` and adds that directory to your user `PATH`.
Open a new terminal if `kotlin-lsp` is not found immediately.

Pin a specific release:

```powershell
$env:KOTLIN_LSP_VERSION = 'v0.20.0'
iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex
```

### Update

Run the same installer again. It overwrites the existing binary and verifies
that the new one starts:

```bash
curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh | bash
kotlin-lsp --version
```

```powershell
iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex
kotlin-lsp --version
```

To update to an exact version, set `KOTLIN_LSP_VERSION` as shown above.

If you installed from crates.io instead of GitHub Releases:

```bash
cargo install kotlin-lsp --locked --force
```

### Manual install

1. Open the [latest release](https://github.com/qdsfdhvh/kotlin-lsp/releases/latest).
2. Download the asset for your platform:
   - macOS Apple Silicon: `kotlin-lsp-darwin-aarch64.tar.gz`
   - macOS Intel: `kotlin-lsp-darwin-x86_64.tar.gz`
   - Linux arm64: `kotlin-lsp-linux-aarch64.tar.gz`
   - Linux x64: `kotlin-lsp-linux-x86_64.tar.gz`
   - Windows arm64: `kotlin-lsp-windows-aarch64.zip`
   - Windows x64: `kotlin-lsp-windows-x86_64.zip`
3. Extract it.
4. Move the binary onto your `PATH`, for example:

```bash
mkdir -p ~/.local/bin
install -m 0755 kotlin-lsp-darwin-aarch64 ~/.local/bin/kotlin-lsp
kotlin-lsp --version
```

On Windows, move `kotlin-lsp.exe` into a directory on your user `PATH`.

### Troubleshooting

- `kotlin-lsp: command not found`: run `which kotlin-lsp` / `Get-Command kotlin-lsp`, then make sure the install directory is on `PATH`.
- Shell still sees an old version: open a new terminal, or check for another earlier `kotlin-lsp` in `PATH`.
- macOS blocks a browser-downloaded binary: run `xattr -d com.apple.quarantine ~/.local/bin/kotlin-lsp`.
- Need to confirm what changed: compare `kotlin-lsp --version` with the [release page](https://github.com/qdsfdhvh/kotlin-lsp/releases).

**Recommended:** Install `fd` and `rg` (ripgrep) for faster file discovery and cross-file search.

## For AI agents (Claude Code, Cursor, Codex, …)

Once `kotlin-lsp` is on your `PATH`, install the bundled agent skill so your AI
tool knows when and how to call it (saves tokens vs. blind `grep`/`rg`):

```bash
npx skills add https://github.com/qdsfdhvh/kotlin-lsp
```

Re-run the same command after updating `kotlin-lsp` so the agent picks up the
latest CLI guidance. If your agent caches skills at startup, restart it after
installing or updating the skill.

This drops [`skills/kotlin-lsp/SKILL.md`](skills/kotlin-lsp/SKILL.md) into your
project's agent directory. The skill teaches the agent to prefer
`kotlin-lsp find` / `refs` / `hover` over text-grep for Kotlin/Java/Swift, and
how to use the `--module`, `--source-set`, `--owner`, and `--json` filters introduced for
agent workflows.

## Setup

**Editor integration:** configure your LSP client to launch `kotlin-lsp` (no arguments — it speaks LSP over stdio). See [contrib/](contrib/) for example configs (Neovim, Zed, Helix, VS Code).

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
| **Go-to-type-definition** | Resolve `val x: Foo` → `Foo` declaration |
| **Hover** | Signature, visibility, KDoc, deprecated warning, data class props |
| **Completion** | Dot-completion, auto-import, deprecated tag, label_details, scored ranking, stdlib |
| **References** | Project-wide `rg --word-regexp` + open buffers |
| **Document/workspace symbol** | Outline view, fuzzy search, dot-qualified extension function queries |
| **Rename** | Project-wide via `WorkspaceEdit` |
| **Inlay hints** | Configurable: lambda `it`, named params, `this`, untyped `val`/`var` |
| **Semantic tokens** | Full syntax highlighting via tree-sitter CST + cross-file resolution |
| **Diagnostics** | Syntax errors, unused/duplicate imports, deprecation warnings, redundant vals |
| **Folding range** | Brace, import, comment blocks with collapsed text |
| **Selection range** | Smart expand via tree-sitter CST |
| **Go-to-implementation** | Transitive subtype lookup (BFS) |
| **Signature help** | Active parameter highlighting |
| **Call hierarchy** | Incoming (rg) + outgoing (CST walk) |
| **On-type formatting** | Auto de-indent on `}` |
| **Document formatting** | ktfmt / google-java-format / swift-format |
| **Range formatting** | Reuses external formatters; clips edit to requested range |
| **Code actions** | Specify type, add names to args, add import, suppress warning, generate overrides |
| **Organize imports** | Sort, dedup, remove unused — CLI + LSP |
| **CLI mode** | `find`, `refs`, `hover`, `complete`, `index`, `sources`, `extract-sources`, `check`, `context`, `call-hierarchy`, `type-hierarchy`, `organize-imports`, `inject`, `tokens`, `tree`, `cache`, `code-action`, `batch-imports`, `new-file`, `benchmark` — scriptable, no daemon |
| **Batch inject** | Resolve all type signatures in a file at once (`kotlin-lsp inject <file>`)

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
kotlin-lsp index-jars                      # index library symbols from *-sources.jar
kotlin-lsp cache stats                     # show index cache diagnostics
kotlin-lsp benchmark                       # run performance benchmarks
kotlin-lsp find Foo --kind class,fun       # filter by symbol kind
kotlin-lsp refs Refresh --owner ScreenAction  # filter by enclosing type
kotlin-lsp refs ScreenAction.Refresh          # auto-detect owner
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
| `--module <frag>` | Filter results by module path fragment |
| `--owner <name>` | Filter results by enclosing class/interface/object name |
| `--source-set <set>` | Filter by source set (e.g. `commonMain`, comma-separated for OR) |
| `--owner <name>` | Filter results by enclosing class/interface/object name |
| `--limit <n>` | Cap result count after filtering |
| `--root <dir>` | Workspace root (default: nearest `.git` dir) |
