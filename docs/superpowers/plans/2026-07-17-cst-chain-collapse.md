# CST Chain Collapse + Repair-Seam Hoist Implementation Plan (Slice 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the last text-splitting chain resolver in `chain.rs` (with a strictness gate so the node walk doesn't get *looser*), and hoist the brace-repair seam into `speculative.rs`, wiring the three unprotected lambda-family consumers.

**Architecture:** The `KIND_NAV_EXPR` arm of `resolve_root_node_type` redirects to the existing node-native segment walk, gated by a new typed `SuffixStrictness` (nav position = `Fail` on unresolved suffixes, receiver positions keep today's `LeakReceiver`). `LambdaResolutionDoc` moves to `speculative.rs` as the transform-agnostic `ResolutionDoc` with seam `lambda_doc_at`; the scope walk, `lambda_params_at_col`, and the named-param resolver get mid-typing resilience. Spec: `docs/superpowers/specs/2026-07-17-cst-chain-collapse-design.md`.

**Tech Stack:** Rust, tree-sitter, existing `InferDeps`/`TestDeps` seam.

## Global Constraints

- Branch `refactor/cst-chain-collapse` → PR to `refactor/unified-resolution`. Suite + pre-commit clippy per commit; final: both clippy profiles, e2e smoke, live probe.
- Decoy discipline: concrete-type assertions with `assert_ne!` against bare generic params; `None`-asserting decoys for the two strictness cases (they bite on index gaps the suite doesn't otherwise model).
- Never resurrect text scanning; if the node walk lacks a capability a test needs, extend the walk.
- Repair stays bounded (`MAX_BRACE_REPAIRS`) and self-verifying; the hoist is behavior-neutral for the two existing consumers.
- The resolver-side `*_in_lines` family (`resolver/infer_lines.rs`) is EXCLUDED from renames — its lines are load-bearing.
- Cheap-agent dispatch for mechanical batches (user directive); controller reviews diffs and runs gates.

---

### Task 1: `SuffixStrictness` gate in the forward walk

**Files:**
- Modify: `src/indexer/infer/chain.rs` (`forward_resolve_segments` ~97-207, `resolve_segments_type` ~524-542, caller sites :229 and :424)
- Test: `src/indexer/infer/mod_tests.rs` (has `super::`-level access to `chain`)

**Interfaces:**
- Produces:
```rust
/// How an unresolvable navigation suffix is handled during a forward walk.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SuffixStrictness {
    /// Receiver-position semantics: an unresolved suffix leaves the receiver
    /// type in place (best-effort; the caller probes members next).
    LeakReceiver,
    /// Expression-position semantics: an unresolved suffix fails the walk —
    /// the expression's own type is unknown (matches the deleted text
    /// walker's per-segment `?`).
    Fail,
}
pub(super) fn forward_resolve_segments(segments, bytes, deps, uri, strictness: SuffixStrictness) -> Option<(String, String)>
pub(super) fn resolve_segments_type(segments, bytes, deps, uri, strictness: SuffixStrictness) -> Option<String>
```
- Consumes: existing `NavSegment`, `resolve_member_type_on`, `SCOPE_FUNCTIONS`.

- [ ] **Step 1: Write the failing decoy** (in `mod_tests.rs`; check its existing helpers for `TestDeps` + `parse_live` usage and mirror them)

```rust
/// Strictness decoy: an unresolved FINAL member must fail the walk under
/// `Fail` (the old text walker's per-segment `?`), while `LeakReceiver`
/// keeps today's receiver-position best-effort.
#[test]
fn unresolved_final_suffix_fails_the_strict_walk() {
    use super::chain::{collect_nav_segments, resolve_segments_type, SuffixStrictness};
    use crate::indexer::live_tree::parse_live;

    let deps = super::deps::TestDeps::new().with_var("wrapper", "Wrapper");
    // NOTE: no field `unknownField` on Wrapper.
    let doc = parse_live("fun f() { wrapper.unknownField }", tree_sitter_kotlin::language()).unwrap();
    let uri = tower_lsp::lsp_types::Url::parse("file:///t/T.kt").unwrap();
    let nav = find_first_node_of_kind(doc.tree.root_node(), "navigation_expression");
    let segments = collect_nav_segments(nav, &doc.bytes);

    assert_eq!(
        resolve_segments_type(&segments, &doc.bytes, &deps, &uri, SuffixStrictness::Fail),
        None,
        "unknown member must not leak the receiver's type"
    );
    assert_eq!(
        resolve_segments_type(&segments, &doc.bytes, &deps, &uri, SuffixStrictness::LeakReceiver)
            .as_deref(),
        Some("Wrapper"),
        "receiver-position semantics unchanged"
    );
}
```

Add a local `find_first_node_of_kind(root, kind) -> Node` helper (recursive walk) if `mod_tests.rs` doesn't have one; check `TestDeps` for the exact `with_var` builder name (grep `fn with_var` in `deps.rs` — if the builder is named differently, use the real one).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp unresolved_final_suffix 2>&1 | tail -5`
Expected: compile error — `SuffixStrictness` not defined / wrong arity.

- [ ] **Step 3: Implement**

In `forward_resolve_segments`: add the `strictness` parameter; in the `NavSegment::Suffix` branch replace

```rust
                last_suffix_resolved = false;
                if let Some(ref cur) = current_type {
                    if let Some(resolved) = resolve_member_type_on(cur, name, deps, uri) {
                        current_type = Some(resolved);
                        last_suffix_resolved = true;
                    } else if SCOPE_FUNCTIONS.contains(&name.as_str()) {
                        // Scope function: receiver type flows through.
                    }
                }
```

with

```rust
                last_suffix_resolved = false;
                if let Some(ref cur) = current_type {
                    if let Some(resolved) = resolve_member_type_on(cur, name, deps, uri) {
                        current_type = Some(resolved);
                        last_suffix_resolved = true;
                    } else if SCOPE_FUNCTIONS.contains(&name.as_str()) {
                        // Scope function: receiver type flows through.
                    } else if strictness == SuffixStrictness::Fail {
                        return None;
                    }
                } else if strictness == SuffixStrictness::Fail {
                    return None;
                }
```

(The second arm also fails the walk when there is no receiver type at all — an unresolved root followed by a suffix that then failed to resolve is covered by the first arm; a root that resolved to nothing ends the strict walk immediately.)

`resolve_segments_type` gains and forwards the parameter. Update the two existing callers with `SuffixStrictness::LeakReceiver`:
- chain.rs:229 (`cst_forward_resolve_receiver_type` — receiver position),
- chain.rs:424 (`resolve_call_expr_type`'s `segments[..len-1]` receiver typing).

- [ ] **Step 4: Run** `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result"` — full suite green (existing behavior pinned by LeakReceiver at both call sites).

- [ ] **Step 5: Commit** `git add -A src/ && git commit -m "feat(infer): typed SuffixStrictness gate on the forward chain walk"`

---

### Task 2: Nav-arm redirect; delete the text walker

**Files:**
- Modify: `src/indexer/infer/chain.rs` (`resolve_root_node_type` nav arm ~393-396; delete `resolve_dotted_text_type` ~558-580 and `uppercase_dotted_type_prefix` ~547-556; fix the stale comment at ~278 "matching the text path's `extract_collection_element_type`" — drop the text-path reference, keep the type-keyed rationale)
- Test: `src/indexer/infer/mod_tests.rs`, `src/indexer/infer/it_this_tests.rs`

**Interfaces:**
- Consumes: Task 1's `resolve_segments_type(..., SuffixStrictness)`.
- Produces: nothing new — deletions plus the redirect.

- [ ] **Step 1: Write the failing decoys**

In `mod_tests.rs` (strict nav-position semantics through the real arm):

```rust
/// Unknown ROOT decoy: `resolve_root_node_type` falls back to `Some(name)`
/// for an unresolvable root ident; combined with a leaking walk this used to
/// be able to resolve a nav to the literal root string. The strict nav arm
/// must yield None instead.
#[test]
fn unknown_root_nav_expression_resolves_to_none() {
    use super::chain::resolve_root_node_type;
    use crate::indexer::live_tree::parse_live;

    let deps = super::deps::TestDeps::new(); // nothing indexed at all
    let doc = parse_live("fun f() { foo.bar }", tree_sitter_kotlin::language()).unwrap();
    let uri = tower_lsp::lsp_types::Url::parse("file:///t/T.kt").unwrap();
    let nav = find_first_node_of_kind(doc.tree.root_node(), "navigation_expression");

    assert_eq!(resolve_root_node_type(nav, &doc.bytes, &deps, &uri), None);
}
```

In `it_this_tests.rs` (generic-survival decoy, end-to-end through `it` typing — mirror the file's existing fixture style with a real `Indexer`):

```rust
/// Generics-survival decoy for the nav-arm redirect: a chain ROOTED at a
/// navigation expression whose type is generic must keep its type args —
/// the deleted text walker stripped them at exit, collapsing List<Product>
/// to List and killing element extraction.
#[test]
fn nav_rooted_generic_chain_keeps_element_type_for_it() {
    let (uri, idx) = indexed(
        "/W.kt",
        "package p\n\
         class Product { val price: Int = 0 }\n\
         class Wrapper { val items: List<Product> = listOf() }\n\
         fun f(wrapper: Wrapper) {\n\
             wrapper.items.map { it }\n\
         }\n",
    );
    let pos = crate::types::CursorPos { line: 4, utf16_col: 25 }; // inside the lambda
    let resolved = find_it_element_type_in_lines(&lines_of(&idx, &uri), pos, &idx, &uri);
    assert_eq!(resolved.as_deref(), Some("Product"));
    assert_ne!(resolved.as_deref(), Some("T"), "bare type param must never leak");
}
```

(Adapt helper names — `indexed`, `lines_of` — to what `it_this_tests.rs` actually provides; the existing tests at the top of the file show the pattern. If this scenario already passes before the redirect via a different path, KEEP the test as a pin and note it in the commit message.)

- [ ] **Step 2: Run to verify** — `cargo test --bin kmp-lsp unknown_root_nav 2>&1 | tail -3` — expected FAIL (arm still routes to the text walker, which for an unindexed `foo` root returns None already — if it passes pre-change, verify it STILL passes post-change; the generic decoy is the one expected to flip).

- [ ] **Step 3: Implement the redirect**

```rust
        k if k == KIND_NAV_EXPR => {
            let segments = collect_nav_segments(node, bytes);
            resolve_segments_type(&segments, bytes, deps, uri, SuffixStrictness::Fail)
        }
```

Delete `resolve_dotted_text_type` and `uppercase_dotted_type_prefix`; remove their imports/uses (`cargo build` finds stragglers). Fix the chain.rs:278 comment.

- [ ] **Step 4: Run the full suite** — `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result|FAILED"`. Any failure here is a real capability the walk lacks — extend the walk (e.g. root resolution), never resurrect text. Timebox: if a gap needs design, stop and flag.

- [ ] **Step 5: Commit** `git add -A src/ && git commit -m "refactor(infer): nav arm resolves via the segment walk — text chain walker deleted"`

---

### Task 3: Hoist the repair seam into `speculative.rs`

**Files:**
- Modify: `src/indexer/infer/speculative.rs` (receives `ResolutionDoc`, `LambdaTreeGate`, `lambda_tree_gate`, `repaired_doc_at`, `lambda_doc_at`; extend the module doc to cover both transforms)
- Modify: `src/indexer/infer/it_this.rs` (delete the moved items; `find_it_element_type_in_lines` + `find_this_context_in_lines` call `speculative::lambda_doc_at`; `MAX_BRACE_REPAIRS` moves with `repaired_doc_at`)
- Test: existing repair tests in `it_this_tests.rs` are the neutrality net — no new tests in this task.

**Interfaces:**
- Produces (later tasks consume these exact names):
```rust
// speculative.rs
pub(crate) enum ResolutionDoc {
    /// The tree from `Indexer::live_doc_or_parse` — authoritative.
    Parsed(std::sync::Arc<LiveDoc>),
    /// An append-only brace-repaired transient reparse (never cached).
    Repaired(LiveDoc),
}
impl ResolutionDoc { pub(crate) fn doc(&self) -> &LiveDoc }
pub(crate) fn lambda_doc_at(idx: &Indexer, uri: &Url, pos: CursorPos) -> Option<ResolutionDoc>
```

- [ ] **Step 1: Move the code.** Cut `LambdaResolutionDoc` (renaming to `ResolutionDoc`), `LambdaTreeGate`, `lambda_tree_gate`, `repaired_doc_at`, `lambda_resolution_doc_at` (renaming to `lambda_doc_at`), and `MAX_BRACE_REPAIRS` from `it_this.rs` into `speculative.rs`. Visibility: `pub(crate)` for `ResolutionDoc`/`doc()`/`lambda_doc_at`; the gate + repair stay private to the module. `lambda_doc_at` needs `&Indexer` — `speculative.rs` may not import `Indexer` yet; add `use crate::indexer::Indexer;` (it's within the same crate module tree — `cursor_node_at` and `lang_for_path`/`parse_live` imports come along with the moved code). Update `it_this.rs` call sites (`:181`, `:201`) to `use super::speculative::{lambda_doc_at, ResolutionDoc};` — the match arms rename mechanically. Update speculative.rs's module doc: it now hosts BOTH healed-doc constructors (marker insertion for receiver derivation; brace repair for lambda resolution) — co-located, not merged.

- [ ] **Step 2: Run the full suite** — `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result"`. Every existing repair test must pass unchanged (the move is behavior-neutral).

- [ ] **Step 3: Commit** `git add -A src/ && git commit -m "refactor(infer): hoist brace-repair seam into speculative.rs as ResolutionDoc"`

---

### Task 4: Wire the scope walk

**Files:**
- Modify: `src/features/completion_context.rs` (`collect_lambda_scopes` ~270-282)
- Test: `src/features/completion_context_tests.rs`

**Interfaces:**
- Consumes: `speculative::lambda_doc_at` (re-export through `src/indexer.rs`'s existing `pub(crate) use self::infer::speculative::{...}` line — add `lambda_doc_at` and `ResolutionDoc` to it).

- [ ] **Step 1: Write the failing test** (multi-line, unclosed `{` — the broken tree forms no `lambda_literal`, so today the scope stack comes back empty)

```rust
#[test]
fn scope_walk_survives_an_unclosed_lambda() {
    let src = "package com.example\n\
               class Item { val price: Int = 0 }\n\
               fun main(items: List<Item>) {\n\
               \x20   items.forEach {\n\
               \x20       \n"; // lambda AND function braces both unclosed
    let (uri, index) = indexed_with_live("/Unclosed.kt", src);
    let scope = ScopeContext::build(Position::new(4, 8), &index, &uri);
    assert_eq!(
        scope.resolve_receiver("it"),
        Some("Item"),
        "brace repair must recover the lambda scope stack mid-typing"
    );
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --bin kmp-lsp scope_walk_survives 2>&1 | tail -5`. Expected: assertion failure (`None`). If it PASSES, the `it_type` merge in `ScopeContext::build` (via the already-repaired `infer_lambda_param_type_at` path) is masking the walk — strengthen the assertion to check `scope.lambda_scopes` is non-empty instead, and note which signal proved the gap.

- [ ] **Step 3: Implement**

```rust
fn collect_lambda_scopes(index: &Indexer, uri: &Url, position: Position) -> Vec<LambdaScope> {
    let cursor = CursorPos {
        line: position.line as usize,
        utf16_col: position.character as usize,
    };
    let Some(resolution) = lambda_doc_at(index, uri, cursor) else {
        return Vec::new();
    };
    let doc = resolution.doc();
    let Some(node) = cursor_node_at(doc, cursor) else {
        return Vec::new();
    };
    CstQuery::new(node, doc, index, uri, ResolveIo::NoRg)
        .lambda_scope()
        .into_iter()
        .map(LambdaScope::from)
        .collect()
}
```

- [ ] **Step 4: Run** — target test passes; full suite green.

- [ ] **Step 5: Commit** `git add -A src/ && git commit -m "feat(complete): scope walk resolves against the repaired tree mid-typing"`

---

### Task 5: `lambda_params_at_col` broken-tree gate

**Files:**
- Modify: `src/indexer/scope.rs` (`cst_lambda_params_at_col` ~299-317)
- Test: `src/indexer/scope_tests.rs`

**Interfaces:** self-contained (restores the existing text-scan fallback in broken states).

- [ ] **Step 1: Write the failing test** (multi-line — the single-line case is masked by `is_lambda_param`'s same-line text check)

```rust
#[test]
fn lambda_params_fall_back_to_the_text_scan_on_a_broken_tree() {
    // Unclosed lambda: the CST forms no lambda_literal, so the CST path
    // used to answer Some(vec![]) and short-circuit the text fallback.
    let src = "fun f(items: List<Item>) {\n    items.map { item ->\n        \n";
    let (uri, indexer) = indexed_with_live(src); // adapt to the file's fixture helper
    let params = indexer.lambda_params_at_col(&uri, 2, 8);
    assert!(
        params.iter().any(|p| p == "item"),
        "broken tree must fall through to the text scan; got {params:?}"
    );
}
```

(Check `scope_tests.rs`'s fixture helpers for how live docs are stored — the test needs `store_live_tree` so `live_doc` finds a tree.)

- [ ] **Step 2: Run to verify failure** — expected: empty `params`.

- [ ] **Step 3: Implement** — in `cst_lambda_params_at_col`, after `let params = collect_cst_lambda_params(node, &doc.bytes);` (inline the current tail expression into a binding):

```rust
        let params = collect_cst_lambda_params(node, &doc.bytes);
        if params.is_empty() && doc.tree.root_node().has_error() {
            // A broken tree may simply have failed to FORM the enclosing
            // lambda_literal (unclosed `{`); empty here does not mean "no
            // params" — let the caller fall through to the text scan.
            return None;
        }
        Some(params)
```

- [ ] **Step 4: Run** — target + full suite.

- [ ] **Step 5: Commit** `git add -A src/ && git commit -m "fix(scope): broken-tree lambda-param CST miss falls through to the text scan"`

---

### Task 6: Wire the named-param resolver + end-to-end pipeline test

**Files:**
- Modify: `src/indexer/infer/it_this.rs` (`find_named_lambda_param_type` ~231-240)
- Test: `src/indexer/infer/it_this_tests.rs` (unit) + `src/resolver/tests.rs` (pipeline)

**Interfaces:**
- Consumes: `speculative::lambda_doc_at` (Task 3).

- [ ] **Step 1: Write the failing unit test** (multi-line, unclosed)

```rust
#[test]
fn named_param_type_resolves_in_an_unclosed_lambda() {
    let (u, idx) = indexed(
        "/N.kt",
        "class Item { val price: Int = 0 }\nfun f(items: List<Item>) {\n    items.map { item ->\n        \n",
    );
    // store the live tree so live_doc_or_parse sees the broken state
    // (adapt to the fixture; index_content + store_live_tree)
    let pos = crate::types::CursorPos { line: 3, utf16_col: 8 };
    assert_eq!(
        find_named_lambda_param_type("item", pos, &idx, &u).as_deref(),
        Some("Item"),
        "brace repair must recover the named param's type mid-typing"
    );
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

```rust
pub(crate) fn find_named_lambda_param_type(
    param_name: &str,
    pos: CursorPos,
    idx: &Indexer,
    uri: &Url,
) -> Option<String> {
    let resolution = super::speculative::lambda_doc_at(idx, uri, pos)?;
    cst_named_lambda_param_type(pos, param_name, resolution.doc(), idx, uri)
}
```

(Update the doc comment: repair-protected like the `it`/`this` resolvers.)

- [ ] **Step 4: Write the end-to-end pipeline test** (in `resolver/tests.rs`, exercising Tasks 4+5+6 together through `run_completions`)

```rust
/// Mid-typing named-param completion: multi-line lambda with BOTH braces
/// unclosed. Exercises the whole repaired path: lambda_params_at_col
/// fall-through (broken-tree gate) → is_lambda_param → complete_lambda_dot →
/// find_named_lambda_param_type (repair-wired).
#[test]
fn named_param_completion_survives_unclosed_lambda() {
    let idx = Indexer::new();
    let app_uri = Url::parse("file:///app/U.kt").unwrap();
    let src = "package app\n\
               class Item { val price: Int = 0 }\n\
               fun f(items: List<Item>) {\n\
               \x20   items.map { item ->\n\
               \x20       item.\n";
    idx.index_content(&app_uri, src);
    idx.store_live_tree(&app_uri, src);
    let (items, _) = crate::features::completion::run_completions(
        &idx,
        &app_uri,
        tower_lsp::lsp_types::Position::new(4, 13),
        false,
    );
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.starts_with("price")),
        "named-param completion in the broken state — got: {labels:?}"
    );
}
```

- [ ] **Step 5: Run** — both tests + full suite green.

- [ ] **Step 6: Commit** `git add -A src/ && git commit -m "feat(infer): named-param resolution repair-wired; end-to-end mid-typing completion test"`

---

### Task 7: Rename carry-in — drop the vestigial `_lines` params (cheap-agent dispatch)

**Files:**
- Modify: `src/indexer/infer/it_this.rs` (3 public fns + doc comments), ~26 production call sites, test files incl. the two shims
- Test: full suite (mechanical change).

**Interfaces:** renames only:
- `find_it_element_type_in_lines(lines, pos, idx, uri)` → `find_it_element_type(pos, idx, uri)`
- `find_this_context_in_lines(pos, idx, uri)` → `find_this_context(pos, idx, uri)` (check whether it still has a `_lines` param — drop only what exists)
- `find_this_element_type_in_lines(...)` → `find_this_element_type(...)`

Dispatch to a subagent with these EXACT pre-handling instructions:
1. FIRST rename the two test shims `fn find_it_element_type` (`src/indexer/infer/it_this_tests.rs:27`, `src/indexer/scope_tests.rs:25`) to `fn it_type_at_line_end` (update their local callers) — otherwise the production rename shadows them.
2. Then rename the three production fns, dropping their `_lines`/`lines` parameters (verify each param really is unused in the body — if any is read, STOP and report instead of renaming that fn).
3. Update every call site (`grep -rn "_in_lines" src/ --include='*.rs'` — EXCLUDE `resolver/infer_lines.rs` and any `resolver/`-side function; those lines are load-bearing).
4. Update the re-export list in `src/indexer.rs` (the `it_this::{...}` use block).
5. Delete the now-false "`_lines` is vestigial" doc notes on the renamed fns.
6. `cargo test --bin kmp-lsp` must be green; do not commit.

- [ ] **Step 1: Dispatch the agent** with the instructions above.
- [ ] **Step 2: Review the diff** (`git diff --stat` + spot-check the shims and one call site per file).
- [ ] **Step 3: Run** the full suite + `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 4: Commit** `git add -A src/ && git commit -m "refactor(infer): drop vestigial _lines params from the it/this resolver family"`

---

### Task 8: Nested-generic-`it` completion test (ledger minor)

**Files:**
- Test: `src/indexer/infer/it_this_tests.rs`

- [ ] **Step 1: Write the test**

```rust
/// Ledger minor (2026-07-04 wave): `it` over a nested-generic element.
/// `List<Optional<Foo>>` → `it` is `Optional<Foo>`; `it.getOrNull()?.`
/// member access must reach Foo's members, not leak `T`.
#[test]
fn nested_generic_it_resolves_to_the_concrete_inner_type() {
    let (u, idx) = indexed(
        "/G.kt",
        "class Foo { val bar: Int = 0 }\n\
         class Optional<T> { fun getOrNull(): T? = null }\n\
         fun f(items: List<Optional<Foo>>) {\n\
             items.map { it }\n\
         }\n",
    );
    let pos = crate::types::CursorPos { line: 3, utf16_col: 17 };
    let resolved = find_it_element_type(pos, &idx, &u); // post-Task-7 name
    assert_eq!(resolved.as_deref(), Some("Optional<Foo>"));
    assert_ne!(resolved.as_deref(), Some("T"));
}
```

- [ ] **Step 2: Run it.** If it FAILS: do NOT fix inference in this slice — mark `#[ignore = "known gap: nested-generic it (ledger 2026-07-17)"]`, record in the ledger, and flag in the PR. If it passes, it's a free pin.
- [ ] **Step 3: Commit** `git add -A src/ && git commit -m "test(infer): pin (or flag) nested-generic it element typing"`

---

### Task 9: Gates, live probe, PR

- [ ] **Step 1:** `cargo test 2>&1 | tail -3 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -2 && cargo clippy --release --all-targets -- -D warnings 2>&1 | tail -2 && cargo test --test lsp_smoke 2>&1 | tail -3`
- [ ] **Step 2:** `cargo build`, then run the live probe (`scratchpad/lsp_probe_cst.py`, BIN at the worktree `target/debug/kmp-lsp`) — scenarios A/B/C must stay green (C exercises the broken-state pipeline this slice touches).
- [ ] **Step 3:** Push (`--no-verify` only for the known false-positive hook patterns; fix any GENUINE new violations first), open PR → `refactor/unified-resolution`. PR body: deletions (2 fns), the strictness gate + its two decoys, the three repair wires with the multi-line RED tests, rename summary, nested-generic-it disposition, probe results.
- [ ] **Step 4:** Ledger entry + memory update (`cst-resolution-unification`: slice 4 done; remaining = 5-remainder, 6).

## Self-review notes

- Spec coverage: §A = Tasks 1-2 (incl. deltas 4-5 as the gate + decoys); §B = Tasks 3-6 (three wires; move-neutrality in Task 3); §C = Tasks 7-8 + comment fixes folded into Tasks 2/3/7; EOF-remap carry-in dispositioned in the spec (no task, per spec). Testing section = decoys in 1-2, RED tests in 4-6, probe in 9.
- Type consistency: `SuffixStrictness` (Tasks 1-2), `ResolutionDoc`/`lambda_doc_at` (Tasks 3-6), post-rename `find_it_element_type` used by Task 8 (runs after Task 7).
- Known judgment points: Task 2 Step 4 gap-handling (extend walk vs stop-and-flag); Task 4 Step 2's masking check; fixture-helper names in test files must be adapted to what each file provides.
