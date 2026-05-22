# Editor setup

`kotlin-lsp` is at `~/.cargo/bin/kotlin-lsp` after `cargo install`. Run `which kotlin-lsp` to confirm it's on your `PATH`.

## VS Code

![VS Code with kotlin-lsp](../demo/vscode.png)

Download the `.vsix` for your platform from the [latest release](https://github.com/Hessesian/kotlin-lsp/releases/latest) and install it:

```bash
# Linux x86_64
code --install-extension kotlin-lsp-linux-x64-vX.Y.Z.vsix

# macOS Apple Silicon
code --install-extension kotlin-lsp-darwin-arm64-vX.Y.Z.vsix

# macOS Intel
code --install-extension kotlin-lsp-darwin-x64-vX.Y.Z.vsix
```

Or install the universal `.vsix` (no bundled binary — `kotlin-lsp` must be on your `PATH`):

```bash
code --install-extension kotlin-lsp-vX.Y.Z.vsix
```

The extension activates automatically for `.kt`, `.java`, and `.swift` files — no other Kotlin plugins needed.

> **Tip:** Disable other Kotlin extensions (`fwcd.kotlin`, `jetbrains.kotlin`) to avoid conflicts.

**Configuration** (optional) — in `.vscode/settings.json`:

```json
{
  "kotlinLsp.path": "/path/to/kotlin-lsp"
}
```

Default: `kotlin-lsp` on `$PATH`.

**Install from source** (if you prefer to build locally):

```bash
cd contrib/vscode && npm install
ln -s "$(pwd)/contrib/vscode" ~/.vscode/extensions/kotlin-lsp.kotlin-lsp-client-0.0.1
```

## Helix

Add to `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "kotlin"
language-servers = ["kotlin-lsp"]
auto-format = false

[[language]]
name = "java"
language-servers = ["kotlin-lsp"]
auto-format = false

[[language]]
name = "swift"
language-servers = ["kotlin-lsp"]
auto-format = false

[language-server.kotlin-lsp]
command = "kotlin-lsp"
```

Restart Helix (or run `:lsp-restart`). Check the server is running: `:lsp-workspace-command` or watch `:log-open`.

## Neovim (nvim-lspconfig)

```lua
local lspconfig = require('lspconfig')
local configs   = require('lspconfig.configs')

if not configs.kotlin_lsp then
  configs.kotlin_lsp = {
    default_config = {
      cmd       = { 'kotlin-lsp' },
      filetypes = { 'kotlin', 'java', 'swift' },
      root_dir  = lspconfig.util.root_pattern(
        'build.gradle', 'build.gradle.kts', 'pom.xml', 'settings.gradle', 'Package.swift', '.git'
      ),
      settings  = {},
    },
  }
end

lspconfig.kotlin_lsp.setup {}
```

Place this in your `init.lua` (or a dedicated `after/ftplugin/kotlin.lua`).

**Completion** — pair with [nvim-cmp](https://github.com/hrsh7th/nvim-cmp):

```lua
require('cmp').setup {
  sources = {
    { name = 'nvim_lsp' },
    -- other sources …
  },
}
```

## Zed

### Recommended: install the extension

The `contrib/zed-extension` bundled in this repo registers `kotlin-lsp` as a
first-class Zed language server, resolving the binary from `$PATH`. This is
the preferred setup — no manual `binary.path` wiring required.

**Install the binary first:**
```bash
cargo install kotlin-lsp
```

**Install the extension:**
```bash
# From the repo root
zed --install-dev-extension contrib/zed-extension
```

Or copy the directory manually and restart Zed:
```bash
cp -r contrib/zed-extension ~/.config/zed/extensions/kotlin-lsp
```

**Recommended `~/.config/zed/settings.json`** (suppresses the default JVM server and enables signature help):

```json
{
  "languages": {
    "Kotlin": {
      "language_servers": ["kotlin-lsp", "!kotlin-language-server"],
      "format_on_save": "off",
      "show_completions_on_input": true,
      "show_completion_documentation": true
    },
    "Java": {
      "language_servers": ["kotlin-lsp"],
      "format_on_save": "off",
      "show_completions_on_input": true
    },
    "Swift": {
      "language_servers": ["kotlin-lsp"],
      "format_on_save": "off"
    }
  }
}
```

> **Signature help** appears automatically when you type `(` or `,` inside a call.
> It updates the active parameter as you add named arguments (`param = value, `).
> If it stops showing, check that `kotlin-language-server` (the JVM server) is not
> also active — it conflicts and the last responder wins.

### Without the extension (manual wiring)

If you prefer not to install the extension, add the full LSP config to
`~/.config/zed/settings.json`:

```json
{
  "languages": {
    "Kotlin": {
      "language_servers": ["kotlin-lsp"],
      "format_on_save": "off",
      "show_completions_on_input": true,
      "show_completion_documentation": true
    },
    "Java": {
      "language_servers": ["kotlin-lsp"],
      "format_on_save": "off"
    },
    "Swift": {
      "language_servers": ["kotlin-lsp"],
      "format_on_save": "off"
    }
  },
  "lsp": {
    "kotlin-lsp": {
      "binary": { "path": "kotlin-lsp", "arguments": ["--stdio"] }
    }
  }
}
```

> **Note:** Zed requires a full restart (not just workspace reload) after changing
> LSP settings. Check **Zed → Help → Open Log** if the server doesn't start.
