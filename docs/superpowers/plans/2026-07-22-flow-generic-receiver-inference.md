# Flow&lt;T&gt; Generic Receiver-Type Inference — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three independently-necessary gaps in receiver-type inference so a value derived from a JAR-defined generic type (`Flow<T>`, via `.first()` or `.map { it -> }`) resolves to its concrete type argument instead of `None` — the root cause of a live-probed rename failure ("identity is ambiguous") on real Kotlin/Android code.

**Architecture:** Three self-contained patches, each mirroring an existing working pattern already in the same file it modifies. No shared code between the three fixes; each has its own task, its own test, and can be reviewed independently. A final task re-runs a live LSP probe against the real project the bug was found in.

**Tech Stack:** Rust, tree-sitter-backed CST inference (`src/indexer/infer/`), the `InferDeps` trait (`src/indexer/infer/deps.rs`) implemented by `Indexer` (`src/indexer.rs`).

## Global Constraints

- Design source of truth: `docs/superpowers/specs/2026-07-22-flow-generic-receiver-inference-design.md`. Do not re-derive reasoning already settled there — the design went through an independent critique that corrected a genuinely unsafe first draft; follow the corrected version exactly.
- **Do NOT modify `forward_resolve_segments` or `resolve_segments_type`** (`src/indexer/infer/chain.rs`) — their existing "resolve every segment I'm handed" contract is used by other callers and is directly tested by `unresolved_final_suffix_fails_the_strict_walk` (`src/indexer/infer/mod_tests.rs:133-162`), which must keep passing unchanged.
- No abbreviated identifiers (AGENTS.md project rule).
- Every existing test must keep passing. Run the FULL suite (`cargo test`) at the end of every task, not just the module you touched — these are shared inference primitives with callers across `resolver/`, `indexer/infer/`, and the feature layer.
- Each fix is independently reviewable; commit each task separately.

---

### Task 1: `find_var_type` gets a CST fallback for chained receivers

**Files:**
- Modify: `src/resolver/infer.rs` (`infer_variable_type_from_cst`'s visibility)
- Modify: `src/indexer.rs` (`InferDeps::find_var_type`, around line 407-409)
- Test: `src/indexer_tests.rs`

**Interfaces:**
- Consumes: `infer_variable_type_from_cst(indexer: &Indexer, name: &str, uri: &Url) -> Option<String>` (already exists in `src/resolver/infer.rs`, currently module-private).
- Produces: no new public interface — `InferDeps::find_var_type` gains a fallback; its signature is unchanged (`fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String>`).

- [ ] **Step 1: Write the failing test**

Add to `src/indexer_tests.rs` (append near the other `find_var_type`/`infer_variable_type_raw`-adjacent tests, e.g. after the test at line ~2269 shown in the file today):

```rust
#[test]
fn find_var_type_resolves_a_chained_call_initializer_via_cst_fallback() {
    // `val userData = repository.userData.first()` -- a chained (nav_expr)
    // receiver that the line-based heuristics in infer_variable_type_raw
    // explicitly reject (parser.rs's call_expr_receiver_method rejects
    // multi-level chaining; the resolver/infer.rs line-scan fallback skips
    // any receiver containing '.'). Only the CST-aware fallback
    // (infer_variable_type_from_cst) can type this.
    let idx = Indexer::new();
    let repo_uri = uri("/Repository.kt");
    idx.index_content(
        &repo_uri,
        concat!(
            "package com.example\n",
            "data class UserData(val shouldHideOnboarding: Boolean)\n",
            "class Repository {\n",
            "    val userData: UserData = TODO()\n",
            "    fun use() {\n",
            "        val local = this.userData\n",
            "        val hasOnboarded = local.shouldHideOnboarding\n",
            "    }\n",
            "}\n",
        ),
    );
    idx.store_live_tree(
        &repo_uri,
        concat!(
            "package com.example\n",
            "data class UserData(val shouldHideOnboarding: Boolean)\n",
            "class Repository {\n",
            "    val userData: UserData = TODO()\n",
            "    fun use() {\n",
            "        val local = this.userData\n",
            "        val hasOnboarded = local.shouldHideOnboarding\n",
            "    }\n",
            "}\n",
        ),
    );

    let var_type = idx.find_var_type("local", &repo_uri);
    assert_eq!(
        var_type.as_deref(),
        Some("UserData"),
        "find_var_type must resolve a chained (this.userData-style) initializer \
         via the CST fallback when the line heuristic can't -- got {var_type:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test find_var_type_resolves_a_chained_call_initializer_via_cst_fallback -- --nocapture`
Expected: FAIL — `find_var_type` returns `None` today (both line heuristics reject the chained `this.userData` receiver).

*(If this exact fixture doesn't reproduce the failure — e.g. if `this.userData` isn't treated as a chained receiver the same way `niaPreferencesDataSource.userData.first()` was in the original bug report — adjust the fixture to a `val local = someObject.someProperty` shape where `someObject` is itself a property access, not a bare identifier; the essential property needed is a receiver whose CST node is a `navigation_expression`, not a bare `simple_identifier`. Verify by reading `parser.rs`'s `call_expr_receiver_method` guard yourself against whatever fixture you land on, rather than assuming the first attempt is right.)*

- [ ] **Step 3: Make `infer_variable_type_from_cst` crate-visible**

In `src/resolver/infer.rs`, find the function signature (around line 431):
```rust
fn infer_variable_type_from_cst(indexer: &Indexer, name: &str, uri: &Url) -> Option<String> {
```
Change to:
```rust
pub(crate) fn infer_variable_type_from_cst(indexer: &Indexer, name: &str, uri: &Url) -> Option<String> {
```

- [ ] **Step 4: Add the fallback to `find_var_type`**

In `src/indexer.rs`, find:
```rust
    fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type_raw(self, var_name, uri)
    }
```
Replace with:
```rust
    fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type_raw(self, var_name, uri)
            .or_else(|| crate::resolver::infer::infer_variable_type_from_cst(self, var_name, uri))
    }
```
(Use the fully-qualified path if `infer_variable_type_from_cst` isn't already imported at the top of `indexer.rs` — check the existing `use crate::resolver::infer::{...}` block first and add it there instead if that's the established style in this file, rather than inlining the full path.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test find_var_type_resolves_a_chained_call_initializer_via_cst_fallback -- --nocapture`
Expected: PASS

- [ ] **Step 6: Check for a hot-path performance regression**

`find_var_type` is called from several hot paths (grep confirms: `resolve_root_node_type`, and other call sites in `chain.rs`/`cst_lambda.rs`/`type_subst.rs` shown in the design doc). The new fallback triggers `live_doc_or_parse` (a tree-sitter re-parse) on every miss for a file that's indexed but not currently open. This project has a documented history of similar per-call scans causing hover/inlay latency stalls.

Run a quick sanity check: `cargo build --release` then time a hover-heavy operation on a large real file (e.g. via the CLI's `hover` subcommand if one exists — check `src/cli/run.rs` — or via a quick LSP probe script against a real project) before and after this change, on a FILE THAT IS INDEXED BUT NOT OPEN (the miss path this fallback adds cost to). This is a sanity check, not a rigorous benchmark — if it's clearly fine (no perceptible difference), note that in your report and move on. If it looks meaningfully slower, report `DONE_WITH_CONCERNS` with the numbers rather than silently shipping it — do not attempt to fix a perf regression yourself without checking with the controller first, since the right mitigation (caching, gating the fallback further) is a design decision, not a mechanical follow-up.

- [ ] **Step 7: Run the full test suite**

Run: `cargo test 2>&1 | grep -E "^test result:|FAILED"`
Expected: all pass, same or greater total count than before this task (one new test added, zero removed).

- [ ] **Step 8: Commit**

```bash
git add src/resolver/infer.rs src/indexer.rs src/indexer_tests.rs
git commit -m "fix(indexer): find_var_type falls back to CST inference for chained receivers

infer_variable_type_raw's two line-based heuristics both explicitly reject
a chained (navigation_expression) receiver -- e.g. val x = a.b.c()  --
so find_var_type returned None for exactly this shape, even though the
sibling infer_receiver_type (used by hover and others) already has this
fallback via infer_variable_type_from_cst. This is the first of three
independently-necessary fixes for a live-probed rename failure on
Flow<T>-derived receivers (val x = someFlow.first())."
```

---

### Task 2: `resolve_callee_chain` pre-slices its segment list

**Files:**
- Modify: `src/indexer/infer/chain.rs` (`resolve_callee_chain`'s `KIND_NAV_EXPR` arm, around line 241-247)
- Test: `src/indexer/infer/mod_tests.rs`

**Interfaces:**
- Consumes: `resolve_segments_type(segments: &[NavSegment<'_>], bytes: &[u8], deps: &impl InferDeps, uri: &Url, strictness: SuffixStrictness) -> Option<String>` (already exists, `chain.rs:546-566` — unchanged by this task).
- Produces: no signature change — `resolve_callee_chain(callee: tree_sitter::Node<'_>, bytes: &[u8], deps: &impl InferDeps, uri: &Url) -> Option<(String, String)>`'s return VALUE changes for a chained callee ending in a method-with-trailing-lambda (now the type *before* that method, not the method's own corrupted return type); its signature and every other input shape are unchanged.

- [ ] **Step 1: Write the failing test**

Add to `src/indexer/infer/mod_tests.rs` (this file already imports `super::chain::{...}` for the sibling test `unresolved_final_suffix_fails_the_strict_walk` at line ~133 — follow that same import style):

```rust
#[test]
fn resolve_callee_chain_reports_receiver_type_before_the_final_method_not_its_return_type() {
    use super::chain::resolve_callee_chain;

    // `container.items.map` -- a nav_expr callee for `container.items.map { ... }`.
    // resolve_callee_chain must report the type of `container.items` (Box<Thing>)
    // as the receiver, and "map" as the method name -- NOT "map"'s own (here,
    // deliberately wrong/unresolvable) return type folded into current_type.
    let uri = test_url("/Chain.kt");
    let deps = super::deps::TestDeps::new()
        .with_var(uri.as_str(), "container", "Container")
        .with_field("Container", "items", "Box<Thing>");
    let doc = live_doc_for("fun f() { container.items.map { it } }\n");
    let nav = find_first_node_of_kind(doc.tree.root_node(), "navigation_expression")
        .expect("nav node");

    let result = resolve_callee_chain(nav, &doc.bytes, &deps, &uri);
    assert_eq!(
        result,
        Some(("Box<Thing>".to_owned(), "map".to_owned())),
        "receiver type must be Box<Thing> (the type before .map), with \"map\" \
         as the separately-reported method name -- got {result:?}"
    );
}
```

`TestDeps::with_field(class_name: &str, field_name: &str, type_name: &str) -> Self` is confirmed
to exist exactly with this signature (`src/indexer/infer/deps.rs:264-275`) — the test above is
verified against the real builder, not a guess.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test resolve_callee_chain_reports_receiver_type_before_the_final_method_not_its_return_type -- --nocapture`
Expected: FAIL — today's `resolve_callee_chain` resolves "map" via `resolve_member_type_on(cur, "map", deps, uri)` inside `forward_resolve_segments`'s `Suffix` arm, which (since `TestDeps` has no method/field named "map" on `Box<Thing>`) returns `None` from `resolve_member_type_on`, hits the `strictness == SuffixStrictness::Fail` check (this call site uses `SuffixStrictness::LeakReceiver`, so it does NOT return `None` outright) — trace the actual current behavior yourself by running the test and reading the actual returned value, since the precise wrong-answer shape depends on `SCOPE_FUNCTIONS`/strictness interaction; don't assume `None` is what you'll see pre-fix, the assertion mismatch itself is what proves the bug.

- [ ] **Step 3: Implement the fix**

In `src/indexer/infer/chain.rs`, find `resolve_callee_chain`'s `KIND_NAV_EXPR` arm:
```rust
        k if k == KIND_NAV_EXPR => {
            let segments = collect_nav_segments(callee, bytes);
            if segments.is_empty() {
                return None;
            }
            forward_resolve_segments(&segments, bytes, deps, uri, SuffixStrictness::LeakReceiver)
        }
```
Replace with:
```rust
        k if k == KIND_NAV_EXPR => {
            let segments = collect_nav_segments(callee, bytes);
            if segments.len() < 2 {
                return None;
            }
            // Mirror resolve_call_expr_type's external pre-slice (this same
            // file, ~line 436-449): the LAST segment is the method being
            // called with the trailing lambda (e.g. "map"), not itself a
            // member access to fold into the receiver's type. Do NOT push
            // this exclusion into forward_resolve_segments/resolve_segments_type
            // themselves -- their "resolve every segment I'm handed" contract
            // is relied on elsewhere (see mod_tests.rs's
            // unresolved_final_suffix_fails_the_strict_walk) and by
            // resolve_call_expr_type's own already-correct pre-sliced call.
            let method_name = match segments.last()? {
                NavSegment::Suffix { name, .. } => name.clone(),
                _ => return None,
            };
            let receiver_type = resolve_segments_type(
                &segments[..segments.len() - 1],
                bytes,
                deps,
                uri,
                SuffixStrictness::LeakReceiver,
            )?;
            Some((receiver_type, method_name))
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test resolve_callee_chain_reports_receiver_type_before_the_final_method_not_its_return_type -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run the existing regression test that guards against breaking this exact class of change**

Run: `cargo test unresolved_final_suffix_fails_the_strict_walk -- --nocapture`
Expected: PASS, unchanged — this test calls `resolve_segments_type` directly (not through `resolve_callee_chain`), so it must be completely unaffected by this task's change, which only touches `resolve_callee_chain`'s own call site.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test 2>&1 | grep -E "^test result:|FAILED"`
Expected: all pass. Pay particular attention to any test involving `.let`/`.also`/`.run`/scope functions or multi-segment chains ending in a trailing lambda — this is the shape this task's fix specifically changes behavior for.

- [ ] **Step 7: Commit**

```bash
git add src/indexer/infer/chain.rs src/indexer/infer/mod_tests.rs
git commit -m "fix(chain): resolve_callee_chain excludes the final segment from receiver-type resolution

The KIND_NAV_EXPR arm handed forward_resolve_segments the FULL segment
list, so for a callee like \`container.items.map\` (the callee of
\`container.items.map { lambda }\`), \"map\" was resolved as a plain member
access into current_type -- corrupting the receiver type with map's own
(often still-generic) return type instead of stopping at the type right
before the call, which is what every caller of resolve_callee_chain
expects per its own doc comment. Fixed by pre-slicing at the call site,
mirroring resolve_call_expr_type's already-correct pattern in this same
file -- forward_resolve_segments/resolve_segments_type themselves are
untouched (an earlier draft of this fix modified them directly and was
caught by review: it would have double-truncated resolve_call_expr_type's
existing correct usage and broken unresolved_final_suffix_fails_the_strict_walk).
Second of three independently-necessary fixes for a live-probed rename
failure on Flow<T>-derived receivers (someFlow.map { it -> ... })."
```

---

### Task 3: `find_class_type_params` gets a JAR fallback

**Files:**
- Modify: `src/indexer.rs` (`InferDeps::find_class_type_params`, around line 423-442)
- Test: `src/indexer_tests.rs`

**Interfaces:**
- Consumes: `Indexer.jar_definitions: DashMap<String, Vec<Location>>`, `Indexer.jar_files: DashMap<String, Arc<FileData>>`, `crate::indexer::jar::ensure_jar_definitions_for` (all already exist and are already used by the sibling `find_fun_callable_info`, same file).
- Produces: no signature change — `InferDeps::find_class_type_params(&self, class_name: &str) -> Vec<String>` now returns real type params for a JAR-defined generic class instead of always `Vec::new()`.

- [ ] **Step 1: Write the failing test**

Add to `src/indexer_tests.rs`. This follows the SAME direct-`SymbolEntry`-construction pattern the existing `jar_symbol_resolved_via_import` test (same file, constructs a `SymbolKind::CLASS` entry) already uses — NOT the function-only `insert_fake_jar_symbol` helper in `it_this_tests.rs` (that helper hardcodes `kind: SymbolKind::FUNCTION`, so it cannot construct a class symbol as-is):

```rust
#[test]
fn find_class_type_params_falls_back_to_jar_indexed_classes() {
    // Simulates a JAR-provided generic class (e.g. kotlinx.coroutines.flow.Flow<T>)
    // whose own type parameter is needed for generic substitution elsewhere
    // (build_type_arg_subst). Before this fix, find_class_type_params only
    // ever read workspace-indexed classes and always returned an empty Vec
    // for any JAR-defined one.
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;
    use tower_lsp::lsp_types::{Location, Position, Range, Url};

    let jar_uri = Url::parse("jar:file:///lib/fake-flow.jar!/").unwrap();
    let idx = Indexer::new();

    let range = Range {
        start: Position { line: 0, character: 0 },
        end: Position { line: 0, character: 4 },
    };
    let flow_symbol = SymbolEntry {
        name: "Flow".into(),
        kind: SymbolKind::CLASS,
        visibility: Visibility::Public,
        range,
        selection_range: range,
        detail: "interface kotlinx.coroutines.flow.Flow<out T>".into(),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(
            vec!["T".to_owned()],
            String::new(),
            String::new(),
            "A cold asynchronous data stream".into(),
        ),
        trailing_lambda: false,
        deprecated: false,
    };

    idx.jar_definitions
        .entry("Flow".into())
        .or_default()
        .push(Location {
            uri: jar_uri.clone(),
            range,
        });
    idx.jar_files.insert(
        jar_uri.to_string(),
        Arc::new(FileData {
            symbols: vec![flow_symbol],
            source_set: SourceSet::Library,
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );

    let type_params = idx.find_class_type_params("Flow");
    assert_eq!(
        type_params,
        vec!["T".to_owned()],
        "must find Flow's own type parameter via the JAR fallback -- got {type_params:?}"
    );

    // Decoy: a genuinely unknown class (neither workspace nor JAR-indexed)
    // must still return an empty Vec, not panic or find something spurious.
    assert!(
        idx.find_class_type_params("TotallyUnknownClass").is_empty(),
        "an unindexed class name must return an empty Vec, not panic"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test find_class_type_params_falls_back_to_jar_indexed_classes -- --nocapture`
Expected: FAIL — `find_class_type_params("Flow")` returns `Vec::new()` today (only ever checks `self.definitions`/`self.files`, never `self.jar_definitions`/`self.jar_files`).

- [ ] **Step 3: Implement the fix**

In `src/indexer.rs`, find `find_class_type_params`:
```rust
    fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
        let Some(locations) = self.definitions.get(class_name) else {
            return Vec::new();
        };
        for loc in locations.iter() {
            let Some(url) = self.file_table.url(loc.file) else {
                continue;
            };
            if let Some(file_data) = self.files.get(url.as_str()) {
                if let Some(sym) = file_data
                    .symbols
                    .iter()
                    .find(|s| s.name == class_name && !s.type_params().is_empty())
                {
                    return sym.type_params().to_vec();
                }
            }
        }
        Vec::new()
    }
```
Replace with:
```rust
    fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
        if let Some(locations) = self.definitions.get(class_name) {
            for loc in locations.iter() {
                let Some(url) = self.file_table.url(loc.file) else {
                    continue;
                };
                if let Some(file_data) = self.files.get(url.as_str()) {
                    if let Some(sym) = file_data
                        .symbols
                        .iter()
                        .find(|s| s.name == class_name && !s.type_params().is_empty())
                    {
                        return sym.type_params().to_vec();
                    }
                }
            }
        }
        // Fallback: JAR-indexed classes. Same synthetic-line-as-index
        // addressing find_fun_callable_info (this same file) already uses
        // for JAR symbols -- a JAR symbol's "line" is really an index into
        // its file's own `symbols` Vec, not a real line number.
        let mut cache_backed_only = 0usize;
        crate::indexer::jar::ensure_jar_definitions_for(self, class_name, &mut cache_backed_only);
        if let Some(jar_locs) = self.jar_definitions.get(class_name) {
            for loc in jar_locs.iter().take(MAX_BY_NAME_DEFS) {
                if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
                    if let Some(sym) = file_data
                        .symbols
                        .get(loc.range.start.line as usize)
                        .filter(|s| s.name == class_name && !s.type_params().is_empty())
                    {
                        return sym.type_params().to_vec();
                    }
                }
            }
        }
        Vec::new()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test find_class_type_params_falls_back_to_jar_indexed_classes -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run the full test suite**

Run: `cargo test 2>&1 | grep -E "^test result:|FAILED"`
Expected: all pass, same or greater total count. This change is purely additive (a new fallback branch reached only when the workspace loop finds nothing) — no existing class's type-param resolution should change.

- [ ] **Step 6: Commit**

```bash
git add src/indexer.rs src/indexer_tests.rs
git commit -m "fix(indexer): find_class_type_params falls back to JAR-indexed classes

Only ever read self.definitions/self.files (workspace-indexed classes),
so any JAR-defined generic class (e.g. kotlinx.coroutines.flow.Flow<T>)
always got an empty Vec -- meaning build_type_arg_subst could never build
a real substitution map for it, no matter how the call to it was reached.
The sibling find_fun_callable_info already has this exact JAR fallback for
functions' type parameters; this gives find_class_type_params the same
two-tier shape. Third of three independently-necessary fixes for a
live-probed rename failure -- this one specifically backs the
val x = someFlow.first() case (resolve_call_expr_type's class-level
substitution path), not the someFlow.map { it } case (which resolves
through a separate mechanism fixed in Task 2)."
```

---

### Task 4: Live-probe verification against the real project

**Files:** None — this is a manual verification step, not a code change.

**Interfaces:** None.

- [ ] **Step 1: Install the fixed binary**

```bash
cargo install --path . --force
```

- [ ] **Step 2: Re-run the exact live probe that found this bug**

Write (or reuse, if still present in the scratchpad from the original investigation) a small Python script driving `~/.cargo/bin/kmp-lsp` via stdio JSON-RPC against `/home/ocel/Work/samples/nowinandroid`, waiting for the `$/progress` "kmp-lsp/indexing" end notification before querying (an unindexed workspace gives misleadingly pessimistic results — this project's standing lesson from prior live-probe work), then sending `textDocument/rename` at the exact two positions that originally failed:

1. `core/data/src/main/kotlin/com/google/samples/apps/nowinandroid/core/data/repository/OfflineFirstNewsRepository.kt`, the `userData.shouldHideOnboarding` reference inside `modelUpdater = { changedIds -> val userData = niaPreferencesDataSource.userData.first(); val hasOnboarded = userData.shouldHideOnboarding ...}` (the `.first()` case — verify the exact current line/column against the real file, since other tasks in this plan's own history may have shifted nothing here but line numbers are never safe to assume stale).
2. `feature/foryou/impl/src/main/kotlin/com/google/samples/apps/nowinandroid/feature/foryou/impl/ForYouViewModel.kt`, the `it.shouldHideOnboarding` reference inside `userDataRepository.userData.map { !it.shouldHideOnboarding }` (the lambda-`it` case).

Only inspect the returned `WorkspaceEdit`/error — do not apply anything to disk. Confirm `git status` in the nowInAndroid checkout is clean before and after.

**Expected:** both renames now SUCCEED (return a real `WorkspaceEdit`, not a refusal) — this is the actual user-facing regression floor for this whole plan. If either still refuses, do not consider this plan done; report back with the exact refusal message and re-open investigation rather than declaring victory on unit tests alone — the live probe is the ground truth this plan exists to satisfy.

- [ ] **Step 3: Report results**

Note the outcome (success/failure, exact edit counts if successful) in the final task report — this becomes part of the PR description's test plan, the same way the original bug's live-probe findings were documented.

---

## Self-Review Notes

**Spec coverage:** all three fixes from the design doc have a task (Task 1 = Fix 1, Task 2 = Fix 2, Task 3 = Fix 3); the design's testing section's per-fix/per-goal guidance is followed exactly (Task 2's test does not use `insert_fake_jar_symbol` since Fix 2 never touches `find_class_type_params`; Task 3's test constructs a JAR class symbol directly, following `indexer_tests.rs`'s own established `jar_symbol_resolved_via_import` pattern, not the function-only `insert_fake_jar_symbol` helper); the design's mandated live-probe re-run is Task 4.

**Ordering:** the three fix tasks have no dependency on each other and could be done in any order or in parallel by independent reviewers — sequenced 1/2/3 here only because that's the order the design doc presents them in. Task 4 must run last, after all three are merged into the same build.

**Type consistency:** `find_var_type`'s signature (`fn(&self, var_name: &str, uri: &Url) -> Option<String>`), `resolve_callee_chain`'s signature (`fn(callee: tree_sitter::Node<'_>, bytes: &[u8], deps: &impl InferDeps, uri: &Url) -> Option<(String, String)>`), and `find_class_type_params`'s signature (`fn(&self, class_name: &str) -> Vec<String>`) are all unchanged by their respective tasks — every fix is a body-only change, confirmed against the design doc's own "no signature change" framing for Fixes 1 and 3, and Fix 2's explicit "return contract unchanged, only which value" framing.

**Known open item carried into Task 1:** the plan flags (Step 6) that `find_var_type`'s new fallback could add a per-miss re-parse cost on a hot path, and explicitly tells the implementer to check and report rather than silently ship or silently fix — this is a real, not-yet-measured risk the design doc raised and this plan does not resolve in advance, by design (measuring requires the actual implementation to exist first).
