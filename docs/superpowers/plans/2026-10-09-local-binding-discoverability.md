# Local-binding discoverability plan

Status (re-verified against `main` at `fbc0f0d8`, 2026-10-25):

| Original slice | State |
|---|---|
| 1 Ratchet test + `AGENTS.md` line | Done, #333 |
| 2 `local_binding_at` + identifier path in `expr_type.rs` | Done, #334 (agreement test was not written) |
| 3 Migrate `fill_when` / `infer.rs`, delete `resolve_declared_type_from_cst` | Done, #335 |
| 4 Nullable diagnostic | Open, now slice B |
| 5 Chain root | Open, now slice C |
| 6 `local_scope_occurrences` as projection | Deferred, now slice I |
| 7 Delete `Resolver::method_return_type` | Mostly obsolete, now slice G |

## Problem

There was no single "which declaration binds this name here" primitive, so features guessed by name or line text (`fun first(item: Apple)` / `fun second(item: Banana)`: `item` in `second` typed as `Apple`, and go-to-definition returned a wrong answer labelled `CstResolved`). #334 added the primitive and fixed the identifier path. What remains:

1. **Silent fallback.** Every consumer collapses "local but untypable" and "not local" into `None`, then falls back to the file-wide by-name lookup, which reintroduces the wrong-type bug whenever the in-scope binding is a lambda parameter, `for` variable, destructuring, or an initializer the CST engine cannot type. Sites: `expr_type.rs` `infer_ident_type` (`find_var_type` after `infer_binding_type`), `fill_when.rs:~74`, `resolver/infer.rs:~190`.
2. **Lossy projection.** `infer_binding_type` passes every type through `dotted_ident_prefix`, which drops `?` and type arguments. So `item: Apple?` still reports non-nullable, and `chain.rs` cannot get generics from the binding. The `LocalBinding::Annotated { declared_type }` node itself keeps both; only the projection loses them.
3. **Unmigrated by-name sites:** `find_var_type` in `cst_lambda.rs` (113, 135, 644, 1102), `type_subst.rs` (281, 304), `chain.rs:575`; `infer_variable_type_raw` in `features/hover.rs` `fallback_local_binding_hover`; a third independent scope walk in `semantic_tokens/params.rs` (`local_binding_names`).

PR #329 reinvented several existing helpers. Agents reuse what `AGENTS.md` names and what sits on `NodeExt`/`StrExt`; they reinvent what lives in odd modules. The fix is fewer duplicate primitives plus mechanical enforcement, not more prose.

## Rule taken from earlier catalogue work

No new surface without a production consumer in the same slice. `CstQuery::receiver_type`, `call_return_type`, `at()`, `io` and `Ambiguous(..)` shipped without consumers and were reverted twice.

## Core type (as landed in #334)

```rust
// src/indexer/infer/cst_symbol.rs
pub(crate) enum LocalBinding<'tree> {
    Annotated { declared_type: Node<'tree> },   // keeps `?` and type arguments
    Initialized { initializer: Node<'tree> },
    Untyped,   // bound locally, type not read: lambda param, for var, destructuring, when subject
    NotLocal,  // member, top-level, import, inherited
}
pub(crate) fn local_binding_at<'tree>(use_site: Node<'tree>, bytes: &[u8]) -> LocalBinding<'tree>;
pub(crate) fn local_binding_of<'tree>(name: &str, point: Node<'tree>, bytes: &[u8]) -> LocalBinding<'tree>;
// src/indexer/infer/expr_type.rs
pub(crate) fn infer_local_type_at(name, point, bytes, deps, uri) -> Option<(String, Url)>; // stripped
```

Differences from the original design: no `declaration` node (slice I adds it when rename needs it), no `Contextual` variant (`Untyped` covers it), and `local_binding_of` climbs with `Node::parent()` (O(depth) per call, O(depth^2) per lookup; marked with a `ponytail:` comment). Acceptable for one-call-per-node consumers; not yet measured for per-`navigation_expression` callers (slice B).

## Ratchet baselines (`tests/architecture_ratchet.rs` at `fbc0f0d8`)

Scans `src/features`, `src/semantic_tokens`, `src/inlay_hints.rs` (excluding `*tests.rs`) for literal substrings. Lower a baseline when a count drops; never raise one.

| Pattern | Allowed |
|---|---|
| `resolver::infer::` | fill_when 3, hover 1, nullable_call_diagnostics 1, semantic_tokens/resolve 1 (total 6) |
| `resolver::infer_lines::` | 0 |
| `infer_variable_type` (also matches `_raw`) | fill_when 1, hover 1 |
| `infer_type_in_lines` | 0 |
| `.definitions.` / `.jar_definitions.` | 0 / 0 |
| `.files.` | references 1, references_verify 1 |
| `.jar_files.` | hover 1, references 1 |
| `.live_lines` | completion 1, references_verify 1 |
| `.extension_by_receiver` | nullable_call_diagnostics 1 |

## Slices

### A. Typed binding outcome (serial, everything else waits on it)

- Replace the `Option` from `infer_binding_type` / `infer_local_type_at` with an enum, e.g. `BindingType { Typed { type_text, declaring_uri }, LocalUnknown, NotLocal }`. `type_text` is the unstripped text; callers that need the bare name call `dotted_ident_prefix` themselves. This fixes problems 1 and 2 at the one place all consumers route through.
- Migrate the three existing consumers so only `NotLocal` reaches `find_var_type` / `infer_variable_type`: `expr_type.rs` `infer_ident_type`, `fill_when.rs`, `resolver/infer.rs`.
- Add the missing agreement test: for a set of fixtures (shadowing, sequential redeclaration, sibling `for`/`when`/`catch`, captured outer local, same-named parameter in a sibling function), the declaration `local_binding_at` picks must be one of the locations `local_scope_occurrences` returns for that use site.
- Before/after measurements recorded in the PR: Moneta `resolution-accuracy` wall time and semantic-token time on the largest file; member recall; a hand-checked sample of ~30 hits that became unresolved, classified right/wrong (decision 3).
- Acceptance: red-first tests for `fun second(item: Banana)` where `item` is a lambda parameter / `for` variable with a same-named annotated parameter in a sibling function (must not resolve to the sibling's type); `item: Apple?` yields nullable text at the `infer_local_type_at` level; existing #334/#335 tests stay green.

### B. Nullable diagnostic via `CstQuery::local_binding()` (after A)

- Add `CstQuery::local_binding()` with `nullable_call_diagnostics.rs` as its production consumer; remove `#[allow(dead_code)]` from `ResolvedType::is_nullable` (`infer/mod.rs:162`).
- Semantics from #329: suppress the diagnostic when a visible local binds the name but is untypable (`LocalUnknown`); never borrow a type from another function.
- Port #329's tests `no_diagnostic_when_nullable_same_named_param_lives_in_another_function` and `still_flags_nullable_annotated_local_in_scope`. Drop its eight line-text helpers. Commit carries `Co-authored-by: abilliontokens <...>` (decision 1).
- Latency gate: this runs once per `navigation_expression` (see the comment at `nullable_call_diagnostics.rs:~189`). Measure diagnostics time on the largest Moneta file before/after. If the `parent()` climb shows up, replace it with a single root-threaded cursor descent inside `cst_symbol.rs` as part of this slice (the slice then stops being parallel-safe with C/D; land it alone).
- Files: `src/features/nullable_call_diagnostics.rs` (+ `_tests.rs`), `src/indexer/infer/mod.rs`.

### C. Chain root keeps generics (after A)

- `chain.rs:575`: root the chain through the binding's unstripped `type_text`, falling back to `find_var_type` only on `NotLocal`.
- Acceptance: `val items: List<Banana>` with a same-named `items: List<Apple>` in a sibling function; `items.first().save()` resolves to `Banana.save`.
- Files: `src/indexer/infer/chain.rs` (+ its tests).

### D. Lambda receivers and type substitution (after A)

- Same migration for `cst_lambda.rs` (113, 135, 644, 1102) and `type_subst.rs` (281, 304). Unverified: whether each site has a CST node in hand; where it does not, thread it from the caller rather than adding a by-name variant.
- Acceptance: `items.forEach { it.save() }` with competing `items` declarations in sibling functions resolves through the in-scope one; same for a `with(item) { save() }` receiver.
- Files: `src/indexer/infer/cst_lambda.rs`, `src/indexer/infer/type_subst.rs` (+ tests).

### E. Hover fallback (after A)

- `fallback_local_binding_hover` asks the binding (through `CstQuery` or the existing hover context, not a new resolver call) instead of `infer_variable_type_raw`.
- Lower ratchet: hover `resolver::infer::` 1 to 0 and `infer_variable_type` 1 to 0.
- Acceptance: hover on a parameter `item` in `second` shows `Banana`, with `fun first(item: Apple)` above it.
- Files: `src/features/hover.rs`, `tests/architecture_ratchet.rs`.

### F. Enforcement (after B-E merge)

- Extend `tests/architecture_ratchet.rs` with a second root set, `src/indexer/infer`, and a `find_var_type(` row whose baseline is whatever remains after B-D (expected: the `NotLocal` fallback in `expr_type.rs`, plus any site D could not migrate). This is the module-scoped ban clippy cannot express, using the mechanism the repo already has.
- Narrow visibility of `resolver::infer::infer_variable_type_raw` to `pub(in crate::resolver)` if E leaves no outside caller (`cli/hover.rs` and `indexer/resolution.rs` currently call the `infer_variable_type` family; check each).
- Files: `tests/architecture_ratchet.rs`, `src/resolver/infer.rs`.

### G. `Resolver::method_return_type` remainder (any time, independent)

- It is now `#[cfg(test)]` (`src/resolver/api.rs:173`) with 4 calls in `src/resolver/infer_tests.rs`. Point those tests at the production entry point (`method_return_type_declared` / `find_method_return_type_for_type`) and delete the wrapper. Add a short helper-relocation rule to `docs/agent-reference.md` section 12.
- Files: `src/resolver/api.rs`, `src/resolver/infer_tests.rs`, `docs/agent-reference.md`.

### H. PR #329 remainder (open decision)

- #329 (author abilliontokens, CONFLICTING) has three commits: pull diagnostics / unified diagnostic sets; stdlib top-level missing-import exemption; nullable leak + arity override-defaults false positive. B covers the nullable half.
- Decision (2026-10-25): port with `Co-authored-by` credit. Scope: the arity override-defaults fix (`call_arg_diagnostics.rs`, 3 tests) and, after a per-commit review, the stdlib missing-import exemption and pull diagnostics. Independent of A-F; one PR per #329 commit, each passing the ratchet without raising a baseline. Close #329 with a note linking the ported PRs.

### I. `local_scope_occurrences` as a projection (deferred, decision 4)

- Add the `declaration` node to `Annotated`/`Initialized`, rebuild `local_scope_occurrences` on the binding, delete the generation walk, and fold `semantic_tokens/params.rs` `local_binding_names` into it. Highest risk: a wrong rename corrupts files.
- Start only after A-E have been on `main` for a while with the agreement test green.

## Dependency graph and parallel work

```
A ──┬── B ──┐
    ├── C ──┤
    ├── D ──┼── F
    └── E ──┘
G (independent)   H (owner decision)   I (after A-E soak)
```

Conflict notes for parallel worktrees (B, C, D, E):

- Only A edits `cst_symbol.rs` and `expr_type.rs`. If B-E find they need a primitive change there, stop and land it as a serial follow-up to A, not inside a parallel slice. Exception: B's latency fallback, which then runs alone.
- B owns `infer/mod.rs` (`CstQuery`). C owns `chain.rs`. D owns `cst_lambda.rs` and `type_subst.rs`. E owns `hover.rs` and is the only parallel slice that edits `tests/architecture_ratchet.rs`.
- Test files follow their source file; shared fixtures in `resolver/tests.rs` or `indexer_tests.rs` are append-only, and the later PR rebases.
- Serena allows one active project at a time: at most one parallel agent uses Serena; the others use `cargo` and file tools.
- Every slice: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt`, red-first test for the bug it fixes, and re-run the literal repro from the problem statement.

## Decisions (made 2026-10-25)

1. Slice B: maintainer lands it; the #329 contributor is credited (tests ported, `Co-authored-by` trailer) because they are not pursuing the fix.
2. Ratchet before #329 (done, #333).
3. A member-recall drop is acceptable if the lost hits were wrong answers, backed by a hand-checked precision sample.
4. Slice I (old slice 6) deferred.

## Enforcement options considered and rejected

- **`clippy.toml` `disallowed-methods`** (used by ruff, uv, zed, codex). The repo has no `clippy.toml`. The lint is crate-wide and path-only: banning `find_var_type` or `infer_variable_type` would fire at every legitimate site inside the inference engine and in tests (13 test calls today), each needing `#[expect(clippy::disallowed_methods, reason = ..)]`. The ratchet already scopes by module and counts, so F extends it instead. `#[expect]` itself is fine on the toolchain (rustc 1.95, stable since 1.81).
- **`src/features/AGENTS.md`**: would repeat the root `AGENTS.md` rule ("Features ask, they don't dig"; "extend its return type; don't write a sibling") that already says this.
- **Capability index / Key-Files existence test / PR-template line**: the Key Files section already names `local_binding_at` and `infer_local_type_at`; an index keyed by question goes stale (zed's experience), and the ratchet catches the regressions that matter mechanically.
- **`cargo dupes` / jscpd**: unproven here; not worth a slice. Run once ad hoc if duplicate helpers reappear.

## Risks

- Recall drop from A: `LocalUnknown` no longer falls back. The biggest source is likely `Initialized` bindings whose initializer the CST engine cannot type but the line-based path can. Response: improve CST initializer inference, not restore the fallback.
- Latency: B is per-`navigation_expression`; the `parent()` climb is O(depth^2) per lookup.
- Two scoping implementations (binding climb, `local_scope_occurrences`) plus a third in `params.rs` coexist until I; the agreement test in A is the guard.

## Not verified

- Whether `infer_expr_type_at_depth` keeps a trailing `?` for call-result initializers (B needs it for `Initialized` bindings).
- Whether every D site has a CST node available without signature changes.
- Whether `declares_name_directly` covers every destructuring and lambda-parameter shape.
- Behaviour of the binding climb on zero-width and error-recovery nodes.
- Which outside callers of the `infer_variable_type` family remain after E (gates F's visibility narrowing).
