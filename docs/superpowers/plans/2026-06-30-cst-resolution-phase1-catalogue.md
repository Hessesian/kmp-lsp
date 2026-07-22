# CST Resolution Unification — Phase 1: Catalogue + absorb semantic_tokens expr walk

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Stand up the `CstResolve` catalogue facade over `InferDeps`, give it a complete `expr_type`
that covers all expression node kinds, route `semantic_tokens` through it, and **delete** its bespoke
`expression_type` walk — a deletion-bearing first slice.

**Architecture:** One catalogue trait (`CstResolve`) implemented for `Indexer` (an `InferDeps`), exposing
CST type resolution. Phase 1 unifies the *expression-type* capability: today it is split between
`indexer/infer/expr_type.rs::infer_expr_type` (literals/call/if/range) and
`semantic_tokens/resolve.rs::expression_type` (ident/nav/call). Phase 1 moves the ident/nav dispatch into
the catalogue so one `expr_type` covers every kind, then routes `semantic_tokens` to it and deletes the
duplicate.

**Tech Stack:** Rust, tree-sitter (`tree_sitter::Node`), `tower_lsp`, binary-only crate (`kmp-lsp`).

## Global Constraints

- Binary-only crate — tests run with `cargo test --bin kmp-lsp` (`--lib` runs 0 tests).
- No `unwrap()`/`expect()` in production code (`#[cfg(test)]` may).
- Tests in companion `*_tests.rs` files; never inline `mod tests {}`.
- Minimal visibility: `pub(crate)` only when crossing modules; `#[warn(unreachable_pub)]` is on.
- No abbreviations in names (`index` not `idx`, `symbol` not `sym`).
- Pre-commit runs `cargo fmt` + `cargo clippy`; if fmt rewrites, `git add -A` and re-commit.
- **Move-don't-rewrite:** copy bodies verbatim, adjust only what the new home requires, delete the
  original. Do not re-derive logic.
- **Each slice deletes its dead functions.** A slice is incomplete while `find_referencing_symbols` shows
  any remaining caller of a function it replaced. The PR diff must show deletions.
- Branch: `refactor/cst-resolution` (already created off `refactor/unified-resolution`).
- **Return-type decision (user, 2026-06-30): rich surface from the start.** Phase 1 defines
  `Resolution<T>` + `ResolvedType` + `Fqn` and `expr_type` returns `Resolution<ResolvedType>`. It wraps
  the existing `Option<String>` functions at the boundary, so it stays **behaviour-preserving**:
  `ResolvedType` carries the inferred type *as-written* (no lossy normalization in Phase 1) and exposes
  it via `as_type_str()`. `expr_type` emits only `Resolved`/`Unresolved` in Phase 1 (`Ambiguous` is part
  of the enum for later methods). The `RawTypeName`/`TypeName` normalized split, the `CstExpr` exhaustive
  dispatch, and construction-sealing remain slice 5.

---

## File structure

- `src/indexer/infer/mod.rs` — **the catalogue.** Gains `CstCtx` (input struct) + the `CstResolve` trait.
  Zero logic beyond the trait's `impl` delegating to submodule functions. (Currently only module decls +
  doc comments.)
- `src/indexer/infer/expr_type.rs` — gains the ident/nav/call dispatch moved out of `semantic_tokens`,
  so `infer_expr_type` (or a new `infer_expr_type_full`) covers every expression kind. Becomes the single
  implementation the catalogue's `expr_type` calls.
- `src/semantic_tokens/resolve.rs` — **delete** `expression_type`, `identifier_type`,
  `navigation_expression_type`, `call_expression_type`; call `CstResolve::expr_type` instead.
- `src/indexer/infer/mod_tests.rs` (new) — `TestDeps`-driven unit tests for the catalogue.
- `src/semantic_tokens_tests.rs` — existing suite is the behaviour net (must stay green).

---

## Task 1: Move ident/nav/call expr dispatch into `expr_type.rs`; cover all kinds

**Files:**
- Modify: `src/indexer/infer/expr_type.rs` (extend `infer_expr_type` dispatch to ident/nav/call)
- Read first (verbatim move source): `src/semantic_tokens/resolve.rs:291-356`
  (`identifier_type`, `navigation_expression_type`, `call_expression_type`)
- Test: `src/indexer/infer/expr_type_tests.rs` (existing companion if present, else create)

**Interfaces:**
- Consumes: `infer_expr_type(node: Node, bytes: &[u8], deps: &impl InferDeps, uri: &Url) -> Option<String>`
  (existing, `expr_type.rs:45`); `InferDeps` methods (`find_var_type`, `find_field_type`,
  `find_fun_return_type`, …); `infer_lambda_param_type_at` (`indexer/scope.rs:211`);
  `find_field_type_in_class` / `infer_variable_type` (`resolver/infer.rs`).
- Produces: `infer_expr_type` now also handles `KIND_SIMPLE_IDENT`/`KIND_TYPE_IDENT`, `KIND_NAV_EXPR`,
  `KIND_THIS_EXPR` — same `Option<String>` signature. This is the single full expression-type resolver.

- [ ] **Step 1: Read the three source functions in full** (`semantic_tokens/resolve.rs:291-356`) and the
  current `infer_expr_type` (`expr_type.rs:41-72`). Confirm the node kinds each handles; the union must be
  literals + call + if/range + prefix/comparison (have) ∪ ident + nav + this (to add).

- [ ] **Step 2: Write a failing test** for the gap (ident + nav through the *infer* entry).

```rust
// src/indexer/infer/expr_type_tests.rs
#[test]
fn infer_expr_type_resolves_navigation_chain_receiver() {
    // `data.field` where `data: Holder` and `Holder.field: Foo` → "Foo"
    let deps = TestDeps::new()
        .with_var("file:///A.kt", "data", "Holder")
        .with_field("file:///A.kt", "Holder", "field", "Foo");
    // build a NAV_EXPR node for `data.field` via the test parse helper, then:
    let ty = infer_expr_type(nav_node, source.as_bytes(), &deps, &uri);
    assert_eq!(ty.as_deref(), Some("Foo"));
}
```

- [ ] **Step 3: Run it, verify it fails** (ident/nav not yet handled by `infer_expr_type`).

Run: `cargo test --bin kmp-lsp infer_expr_type_resolves_navigation_chain_receiver`
Expected: FAIL (`None`, arm falls through `_ => None`).

- [ ] **Step 4: Move the dispatch.** Add match arms to `infer_expr_type` for `KIND_SIMPLE_IDENT |
  KIND_TYPE_IDENT`, `KIND_NAV_EXPR`, `KIND_THIS_EXPR`, copying the bodies of `identifier_type` /
  `navigation_expression_type` / `call_expression_type` **verbatim**, adapting parameters: they took
  `(node, doc, starts, indexer, uri)`; here use `(node, bytes, deps, uri)`. Where they called
  `expression_type(receiver, …)` recursively, call `infer_expr_type(receiver, bytes, deps, uri)`. Where
  they used `indexer.infer_lambda_param_type_at(name, uri, pos)`, keep that call (the `deps`/`Indexer`
  provides it — see Step 5 note). Add private helpers in `expr_type.rs` as needed (don't inline a large
  match arm — extract `identifier_type`/`navigation_expression_type`/`call_expression_type` as private
  fns *in this file*).

- [ ] **Step 5: Reconcile the `infer_lambda_param_type_at` dependency.** It is an `Indexer` method
  (`scope.rs:211`), not on `InferDeps`. If `infer_expr_type`'s new arms need it, add it to the `InferDeps`
  trait (default `None`, real impl on `Indexer`) so the move stays within the `deps` seam. Confirm
  `TestDeps` still compiles (default returns `None`).

- [ ] **Step 6: Run the new test + full suite.**

Run: `cargo test --bin kmp-lsp`
Expected: new test PASS; baseline (1426+) all green.

- [ ] **Step 7: Commit.**

```bash
git add -A
git commit -m "refactor(infer): make infer_expr_type cover ident/nav/this kinds"
```

---

## Task 2: Introduce the `CstResolve` catalogue + `CstCtx` + rich return types in `mod.rs`

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add `CstCtx`, `Resolution<T>`, `ResolvedType`, `Fqn`, the
  `CstResolve` trait + `impl CstResolve for Indexer`)
- Test: `src/indexer/infer/mod_tests.rs` (create)

**Interfaces:**
- Consumes: `infer_expr_type` (now full, from Task 1).
- Produces:
  ```rust
  /// Per-request CST resolution input: document bytes + URI + IO policy.
  pub(crate) struct CstCtx<'a> { pub bytes: &'a [u8], pub uri: &'a Url, pub io: ResolveIo }

  /// Importable fully-qualified name (newtype over the FQN string).
  pub(crate) struct Fqn(pub String);

  /// Outcome of resolving something to `T`. Reused across the catalogue so an agent
  /// learns the three outcomes once and reads them off every signature.
  pub(crate) enum Resolution<T> { Resolved(T), Ambiguous(Vec<Fqn>), Unresolved }

  /// A resolved expression type. Phase 1: carries the inferred type *as-written*
  /// (no lossy normalization); the RawTypeName/TypeName split is slice 5.
  pub(crate) struct ResolvedType { type_name: String, nullable: bool }
  impl ResolvedType {
      /// Construct from an inferred type string (nullability = trailing `?`).
      fn from_inferred(raw: String) -> Self { /* nullable = raw.is_nullable() (StrExt) */ }
      /// The type as-written (what the old Option<String> callers consumed).
      pub fn as_type_str(&self) -> &str { &self.type_name }
      pub fn is_nullable(&self) -> bool { self.nullable }
  }
  impl<T> Resolution<T> {
      /// `Resolved(t) -> Some(t)`, else `None`. Bridges callers not yet ambiguity-aware.
      pub fn resolved(self) -> Option<T> { /* match */ }
      pub fn resolved_ref(&self) -> Option<&T> { /* match */ }
  }

  /// The catalogue of CST-driven type resolution. (Phase 1: expr_type only.)
  pub(crate) trait CstResolve {
      /// Type of any expression node. Covers ident/nav/call/literals/this/if/range.
      fn expr_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ResolvedType>;
  }
  ```
  (`io` is carried now though Phase 1 does not branch on it — wiring the seam; later slices add methods.
  `Ambiguous` is unused by `expr_type` in Phase 1 but the enum is complete for later methods.)

- [ ] **Step 1: Write a failing catalogue test** (via the real `Indexer`, smallest path).

```rust
// src/indexer/infer/mod_tests.rs
#[test]
fn cst_resolve_expr_type_resolves_int_literal() {
    let index = Indexer::new();
    let uri = uri("/A.kt");
    let source = "val n = 1\n";
    index.index_content(&uri, source);
    // integer-literal node for `1` (build via the existing expr_type test parse helper):
    let ctx = CstCtx { bytes: source.as_bytes(), uri: &uri, io: ResolveIo::IndexOnly };
    let resolved = index.expr_type(int_literal_node, &ctx).resolved();
    assert_eq!(resolved.map(|t| t.as_type_str().to_owned()).as_deref(), Some("Int"));
}
```

- [ ] **Step 2: Run it, verify it fails** (trait/types not defined).

Run: `cargo test --bin kmp-lsp cst_resolve_expr_type_resolves_int_literal`
Expected: FAIL (does not compile / method missing).

- [ ] **Step 3: Define the types + trait in `mod.rs`**, with `impl CstResolve for Indexer` wrapping the
  existing `Option<String>` function at the boundary:

```rust
fn expr_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ResolvedType> {
    match crate::indexer::infer::expr_type::infer_expr_type(node, ctx.bytes, self, ctx.uri) {
        Some(raw) => Resolution::Resolved(ResolvedType::from_inferred(raw)),
        None => Resolution::Unresolved,
    }
}
```
Re-export `CstCtx`, `CstResolve`, `Resolution`, `ResolvedType`, `Fqn` from `mod.rs` per the "mod.rs is the
catalogue" rule. Keep zero logic in the impl beyond the wrap. Use `StrExt::is_nullable` for the nullable
flag (do not re-derive `ends_with('?')`).

- [ ] **Step 4: Run the test + full suite.**

Run: `cargo test --bin kmp-lsp`
Expected: new test PASS; suite green.

- [ ] **Step 5: Commit.**

```bash
git add -A
git commit -m "refactor(infer): add CstResolve catalogue facade with expr_type"
```

---

## Task 3: Route `semantic_tokens` through `CstResolve`; DELETE the bespoke walk

**Files:**
- Modify: `src/semantic_tokens/resolve.rs` (callers at `:161`, `:322`, `:345`; delete `:291/313/335/358`)
- Test: `src/semantic_tokens_tests.rs` (existing net)

**Interfaces:**
- Consumes: `CstResolve::expr_type` + `CstCtx` (Task 2).
- Produces: `semantic_tokens/resolve.rs` no longer defines `expression_type`/`identifier_type`/
  `navigation_expression_type`/`call_expression_type`; no `use crate::resolver::infer::{...}` leakage for
  expression typing.

- [ ] **Step 1: Enumerate callers.** Use `find_referencing_symbols` on `expression_type`
  (`semantic_tokens/resolve.rs`): expect the recursive callers (`navigation_expression_type:322`,
  `call_expression_type:345`) and `walk_kotlin_references:161`. Confirm no callers outside this file.

- [ ] **Step 2: Replace the call sites.** At each caller, build a `CstCtx { bytes: &doc.bytes, uri,
  io: ResolveIo::IndexOnly }` (semantic tokens is a hot path → `IndexOnly`) and call
  `indexer.expr_type(node, &ctx)` in place of `expression_type(node, doc, starts, indexer, uri)`. The old
  callers consumed `Option<String>`; map the new return with
  `.resolved().map(|t| t.as_type_str().to_owned())` (or `.resolved_ref()` where a borrow suffices) so the
  surrounding logic is unchanged. The `starts` argument is no longer needed for these calls.

- [ ] **Step 3: DELETE** `expression_type` (358), `identifier_type` (291), `navigation_expression_type`
  (313), `call_expression_type` (335) from `semantic_tokens/resolve.rs`. Remove now-unused imports
  (`find_field_type_in_class`, `infer_variable_type` if unused; `member_return_type` if unused).

- [ ] **Step 4: Verify zero remaining references.**

Run: `grep -rn "expression_type\|identifier_type\|navigation_expression_type\|call_expression_type" src/semantic_tokens/`
Expected: no matches (definitions gone, callers routed).

- [ ] **Step 5: Run the semantic-tokens suite + full suite.**

Run: `cargo test --bin kmp-lsp -- semantic` then `cargo test --bin kmp-lsp`
Expected: all green (behaviour preserved; the walk now goes through the catalogue).

- [ ] **Step 6: Diff highlighting output (spot check).** Pick one semantic-tokens test fixture; confirm
  the token stream is unchanged vs `refactor/unified-resolution` (behaviour-preserving deletion).

- [ ] **Step 7: Commit.**

```bash
git add -A
git commit -m "refactor(semantic-tokens): route expression typing through CstResolve; delete bespoke walk"
```

---

## Phase 1 acceptance

- `CstResolve` + `CstCtx` exist in `infer/mod.rs`; `expr_type` covers every expression kind.
- `semantic_tokens` no longer has its own expression walk; the four functions are deleted; PR diff is
  net-negative in `semantic_tokens/resolve.rs`.
- `cargo test --bin kmp-lsp` green; `cargo clippy` clean; highlighting output unchanged.

---

## Roadmap — subsequent slices (each its own plan, written just-in-time)

Each is a PR that **deletes** the functions it replaces (`find_referencing_symbols` = 0 callers before
close). Detailed bite-sized steps are written when the slice is reached, because each depends on the
shapes the previous slice lands.

- **Slice 2 — route the remaining CST consumers** (`completion_context`, `inlay_hints`,
  `nullable_call_diagnostics`, hover, signature_help, `fill_when`) onto `CstResolve`; add `receiver_type`
  + `lambda_scope` catalogue methods (delegating for now); delete each consumer's hand-rolled entry use.
- **Slice 3 — collapse the lambda triad.** `lambda_scope` becomes the one CST classifier; fold
  `receiver.rs` text variants + `it_this.rs` line variants in; **re-home heuristics** (scope-function
  receiver classification → `ThisLambdaCtx::Receiver`) onto CST inputs; delete the text/line-scanning
  mechanisms. Promote + extend the existing `LambdaScope` (`completion_context.rs:16`).
- **Slice 4 — collapse the chain walk.** `chain.rs` becomes the chain step of `expr_type`; delete the
  echoed chain logic in `receiver.rs`.
- **Slice 5 — type-driven hardening / sweep.** Introduce `Resolution<T>` (generalising `SignatureResult`),
  `ResolvedType`/`TypeName`/`RawTypeName`/`Fqn`, the `CstExpr` exhaustive dispatch, construction-sealed
  outputs; migrate the catalogue surface off `Option<String>`. Remove dead helpers/constants.
- **Slice 6 — CST-aware navigation family** (go-to-def, goto-impl, find-refs, document-highlight, rename):
  generalise the `CursorContext` → CST bridge through `CstResolve`; string + rg fallback preserved.

See `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md` for the full design,
reuse inventory, and the governing principle (CST is the source of structure; string parsing is a
mistake; keep only genuine heuristics).
