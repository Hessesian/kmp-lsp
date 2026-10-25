# Local-binding discoverability plan

Status: plan only. Drafted 2026-10-09/13. Slices 2 and part of 3 have since landed as #334 and #335; re-check each slice against `main` before starting it.

## Problem

Agents reuse helpers that `AGENTS.md` names or that sit on an always-imported extension trait (`NodeExt`, `StrExt`). They reinvent helpers that live in odd modules or are private. PR #329 reinvented `resolve_declared_type_from_cst`, `param_names_from_sig`, `enclosing_class_name`, `named_arg_label` and `parse_workspace_data`.

The deeper cause is a correctness bug. There is no single "which declaration binds this name here" primitive, so features guess by name or by line text:

- `fun first(item: Apple)` then `fun second(item: Banana)`: `CstQuery::expr_type` on `item` in `second` returns `Apple`.
- Go-to-definition on `item.save()` there returns `CstResolved` pointing at `Apple.save`: a wrong answer labelled precise.
- `val item = Banana()` with a same-named parameter elsewhere returns `Apple`.
- A local declared after the use site beats the constructor property.
- `item: Apple?` returns `nullable = false`.
- Nested-block shadowing and sequential redeclaration are already correct.

## Rule taken from earlier catalogue work

No new surface without a production consumer in the same slice. `CstQuery::receiver_type`, `call_return_type`, `at()`, `io` and `Ambiguous(..)` shipped without consumers and were reverted twice. This is why `CstQuery::local_binding()` is not in the first slice.

## Core type

```rust
// src/indexer/infer/cst_symbol.rs
pub(crate) enum LocalBinding<'tree> {
    Annotated { declaration: Node<'tree>, declared_type: Node<'tree> },
    Initialized { declaration: Node<'tree>, initializer: Node<'tree> },
    Contextual { declaration: Node<'tree> },
    NotLocal,
}
pub(crate) fn local_binding_at<'tree>(use_site: Node<'tree>, bytes: &[u8]) -> LocalBinding<'tree>;
```

- Carries nodes, not owned strings; owned data is what forced #329 to re-parse lines.
- Typed directly: `val`/`var`, function, `catch` and primary-constructor parameters. Lambda parameters, `for` variables and destructuring are recognised but still typed by today's path.
- Declaration order: only siblings strictly before the use site count.
- One cursor descent from the root; no `parent()` calls (O(depth) each).

## Slices (one PR each)

1. **Ratchet test + `AGENTS.md` line.** `tests/architecture_ratchet.rs` scans `src/features`, `src/semantic_tokens`, `src/inlay_hints.rs` as text and fails when counts of forbidden patterns differ from baseline. No behaviour change. `clippy.toml` rejected: crate-wide, cannot express field reads. (Landed as #333.)
2. **`local_binding_at` + identifier path in `expr_type.rs`.** Red tests first (the first three bugs above observed failing on main), pinning tests, and an agreement test against the rename scope walk. Changes go-to-definition, rename identity, semantic tokens, completion, inlay hints. (Landed as #334.)
3. **Migrate `fill_when.rs:73` and `infer.rs:191`; delete `resolve_declared_type_from_cst`.** Replaces rather than moves (agent-reference §12a), because semantics change on purpose. (`fill_when` landed as #335.)
4. **`CstQuery::local_binding()` + nullable diagnostic.** The #329 nullable fix done on the CST; removes `#[allow(dead_code)]` from `ResolvedType::is_nullable`. Maintainer lands this; the #329 contributor's tests are credited and ported (see below).
5. **Chain root** (`chain.rs:575`) through the binding, keeping generics.
6. **`local_scope_occurrences` as a projection** of the binding; deletes the generation walk. Highest risk: a wrong rename corrupts files. Defer until 2-5 have been on main for a while.
7. **Delete `Resolver::method_return_type`** (0 production callers, 5 test calls); add a helper-relocation rule to `agent-reference.md` §12.

Stacking: 1 and 2 independent. 3, 4, 5 each stack on 2. 6 waits for 2-5. 7 any time.

## Ratchet baselines (at `d8f2a4ea`)

| Pattern | Count |
|---|---|
| `resolver::{infer,infer_lines,qualified,extension,package_scope,imports,hierarchy}::` | 7 |
| `infer_variable_type*` | 2 |
| `infer_type_in_lines` | 0 |
| `.definitions` / `.jar_definitions` | 0 |
| `.files.` / `.jar_files.` + `get`/`iter`/`contains_key` | 4 |
| `.live_lines` | 2 |
| `.extension_by_receiver` | 1 |

Counts came from regexes; recompute as literal substrings at implementation.

## PR #329

- Ratchet fails #329 as written (resolver paths 7->9, `.definitions` 0->2, `.files` 4->7, more).
- Slice 2 does not conflict textually; slices 3 and 4 do.
- After slice 2, the contributor's eight sniffing functions reduce to the slice 4 match.

## Decisions (made 2026-10-25)

1. Slice 4: maintainer lands it; the #329 contributor is credited (their tests ported, `Co-authored-by` trailer on the commit) because they are not pursuing the fix.
2. Ratchet before #329.
3. A member-recall drop is acceptable if the lost hits were wrong answers, backed by a hand-checked precision sample.
4. Slice 6 deferred.

## Risks

- Slices 2-5 touch keystroke-latency paths. Measure Moneta `resolution-accuracy` wall time and semantic-token time on the largest file, before and after.
- Recall may drop: wrong `CstResolved` answers become unresolved, and the benchmark measures recall only.
- Two scoping implementations coexist from slice 2 to slice 6, guarded by the agreement test.

## Not verified

- Whether `goto_first_child_for_byte` lands correctly on zero-width or error-recovery nodes.
- Whether `declares_name_directly` covers every destructuring and lambda-parameter shape.
- Whether initializer inference keeps a trailing `?` for call results; slice 4 depends on it.

## Complementary agent-facing fixes (from the discussion)

- Add a capability index to `AGENTS.md`, keyed by question ("type of a local at a position -> `local_binding_at`"), with a CI grep that every listed symbol still exists.
- Add a PR-template line: "New helper functions: list each and what you searched for."
