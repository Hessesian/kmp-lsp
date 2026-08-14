//! Tests for [`classify_symbol_at`], [`resolve_identity`], and
//! [`local_scope_occurrences`] -- CST identifier classification and the
//! local-variable rename fast path.

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
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: 8,
        },
    )
    .unwrap();
    assert_eq!(sym.name, "User");
    assert!(matches!(
        sym.role,
        SymbolRole::Declaration { indexed: true }
    ));
}

#[test]
fn classifies_a_typed_member_reference() {
    let src = "class User { fun save() {} }\nfun f(user: User) { user.save() }\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    // cursor on "save" in "user.save()"
    let col = src.lines().nth(1).unwrap().find("save").unwrap() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 1,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    assert_eq!(sym.name, "save");
    match sym.role {
        SymbolRole::Reference {
            receiver_type: Some(t),
            is_call: true,
            ..
        } => assert_eq!(t, "User"),
        other => panic!("expected typed call reference, got {other:?}"),
    }
}

#[test]
fn no_symbol_inside_a_string_literal() {
    let (u, idx) = indexed_with_live("/D.kt", "fun f() { val s = \"User\" }\n");
    let col = "fun f() { val s = \"".len() as u32;
    assert!(classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: col as usize
        }
    )
    .is_none());
}

#[test]
fn classifies_an_import_segment() {
    let (u, idx) = indexed_with_live("/D.kt", "import com.example.User\n");
    let col = "import com.example.".len() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    assert_eq!(sym.name, "User");
    assert!(matches!(sym.role, SymbolRole::ImportSegment));
}

/// The cursor's own ancestor chain sits inside an `ERROR` node — deeply
/// nested unclosed call args (`foo(bar(baz(qux`), not just an unrelated
/// MISSING-semicolon artifact elsewhere in the file. `lambda_doc_at`'s
/// brace-repair only accepts a candidate whose cursor gains an enclosing
/// `lambda_literal`; none of these unclosed parens can ever become one
/// (they're call-argument lists, not lambda braces), so repair exhausts
/// `MAX_BRACE_REPAIRS` and `lambda_doc_at` returns `None` — the raw-tree
/// fallback in `classify_symbol_at` is what actually serves this request.
/// Verified empirically (see fix report): `lambda_doc_at` returns `None`
/// for this exact snippet/position, and the cursor's ancestor chain is
/// `["simple_identifier", "value_argument", "ERROR"]`.
///
/// Every check in `classify_symbol_at` after acquiring the doc is an
/// exact `node.kind() == ...` match against the identifier's *parent*
/// kind; here that parent is `ERROR`, which matches none of them, so the
/// function falls closed to the bare-reference case with no fabricated
/// receiver/call info — never a wrong classification.
#[test]
fn safely_degrades_when_cursor_sits_inside_an_error_node() {
    let src = "class User {\nfun f() {\nif (foo(bar(baz(qux\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    let col = src.lines().nth(2).unwrap().find("qux").unwrap();
    let pos = CursorPos {
        line: 2,
        utf16_col: col,
    };

    // Empirical precondition: lambda_doc_at must actually fail here, or
    // this test isn't exercising the raw-tree fallback at all.
    assert!(
        super::super::speculative::lambda_doc_at(&idx, &u, pos).is_none(),
        "expected lambda_doc_at to return None (brace repair exhausted) \
         so classify_symbol_at's raw-tree fallback is what's under test"
    );

    // Empirical precondition: the cursor's own node sits inside an ERROR
    // node, not just somewhere unrelated in the tree.
    let doc = idx.live_doc_or_parse(&u).unwrap();
    let node = super::super::cst_lambda::cursor_node_at(&doc, pos).unwrap();
    assert_eq!(node.kind(), KIND_SIMPLE_IDENT);
    assert_eq!(node.utf8_text(&doc.bytes).unwrap(), "qux");
    assert_eq!(node.parent().unwrap().parent().unwrap().kind(), "ERROR");

    // The actual behavior under test: no panic, and no fabricated
    // classification. `qux`'s immediate parent is `value_argument`
    // inside the ERROR node — none of is_declaration_site's or the
    // member-reference branch's exact-kind checks match an ERROR
    // ancestor, so this falls through to the bare-reference case with
    // name echoed verbatim and nothing fabricated (no receiver, not
    // marked as a call).
    let sym = classify_symbol_at(&idx, &u, pos).expect("falls to bare reference, not None");
    assert_eq!(sym.name, "qux");
    match sym.role {
        SymbolRole::Reference {
            receiver_type: None,
            is_call: false,
            ..
        } => {}
        other => panic!("expected bare, unfabricated reference, got {other:?}"),
    }
}

/// House decoy: an untypeable receiver must not silently attach a wrong
/// or stale receiver_type.
#[test]
fn untypeable_receiver_yields_no_receiver_type() {
    let src = "fun f(x: Unknown) { x.save() }\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    let col = src.find("save").unwrap() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    match sym.role {
        SymbolRole::Reference {
            receiver_type: None,
            ..
        } => {}
        other => panic!("expected no receiver_type, got {other:?}"),
    }
}

/// House decoy: two classes with an identically-named member. A
/// receiver-typed reference must resolve to the RIGHT one only.
#[test]
fn typed_reference_resolves_to_the_correct_same_named_member() {
    let src = "class User { fun save() {} }\n\
               class File { fun save() {} }\n\
               fun f(user: User) { user.save() }\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 2,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    let identity = resolve_identity(&sym, &idx, &u);
    match identity {
        NavigationSource::CstResolved(defs) => {
            assert_eq!(defs.len(), 1);
            assert_eq!(
                defs[0].range.start.line, 0,
                "must resolve to User.save, not File.save"
            );
        }
        NavigationSource::NameScan(_) => panic!("typed receiver should resolve CST-resolved"),
    }
}

#[test]
fn declaration_resolves_to_its_own_location() {
    let (u, idx) = indexed_with_live("/D.kt", "class User\n");
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: 8,
        },
    )
    .unwrap();
    match resolve_identity(&sym, &idx, &u) {
        NavigationSource::CstResolved(defs) => assert_eq!(defs.len(), 1),
        NavigationSource::NameScan(_) => panic!("declaration must be CstResolved"),
    }
}

/// Reviewer-reported gap (task-3 review): a bare function parameter is a
/// `Declaration` per `is_declaration_site`, but `KOTLIN_DEFINITIONS`
/// (`queries.rs`) never indexes plain `parameter` nodes into `f.symbols`
/// — only class/object/interface/fun/property/enum-entry/companion/
/// type-alias and `val`/`var` constructor params are indexed. A
/// name-based lookup for an unindexed declaration falls through to
/// `find_local_declaration`'s same-file first-textual-match scan, which
/// isn't anchored to the cursor: with two functions that both declare a
/// parameter named `id`, the cursor on the SECOND function's `id`
/// parameter must not be silently resolved (as `CstResolved`) to the
/// FIRST function's `id`.
#[test]
fn unindexed_param_declaration_is_namescan_not_cst_resolved() {
    let src = "fun a(id: Int) {}\nfun b(id: String) { println(id) }\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    // cursor on the declaration-site "id" of `b`'s parameter (first
    // occurrence on line 1 — the parameter, not the `println(id)` usage).
    let col = src.lines().nth(1).unwrap().find("id").unwrap() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 1,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    assert_eq!(sym.name, "id");
    assert!(
        matches!(sym.role, SymbolRole::Declaration { indexed: false }),
        "expected an unindexed Declaration, got {:?}",
        sym.role
    );
    match resolve_identity(&sym, &idx, &u) {
        NavigationSource::NameScan(_) => {}
        NavigationSource::CstResolved(defs) => panic!(
            "unindexed param declaration must not be CstResolved (got line {:?}, expected NameScan)",
            defs.first().map(|d| d.range.start.line)
        ),
    }
}

#[test]
fn untyped_receiver_falls_back_to_name_scan() {
    let src = "fun f(x: Unknown) { x.save() }\n";
    let (u, idx) = indexed_with_live("/D.kt", src);
    let col = src.find("save").unwrap() as u32;
    let sym = classify_symbol_at(
        &idx,
        &u,
        CursorPos {
            line: 0,
            utf16_col: col as usize,
        },
    )
    .unwrap();
    assert!(matches!(
        resolve_identity(&sym, &idx, &u),
        NavigationSource::NameScan(_)
    ));
}

#[test]
fn local_scope_occurrences_collects_declaration_and_every_reference() {
    let (file_uri, indexer) = indexed_with_live(
        "/D.kt",
        "fun run() {\n    val total = 0\n    print(total)\n    print(total)\n}\n",
    );
    // cursor on the declaration ("val total")
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(1, 8))
        .expect("total is a local variable");
    assert_eq!(
        locations.len(),
        3,
        "declaration + 2 references, got {locations:?}"
    );
}

#[test]
fn local_scope_occurrences_works_starting_from_a_reference_not_just_the_declaration() {
    let (file_uri, indexer) = indexed_with_live(
        "/D.kt",
        "fun run() {\n    val total = 0\n    print(total)\n}\n",
    );
    // cursor on the reference inside print(total), not the declaration
    let reference_column = "    print(total)".find("total").unwrap() as u32;
    let locations =
        local_scope_occurrences(&indexer, &file_uri, Position::new(2, reference_column))
            .expect("total is a local variable, even starting from a reference");
    assert_eq!(
        locations.len(),
        2,
        "declaration + 1 reference, got {locations:?}"
    );
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
        locations
            .iter()
            .all(|location| location.range.start.line == 1 || location.range.start.line == 5),
        "only outer-scope occurrences (lines 1 and 5) may appear, got {locations:?}"
    );
}

#[test]
fn local_scope_occurrences_returns_none_for_a_non_local_name() {
    let (file_uri, indexer) = indexed_with_live("/D.kt", "class User { fun save() {} }\n");
    // cursor on the class name -- not a local of any enclosing function/lambda
    let result = local_scope_occurrences(&indexer, &file_uri, Position::new(0, 8));
    assert!(
        result.is_none(),
        "a class-level declaration is not a local, got {result:?}"
    );
}

/// PR #229 Copilot review finding: a reference inside a nested lambda
/// that merely CAPTURES an outer local (no shadowing) must still take
/// the local fast path, not fall through to the slower cross-file path.
#[test]
fn local_scope_occurrences_finds_captured_outer_local_from_nested_lambda_reference() {
    let source =
        "fun outer() {\n    val total = 0\n    val block = {\n        print(total)\n    }\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let reference_line = source.lines().nth(3).unwrap();
    let column = reference_line.find("total").unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(3, column))
        .expect("captured-not-shadowed local must still resolve via the fast path");
    assert_eq!(
        locations.len(),
        2,
        "declaration + the one captured reference, got {locations:?}"
    );
}

/// House decoy (Fable's most severe finding): two unrelated sibling
/// `for` loops reusing the same loop-variable name must never be
/// conflated -- the outward climb must stop at the FIRST loop that owns
/// the name, not widen past it into an unrelated sibling.
#[test]
fn local_scope_occurrences_does_not_conflate_two_sibling_for_loops() {
    let source = "fun demo() {\n    for (i in 0..2) {\n        print(i)\n    }\n    for (i in 0..2) {\n        print(i)\n    }\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let first_body_line = source.lines().nth(2).unwrap();
    let column = first_body_line.find("print(i)").unwrap() + "print(".len();
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(2, column as u32))
        .expect("i is a local for-loop variable");
    assert_eq!(
        locations.len(),
        2,
        "declaration + 1 reference from the FIRST loop only, got {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 1 || location.range.start.line == 2),
        "must not include the second loop's own i (lines 4-5), got {locations:?}"
    );
}

/// Same house decoy for `when (val x = ...)` subject bindings.
#[test]
fn local_scope_occurrences_does_not_conflate_two_sibling_when_subjects() {
    let source = "fun demo() {\n    when (val x = compute()) {\n        1 -> print(x)\n        else -> print(x)\n    }\n    when (val x = compute()) {\n        1 -> print(x)\n        else -> print(x)\n    }\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let first_branch_line = source.lines().nth(2).unwrap();
    let column = first_branch_line.find("print(x)").unwrap() + "print(".len();
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(2, column as u32))
        .expect("x is a local when-subject binding");
    assert_eq!(
        locations.len(),
        3,
        "subject declaration + 2 references from the FIRST when only, got {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line <= 3),
        "must not include the second when's own x (lines 5-8), got {locations:?}"
    );
}

/// `catch`'s exception variable is a bare `simple_identifier` child of
/// `catch_block` (no `variable_declaration` wrapper, unlike a `val`/`var`
/// or a `for`/`when` binding) -- the one real `is_declaration_site` gap.
#[test]
fn local_scope_occurrences_scopes_catch_exception_variable_to_its_own_block() {
    let source = "fun demo() {\n    try {\n        risky()\n    } catch (e: Exception) {\n        print(e)\n    }\n    try {\n        risky()\n    } catch (e: Exception) {\n        print(e)\n    }\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let first_catch_body_line = source.lines().nth(4).unwrap();
    let column = first_catch_body_line.find("print(e)").unwrap() + "print(".len();
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(4, column as u32))
        .expect("e is the first catch block's exception variable");
    assert_eq!(
        locations.len(),
        2,
        "exception declaration + 1 reference from the FIRST catch only, got {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 3 || location.range.start.line == 4),
        "must not include the second catch's own e (lines 8-9), got {locations:?}"
    );
}

/// A `val` declared inside an `if` block is scoped to that block, not
/// merged with an unrelated sibling `if` block's same-named local.
#[test]
fn local_scope_occurrences_scopes_a_val_declared_inside_an_if_block() {
    let source = "fun demo() {\n    if (true) {\n        val total = 1\n        print(total)\n    }\n    if (true) {\n        val total = 2\n        print(total)\n    }\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let first_reference_line = source.lines().nth(3).unwrap();
    let column = first_reference_line.find("total").unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(3, column))
        .expect("total is declared inside the first if-block");
    assert_eq!(
        locations.len(),
        2,
        "declaration + 1 reference from the FIRST if-block only, got {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 2 || location.range.start.line == 3),
        "must not include the second if-block's own total (lines 6-7), got {locations:?}"
    );
}

/// Same-scope sequential re-declaration (`val x = ...; val x = ...`):
/// renaming the first declaration must not touch the second's own name,
/// and vice versa. `val x = x as String`'s own RHS `x` belongs to the
/// FIRST `x` -- the second doesn't exist yet while its initializer runs.
#[test]
fn local_scope_occurrences_separates_sequential_redeclarations_first_declaration() {
    let source =
        "fun demo() {\n    val x: Any = \"hello\"\n    val x = x as String\n    print(x)\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let first_declaration_line = source.lines().nth(1).unwrap();
    let column = first_declaration_line.find('x').unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(1, column))
        .expect("x has a valid first declaration");
    assert_eq!(
        locations.len(),
        2,
        "first declaration + the second declaration's own initializer reference, got {locations:?}"
    );
    assert!(
        locations.iter().all(|location| location.range.start.line == 1
            || (location.range.start.line == 2 && location.range.start.character > 10)),
        "must not include the second declaration's own name or the final print(x), got {locations:?}"
    );
}

#[test]
fn local_scope_occurrences_separates_sequential_redeclarations_second_declaration() {
    let source =
        "fun demo() {\n    val x: Any = \"hello\"\n    val x = x as String\n    print(x)\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let second_declaration_line = source.lines().nth(2).unwrap();
    let column = second_declaration_line.find('x').unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(2, column))
        .expect("x has a valid second declaration");
    assert_eq!(
        locations.len(),
        2,
        "second declaration + the final print(x), got {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 2 || location.range.start.line == 3),
        "must not include the first declaration or its own initializer, got {locations:?}"
    );
}

/// A reference textually before any declaration of its name (invalid
/// Kotlin, real during mid-typing) has no valid local declaration to
/// anchor to -- safe fallthrough (`None`), not a partial/wrong rename.
/// No CST trick applies here: nothing is syntactically broken.
#[test]
fn local_scope_occurrences_returns_none_for_a_forward_reference_before_any_declaration() {
    let source = "fun demo() {\n    print(x)\n    val x = 1\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let reference_line = source.lines().nth(1).unwrap();
    let column = reference_line.find('x').unwrap() as u32;
    let result = local_scope_occurrences(&indexer, &file_uri, Position::new(1, column));
    assert!(
        result.is_none(),
        "a reference before any declaration must not resolve, got {result:?}"
    );
}

/// Regression pin: destructuring names (`val (a, b) = pair`, and the same
/// shape inside a `for` loop header) were already correctly recognized
/// by `is_declaration_site` before this fix (each wraps its name in its
/// own `variable_declaration`, same as a plain `val`) -- confirmed
/// unaffected by the scope-boundary widening.
#[test]
fn local_scope_occurrences_handles_a_destructured_declaration() {
    let source = "fun demo() {\n    val (a, b) = pair()\n    print(a)\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let reference_line = source.lines().nth(2).unwrap();
    let column = reference_line.find('a').unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(2, column))
        .expect("a is a destructured local");
    assert_eq!(
        locations.len(),
        2,
        "destructured declaration + 1 reference, got {locations:?}"
    );
}

/// Real-world bug (found renaming `appState` in `NiaApp(appState =
/// appState, ...)`): a named-argument label is textually identical to a
/// local it's often paired with, but names the CALLEE's parameter, not
/// the caller's local. It must never be swept into the local's rename
/// group, and rename must never be triggered directly from it (a
/// different symbol, needing its own resolution, not this fast path).
#[test]
fn local_scope_occurrences_excludes_named_argument_labels_from_a_local_variables_occurrences() {
    let source =
        "fun demo() {\n    val appState = 1\n    NiaApp(\n        appState = appState,\n    )\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let declaration_line = source.lines().nth(1).unwrap();
    let column = declaration_line.find("appState").unwrap() as u32;
    let locations = local_scope_occurrences(&indexer, &file_uri, Position::new(1, column))
        .expect("appState is a local variable");
    assert_eq!(
        locations.len(),
        2,
        "declaration + the value reference only, excluding the named-argument label, got {locations:?}"
    );
    let argument_line = source.lines().nth(3).unwrap();
    let label_column = argument_line.find("appState").unwrap() as u32;
    assert!(
        locations
            .iter()
            .all(|location| !(location.range.start.line == 3
                && location.range.start.character == label_column)),
        "must not include the named-argument label itself, got {locations:?}"
    );
}

#[test]
fn local_scope_occurrences_returns_none_when_cursor_is_on_a_named_argument_label() {
    let source =
        "fun demo() {\n    val appState = 1\n    NiaApp(\n        appState = appState,\n    )\n}\n";
    let (file_uri, indexer) = indexed_with_live("/D.kt", source);
    let argument_line = source.lines().nth(3).unwrap();
    let label_column = argument_line.find("appState").unwrap() as u32;
    let result = local_scope_occurrences(&indexer, &file_uri, Position::new(3, label_column));
    assert!(
        result.is_none(),
        "a named-argument label names the callee's parameter, not the local -- \
         the local fast path must not claim this position, got {result:?}"
    );
}

/// The walk descends only the body that declares the name, but one expression
/// nested that deeply inside it still reaches the same frame count -- and this
/// walker's frames are heavy enough to exhaust 8 MiB at ~10,000 levels.
#[test]
fn local_scope_occurrences_survives_a_pathologically_deep_body() {
    let n = 4_000; // ~8x MAX_CST_DESCENT_DEPTH (512)
    let mut src = String::from("fun run() {\n    val total = 0\n    print(total");
    for _ in 0..n {
        src.push_str("+1");
    }
    src.push_str(")\n}\n");

    let handle = std::thread::Builder::new()
        // Deliberately small: the guard caps the walk at 512 frames whatever
        // the stack size, and n=4,000 overflows 2 MiB unguarded, so this pins
        // the same defect as an 8 MiB thread would at n=10,000 -- without the
        // quadratic parse-depth cost of getting there.
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let (file_uri, indexer) = indexed_with_live("/Deep.kt", &src);
            local_scope_occurrences(&indexer, &file_uri, Position::new(1, 8))
        })
        .unwrap();
    // A stack overflow aborts the process rather than failing this join.
    let result = handle.join().expect("must not overflow the stack");
    assert!(
        result.is_none(),
        "a truncated walk must decline the local fast path, not rename a subset"
    );
}

/// The scope search is iterative, so depth costs it no stack — but it used to
/// cost quadratic *time*, because it asked tree-sitter for each node's parent
/// (an O(depth) operation) once per node. At this size that took minutes.
#[test]
fn local_scope_occurrences_is_not_quadratic_in_nesting_depth() {
    let n = 20_000;
    let mut src = String::from("fun run() {\n    val total = 0\n    print(total");
    for _ in 0..n {
        src.push_str("+1");
    }
    src.push_str(")\n}\n");
    let (file_uri, indexer) = indexed_with_live("/Deep.kt", &src);

    let start = std::time::Instant::now();
    let _ = local_scope_occurrences(&indexer, &file_uri, Position::new(1, 8));
    let elapsed = start.elapsed();
    // Quadratic cost here is ~3.5 minutes; linear is tens of milliseconds. The
    // bound is loose enough that only a return to quadratic can trip it.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "took {elapsed:?} — the parent lookup is quadratic again"
    );
}
