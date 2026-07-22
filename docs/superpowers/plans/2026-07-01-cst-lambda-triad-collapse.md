# CST Lambda-Triad Collapse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Introduce the `CstQuery<D: InferDeps>` struct as the home of CST resolution, then collapse the
three overlapping lambda/`it`/`this`/receiver mechanisms (`cst_lambda.rs` CST spine + `receiver.rs` text
+ `it_this.rs` line) into the single CST-driven walk, deleting the text/line structure-recovery
(~800–900 lines) while re-homing genuine heuristics.

**Architecture:** The CST path is already primary (`cst_it_or_this_type`/`locate_and_extract`) but has
text/line *fallbacks* "for cases the CST resolver can't handle yet." Collapse = a deletion *sequence*:
each step strengthens the CST path to subsume one fallback (with a decoy test), then deletes that
now-dead fallback. The deletions hang off `CstQuery`, which replaces the threaded `(node, doc, idx, uri)`
param bundle.

**Tech Stack:** Rust, tree-sitter, `tower_lsp`, binary-only crate (`kmp-lsp`). Branch
`refactor/cst-resolution-s2` (stacked on `refactor/cst-resolution` / PR #196).

## Global Constraints

- `cargo test --bin kmp-lsp` (binary-only crate). No `unwrap()`/`expect()` in production. No abbreviations.
  Tests in companion `*_tests.rs`. `#[warn(unreachable_pub)]` on → new items `pub(crate)`.
- Pre-commit fmt+clippy; if fmt rewrites, `git add -A` and re-commit.
- **Move-don't-rewrite.** **Each step deletes its now-dead functions** (`find_referencing_symbols` = 0
  before close) AND ends with `cargo test --bin kmp-lsp` green.
- **Behaviour-preserving for hover / inlay / completion lambda typing** — the existing lambda/completion/
  inlay suites are the net; where a step changes a path, add a decoy test FIRST; never edit a test to
  force green.
- `TestDeps` must keep driving the engine — `CstQuery` is generic over `D: InferDeps`, holds `&D` (NOT
  `&Indexer`).
- Full triad map (call graph, per-fn classification, blockers): `.superpowers/sdd/lambda-triad-map.md`.

---

## Task 1: Introduce `CstQuery<'a, D: InferDeps>`; migrate `expr_type` onto it

**Files:**
- Modify: `src/indexer/infer/mod.rs` (add `CstQuery`; keep/redefine `expr_type` as a method)
- Modify: callers of `Indexer::expr_type` — `src/semantic_tokens/resolve.rs` (the one routed in Phase 1)
- Test: `src/indexer/infer/mod_tests.rs`

**Interfaces:**
- Consumes: `infer_expr_type(node, bytes, deps, uri) -> Option<String>` (Phase 1, full-kind); `LiveDoc`
  (`src/indexer/live_tree.rs`); `ResolveIo`, `Resolution<T>`, `ResolvedType` (Phase 1, in `infer/mod.rs`).
- Produces:
  ```rust
  pub(crate) struct CstQuery<'a, D: InferDeps> {
      node: Node<'a>, doc: &'a LiveDoc, deps: &'a D, uri: &'a Url, io: ResolveIo,
  }
  impl<'a, D: InferDeps> CstQuery<'a, D> {
      pub(crate) fn new(node: Node<'a>, doc: &'a LiveDoc, deps: &'a D, uri: &'a Url, io: ResolveIo) -> Self;
      fn at(&self, node: Node<'a>) -> Self;   // walk step: Self { node, ..*self }
      pub(crate) fn expr_type(&self) -> Resolution<ResolvedType>;
  }
  ```
  `expr_type` wraps `infer_expr_type(self.node, &self.doc.bytes, self.deps, self.uri)` →
  `Some(s) => Resolved(ResolvedType::from_inferred(s))`, `None => Unresolved` (identical mapping to the
  Phase-1 `CstResolve::expr_type`, just relocated onto the struct). Keep the Phase-1
  `CstResolve`/`CstCtx` defined for now ONLY if other code still uses them; otherwise delete them in this
  task (the only caller is semantic_tokens, migrated below) — `find_referencing_symbols` to confirm.

- [ ] **Step 1: Write the failing test** — `CstQuery::new(int_literal_node, &doc, &index, &uri,
  ResolveIo::IndexOnly).expr_type().resolved()` returns `Some("Int")`. (Mirror the existing Phase-1
  `cst_resolve_expr_type_resolves_int_literal` test in `mod_tests.rs`, adapted to the struct.)
- [ ] **Step 2: Run it, verify it fails** (`CstQuery` undefined).
  Run: `cargo test --bin kmp-lsp cst_query`  Expected: FAIL (compile).
- [ ] **Step 3: Define `CstQuery` + `new` + `at` + `expr_type` in `mod.rs`**, generic over `D: InferDeps`,
  holding `&'a LiveDoc` and `&'a D`. Re-export from `mod.rs`. Zero logic beyond the wrap.
- [ ] **Step 4: Migrate the `semantic_tokens` caller.** In `src/semantic_tokens/resolve.rs`, replace
  `indexer.expr_type(node, &ctx)` (the Phase-1 call) with
  `CstQuery::new(node, doc, indexer, uri, ResolveIo::IndexOnly).expr_type()` (same `.resolved().map(|t|
  t.as_type_str().to_owned())` mapping). Delete the now-unused Phase-1 `CstResolve` trait + `CstCtx` if no
  other callers remain (`find_referencing_symbols`).
- [ ] **Step 5: Run focused + full suite.**
  Run: `cargo test --bin kmp-lsp -- semantic` then `cargo test --bin kmp-lsp`  Expected: green (≈1441).
- [ ] **Step 6: Commit.**
  `git commit -m "refactor(infer): introduce CstQuery struct; move expr_type onto it"`

---

## Deletion sequence (subsequent tasks — each its own task, detailed just-in-time)

From `.superpowers/sdd/lambda-triad-map.md`. Each builds the next catalogue method (`receiver_type`,
`lambda_scope`) onto `CstQuery` as needed, strengthens the CST path to subsume a fallback, then deletes
it. Ordered by risk:

| Task | What | ~Del | Risk |
|---|---|---|---|
| 2 | Delete `find_this_context_text` (no-CST `this` fallback; CST `classify_this_lambda_context` subsumes) | 54 | LOW |
| 3 | Redirect `completion.rs:266` off `find_it_element_type`; delete `find_it_element_type` | 9 | LOW-MED |
| 4 | Extend `cst_forward_resolve_receiver_type` for collection fns; drop `lambda_receiver_type_from_context` call in `cst_it_or_this_type:529` | 8 | MED |
| 5 | Delete named-param text-scan fallback in `find_named_lambda_param_type*` | 55 | MED |
| 6 | Delete `find_it_element_type_in_lines_impl` text body (needs live_doc always present) | 70 | HIGH |
| 7 | Rewrite `completion_context.build_lambda_scope` to CST scope-walk (add `lambda_scope` on `CstQuery`); drop direct `lambda_receiver_type_from_context` call (`completion_context.rs:296`) | 30 | HIGH |
| 8 | Move 4 leaf fns (`uppercase_dotted_type_prefix`→chain, `resolve_call_params`/`fun_trailing_lambda_this_type`→cst_lambda); delete rest of `receiver.rs`; convert 14 `indexer_tests.rs` calls to CST | 480 | HIGH |
| 9 | Delete `lambda_receiver_type_named_arg_ml` + call sites; dead constants/structural helpers | 160 | MED/LOW |

**Blockers (clear before Task 8):** `chain.rs:14`, `cst_lambda.rs:{129,176,255,891}`,
`completion_context.rs:296`, `completion.rs:266`, 14 `indexer_tests.rs` calls — all enumerated in the map.
HIGH-risk back half (6–8) pauses for human review before proceeding.

See `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md` for the design + principle
(CST is the source of structure; delete text/line structure-recovery; keep genuine heuristics re-homed).
