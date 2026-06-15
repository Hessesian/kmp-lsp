# GitHub Copilot CLI integration

kmp-lsp integrates with the [GitHub Copilot CLI](https://githubnext.com/projects/copilot-cli/) to give Copilot full code-intelligence tools when working on Kotlin/Java/Swift projects.

> **Requires:** `copilot --experimental` (or `--exp`) — the `lsp` tool is only available in experimental mode.

## Setup

**1. Add kmp-lsp to Copilot's LSP config** (`~/.copilot/lsp-config.json`):

```json
{
  "lspServers": {
    "kmp-lsp": {
      "command": "/home/user/.cargo/bin/kmp-lsp",
      "args": [],
      "env": {
        "KMP_LSP_MAX_FILES": "20000"
      },
      "fileExtensions": {
        ".kt": "kotlin",
        ".kts": "kotlin",
        ".java": "java",
        ".swift": "swift"
      }
    }
  }
}
```

> **Note:** `command` must be an **absolute path** — Copilot does not expand `~` or use your shell's `PATH`.  
> The default cargo install location is `~/.cargo/bin/kmp-lsp`; substitute your actual home directory (or run `which kmp-lsp` to confirm).

**2. (Optional) Install the Copilot skill extension** for a richer agent experience — it injects indexing status context automatically and provides `kmp_lsp_status` and `kmp_lsp_set_workspace` tools:

```bash
mkdir -p ~/.copilot/extensions/kmp-lsp
cp contrib/copilot-extension/extension.mjs ~/.copilot/extensions/kmp-lsp/
```

The extension provides:
- **`kmp_lsp_status`** — check indexing phase, file counts, symbol count, and ETA before running queries
- **`kmp_lsp_set_workspace`** — switch the indexed project at runtime without restarting Copilot
- **Auto-injected context** — when you open a session, indexing status and LSP capabilities are injected automatically

## Agentic workflow

Once configured, Copilot can navigate your codebase using:

```
lsp workspaceSymbol "MyClass"         → find any class/function by name (includes signature)
lsp documentSymbol <file>             → list all symbols in a file with line numbers
lsp hover <file> <line> <col>         → get type signature and docs at a position
lsp goToDefinition <file> <line> <col>→ jump to the definition
lsp findReferences <file> <line> <col>→ find all usages across the project
lsp incomingCalls <file> <line> <col> → find all callers of a function
lsp outgoingCalls <file> <line> <col> → find all functions called by a function
```

**Tip:** `workspaceSymbol` results include the declaration signature (e.g. `fun processPayment(amount: BigDecimal, currency: String): Result<Unit>`), so you rarely need a follow-up `hover` call.

The CLI also provides commands that complement LSP navigation:

```bash
kmp-lsp check src/Foo.kt            # instant tree-sitter syntax check — no index needed
kmp-lsp check src/ --json           # check a whole directory, JSON output, exit 1 on errors
kmp-lsp refs Event --exclude-imports # refs without import-statement noise
kmp-lsp diagnose src/Foo.kt --root . # call-arg + syntax diagnostics (needs index)
```

Use `kmp-lsp check` immediately after editing a file to verify syntax before running slower LSP queries. Use `--exclude-imports` on `refs` for common names (`Event`, `Result`, `State`) that would otherwise flood output with import lines.

## Serena MCP integration

[Serena](https://github.com/oraios/serena) is an MCP server that wraps an LSP backend to expose symbol-level tools (`get_symbols_overview`, `find_referencing_symbols`, `replace_symbol_body`, etc.) directly to coding agents via the Model Context Protocol.

When configured with kmp-lsp as its backend, Serena provides fast structural awareness **without requiring an IDE** — kmp-lsp starts instantly and needs no JVM.

### Setup

**1. Install Serena:**

```bash
# Install uv (if not already installed)
curl -LsSf https://astral.sh/uv/install.sh | sh

# Install Serena
uv tool install serena-agent
```

**2. Initialise Serena and create a project:**

```bash
serena init
cd /path/to/your/project
serena project create --language kotlin
```

This creates `.serena/project.yml` in your project root.

**3. Point Serena at kmp-lsp** — edit `.serena/project.yml`:

```yaml
ls_specific_settings:
  kotlin:
    ls_path: "/home/user/.cargo/bin/kmp-lsp"   # absolute path from `which kmp-lsp`
```

> Using kmp-lsp here avoids the JVM startup overhead of JetBrains' `kotlin-language-server`, which can cause MCP timeouts. kmp-lsp is instant.

**4. Wire Serena into the Copilot CLI** — create `.mcp.json` at your repo root:

```json
{
  "mcpServers": {
    "serena": {
      "command": "uvx",
      "args": ["--from", "serena-agent", "serena", "startmcp", "--context", "ide", "--project-path", "/path/to/your/project"]
    }
  }
}
```

> Use `--context ide` for the Copilot CLI (not `jb-copilot-plugin`, which is for JetBrains IDE only).

**5. (Optional) Gitignore local config:**

```bash
echo '.serena/' >> .gitignore
echo '.mcp.json' >> .gitignore
```

### Using both layers together

kmp-lsp (via the `lsp` tool) and Serena MCP tools are complementary — use them in the same task turn:

| Task | Tool |
|---|---|
| List symbols in a file | `serena-get_symbols_overview` |
| Find all callers of a symbol | `serena-find_referencing_symbols` |
| Replace a method/class body | `serena-replace_symbol_body` |
| Find interface implementors (transitive) | `lsp goToImplementation` |
| Cross-file semantic rename | `lsp rename` |
| Locate symbol file + line | `lsp workspaceSymbol` |
| Type signatures and docs | `lsp hover` |
| Verify syntax after an edit | `kmp-lsp check <file>` (instant, no index) |
| Refs without import noise | `kmp-lsp refs <Name> --exclude-imports` |
| Deep diagnostics (call-arg + syntax) | `kmp-lsp diagnose <file> --root .` |

**Rule of thumb:** Serena for structural orientation and body edits; kmp-lsp LSP for type-safe navigation and renaming; kmp-lsp CLI for fast syntax checks and filtered reference queries.

### Limitations

- Serena exposes kmp-lsp's structural layer only — type inference is not available through Serena's MCP tools; for diagnostics use `kmp-lsp check` (syntax, instant) or `kmp-lsp diagnose` (call-arg + syntax, needs index)
- `replace_symbol_body` is reliable for focused method/class swaps; fall back to direct `edit` tool for large structural rewrites
- `lsp rename` and `lsp goToImplementation` require a completed index — call `kmp_lsp_status` before using them if the session is cold

---

## Workspace root

By default, kmp-lsp uses the LSP client's `rootUri` — which is your current working directory. This means switching between projects works automatically.

If you need to override, set the `KMP_LSP_WORKSPACE_ROOT` env var, or write a path to `~/.config/kmp-lsp/workspace`.
