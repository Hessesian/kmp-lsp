# CST Receiver Derivation for Completion — Design

Status: **approved design** (brainstormed with the user 2026-07-17; decisions: full depth,
no string fallback, rust-analyzer-style marker insertion + incremental reparse).
Slice of [CST resolution unification](2026-06-30-cst-resolution-unification-design.md).
Branch: off `refactor/unified-resolution`.

## Context (why)

Dot-completion's receiver is derived today by three stacked string mechanisms in front of a CST
engine that already does the job properly elsewhere:

1. `ReceiverExpr::parse` (`resolver/complete.rs`, ~100 lines) — backward byte scan with hand-rolled
   paren balancing and `?.` normalization via `str::replace`.
2. `join_fluent_chain_continuation` (`features/completion.rs`, ~70 lines) — upward line walk for
   multiline fluent chains, with string-literal-blind `//` comment stripping.
3. `resolve_dotted_receiver_type` (`resolver/complete.rs`) — resolves the derived chain by splitting
   the *string* on `.`, re-deriving what `indexer/infer/chain.rs` (`collect_nav_segments` +
   `forward_resolve_segments`, with generics substitution) already does on the tree.

These scanners are the "shaky code" (user's words) behind a steady stream of per-idiom patches
(balanced-arg skipping, comment fusion, safe-call normalization). They also produce false
positives: a `.` inside a string literal or comment happily "finds" a receiver.

The mid-typing problem that historically justified the string path — `foo.` parses with tree-sitter
ERROR nodes — is solved the way rust-analyzer solves it: insert a fake identifier at the cursor
(`intellijRulezz` there), reparse incrementally, and read the now-well-formed tree.

## Goals / non-goals

**Goals**

1. Derive the dot-completion receiver from the CST via marker-insertion speculative parse.
2. Resolve the receiver's *type* through the CST engine (`CstQuery::expr_type` → chain.rs walk).
3. Delete: `ReceiverExpr::parse`, `join_fluent_chain_continuation`, `resolve_dotted_receiver_type`'s
   string chain walk. No string fallback remains for receiver derivation.
4. Existing completion test suites pass unchanged (intentional behavior changes called out).

**Non-goals**

- Replacing the remaining type-resolution fallbacks (smart-cast, uppercase-type-name, fn-type
  extraction, `function_return_type`, `infer_callable_param_return_type`) — they are legitimate
  rules keyed off the receiver *text*; unifying them into the CST engine is a later slice.
- Touching bare-word completion, annotation completion, named-arg completion, or the lambda-scope
  machinery (`ScopeContext`, `CstQuery::lambda_scope`) — already CST-driven or out of scope.
- The string engine used by go-to-def/find-refs (`resolver/resolve.rs`) — untouched, per the parent
  design's domain split.

## Design

### Data flow

Before:

```
line text → split_prefix → join_fluent_chain_continuation → ReceiverExpr::parse → chain STRING
          → complete_dot_expr → resolve_dotted_receiver_type (split on '.') → member collection
```

After:

```
cursor → speculative parse (marker at cursor, tree clone + InputEdit + incremental reparse)
       → marker node → ascend to navigation_expression → receiver SUBTREE
       → classify + resolve type while the speculative tree is alive
       → DotReceiver (small owned value; no tree lifetimes escape)
       → member collection (direct + inherited + jar tails) — unchanged
```

Structural inversion: receiver-type resolution moves *up* into context analysis
(`CompletionContext::analyse` time); member collection consumes a resolved value.

```rust
enum DotReceiver {
    /// "it", "this", "this@label" — ScopeContext resolves, unchanged.
    Scope(String),
    Super,
    /// Any other receiver expression. `resolved` is the CST-inferred type;
    /// `text` feeds the retained text-keyed fallbacks when `resolved` is None.
    Expr { text: String, resolved: Option<ResolvedType> },
}
```

### Unit 1: speculative parse (new submodule under `indexer/infer/`)

`(uri, position) → Option<receiver subtree + speculative LiveDoc>` exposed through the `CstQuery`
catalogue in `indexer/infer/mod.rs` (the CstResolve convention).

- `live_doc_or_parse(uri)` for the base doc; `utf16_col_to_byte` for the cursor offset (reusing
  `cursor_node_at`'s end-of-line/EOF handling).
- Insert marker identifier `kmpLspRulezz` at the cursor byte offset into a copy of the bytes;
  `Tree::clone` + `Tree::edit(InputEdit)` + reparse with the old tree as hint (parser from the
  existing thread-local pool in `parser.rs`). Result wrapped as a transient `LiveDoc` — request-local,
  never shared, no invalidation concerns.
- Find the node at the marker; ascend identifier → `navigation_suffix` → `navigation_expression`;
  the receiver is the nav-expr's left subtree (identifier, nested nav-expr, `call_expression`,
  `super_expression`, `this_expression` — all shapes `chain.rs` walks). The ascent tolerates ERROR
  ancestors (same posture as `it_this.rs`).
- No `navigation_expression` ancestor → no receiver → bare-word completion. This is also the fix
  for today's false positives: a cursor inside a string literal or comment lands in a
  string/comment node and never yields a receiver. (Inside `${...}` interpolation the grammar
  *does* form nav-exprs, so interpolated dot-completion works — a bonus over the byte scanner.)

Multiline fluent chains need zero special handling — the parser sees the real file. Same for `?.`
(grammar-native) and nested/unbalanced argument lists.

### Unit 2: receiver classification + type resolution

While the speculative tree is alive:

1. Receiver text `it` / `this` / `this@label` → `DotReceiver::Scope` (ScopeContext resolves,
   unchanged). `super` (by node kind) → `DotReceiver::Super` → `complete_super`.
2. Otherwise `CstQuery::expr_type` on the receiver subtree (against the speculative doc — resolution
   reads node text + index only). Resolved → done.
3. Unresolved → `DotReceiver::Expr { text, resolved: None }`; downstream, the retained text-keyed
   ladder runs in this order (mirroring today's semantics):
   smart-cast (`infer_receiver_type_at`) → variable lookup (`infer_receiver_type`) + fn-type
   extraction → uppercase ident → type-name receiver → `function_return_type` →
   `infer_callable_param_return_type`.

`ReceiverExpr { chain, is_call }` and its `is_call` special-casing disappear: the CST knows a call
when it sees one.

### Expected chain.rs gap

`resolve_dotted_receiver_type` treats an uppercase first segment as a type name
(`MaterialTheme.colorScheme.` → root = the *type* `MaterialTheme`). If
`chain.rs::resolve_root_node_type` lacks type-name-rooted resolution, the completion suite will
expose it. The fix is extending chain.rs's root resolution — improving the authoritative engine —
never keeping the string walker. Budgeted as an expected task.

### Deletions

- `ReceiverExpr::parse` + the `ReceiverExpr` struct (replaced by `DotReceiver`).
- `join_fluent_chain_continuation` + `MAX_FLUENT_CHAIN_LINES` + its comment-stripping helper.
- `resolve_dotted_receiver_type`.
- Candidate (plan-phase inventory decides): the `is_lambda_param` line-scan routing in
  `run_completions`, if `ScopeContext::named_param_type` already answers it.
- `complete_symbol`'s `dot_receiver: Option<&str>` entry stays for its non-completion callers
  (plain variable receivers; no speculative parse needed there).

### Error handling

- `live_doc_or_parse` → None: no receiver, bare completion (the string path had nothing to scan
  either).
- Unclosed-delimiter mid-typing states (`foo(bar.`, `items.filter { it.`): marker makes the token
  clean; if recovery still forms no nav-expr, the result is bare-word completion — degraded, never
  wrong. Characterized by tests.
- Completion cache key `(uri, before_prefix, line)` unchanged; one speculative parse per cache
  miss; generation check already brackets `run_completions`.

## Testing

- RED-first characterization tests for the receiver-derivation unit: `x.`, `x.pref`, multiline
  `.padd` continuation (the Compose idiom), `?.`, chains with nested call args and trailing
  lambdas, `// comments` between chain lines, cursor inside a string literal (expect None), cursor
  inside `${}` interpolation (expect receiver), unclosed `(` and `{` before the cursor,
  `it.` / `this.` / `this@label.`, `super.`, `Modifier.` companion case, `listOf(1).` generics.
- The existing completion suites (`resolver/tests.rs`, `features/completion_tests.rs`,
  `features/completion_context_tests.rs`) are the regression net and must pass; any intentional
  behavior change is called out explicitly in the PR.
- Live probe on the real project (`lsp_probe.py` variant) before merge: dot-completion on a real
  multiline Modifier chain, a `?.` receiver, and a broken mid-edit state.
