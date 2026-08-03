# CST Resolution Catalogue — Lambda `this`/receiver Walk Consolidation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close a real, verified duplicate-walk gap between `src/indexer/infer/cst_lambda.rs` and
the `CstQuery` catalogue (`src/indexer/infer/mod.rs`), continuing design doc step 3 ("collapse the
lambda triad") without repeating the two prior reverts' root cause (capability added with no real,
same-task consumer).

**Two prior reverts — do not repeat the pattern:**
1. `CstQuery::receiver_type()`/`call_return_type()` were added with unit tests only; no real
   consumer was ever wired in the same or a following task; both were deleted.
2. `Resolution::Ambiguous(Vec<Fqn>)`/`Fqn`/`resolved_ref()`/`CstQuery::at()`/`CstQuery.io` were
   shipped `#[allow(dead_code)]` behind "wiring seam for later" comments with zero real callers;
   all were deleted (PR #247).

Every task below adds at most one small piece of new surface, and in the **same task** deletes the
old code path it replaces and re-verifies the real production call site(s) still pass. Where a real,
already-existing production call site could not be found for a capability, it is not planned here —
see "Explicitly descoped" at the end.

**Architecture:** No new walk invented. `src/indexer/infer/cst_lambda.rs` currently contains **two
separate, byte-for-byte-shaped ancestor-walk loops** that both do "walk `lambda_literal` ancestors
from a node, classify each via `lambda_this_ctx`":

- `all_this_receivers_at` (`cst_lambda.rs:510-534`) — collects every `Resolved` receiver.
- `cst_this_context` (`cst_lambda.rs:638-657`) — stops at the first `Resolved`/`Receiver`.

Verified via grep (excluding test files): each has **exactly one caller**, both in `it_this.rs`
(`cst_lambda.rs:510` ← `it_this.rs:115`; `cst_lambda.rs:638` ← `it_this.rs:90`) — so merging them is
a pure, low-risk, fully move-don't-rewrite consolidation with the existing test suite
(`it_this_tests.rs`) as the regression net; nothing downstream changes.

Once merged, `all_this_receivers_at` becomes the real target for a genuine new `CstQuery` method:
`it_this::all_lambda_receivers_at` (the only production-facing name — real call site
`src/features/missing_import_diagnostics.rs:244`, verified, unchanged by this plan) is the
position→node bridge; today it calls `cst_lambda::all_this_receivers_at` directly. Task 2 adds
`CstQuery::all_this_receivers()` and — in the same task — redirects that bridge onto it, deleting
the direct call. This mirrors the exact pattern `completion_context.rs::collect_lambda_scopes`
already uses for `CstQuery::lambda_scope()` (position→node bridge, then a `CstQuery` call).

**Tech stack:** Rust, tree-sitter (`tree_sitter::Node`), binary-only crate (`kmp-lsp`,
`cargo test --bin kmp-lsp`; `--lib` runs 0 tests).

**Spec:** `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md` (step 3,
"collapse the lambda triad").

---

## Verification performed before writing this plan (so the next reader doesn't have to redo it)

- Re-grepped every `it_this.rs` public function's callers repo-wide, filtered to real production
  (non-test, non-doc-comment, non-`indexer.rs`-re-export) call sites:
  - `find_it_element_type`: `features/completion.rs`, `semantic_tokens/resolve.rs`,
    `indexer/scope.rs`, `indexer/infer/speculative.rs`.
  - `find_this_context`: `indexer/scope.rs`, `features/completion_context.rs`.
  - `find_this_element_type`: `semantic_tokens/resolve.rs`.
  - `find_named_lambda_param_type`: `features/completion.rs`, `indexer/scope.rs`.
  - `is_lambda_param`: `features/completion.rs`.
  - `all_lambda_receivers_at`: `features/missing_import_diagnostics.rs:244` — **only one**.
- `receiver.rs` (the design doc's "text-context heuristics" engine) **no longer exists** as a file —
  already folded away; confirms the prior plan's finding that `it_this.rs`'s functions are
  CST-driven internally, not text/line scans, is still accurate.
- `cst_lambda::all_this_receivers_at` and `cst_lambda::cst_this_context` each have exactly one
  caller, both in `it_this.rs`, confirmed by reading both functions' bodies directly — both are
  independent copies of the same "walk ancestors, `if kind == KIND_LAMBDA_LIT { classify }`" loop,
  differing only in whether they collect every match or return on the first one.
- `CstQuery::new` is a 4-arg constructor (`node, doc, deps, uri`) — no `io` field (removed in
  PR #247); `Indexer` implements `InferDeps`, so `CstQuery::new(node, doc, idx, uri)` type-checks
  wherever `idx: &Indexer` is already in scope (as it is in every `it_this.rs` bridge function).
  `mod.rs`'s "Known gaps" doc block is otherwise accurate as of this session.
- `completion_context.rs::ScopeContext::build` (lines ~181-205) dual-purposes
  `LambdaScopeInfo.it_type` for both the implicit `it` parameter and (via `resolve_labeled_receiver`)
  `this@label` receiver resolution, and its last step
  (`lambda_scopes.last_mut().it_type = index.infer_lambda_param_type_at(IT, uri, position)`)
  overwrites whichever scope is last in the vec — confirmed by reading the file. This is the concrete
  reason `LambdaScope` promotion (design doc's `this: ThisLambdaCtx` field) is descoped below rather
  than attempted in this slice.

---

## Task 1: Merge the two duplicate `this`-classification ancestor walks in `cst_lambda.rs`

**Rationale:** `all_this_receivers_at` and `cst_this_context` are two independently-written copies
of "walk `lambda_literal` ancestors from a node, classify each via `lambda_this_ctx`" — exactly the
kind of divergent-copy risk the design doc calls "the bug factory." Zero new public surface; the
existing test suite (each function's sole caller's own tests) is the complete regression net.

**Files:**
- Modify: `src/indexer/infer/cst_lambda.rs`

**Interfaces:**
- New private helper, placed directly above `all_this_receivers_at`:
  ```rust
  /// Ancestor `lambda_literal` nodes from `start_node` outward (innermost first —
  /// `start_node` itself first if it IS a `lambda_literal`), each classified via
  /// [`lambda_this_ctx`]. The shared walk step behind [`all_this_receivers_at`]
  /// (collects every `Resolved`) and [`cst_this_context`] (stops at the first
  /// `Resolved`/`Receiver`) — previously two separate copies of the same
  /// ancestor-walk-and-classify loop.
  ///
  /// Always walks the full ancestor chain before either caller inspects the
  /// result, trading `cst_this_context`'s previous early-exit for one shared
  /// shape. Lambda nesting in real Kotlin code is shallow (rarely >3-4 levels),
  /// so the extra `lambda_this_ctx` calls on ancestors past the first match are
  /// cheap.
  fn this_lambda_ancestor_ctxs(
      start_node: tree_sitter::Node<'_>,
      doc: &crate::indexer::live_tree::LiveDoc,
      deps: &impl InferDeps,
      uri: &Url,
  ) -> Vec<ThisLambdaCtx> {
      let mut ctxs = Vec::new();
      let mut cur = start_node;
      loop {
          if cur.kind() == KIND_LAMBDA_LIT {
              ctxs.push(lambda_this_ctx(cur, doc, deps, uri));
          }
          let Some(parent) = cur.parent() else { break };
          cur = parent;
      }
      ctxs
  }
  ```

- [ ] **Step 1:** Add `this_lambda_ancestor_ctxs` above `all_this_receivers_at`
  (`cst_lambda.rs:503`, right before its doc comment).

- [ ] **Step 2:** Rewrite `all_this_receivers_at`'s body (keep its existing doc comment, it's still
  accurate) to:
  ```rust
  pub(crate) fn all_this_receivers_at(
      start_node: tree_sitter::Node<'_>,
      doc: &crate::indexer::live_tree::LiveDoc,
      deps: &impl InferDeps,
      uri: &Url,
  ) -> Vec<String> {
      this_lambda_ancestor_ctxs(start_node, doc, deps, uri)
          .into_iter()
          .filter_map(|ctx| match ctx {
              ThisLambdaCtx::Resolved(receiver_type) => Some(receiver_type),
              ThisLambdaCtx::Receiver | ThisLambdaCtx::NotReceiver => None,
          })
          .collect()
  }
  ```

- [ ] **Step 3:** Rewrite `cst_this_context`'s body (`cst_lambda.rs:638-657`, keep its doc comment)
  to:
  ```rust
  pub(super) fn cst_this_context(
      start_node: tree_sitter::Node<'_>,
      doc: &crate::indexer::live_tree::LiveDoc,
      idx: &impl InferDeps,
      uri: &Url,
  ) -> ThisContext {
      for ctx in this_lambda_ancestor_ctxs(start_node, doc, idx, uri) {
          match ctx {
              ThisLambdaCtx::Resolved(t) => return ThisContext::Resolved(t),
              ThisLambdaCtx::Receiver => return ThisContext::InsideReceiver,
              ThisLambdaCtx::NotReceiver => {}
          }
      }
      ThisContext::NotFound
  }
  ```

- [ ] **Step 4: Run the full suite.**
  Run: `cargo test --bin kmp-lsp`
  Expected: all green, in particular every `it_this_tests.rs` test covering
  `all_lambda_receivers_at` and `find_this_context`/`ThisContext`, plus the tests in
  `indexer/scope_tests.rs` and `completion_context`'s own tests — the real production call sites
  (`indexer/scope.rs`, `features/completion_context.rs`, `features/missing_import_diagnostics.rs`)
  are exercised transitively through these.

- [ ] **Step 5: Commit.**
  ```bash
  git add -A
  git commit -m "refactor(infer): merge cst_lambda's two duplicate this-classification ancestor walks"
  ```

---

## Task 2: Add `CstQuery::all_this_receivers()` and wire `it_this::all_lambda_receivers_at` onto it

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add the method)
- Modify: `src/indexer/infer/it_this.rs` (redirect the bridge function's implementation; adjust
  imports)

**Interfaces:**
- New `CstQuery` method, added to the existing `impl<'a, D: InferDeps> CstQuery<'a, D>` block,
  placed after `expr_type()`:
  ```rust
  /// Every enclosing lambda receiver type at the bound node, innermost-first —
  /// the order Kotlin resolves an implicit-receiver call in (nearest wins). An
  /// unresolvable or non-receiver lambda is skipped; the walk continues outward.
  ///
  /// Used to check a bare call/type reference against every candidate receiver
  /// in scope (e.g. `item()` inside `with(x) { }` nested in a builder belongs to
  /// the outer receiver even when `x`'s type can't be resolved).
  pub(crate) fn all_this_receivers(&self) -> Vec<String> {
      cst_lambda::all_this_receivers_at(self.node, self.doc, self.deps, self.uri)
  }
  ```

- **Real production call site being swapped onto it:** `features/missing_import_diagnostics.rs:244`
  (that file is unchanged by this task — it calls
  `it_this::all_lambda_receivers_at(pos, indexer, uri)`, whose *implementation* changes below). This
  is the identical pattern `completion_context.rs::collect_lambda_scopes` already uses for
  `CstQuery::lambda_scope()`: a stable position-based bridge function whose body constructs a
  `CstQuery` and calls the catalogue method.

- [ ] **Step 1:** Add the method above to `mod.rs`. No new `use` needed — `cst_lambda` is already
  `pub(super) mod cst_lambda;` and used by `lambda_scope()`.

- [ ] **Step 2:** In `it_this.rs`, change the import block (currently around lines 26-29):
  ```rust
  use super::cst_lambda::{
      cst_it_element_type, cst_named_lambda_param_type, cst_this_context, cursor_node_at,
  };
  use super::CstQuery;
  ```
  (dropped `all_this_receivers_at` from the `cst_lambda` import list — no longer called directly
  from this file; added `use super::CstQuery;`.)

- [ ] **Step 3:** Rewrite `all_lambda_receivers_at`'s body (`it_this.rs:107-116`, keep its doc
  comment) to:
  ```rust
  pub(crate) fn all_lambda_receivers_at(pos: CursorPos, idx: &Indexer, uri: &Url) -> Vec<String> {
      let Some(resolution) = lambda_doc_at(idx, uri, pos) else {
          return vec![];
      };
      let doc = resolution.doc();
      let Some(node) = cursor_node_at(doc, pos) else {
          return vec![];
      };
      CstQuery::new(node, doc, idx, uri).all_this_receivers()
  }
  ```
  This deletes the old direct call to `cst_lambda::all_this_receivers_at` — the old code path is
  gone, not left as a parallel mechanism.

- [ ] **Step 4: Run the full suite.**
  Run: `cargo test --bin kmp-lsp`
  Expected: all green — same regression net as Task 1's Step 4 (the `all_lambda_receivers_at` tests
  in `it_this_tests.rs` exercise this exact code path unchanged from the outside).

- [ ] **Step 5: Run clippy.**
  Run: `cargo clippy --all-targets --all-features -- -D warnings`
  Expected: clean — watch for an unused-import warning if Step 2's edit isn't exact.

- [ ] **Step 6: Commit.**
  ```bash
  git add -A
  git commit -m "feat(infer): add CstQuery::all_this_receivers, wire all_lambda_receivers_at onto it"
  ```

---

## Task 3: Update `mod.rs`'s "Known gaps" doc comment

**Files:**
- Modify: `src/indexer/infer/mod.rs` (module doc comment, `it_this` bullet)

**Rationale:** Design doc Goal 3 — the catalogue's own doc must stay honest about what's still
outside it. After Task 2, `all_lambda_receivers_at` is no longer a capability that bypasses
`CstQuery` internally (even though it's still a `CursorPos`-taking flat export by name) — the
bullet must reflect that precisely, not overstate or understate what changed.

- [ ] **Step 1:** Replace the current `it_this` bullet:
  ```
  //! - `it_this` (`find_it_element_type`, `find_this_context`, `find_this_element_type`,
  //!   `find_named_lambda_param_type`, `is_lambda_param`, `all_lambda_receivers_at`) — CST-driven
  //!   internally already (delegates to `cst_lambda`), but takes a `CursorPos` + does its own
  //!   repair-gated node acquisition; folding into `CstQuery`'s bound-`Node` model needs a
  //!   `CstQuery::at_position` bridge — deferred, see the design doc's lambda-triad/`LambdaScope`-
  //!   promotion step.
  ```
  with:
  ```
  //! - `it_this` (`find_it_element_type`, `find_this_context`, `find_this_element_type`,
  //!   `find_named_lambda_param_type`, `is_lambda_param`) — CST-driven internally already
  //!   (delegates to `cst_lambda`), but takes a `CursorPos` + does its own repair-gated node
  //!   acquisition; folding into `CstQuery`'s bound-`Node` model needs a `CstQuery::at_position`
  //!   bridge — deferred, see the design doc's lambda-triad/`LambdaScope`-promotion step.
  //!   `all_lambda_receivers_at` is the one exception: its position→node bridge now constructs a
  //!   `CstQuery` and calls `all_this_receivers()` directly (2026-08-03) — still a flat
  //!   `CursorPos`-taking export by name, but no longer bypasses the catalogue underneath.
  ```

- [ ] **Step 2: Run the full suite** (doc-only change).
  Run: `cargo test --bin kmp-lsp`
  Expected: all green.

- [ ] **Step 3: Commit.**
  ```bash
  git add -A
  git commit -m "docs(infer): mod.rs catalogue doc reflects all_lambda_receivers_at's CstQuery wiring"
  ```

---

## Task 4: Final verification pass

- [ ] **Step 1: Full suite.**
  Run: `cargo test --bin kmp-lsp`
  Expected: all green (binary-only crate — `cargo test --lib` runs 0 tests, not a signal).

- [ ] **Step 2: Clippy, both gates.**
  Run: `cargo clippy -- -D warnings` (AGENTS.md baseline)
  Run: `cargo clippy --all-targets --all-features -- -D warnings` (catches issues only visible with
  tests compiled)
  Expected: both clean.

- [ ] **Step 3: `cargo fmt --check`.**
  Expected: no diff.

- [ ] **Step 4: Confirm the consolidation left no orphaned duplicate.**
  Run: `grep -n "fn all_this_receivers_at\|fn cst_this_context\|fn this_lambda_ancestor_ctxs" src/indexer/infer/cst_lambda.rs`
  Expected: exactly one definition of each; `this_lambda_ancestor_ctxs` has exactly two callers
  (`all_this_receivers_at`, `cst_this_context`) — confirm via
  `grep -n "this_lambda_ancestor_ctxs" src/indexer/infer/cst_lambda.rs`.

- [ ] **Step 5: Confirm no parallel mechanism was left for `all_lambda_receivers_at`.**
  Run: `grep -rn "all_this_receivers_at" src --include="*.rs" | grep -v _tests.rs`
  Expected: the `cst_lambda.rs` definition, `mod.rs`'s new `CstQuery::all_this_receivers` call, and
  nothing calling `cst_lambda::all_this_receivers_at` directly from `it_this.rs` anymore.

---

## Explicitly descoped (and why — matching the reverted plan's own rigor bar)

- **`CstQuery::at_position` bridge + folding `find_it_element_type`/`find_this_context`/
  `find_named_lambda_param_type`/`is_lambda_param` fully into `CstQuery`'s bound-`Node` model.**
  Real call sites exist (spread across `completion.rs`, `scope.rs`, `semantic_tokens/resolve.rs`,
  `completion_context.rs`, `speculative.rs`) but they span 5 different feature files each with
  slightly different repair-gating requirements (some need `lambda_doc_at`'s speculative
  brace-repair, `missing_import_diagnostics.rs` — the one case handled here — does not, since it
  runs on an already-fully-parsed doc). Wiring all of them in one slice is materially riskier than
  Tasks 1-2 above; the natural next increment once someone is ready to design the bridge itself.
- **`LambdaScope` promotion** (design doc: add `this: ThisLambdaCtx` field, upgrade `String`s to
  `ReceiverType`). Investigated concretely this session and found a real landmine, not just
  unscoped effort: `completion_context.rs::ScopeContext::build` reuses `LambdaScopeInfo.it_type` for
  **two different concepts** — the implicit `it` parameter AND (via `resolve_labeled_receiver`) the
  `this@label` receiver type — and its final line
  (`lambda_scopes.last_mut().it_type = index.infer_lambda_param_type_at(IT, uri, position)`)
  overwrites whichever scope happens to be last. Any change to `cst_lambda_scopes`'s current filter
  (`lambda_scope_info` returns `None`, dropping the entry, when both `it_type` and `named_params` are
  empty) changes which scope is "last" and therefore which scope receives that overwrite — a real,
  silent behavior-changing risk that needs its own investigation + decoy-test-first task, not
  something to bundle blind into this slice.
- **`CstQuery::receiver_type()`/`call_return_type()`.** Unchanged from both prior revert
  post-mortems: no real consumer was found this session either (the same three candidates —
  `chain.rs`, `type_subst.rs`, `expr_type.rs::infer_navigation_expr_type` — are still not drop-in
  fits, for the same reasons already documented). Only reconsider once a real consumer is named up
  front, in the same task that adds the method.
- **`chain.rs` collapse** (design doc step 4) — untouched, unstarted, out of this slice's blast
  radius.
- **`CstExpr` exhaustive dispatch, `RawTypeName`/`TypeName` split, construction-sealed
  `ReceiverType`/`ResolvedType`** (design doc step 5, "Sweep") — untouched, confirmed still
  unstarted by reading the actual code (`expr_type.rs::infer_expr_type` is still a plain `match`
  with a `_ => None` arm; `ResolvedType` still wraps one bare `String`).
- **CST-aware navigation** (design doc step 6) — depends on steps 3-5 landing first; untouched.

## Testing & verification (gates, matching this repo's conventions)

- `cargo test --bin kmp-lsp` after every task (binary-only crate; `--lib` is not a signal).
- `cargo clippy --all-targets --all-features -- -D warnings` before each task's final commit.
- No new `TestDeps` methods needed; no test files need new content — Tasks 1-2 are pure internal
  refactors behind stable public signatures, so the existing `it_this_tests.rs` and
  `missing_import_diagnostics.rs` test suites are the complete regression net.
