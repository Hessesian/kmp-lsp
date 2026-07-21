# CST-Aware Rename (Slice 6c) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `rename.rs`'s text/brace-scan rename (local and cross-file) with a CST-verified rename: a local-variable fast path that walks the actual parse tree (never refuses, never crosses scope), and a cross-file path that reuses 6b's find-references verification pipeline, refuses on ambiguous identity or proven override participation, and includes unverified (`NameScan`) candidates rather than refusing on them.

**Architecture:** Two independent new capabilities (`local_scope_occurrences` in the CST classification domain; `proven_overrides` symmetric override detection in `references_verify.rs`) land first, each fully unit-tested standalone. A third task extracts `find_references_with_qualifier`'s guts into a reusable `verified_references_for` so rename can consume the same `VerifiedReferences` find-references already produces, instead of duplicating recall+verification. A fourth task rewrites `rename.rs`'s `rename_impl` to dispatch local-vs-cross-file and assemble the final edit set, deleting the now-superseded text-scan logic. A fifth task adds the house-decoy and symmetric-refusal integration tests the spec's Testing section requires.

**Tech Stack:** Rust, tree-sitter CST traversal, existing `Indexer`/`resolver`/`features` module structure.

## Global Constraints

- **Prerequisite:** this plan requires `docs/superpowers/plans/2026-07-20-cst-references-6b-hardening.md` fully merged first — Task 2 below calls `Resolver::receiver_type_agreement` (the catalogue trait method that plan adds to `src/resolver/api.rs`, with the `sidecar_budget` parameter it introduces), and builds on its Declaration-arm fix.
- **Catalogue discipline:** `src/resolver/api.rs`'s `Resolver` trait is this codebase's resolution capability catalogue — any new intent-named "resolve X against Y" capability this plan needs goes there as a trait method (`indexer.the_method(...)`), not as a new free function requiring its own manual `mod.rs` re-export. `local_scope_occurrences` (Task 1) is a CST subtree walk, not a resolution-capability lookup — it stays a free function in `cst_symbol.rs`, consistent with that module's own `mod.rs`-is-the-catalogue re-export pattern (`indexer/infer/mod.rs` already re-exports its types and functions from one place; this plan doesn't need to touch that structure).
- Design source of truth: `docs/superpowers/specs/2026-07-19-cst-navigation-design.md`, section "6c — rename". Do not re-derive reasoning already settled there.
- No abbreviated identifiers (AGENTS.md). Use full words throughout, including in every test this plan adds.
- Rename refusals are errors BY DESIGN and must carry a human-readable reason (surfaced as an LSP request error — Helix shows it in the status line). Every other feature in this codebase never errors; do not let that convention bleed into this one.
- `NameScan` residue in the cross-file edit set is INCLUDED, not a refusal condition — only `rejected` (proven wrong) and proven override participation refuse. See the spec's "Policy gate" for the full reasoning; do not re-litigate it in code review.
- Every candidate this pass could prove is a different identity must never be renamed. Every candidate it could prove IS the target identity must be renamed. Everything in between (`NameScan`) is included at today's pre-6b trust level, not a new one.

---

### Task 1: `local_scope_occurrences` — CST-walked local rename fast path

**Files:**
- Modify: `src/indexer/infer/cst_symbol.rs` (new functions, appended after `resolve_identity`)
- Test: `src/indexer/infer/cst_symbol.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `Indexer::live_doc_or_parse`, `crate::indexer::cursor_node_at`, `NodeExt::utf8_text_owned`, `is_declaration_site` (all already exist).
- Produces: `pub(crate) fn local_scope_occurrences(indexer: &Indexer, uri: &Url, cursor_position: Position) -> Option<Vec<Location>>` — `None` when the cursor isn't on a name that is itself declared as a local (`val`/`var`/parameter) inside an enclosing function/lambda body (falls through to the cross-file path in Task 4). `Some(locations)` is every occurrence of that local within its enclosing body, CST-verified — the declaration plus every reference, excluding any occurrence inside a nested function/lambda that itself redeclares the same name (shadowing).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/indexer/infer/cst_symbol.rs` (reuse the existing `indexed_with_live` helper already defined there):

```rust
    #[test]
    fn local_scope_occurrences_collects_declaration_and_every_reference() {
        let (file_uri, indexer) = indexed_with_live(
            "/D.kt",
            "fun run() {\n    val total = 0\n    print(total)\n    print(total)\n}\n",
        );
        // cursor on the declaration ("val total")
        let locations = local_scope_occurrences(
            &indexer,
            &file_uri,
            Position::new(1, 8),
        )
        .expect("total is a local variable");
        assert_eq!(locations.len(), 3, "declaration + 2 references, got {locations:?}");
    }

    #[test]
    fn local_scope_occurrences_works_starting_from_a_reference_not_just_the_declaration() {
        let (file_uri, indexer) = indexed_with_live(
            "/D.kt",
            "fun run() {\n    val total = 0\n    print(total)\n}\n",
        );
        // cursor on the reference inside print(total), not the declaration
        let reference_column = "    print(total)".find("total").unwrap() as u32;
        let locations = local_scope_occurrences(
            &indexer,
            &file_uri,
            Position::new(2, reference_column),
        )
        .expect("total is a local variable, even starting from a reference");
        assert_eq!(locations.len(), 2, "declaration + 1 reference, got {locations:?}");
    }

    #[test]
    fn local_scope_occurrences_excludes_a_shadowing_nested_declaration() {
        let (file_uri, indexer) = indexed_with_live(
            "/D.kt",
            "fun outer() {\n    val total = 0\n    val block = { total: Int ->\n        print(total)\n    }\n    print(total)\n}\n",
        );
        // cursor on the OUTER declaration
        let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(1, 8))
            .expect("total is a local variable");
        // Must include: outer declaration (line 1) + the outer print(total) (line 5).
        // Must NOT include: the lambda's own "total" param (line 2) or its
        // print(total) reference (line 3) -- those refer to the shadowing param.
        assert_eq!(
            locations.len(),
            2,
            "shadowed occurrences inside the nested lambda must be excluded, got {locations:?}"
        );
        assert!(
            locations.iter().all(|location| location.range.start.line == 1
                || location.range.start.line == 5),
            "only outer-scope occurrences (lines 1 and 5) may appear, got {locations:?}"
        );
    }

    #[test]
    fn local_scope_occurrences_returns_none_for_a_non_local_name() {
        let (file_uri, indexer) =
            indexed_with_live("/D.kt", "class User { fun save() {} }\n");
        // cursor on the class name -- not a local of any enclosing function/lambda
        let result = local_scope_occurrences(&indexer, &file_uri, Position::new(0, 8));
        assert!(result.is_none(), "a class-level declaration is not a local, got {result:?}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib local_scope_occurrences -- --nocapture`
Expected: FAIL with "cannot find function `local_scope_occurrences` in this scope" (or similar) — the function doesn't exist yet.

- [ ] **Step 3: Implement `local_scope_occurrences` and its helpers**

Append to `src/indexer/infer/cst_symbol.rs`, after `resolve_identity` and before the `#[cfg(test)]` module:

```rust
/// For the local variable / lambda-parameter the cursor is on (either its
/// declaration or any reference to it), collect every occurrence within its
/// enclosing function/lambda body via a CST subtree walk — no rg, no index,
/// no cross-file verification. Returns `None` when the name under the cursor
/// isn't itself declared as a local inside an enclosing function/lambda body
/// — callers fall through to the cross-file path in that case.
///
/// Every returned `Location` is `CstResolved` by construction: it comes from
/// walking the actual parse tree, not a text scan. A nested function/lambda
/// that redeclares the same name shadows it — occurrences inside that nested
/// scope are excluded, since they refer to the shadowing declaration, not
/// this one.
pub(crate) fn local_scope_occurrences(
    indexer: &Indexer,
    uri: &Url,
    cursor_position: Position,
) -> Option<Vec<Location>> {
    let doc = indexer.live_doc_or_parse(uri)?;
    let cursor = CursorPos {
        line: cursor_position.line as usize,
        utf16_col: cursor_position.character as usize,
    };
    let cursor_node = crate::indexer::cursor_node_at(&doc, cursor)?;
    let name = cursor_node.utf8_text_owned(&doc.bytes)?;
    let body = enclosing_local_body(cursor_node)?;

    // The name under the cursor must itself be declared as a local directly
    // inside `body` (a val/var/parameter) — not a captured outer variable, a
    // class member, or a top-level symbol. Only then is the local fast path
    // valid; anything else falls through to the cross-file path.
    find_local_declaration_in_body(body, &name, &doc.bytes)?;

    let mut occurrence_nodes = Vec::new();
    visit_unshadowed_name_matches(body, &name, false, &doc.bytes, &mut |node| {
        occurrence_nodes.push(node)
    });

    let full_text = std::str::from_utf8(&doc.bytes).ok()?;
    let locations: Vec<Location> = occurrence_nodes
        .into_iter()
        .filter_map(|node| node_to_location(uri, node, full_text))
        .collect();
    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

/// The narrowest enclosing function/lambda body containing `node`.
fn enclosing_local_body(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if matches!(
            parent.kind(),
            k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT
        ) {
            return Some(parent);
        }
        current = parent;
    }
    None
}

/// Returns `true` when `scope` (a nested `fun`/lambda body) itself declares a
/// parameter or local named `name` — i.e. it shadows whatever declared `name`
/// outside `scope`. Does not descend into scopes nested inside `scope` — each
/// nested scope's own shadow status is evaluated independently when the outer
/// walk reaches it.
fn nested_scope_shadows(scope: Node<'_>, name: &str, bytes: &[u8]) -> bool {
    let mut stack = vec![scope];
    while let Some(node) = stack.pop() {
        if is_declaration_site(node) && node.utf8_text_owned(bytes).as_deref() == Some(name) {
            return true;
        }
        if node.id() != scope.id()
            && matches!(
                node.kind(),
                k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT
            )
        {
            continue;
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    false
}

/// Walk `node`'s subtree, calling `visit` on every `simple_identifier` whose
/// text equals `name`, skipping the subtree of any nested `fun`/lambda body
/// that itself redeclares `name` (shadowing).
fn visit_unshadowed_name_matches<'a>(
    node: Node<'a>,
    name: &str,
    already_shadowed: bool,
    bytes: &[u8],
    visit: &mut impl FnMut(Node<'a>),
) {
    if !already_shadowed
        && node.kind() == KIND_SIMPLE_IDENT
        && node.utf8_text_owned(bytes).as_deref() == Some(name)
    {
        visit(node);
    }
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        let child_is_nested_scope = matches!(
            child.kind(),
            k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT
        );
        let child_shadowed = already_shadowed
            || (child_is_nested_scope && nested_scope_shadows(child, name, bytes));
        visit_unshadowed_name_matches(child, name, child_shadowed, bytes, visit);
    }
}

/// The first (outermost, unshadowed) declaration-site node for `name` inside
/// `body`'s subtree, or `None` if `body` doesn't itself declare `name`.
fn find_local_declaration_in_body<'a>(
    body: Node<'a>,
    name: &str,
    bytes: &[u8],
) -> Option<Node<'a>> {
    let mut found = None;
    visit_unshadowed_name_matches(body, name, false, bytes, &mut |node| {
        if found.is_none() && is_declaration_site(node) {
            found = Some(node);
        }
    });
    found
}

/// Convert a tree-sitter node's byte-based position into an LSP `Location`
/// with UTF-16 columns. Assumes `node` is single-line (true for every
/// `simple_identifier` this module deals with).
fn node_to_location(uri: &Url, node: Node<'_>, full_text: &str) -> Option<Location> {
    let row = node.start_position().row;
    let start_byte_column = node.start_position().column;
    let end_byte_column = node.end_position().column;
    let line_text = full_text.lines().nth(row)?;
    let start_character = crate::features::text_utils::utf16_column(&line_text[..start_byte_column]);
    let end_character = crate::features::text_utils::utf16_column(&line_text[..end_byte_column]);
    Some(Location {
        uri: uri.clone(),
        range: tower_lsp::lsp_types::Range::new(
            Position::new(row as u32, start_character),
            Position::new(row as u32, end_character),
        ),
    })
}
```

This needs `KIND_SIMPLE_IDENT` (already imported at the top of the file) and `Location` — check the existing `use` block at the top of `src/indexer/infer/cst_symbol.rs` and add `Location` to the `tower_lsp::lsp_types` import if it isn't already present:

```rust
use tower_lsp::lsp_types::{Location, Position, Url};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib local_scope_occurrences -- --nocapture`
Expected: all four PASS.

*(If `visit_unshadowed_name_matches`'s closure-based recursion hits a lifetime/borrow error — `Node<'a>` inside `&mut impl FnMut(Node<'a>)` across recursive calls is the riskiest part of this sketch to compile on the first try — the fix is almost always making the closure's captured state (`&mut Vec<Node<'a>>` / `&mut Option<Node<'a>>`) explicit rather than inferred; do not restructure the shadow-detection logic itself to work around a compile error without re-reading why the borrow checker rejected it.)*

- [ ] **Step 5: Re-export `local_scope_occurrences` through `src/indexer.rs`**

`cst_symbol.rs`'s public items (`classify_cursor`, `resolve_identity`, `NavigationSource`, etc.) are already re-exported up to `crate::indexer::*` from one place, matching this codebase's established "each module's top file re-exports its own surface" convention — Task 4 (`rename.rs`) needs `crate::indexer::local_scope_occurrences` to resolve, so add it to that same list rather than reaching into `crate::indexer::infer::cst_symbol::local_scope_occurrences` directly.

In `src/indexer.rs`, find:
```rust
    cst_symbol::{
        classify_cursor, classify_symbol_at, is_call_callee, is_declaration_site,
        navigation_member_ident, navigation_receiver_node, resolve_identity, NavigationSource,
        SymbolAtCursor, SymbolRole,
    },
```
Replace with:
```rust
    cst_symbol::{
        classify_cursor, classify_symbol_at, is_call_callee, is_declaration_site,
        local_scope_occurrences, navigation_member_ident, navigation_receiver_node,
        resolve_identity, NavigationSource, SymbolAtCursor, SymbolRole,
    },
```

Run: `cargo build 2>&1 | tail -20`
Expected: clean build — confirms the re-export resolves and nothing else in this list needed adjusting.

- [ ] **Step 6: Commit**

```bash
git add src/indexer/infer/cst_symbol.rs src/indexer.rs
git commit -m "feat(indexer): local_scope_occurrences -- CST-walked local rename fast path

Given a cursor on a local variable or lambda parameter (declaration or any
reference to it), collects every occurrence within its enclosing
function/lambda body via a tree-sitter subtree walk. Excludes occurrences
inside a nested function/lambda that shadows the name. Returns None for
anything that isn't itself a local of an enclosing body, so callers can
fall through to the cross-file path. Every occurrence is CstResolved by
construction -- this is a strict improvement over rename.rs's existing
brace-depth text scan, which this plan's Task 4 replaces."
```

---

### Task 2: `proven_overrides` — symmetric cross-file override detection

**Files:**
- Modify: `src/features/references_verify.rs` (`VerifiedReferences` struct, `verify_candidates` signature and Declaration arm)
- Modify: `src/features/references.rs` (the one existing call site — signature now takes 2 more parameters)
- Test: `src/features/references_verify.rs` (new tests)

**Interfaces:**
- Consumes: `Resolver::receiver_type_agreement(&self, candidate_type, candidate_uri, target_type, sidecar_budget: usize) -> ReceiverTypeAgreement` — the catalogue trait method the 6b-hardening plan adds to `src/resolver/api.rs`. Call as `indexer.receiver_type_agreement(...)`, not as a free function — `references_verify.rs` already imports `Resolver` for this from that plan's Task 1.
- Produces: `VerifiedReferences.proven_overrides: Vec<Location>` — Declaration-role candidates proven, in either direction, to be in an override relationship with the query's declaring type.
- Produces: `verify_candidates(indexer, query_declaring_type: Option<&str>, query_declaring_type_uri: Option<&str>, sidecar_budget: usize, candidates: Vec<Location>) -> VerifiedReferences` — two new parameters. `query_declaring_type_uri` enables the reverse-direction check (only meaningful, and only ever supplied, when the query's declaring type is itself declared at a known URI — see Task 4). `sidecar_budget` replaces the hardcoded `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` the 6b-hardening plan's Declaration-arm fix used inline — find-references passes the interactive default, rename (Task 4) passes a much larger budget so its walk can run to completion.

- [ ] **Step 1: Write the failing tests**

Add to `src/features/references_verify.rs`'s test module:

```rust
    /// The symmetric half of override detection: renaming FROM the concrete
    /// override's own declaration must ALSO detect the interface declaration
    /// as a proven override participant -- not just the forward direction
    /// (querying from the interface, finding the override).
    #[test]
    fn override_detected_symmetrically_from_the_concrete_side() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // Candidate: the INTERFACE's own declaration (line 0).
        let interface_column = source.lines().next().unwrap().find("save").unwrap() as u32;
        let interface_candidate = location(&file_uri, 0, interface_column, interface_column + 4);

        // Query: "DerivedUser" (the CONCRETE/override side), declared at
        // file_uri itself -- exactly the case Task 4 supplies
        // query_declaring_type_uri for (cursor on a Declaration).
        let result = verify_candidates(
            &indexer,
            Some("DerivedUser"),
            Some(file_uri.as_str()),
            usize::MAX,
            vec![interface_candidate.clone()],
        );
        assert_eq!(
            result.proven_overrides,
            vec![interface_candidate],
            "querying from the override side must still detect the interface \
             declaration as a proven override participant"
        );
    }

    /// The forward direction (querying from the interface, the override's
    /// declaration is the candidate) must also populate `proven_overrides` --
    /// not just classify CstResolved.
    #[test]
    fn override_detected_from_the_interface_side_populates_proven_overrides() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let override_column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let override_candidate = location(&file_uri, 1, override_column, override_column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            usize::MAX,
            vec![override_candidate.clone()],
        );
        assert_eq!(result.proven_overrides, vec![override_candidate]);
    }

    /// House decoy: two UNRELATED classes with same-named methods must never
    /// populate `proven_overrides` -- only a proven supertype/subtype
    /// relationship does.
    #[test]
    fn unrelated_same_named_declaration_is_not_a_proven_override() {
        let source = "class User { fun save() {} }\nclass File { fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let file_column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let file_candidate = location(&file_uri, 1, file_column, file_column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            usize::MAX,
            vec![file_candidate],
        );
        assert!(result.proven_overrides.is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib override_detected -- --nocapture` and `cargo test --lib unrelated_same_named_declaration_is_not_a_proven_override -- --nocapture`
Expected: compile failure (`proven_overrides` field and the 5-argument `verify_candidates` don't exist yet) or, once you stub the signature to compile, assertion failure — the field is always empty.

- [ ] **Step 3: Implement `proven_overrides` and the new `verify_candidates` signature**

In `src/features/references_verify.rs`, replace the struct:

```rust
pub(crate) struct VerifiedReferences {
    pub kept: Vec<NavigationSource<Location>>,
    // Intentionally excluded from `find_references_with_qualifier`'s output —
    // dropping proven-unrelated candidates is the whole point of this pass.
    // Read only by this module's own tests, which assert rejection actually
    // happens rather than candidates silently vanishing from `kept`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub rejected: Vec<Location>,
    /// Declaration-role candidates proven, in either direction, to be in an
    /// override relationship with the query's declaring type. Ignored by
    /// find-references (its call always passes `query_declaring_type_uri:
    /// None`, so this is always empty there); consumed only by 6c rename to
    /// decide the override-participation refusal. A candidate here is ALSO
    /// present in `kept` as `CstResolved` — the two fields answer different
    /// questions ("is this the same identity" vs. "does an override relate
    /// to it") and are not mutually exclusive.
    #[cfg_attr(not(test), allow(dead_code))]
    pub proven_overrides: Vec<Location>,
}
```

Replace the `verify_candidates` signature and the `None` early-return:

```rust
pub(crate) fn verify_candidates(
    indexer: &Indexer,
    query_declaring_type: Option<&str>,
    query_declaring_type_uri: Option<&str>,
    sidecar_budget: usize,
    candidates: Vec<Location>,
) -> VerifiedReferences {
    let Some(query_declaring_type) = query_declaring_type else {
        // No query identity — every candidate is exactly today's behavior.
        return VerifiedReferences {
            kept: candidates
                .into_iter()
                .map(NavigationSource::NameScan)
                .collect(),
            rejected: Vec::new(),
            proven_overrides: Vec::new(),
        };
    };
    let query_declaring_type = ReceiverType::from_raw(query_declaring_type.to_owned()).leaf;

    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    let mut proven_overrides = Vec::new();
    let mut io_budget = MAX_VERIFICATION_IO_OPERATIONS;
```

Replace the `Reference` arm's `receiver_type_agreement` call to pass `sidecar_budget` instead of the hardcoded constant (this arm never touches `proven_overrides` — override detection is a Declaration-arm-only concept, a reference *through* an instance is not itself a declaration that could shadow/override anything):

```rust
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                    sidecar_budget,
                ) {
```

Replace the whole `Declaration` arm (this supersedes the 6b-hardening plan's version — that plan's fix is still exactly the forward-direction half of what follows):

```rust
            crate::indexer::SymbolRole::Declaration { .. } => {
                let enclosing_class =
                    indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                match enclosing_class {
                    Some(class_name) => {
                        let candidate_type = ReceiverType::from_raw(class_name).leaf;

                        let forward_will_walk = candidate_type != query_declaring_type
                            && indexer.has_type_definition(&candidate_type);
                        if forward_will_walk {
                            if io_budget == 0 {
                                kept.push(NavigationSource::NameScan(candidate));
                                continue;
                            }
                            io_budget -= 1;
                        }
                        let forward = indexer.receiver_type_agreement(
                            &candidate_type,
                            candidate.uri.as_str(),
                            &query_declaring_type,
                            sidecar_budget,
                        );

                        // Reverse direction: is the QUERY a subtype of the
                        // CANDIDATE's type -- i.e. the cursor is on the
                        // override, and this candidate is the base it
                        // overrides? Only meaningful (and only ever
                        // attempted) when the caller knows the query's own
                        // declaring URI -- see Task 4 in the 6c rename plan
                        // for when that's available.
                        let reverse = query_declaring_type_uri.map(|query_uri| {
                            let reverse_will_walk = query_declaring_type != candidate_type
                                && indexer.has_type_definition(&query_declaring_type);
                            if reverse_will_walk {
                                if io_budget == 0 {
                                    return ReceiverTypeAgreement::Unresolvable;
                                }
                                io_budget -= 1;
                            }
                            indexer.receiver_type_agreement(
                                &query_declaring_type,
                                query_uri,
                                &candidate_type,
                                sidecar_budget,
                            )
                        });

                        let is_proven_override = matches!(forward, ReceiverTypeAgreement::Inherited)
                            || matches!(reverse, Some(ReceiverTypeAgreement::Inherited));
                        if is_proven_override {
                            proven_overrides.push(candidate.clone());
                        }

                        match forward {
                            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                                kept.push(NavigationSource::CstResolved(candidate));
                            }
                            ReceiverTypeAgreement::Unrelated
                            | ReceiverTypeAgreement::Unresolvable => {
                                kept.push(NavigationSource::NameScan(candidate));
                            }
                        }
                    }
                    None => kept.push(NavigationSource::NameScan(candidate)),
                }
            }
```

And the final return:

```rust
    VerifiedReferences {
        kept,
        rejected,
        proven_overrides,
    }
}
```

- [ ] **Step 4: Update `references.rs`'s call site**

In `src/features/references.rs`, `find_references_with_qualifier`'s call to `verify_candidates`:

```rust
    let verified = crate::features::references_verify::verify_candidates(
        index,
        query_declaring_type.as_deref(),
        None,
        crate::resolver::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        locations,
    );
```

Find-references never needs the reverse override check (its `query_declaring_type` always comes from a *reference*'s receiver type per the existing derivation logic just above this call — `SymbolRole::Reference { receiver_type: Some(t), .. } => Some(t.clone())` — or, when the cursor is on a Declaration, `enclosing_class_at` already gives the enclosing type at the SAME `uri`; but find-references has no use for override detection at all, so it passes `None` unconditionally rather than threading a URI it would never consume). Add the import:

```rust
use crate::resolver::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib references_verify:: -- --nocapture`
Expected: all pass, including the three new tests and every pre-existing one (`unrelated_candidate_is_rejected_not_dropped_silently`, `inherited_candidate_is_kept_as_cst_resolved`, `no_query_identity_passes_every_candidate_through_as_name_scan`, `budget_exhaustion_never_rejects_only_skips_verification`, `override_declaration_is_kept_as_cst_resolved_not_name_scan`, `unresolvable_and_exact_agreement_do_not_spend_walk_budget` — the last five updated to pass `None` and a budget as the two new arguments, `usize::MAX` or `MAX_VERIFICATION_IO_OPERATIONS` are both fine choices there since none of them exercise the reverse check).

Run: `cargo test --lib references_tests:: -- --nocapture`
Expected: all pass unchanged — find-references' own behavior must be identical to before this task.

- [ ] **Step 6: Commit**

```bash
git add src/features/references_verify.rs src/features/references.rs
git commit -m "feat(references): symmetric proven_overrides for cross-file rename

VerifiedReferences gains proven_overrides: Vec<Location> -- Declaration-role
candidates proven, in either direction, to be in an override relationship
with the query's declaring type. The forward direction (query is the base,
candidate is the override) reuses the 6b-hardening Declaration-arm fix; the
reverse direction (query is the override, candidate is the base) is new,
gated on the caller supplying query_declaring_type_uri -- only meaningful
when the query's own declaring file is known. find-references passes None
and never populates this field. sidecar_budget is now a verify_candidates
parameter instead of a hardcoded constant, so a caller with a different
latency tolerance (6c rename) can pass a larger one."
```

---

### Task 3: Extract `verified_references_for` for rename to reuse

**Files:**
- Modify: `src/features/references.rs` (extract the guts of `find_references_with_qualifier`)

**Interfaces:**
- Produces: `pub(crate) async fn verified_references_for(name: &str, qualifier: Option<&str>, uri: &Url, position: Position, include_decl: bool, index: &Indexer, sidecar_budget: usize, detect_reverse_overrides: bool) -> (crate::features::references_verify::VerifiedReferences, Option<String>, Option<String>)` — the third and fourth elements of the tuple are `query_declaring_type` and `query_declaring_type_uri` respectively (Task 4 needs both: the type name for the refusal message, the URI because it's already computed here and re-deriving it would duplicate the `classify_cursor` call). `detect_reverse_overrides` is an explicit intent flag, added after this task's own review found a real bug in its first draft (see below) — it gates ONLY whether the reverse-direction override check runs inside this call's own `verify_candidates` invocation, independent of whether a URI happens to be computable.
- Consumes: everything `find_references_with_qualifier` already consumes — this task moves code, it does not write new logic (beyond the explicit gating flag).

**Design note, resolved during this task's own review (not left for Task 4 to discover):** `query_declaring_type_uri` is a real, useful fact about the query regardless of caller — but whether the REVERSE override check should actually run is a separate question a caller must control explicitly. Without `detect_reverse_overrides`, find-references' own call would silently start spending `io_budget` on reverse-direction walks its output never uses (`proven_overrides` is discarded by `find_references_with_qualifier`'s thin wrapper) — a regression to find-references' verification thoroughness on the common base-class/override scenario, contradicting the prior task's own reviewed invariant ("find-references never needs the reverse override check"). Gating on `sidecar_budget`'s specific value (e.g. "only if `usize::MAX`") would be a fragile, implicit coupling between two unrelated concerns — don't do that. The explicit boolean is the correct fix.

- [ ] **Step 1: Extract the function**

In `src/features/references.rs`, change `find_references_with_qualifier` from doing recall + verification + assembly inline to calling a new `verified_references_for`:

```rust
pub(crate) async fn find_references_with_qualifier(
    name: &str,
    qualifier: Option<&str>,
    uri: &Url,
    position: Position,
    include_decl: bool,
    index: &Indexer,
) -> Vec<Location> {
    let (verified, _query_declaring_type, _query_declaring_type_uri) = verified_references_for(
        name,
        qualifier,
        uri,
        position,
        include_decl,
        index,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        false,
    )
    .await;

    let mut resolved_first: Vec<Location> = Vec::with_capacity(verified.kept.len());
    let mut name_scanned: Vec<Location> = Vec::new();
    for source in verified.kept {
        match source {
            NavigationSource::CstResolved(location) => resolved_first.push(location),
            NavigationSource::NameScan(location) => name_scanned.push(location),
        }
    }
    resolved_first.append(&mut name_scanned);
    resolved_first
}

/// Recall (rg + index, unchanged from 6b) plus 6b's per-candidate CST
/// verification, exposed as the raw `VerifiedReferences` — the shared entry
/// point both find-references and 6c rename build on. Also returns the
/// query's declaring type and (when known) its declaring URI, so a caller
/// that needs them (rename's override-participation refusal message) doesn't
/// have to re-run `classify_cursor` itself.
///
/// `detect_reverse_overrides` controls ONLY whether this call's own
/// `verify_candidates` invocation runs the reverse-direction override check —
/// NOT whether `query_declaring_type_uri` is computed (it always is, when
/// derivable, since it's a fact about the query useful to any caller).
/// find-references passes `false` (it never reads `proven_overrides` and
/// must not spend budget on a check it can't use); rename passes `true`.
pub(crate) async fn verified_references_for(
    name: &str,
    qualifier: Option<&str>,
    uri: &Url,
    position: Position,
    include_decl: bool,
    index: &Indexer,
    sidecar_budget: usize,
    detect_reverse_overrides: bool,
) -> (
    crate::features::references_verify::VerifiedReferences,
    Option<String>,
    Option<String>,
) {
    let line = position.line;
    let (parent_class, declared_pkg) =
        resolve_scope_with_qualifier(index, uri, line, name, qualifier);

    let is_jar_symbol_usage = !name.starts_with_uppercase()
        && !index.is_declared_in(uri, name)
        && index.jar_declaration_scope(name).is_some();

    let owner_class = if !name.starts_with_uppercase()
        && declared_pkg.is_some()
        && qualifier.is_none()
        && !is_jar_symbol_usage
    {
        outer_class_for_decl_site(index, uri, line)
    } else {
        None
    };

    let field_owner = if qualifier.is_none() && declared_pkg.is_some() && !is_jar_symbol_usage {
        field_owner_for_decl(index, uri, name, line)
    } else {
        None
    };

    let decl_files = declaration_files_for(
        index,
        name,
        parent_class.as_deref(),
        declared_pkg.as_deref(),
        uri,
    );

    let search = ReferenceSearch {
        uri: uri.clone(),
        name: name.to_string(),
        include_decl,
        parent_class,
        declared_pkg,
        decl_files,
        owner_class,
        field_decl_line: field_owner.is_some().then_some(line),
        field_owner,
    };

    let mut locations = rg_locations(&search, index).await;
    locations.retain(|loc| !index.is_library_uri(&loc.uri));
    if !index.is_library_uri(uri) && !crate::jar_extract::is_extracted_jar_source(uri) {
        add_current_file_locations(
            index,
            uri,
            name,
            search.parent_class.as_deref(),
            search.owner_class.as_deref(),
            include_decl,
            &mut locations,
        );
    }

    let (query_declaring_type, query_declaring_type_uri) = match classify_cursor(index, uri, position) {
        Some(symbol) => match &symbol.role {
            SymbolRole::Declaration { .. } => (
                index.enclosing_class_at(uri, line),
                Some(uri.as_str().to_owned()),
            ),
            SymbolRole::Reference {
                receiver_type: Some(receiver_type),
                ..
            } => (Some(receiver_type.clone()), None),
            _ => (None, None),
        },
        None => (None, None),
    };

    let verify_uri_arg = detect_reverse_overrides
        .then_some(query_declaring_type_uri.as_deref())
        .flatten();
    let verified = crate::features::references_verify::verify_candidates(
        index,
        query_declaring_type.as_deref(),
        verify_uri_arg,
        sidecar_budget,
        locations,
    );
    (verified, query_declaring_type, query_declaring_type_uri)
}
```

This is a pure extraction: every line of logic is unchanged from the current `find_references_with_qualifier`, just moved into the new function. The `query_declaring_type_uri` derivation (`Some(uri.as_str().to_owned())` on the `Declaration` branch — the cursor's own file, exactly the case established in Task 2) is always computed and returned; `detect_reverse_overrides` independently controls whether it's actually forwarded into `verify_candidates`. `find_references_with_qualifier` discards the returned tuple's type/URI via `_query_declaring_type_uri` AND passes `false` for the flag — both are needed, since discarding the tuple alone would not have stopped the reverse walk from running inside `verify_candidates` itself.

- [ ] **Step 2: Run the full references test suite to confirm zero behavior change**

Run: `cargo test --lib references_tests:: references_verify:: -- --nocapture`
Expected: all pass, identical to before this task — `find_references_with_qualifier`'s observable behavior is byte-for-byte unchanged; only its internals moved.

- [ ] **Step 3: Commit**

```bash
git add src/features/references.rs
git commit -m "refactor(references): extract verified_references_for

find_references_with_qualifier's recall+verification guts move into a new
verified_references_for, which also returns the query's declaring type and
(when known) its declaring URI. find_references_with_qualifier becomes a
thin wrapper that flattens VerifiedReferences.kept for its own callers --
behavior unchanged. 6c rename's Task 4 calls verified_references_for
directly to get the full VerifiedReferences, including rejected and
proven_overrides, which find_references_with_qualifier's flattened
Vec<Location> doesn't expose."
```

---

### Task 4: Rewrite `rename_impl` on the verified pipeline

**Files:**
- Modify: `src/features/rename.rs` (replace the cross-file and local rename logic; delete now-dead code)
- Test: `src/features/rename_tests.rs` if it exists, or a new `#[path = "rename_tests.rs"] mod tests` block — check for an existing test file first with `ls src/features/rename_tests.rs` before assuming; if none exists, add a `#[cfg(test)] mod tests` block directly in `rename.rs` matching the pattern `references_verify.rs` and `hierarchy_tests.rs` use.

**Interfaces:**
- Consumes: `local_scope_occurrences` (Task 1), `verified_references_for` (Task 3), `resolve_identity`/`classify_cursor` (existing), `Indexer::is_library_uri` (existing).
- Produces: `rename_impl`'s external signature is UNCHANGED (`pub(crate) async fn rename_impl(indexer: &Arc<Indexer>, uri: &Url, pos: Position, new_name: &str) -> Result<Option<WorkspaceEdit>>`) — the backend adapter calling it needs no changes. Internally, it now returns `Err` (a real LSP error, not `Ok(None)`) for refusal cases, per the spec's "rename refusals are errors BY DESIGN" rule — check how the adapter surfaces `Result::Err` from this function before assuming `Ok(None)` and `Err` are handled identically; if the adapter currently maps `Ok(None)` to "no rename possible, no error shown," confirm it will surface an `Err`'s message as the LSP error the spec requires (Helix status line) rather than swallowing it silently — search `src/backend/*.rs` for the `rename_impl` call site before writing Step 3, and adjust Step 3's error construction to match whatever error type that call site expects (`tower_lsp::jsonrpc::Error` most likely, given the existing `Result` alias in this file is `tower_lsp::jsonrpc::Result`).

- [ ] **Step 1: Find and read the `rename_impl` call site**

Run: `grep -rn "rename_impl" src/backend/*.rs`

Read the matched file(s) to confirm exactly how the returned `Result<Option<WorkspaceEdit>>` is handled today, and what `tower_lsp::jsonrpc::Error` construction (message, code) this codebase already uses elsewhere for a similar user-facing refusal — search `grep -rn "jsonrpc::Error" src/` for an existing pattern to match rather than inventing a new error-construction style.

- [ ] **Step 2: Write the failing tests**

Add tests to `src/features/rename.rs`'s test module (or `rename_tests.rs`, per whichever this codebase already uses — follow Step 1's finding):

```rust
    #[test]
    fn cross_file_rename_refuses_on_override_from_the_interface_side() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n\
                      fun caller(user: User) { user.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // cursor on the interface's own declaration
        let column = source.lines().next().unwrap().find("save").unwrap() as u32;
        let result = futures::executor::block_on(rename_impl(
            &indexer,
            &file_uri,
            Position::new(0, column),
            "persist",
        ));
        assert!(
            result.is_err(),
            "renaming an interface member with a real override must refuse, got {result:?}"
        );
    }

    #[test]
    fn cross_file_rename_refuses_on_override_from_the_concrete_side() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // cursor on the OVERRIDE's own declaration -- the symmetric direction
        let column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let result = futures::executor::block_on(rename_impl(
            &indexer,
            &file_uri,
            Position::new(1, column),
            "persist",
        ));
        assert!(
            result.is_err(),
            "renaming FROM the override side must ALSO refuse -- symmetric with \
             the interface side, got {result:?}"
        );
    }

    #[test]
    fn cross_file_rename_renames_a_clean_no_override_multi_call_site_member() {
        let source = "class Logger { fun log(message: String) {} }\n\
                      fun a(logger: Logger) { logger.log(\"a\") }\n\
                      fun b(logger: Logger) { logger.log(\"b\") }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let column = source.lines().next().unwrap().find("log").unwrap() as u32;
        let result = futures::executor::block_on(rename_impl(
            &indexer,
            &file_uri,
            Position::new(0, column),
            "write",
        ))
        .expect("no override, no ambiguity -- must succeed")
        .expect("must produce an edit");
        let edits = result
            .changes
            .expect("must have changes")
            .remove(&file_uri)
            .expect("must edit this file");
        assert_eq!(
            edits.len(),
            3,
            "declaration + 2 call sites, got {edits:?}"
        );
    }

    #[test]
    fn local_rename_uses_the_cst_fast_path_and_never_refuses() {
        let source = "fun run() {\n    val total = 0\n    print(total)\n}\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let result = futures::executor::block_on(rename_impl(
            &indexer,
            &file_uri,
            Position::new(1, 8),
            "sum",
        ))
        .expect("a local rename must never refuse")
        .expect("must produce an edit");
        let edits = result.changes.expect("must have changes");
        assert_eq!(edits.get(&file_uri).map(Vec::len), Some(2));
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib rename -- --nocapture`
Expected: FAIL (or compile error) — `rename_impl` doesn't yet dispatch to `local_scope_occurrences`/`verified_references_for`, and today's `collect_reference_locations`/`build_workspace_edit` never refuses on anything.

- [ ] **Step 4: Rewrite `rename_impl` and delete superseded code**

In `src/features/rename.rs`, DELETE these now-fully-superseded items: `any_local_var_decl_in_scope`, `cst_cursor_is_method` (test-only helper, no longer needed once local dispatch goes through `local_scope_occurrences`), `enclosing_scope`, `rename_in_scope`, `RenameCursorSymbol`, `resolve_cursor_symbol`, `rename_local_symbol`, `definition_files_for_rename`, `collect_reference_locations`, `reference_candidate_files`, `rename_lines_for_file`, `build_workspace_edit`. Keep `prepare_rename_impl` exactly as-is (it doesn't touch any of this logic).

Replace `rename_impl` with:

```rust
fn workspace_edit_from_locations(locations: &[Location], new_name: &str) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for location in locations {
        changes
            .entry(location.uri.clone())
            .or_default()
            .push(TextEdit {
                range: location.range,
                new_text: new_name.to_owned(),
            });
    }
    for edits in changes.values_mut() {
        edits.sort_by_key(|edit| Reverse(edit.range.start));
    }
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

fn refusal(reason: &str) -> tower_lsp::jsonrpc::Error {
    // Follow whatever construction pattern Step 1 found already in use
    // elsewhere in this codebase for a user-facing jsonrpc::Error; this is a
    // placeholder shape only -- replace `Error::invalid_request` with the
    // actual constructor/code this codebase's existing errors use, found in
    // Step 1's grep, before treating this as final.
    tower_lsp::jsonrpc::Error {
        code: tower_lsp::jsonrpc::ErrorCode::InvalidRequest,
        message: reason.to_owned().into(),
        data: None,
    }
}

pub(crate) async fn rename_impl(
    indexer: &Arc<Indexer>,
    uri: &Url,
    pos: Position,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>> {
    if let Some(locations) = crate::indexer::local_scope_occurrences(indexer, uri, pos) {
        return Ok(Some(workspace_edit_from_locations(&locations, new_name)));
    }

    let Some(symbol) = crate::indexer::classify_cursor(indexer, uri, pos) else {
        return Ok(None);
    };

    let identity = crate::indexer::resolve_identity(&symbol, indexer, uri);
    let crate::indexer::NavigationSource::CstResolved(definitions) = identity else {
        return Err(refusal("identity is ambiguous — could not resolve a single definition"));
    };
    if definitions.len() != 1 {
        return Err(refusal("identity is ambiguous — matches more than one definition"));
    }
    if indexer.is_library_uri(&definitions[0].uri) {
        return Err(refusal("defined in a library — cannot rename a library symbol"));
    }

    let qualifier = None; // rename's cursor site has no dot-qualifier context today; matches prior behavior.
    let (verified, _query_declaring_type, _query_declaring_type_uri) =
        crate::features::references::verified_references_for(
            &symbol.name,
            qualifier,
            uri,
            pos,
            true,
            indexer,
            usize::MAX,
            true, // detect_reverse_overrides: rename needs proven_overrides populated
        )
        .await;

    if !verified.proven_overrides.is_empty() {
        return Err(refusal(
            "renaming an override relationship is not supported — rename the exact \
             declaration you need",
        ));
    }

    let edit_locations: Vec<Location> = verified
        .kept
        .into_iter()
        .map(|source| match source {
            crate::indexer::NavigationSource::CstResolved(location) => location,
            crate::indexer::NavigationSource::NameScan(location) => location,
        })
        .collect();
    if edit_locations.is_empty() {
        return Ok(None);
    }

    Ok(Some(workspace_edit_from_locations(&edit_locations, new_name)))
}
```

Update the imports at the top of `src/features/rename.rs` — remove anything only the deleted functions used (`crate::features::references::resolve_scope`, `crate::features::text_utils::is_keyword_for_file` if `prepare_rename_impl` no longer needs it — check, it still does for the keyword guard; `crate::indexer::cst_cursor_is_local_var` is no longer called directly by this file once `resolve_cursor_symbol` is deleted — confirm via `cargo build`'s unused-import warnings rather than guessing), and add:

```rust
use crate::features::references::verified_references_for;
```

(or qualify inline as shown above — either is fine, match the existing import style in this file).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib rename -- --nocapture`
Expected: all four new tests PASS.

Run: `cargo build 2>&1 | grep -i "warning: unused"`
Expected: no unused-import or unused-function warnings — confirms the deletions in Step 4 were complete (nothing left half-referencing a deleted function).

- [ ] **Step 6: Run the full test suite**

Run: `cargo test --lib 2>&1 | tail -60`
Expected: all pass. Pay particular attention to any existing `rename`-adjacent tests this plan didn't touch directly (e.g. `prepare_rename_impl` tests) — they must be unaffected since that function wasn't modified.

- [ ] **Step 7: Commit**

```bash
git add src/features/rename.rs
git commit -m "feat(rename): CST-verified rename replaces text/brace-scan rename

Local rename now goes through local_scope_occurrences (Task 1) -- a real
CST subtree walk, never refuses, never crosses scope boundaries. Cross-file
rename now goes through verified_references_for (Task 3): refuses on
non-unique or library-owned identity, refuses on proven override
participation in either direction (Task 2), otherwise renames the full
kept set (CstResolved + NameScan, rejected excluded) -- a strict
improvement over today's zero-verification whole-word rg replace, which
this deletes. Deleted now-dead: enclosing_scope, rename_in_scope,
resolve_cursor_symbol, rename_local_symbol, collect_reference_locations,
reference_candidate_files, build_workspace_edit, definition_files_for_rename,
rename_lines_for_file, any_local_var_decl_in_scope, cst_cursor_is_method."
```

---

### Task 5: House-decoy and live-probe verification

**Files:**
- Modify: `src/features/rename.rs`'s test module (or `rename_tests.rs`)

**Interfaces:** None new — this task is tests only, closing the gaps the spec's Testing section and the independent critique both called out.

- [ ] **Step 1: Add the accepted-residual-risk pinning test**

Per the spec's Testing section: "a `NameScan` candidate that is a genuinely different symbol... is the known residual risk this policy accepts: assert that it currently *does* get included in the edit set." Add:

```rust
    /// KNOWN ACCEPTED RISK, pinned deliberately (see the spec's Policy gate):
    /// a NameScan candidate whose receiver type is genuinely unresolvable
    /// (not proven wrong, not proven right) is included in the rename edit
    /// set at today's pre-6b trust level. This is NOT a "this is caught"
    /// test -- it is a "this is the accepted gap" pin. If a future change
    /// narrows this risk, update this test's expectation deliberately; it
    /// must not be allowed to silently start passing for a different reason.
    #[test]
    fn unresolvable_receiver_candidate_is_included_not_excluded() {
        let source = "class User { fun save() {} }\n\
                      fun caller(user: Ghost) { user.save() }\n\
                      fun real(user: User) { user.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let result = futures::executor::block_on(rename_impl(
            &indexer,
            &file_uri,
            Position::new(2, column),
            "persist",
        ))
        .expect("must succeed -- no override, no proven-wrong candidate")
        .expect("must produce an edit");
        let edits = result.changes.expect("must have changes");
        let file_edits = edits.get(&file_uri).expect("must edit this file");
        // The declaration (line 0), the real call site (line 2), AND the
        // Ghost-typed call site (line 1, receiver type unresolvable --
        // classified NameScan, not rejected) are all present: 3 edits.
        assert_eq!(
            file_edits.len(),
            3,
            "the unresolvable-receiver candidate on line 1 must be included, \
             per the accepted-risk policy, got {file_edits:?}"
        );
    }
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test --lib unresolvable_receiver_candidate_is_included_not_excluded -- --nocapture`
Expected: PASS. If it fails with 2 edits instead of 3, `user: Ghost` classified as something OTHER than `Unresolvable` (e.g. the recall scan itself excluded it, or it resolved `Rejected`) — re-check against `src/rg.rs`'s recall matching before assuming the rename logic is wrong; this test's purpose is to PIN current behavior, not to enforce a specific number regardless of what recall actually finds.

- [ ] **Step 3: Live probe on the real project (manual, not automated)**

Per the spec's Testing section, before this plan's branch merges: run a live LSP session against a real project (reuse the pattern from `docs/superpowers/specs/2026-07-19-cst-navigation-design.md`'s prior probes — a Python script driving `textDocument/rename` over stdio JSON-RPC) covering:
1. Rename refusal on a jar/library symbol (a call to an Android SDK method) — must return an LSP error, not a silent no-op.
2. Rename refusal on a real override, from BOTH directions — the interface method and the concrete override, same member, two separate probe requests.
3. A genuine multi-call-site member with NO override relationship (a top-level function or concrete non-overriding method) — must successfully rename every real call site. This is also the measurement the spec's Policy gate flagged as never having been directly gathered — capture the actual CstResolved/NameScan/rejected counts here and record them in the project's `cst-resolution-unification` memory file alongside this session's other findings.

Document the results in the PR description before requesting review — this step has no automated pass/fail; it's a manual gate the spec requires before merge, not a `cargo test` step.

- [ ] **Step 4: Commit**

```bash
git add src/features/rename.rs
git commit -m "test(rename): pin the accepted NameScan-inclusion residual risk

Explicit pinning test for the one behavior the softened Policy gate
deliberately accepts: an unresolvable-receiver candidate (not proven
wrong, not proven right) is included in the rename edit set, not
excluded. This is a pin, not a correctness claim -- if this narrows in
the future, this test's expectation must be updated deliberately."
```

---

## Self-Review Notes

**Spec coverage:**
- Local-variable fast path (spec) → Task 1 (`local_scope_occurrences`) + Task 4's dispatch (`rename_impl`'s first branch).
- Symmetric override detection (spec, resolved per the independent critique) → Task 2 (`proven_overrides`, both directions).
- `NameScan` inclusion policy, `rejected` exclusion (spec's Policy gate) → Task 4's edit-set assembly.
- Non-unique/library identity refusal (spec step 1) → Task 4's `resolve_identity`/`is_library_uri` checks.
- Refusal is a typed LSP error, not `Ok(None)` (spec's Error handling section) → Task 4's `refusal()` helper — **flagged in Task 4 as needing verification against the actual backend adapter and existing error-construction pattern before treating the sketch as final; this is the one piece of this plan not independently confirmed against running code.**
- Testing section's house decoys, both-direction override refusal, clean-fixture full rename, accepted-NameScan-risk pin, live probe → Tasks 4 and 5.

**Ordering dependency on the 6b-hardening plan:** Task 2 of this plan calls `receiver_type_agreement` with a `sidecar_budget` parameter that plan introduces; it also builds directly on that plan's Declaration-arm fix (this plan's Task 2 supersedes that plan's version of the same match arm with a richer one — merge that plan first, or this plan's Task 2 diff will not apply cleanly).

**Type consistency:** `VerifiedReferences` gains `proven_overrides` in Task 2 and is consumed with that exact field name in Task 4. `verify_candidates`'s final 5-parameter signature — `(indexer, query_declaring_type: Option<&str>, query_declaring_type_uri: Option<&str>, sidecar_budget: usize, candidates: Vec<Location>)` — is identical between Task 2 (where it's defined) and Task 3 (where `verified_references_for` calls it). `local_scope_occurrences`'s signature — `(indexer: &Indexer, uri: &Url, cursor_position: Position) -> Option<Vec<Location>>` — is identical between Task 1 (definition) and Task 4 (call site in `rename_impl`).

**Known open risk carried forward, not silently fixed:** a Declaration-role candidate that would prove an override only gets checked if `verify_candidates`'s `io_budget` hasn't already exhausted by the time the loop reaches it — candidate processing order is whatever `rg_locations` produces, not sorted by role. For rename this is mitigated by `sidecar_budget` being `usize::MAX` (Task 4 passes it), but `MAX_VERIFICATION_IO_OPERATIONS` (the candidate-count budget, unchanged at 48) is a SEPARATE cap this plan does not raise for rename — a rename with more than 48 candidates could still miss a late-appearing override if the disk-read charges exhaust `io_budget` before reaching it. This is real and not addressed here; Task 5's live probe step should specifically watch for this on a large real member, and if it manifests, a follow-up plan should front-load Declaration-role candidates or raise `MAX_VERIFICATION_IO_OPERATIONS` for the rename path specifically — do not silently paper over a live-probe finding of this shape by lowering the test's expectations.
