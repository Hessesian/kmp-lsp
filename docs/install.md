# Install

## cargo

```bash
cargo install kmp-lsp
```

> No Cargo? Get it at [rustup.rs](https://rustup.rs). After install, `kmp-lsp` is at `~/.cargo/bin/` — make sure it's on your `PATH`.

**Optional:** Install `fd` and `rg` (ripgrep) for faster file discovery and cross-file search.

## One-liner (Linux / macOS)

Installs both `kmp-lsp` and the native JAR indexer sidecar:

```bash
curl -fsSL https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.sh | bash
```

## Windows (PowerShell)

Installs both `kmp-lsp.exe` and `kmp-jar-indexer.exe`:

```powershell
iwr -useb https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.ps1 | iex
```

## cargo-binstall

Downloads the pre-built binary — no compilation:

```bash
cargo binstall kmp-lsp
```

## mise

Via the aqua backend:

```bash
mise use -g aqua:Hessesian/kmp-lsp
```

## mason.nvim (Neovim)

Once listed in the registry:

```lua
require("mason").setup()
require("mason-lspconfig").setup({ ensure_installed = { "kotlin_ls" } })
```

## JAR indexer sidecar

For full JAR/library type information (Compose, AndroidX, Kotlin stdlib docs), the native sidecar is needed. The `install.sh` and mise/aqua channels install both binaries automatically. cargo-binstall and mason.nvim install only `kmp-lsp` — in those cases, download the matching tarball manually to get the sidecar too:

```bash
# Linux x86_64 example — both binaries extracted from one tarball
tar -xzf kmp-lsp-linux-x86_64.tar.gz
mv kmp-lsp ~/.cargo/bin/
mv kmp-jar-indexer ~/.cargo/bin/
```

The sidecar is a self-contained native binary — **no JVM required**. Starts in ~4 ms.

> **Fallback**: if the native sidecar is absent but `java` is on your PATH, `kmp-lsp` automatically falls back to the JAR version.

## Next steps

- [Wire up your editor](editors.md)
- [Integrate with an AI coding agent](copilot.md)
