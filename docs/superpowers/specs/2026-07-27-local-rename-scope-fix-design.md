# Local Rename Scope Fix — Design (PR #229 follow-up)

Status: **approved design** (brainstormed with the user 2026-07-27; rechecked against
`AGENTS.md` and the parent CST design's "Type-driven correctness" + "CST is the source of
structure" sections, and against slice 6's own spec
([2026-07-19-cst-navigation-design.md](2026-07-19-cst-navigation-design.md)); independently
critiqued via a Fable subagent, with one of its findings narrowed after verifying against the
actual shipped grammar rather than accepted at face value — see "Critique findings applied"
below). Fixes a Copilot-review finding on PR #229 (`refactor/cst-navigation-6c`, CST-verified
rename slice 6c). Branch: `refactor/cst-navigation-6c` (existing worktree
`.claude/worktrees/agent-a7ed0dc8f5545d5a1`, already checked out there).

## Context (why)

`local_scope_occurrences` (`src/indexer/infer/cst_symbol.rs`) is 6c's local-variable rename
fast path: when a symbol is local/lambda-param and every reference lives in one file, rename
walks the CST directly instead of going through the slower cross-file `rg`+index path. Its own
doc comment promises "declaration or any reference to it" works from any occurrence.

Copilot's review finding: a reference inside a **nested lambda that merely captures an outer
local** (no shadowing) breaks that promise. `enclosing_local_body` finds only the *narrowest*
enclosing `function_declaration`/`lambda_literal`; `find_local_declaration_in_body` then
requires the name be declared *directly* inside that narrowest body. A captured-not-shadowed
reference fails this check, the whole function returns `None`, and rename silently falls
through to the slower cross-file path — contradicting the doc comment, though not incorrect
(the cross-file path still renames correctly, just slower and without the fast-path's
guarantees).

Tracing the root cause surfaced a second, related gap: Kotlin's local scoping is
block-scoped — `if`/`for`/`while`/`when`-branch bodies and `try`/`catch`/`finally` bodies each
introduce their own scope — none of which `enclosing_local_body` currently models. The user
chose to fix "full block scoping" now rather than the narrower capture-only gap, since both
symptoms trace to the same missing concept: only `function_declaration`/`lambda_literal` are
recognized as scope boundaries today.

## Critique findings applied

An independent Fable-subagent critique, given the real code and verified CST shapes, stress-tested
the first draft of this design against concrete Kotlin examples. Its findings were then
**re-verified against the actual shipped grammar** (dumping real parse trees via
`cargo run --bin kmp-lsp -- tree <file>`) before being folded in, which corrected one:

1. **Narrowed: `is_declaration_site` needs exactly one new case, not two.** Fable flagged that
   `is_declaration_site` doesn't recognize `for_statement` or `catch_block` as declaration-parent
   kinds. Verified against the real grammar:
   - A `for`-loop variable (`for (i in 0..10)`) parses as
     `for_statement → variable_declaration → simple_identifier "i"` — `variable_declaration`
     is **already** a recognized parent kind (`KIND_VAR_DECL`, `is_declaration_site` line 41).
     Same shape for a `when (val x = ...)` subject binding
     (`when_subject → variable_declaration → simple_identifier "x"`) and for destructuring
     (`val (a, b) = pair` and `for ((k, v) in map)` both wrap each name in its own
     `variable_declaration`, one level under `multi_variable_declaration`).
     **No change needed** for any of these — `is_declaration_site` only looks at the immediate
     parent, and that parent is already `variable_declaration` in every case.
   - A `catch` block's exception variable (`catch (e: Exception)`) parses as
     `catch_block → simple_identifier "e"` — a **bare** identifier, no `variable_declaration`
     wrapper. This is the one real gap: add
     `pk == KIND_CATCH_BLOCK => node.kind() == KIND_SIMPLE_IDENT`.
2. **Confirmed: the real gap is the scope-*boundary* set, not declaration-site recognition.**
   `enclosing_local_body`/`nested_scope_shadows`/`visit_unshadowed_name_matches` only stop at
   `function_declaration`/`lambda_literal` today. A name bound in a `for`/`when` header is
   *outside* the block's `{}` (sibling to `control_structure_body`, not inside it), so widening
   only the {}-block set doesn't help — the wrapper node itself must be a boundary, or an
   outward climb (below) can walk straight past it into an unrelated sibling `for`/`when`'s
   same-named binding. See "Scope boundaries" below.
3. **Confirmed: `for_statement`/`when_expression` need no "own name" special case.**
   `enclosing_local_body`'s existing doc comment explains why `function_declaration` needs one:
   a function's own name is a *direct* `simple_identifier` child of `KIND_FUN_DECL` itself, so
   climbing from the cursor sitting on that name would otherwise wrongly treat the function's
   own declaration as local to its own body. Verified this doesn't apply to `for_statement`/
   `when_expression`: their bound names sit one level deeper (through `variable_declaration`),
   so climbing from the loop variable's own identifier naturally reaches `variable_declaration`
   first, then `for_statement` — never landing on the boundary node while sitting directly on
   its name the way `function_declaration` does.
4. **Confirmed: Section 3 (same-scope re-declaration) needs the generation-counter rewrite.**
   Fable's own motivating case — `val x: Any = "hello"; val x = x as String` — breaks a naive
   "flatten in tree order, split at each declaration" pass: decl B's own LHS name sits *before*
   its initializer's RHS `x` reference in tree order, so a naive split misattributes the RHS
   self-reference to decl B instead of decl A. Fixed by extending the existing recursive walk's
   `already_shadowed: bool` into a generation counter (see "Sequential re-declaration" below)
   instead of building parallel flatten/tag/group machinery.
5. **Confirmed: forward-reference case is real, and the intellijRulezz marker-insertion trick
   does not apply to it.** That trick (`src/indexer/infer/speculative.rs`) repairs a
   *syntactically broken* parse — `foo.` produces an ERROR node because the member name is
   missing — by inserting a fake identifier so tree-sitter can produce a well-formed tree to
   query. A reference sitting before its own declaration (e.g. mid-refactor, a call dragged
   above its `val`) parses perfectly cleanly — no ERROR node, nothing broken to repair. It's
   syntactically valid Kotlin-shaped CST that merely violates declare-before-use, which
   tree-sitter never checks. Fixed instead by an explicit byte-position rule (see "Forward
   references" below) — no CST trick needed.
6. **Noted, not blocking:** the outward climb re-checks each candidate scope level in turn; no
   memoization across levels. Theoretical `O(depth² · size)` in pathological deep nesting,
   fine in practice given real Kotlin nesting depth. Recorded in "Risks," not a design change.

## Decisions locked with the user

- **Full block scoping**, not the narrower capture-only fix: `if`/`for`/`while`/`when`-branch
  bodies and `try`/`catch`/`finally` all become recognized scope boundaries.
- **Lexical order is in scope too**: same-scope sequential re-declaration
  (`val x = ...; val x = ...`) and forward references are both handled, not deferred.
- **No CST marker-insertion trick for forward references** — confirmed not applicable (finding 5
  above); a plain byte-position check is the correct, simpler fix.
- **Code quality directive, binding on the implementation plan**: no reinventing existing
  machinery, and the result should read as the only correct solution in hindsight — reuse
  `NodeExt::first_child_of_kind` (already imported in `cst_symbol.rs`), the existing `KIND_*`
  constants (`KIND_CATCH_BLOCK`, `KIND_CONTROL_STRUCTURE_BODY`, `KIND_FOR_STMT`, `KIND_WHEN_EXPR`
  already exist in `queries.rs`), and `is_declaration_site` unchanged except the one addition
  above. This directly extends `AGENTS.md`'s "Rust types model behaviour — code should be
  obvious in retrospect" rule to this fix specifically.

## Goals / non-goals

**Goals**

1. `local_scope_occurrences` finds the correct declaring scope for a name via an outward climb
   through *all* recognized scope boundaries, not just the narrowest function/lambda — fixing
   the original captured-not-shadowed Copilot finding.
2. `if`/`for`/`while`/`when`-branch bodies, `try`/`catch`/`finally` bodies each correctly bound
   their own locals — two unrelated sibling `for`/`when`/`if` constructs reusing a name never
   get conflated.
3. Same-scope sequential re-declaration (`val x = ...; val x = ...`) resolves to exactly the
   generation the cursor/reference belongs to, not every generation of that name.
4. A reference textually before any declaration of its name (invalid Kotlin, real during
   mid-typing) safely falls through to the cross-file path — never a silent wrong-scope rename.

**Non-goals**

- Cross-file rename semantics — untouched by this fix (local-only fast path).
- Renaming an override across a type hierarchy — existing, separate non-goal from slice 6c.
- Fixing the general "declared later in the same block that widens a lambda's own body scope"
  precision case, where `find_local_declaration_in_body` may find an unrelated nested
  declaration when checking narrower-than-necessary bodies — the new `declares_name_directly`
  (below) already fixes this as a side effect of the redesign, not called out as its own goal.

## Design

### Reuse inventory (existing — do not reinvent)

- `NodeExt::first_child_of_kind` (`src/indexer/node_ext.rs`) — already imported in
  `cst_symbol.rs`; used to pull a `for_statement`'s `control_structure_body` child, a
  `when_expression`'s `when_subject` child, etc.
- `KIND_CATCH_BLOCK`, `KIND_CONTROL_STRUCTURE_BODY`, `KIND_FOR_STMT`, `KIND_WHEN_EXPR`,
  `KIND_WHEN_SUBJECT` already exist in `queries.rs`. Two are missing and need adding, following
  the existing "Kotlin structural / scope node kinds" section (`queries.rs:410-420`):
  ```rust
  pub(crate) const KIND_TRY_EXPR: &str = "try_expression";
  pub(crate) const KIND_FINALLY_BLOCK: &str = "finally_block";
  ```
- **Independent precedent for the `for_statement` split**: `semantic_tokens/params.rs`'s
  `emit_param_refs_in_scope` already separates a `for_statement`'s header from its
  `control_structure_body` child (`first_child_of_kind(node, KIND_CONTROL_STRUCTURE_BODY)`) for
  a different feature (parameter-shadow highlighting). Cited here as evidence the split this
  design proposes is the idiomatic shape in this codebase, not a novel invention — **not** a
  call site to merge with; that code serves a narrower, differently-shaped purpose
  (params-only shadow tracking) and is out of scope for this fix.
- `is_declaration_site` (`cst_symbol.rs:26`) — unchanged except the one `KIND_CATCH_BLOCK` arm.

### Scope boundaries: a two-shape enum

Not every scope-boundary node has the same shape. `function_declaration`/`lambda_literal`
already need special handling because a name bound in their *header* (parameters) is a sibling
of the body, not inside it — `for_statement`/`when_expression` are the same shape for a
different reason (loop variable / subject binding, respectively). `control_structure_body`,
`catch_block`, `finally_block`, `try_expression` are the other shape: the node itself *is* the
scope, nothing bound outside it. Per the parent design's "Lambda scope as a sum type" rule, this
distinction becomes a type, not a repeated `matches!` at each call site:

```rust
enum ScopeBoundary<'a> {
    /// The scope is the node's entire subtree, including a header-bound name
    /// that sits outside its inner block (`function_declaration`'s own
    /// parameters, `for_statement`'s loop variable, `when_expression`'s
    /// subject binding).
    WholeNode(Node<'a>),
    /// The scope is exactly this node's subtree — a brace-delimited block
    /// with no binding of its own outside it.
    Block(Node<'a>),
}

fn scope_boundary_at(node: Node<'_>) -> Option<ScopeBoundary<'_>> {
    match node.kind() {
        k if k == KIND_FUN_DECL
            || k == KIND_LAMBDA_LIT
            || k == KIND_FOR_STMT
            || k == KIND_WHEN_EXPR =>
        {
            Some(ScopeBoundary::WholeNode(node))
        }
        k if k == KIND_CONTROL_STRUCTURE_BODY
            || k == KIND_CATCH_BLOCK
            || k == KIND_FINALLY_BLOCK
            || k == KIND_TRY_EXPR =>
        {
            Some(ScopeBoundary::Block(node))
        }
        _ => None,
    }
}
```

`enclosing_local_body`, `nested_scope_shadows`, and `visit_unshadowed_name_matches` all switch
their current hardcoded `matches!(kind, FUN_DECL | LAMBDA_LIT)` check to
`scope_boundary_at(node).is_some()`. `enclosing_local_body`'s existing "own name" skip stays,
gated on `parent.kind() == KIND_FUN_DECL` exactly as today (finding 3 confirmed no other boundary
kind needs it).

### The outward climb (fixes the original Copilot finding)

Two walks were previously conflated under one name and must be separated:

1. **"Does this exact scope level declare `name`?"** — a new `declares_name_directly(scope,
   name, bytes) -> Option<Node>` that walks `scope`'s subtree but treats every nested
   `scope_boundary_at` match as **opaque** (does not descend into it at all). This is
   deliberately stricter than the existing `find_local_declaration_in_body`, which recurses into
   nested scopes and could find an unrelated nested lambda's own parameter of the same name —
   imprecise, and the redesign fixes this as a side effect (see "Non-goals").
2. **"Collect every unshadowed occurrence once the declaring scope is known"** — the existing
   `visit_unshadowed_name_matches`/`nested_scope_shadows` machinery, kept, with only its
   boundary-recognition switched to `scope_boundary_at` as above.

`find_local_declaration_in_body` (`cst_symbol.rs:460`) is superseded by `declares_name_directly`
and removed, not kept alongside it as a redundant parallel function — confirmed via
`grep`/Serena reference search that its only caller is `local_scope_occurrences` itself, and
`local_scope_occurrences`'s only caller in turn is `features/rename.rs` (not shared with
highlight, despite slice 6a's original spec proposing to share it — a divergence already noted
above). No other consumer to keep it compatible with.

`local_scope_occurrences` climbs boundaries outward from the cursor, narrowest first, calling
`declares_name_directly` at each level, and uses the **first** level that succeeds as the
declaring scope for the collection walk. It never widens past a level that already owns the
name — this is what prevents two unrelated sibling `for`/`when` constructs reusing a name from
being merged (the single most severe finding Fable raised: without checking each level in order
and stopping at the first hit, an outward climb that skips straight to the enclosing function
body could find a completely unrelated sibling loop's same-named variable).

### Sequential re-declaration (Section 3 rewrite)

Within a `statements` list, sibling declarations of the same name are separate generations.
`already_shadowed: bool` becomes a `generation: usize` counter threaded through the walk,
incremented once after a declaring statement's initializer has finished being visited (not when
the declared name itself is seen) — so `val x: Any = "hello"; val x = x as String` visits the
first `val x`'s declaration at generation 0, does **not** increment yet, visits the second `val
x`'s initializer expression (its `x as String`'s `x` reference) still at generation 0 — correctly
attributing it to the first declaration — and only then increments to generation 1 for the
second declaration itself and anything after it. Nested non-boundary constructs (an `if` inside
the same block, say) inherit whatever generation is current when the walk reaches them; they
don't introduce new generations of their own (that's a different, spatial axis — nested-scope
shadowing — already handled by `nested_scope_shadows`, orthogonal to and composed with this
sequential one). A query for a specific occurrence (cursor position or declaration node) is
resolved to its own generation number first, then only same-generation occurrences are
collected — never every generation of the name in that scope.

### Forward references

Generation number alone under-constrains a forward reference: a reference textually before any
declaration and a same-named declaration appearing later both end up tagged generation 0 (the
counter hasn't incremented yet at either point), so generation-matching alone would wrongly
treat them as one group. The additional rule: an occurrence belongs to generation *G* only if
its byte position is at or after generation *G*'s declaring statement's byte position. A
reference with no declaration at or before it in the same scope (byte-position check fails for
every generation, including generation 0) yields no valid local declaration — `local_scope_
occurrences` returns `None` for that cursor position, exactly like today's fallback when nothing
is found at all, and rename falls through to the cross-file path.

## Error handling

- No declaring scope found anywhere in the outward climb → `None`, cross-file path (unchanged
  behavior, now reached via the wider boundary set instead of only the narrowest body).
- Forward reference with no valid preceding declaration → `None`, same fallback (see above).
- Every other outcome (declaration found, some generation, all occurrences collected) returns
  `Some(Vec<Location>)` exactly as today.

## Testing

- **House decoy, sibling `for`**: two unrelated `for (i in ...)` loops in the same function
  reusing `i`; rename from a reference inside one must only touch that loop's own occurrences —
  asserts the other loop's `i` is absent (Fable's most severe finding, direct regression test).
- **House decoy, sibling `when`**: same shape for two `when (val x = ...)` expressions.
- Capture-without-shadowing: a nested lambda referencing (not redeclaring) an outer local — the
  original Copilot finding — renames the full set across the outer scope and the capturing
  lambda.
- `catch` exception variable: rename from inside the catch body renames the `catch (e: ...)`
  binding and every reference; a second, unrelated `catch (e: ...)` block elsewhere is excluded.
- `if`/`while`/`try`/`finally` block-local `val`: declared and used only inside the block,
  correctly scoped to it (not visible outside, not confused with a same-named outer local).
- Sequential re-declaration: `val x: Any = "hello"; val x = x as String` — renaming the second
  `x` (or a reference after it) must not touch the first declaration or its own initializer's
  string literal; renaming the first must not touch the second.
- Forward reference: a reference to `x` textually before any `val x` in the same scope — asserts
  `local_scope_occurrences` returns `None` (safe fallthrough), not a partial/wrong rename.
- Destructuring, confirmed unaffected: `val (a, b) = pair` and `for ((k, v) in map)` — existing
  `is_declaration_site` behavior, included as a regression pin since finding 1 depended on it.
- Existing `local_scope_occurrences_*` test suite (`cst_symbol.rs`, 4 tests) is the behavior
  floor — must pass unchanged.
- Live probe on the real project before merge: rename a local variable that appears inside a
  nested lambda capture, and a local declared inside an `if`/`when` block, in an actual
  multi-branch function.

## Risks

- Outward climb re-scans each candidate level from scratch with no memoization across levels —
  theoretical `O(depth² · size)`, acceptable given real Kotlin nesting depth (Fable finding 6,
  not blocking).
