# Zed Extension for kmp-lsp

A lightweight Zed extension that wires `kmp-lsp` (tree-sitter, no JVM) as the
language server for Kotlin, Java, Swift, and Android layout XML files.

## Prerequisites

Install the binary from the repo (ViewBinding navigation is on
`feature/viewbinding-navigation` and is not yet on crates.io):

```sh
# From the repo root, on feature/viewbinding-navigation
cargo install --path . --force
```

After installing or upgrading the binary, **fully restart Zed** (not just a
workspace reload) so the LSP process picks up the new build.

For **XML → Kotlin** ViewBinding navigation (references on `@+id/field`,
implementation on `<TextView>`), also install Zed's XML extension so `.xml`
files are recognized as the `XML` language:

- Extensions panel → search **XML**, or run `zed://extension/xml` in the
  command palette.

## Installation (local dev)

```sh
# From the repo root
zed --install-dev-extension contrib/zed-extension
```

Or copy the directory to `~/.config/zed/extensions/kmp-lsp/` and restart Zed.

Re-run `zed --install-dev-extension contrib/zed-extension` after pulling
extension changes (for example when `extension.toml` gains the `XML` language).

## Zed settings

Add to `~/.config/zed/settings.json` to suppress the default JVM-based server
and enable ViewBinding navigation in both directions:

```json
{
  "languages": {
    "Kotlin": {
      "language_servers": ["kmp-lsp", "!kotlin-language-server"],
      "format_on_save": "off"
    },
    "Java": {
      "language_servers": ["kmp-lsp"],
      "format_on_save": "off"
    },
    "XML": {
      "language_servers": ["kmp-lsp", "..."],
      "format_on_save": "off"
    }
  },
  "lsp": {
    "kmp-lsp": {
      "initialization_options": {
        "indexingOptions": {
          "sourcePaths": []
        }
      }
    }
  }
}
```

### ViewBinding navigation

| Direction | What works | Requires |
| --- | --- | --- |
| Kotlin → XML | definition on `binding.field` / `FooBarBinding`; hover; references | Kotlin/Java settings above |
| XML → Kotlin | references on `@+id/field`; implementation on layout tags | XML extension + `XML` language server entry above |

**Build once:** hover, implementation on `FooBarBinding`, and field types need
generated `*Binding.java` under `build/`. Without a build you still get
layout navigation and a build-required diagnostic on the import.

**No watcher settings:** kmp-lsp does not use dynamic LSP file-watcher
registration. Layout XML freshness comes from Zed's native
`workspace/didChangeWatchedFiles`; generated bindings under gitignored `build/`
are polled server-side.

## Why this exists

Zed only starts language servers registered by an extension. The community Kotlin
extension always downloads from JetBrains TeamCity and ignores `binary.path`
overrides. This extension registers `kmp-lsp` as a first-class server name,
resolving the binary from `$PATH` — no symlinks required.
