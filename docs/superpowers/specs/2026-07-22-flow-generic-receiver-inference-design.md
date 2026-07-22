# Flow&lt;T&gt; Generic Receiver-Type Inference — Design

Status: **approved design** (investigated via three parallel scout agents, then independently
re-verified line-by-line against the real code before this doc was written; no open questions
remain — every claim below cites the exact current code).

## Context (why)

Live-probing the just-shipped CST-verified rename (PR #229) against a real Android project
(nowInAndroid) found: renaming a data class field from a REFERENCE site fails ("identity is
ambiguous") whenever the receiver's type is derived by substituting `kotlinx.coroutines.flow.Flow<T>`'s
generic type argument — both `val x = someFlow.first()` (`T` from a suspend terminal operator's
return type) and `someFlow.map { it -> ... }` (`T` as the lambda parameter's type). Renaming from
the field's own declaration works fine; only reference-site rename is affected.

A companion investigation found: this is genuinely the first user-visible break from this gap.
Go-to-definition silently falls back to an older `CursorContext`-based resolver that still
produces the right answer; find-references' recall never depended on this classifier in the
first place (only optional post-hoc verification does, and that verification only *subtracts*
proven-wrong candidates, never blocks); hover doesn't use this code path at all. Rename is
unique in requiring the CST engine's `resolve_identity` to succeed with no fallback — by design,
since a fallback here would reintroduce the exact "silent wrong edit" risk 6c's design was built
to eliminate. **The fix belongs in the shared inference engine, not in loosening rename's
identity check.**

A second companion investigation found this partially re-surfaces a conclusion from
`docs/superpowers/specs/2026-07-20-cst-find-references-design.md:28-35`, which examined the same
failure shape (a generic-typed receiver never reaching `resolve_identity`'s narrowing logic) and
concluded it was "unreachable... no bug to retrofit." The live probe now contradicts that —
`Flow<T>` is a JAR-defined generic, which that analysis didn't distinguish from a workspace-defined
one. See "Non-goals" for the precise scope of what this doc corrects.

## Goals / non-goals

**Goals**
1. `val x = someFlow.first()`-shaped local variables (a chained receiver ending in a call) get a
   correctly-typed CST-aware fallback when the line-based heuristic can't type them.
2. `someFlow.map { it -> ... }`'s lambda parameter type resolves to the flow's concrete element
   type, not a leftover generic placeholder.
3. JAR-defined generic classes' own type parameters (e.g. `Flow<T>`'s `T`) become resolvable at
   all — today only JAR-defined *functions'* type parameters are (`find_fun_callable_info` already
   has a JAR fallback; its sibling for classes, `find_class_type_params`, does not).
4. Correct the `2026-07-20` spec's stale "unreachable" conclusion with a pointer to this doc.

**Non-goals**
- A general rewrite of the resolution engine's fragmented parallel paths (`parser.rs`'s RHS
  extraction, `resolver/infer.rs`'s legacy line-scan, `indexer/infer/chain.rs`'s CST walk). That's
  the long-standing, separately-tracked "unified-resolution" effort; these three fixes are
  self-contained patches within the existing architecture, not a step toward collapsing it.
- Making rename accept an unverified fallback path. That would reintroduce exactly the risk 6c
  was built to eliminate — the correct fix is making the underlying inference actually correct,
  not loosening what rename requires of it.
- Any other JAR-defined generic type beyond what these three fixes generally enable — this doc
  doesn't hunt for every stdlib generic that might still fail; it fixes the three specific,
  independently-necessary gaps found, which happen to generalize to any JAR-defined generic class.

## The three bugs, each independently necessary

Traced end-to-end from `rename_impl` → `classify_cursor`/`classify_symbol_at` →
`CstQuery::expr_type()` → `infer_expr_type` → `infer_ident_type` (`src/indexer/infer/expr_type.rs:120`),
which tries `deps.find_contextual_type` (lambda/`it`) then `deps.find_var_type` (plain variable).

### Bug 1 — `find_var_type` has no CST fallback for chained receivers

`src/indexer.rs:407-409`:
```rust
fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
    infer_variable_type_raw(self, var_name, uri)
}
```
`infer_variable_type_raw` bottoms out in two independent heuristics that both explicitly bail on
a chained (`nav_expr`) receiver:
- `parser.rs:2066-2068`'s `call_expr_receiver_method`: `// Reject multi-level chaining... if receiver_node.kind() == KIND_NAV_EXPR { return None; }`
- `resolver/infer.rs:704`'s line-scan fallback: `if receiver == "this" || receiver == "super" || receiver.contains('.') { continue; }`

So for `val userData = niaPreferencesDataSource.userData.first()`, both heuristics reject the
chained `niaPreferencesDataSource.userData` receiver and `find_var_type` returns `None` —
`userData`'s type is never even attempted via the CST-aware path.

The sibling `infer_receiver_type` (`src/resolver/infer.rs:157-181`, used by hover and others, NOT
by rename's classifier) already has the missing fallback:
```rust
ReceiverKind::Variable(name) => match infer_variable_type_raw(indexer, name, uri) {
    Some(raw) => raw,
    // CST fallback for initializers the line heuristics miss (e.g. `val x = remember { Foo() }` → `Foo`).
    None => infer_variable_type_from_cst(indexer, name, uri)?,
},
```

**Fix:** give `find_var_type` the same fallback.

### Bug 2 — `forward_resolve_segments` resolves the final segment instead of excluding it

`forward_resolve_segments` (`src/indexer/infer/chain.rs:109-225`) is used (via `resolve_callee_chain`)
to compute "the receiver type right before the trailing lambda's call" for `someFlow.map { it -> }`.
Its `NavSegment::Suffix` arm (lines 139-164) applies to *every* suffix uniformly, including the
last one:
```rust
if let Some(resolved) = resolve_member_type_on(cur, name, deps, uri) {
    current_type = Some(resolved);
    last_suffix_resolved = true;
}
```
For `userDataRepository.userData.map`, the segments are `Root(userDataRepository)`,
`Suffix("userData")`, `Suffix("map")` — "map" is *also* a plain `Suffix` here (the call's trailing
lambda means "map" never becomes a `NavSegment::CallExpr`). So `current_type` gets overwritten
with `map`'s own (still-generic, function-level-parameterized) return type instead of stopping at
`Flow<UserData>` — corrupting exactly the value downstream generic substitution needs.

The sibling `resolve_call_expr_type` (`chain.rs:418-495`) already does this correctly — it
explicitly excludes the final segment before resolving the receiver, then computes the called
function's own return type as a separate, later step (with its own substitution logic, lines
453-467):
```rust
let receiver_type = if callee.kind() == KIND_NAV_EXPR {
    let segments = collect_nav_segments(callee, bytes);
    if segments.len() >= 2 {
        resolve_segments_type(&segments[..segments.len() - 1], bytes, deps, uri, SuffixStrictness::LeakReceiver)
    } else { None }
} else { resolve_root_node_type(callee, bytes, deps, uri) };
```

**Fix — corrected after independent critique found the first draft unsafe.** The first draft of
this doc proposed changing `forward_resolve_segments`'s own core loop to skip its last segment.
That is wrong and would have shipped a regression: `resolve_segments_type` (`chain.rs:546-566`,
contract: "the final type after all segments") is a **thin wrapper that calls
`forward_resolve_segments` on the exact list it's given, with no truncation of its own**.
`resolve_call_expr_type` already calls `resolve_segments_type(&segments[..len-1], ...)` —
pre-sliced *at the call site*. If `forward_resolve_segments` ALSO excluded its own last segment
internally, `resolve_call_expr_type`'s already-correct call would double-truncate (resolving one
segment short of where it should stop), and the existing test
`unresolved_final_suffix_fails_the_strict_walk` (`src/indexer/infer/mod_tests.rs:133-162`, which
calls `resolve_segments_type` directly and asserts the **last** segment IS attempted) would start
failing. `forward_resolve_segments`/`resolve_segments_type`'s "resolve every segment I'm handed"
contract must stay exactly as-is.

The truncation belongs at the caller that currently lacks it: `resolve_callee_chain`'s
`KIND_NAV_EXPR` arm (`chain.rs:241-247`), which today hands `forward_resolve_segments` the *full*
segment list and is the one place actually trying to answer "receiver type right before this
callee's own trailing call" — exactly the question `resolve_call_expr_type` already answers
correctly for full call expressions, just not yet mirrored here for the bare-callee case:

```rust
k if k == KIND_NAV_EXPR => {
    let segments = collect_nav_segments(callee, bytes);
    if segments.len() < 2 {
        return None; // no receiver to report for a bare/rootless callee
    }
    // Mirror resolve_call_expr_type's external pre-slice (chain.rs:436-449):
    // the LAST segment is the method being called (`map`), not itself a
    // member access to fold into the receiver's type.
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
This keeps `forward_resolve_segments`/`resolve_segments_type` untouched and their existing test
(`mod_tests.rs:133-162`) passing unchanged, while giving `resolve_callee_chain` the correct
"receiver before the call" value for `userDataRepository.userData.map` (`Flow<UserData>`, not
`map`'s own corrupted return type).

**Which goal this actually fixes — corrected.** The first draft implied Bug 3 backs both Goal 1
and Goal 2. It doesn't: Goal 1 (`.first()`) resolves through `resolve_call_expr_type`'s
class-level path (`find_method_return_type_for_type` + `build_type_arg_subst`, which calls
`find_class_type_params` — this is what Bug 3 fixes). Goal 2 (`.map { it }`) resolves through a
**separate, independent mechanism** — `build_ext_fn_type_subst` (`type_subst.rs:108-150`), which
derives its substitution purely from the extension function's own declared receiver-type text
(`"Flow<T>"`) and its own `type_params` (`["T","R"]`), both already supplied by
`find_fun_callable_info`'s existing, working JAR fallback — **it never calls
`find_class_type_params` at all**. So Goal 2 needs only this fix (Bug 2); Bug 3 is what Goal 1
needs. Both fixes are still required (this doc's "three independently-necessary" framing holds),
but the internal wiring is per-goal, not shared — get this right when writing the plan so tests
target the actual mechanism each goal depends on.

### Bug 3 — `find_class_type_params` never reads JAR-indexed classes

`src/indexer.rs:423-442`:
```rust
fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
    let Some(locations) = self.definitions.get(class_name) else { return Vec::new(); };
    for loc in locations.iter() {
        ...self.files.get(url.as_str())...
    }
    Vec::new()
}
```
Only ever reads `self.definitions`/`self.files` (workspace-indexed symbols). `Flow<T>` is declared
in the kotlinx-coroutines-core JAR, so this always returns an empty `Vec` for `"Flow"` — meaning
`build_type_arg_subst` (`indexer/infer/type_subst.rs:21-39`, called from both
`resolve_member_type_on` and `resolve_call_expr_type`) can never build a `T → UserData`
substitution for `Flow`, no matter how the call is reached. This is why even a correctly-reached
`Flow<UserData>.first(): T` still can't resolve to `UserData` — the substitution map is always
empty for any JAR-defined generic class.

The sibling `find_fun_callable_info` (`src/indexer.rs:467-506`) already has exactly this fallback
for *functions'* type parameters:
```rust
let from_workspace = self.find_in_workspace_defs(fn_name, |loc| { ... });
if from_workspace.is_some() { return from_workspace; }
// Fallback: JAR-indexed files (sidecar symbols carry type_params)...
let mut cache_backed_only = 0usize;
crate::indexer::jar::ensure_jar_definitions_for(self, fn_name, &mut cache_backed_only);
let jar_locs = self.jar_definitions.get(fn_name)?;
for loc in jar_locs.iter().take(MAX_BY_NAME_DEFS) {
    if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
        if let Some(sym) = file_data.symbols.get(loc.range.start.line as usize)
            .filter(|s| s.name == fn_name && !s.type_params().is_empty())
        {
            return Some(CallableInfo { type_params: sym.type_params().to_vec(), ... });
        }
    }
}
```
Note the JAR-symbol addressing convention: `loc.range.start.line as usize` is a synthetic index
into the JAR file's `symbols` Vec (O(1) direct addressing), not a real line number — this is an
existing, established convention in this codebase for JAR-indexed symbols, not something this fix
invents.

**Fix:** give `find_class_type_params` the identical two-tier (workspace-first, JAR-fallback)
shape, addressing JAR symbols the same synthetic-line-as-index way `find_fun_callable_info` does.

## Design: the three fixes

### Fix 1 — `src/indexer.rs`, `InferDeps::find_var_type`

```rust
fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
    infer_variable_type_raw(self, var_name, uri)
        .or_else(|| infer_variable_type_from_cst(self, var_name, uri))
}
```
**Verified visibility gap (caught by independent critique — not "confirm when implementing," a
required action item):** `infer_variable_type_from_cst` (`src/resolver/infer.rs:431`) is
module-private to `resolver::infer` — no `pub` at all. `infer_receiver_type` can call it because
it lives in the same module; `find_var_type` (`src/indexer.rs`, a different module) cannot,
as-is. **Fix 1 must also add `pub(crate)` to `infer_variable_type_from_cst`'s declaration** or it
will not compile. This is small and mechanical but is a real, required part of the patch, not an
implementation detail to sort out later.

This is a behavior-additive change: every existing caller of `find_var_type` that already
succeeded via `infer_variable_type_raw` sees no change; only the previously-`None`
chained-receiver case gains a real answer. **Perf note, flagged by critique, not yet
benchmarked:** `find_var_type` is called from several hot paths (`resolve_root_node_type`,
`receiver_aware_params`, `cst_with_receiver_ctx`, `classify_this_lambda_context`); the new
fallback triggers a tree-sitter re-parse (`live_doc_or_parse`) on every miss for indexed-but-closed
files. Given this project's prior history of similar per-call scans causing hover/inlay stalls
(see `[[perf-by-name-scan-cherry-pick]]` memory), the implementer should sanity-check this isn't a
regression (e.g. time a hover-heavy operation on a large real file before/after) rather than
assume it's free — flag DONE_WITH_CONCERNS if it looks costly rather than silently shipping it.

### Fix 2 — `src/indexer/infer/chain.rs`, `resolve_callee_chain`'s `KIND_NAV_EXPR` arm

**Do NOT modify `forward_resolve_segments` or `resolve_segments_type` themselves** — their
existing "resolve every segment I'm handed" contract is correct and load-bearing (see the
"Fix — corrected" note above; the first draft of this section proposed changing the shared core
loop and an independent critique found it would double-truncate `resolve_call_expr_type`'s
already-correct usage and break `mod_tests.rs:133-162`). The fix lives entirely in
`resolve_callee_chain`'s `KIND_NAV_EXPR` arm (`chain.rs:241-247`), pre-slicing the segments
exactly like `resolve_call_expr_type` already does before calling `resolve_call_expr_type`'s own
sibling helper `resolve_segments_type`:

```rust
k if k == KIND_NAV_EXPR => {
    let segments = collect_nav_segments(callee, bytes);
    if segments.len() < 2 {
        return None;
    }
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
The function's return contract (`Option<(String, String)>` = `(receiver_type, method_name)`) is
unchanged. `forward_resolve_segments`/`resolve_segments_type` never see a truncated-then-re-truncated
list; every other caller of either is completely unaffected.

### Fix 3 — `src/indexer.rs`, `InferDeps::find_class_type_params`

```rust
fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
    if let Some(locations) = self.definitions.get(class_name) {
        for loc in locations.iter() {
            let Some(url) = self.file_table.url(loc.file) else { continue };
            if let Some(file_data) = self.files.get(url.as_str()) {
                if let Some(sym) = file_data.symbols.iter()
                    .find(|s| s.name == class_name && !s.type_params().is_empty())
                {
                    return sym.type_params().to_vec();
                }
            }
        }
    }
    // Fallback: JAR-indexed files. Same synthetic-line-as-index addressing
    // find_fun_callable_info already uses for JAR symbols.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(self, class_name, &mut cache_backed_only);
    if let Some(jar_locs) = self.jar_definitions.get(class_name) {
        for loc in jar_locs.iter().take(MAX_BY_NAME_DEFS) {
            if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
                if let Some(sym) = file_data.symbols.get(loc.range.start.line as usize)
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
Behavior-preserving for every class that already resolved via the workspace path (the fallback is
only reached when the workspace loop finds nothing); newly returns real type params for
JAR-defined generic classes, which previously always got an empty `Vec`.

## Testing

**`TestDeps` (`src/indexer/infer/deps.rs`) cannot test Fix 3 — verified by independent critique.**
`TestDeps` is a from-scratch mock of the `InferDeps` trait; its `find_class_type_params` just
reads a `HashMap` populated by `.with_class_params(...)` and never touches
`Indexer::definitions`/`jar_definitions`/`jar_files` at all. A `TestDeps`-based test can prove
downstream substitution logic behaves correctly *given* a non-empty type-params list — it cannot
exercise or catch a regression in `Indexer::find_class_type_params`'s own
workspace-then-JAR-fallback branching, which is the actual code Fix 3 adds. Using `TestDeps` here
would be false confidence.

The codebase already has the right tool: `insert_fake_jar_symbol`
(`src/indexer/infer/it_this_tests.rs:2047-2107`) builds a **real** `Indexer` and inserts a
synthetic symbol directly into `idx.jar_files`/`idx.jar_definitions` — exactly what's needed to
exercise Fix 3's real fallback branch. Several existing regression tests already use it for
`find_fun_callable_info`'s JAR path, including `Flow<T>`-shaped cases
(`regression_jar_collect_on_flow_t_container_generic_not_leak`,
`regression_productflow_param_collect_result_not_t`); Fix 3's own test should follow the same
pattern with a `SymbolKind::CLASS` entry instead of a function entry.

Per-fix and per-goal testing (**do not conflate which fix backs which goal** — see the corrected
Bug 2/3 analysis above):
- **Fix 1** (`find_var_type`'s CST fallback): a unit test on `Indexer::find_var_type` directly —
  a `val x = a.b.c()`-shaped chained initializer that the line heuristic can't type, asserting the
  CST fallback now returns the right answer where it previously returned `None`.
- **Fix 2** (`resolve_callee_chain`'s pre-slice): a unit test on `resolve_callee_chain` directly
  (or `cst_it_element_type`/whatever calls it for the lambda-`it` path) proving a
  `receiver.member.method { lambda }`-shaped callee now returns `(type-of-receiver.member,
  "method")` instead of `method`'s own corrupted return type — this is Goal 2's actual regression
  floor, and does NOT need `insert_fake_jar_symbol` since it never touches
  `find_class_type_params`; a workspace-defined stand-in generic (e.g. a test-fixture `class
  Box<T>`) is sufficient and appropriate here.
- **Fix 3** (`find_class_type_params`'s JAR fallback): a unit test using
  `insert_fake_jar_symbol` to insert a synthetic JAR-defined generic class, asserting
  `find_class_type_params` now returns its real type params instead of an empty `Vec` — this is
  Goal 1's actual regression floor.
- **End-to-end, both goals combined:** a live-probe re-run against the real nowInAndroid project
  (the exact two cursor positions this bug was found at — `OfflineFirstNewsRepository.kt`'s
  `.first()`-derived `userData` and `ForYouViewModel.kt`'s lambda `it`) before merge, confirming
  `classify_cursor`'s `receiver_type` now resolves to `Some("UserData")` at both, and — the
  original symptom that started this investigation — that rename from those exact reference sites
  now succeeds instead of refusing.

## Spec correction

`docs/superpowers/specs/2026-07-20-cst-find-references-design.md`'s "Retrofit" analysis (lines
28-35) concluded a generic-typed receiver reaching `resolve_identity`'s `Reference` arm was
"unreachable" because `infer_ident_type` strips generics from the type STRING before it's
produced. That analysis is about a *different* mechanism (string-stripping for an exact-key
lookup) than this bug (never attempting substitution in the first place for a JAR-defined
generic's own type parameters) — both are real, but the earlier analysis's "no bug to retrofit"
conclusion for the broader class of "generic-typed receiver" scenarios doesn't hold now that a
live probe found a concrete case. Add a short correction note there pointing at this doc, rather
than rewriting that doc's own (still largely correct, for the specific narrower case it examined)
analysis.
