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

**Fix:** `forward_resolve_segments` must not resolve the *last* suffix into `current_type` when
that suffix is itself the method being called with a trailing lambda — mirror
`resolve_call_expr_type`'s exclusion. (Confirmed this is necessary independent of Bug 3: even with
Bug 3 fixed, `map`'s own raw return type uses `map`'s *function-level* type parameter — commonly
named `R` in the library source, distinct from `Flow`'s *class-level* `T` — so resolving "map" as
a plain member access would still produce an unresolved placeholder even with class-level
substitution working. The two bugs are independently necessary, not alternatives.)

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
`infer_variable_type_from_cst` already exists (used by `infer_receiver_type`) and is
`pub(crate)`/reachable from `indexer.rs` today (confirm the exact visibility/import path when
implementing — `infer_receiver_type` calls it directly in the same crate). This is a
behavior-additive change: every existing caller of `find_var_type` that already succeeded via
`infer_variable_type_raw` sees no change; only the previously-`None` chained-receiver case gains
a real answer.

### Fix 2 — `src/indexer/infer/chain.rs`, `forward_resolve_segments`

Split the final segment out of the walk, mirroring `resolve_call_expr_type`'s
`&segments[..segments.len() - 1]` exclusion, then resolve the excluded final segment's *own*
return type as a separate, later step using the same `find_method_return_type_for_type` +
`build_type_arg_subst`/`apply_type_subst` combination `resolve_call_expr_type` already uses
(lines 461-465) — do not invent a new substitution mechanism, reuse the existing one. The
function's return contract (`Option<(String, String)>` = `(receiver_type, method_name)`) is
unchanged; only *which* value ends up as `receiver_type` when the last segment is itself a
call-with-trailing-lambda changes (it becomes "the type before that call," not "that call's
return type" — which is what every caller of `resolve_callee_chain`/`forward_resolve_segments`
already expects per its own doc comment: "returning the type of the expression before the final
method call").

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

Each fix needs its own unit test at the level it changes (the existing `InferDeps`/`TestDeps` fake
in `src/indexer/infer/deps.rs` is the established pattern for testing these in isolation — see
`it_this_tests.rs`'s `.with_class_params(...)` usage), PLUS one end-to-end house-decoy proving the
combination works: a live-probe-equivalent test (or, if the existing test harness can construct
an in-memory JAR-symbol fixture cheaply, an integration test) asserting `classify_cursor` on a
`someFlow.first()`-derived reference and a `someFlow.map { it -> }` lambda `it` reference both
produce `receiver_type: Some("UserData")` (or an equivalent concrete type), not `None` — this is
the actual user-facing symptom and the regression floor for this fix.

Since `Flow` itself lives in a real JAR the unit-test harness doesn't materialize, the end-to-end
test should use a workspace-defined stand-in generic type with the identical shape (a `class
Box<T>` or similar declared directly in the test fixture, deliberately indexed the same way a JAR
symbol would be via `TestDeps`) to prove the mechanism, PLUS a live-probe re-run against the real
nowInAndroid project (same two cursor positions this bug was found at) before merge, to prove the
real JAR case specifically.

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
