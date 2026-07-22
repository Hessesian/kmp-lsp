# CST-Aware Navigation — Sub-slice 6a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the shared CST symbol-classification core (`classify_symbol_at` + `resolve_identity`) and wire it into go-to-definition, goto-implementation, and document-highlight — each gaining precise, receiver-typed identity while the string engine remains the guaranteed fallback.

**Architecture:** Promote `semantic_tokens`' existing declaration/receiver-typing walk into a shared `indexer/infer/cst_symbol.rs` module rather than writing a new CST pass (independent-critique finding). `classify_symbol_at` uses it to produce a `SymbolAtCursor` (declaration / member-reference-with-typed-receiver / import-segment / not-a-symbol); `resolve_identity` turns that into a `NavigationSource<Definitions>` built on the existing `find_definition_qualified` lookup. Each feature tries the CST path first and falls through to its current behavior — wrapped `NameScan` — when the CST can't establish identity. Spec: `docs/superpowers/specs/2026-07-19-cst-navigation-design.md`.

**Tech Stack:** Rust, tree-sitter, existing `InferDeps`/`CstQuery`/`Resolver` seams.

## Global Constraints

- Scope cut (documented, not silent): 6a classifies **declarations**, **member references with a receiver** (typed via `CstQuery::expr_type`), and **import segments**. Plain local `val`/`var` references and bare unqualified names continue through today's path unchanged, labeled `NameScan` — a general CST-based local-variable declaration resolver (handling nested-scope shadowing, destructuring, `when`-branch bindings) is a bigger undertaking than this sub-slice and is explicitly deferred. `it`/`this`/named-lambda-param references already have full CST + repair-wired machinery (slices 1-4) and are NOT re-derived here — they stay on their existing path (`CursorContext.contextual`), which is already correct.
- Every feature's `NameScan` fallback must reproduce EXACTLY today's behavior — the existing test suites for `definition.rs`, `implementation.rs`, `highlight.rs` are the regression floor and must pass unchanged.
- No feature may ERROR when the CST can't classify — `classify_symbol_at` returning `None` means "use today's path," never a failure.
- House decoy for every wired feature: two unrelated classes with an identically-named member (`User.save()` / `File.save()`) — the CST path must never confuse them.
- Gates per commit: `cargo test` + pre-commit clippy. Final: both clippy profiles, e2e smoke, live probe on the real project.
- Branch: `refactor/cst-navigation`, PR → `refactor/unified-resolution`.

---

### Task 1: Promote the classification helpers out of `semantic_tokens`

**Files:**
- Create: `src/indexer/infer/cst_symbol.rs`
- Modify: `src/indexer/infer/mod.rs` (add `pub(super) mod cst_symbol;` next to the other `mod` declarations; add to the `pub(crate) use self::infer::{...}` re-export block in `src/indexer.rs`)
- Modify: `src/semantic_tokens/helpers.rs` (delete the 4 moved functions)
- Modify: `src/semantic_tokens/resolve.rs` (update the `use super::helpers::{...}` import to pull the moved 4 from the new location)
- Test: existing `semantic_tokens` test suites are the neutrality net — no new tests in this task.

**Interfaces:**
- Produces (later tasks rely on these exact names/signatures, unchanged from their current form in `semantic_tokens/helpers.rs`):
```rust
pub(crate) fn is_declaration_site(node: tree_sitter::Node<'_>) -> bool
pub(crate) fn navigation_receiver_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>>
pub(crate) fn navigation_member_ident(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>>
pub(crate) fn is_call_callee(node: tree_sitter::Node<'_>) -> bool
```

- [ ] **Step 1: Move the code verbatim**

Cut these four functions (with their doc comments) from `src/semantic_tokens/helpers.rs` — `is_declaration_site` (currently `pub(super)`, ~line 236), `navigation_receiver_node` (~361), `navigation_member_ident` (~367), `is_call_callee` (~375) — into a new file:

```rust
//! Shared CST identifier classification: declaration-vs-reference, and
//! receiver/member extraction from a `navigation_expression`.
//!
//! Originally written for semantic-token coloring (`semantic_tokens/resolve.rs`);
//! promoted here because `classify_symbol_at` (the navigation-feature
//! classifier: go-def, goto-impl, highlight) needs the identical walk —
//! two independent CST passes answering "declaration or reference?" and
//! "what's the receiver of this member access?" would drift from each other.

use tree_sitter::Node;

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_DECL, KIND_CLASS_PARAM, KIND_COMPANION_OBJ, KIND_ENUM_ENTRY, KIND_FUN_DECL,
    KIND_NAV_SUFFIX, KIND_OBJECT_DECL, KIND_PARAMETER, KIND_SIMPLE_IDENT, KIND_TYPE_ALIAS,
    KIND_TYPE_IDENT, KIND_TYPE_PARAM, KIND_VAR_DECL,
};

// (paste the four functions here verbatim, changing `pub(super)` to `pub(crate)`)
```

Change every `pub(super)` on the four moved functions to `pub(crate)` (they now cross a module boundary). Keep the `KIND_*` constant imports needed by the moved bodies — check each function against `semantic_tokens/helpers.rs`'s current top-of-file imports to get the exact list (the sketch above is a best-effort list; the compiler will flag anything missing).

- [ ] **Step 2: Register the module and re-export**

In `src/indexer/infer/mod.rs`, add near the other `pub(super) mod` lines:
```rust
pub(super) mod cst_symbol;
```

In `src/indexer.rs`'s existing `pub(crate) use self::infer::{...}` block (the large one that already re-exports `speculative::{...}`, `chain`, etc.), add:
```rust
    cst_symbol::{is_call_callee, is_declaration_site, navigation_member_ident, navigation_receiver_node},
```

- [ ] **Step 3: Fix the semantic_tokens import**

In `src/semantic_tokens/resolve.rs`, the existing line:
```rust
use super::helpers::{
    is_annotation_reference, is_call_callee, is_declaration_site, is_inside_lambda_parameters,
    is_named_argument_label, is_navigation_receiver, is_top_level_call_name, is_type_reference,
    navigation_member_ident, navigation_receiver_node, node_text, push_token, visit_tree,
};
```
becomes two imports — the four moved functions from the new home, the rest unchanged from `helpers`:
```rust
use crate::indexer::{is_call_callee, is_declaration_site, navigation_member_ident, navigation_receiver_node};
use super::helpers::{
    is_annotation_reference, is_inside_lambda_parameters, is_named_argument_label,
    is_navigation_receiver, is_top_level_call_name, is_type_reference, node_text, push_token,
    visit_tree,
};
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result|FAILED"`
Expected: identical pass count to the pre-task baseline (`cargo test --bin kmp-lsp 2>&1 | grep "^test result"` before you start, to know the number) — this is a pure move, zero behavior change.

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "refactor(infer): promote CST declaration/receiver helpers out of semantic_tokens

Shared home for classify_symbol_at (slice 6) — avoids a second CST pass
answering the identical declaration-vs-reference and receiver-extraction
questions semantic_tokens already solved."
```

---

### Task 2: `SymbolAtCursor` + `classify_symbol_at`

**Files:**
- Modify: `src/indexer/infer/cst_symbol.rs` (add the classifier below the promoted helpers)
- Test: inline `#[cfg(test)]` module in the same file.

**Interfaces:**
- Consumes: Task 1's helpers; `Indexer::live_doc_or_parse`/`lambda_doc_at` (`speculative.rs`) for tree acquisition; `cursor_node_at` (`cst_lambda.rs`); `CstQuery::new(...).expr_type()` (`infer/mod.rs`); `KIND_IMPORT_HEADER`, `KIND_NAV_EXPR`, `KIND_STRING_LITERAL`, `KIND_MULTILINE_STRING_LITERAL`, `KIND_LINE_COMMENT`/`KIND_MULTILINE_COMMENT` (check the exact comment-kind constant names in `queries.rs` — grep `COMMENT`).
- Produces (Tasks 3-6 rely on these exact names):
```rust
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
}

pub(crate) enum SymbolRole {
    Declaration,
    /// `receiver_type` is `Some` only when the reference is a member access
    /// (`x.name`) AND the receiver's type resolved via `CstQuery::expr_type`.
    /// `is_call` is true when the reference is the callee of a call_expression.
    Reference { receiver_type: Option<String>, is_call: bool },
    ImportSegment,
}

pub(crate) fn classify_symbol_at(
    indexer: &Indexer, uri: &Url, pos: CursorPos,
) -> Option<SymbolAtCursor>
```

- [ ] **Step 1: Write the failing characterization tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn indexed_with_live(path: &str, src: &str) -> (Url, Indexer) {
        let u = uri(path);
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        (u, idx)
    }

    #[test]
    fn classifies_a_class_declaration() {
        let (u, idx) = indexed_with_live("/D.kt", "class User { val id: Int = 0 }\n");
        // cursor on "User"
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: 8 }).unwrap();
        assert_eq!(sym.name, "User");
        assert!(matches!(sym.role, SymbolRole::Declaration));
    }

    #[test]
    fn classifies_a_typed_member_reference() {
        let src = "class User { fun save() {} }\nfun f(user: User) { user.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        // cursor on "save" in "user.save()"
        let col = src.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 1, utf16_col: col as usize }).unwrap();
        assert_eq!(sym.name, "save");
        match sym.role {
            SymbolRole::Reference { receiver_type: Some(t), is_call: true } => assert_eq!(t, "User"),
            other => panic!("expected typed call reference, got {other:?}"),
        }
    }

    #[test]
    fn no_symbol_inside_a_string_literal() {
        let (u, idx) = indexed_with_live("/D.kt", "fun f() { val s = \"User\" }\n");
        let col = "fun f() { val s = \"".len() as u32;
        assert!(classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: col as usize }).is_none());
    }

    #[test]
    fn classifies_an_import_segment() {
        let (u, idx) = indexed_with_live("/D.kt", "import com.example.User\n");
        let col = "import com.example.".len() as u32;
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: col as usize }).unwrap();
        assert_eq!(sym.name, "User");
        assert!(matches!(sym.role, SymbolRole::ImportSegment));
    }

    /// House decoy: an untypeable receiver must not silently attach a wrong
    /// or stale receiver_type.
    #[test]
    fn untypeable_receiver_yields_no_receiver_type() {
        let src = "fun f(x: Unknown) { x.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        let col = src.find("save").unwrap() as u32;
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: col as usize }).unwrap();
        match sym.role {
            SymbolRole::Reference { receiver_type: None, .. } => {}
            other => panic!("expected no receiver_type, got {other:?}"),
        }
    }
}
```

You'll need `#[derive(Debug)]` on `SymbolRole` for the `{other:?}` panics — add it.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp cst_symbol 2>&1 | tail -10`
Expected: compile errors (types/fn not defined).

- [ ] **Step 3: Implement**

```rust
use crate::indexer::live_tree::lang_for_path;
use crate::indexer::{CstQuery, Indexer, Resolution, ResolveIo};
use crate::queries::{KIND_IMPORT_HEADER, KIND_NAV_EXPR};
use crate::types::CursorPos;
use tower_lsp::lsp_types::Url;

#[derive(Debug, Clone)]
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
}

#[derive(Debug, Clone)]
pub(crate) enum SymbolRole {
    Declaration,
    Reference {
        receiver_type: Option<String>,
        is_call: bool,
    },
    ImportSegment,
}

/// Classify the identifier under `pos`: declaration, member reference (with
/// receiver type resolved via the CST where possible), or import segment.
/// Returns `None` for non-identifier positions (strings, comments,
/// whitespace) — callers treat that exactly like today's "nothing under the
/// cursor" case, never an error.
///
/// Acquisition goes through `lambda_doc_at` so mid-typing states (an
/// unclosed brace above the cursor) still classify against a repaired tree.
pub(crate) fn classify_symbol_at(
    indexer: &Indexer,
    uri: &Url,
    pos: CursorPos,
) -> Option<SymbolAtCursor> {
    let resolution = super::speculative::lambda_doc_at(indexer, uri, pos)?;
    let doc = resolution.doc();
    let node = super::cst_lambda::cursor_node_at(doc, pos)?;

    if !matches!(node.kind(), crate::queries::KIND_SIMPLE_IDENT | crate::queries::KIND_TYPE_IDENT) {
        return None;
    }
    let name = node.utf8_text_owned(&doc.bytes)?;

    if is_declaration_site(node) {
        return Some(SymbolAtCursor { name, role: SymbolRole::Declaration });
    }

    if node
        .parent()
        .is_some_and(|p| p.kind() == KIND_IMPORT_HEADER)
    {
        return Some(SymbolAtCursor { name, role: SymbolRole::ImportSegment });
    }

    // Member reference: the identifier is the member name of a nav_expr's suffix.
    if let Some(nav) = node.parent().and_then(|suffix| {
        (suffix.kind() == crate::queries::KIND_NAV_SUFFIX).then_some(suffix)
    }).and_then(|suffix| suffix.parent()) {
        if nav.kind() == KIND_NAV_EXPR && navigation_member_ident(nav).is_some_and(|m| m.id() == node.id()) {
            let is_call = is_call_callee(nav);
            let receiver_type = navigation_receiver_node(nav).and_then(|receiver| {
                match CstQuery::new(receiver, doc, indexer, uri, ResolveIo::IndexOnly).expr_type() {
                    Resolution::Resolved(t) => Some(t.as_type_str().to_owned()),
                    _ => None,
                }
            });
            return Some(SymbolAtCursor {
                name,
                role: SymbolRole::Reference { receiver_type, is_call: is_call && nav.id() == node.parent()?.parent()?.id() },
            });
        }
    }

    // Bare reference (local var, top-level name, etc.) — no receiver, scope
    // resolution deferred (see Global Constraints). Callers fall through to
    // today's NameScan path for these.
    let is_call = node
        .parent()
        .is_some_and(|p| p.kind() == crate::queries::KIND_CALL_EXPR && p.child(0).map(|c| c.id()) == Some(node.id()));
    Some(SymbolAtCursor {
        name,
        role: SymbolRole::Reference { receiver_type: None, is_call },
    })
}
```

The `is_call` computation inside the nav-suffix branch has a redundant clause (`is_call && nav.id() == ...`) from an early draft — simplify to just `is_call_callee(nav)` (that's already exactly "is this nav_expr itself the callee of an outer call_expression," which is what `is_call_callee` computes given `nav` as the node). Fix this during implementation — the characterization test `classifies_a_typed_member_reference` will catch it if wrong (expects `is_call: true` for `user.save()`).

Note the string/comment case: a cursor inside a string or comment lands on a `string_literal`/`comment` node (or their content), not `simple_identifier`/`type_identifier` — the `matches!` guard at the top handles it, matching the `no_symbol_inside_a_string_literal` decoy.

- [ ] **Step 4: Run and fix until green**

Run: `cargo test --bin kmp-lsp cst_symbol 2>&1 | tail -20`
Debug via `to_sexp()` dumps on any assertion mismatch (same technique used throughout slices 4-5) — tree-sitter's exact node shape for `import_header` paths and nav-suffix nesting may need one iteration to match.

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(infer): SymbolAtCursor + classify_symbol_at — CST symbol classification"
```

---

### Task 3: `NavigationSource<T>` + `resolve_identity`

**Files:**
- Modify: `src/indexer/infer/cst_symbol.rs`
- Test: same file's test module.

**Interfaces:**
- Consumes: Task 2's `SymbolAtCursor`/`SymbolRole`; `Indexer::find_definition_qualified(name, qualifier, uri) -> Vec<Location>` (`indexer/lookup.rs:38`).
- Produces:
```rust
pub(crate) enum NavigationSource<T> {
    CstResolved(T),
    NameScan(T),
}

pub(crate) fn resolve_identity(
    sym: &SymbolAtCursor, indexer: &Indexer, uri: &Url,
) -> NavigationSource<Definitions>
```
(`Definitions` — `resolver/api.rs` — is already `pub(crate)`; import it.)

- [ ] **Step 1: Write the failing decoy tests**

```rust
    /// House decoy: two classes with an identically-named member. A
    /// receiver-typed reference must resolve to the RIGHT one only.
    #[test]
    fn typed_reference_resolves_to_the_correct_same_named_member() {
        let src = "class User { fun save() {} }\n\
                   class File { fun save() {} }\n\
                   fun f(user: User) { user.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 2, utf16_col: col as usize }).unwrap();
        let identity = resolve_identity(&sym, &idx, &u);
        match identity {
            NavigationSource::CstResolved(defs) => {
                assert_eq!(defs.len(), 1);
                assert_eq!(defs[0].range.start.line, 0, "must resolve to User.save, not File.save");
            }
            NavigationSource::NameScan(_) => panic!("typed receiver should resolve CST-resolved"),
        }
    }

    #[test]
    fn declaration_resolves_to_its_own_location() {
        let (u, idx) = indexed_with_live("/D.kt", "class User\n");
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: 8 }).unwrap();
        match resolve_identity(&sym, &idx, &u) {
            NavigationSource::CstResolved(defs) => assert_eq!(defs.len(), 1),
            NavigationSource::NameScan(_) => panic!("declaration must be CstResolved"),
        }
    }

    #[test]
    fn untyped_receiver_falls_back_to_name_scan() {
        let src = "fun f(x: Unknown) { x.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        let col = src.find("save").unwrap() as u32;
        let sym = classify_symbol_at(&idx, &u, CursorPos { line: 0, utf16_col: col as usize }).unwrap();
        assert!(matches!(resolve_identity(&sym, &idx, &u), NavigationSource::NameScan(_)));
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test --bin kmp-lsp resolve_identity 2>&1 | tail -10` (compile error).

- [ ] **Step 3: Implement**

```rust
use crate::resolver::Definitions;

#[derive(Debug)]
pub(crate) enum NavigationSource<T> {
    /// Identity established from the CST + index: precise, ranked first.
    CstResolved(T),
    /// Name-based scan: today's behavior, visibly labeled.
    NameScan(T),
}

/// Resolve `sym`'s identity to its definition site(s).
///
/// `CstResolved` when the CST gave enough information to trust the result
/// (a declaration is trivially its own definition; a receiver-typed member
/// reference is looked up ON that type). `NameScan` for everything the CST
/// couldn't narrow — an untyped receiver, or a bare reference resolved by
/// today's name-based `find_definition_qualified(name, None, uri)` (which
/// can span multiple same-named workspace symbols).
pub(crate) fn resolve_identity(
    sym: &SymbolAtCursor,
    indexer: &Indexer,
    uri: &Url,
) -> NavigationSource<Definitions> {
    match &sym.role {
        SymbolRole::Declaration => {
            NavigationSource::CstResolved(Definitions(indexer.find_definition_qualified(&sym.name, None, uri)))
        }
        SymbolRole::Reference { receiver_type: Some(ty), .. } => {
            let locs = indexer.find_definition_qualified(&sym.name, Some(ty), uri);
            if locs.is_empty() {
                NavigationSource::NameScan(Definitions(locs))
            } else {
                NavigationSource::CstResolved(Definitions(locs))
            }
        }
        SymbolRole::Reference { receiver_type: None, .. } | SymbolRole::ImportSegment => {
            NavigationSource::NameScan(Definitions(indexer.find_definition_qualified(&sym.name, None, uri)))
        }
    }
}
```

Note: `declaration_resolves_to_its_own_location` relies on `find_definition_qualified("User", None, uri)` finding exactly the class itself — verify this is true for a bare class name with no other `User` in the workspace (it should be, via `resolve_symbol`'s local-definitions step). If the test's assertion needs adjustment because a `Declaration`'s OWN location isn't what `find_definition_qualified` returns by name (e.g. it returns the constructor too), adjust the assertion to match observed reality — the point being pinned is "declarations are always CstResolved," not the exact count.

- [ ] **Step 4: Run and fix until green.**

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(infer): NavigationSource + resolve_identity — typed-first symbol-to-definition"
```

---

### Task 4: Wire go-to-definition

**Files:**
- Modify: `src/features/definition.rs` (`find_definition`)
- Modify: `src/backend/nav.rs` (`goto_definition_impl` — needs `position` as `CursorPos`, already has it as `Position`)
- Test: `src/features/definition_tests.rs` if it exists, else create `src/features/definition.rs`'s `#[cfg(test)]` inline module (check which pattern the file's siblings use — `implementation.rs` has `#[path = "implementation_tests.rs"] mod tests;`; follow that).

**Interfaces:**
- Consumes: `classify_symbol_at`, `resolve_identity`, `NavigationSource` (Task 2-3), re-exported through `src/indexer.rs`.

- [ ] **Step 1: Write the failing house-decoy test**

```rust
#[tokio::test]
async fn goto_definition_disambiguates_same_named_members_via_receiver_type() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/D.kt").unwrap();
    let src = "class User { fun save() {} }\n\
               class File { fun save() {} }\n\
               fun f(user: User) { user.save() }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
    let ctx = CursorContext::build(&idx, &uri, Position::new(2, col)).unwrap();
    let response = find_definition(&ctx, &idx, &uri, Position::new(2, col)).await.unwrap();
    let loc = match response {
        GotoDefinitionResponse::Scalar(l) => l,
        other => panic!("expected a single location, got {other:?}"),
    };
    assert_eq!(loc.range.start.line, 0, "must jump to User.save, not File.save");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp goto_definition_disambiguates 2>&1 | tail -15`
Expected: FAIL — today's `find_definition` resolves `save` via `ctx.word`/`ctx.qualifier` (text `"user"`), which is NOT type-directed the same way; verify by running first whether it happens to pass already (if `find_definition_qualified("save", Some("user"), uri)` already resolves correctly via the string-qualifier path, this decoy won't be RED — in that case, use `user: User` interchangeably with a **local var whose declared type differs from its name-implied type**, e.g. rename the parameter to `x: User` so the qualifier text `"x"` carries no type information at all, forcing today's path to guess by container-name matching and get it wrong or ambiguous). Adjust the fixture until the decoy is genuinely RED before proceeding — this is required, not optional (RED-first).

- [ ] **Step 3: Implement**

In `src/features/definition.rs`, add near the top of `find_definition` (before the existing `this`/`super` special cases — those stay first since they're keyword-driven, not identity-classified):

```rust
pub(crate) async fn find_definition(
    ctx: &CursorContext,
    index: &(impl SymbolIndex + DocumentAccess + SearchAccess),
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    // CST-resolved path first: precise for declarations and receiver-typed
    // member references. Falls through to the string-first path below for
    // everything the CST can't narrow (locals, untyped receivers, keywords).
    if let Some(cst_response) = try_cst_resolved_definition(index, uri, position) {
        return Some(cst_response);
    }

    // `this` → enclosing class definition.
    ...
```

Add the helper (needs `index` to be `&Indexer` for `classify_symbol_at`/`resolve_identity` — check whether `SymbolIndex + DocumentAccess + SearchAccess` is generic enough or whether this function needs a concrete `&Indexer` parameter threaded in from the caller; `Indexer` is the only real implementor of these traits in production, and `backend/nav.rs` already has `&*self.indexer: &Indexer` — thread it through as an additional parameter rather than fighting the trait bounds):

```rust
fn try_cst_resolved_definition(
    indexer: &crate::indexer::Indexer,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let cursor = crate::types::CursorPos {
        line: position.line as usize,
        utf16_col: position.character as usize,
    };
    let sym = crate::indexer::classify_symbol_at(indexer, uri, cursor)?;
    match crate::indexer::resolve_identity(&sym, indexer, uri) {
        crate::indexer::NavigationSource::CstResolved(defs) if !defs.is_empty() => {
            locs_to_opt_response(defs.0)
        }
        _ => None,
    }
}
```

Update the signature of `find_definition` to take `indexer: &crate::indexer::Indexer` as an explicit extra parameter (alongside the existing generic `index`), and update the one call site in `src/backend/nav.rs`:
```rust
let response = def::find_definition(&ctx, &*self.indexer, &self.indexer, uri, position).await;
```
(Passing `&*self.indexer` twice — once through the existing generic bound, once concretely — is a bit awkward; if it compiles cleanly and reads confusingly, an alternative is to change `find_definition`'s generic bound from `impl SymbolIndex + DocumentAccess + SearchAccess` to a concrete `&Indexer` throughout, since `Indexer` is the only production implementor — check whether any TEST fixture relies on a fake implementor of just those traits before making that change; if none does, the concrete-type simplification is cleaner and preferred.)

Add the necessary re-exports to `src/indexer.rs`'s `pub(crate) use self::infer::{...}` block:
```rust
    cst_symbol::{classify_symbol_at, resolve_identity, NavigationSource, SymbolAtCursor, SymbolRole},
```

- [ ] **Step 4: Run the decoy + full suite**

Run: `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result|FAILED"`
Expected: decoy passes; every existing `definition`-related test passes unchanged (NameScan parity).

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(definition): go-to-definition tries CST-resolved identity first"
```

---

### Task 5: Wire goto-implementation (fixes the call-site gap)

**Files:**
- Modify: `src/backend/nav.rs` (`goto_implementation_impl`)
- Modify: `src/features/implementation.rs` (`find_implementation` — new entry variant)
- Test: `src/features/implementation_tests.rs`

**Interfaces:**
- Consumes: `classify_symbol_at` (Task 2).

- [ ] **Step 1: Write the failing decoy**

Confirmed gap (verified against the code): `goto_implementation_impl` always calls `find_implementation(&ctx.word, indexer, uri, position.line)`, and `declaring_class_of_method` only matches when `position.line` equals the EXACT declaration line of a method named `word` in the current file — so invoking goto-implementation from a CALL SITE (`service.load()`, not the `fun load()` declaration itself) falls through to the type-implementations path treating `"load"` as a type name, and returns nothing.

```rust
#[tokio::test]
async fn goto_implementation_from_a_call_site_finds_overrides() {
    let idx = Indexer::new();
    let iface_uri = Url::parse("file:///t/IService.kt").unwrap();
    let impl_uri = Url::parse("file:///t/RealService.kt").unwrap();
    let call_uri = Url::parse("file:///t/Caller.kt").unwrap();
    idx.index_content(&iface_uri, "interface IService { fun load() }\n");
    idx.index_content(
        &impl_uri,
        "class RealService : IService { override fun load() {} }\n",
    );
    let call_src = "fun f(service: IService) { service.load() }\n";
    idx.index_content(&call_uri, call_src);
    idx.store_live_tree(&call_uri, call_src);
    let col = call_src.find("load").unwrap() as u32;

    let response = find_implementation_at(&idx, &call_uri, Position::new(0, col)).await;
    let GotoDefinitionResponse::Array(locs) = response.expect("must find the override") else {
        panic!("expected an array response");
    };
    assert!(locs.iter().any(|l| l.uri == impl_uri));
}
```

(`find_implementation_at` is the new entry point this task adds — write the test against the name you're about to implement, matching the plan's Interfaces.)

- [ ] **Step 2: Run to verify failure** — compile error (function doesn't exist yet) or, once stubbed to call the OLD `find_implementation(word, ...)`, a runtime failure (empty result) confirming the gap.

- [ ] **Step 3: Implement**

Add a new entry point in `implementation.rs` that tries CST classification first:

```rust
/// Find implementations for the symbol at `position` — CST-resolved first
/// (works from a CALL SITE via the receiver's type, not just the declaration
/// line), falling back to the existing name+line path.
pub(crate) async fn find_implementation_at(
    indexer: &crate::indexer::Indexer,
    uri: &Url,
    position: tower_lsp::lsp_types::Position,
) -> Option<GotoDefinitionResponse> {
    let cursor = crate::types::CursorPos {
        line: position.line as usize,
        utf16_col: position.character as usize,
    };
    if let Some(sym) = crate::indexer::classify_symbol_at(indexer, uri, cursor) {
        if let crate::indexer::SymbolRole::Reference { receiver_type: Some(ty), is_call: true } = &sym.role {
            if let Some(response) = find_method_implementations(&sym.name, ty, indexer, uri).await {
                return Some(response);
            }
        }
    }
    let (word, _) = indexer.word_and_qualifier_at(uri, position)?;
    find_implementation(&word, indexer, uri, position.line).await
}
```

(`find_method_implementations` is already `pub(crate)`-visible within the module — check its current visibility is at least `pub(super)`/accessible from this new function in the same file; it is, since both live in `implementation.rs`.)

Update `src/backend/nav.rs`'s `goto_implementation_impl` to call `imp::find_implementation_at(&*self.indexer, uri, position).await` instead of the old `find_implementation(&ctx.word, ...)` call — the `CursorContext::build` call above it becomes unused for this handler; check whether `ctx` is used for anything else in that function (it isn't, per the code read during planning) and remove the now-dead `CursorContext::build` call and its `let Some(ctx) = ... else { return Ok(None) }` guard, replacing the early-return with a direct `Option`-based flow through `find_implementation_at`.

- [ ] **Step 4: Run and fix until green; full suite.**

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(implementation): goto-implementation resolves from call sites via receiver type"
```

---

### Task 6: Fix document-highlight scoping

**Files:**
- Modify: `src/features/highlight.rs`
- Test: create `src/features/highlight_tests.rs` (the file currently has none) + `#[path = "highlight_tests.rs"] mod tests;` at the bottom of `highlight.rs`, matching the sibling pattern in `implementation.rs`.

**Interfaces:**
- Consumes: `classify_symbol_at` (Task 2); a new local helper `enclosing_body`.

- [ ] **Step 1: Write the failing decoy (confirmed bug: today highlights the whole file)**

```rust
use super::compute_document_highlight;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{Position, Url};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

fn indexed_with_live(path: &str, src: &str) -> (Url, Indexer) {
    let u = uri(path);
    let idx = Indexer::new();
    idx.index_content(&u, src);
    idx.store_live_tree(&u, src);
    idx.set_live_lines(&u, src);
    (u, idx)
}

/// Confirmed bug: a local variable named `total` in one function must not
/// highlight an unrelated local ALSO named `total` in a different function.
#[test]
fn highlight_does_not_cross_function_boundaries() {
    let src = "fun a() {\n    val total = 1\n    println(total)\n}\n\
               fun b() {\n    val total = 2\n    println(total)\n}\n";
    let (u, idx) = indexed_with_live("/H.kt", src);
    // cursor on `total` inside fn a() (line 2, the println use).
    let highlights = compute_document_highlight(&u, Position::new(2, 14), &idx).unwrap();
    assert_eq!(
        highlights.len(),
        2,
        "must highlight only fn a()'s two occurrences, not fn b()'s — got {highlights:?}"
    );
    assert!(highlights.iter().all(|h| h.range.start.line <= 2));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp highlight_does_not_cross 2>&1 | tail -10`
Expected: FAIL — 4 highlights (today's whole-file word match finds both functions' `total`).

- [ ] **Step 3: Implement**

Add to `src/features/highlight.rs`:

```rust
/// The narrowest enclosing function/lambda body (or the whole file if
/// neither exists) containing `node` — the boundary document-highlight
/// searches within. Coarser than full lexical scoping (doesn't distinguish
/// nested shadowing), but it NEVER crosses into an unrelated function, which
/// is the bug this exists to fix.
fn enclosing_body(node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if matches!(parent.kind(), k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT) {
            return parent;
        }
        cur = parent;
    }
    cur
}
```

Rewrite `compute_document_highlight` to scope the search when the CST classifies the cursor as a `Declaration` or an unqualified local `Reference` (the cases `classify_symbol_at` can place inside a specific function body), keeping today's whole-file behavior for everything else:

```rust
pub(crate) fn compute_document_highlight(
    uri: &Url,
    pos: Position,
    index: &(impl SymbolIndex + DocumentAccess),
) -> Option<Vec<DocumentHighlight>> {
    let (name, _) = index.word_and_qualifier_at(uri, pos)?;
    let lines = index.mem_lines_for(uri.as_str())?;

    // Scope narrowing: only for the Indexer concrete type (classify_symbol_at
    // needs it) — DocumentAccess/SymbolIndex are trait objects in tests, so
    // this block is best-effort and falls through to the whole-file scan
    // when a concrete Indexer isn't available. `compute_document_highlight`'s
    // real caller (backend/handlers.rs) always passes a concrete Indexer.
    let scope_range: Option<Range> = index
        .as_indexer()
        .and_then(|indexer| {
            let cursor = crate::types::CursorPos { line: pos.line as usize, utf16_col: pos.character as usize };
            let doc = indexer.live_doc_or_parse(uri)?;
            let node = crate::indexer::cursor_node_at(&doc, cursor)?;
            let body = enclosing_body(node);
            Some(Range::new(
                Position::new(body.start_position().row as u32, 0),
                Position::new(body.end_position().row as u32, u32::MAX),
            ))
        });

    let decl_lines: std::collections::HashSet<u32> = index
        .definition_locations(&name)
        .into_iter()
        .filter(|loc| loc.uri == *uri)
        .map(|loc| loc.range.start.line)
        .collect();

    let mut highlights = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        let line_idx = line_idx as u32;
        if let Some(ref scope) = scope_range {
            if line_idx < scope.start.line || line_idx > scope.end.line {
                continue;
            }
        }
        for abs in word_byte_offsets(line, &name) {
            let col = utf16_column(&line[..abs]);
            let col_end = col + utf16_column(&name);
            let range = Range::new(Position::new(line_idx, col), Position::new(line_idx, col_end));
            let kind = if decl_lines.contains(&line_idx) {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            };
            highlights.push(DocumentHighlight { range, kind: Some(kind) });
        }
    }

    (!highlights.is_empty()).then_some(highlights)
}
```

This references `index.as_indexer()` — check whether `SymbolIndex`/`DocumentAccess` already has a downcast-to-concrete-`Indexer` escape hatch (grep `as_indexer\|fn as_any` in `features/traits.rs`). If none exists, the simplest correct alternative (preferred — avoids inventing a new trait method for one caller) is to change `compute_document_highlight`'s signature to take `index: &crate::indexer::Indexer` directly instead of the generic bound, since — like `find_definition` in Task 4 — `Indexer` is the only production implementor; verify no test fixture needs the generic bound before doing this, then update the one call site in `src/backend/handlers.rs:157`.

- [ ] **Step 4: Run and fix until green; full suite green.**

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "fix(highlight): scope occurrences to the enclosing function — no more cross-function bleed"
```

---

### Task 7: Gates, live probe, PR

- [ ] **Step 1:** `cargo test 2>&1 | tail -5 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -2 && cargo clippy --release --all-targets -- -D warnings 2>&1 | tail -2 && cargo test --test lsp_smoke 2>&1 | tail -3`

- [ ] **Step 2:** Build (`cargo build`) and live-probe on Moneta (adapt `scratchpad/lsp_probe_cst.py`'s harness): go-to-definition from a Compose member call (`Modifier.padding` or similar) resolves to the correct symbol; goto-implementation from a call site on an interface method (pick a real `interface`/`override` pair in the project) returns the override; document-highlight on a local variable stays within its function. BIN must point at the worktree `target/debug/kmp-lsp`.

- [ ] **Step 3:** Push, open PR → `refactor/unified-resolution`. PR body: what shipped (classifier + 3 wired features), the confirmed pre-existing bugs fixed as a side effect (goto-impl-from-call-site returned nothing; highlight had zero scoping), the documented scope cut (general local-variable CST resolution deferred), decoy list, probe results.

- [ ] **Step 4:** Ledger entry + memory update. Note in both: 6b (find-references) is next, building directly on `classify_symbol_at`/`resolve_identity`; 6c (rename) after that, gated on 6c's own live-probe measurement of cross-file refusal rate per the spec's flagged policy question.

## Self-review notes

- Spec coverage: classifier core (spec §Core) = Tasks 1-3, with Task 1 directly implementing the F1 critique fix (reuse, not reinvent) and Task 3 directly implementing the F7 critique fix (named `resolve_identity`). Go-def/goto-impl/highlight (spec §6a) = Tasks 4-6. `local_scope_occurrences` (spec/F9) is intentionally NOT built as a separate general primitive in this plan — Task 6's `enclosing_body` is a deliberately narrower, sufficient version for highlight's confirmed bug; a fuller `local_scope_occurrences` usable by 6c's local-rename fast path is deferred to 6c's own plan, which can generalize `enclosing_body` once rename's exact needs (edit-set enumeration, not just a boolean scope check) are in front of it — recorded here so 6c's planning doesn't rediscover this from scratch.
- Documented scope cut: general local-`val`/`var` CST declaration resolution (nested shadowing, destructuring, `when`-bindings) is explicitly deferred — Global Constraints explains why (bigger than one sub-slice) and what stays on today's path in the meantime (unchanged, NameScan-labeled).
- Type consistency: `SymbolAtCursor`/`SymbolRole`/`NavigationSource`/`classify_symbol_at`/`resolve_identity` signatures introduced in Tasks 2-3 are used identically in Tasks 4-6.
- Known judgment points flagged for the implementer: Task 4's generic-bound-vs-concrete-`Indexer` question (resolve by checking test fixtures, prefer concrete); Task 6's `as_indexer` escape hatch (same resolution); Task 4 Step 2's RED-verification requirement (the decoy fixture must be adjusted if the naive version happens to already pass).
