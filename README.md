# kotlin-lsp

[![crates.io](https://img.shields.io/crates/v/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![downloads](https://img.shields.io/crates/d/kotlin-lsp)](https://crates.io/crates/kotlin-lsp)
[![release](https://img.shields.io/github/v/release/qdsfdhvh/kotlin-lsp)](https://github.com/qdsfdhvh/kotlin-lsp/releases/latest)
[![build](https://img.shields.io/github/actions/workflow/status/qdsfdhvh/kotlin-lsp/ci.yml)](https://github.com/qdsfdhvh/kotlin-lsp/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/kotlin-lsp)](LICENSE)

A fast, no-JVM **symbol engine** for Kotlin, Java, and Swift — with a
scriptable CLI and LSP transport.  
Built with [tree-sitter](https://tree-sitter.github.io/) — instant startup,
low memory, zero external runtime.

![kotlin-lsp CLI demo](demo/cli.gif)

---

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

**Recommended:** Install `fd` and `rg` (ripgrep) for faster file discovery and
cross-file search.

---

## For AI agents

`kotlin-lsp` is designed for AI-agent workflows. Once it's on your `PATH`,
install the bundled agent skill so your AI tool knows when and how to call it:

```bash
npx skills add https://github.com/qdsfdhvh/kotlin-lsp
```

Re-run the same command after updating `kotlin-lsp` so the agent picks up the
latest CLI guidance. If your agent caches skills at startup, restart it after
installing or updating the skill.

The skill teaches the agent to prefer `kotlin-lsp find` / `refs` / `hover`
over text-grep for Kotlin/Java/Swift symbols — saving tokens and returning
structured results. See [`skills/kotlin-lsp/SKILL.md`](skills/kotlin-lsp/SKILL.md)
for the full command reference.

---

## CLI

`kotlin-lsp` works standalone — no editor, no daemon.

**Output is AI-tuned by default:** text mode is minimal (grouped by file,
structural annotation), `--json` emits compact JSON, and `--relative` is
auto-enabled when stdout is piped.

```bash
kotlin-lsp find MyViewModel              # search declarations
kotlin-lsp refs MyViewModel              # find all references
kotlin-lsp hover src/Foo.kt 42 10        # hover info at line 42, col 10
kotlin-lsp complete src/Foo.kt 42 --dot  # completions after last '.' on line 42
kotlin-lsp context src/Foo.kt 42 10      # one-stop: def + sig + doc + refs
kotlin-lsp check src/Foo.kt              # syntax + import + deprecation diagnostics
kotlin-lsp call-hierarchy src/Foo.kt 42 10  # incoming + outgoing call chain
kotlin-lsp type-hierarchy Activity       # super/subtype tree
kotlin-lsp organize-imports src/Foo.kt   # sort, dedup, remove unused
kotlin-lsp inject src/Foo.kt             # batch-resolve all type signatures
kotlin-lsp code-action src/Foo.kt 42 10  # list applicable code actions
kotlin-lsp batch-imports src/Foo.kt      # scan for import candidates
kotlin-lsp new-file activity Activity    # generate file from template
kotlin-lsp index --root ./android        # pre-build cache
kotlin-lsp sources --root ./android      # list detected source roots
kotlin-lsp extract-sources               # unpack library sources from Gradle cache
kotlin-lsp index-jars                    # index library symbols from *-sources.jar
kotlin-lsp cache stats                   # show index cache diagnostics
kotlin-lsp benchmark                     # run performance benchmarks
```

### Flags

| Flag | Behaviour |
|------|-----------|
| _(none)_ | Auto: use cached index if available, fall back to fast `rg`/`fd` |
| `--fast` | Always use `rg`/`fd`; instant, no index needed |
| `--smart` | Require index; build it if missing |
| `--json` | Compact JSON output (no whitespace); pipe to `jq` for human reading |
| `--relative` | Print workspace-relative paths. **Auto-enabled when stdout isn't a TTY** (typical AI agent invocation) |
| `--absolute` | Force absolute paths; opt out of the non-TTY auto-relative default |
| `--flat` | Use legacy grep-style `<path>:<line>:<col>: <name>` format (one full path per line) |
| `--module <frag>` | Filter results by module path fragment |
| `--source-set <set>` | Filter by source set (e.g. `commonMain`, comma-separated for OR) |
| `--owner <name>` | Filter results by enclosing class/interface/object name |
| `--kind class,fun` | Filter by symbol kind |
| `--limit <n>` | Cap result count after filtering |
| `--root <dir>` | Workspace root (default: nearest `.git` dir) |

### Library sources

Library symbols (Compose, AndroidX, coroutines, stdlib, …) are resolved
automatically once you extract them:

```bash
kotlin-lsp extract-sources        # one-time: unpack *-sources.jar from Gradle cache
kotlin-lsp index-jars             # one-time: index library symbols
```

- **Android SDK** (`Activity`, `Context`, `View`, …) — detected from
  `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`
- **Gradle library sources** — extracted from `*-sources.jar` in the Gradle cache
- **IntelliJ/Android Studio projects** — `workspace.json` source roots are picked
  up automatically

---

## Features

### CLI capabilities (primary surface)

| Command | What it does |
|---------|-------------|
| `find` | Declaration search — qualified name, `--owner`, `--kind`, `--module`, `--source-set`, `--limit` |
| `refs` | All references — same filters, plus `--explain` for provenance |
| `hover` | Signature, visibility, KDoc, deprecated warning, data class props |
| `complete` | Dot-completion, auto-import, scored ranking, stdlib |
| `context` | One-stop: definition + signature + doc + reference count, `--expand` for chains |
| `check` | Syntax errors, unused/duplicate imports, deprecation warnings, redundant vals |
| `call-hierarchy` | Incoming (rg) + outgoing (CST walk) call chain |
| `type-hierarchy` | Supertype/subtype tree (BFS) |
| `organize-imports` | Sort, dedup, remove unused — CLI + LSP |
| `inject` | Batch-resolve all type signatures in a file |
| `code-action` | List/apply code actions from the command line |
| `batch-imports` | Scan file for import candidates |
| `new-file` | Generate file from template |
| `cache stats` | Index cache diagnostics |
| `benchmark` | Performance benchmarks |
| `sources` | List auto-discovered source roots, `--explain` for provenance |
| `extract-sources` | Unpack `*-sources.jar` from Gradle cache |
| `index-jars` | Index library symbols from extracted sources |

### LSP capabilities (compatibility transport)

All CLI commands are also available as LSP handlers when the binary is launched
as a language server. Most are also callable directly from the CLI; a few are
visual-only editor affordances that are maintained for compatibility.

| LSP handler | CLI equivalent | Notes |
|-------------|---------------|-------|
| `textDocument/definition` | `kotlin-lsp find` | |
| `textDocument/typeDefinition` | &mdash; | Resolves `val x: Foo` → `Foo` |
| `textDocument/declaration` | `kotlin-lsp find` | Delegates to definition |
| `textDocument/implementation` | `kotlin-lsp type-hierarchy` | Transitive subtype lookup |
| `textDocument/hover` | `kotlin-lsp hover` | |
| `textDocument/completion` | `kotlin-lsp complete` | |
| `textDocument/references` | `kotlin-lsp refs` | |
| `textDocument/documentSymbol` | &mdash; | Outline / workspace symbol |
| `textDocument/codeAction` | `kotlin-lsp code-action` | |
| `textDocument/rename` | &mdash; | Project-wide rename |
| `textDocument/formatting` | &mdash; | ktfmt / google-java-format / swift-format |
| `textDocument/rangeFormatting` | &mdash; | Clips format to requested range |
| `textDocument/callHierarchy` | `kotlin-lsp call-hierarchy` | |
| `textDocument/inlayHint` | &mdash; | Configurable inline type hints |
| `textDocument/signatureHelp` | &mdash; | Editor popup (use `hover` or `context` in CLI) |
| `textDocument/semanticTokens` | `kotlin-lsp tokens` | Syntax highlighting — editor only |
| `textDocument/documentHighlight` | &mdash; | Editor occurrence highlight — editor only |
| `textDocument/foldingRange` | &mdash; | Code folding — editor only |
| `textDocument/selectionRange` | &mdash; | Expression selection — editor only |
| `textDocument/onTypeFormatting` | &mdash; | Auto-indent on `}` — editor only |

### What gets indexed

| Language | Symbols |
|----------|---------|
| **Kotlin** | `class`, `interface`, `object`, `fun`, `val`, `var`, `typealias`, constructor params, enum entries |
| **Java** | `class`, `interface`, `enum`, `method`, `field`, `enum_constant` |
| **Swift** | `class`, `struct`, `enum`, `protocol`, `func`, `let`, `var`, `typealias`, `extension`, `init`, enum cases |

---

## Editor setup

`kotlin-lsp` speaks LSP over stdio. Configure your editor to launch the
`kotlin-lsp` binary (no arguments). See [contrib/](contrib/) for example configs:

| Editor | File |
|--------|------|
| **Neovim** | `contrib/nvim-kotlin-lsp.lua` |
| **Zed** | `contrib/zed-kotlin-lsp.json` |
| **Helix** | `contrib/helix-kotlin-lsp.toml` |
| **VS Code** | (manual `settings.json`) |

Once connected, LSP features work immediately — `rg` fallback handles symbols
while the index builds in the background.
