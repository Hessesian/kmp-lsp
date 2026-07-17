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

- **Capability-gated** (revised 2026-07-16 after live feedback — the
  first, unconditional revision was invisible in the user's client):
  Helix renders only label + kind in the completion menu and never reads
  `labelDetails` (helix-term/src/ui/completion.rs,
  `menu::Row::new([label, kind])`); its doc popup renders `detail` +
  documentation. It also does not advertise
  `completionItem.labelDetailsSupport`. So:
  - client advertises `labelDetailsSupport` → set
    `labelDetails.description`, keep `detail` as-is;
  - client does not → **fold the package into `detail`** for materialized
    candidates: `detail = "package <qualifier>\n<signature>"` (reads as a
    Kotlin header line in the doc popup's code fence). Unmaterialized
    stubs already show the package as their whole `detail` — unchanged.
  The flag is detected at `initialize`
  (`completionItem.labelDetailsSupport`) and stored on the `Indexer`
  (`AtomicBool`, default false — the CLI path gets the fold, which its
  `detail` column prints usefully).
- `completionItem/resolve` overwrites `detail` with the enriched
  signature; it must **preserve** a leading `package …` line from the
  incoming item so the fold survives resolution (Helix advertises
  `resolve_support: [detail]` and applies the resolved value).
- **Stub candidates resolve on demand** (added 2026-07-17 after the second
  live report — "package is there but not signature nor docs"): an
  unmaterialized candidate is served as a stub with no location `data`, so
  resolve was a silent no-op and the selected item never gained a
  signature or docs. Stubs now carry their FQN in `data` (`DATA_FQN` =
  `"f"`), and `resolve_completion_item` materializes that ONE candidate
  (unbudgeted, same policy as hover — the user selected it) via a new
  `IndexRead::materialize_completion_candidate` hook, then runs the normal
  doc enrichment on the upgraded location data. This is the LSP-intended
  lazy split: the list-wide pass stays budgeted, the selected item pays
  full price.
- Scope: the bare-name cross-package path only. Extension and member
  completion already show the package where it matters, and
  `add_cross_package_name_without_imports` has no FQN to show.

`lsp-types` 0.94.1 (tower-lsp 0.20) already carries
`CompletionItem::label_details: Option<CompletionItemLabelDetails>`.

## Testing

Unit tests on `complete_bare` output: identically-named candidates from two
packages each carry their own `labelDetails.description` (flag on); a
materialized candidate keeps its signature `detail` alongside the package
hint (flag on); with the flag off (default), the materialized candidate
folds the package into `detail` as `package <qualifier>\n<signature>` and
sends no `labelDetails`; `resolve_completion_item` preserves a leading
`package …` line when replacing `detail` with the enriched signature.
