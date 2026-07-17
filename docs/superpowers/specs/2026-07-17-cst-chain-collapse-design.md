# CST Chain Collapse + Repair-Seam Hoist — Design (Slice 4)

Status: **approved design** (brainstormed with the user 2026-07-17; decisions: repair hoist =
lambda family only; nav arm redirects to the existing segment walk).
Slice 4 of [CST resolution unification](2026-06-30-cst-resolution-unification-design.md);
refines the sketch in `docs/superpowers/plans/2026-07-05-post-fable-roadmap-refinements.md` §4.
Branch: `refactor/cst-chain-collapse` off `refactor/unified-resolution` (post-#222).

## Context (why)

Two collapse targets remain in `indexer/infer/chain.rs` and its surroundings after the
receiver-derivation slice (#222):

1. **A third chain resolver.** `resolve_root_node_type`'s `KIND_NAV_EXPR` arm holds a real CST
   node but flattens it to text and calls `resolve_dotted_text_type` — a `split('.')` walker,
   the same shaky-code class #222 deleted from `resolver/complete.rs`. chain.rs already owns the
   node-native walk (`collect_nav_segments` + `forward_resolve_segments`, wrapped by
   `resolve_segments_type`).
2. **The brace-repair mechanism is private to `it`/`this` resolution.** `lambda_resolution_doc_at`
   (append-only `}` repair, self-verifying, bounded) protects `find_it_element_type_in_lines` and
   `find_this_context_in_lines` — but the scope walk (`collect_lambda_scopes`,
   `features/completion_context.rs`) and the named-param path (`it_this.rs`) still resolve against
   the broken tree in mid-typing states, exactly the states #222 made receiver derivation
   resilient to.

Plus small debts the wave follow-ups tracked: vestigial `_lines: &[String]` params on the
`*_in_lines` family (~10 production sites, ~40 with tests), the stale "text path's" comment at
chain.rs:278, and the nested-generic-`it` completion test.

## Goals / non-goals

**Goals**

1. Delete `resolve_dotted_text_type` and (its now-dead helper) `uppercase_dotted_type_prefix`;
   the nav arm resolves through the node-native segment walk.
2. Hoist the brace-repair seam into `indexer/infer/speculative.rs` and wire the two unprotected
   lambda-family consumers (scope walk, named-param path).
3. Carry-ins: drop `_lines` vestigial params (renaming the `*_in_lines` fns), fix stale comments,
   add the nested-generic-`it` completion test.
4. Existing suites pass; intended behavior deltas are decoy-gated and listed in the PR.

**Non-goals**

- Unifying chain.rs's segment walk with expr_type.rs's recursive nav resolver — a possible later
  consolidation, out of scope here (roadmap: slice 4 = chain.rs internals).
- Paren repair / cst_cursor call-info resilience — a NEW transform, not the house mechanism;
  separate slice if ever needed.
- The string engine (`resolver/`) — heuristic by design, untouched. If a chain.rs function's only
  callers turn out to be resolver-side, it MOVES there rather than being deleted.

## Design

### A. Node-native nav resolution

```rust
k if k == KIND_NAV_EXPR => {
    let segments = collect_nav_segments(node, bytes);
    resolve_segments_type(&segments, bytes, deps, uri)
}
```

Deleted: `resolve_dotted_text_type` (~24 lines), `uppercase_dotted_type_prefix` (~10 lines; its
only production caller is the deleted fn). No recursion hazard: `collect_nav_segments` flattens
nested nav trees, so roots handed back to `resolve_root_node_type` are never nav nodes.

**Deliberate behavior deltas (each decoy-gated):**

1. **Generics survive.** The old exit normalization (`dotted_ident_prefix`) stripped `<...>`, so
   a nav root like `wrapper.items` with `items: List<Product>` resolved to bare `List`, killing
   downstream element-type extraction. The forward walk returns the raw type. Decoy: a chain
   rooted at a nav expression with a generic type must produce the CONCRETE element type, with
   `assert_ne!` against the bare parameter name (house pattern).
2. **Methods resolve mid-chain.** The text walker resolved fields only; `resolve_member_type_on`
   probes fields then methods. Strictly more capable.
3. **The whole-dotted-text-as-variable first-try disappears** (`find_var_type("a.b")`). The suite
   arbitrates; if anything relied on it, the fix is a real segment-walk capability, never a text
   resurrect.

### B. Repair-seam hoist (lambda family only)

Move from `it_this.rs` into `indexer/infer/speculative.rs`: `LambdaResolutionDoc`
(`Parsed(Arc<LiveDoc>)` / `Repaired(LiveDoc)`), `LambdaTreeGate` + `lambda_tree_gate`,
`repaired_doc_at`, `lambda_resolution_doc_at` (public seam name:
`speculative::lambda_doc_at(idx, uri, pos) -> Option<LambdaResolutionDoc>`).

`speculative.rs` becomes the home of both transient-healed-doc constructors — marker insertion
(receiver derivation) and brace repair (lambda resolution). **Co-located, not merged**: different
transforms answering different questions, sharing the module and parse helpers. The move is
behavior-neutral for the two existing consumers (their repair tests keep passing unchanged).

Newly wired consumers, each RED-first (failing test with an unclosed `{` above the cursor first):

- **Scope walk** — `collect_lambda_scopes` (`features/completion_context.rs`): bare-word
  completion inside a just-opened, unclosed lambda must still see the lambda scope stack.
- **Named-param path** — the `it_this.rs` consumer at the raw `live_doc_or_parse` site:
  `items.map { item -> item. }` with the brace unclosed must still type `item`.

Out of scope: `cst_cursor.rs` call-info sites (their broken state is an unclosed `(`).

### C. Carry-ins (mechanical, separate commits)

- Rename `find_it_element_type_in_lines` → `find_it_element_type`,
  `find_this_context_in_lines` → `find_this_context`,
  `find_this_element_type_in_lines` → `find_this_element_type` (and any sibling `*_in_lines`
  whose `_lines` param is vestigial); drop the `_lines` params; update ~40 call sites.
  Mechanical — dispatched to a subagent with exact instructions.
- Fix stale comments: chain.rs:278 ("matching the text path's …"), the `_lines is vestigial`
  doc note (made true by the rename), and any comment referencing the deleted fns.
- Nested-generic-`it` completion test (ledger minor): e.g. `it` inside a lambda over
  `List<Optional<Foo>>` must complete `Foo`-typed members after `.getOrNull()?.`-style access.
  If it exposes a real inference gap, STOP AND FLAG (fix belongs to its own change, not this
  slice).

## Error handling

- Repair remains bounded (`MAX_BRACE_REPAIRS`) and self-verifying; unverified candidates are
  discarded and the caller returns the authoritative `None` — unchanged semantics, now shared.
- The nav arm returns `None` exactly where `resolve_segments_type` does; no new panic paths.

## Testing

- Decoys per behavior delta in A (concrete-type assertions; `assert_ne!` bare generic params —
  the type_subst wrong-answer-factory trap from the roadmap).
- RED-first repair tests per new consumer in B; existing repair tests pin the move's neutrality.
- Full suite + clippy per commit; live probe (`lsp_probe_cst.py` — its scenario C exercises the
  broken-state pipeline end-to-end) before the PR.
- One PR: `refactor/cst-chain-collapse` → `refactor/unified-resolution`, commits per component.
