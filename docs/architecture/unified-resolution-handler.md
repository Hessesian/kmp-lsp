# Unified Resolution Handler — IO Policy

Status: **proposed** (design before code). Scope: `src/resolver/resolve.rs`.

## Problem

`resolve_symbol`'s resolution chain (local → local-decl → imports → same-package →
star → hierarchy → rg → global-defs) exists **three times**, hand-cloned. The clones
differ from each other *only by IO policy* — which subprocesses (`rg`, `fd`) and which
fallbacks are allowed — yet each clone re-derives the entire chain:

| chain step                     | `resolve_symbol_inner` (Full) | `resolve_symbol_no_rg` (NoRg) | `resolve_type_index_only_simple` (IndexOnly) |
|--------------------------------|:--:|:--:|:--:|
| cold-file on-demand index (fs) | ✓ | ✗ | ✗ |
| local-decl line scan           | ✓ | ✗ | ✗ |
| import resolution              | + `fd` | + `fd` | **index-only (stale clone)** |
| Swift fast path                | ✓ | ✗ | ✗ |
| same-package                   | ✓ | ✓ | ✓ |
| star imports                   | + `rg` in pkg dir | index-only | index-only |
| class hierarchy walk           | ✓ | ✗ | ✗ |
| project-wide `rg`              | ✓ | ✗ | ✗ |
| global-defs tail fallback      | none | first match | unique-only |

This is the "duplicates are the bug factory" thesis in the resolution plan, made
concrete. Two costs:

1. **Divergence bug (latent).** `resolve_via_imports_index_only` (resolve.rs:385) is not
   "`resolve_via_imports` minus `fd`" — it is a *stale, simpler clone*. The IO import path
   (resolve.rs:846) disambiguates nested sealed classes via `import_container_chain`
   (`expected_chain`); the index-only clone still uses the older `extract_container_from_import`
   and so mis-disambiguates `Contract.State.Idle` vs `Contract.Event.Idle`. The `IndexOnly`
   path (used by `fill_when`) silently resolves to the wrong nested member.
2. **Maintenance tax.** Every resolution fix must be applied in up to three places or it
   diverges further (this is how #1 happened).

## Design

One private chain parameterized by an IO policy; the three public entry points become
thin wrappers so **no caller changes**.

```rust
/// Which IO and fallbacks a resolution pass may use. The plan's "IoPolicy".
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveIo {
    /// Navigation (go-to-def, hover): may spawn `fd`/`rg`, walk the class
    /// hierarchy, and index a cold file on demand. No global-defs tail fallback.
    Full,
    /// Index-only, but imports may still `fd`. No `rg`, no hierarchy, no cold
    /// index. Tail fallback: first global-defs match. (completion/highlight hot path)
    NoRg,
    /// Strictly in-memory: no `fd`, no `rg`, no hierarchy. Tail fallback:
    /// unique global-defs match only (ambiguity-safe). (diagnostics keystroke path)
    IndexOnly,
}
```

The policy collapses to four behavioural knobs read inside the one chain:

| `ResolveIo` | cold-index + local-decl + swift + hierarchy + rg | imports `fd` | star `rg` | tail fallback |
|-------------|:--:|:--:|:--:|:--|
| `Full`      | ✓ | ✓ | ✓ | none |
| `NoRg`      | ✗ | ✓ | ✗ | first |
| `IndexOnly` | ✗ | ✗ | ✗ | unique-only |

`import resolution` becomes a single `resolve_via_imports(indexer, name, uri, allow_fd:
bool)` — the rich version (with `import_container_chain` disambiguation) gated by one
bool; the stale `resolve_via_imports_index_only` is deleted (this is the bug fix in #1).

The dotted-name pre-handler (`Outer.Nested`) also differs today (Full walks deep nesting;
IndexOnly does a single split). Folded into the unified entry: Full keeps the
multi-segment walk; NoRg/IndexOnly keep the single split. Same code, policy-keyed.

### Public surface (unchanged signatures, now wrappers)

```rust
pub(crate) fn resolve_symbol(idx, name, qualifier, from_uri)      // → chain(Full)   (+ qualifier/dotted pre-pass)
pub(crate) fn resolve_symbol_no_rg(idx, name, from_uri)           // → chain(NoRg)
pub(crate) fn resolve_type_index_only(idx, name, from_uri)        // → chain(IndexOnly) (+ dotted pre-pass)
```

The 18 `resolve_symbol_no_rg` callers and the 1 `resolve_type_index_only` caller
(`fill_when`) are untouched — they keep calling the same-named wrapper.

## Deletion accounting

| delete | ~lines |
|---|--:|
| `resolve_symbol_no_rg` body                | 38 |
| `resolve_type_index_only_simple` body      | 38 |
| `resolve_via_imports_index_only`           | 58 |
| **deleted total**                          | **134** |

| add | ~lines |
|---|--:|
| `ResolveIo` enum + doc                      | 18 |
| policy branch points in the unified chain   | ~25 |
| `allow_fd` param threading in imports       | ~12 |
| **added total**                             | **~55** |

**Net ≈ −80**, plus the latent nested-class divergence bug closed.

## Verification

- `cargo test --bin kmp-lsp` green (binary-only crate; `--lib` runs 0 tests).
  Focused loops: `-- nullable_call`, `-- resolution`, `-- fill_when`.
- Behaviour-preserving for `Full`/`NoRg`: the existing resolver + completion + hierarchy
  suites are the net (every caller routes through the unchanged wrapper).
- `IndexOnly` *changes* on purpose: add a RED test with nested sealed-class decoys
  (`Contract.State.Idle` vs `Contract.Event.Idle`) proving the index-only import path now
  disambiguates correctly. This is the only intended behaviour change.

## Risk

Hottest path in the server; touched by ~18 callers. Mitigation: signatures of the three
public fns are preserved, so the blast radius is contained to `resolve.rs` internals; the
only intended behaviour delta is the `IndexOnly` import disambiguation, which is covered
by a new RED test rather than left implicit.
