use super::*;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

// `#[tokio::test]` + `.await` rather than `futures::executor::block_on`:
// `verified_references_for`'s rg search runs on `tokio::task::spawn_blocking`,
// which panics ("no reactor running") without a real Tokio runtime driving
// it -- `block_on` alone doesn't provide one. Matches the convention already
// used by every other async feature test in this codebase (e.g.
// `definition_tests.rs`, `references_tests.rs`).
#[tokio::test]
async fn cross_file_rename_refuses_on_override_from_the_interface_side() {
    let source = "open class User { fun save() {} }\n\
                  class DerivedUser : User() { override fun save() {} }\n\
                  fun caller(user: User) { user.save() }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    // cursor on the interface's own declaration
    let column = source.lines().next().unwrap().find("save").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "persist").await;
    assert!(
        result.is_err(),
        "renaming an interface member with a real override must refuse, got {result:?}"
    );
}

#[tokio::test]
async fn cross_file_rename_refuses_on_override_from_the_concrete_side() {
    let source = "open class User { fun save() {} }\n\
                  class DerivedUser : User() { override fun save() {} }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    // cursor on the OVERRIDE's own declaration -- the symmetric direction
    let column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(1, column), "persist").await;
    assert!(
        result.is_err(),
        "renaming FROM the override side must ALSO refuse -- symmetric with \
         the interface side, got {result:?}"
    );
}

#[tokio::test]
async fn cross_file_rename_renames_a_clean_no_override_multi_call_site_member() {
    let source = "class Logger { fun log(message: String) {} }\n\
                  fun a(logger: Logger) { logger.log(\"a\") }\n\
                  fun b(logger: Logger) { logger.log(\"b\") }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let column = source.lines().next().unwrap().find("log").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "write")
        .await
        .expect("no override, no ambiguity -- must succeed")
        .expect("must produce an edit");
    let edits = result
        .changes
        .expect("must have changes")
        .remove(&file_uri)
        .expect("must edit this file");
    assert_eq!(edits.len(), 3, "declaration + 2 call sites, got {edits:?}");
}

/// The confirmed rename-corruption bug: a same-named, wrong-arity call
/// site inside the declaration's own body is a name collision (meant to
/// bind to a differently-shaped function elsewhere), not a genuine
/// reference -- renaming the declaration must not also rewrite it.
#[tokio::test]
async fn rename_does_not_corrupt_a_wrong_arity_self_call() {
    let source = "class CoroutineScope\n\
                  fun collect(scope: CoroutineScope, block: Int) {\n\
                      collect(block)\n\
                  }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let column = source.lines().nth(1).unwrap().find("collect").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(1, column), "gather")
        .await
        .expect("no override, no ambiguity -- must succeed")
        .expect("must produce an edit");
    let edits = result
        .changes
        .expect("must have changes")
        .remove(&file_uri)
        .expect("must edit this file");
    assert!(
        edits.iter().all(|edit| edit.range.start.line != 2),
        "must not rewrite the wrong-arity self-call on line 2, got: {edits:?}"
    );
}

/// Genuine same-arity self-recursion must still be renamed everywhere --
/// the arity filter must not become a blanket "never touch a self-call."
#[tokio::test]
async fn rename_still_renames_a_genuine_same_arity_self_recursive_call() {
    let source = "fun factorial(n: Int): Int {\n\
                      return factorial(n - 1)\n\
                  }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let column = source.lines().next().unwrap().find("factorial").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "fact")
        .await
        .expect("same-arity self-recursion must not be treated as ambiguous")
        .expect("must produce an edit");
    let edits = result
        .changes
        .expect("must have changes")
        .remove(&file_uri)
        .expect("must edit this file");
    assert_eq!(
        edits.len(),
        2,
        "declaration + the recursive call, got: {edits:?}"
    );
}

/// Renaming a non-callable declaration (a class) must be completely
/// unaffected by the arity filter -- `declaration_param_counts` returns
/// `None` for non-callable kinds, so no filtering runs at all.
#[tokio::test]
async fn rename_of_a_class_is_unaffected_by_the_arity_filter() {
    let source = "class Widget\n\
                  fun make(): Widget { return Widget() }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let column = source.lines().next().unwrap().find("Widget").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "Gadget")
        .await
        .expect("class rename must succeed")
        .expect("must produce an edit");
    let edits = result
        .changes
        .expect("must have changes")
        .remove(&file_uri)
        .expect("must edit this file");
    assert_eq!(
        edits.len(),
        3,
        "declaration + return type + constructor call, got: {edits:?}"
    );
}

#[tokio::test]
async fn local_rename_uses_the_cst_fast_path_and_never_refuses() {
    let source = "fun run() {\n    val total = 0\n    print(total)\n}\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let result = rename_impl(&indexer, &file_uri, Position::new(1, 8), "sum")
        .await
        .expect("a local rename must never refuse")
        .expect("must produce an edit");
    let edits = result.changes.expect("must have changes");
    assert_eq!(edits.get(&file_uri).map(Vec::len), Some(2));
}

/// KNOWN ACCEPTED RISK, pinned deliberately (see the spec's Policy gate):
/// a NameScan candidate whose receiver type is genuinely unresolvable
/// (not proven wrong, not proven right) is included in the rename edit
/// set at today's pre-6b trust level. This is NOT a "this is caught"
/// test -- it is a "this is the accepted gap" pin. If a future change
/// narrows this risk, update this test's expectation deliberately; it
/// must not be allowed to silently start passing for a different reason.
#[tokio::test]
async fn unresolvable_receiver_candidate_is_included_not_excluded() {
    // `caller`'s parameter is named `ghost`, not `user` -- `find_var_type`'s
    // scan (`infer_type_in_lines_raw`) is whole-file and position-blind, so
    // reusing the same param name in two functions would make it return
    // whichever declaration comes first in the file regardless of which
    // one the cursor is actually in, corrupting BOTH sites' receiver-type
    // resolution rather than exercising the intended "genuinely
    // unresolvable receiver" case. Distinct names sidestep that
    // (pre-existing, unrelated) collision -- same fix already applied in
    // `references_verify.rs`'s `exact_reference_agreement_does_not_spend_walk_budget`.
    let source = "class User { fun save() {} }\n\
                  fun caller(ghost: Ghost) { ghost.save() }\n\
                  fun real(user: User) { user.save() }\n";
    let file_uri = uri("/D.kt");
    let indexer = std::sync::Arc::new(Indexer::new());
    indexer.index_content(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);

    let column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
    let result = rename_impl(&indexer, &file_uri, Position::new(2, column), "persist")
        .await
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
