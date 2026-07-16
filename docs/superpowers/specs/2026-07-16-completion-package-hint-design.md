# Completion package hint (`labelDetails.description`)

**Date:** 2026-07-16
**Status:** approved (user: "let's do the correct version always")

## Problem

Bare-name cross-package completion can offer several identically-named
candidates (five `Modifier`s: `androidx.compose.ui`, `com.google.devtools.
ksp.symbol`, …). The only disambiguator today is `CompletionItem.detail`,
and it is inconsistent: an unmaterialized jar candidate shows its package
there, but once the symbol is materialized `jar_symbol_detail` replaces it
with the bare signature (`interface Modifier`) — the package disappears and
the candidates become indistinguishable in the completion list.

## Design

Set `CompletionItem.labelDetails.description = <package qualifier>` on
**every** item built by `add_cross_package_symbol` (src/resolver/complete.rs)
— both the materialized (signature-`detail`) and stub (package-`detail`)
variants. The qualifier is the FQN minus its last segment, the same string
the stub `detail` uses today (for nested classes that is `pkg.Outer`, which
disambiguates even better).

- **Unconditional** — not gated on the client's `labelDetailsSupport`
  capability (user decision). Clients that don't render the field ignore
  it; nothing regresses because `detail` behavior is unchanged.
- `detail` stays exactly as-is: real signature when materialized, package
  qualifier (import-needed stubs only) otherwise. Slight duplication for
  stubs in labelDetails-rendering clients is acceptable and short-lived
  (stubs upgrade to signatures once materialized).
- Scope: the bare-name cross-package path only. Extension and member
  completion already show the package where it matters, and
  `add_cross_package_name_without_imports` has no FQN to show.

`lsp-types` 0.94.1 (tower-lsp 0.20) already carries
`CompletionItem::label_details: Option<CompletionItemLabelDetails>`.

## Testing

Unit tests on `complete_bare` output: identically-named candidates from two
packages each carry their own `labelDetails.description`; a materialized
candidate keeps its signature `detail` alongside the package hint.
