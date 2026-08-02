# CST Resolution Catalogue — Gap Closure (facade completion + real `Ambiguous`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the two structural gaps a 2026-08-02 audit found between
`docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md` and the current state of
`src/indexer/infer/mod.rs`:

1. The design doc's `CstQuery` skeleton promises `receiver_type()` and `call_return_type()`; neither
   exists. Add both as real, tested, thin delegations — matching exactly how `expr_type()` and
   `lambda_scope()` already work — and make the catalogue's own doc comment honest about what still
   lives outside it and why.
2. `Resolution::Ambiguous(Vec<Fqn>)` and `Fqn` are `#[allow(dead_code)]` — never constructed. Migrate
   `sig.rs`'s `SignatureResult` (which already models found/ambiguous/absent for call signatures) onto
   `Resolution<Signature>`, so `Ambiguous` becomes load-bearing instead of decorative.

**Architecture:** No new engine, no new walk — this slice is entirely surface-level, consistent with
the design doc's own "Sequencing" discipline (`mod.rs` is the catalogue; move-don't-rewrite; each step
green before the next). `receiver_type()`/`call_return_type()` are built from pieces that already exist
today: `ReceiverType::from_raw` (`resolver/infer.rs:124`, a pure string→struct parser, domain-agnostic)
and the same `InferDeps` return-type lookups `expr_type.rs::infer_navigation_expr_type` already calls.
The `SignatureResult` migration reuses `sig.rs`'s existing `Unique`/`Overloaded`/`NotFound`/
`UnresolvableReceiver` detection logic verbatim; only the wrapper type changes. Spec:
`docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`.

**Tech Stack:** Rust, tree-sitter (`tree_sitter::Node`), `tower_lsp`, binary-only crate (`kmp-lsp`).

## Explicit scope call (read before starting)

This plan was written after re-verifying the audit's claims against `76d3f2b` (see "Drift check"
below — the audit held up; nothing here is stale). Two scope decisions the audit asked for:

**Gap 1 — narrow, not the full ~25-function migration.** Only `receiver_type()` and
`call_return_type()` are added to `CstQuery` in this slice — the two methods the design doc's own
`CstQuery` skeleton explicitly promises and no more. The other ~23 free functions
(`it_this.rs`'s `find_it_element_type`/`find_this_context`/`find_named_lambda_param_type`/
`is_lambda_param`/`all_lambda_receivers_at`, all of `sig.rs` except the migrated `SignatureResult`,
`cst_symbol.rs`'s navigation family, `args.rs`, `type_subst.rs`, `lambda.rs`) stay as direct submodule
exports. Reasons, checked against the real code, not assumed:
- `it_this.rs`'s functions are **already CST-driven internally** (they delegate straight into
  `cst_lambda.rs`'s `cst_it_element_type`/`cst_this_context`/etc. — confirmed by reading
  `src/indexer/infer/it_this.rs:63-116`; the "line scan" framing in the design doc's fragmentation
  table predates that internal migration). The remaining gap is *only* that they take a `CursorPos`
  and do their own repair-gated node acquisition (`lambda_doc_at` → `cursor_node_at`), a different
  entry shape from `CstQuery`'s bound-`Node` model. Folding them into `CstQuery` for real would mean
  designing a `CstQuery::at_position` bridge *and* extending `LambdaScope` with the design doc's
  promised `this: ThisLambdaCtx` field and upgrading its `String`s to `ReceiverType` — real design work
  the doc itself schedules as its own step (the "collapse the lambda triad" / `LambdaScope` promotion
  work), not a two-method surface fix. Forcing it into this slice would violate "move-don't-rewrite."
- Real external consumer counts stay small either way (measured via grep, `find_referencing_symbols`
  hit repo-wide MCP-state issues from concurrent worktree use this session so grep was the fallback —
  see Verification section): `find_it_element_type` 3 production call sites
  (`features/completion.rs`, `indexer/scope.rs`, `semantic_tokens/resolve.rs`), `find_this_context` 2,
  `find_named_lambda_param_type` 3, `is_lambda_param` 2, `all_lambda_receivers_at` 1,
  `find_this_element_type` 1 — small enough that a future slice can move them without urgency, not so
  large that leaving them is obviously wrong either.
- Every other candidate location to "prove `receiver_type()`/`call_return_type()` aren't decorative" by
  wiring a real consumer turned out to be a bad fit: `nullable_call_diagnostics.rs` is on the **string**
  engine (`resolver::infer::infer_receiver_type`, not CST — out of scope per the design doc's own
  non-goals), and `expr_type.rs::infer_navigation_expr_type` (the one CST function that already computes
  receiver-then-call-return-type inline) is shared/recursive — rewriting it to call back into `CstQuery`
  is exactly the kind of internals-rewrite the design doc's sequencing discipline defers to a dedicated
  step. So this slice lands the two methods with real `TestDeps`-driven unit tests (matching the
  precedent Phase 1 itself set — its Task 2 added `expr_type` to the catalogue with only a unit test;
  routing a real consumer was a *separate* Task 3). Wiring a first real consumer is a natural
  immediately-following micro-slice, not bundled here.
- Because leaving the facade silently incomplete would itself violate the design doc's Goal 3, this
  slice also updates `mod.rs`'s doc comment to say plainly which capability families remain outside
  `CstQuery` and why (Task 3) — cheap, and keeps the catalogue honest without requiring the migration.

**Gap 2 — `SignatureResult` migration IS in scope for this slice**, not deferred, for one hard reason:
`SignatureResult`'s `Overloaded` variant is the **only** genuine ambiguity-detection logic anywhere in
this codebase today (verified — grepped for `Ambiguous`, `overload`, `candidates` across
`indexer/infer/`; the only other "pick one of several" spot is `cst_lambda.rs`'s
`pick_outer_scoped_signature`, which disambiguates *outer-scoped lambda parameter signature text*, a
different shape of problem that doesn't produce `Fqn`s). If this migration is deferred, `Ambiguous`
stays permanently unconstructed and Gap 2 is not actually closed, just re-described. The design doc
itself already calls this migration out by name ("`SignatureResult`... becomes `Resolution<Signature>`
post-catalogue") — this slice **is** "post-catalogue" (Phase 1 shipped; `CstQuery` exists).
`Resolution<T>::Ambiguous` keeps its `Vec<Fqn>` shape (not generalized to `Vec<T>`) — candidate `Fqn`s
are cheaply constructible from the location URIs `sig.rs` already has in hand at every `Overloaded`
site (see Task 4), so there was no real need to widen the enum's contract for this migration to work.

**Out of scope for this slice (design doc "step 5: Sweep", confirmed still entirely unstarted by
reading the actual code):**
- `CstExpr` exhaustive-dispatch enum — `expr_type.rs::infer_expr_type` (`src/indexer/infer/expr_type.rs:60`)
  is still a plain `match node.kind() { ... _ => None }`, confirmed by reading the file; a new node kind
  added elsewhere would silently fall through. Not touched here — this slice does not change dispatch
  shape, only adds two facade methods and a type migration.
- `RawTypeName`/`TypeName` split — `ResolvedType` (`mod.rs:98`) still wraps one bare `String`. Not
  touched.
- Folding `it_this.rs`/`sig.rs`(remainder)/`cst_symbol.rs`/`args.rs`/`type_subst.rs`/`lambda.rs` into
  `CstQuery` — per the Gap 1 call above.
- Construction-sealing `ReceiverType`/`ResolvedType` (private-field, catalogue-only construction) — not
  attempted; `ReceiverType` is reused as-is from `resolver/infer.rs` where its fields are already
  `pub(crate)` and used by string-engine call sites this slice does not touch.

## Drift check against the audit summary (verified 2026-08-02 against `76d3f2b`)

- The `src/indexer.rs` re-export block quoted in the audit is **verbatim accurate** — read the file,
  byte-for-byte match (lines 21-61).
- `CstQuery` really does expose only `expr_type()` and `lambda_scope()` (`mod.rs:180-199`) — confirmed.
- `SignatureResult` really is still its own 4-variant enum (`sig.rs:44-56`): `Unique{params_text,
  param_counts}` / `Overloaded` / `NotFound` / `UnresolvableReceiver` — variant names unchanged from the
  audit summary.
- `Resolution::Ambiguous` really is never constructed anywhere — grepped `Ambiguous` repo-wide; every
  hit is in `mod.rs` itself (the definition + the two `match` arms in `resolved()`/`resolved_ref()`).
  No drift.
- One correction to the audit's framing, not a contradiction: the audit's "it_this.rs — 523 — line-string
  scans" description (inherited from the design doc's original fragmentation table) is **stale** for the
  current code. `it_this.rs`'s public functions already delegate to CST (`cst_lambda.rs`) internally;
  what's missing is only the `CstQuery` facade wiring, not a text-scanning engine. This *strengthens* the
  case for Gap 1's narrow scope call above (folding it in is a facade-only exercise, not an engine
  rewrite — still real work, just different work than "delete more text scanning").
- One new finding not in the original audit: `Resolution<T>` currently derives nothing (`mod.rs:67`, no
  `#[derive(...)]` above the enum) — `Debug`/`Clone` must be added for the `SignatureResult` migration
  to compile (`call_arg_diagnostics.rs` clones a `HashMap<_, SignatureResult>` cache; `sig_tests.rs`
  panics with `{other:?}` on non-matching arms). Folded into Task 4 below; not a scope change, just a
  compile-order dependency worth flagging up front.

---

## Task 1: Add `receiver_type()` to `CstQuery`

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add the method + import `ReceiverType`)
- Test: `src/indexer/infer/mod_tests.rs`

**Interfaces:**
- Consumes: `Self::expr_type()` (existing, `mod.rs:189`); `ReceiverType::from_raw(raw: String) ->
  ReceiverType` (existing, pure, `resolver/infer.rs:124` — already reused as-is per the design doc's
  reuse inventory).
- Produces:
  ```rust
  pub(crate) fn receiver_type(&self) -> Resolution<ReceiverType> {
      match self.expr_type() {
          Resolution::Resolved(resolved) => Resolution::Resolved(
              ReceiverType::from_raw(resolved.as_type_str().to_owned()),
          ),
          Resolution::Ambiguous(candidates) => Resolution::Ambiguous(candidates),
          Resolution::Unresolved => Resolution::Unresolved,
      }
  }
  ```
  (The `Ambiguous` arm is unreachable today — `expr_type()` never emits it — but must be written for
  exhaustiveness; this is intentionally the same "wiring seam for later" pattern `mod.rs` already uses
  elsewhere, e.g. the `io` field.)

- [ ] **Step 1: Add the `use` for `ReceiverType`.** `use crate::resolver::infer::ReceiverType;` near the
  top of `mod.rs`, next to the existing `use self::deps::InferDeps;`.

- [ ] **Step 2: Write a failing test.**

```rust
// src/indexer/infer/mod_tests.rs
#[test]
fn cst_query_receiver_type_splits_qualified_generic_nullable() {
    let source = "fun f() = holder\n"; // `holder: Outer.Inner<Param>?`
    let live_doc = live_doc_for(source);
    let ident_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/Receiver.kt");
    indexer.index_content(&uri, source);
    // register `holder: Outer.Inner<Param>?` via TestDeps or an indexed val decl —
    // use whichever seam `CstQuery::expr_type` unit tests already use for var types
    // (see existing `cst_query_expr_type_*` tests above this one in the same file).

    let receiver = CstQuery::new(ident_node, &live_doc, &indexer, &uri, ResolveIo::IndexOnly)
        .receiver_type()
        .resolved()
        .expect("holder should resolve");
    assert_eq!(receiver.qualified, "Outer.Inner");
    assert_eq!(receiver.outer, "Outer");
    assert_eq!(receiver.leaf, "Inner");
    assert!(receiver.nullable);
}
```

- [ ] **Step 3: Run it, confirm it fails to compile** (`receiver_type` does not exist yet).

Run: `cargo test --bin kmp-lsp cst_query_receiver_type_splits_qualified_generic_nullable`
Expected: compile error.

- [ ] **Step 4: Add the method** (code above). Re-export `ReceiverType` is not needed separately — it's
  already `pub(crate)` in `resolver::infer` and reachable; only `mod.rs` needs the `use`.

- [ ] **Step 5: Run the test + full suite.**

Run: `cargo test --bin kmp-lsp`
Expected: new test PASS; baseline all green.

- [ ] **Step 6: Commit.**

```bash
git add -A
git commit -m "feat(infer): add CstQuery::receiver_type, thin delegation to expr_type + ReceiverType::from_raw"
```

---

## Task 2: Add `call_return_type()` to `CstQuery`

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add the method + import `ReturnType`)
- Test: `src/indexer/infer/mod_tests.rs`

**Interfaces:**
- Consumes: `InferDeps::find_method_return_type_for_type(&self, receiver_qualified: &str, name: &str,
  uri: &Url) -> Option<String>`, `InferDeps::find_fun_return_type_reachable(&self, name: &str, uri:
  &Url) -> Option<String>`, `InferDeps::find_fun_return_type(&self, name: &str) -> Option<String>` — all
  existing (`indexer/infer/deps.rs`), the exact three calls `expr_type.rs::infer_navigation_expr_type`
  already chains at `expr_type.rs:185-188` for the receiver-known case; the same
  reachable→by-name fallback order is used for the no-receiver case, matching
  `expr_type.rs`'s own comment about mirroring `Resolver::function_return_type`.
- Produces:
  ```rust
  pub(crate) fn call_return_type(
      &self,
      receiver: Option<&ReceiverType>,
      name: &str,
  ) -> Resolution<ReturnType> {
      let result = match receiver {
          Some(receiver_type) => self
              .deps
              .find_method_return_type_for_type(&receiver_type.qualified, name, self.uri)
              .or_else(|| self.deps.find_fun_return_type_reachable(name, self.uri))
              .or_else(|| self.deps.find_fun_return_type(name)),
          None => self
              .deps
              .find_fun_return_type_reachable(name, self.uri)
              .or_else(|| self.deps.find_fun_return_type(name)),
      };
      match result {
          Some(raw) => Resolution::Resolved(ReturnType(raw)),
          None => Resolution::Unresolved,
      }
  }
  ```

- [ ] **Step 1: Add the `use` for `ReturnType`.** `use crate::resolver::api::ReturnType;` in `mod.rs`.
  Check `ReturnType`'s field (`resolver/api.rs:62`, `pub(crate) struct ReturnType(pub String)`) is
  visible from `indexer::infer` — it is (`pub(crate)`, cross-module, same as `ReceiverType`).

- [ ] **Step 2: Write a failing test** (`TestDeps`-driven, mirrors the `deps` seam directly — no CST
  node needed for the receiver-known arm since `call_return_type` takes an already-resolved
  `&ReceiverType`, not a node):

```rust
// src/indexer/infer/mod_tests.rs
#[test]
fn cst_query_call_return_type_resolves_method_on_known_receiver() {
    let source = "fun f() = 1\n"; // node content unused by this arm; CstQuery still needs a bound node
    let live_doc = live_doc_for(source);
    let any_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let uri = test_url("/CallReturn.kt");
    let deps = super::deps::TestDeps::new()
        .with_method_return_for_type("Repository", "load", "Item");
    let receiver = super::super::ReceiverType::from_raw("Repository".to_owned());

    let query = CstQuery::new(any_node, &live_doc, &deps, &uri, ResolveIo::IndexOnly);
    let resolved = query.call_return_type(Some(&receiver), "load").resolved();
    assert_eq!(resolved.map(|r| r.0).as_deref(), Some("Item"));
}
```
(`TestDeps::with_method_return_for_type` already exists — used by `mod_tests.rs`'s existing chain tests,
e.g. `resolve_callee_chain_does_not_corrupt_receiver_when_final_method_is_indexed_and_generic`.)

- [ ] **Step 3: Run it, confirm it fails to compile.**

Run: `cargo test --bin kmp-lsp cst_query_call_return_type_resolves_method_on_known_receiver`
Expected: compile error (`call_return_type` does not exist).

- [ ] **Step 4: Add the method** (code above).

- [ ] **Step 5: Run the test + full suite.**

Run: `cargo test --bin kmp-lsp`
Expected: new test PASS; baseline all green.

- [ ] **Step 6: Commit.**

```bash
git add -A
git commit -m "feat(infer): add CstQuery::call_return_type, thin delegation to the existing InferDeps return-type chain"
```

---

## Task 3: Make `mod.rs`'s catalogue doc comment honest about what's still outside it

**Files:**
- Modify: `src/indexer/infer/mod.rs` (the module doc comment at the top of the file, lines 1-28)

**Rationale:** Design doc Goal 3 — "agents read one file and route to the existing capability instead
of reinventing" — is undermined if `mod.rs` implies `CstQuery` is the complete surface when ~23
functions bypass it. This is documentation-only, zero behavior change, but it's the cheapest available
move that keeps the catalogue's stated promise honest without requiring the full migration this plan
explicitly defers (see "Explicit scope call" above).

- [ ] **Step 1: Add a "Known gaps" subsection to the module doc comment**, after the existing "Types
  produced" table, naming each capability family still exported flat from `src/indexer.rs` instead of
  through `CstQuery`, one line each:
  - `it_this` (`find_it_element_type`, `find_this_context`, `find_this_element_type`,
    `find_named_lambda_param_type`, `is_lambda_param`, `all_lambda_receivers_at`) — CST-driven
    internally already (delegates to `cst_lambda`), but takes a `CursorPos` + does its own repair-gated
    node acquisition; folding into `CstQuery`'s bound-`Node` model needs a `CstQuery::at_position`
    bridge — deferred, see the design doc's lambda-triad/`LambdaScope`-promotion step.
  - `sig` (signature/param-text helpers other than `Signature`/`Resolution<Signature>` — see below) —
    pure string/slice helpers, several IO-bound (`find_fun_signature_full` may trigger on-demand rg
    indexing); not expression-type resolution, out of `CstQuery`'s "type of a bound node" remit.
  - `cst_symbol` (`classify_cursor`, `resolve_identity`, navigation helpers) — the symbol-identity
    navigation family's own facade (design doc step 6, already CST-first with string+rg fallback);
    intentionally a peer of `CstQuery`, not a submodule of it.
  - `args`, `type_subst`, `lambda` — low-level primitives (`extract_first_arg`, generic-substitution
    string ops, lambda-type-string decomposition) consumed *by* the CST engine's own submodules
    (`cst_lambda.rs`, `chain.rs`), not independently by features; not judged to need a facade.

- [ ] **Step 2: Run the full suite** (doc-only change, but confirms nothing else broke in the same
  commit if Task 1/2 land together).

Run: `cargo test --bin kmp-lsp`
Expected: all green (no production code changed).

- [ ] **Step 3: Commit.**

```bash
git add -A
git commit -m "docs(infer): mod.rs catalogue doc names what's still outside CstQuery and why"
```

---

## Task 4: Migrate `SignatureResult` onto `Resolution<Signature>`

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add `#[derive(Debug, Clone)]` to `Resolution<T>` and `Fqn`; remove
  the `#[allow(dead_code)]` on both now that `Ambiguous`/`Fqn` are constructed for real)
- Modify: `src/indexer/infer/sig.rs` (replace the `SignatureResult` enum with a `Signature` struct;
  retarget `resolve_call_signature` and its helpers to return `Resolution<Signature>`)
- Modify: `src/indexer/infer/sig_tests.rs` (9 match/assert sites)
- Modify: `src/features/call_arg_diagnostics.rs` (the `sig_cache` type + the 4-arm match)
- Modify: `src/indexer.rs` (re-export list: `SignatureResult` → `Signature`)

**Interfaces:**
- `Signature` (new, in `sig.rs`) — exactly today's `SignatureResult::Unique`'s payload, promoted to its
  own type:
  ```rust
  #[derive(Debug, Clone)]
  pub(crate) struct Signature {
      pub(crate) params_text: String,
      pub(crate) param_counts: (usize, usize),
  }
  ```
- `resolve_call_signature(call: &CallSite, idx: &Indexer) -> Resolution<Signature>` (signature changes
  from returning `SignatureResult`).
- Mapping from today's 4 variants (verified against `sig.rs:906-1091` and the one external consumer,
  `call_arg_diagnostics.rs:166-174`, which already treats `Overloaded | NotFound | UnresolvableReceiver`
  identically — collapsing the latter two into one `Unresolved` is behavior-preserving, not a guess):
  - `Unique { params_text, param_counts }` → `Resolution::Resolved(Signature { params_text,
    param_counts })`
  - `NotFound` → `Resolution::Unresolved`
  - `UnresolvableReceiver` → `Resolution::Unresolved`
  - `Overloaded` → `Resolution::Ambiguous(candidates)` — see Step 2 for what populates `candidates`.

- [ ] **Step 1: Add derives.** In `mod.rs`: `#[derive(Debug, Clone)]` above `pub(crate) enum
  Resolution<T> { ... }` and above `pub(crate) struct Fqn(pub(crate) String);`. Remove the
  `#[allow(dead_code)]` line above `Fqn` and the one above the `Ambiguous(Vec<Fqn>)` variant (both
  become real once Step 3 constructs them). Leave `resolved_ref`'s `#[allow(dead_code)]` — it's still
  genuinely unused; do not remove it speculatively.

- [ ] **Step 2: Decide and implement how `Overloaded` sites populate `Vec<Fqn>`.** Two call sites in
  `sig.rs` return `Overloaded` today:
  - `build_result` (`sig.rs:1071-1091`) — has the deduplicated `(params_text, (u8,u8))` candidates in
    hand (`deduped`) but not their defining locations (the `Location`s were consumed earlier in
    `resolve_qualified`/`resolve_unqualified` and not threaded into `found`/`deduped`). Thread the
    `Location`'s URI through: change `found`/`entries`'s element type from `(String, (u8,u8))` to
    `(String, (u8,u8), String /* defining uri */)` (or a small local struct) in
    `collect_params_from_file`'s return and the two call sites that build `found`/`all`; use the
    defining URI (or `format!("{uri}#{name}", ...)` if a slightly richer identity is cheap at that
    point — implementer's call, no consumer inspects the payload yet) to build each `Fqn`.
  - The two early-bail "ubiquitous name" fast paths (`resolve_qualified` at `sig.rs:930-932`,
    `resolve_unqualified` at `sig.rs:1021-1023`) — deliberately skip scanning to avoid a multi-second
    stall (see the existing comments at those lines); no real candidates are available. Use
    `Resolution::Ambiguous(vec![])` there — document with a one-line comment: "known-ambiguous
    (ubiquitous name), candidates not enumerated for performance — see `total_definition_count`."

- [ ] **Step 3: Replace the enum.** Delete `SignatureResult` (`sig.rs:44-56`). Add `Signature` (Step
  0's shape). Change `resolve_call_signature`'s return type and every internal function that returns
  `SignatureResult` (`resolve_qualified`, `resolve_unqualified`, `build_result`, and
  `resolve_call_signature` itself — grep `-> SignatureResult` in `sig.rs` to enumerate all of them, do
  not rely on this list being exhaustive) to `Resolution<Signature>`, applying the Step-above mapping at
  every `return` site. `Resolution` and `Fqn` are already visible in `sig.rs` (same crate, `pub(crate)`)
  — add `use super::{Fqn, Resolution};` if not already imported via a glob.

- [ ] **Step 4: Update `sig_tests.rs` (9 sites).** Grep `SignatureResult` in the file first to get exact
  line numbers (they will have shifted from the audit's byte offsets). Pattern-translate:
  - `SignatureResult::Unique { params_text, param_counts } => (...)` → `Resolution::Resolved(Signature {
    params_text, param_counts }) => (...)`
  - `matches!(resolve_call_signature(&call, &idx), SignatureResult::Overloaded)` →
    `matches!(resolve_call_signature(&call, &idx), Resolution::Ambiguous(_))`
  - `SignatureResult::NotFound` (one site, asserting a same-package-test-helper miss) →
    `Resolution::Unresolved`
  - Any bare `other => panic!("... got {other:?}")` catch-all arms keep working once `Resolution<T>`
    derives `Debug` (Step 1).

- [ ] **Step 5: Update `call_arg_diagnostics.rs`.**
  - `sig_cache: HashMap<(String, Option<String>), SignatureResult>` → `HashMap<(String, Option<String>),
    Resolution<Signature>>` (2 occurrences: the field declaration and the function-parameter type, per
    the earlier grep at `call_arg_diagnostics.rs:46` and `:83`/`:116`).
  - The match at `call_arg_diagnostics.rs:166-174`:
    ```rust
    let (params_text, (required, total)) = match sig_result {
        Resolution::Resolved(Signature { params_text, param_counts }) => (params_text, param_counts),
        Resolution::Ambiguous(_) | Resolution::Unresolved => return None,
    };
    ```
  - Update the `use` at `call_arg_diagnostics.rs:14-16`: `SignatureResult` → `Signature`, `Resolution`
    now needed too (`use crate::indexer::{..., Resolution, Signature, ...}` — `Resolution` is already
    re-exported from `indexer.rs`; confirm it's in scope, it already is per `indexer.rs:61`).

- [ ] **Step 6: Update `src/indexer.rs`'s re-export block.** In the `sig::{...}` group
  (`indexer.rs:52-58`), replace `SignatureResult` with `Signature`. Leave everything else in that group
  unchanged.

- [ ] **Step 7: Run the full suite + clippy.**

Run: `cargo test --bin kmp-lsp`
Expected: all green — this is a type-rename-shaped migration, not a behavior change (verified: the one
external consumer already treated `Overloaded`/`NotFound`/`UnresolvableReceiver` identically).

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean. Watch specifically for an unused-import warning on `Fqn`/`Resolution` in `sig.rs` if
the `use` isn't threaded correctly, and for the now-required `T: Debug`/`T: Clone` bounds surfacing
anywhere `Resolution<T>` is used with a `T` that doesn't derive them (only `Signature`/`ResolvedType`
are instantiated today; both need `Clone`, and `Signature` needs `Debug` for the test panics above —
add derives as needed, matching `sig.rs`'s existing `#[derive(Debug, Clone)]` convention on nearby
types).

- [ ] **Step 8: Commit.**

```bash
git add -A
git commit -m "refactor(infer): migrate SignatureResult onto Resolution<Signature>, making Ambiguous real"
```

---

## Task 5: Final verification pass

- [ ] **Step 1: Full suite.**

Run: `cargo test --bin kmp-lsp`
Expected: all green (binary-only crate — `cargo test --lib` runs 0 tests, do not use it as a signal).

- [ ] **Step 2: Clippy, both the repo's stated gate and the stricter one.**

Run: `cargo clippy -- -D warnings` (AGENTS.md's baseline gate)
Run: `cargo clippy --all-targets --all-features -- -D warnings` (catches issues only visible with tests
compiled)
Expected: both clean.

- [ ] **Step 3: `cargo fmt` check** (pre-commit already runs this; confirm no drift before the final
  commit).

Run: `cargo fmt --check`
Expected: no diff. If it rewrites, `git add -A` and fold into the last commit's follow-up per repo
convention (pre-commit hook re-stage rule in `AGENTS.md`).

- [ ] **Step 4: Confirm no remaining `#[allow(dead_code)]` on `Fqn`/`Ambiguous` that should have been
  removed in Task 4.**

Run: `grep -n "allow(dead_code)" src/indexer/infer/mod.rs`
Expected: the line above `Fqn` and the line above `Ambiguous(Vec<Fqn>)` are gone; `resolved_ref`'s stays
(still genuinely unused — do not remove without a real caller).

- [ ] **Step 5: Grep for `SignatureResult` repo-wide — must be zero hits** (confirms the rename in Task
  4 is complete, not partial).

Run: `grep -rn "SignatureResult" src`
Expected: no matches.

---

## Testing & verification (gates, matching this repo's actual conventions)

- `cargo test --bin kmp-lsp` after every task (binary-only crate; `--lib` is not a signal — see
  `AGENTS.md` and the 2026-06-30 design doc's own "Testing & verification" section).
- `cargo clippy --all-targets --all-features -- -D warnings` before the final commit of the slice;
  `cargo clippy -- -D warnings` (AGENTS.md's simpler baseline) is also clean throughout since it's a
  subset of the stricter check.
- No new `TestDeps` methods needed — `with_method_return_for_type` (used by Task 2's test) and the
  existing var/field-type builders already cover what this slice needs.
- `find_referencing_symbols` (Serena) is the design doc's prescribed anti-reinvention check for
  enumerating consumers before routing/deleting; this session hit Serena MCP state issues from
  concurrent worktree use (per this repo's own memory notes on prior sessions), so consumer counts in
  this plan were measured with `grep -rlP` fallbacks instead — re-run `find_referencing_symbols` on
  `SignatureResult`/`resolve_call_signature`/`receiver_type`/`call_return_type` during implementation if
  Serena is healthy at that point, as a second check before Task 4/Task 5's final commits.
- Decoy check for Task 4 specifically (per this repo's "house decoy" convention): a test where two
  distinct-arity same-named functions exist (already covered by `sig_tests.rs`'s existing Overloaded
  tests at the lines identified in Task 4 Step 4) must still bail to `Ambiguous`, not silently pick one
  — this is exactly what Task 4's migration must not regress.

## Roadmap — what this slice deliberately does not attempt (for the next slice's context)

- Wire a first real consumer onto `CstQuery::receiver_type()`/`call_return_type()` (Task 1/2 land with
  unit tests only, per the Gap 1 scope call above). Natural next candidates once picked: any *new* CST
  consumer work (not a rewrite of `expr_type.rs`'s existing recursive internals).
- Fold `it_this.rs`'s position-based lambda/it/this family into `CstQuery` — needs a
  `CstQuery::at_position` bridge plus the design doc's promised `LambdaScope` extension (`this` field,
  `String`→`ReceiverType` upgrade). This is the design doc's own "collapse the lambda triad at the
  facade layer" follow-on, now that the engine layer is confirmed already CST-driven.
- `CstExpr` exhaustive dispatch, `RawTypeName`/`TypeName` split, construction-sealed `ReceiverType`/
  `ResolvedType` — design doc "step 5: Sweep," confirmed untouched, unattempted here.
- Generalizing `Resolution<T>::Ambiguous` from `Vec<Fqn>` to something richer — not needed for this
  slice (see the explicit scope call above); revisit only if a future consumer needs per-candidate data
  beyond an identifying `Fqn`.
