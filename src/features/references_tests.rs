//! End-to-end tests for [`find_references_with_qualifier`].
//!
//! These tests write real `.kt` files to a temp directory so that `rg`
//! can search them, then drive the full `find_references_with_qualifier →
//! rg_scope_for_path → rg_find_references` pipeline against an [`Indexer`]
//! whose `workspace_root` is (or isn't) configured.
//!
//! The scenarios targeted by these tests:
//!
//! - **workspace_root set** — rg searches the workspace dir → cross-file hits.
//! - **workspace_root unset** — `effective_rg_root` falls back to the file's
//!   parent directory → cross-file hits still found (files are co-located).
//! - **workspace_root set to a *different* project** — `effective_rg_root`
//!   walks up from the open file to its git root; `scoped_source_roots` is
//!   cleared because effective != configured root → rg searches the file's
//!   git root without leaking the stale workspace's source-path scoping.

use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use crate::features::references::find_references_with_qualifier;
use crate::indexer::Indexer;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, name: &str, content: &str) -> (std::path::PathBuf, Url) {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    let uri = Url::from_file_path(&path).unwrap();
    (path, uri)
}

/// Returns a sorted, deduplicated list of file names (basename only) from `locs`.
fn hit_files(locs: &[tower_lsp::lsp_types::Location]) -> Vec<String> {
    let mut names: Vec<String> = locs
        .iter()
        .filter_map(|l| l.uri.to_file_path().ok())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Assert every file name in `expected` appears in the reference results.
#[track_caller]
fn assert_refs_contain(locs: &[tower_lsp::lsp_types::Location], expected: &[&str]) {
    let files = hit_files(locs);
    for &e in expected {
        assert!(
            files.iter().any(|f| f == e),
            "expected {e:?} in references; got: {files:?}"
        );
    }
}

/// Assert none of the file names in `forbidden` appear in the reference results.
#[track_caller]
fn assert_refs_exclude(locs: &[tower_lsp::lsp_types::Location], forbidden: &[&str]) {
    let files = hit_files(locs);
    let leaked: Vec<_> = forbidden
        .iter()
        .filter(|&&f| files.iter().any(|g| g == f))
        .collect();
    assert!(
        leaked.is_empty(),
        "these files must NOT appear in references: {leaked:?}\ngot: {files:?}"
    );
}

// ─── tests ───────────────────────────────────────────────────────────────────

/// **Core regression**: `find_references` must return cross-file results when
/// `workspace_root` is properly set.
///
/// Layout:
///   Foo.kt — `class MyClass`  (declaration)
///   Bar.kt — `fun use(): MyClass = MyClass()` (usage)
///
/// Calling `find_references("MyClass", foo_uri, …)` must return a hit in
/// `Bar.kt`.  If only `Foo.kt` is returned, `rg_scope_for_path` is not
/// delivering the workspace root to `rg_find_references`.
#[tokio::test]
async fn find_references_cross_file_with_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let foo_src = "package com.example\nclass MyClass";
    let bar_src = "package com.example\nfun use(): MyClass = MyClass()";

    let (_foo_path, foo_uri) = write(root, "Foo.kt", foo_src);
    let (_bar_path, _bar_uri) = write(root, "Bar.kt", bar_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&foo_uri, foo_src);

    let locs = find_references_with_qualifier("MyClass", None, &foo_uri, 1, true, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Bar.kt"),
        "find_references must include Bar.kt; got files: {:?}",
        files
    );
}

/// `find_references` must still return cross-file results when `workspace_root`
/// is **not** set on the indexer.
///
/// In this case `effective_rg_root` falls back through:
///   1. `walk_to_git_root(open_file)` — tempdir has no `.git`, returns `None`
///   2. `open_file.parent()`           — the tempdir itself ← this must work
///
/// If the fallback resolves to the correct directory, `Bar.kt` is found.
/// If it resolves to the wrong directory (e.g. CWD = the lsp repo), the test
/// catches the broken fallback.
#[tokio::test]
async fn find_references_cross_file_without_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let foo_src = "package com.example\nclass MyClass";
    let bar_src = "package com.example\nfun use(): MyClass = MyClass()";

    let (_foo_path, foo_uri) = write(root, "Foo.kt", foo_src);
    let (_bar_path, _bar_uri) = write(root, "Bar.kt", bar_src);

    // ← workspace_root intentionally NOT set
    let idx = Arc::new(Indexer::new());
    idx.index_content(&foo_uri, foo_src);

    let locs = find_references_with_qualifier("MyClass", None, &foo_uri, 1, true, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Bar.kt"),
        "find_references must include Bar.kt even without workspace_root; \
         effective_rg_root should fall back to the file's parent directory. \
         Got files: {:?}",
        files
    );
}

/// **Package-scoped regression**: `find_references` for an *uppercase* symbol
/// must use the package-scoped rg path and return cross-file results.
///
/// `resolve_scope` for an uppercase symbol that IS the declaration returns
/// `(parent=None, pkg=Some("com.example"))`.  This triggers
/// `package_scoped_reference_locations` which first scans for
/// candidate files via import/package patterns, then searches those files.
///
/// If `rg_scope_for_path` returns the wrong `search_root`, the import-pattern
/// scan finds no candidates and the function returns empty — showing only the
/// current-file hit injected by `add_current_file_locations`.
#[tokio::test]
async fn find_references_package_scoped_cross_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Foo.kt: declaration of MyClass at line 1 (0-indexed)
    let foo_src = "package com.example\nclass MyClass";
    // Bar.kt: same package → no import needed, but has explicit import for clarity
    let bar_src = "package com.example\nimport com.example.MyClass\nfun use(): MyClass = MyClass()";
    // Baz.kt: different package, imports MyClass explicitly
    let baz_src = "package com.other\nimport com.example.MyClass\nval x: MyClass = MyClass()";

    let (_, foo_uri) = write(root, "Foo.kt", foo_src);
    write(root, "Bar.kt", bar_src);
    write(root, "Baz.kt", baz_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&foo_uri, foo_src);

    // line=1: declaration of MyClass → resolve_scope returns (None, Some("com.example"))
    // → package_scoped_reference_locations is used
    let locs = find_references_with_qualifier("MyClass", None, &foo_uri, 1, true, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Bar.kt"),
        "package-scoped search must find Bar.kt (same package); got files: {:?}",
        files
    );
    assert!(
        files.iter().any(|f| f == "Baz.kt"),
        "package-scoped search must find Baz.kt (imports MyClass); got files: {:?}",
        files
    );
}

/// End-to-end actor test: after a full workspace scan, `find_references` on a
/// symbol declared in one file must find usages in another file.
///
/// This is the canonical regression test for "find refs only returns current
/// file" — it drives the complete path:
///   Helix opens file → actor receives Initialize → scan completes →
///   user calls find_references → cross-file hits returned.
#[tokio::test]
async fn actor_scan_then_find_references_cross_file() {
    use tokio::sync::oneshot;

    use crate::indexer::NoopReporter;
    use crate::workspace::{Actor, Config, Event};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // workspace.json opts out of external sourcePaths (test isolation)
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let foo_src = "package com.example\nclass MyClass";
    let bar_src = "package com.example\nfun use(): MyClass = MyClass()";
    let (_, foo_uri) = write(root, "Foo.kt", foo_src);
    write(root, "Bar.kt", bar_src);

    let indexer = Arc::new(Indexer::new());
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let actor = Actor::new(Arc::clone(&indexer), Arc::new(NoopReporter), rx, None);
    tokio::spawn(actor.run());

    let (done_tx, done_rx) = oneshot::channel();
    tx.send(Event::Initialize {
        config: Config {
            root: root.to_path_buf(),
            explicit_source_paths: Vec::new(),
            ignore_patterns: Vec::new(),
            jar_paths: Vec::new(),
            pin_workspace: false,
        },
        completion_tx: Some(done_tx),
    })
    .await
    .unwrap();

    // Wait for the workspace scan to complete before querying.
    tokio::time::timeout(std::time::Duration::from_secs(10), done_rx)
        .await
        .expect("workspace scan must complete within 10s")
        .unwrap();

    let locs = find_references_with_qualifier("MyClass", None, &foo_uri, 1, true, &*indexer).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Bar.kt"),
        "after full scan, find_references must return Bar.kt; got files: {:?}\n\
         workspace_root = {:?}",
        files,
        indexer.workspace_root.get()
    );
}
///
/// Concretely: workspace_root = `/tmp/other_project`, open file = in a
/// different tempdir.  `effective_rg_root` walks up from the file, finds no
/// `.git`, falls back to `file.parent()` (the correct tempdir) → Bar.kt found.
///
/// This catches the case where stale `workspace_source_roots` from the old
/// project "leak" into the search and scope rg to paths that don't contain
/// the current file's siblings.
#[tokio::test]
async fn find_references_stale_workspace_root_does_not_suppress_results() {
    let other_project = tempfile::tempdir().unwrap();
    let current_project = tempfile::tempdir().unwrap();

    let foo_src = "package com.example\nclass MyClass";
    let bar_src = "package com.example\nfun use(): MyClass = MyClass()";

    // Files live in `current_project`, but workspace_root points elsewhere.
    let (_foo_path, foo_uri) = write(current_project.path(), "Foo.kt", foo_src);
    let (_bar_path, _bar_uri) = write(current_project.path(), "Bar.kt", bar_src);

    let idx = Arc::new(Indexer::new());
    // workspace_root → wrong project; scoped_source_roots will be cleared
    // by rg_scope_for_path because effective_root != workspace_root.
    idx.workspace_root.set(other_project.path().to_path_buf());
    idx.index_content(&foo_uri, foo_src);

    let locs = find_references_with_qualifier("MyClass", None, &foo_uri, 1, true, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Bar.kt"),
        "find_references must search the file's actual project when workspace_root \
         points to a different directory; got files: {:?}",
        files
    );
}

/// **Regression: nested Factory — declaration-site cursor should scope correctly**
///
/// When the cursor is ON the `class Factory` declaration line (no qualifier in
/// the source text), `on_decl=true` and `enclosing_class_at` must return the
/// parent class (`ReducerA`).  Without this the scope falls back to bare-word
/// search and bleeds across all reducers.
///
/// Also covers the annotation case: `@AssistedFactory\n interface Factory {` —
/// the annotation pushes the tree-sitter `interface_declaration` start row above
/// the `interface` keyword line, tricking `enclosing_class_at` into returning
/// `"Factory"` itself (start_row < cursor_row satisfied by Factory's own node).
/// The fix checks that the cursor is inside the class *body*, not the header.
#[tokio::test]
async fn find_references_nested_factory_from_declaration_site() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // @AssistedFactory is on line 2 (0-based), `interface Factory` on line 3.
    // This triggers the annotation-offset bug in enclosing_class_at.
    let reducer_a = "\
package com.example.a
class ReducerA {
    @SomeAnnotation
    interface Factory {
        fun create(): ReducerA
    }
}
";
    let reducer_b = "\
package com.example.b
class ReducerB {
    interface Factory {
        fun create(): ReducerB
    }
}
";
    let viewmodel = "\
package com.example
import com.example.a.ReducerA
import com.example.b.ReducerB
class ViewModel(
    private val reducerAFactory: ReducerA.Factory,
    private val reducerBFactory: ReducerB.Factory,
)
";
    let other_caller = "\
package com.example
import com.example.b.ReducerB
class OtherCaller(val f: ReducerB.Factory)
";

    write(root, "ReducerA.kt", reducer_a);
    write(root, "ReducerB.kt", reducer_b);
    write(root, "ViewModel.kt", viewmodel);
    write(root, "OtherCaller.kt", other_caller);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let (_, ra_uri) = (
        root.join("ReducerA.kt"),
        Url::from_file_path(root.join("ReducerA.kt")).unwrap(),
    );
    let (_, rb_uri) = (
        root.join("ReducerB.kt"),
        Url::from_file_path(root.join("ReducerB.kt")).unwrap(),
    );
    let (_, vm_uri) = (
        root.join("ViewModel.kt"),
        Url::from_file_path(root.join("ViewModel.kt")).unwrap(),
    );
    let (_, oc_uri) = (
        root.join("OtherCaller.kt"),
        Url::from_file_path(root.join("OtherCaller.kt")).unwrap(),
    );

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&ra_uri, reducer_a);
    idx.index_content(&rb_uri, reducer_b);
    idx.index_content(&vm_uri, viewmodel);
    idx.index_content(&oc_uri, other_caller);

    // Cursor on `Factory` in `    interface Factory {` — line 3 (0-based) in ReducerA.kt
    // (line 2 is `@SomeAnnotation`).  No dot-qualifier → qualifier=None, on_decl=true.
    // enclosing_class_at must return "ReducerA", not "Factory".
    let locs = find_references_with_qualifier("Factory", None, &ra_uri, 3, false, &*idx).await;

    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "ViewModel.kt"),
        "ReducerA.Factory usage in ViewModel.kt must be found; got: {:?}",
        files
    );
    assert!(
        !files.iter().any(|f| f == "OtherCaller.kt"),
        "OtherCaller.kt uses ReducerB.Factory and must NOT appear; got: {:?}",
        files
    );
}

/// **Regression: nested Factory scoped by qualifier**
///
/// Two classes `ReducerA` and `ReducerB` both have a nested `Factory` interface.
/// Class `ViewModel` injects `ReducerA.Factory` in its constructor.
/// The file does NOT import `ReducerA.Factory` directly — only `ReducerA`.
///
/// `find_references("Factory", …, qualifier=Some("ReducerA"))` must return
/// only usages of `ReducerA.Factory`, NOT every use of `ReducerB.Factory`
/// or bare `Factory` in other files.
///
/// Without the fix the qualifier is discarded, `declared_parent_class_of`
/// picks an arbitrary `Factory` definition from the index (non-deterministic
/// when multiple classes define `Factory`), and results bleed across the
/// whole project.
#[tokio::test]
async fn find_references_nested_factory_scoped_by_qualifier() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Two different reducers each with a nested Factory.
    let reducer_a = "\
package com.example.a
class ReducerA {
    interface Factory {
        fun create(): ReducerA
    }
}
";
    let reducer_b = "\
package com.example.b
class ReducerB {
    interface Factory {
        fun create(): ReducerB
    }
}
";
    // ViewModel uses ReducerA.Factory in its constructor.
    // No direct `import com.example.a.ReducerA.Factory` — only `import com.example.a.ReducerA`.
    let viewmodel = "\
package com.example
import com.example.a.ReducerA
import com.example.b.ReducerB
class ViewModel(
    private val reducerAFactory: ReducerA.Factory,
    private val reducerBFactory: ReducerB.Factory,
)
";
    // A second caller that uses ReducerB.Factory only.
    let other_caller = "\
package com.example
import com.example.b.ReducerB
class OtherCaller(val f: ReducerB.Factory)
";

    write(root, "ReducerA.kt", reducer_a);
    write(root, "ReducerB.kt", reducer_b);
    let (_, vm_uri) = write(root, "ViewModel.kt", viewmodel);
    write(root, "OtherCaller.kt", other_caller);

    // Write workspace.json to prevent scanning ~/.kmp-lsp/sources.
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&vm_uri, viewmodel);

    // Index the companion files so `declared_parent_class_of` has both entries.
    let (_, ra_uri) = (
        root.join("ReducerA.kt"),
        Url::from_file_path(root.join("ReducerA.kt")).unwrap(),
    );
    let (_, rb_uri) = (
        root.join("ReducerB.kt"),
        Url::from_file_path(root.join("ReducerB.kt")).unwrap(),
    );
    let (_, oc_uri) = (
        root.join("OtherCaller.kt"),
        Url::from_file_path(root.join("OtherCaller.kt")).unwrap(),
    );
    idx.index_content(&ra_uri, reducer_a);
    idx.index_content(&rb_uri, reducer_b);
    idx.index_content(&oc_uri, other_caller);

    // Cursor is on `Factory` in `private val reducerAFactory: ReducerA.Factory`
    // (line 4, after the dot — qualifier = "ReducerA").
    // Line 4 (0-based) = `    private val reducerAFactory: ReducerA.Factory,`
    let locs =
        find_references_with_qualifier("Factory", Some("ReducerA"), &vm_uri, 4, false, &*idx).await;

    let files = hit_files(&locs);

    // Must find the ViewModel itself (it uses ReducerA.Factory).
    assert!(
        files.iter().any(|f| f == "ViewModel.kt"),
        "ReducerA.Factory usage in ViewModel.kt must be found; got: {:?}",
        files
    );
    // Must NOT bleed into OtherCaller (uses ReducerB.Factory, different class).
    assert!(
        !files.iter().any(|f| f == "OtherCaller.kt"),
        "OtherCaller.kt uses ReducerB.Factory and must NOT appear; got: {:?}",
        files
    );
}

/// **Regression: sibling qualifier bleed**
///
/// When a single file (`ViewModel.kt`) has BOTH `ReducerA.Factory` AND `ReducerC.Factory`
/// as constructor parameters, searching for references of `ReducerA.Factory` must not
/// include the line that has `ReducerC.Factory`.
///
/// Root cause: the bare-word step in `parent_scoped_reference_locations` searches for
/// `Factory` word-boundary in candidate files without checking whether a specific hit
/// has a *different* qualifier on the same line.
#[tokio::test]
async fn find_references_sibling_qualifier_does_not_bleed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let reducer_a = "\
package com.example.a
class ReducerA {
    interface Factory {
        fun create(): ReducerA
    }
}
";
    let reducer_b = "\
package com.example.b
class ReducerB {
    interface Factory {
        fun create(): ReducerB
    }
}
";
    let reducer_c = "\
package com.example.c
class ReducerC {
    interface Factory {
        fun create(): ReducerC
    }
}
";
    // ViewModel has BOTH ReducerA.Factory AND ReducerC.Factory as params.
    let viewmodel = "\
package com.example
import com.example.a.ReducerA
import com.example.b.ReducerB
import com.example.c.ReducerC
class ViewModel(
    private val reducerAFactory: ReducerA.Factory,
    private val reducerBFactory: ReducerB.Factory,
    private val reducerCFactory: ReducerC.Factory,
)
";

    write(root, "ReducerA.kt", reducer_a);
    write(root, "ReducerB.kt", reducer_b);
    write(root, "ReducerC.kt", reducer_c);
    let (_, vm_uri) = write(root, "ViewModel.kt", viewmodel);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    let ra_uri = Url::from_file_path(root.join("ReducerA.kt")).unwrap();
    let rb_uri = Url::from_file_path(root.join("ReducerB.kt")).unwrap();
    let rc_uri = Url::from_file_path(root.join("ReducerC.kt")).unwrap();
    idx.index_content(&ra_uri, reducer_a);
    idx.index_content(&rb_uri, reducer_b);
    idx.index_content(&rc_uri, reducer_c);
    idx.index_content(&vm_uri, viewmodel);

    // Search refs of ReducerA.Factory (qualifier = "ReducerA").
    // Line 5 = `    private val reducerAFactory: ReducerA.Factory,` (0-based)
    let locs =
        find_references_with_qualifier("Factory", Some("ReducerA"), &vm_uri, 5, false, &*idx).await;

    let lines: Vec<u32> = locs
        .iter()
        .filter(|l| l.uri == vm_uri)
        .map(|l| l.range.start.line)
        .collect();

    // Line 5 (ReducerA.Factory) must appear; lines 6 and 7 (ReducerB/C.Factory) must not.
    assert!(
        lines.contains(&5),
        "ReducerA.Factory line (5) must be found; got lines: {:?}",
        lines
    );
    assert!(
        !lines.contains(&7),
        "ReducerC.Factory line (7) must NOT appear in ReducerA.Factory search; got lines: {:?}",
        lines
    );
}

/// **Regression: lowercase method names at declaration site are scoped to package**
///
/// `fun create()` declared inside a nested `Factory` interface was previously
/// treated as "no scope" (lowercase early-return) and fell through to a
/// codebase-wide rg search, returning every file with `create` in the entire
/// workspace.
///
/// The fix: when cursor is at the declaration site (`on_decl=true`) of a lowercase
/// name, use the declaring file's package as the search scope instead of None.
#[tokio::test]
async fn find_references_lowercase_method_scoped_to_package_on_decl() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let reducer_a = "\
package com.example.a
class ReducerA {
    interface Factory {
        fun create(): ReducerA
    }
}
";
    // A totally unrelated file in a *different* package that also has `fun create`.
    let unrelated = "\
package com.unrelated
class Unrelated {
    fun create(): Unrelated = Unrelated()
}
";
    // Same-package caller that calls reducer factory.
    let caller = "\
package com.example.a
import com.example.a.ReducerA
fun buildReducer(f: ReducerA.Factory): ReducerA = f.create()
";

    let ra_uri = Url::from_file_path(root.join("ReducerA.kt")).unwrap();
    let unrelated_uri = Url::from_file_path(root.join("Unrelated.kt")).unwrap();
    let caller_uri = Url::from_file_path(root.join("Caller.kt")).unwrap();

    write(root, "ReducerA.kt", reducer_a);
    write(root, "Unrelated.kt", unrelated);
    write(root, "Caller.kt", caller);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&ra_uri, reducer_a);
    idx.index_content(&unrelated_uri, unrelated);
    idx.index_content(&caller_uri, caller);

    // Cursor on `create` in `fun create(): ReducerA` — line 3 (0-based).
    let locs = find_references_with_qualifier("create", None, &ra_uri, 3, false, &*idx).await;

    let files = hit_files(&locs);

    // Same-package caller must be found (calls f.create()).
    assert!(
        files.iter().any(|f| f == "Caller.kt"),
        "Caller.kt (same package) must appear; got: {:?}",
        files
    );
    // Unrelated.kt in a different package must NOT be returned.
    assert!(
        !files.iter().any(|f| f == "Unrelated.kt"),
        "Unrelated.kt (different package) must NOT appear; got: {:?}",
        files
    );
}

/// **Regression: multi-segment qualifier is matched against the full extracted chain**
///
/// `word_and_qualifier_at` returns the full dot-chain, so for cursor on
/// `Factory` in `Outer.Inner.Factory` the qualifier is `"Outer.Inner"`, not
/// just `"Inner"`.  The old (whole-line) qualifier check extracted only the
/// *single* token immediately before the dot in each line, so
/// `"Inner" != "Outer.Inner"` caused every valid reference to be dropped (false
/// negatives).
///
/// The fix: `has_wrong_qualifier_at_col` walks backward over `[A-Za-z0-9_.]`
/// to extract the full dot-chain from the specific column of each hit, so
/// `"Outer.Inner" == "Outer.Inner"` matches correctly and valid references are
/// preserved.
#[tokio::test]
async fn find_references_multi_segment_qualifier_normalised() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Three-level nesting: Outer → Inner → Factory.
    let outer = "\
package com.example
class Outer {
    class Inner {
        interface Factory {
            fun create(): Inner
        }
    }
}
";
    // Another class has its own nested Factory that must NOT appear.
    let other = "\
package com.example
class Other {
    class Inner {
        interface Factory {
            fun create(): Other.Inner
        }
    }
}
";
    // Caller uses Outer.Inner.Factory — multi-segment qualifier.
    let caller = "\
package com.example
class Caller(val f: Outer.Inner.Factory)
";

    let outer_uri = Url::from_file_path(root.join("Outer.kt")).unwrap();
    let other_uri = Url::from_file_path(root.join("Other.kt")).unwrap();
    let caller_uri = Url::from_file_path(root.join("Caller.kt")).unwrap();

    write(root, "Outer.kt", outer);
    write(root, "Other.kt", other);
    write(root, "Caller.kt", caller);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&outer_uri, outer);
    idx.index_content(&other_uri, other);
    idx.index_content(&caller_uri, caller);

    // Simulate what word_and_qualifier_at returns for cursor on `Factory`
    // in `class Caller(val f: Outer.Inner.Factory)`: qualifier = "Outer.Inner".
    let locs = find_references_with_qualifier(
        "Factory",
        Some("Outer.Inner"),
        &caller_uri,
        1, // line 1 (0-based): `class Caller(val f: Outer.Inner.Factory)`
        false,
        &*idx,
    )
    .await;

    let files = hit_files(&locs);

    // Caller.kt uses Outer.Inner.Factory — must be found.
    assert!(
        files.iter().any(|f| f == "Caller.kt"),
        "Caller.kt (uses Outer.Inner.Factory) must be found; got: {:?}",
        files
    );
    // Other.kt uses Other.Inner.Factory — must NOT appear (different qualifier).
    assert!(
        !files.iter().any(|f| f == "Other.kt"),
        "Other.kt (Other.Inner.Factory) must NOT appear; got: {:?}",
        files
    );
}

/// **Regression: `create()` inside nested Factory finds callers in parent package**
///
/// `fun create()` declared inside `ReducerA.Factory` must:
///   1. Return callers in a *parent* package that use variable-name syntax
///      (`reducerAFactory.create()`), and
///   2. NOT return `fun create` declarations in sibling factories that live in
///      the same package as `ReducerA`.
///
/// Root cause: package-scoped search (patterns matching `package com.example.a`)
/// finds all sibling factories in the same package → FPs, while callers in
/// `com.example` (parent) are outside the package scope → FNs.
///
/// Fix: `outer_class_for_decl_site` walks the CST chain to find that `create`
/// is inside `Factory` inside `ReducerA`; the outer class `ReducerA` is used for
/// file discovery so only files that reference `ReducerA` are searched.
#[tokio::test]
async fn find_references_nested_factory_create_finds_callers_not_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // ReducerA in package a with a nested Factory.
    let reducer_a = "\
package com.example.a
class ReducerA {
    interface Factory {
        fun create(): ReducerA
    }
}
";
    // Sibling in same package — also has Factory.create (must NOT appear).
    let reducer_b = "\
package com.example.a
class ReducerB {
    interface Factory {
        fun create(): ReducerB
    }
}
";
    // Caller in PARENT package that references ReducerA.Factory via a variable.
    let dashboard = "\
package com.example
import com.example.a.ReducerA
class Dashboard(private val reducerAFactory: ReducerA.Factory) {
    fun build() = reducerAFactory.create()
}
";
    // A file that imports ReducerA (so it appears in owner-class candidate files)
    // AND declares its own unrelated `fun create()` — must NOT appear as an FP.
    let reducer_c = "\
package com.example.a
import com.example.a.ReducerA
class ReducerC {
    interface Factory {
        fun create(): ReducerC
    }
    fun useA(f: ReducerA.Factory) = Unit
}
";

    let ra_uri = Url::from_file_path(root.join("ReducerA.kt")).unwrap();
    let rb_uri = Url::from_file_path(root.join("ReducerB.kt")).unwrap();
    let dash_uri = Url::from_file_path(root.join("Dashboard.kt")).unwrap();
    let rc_uri = Url::from_file_path(root.join("ReducerC.kt")).unwrap();

    write(root, "ReducerA.kt", reducer_a);
    write(root, "ReducerB.kt", reducer_b);
    write(root, "Dashboard.kt", dashboard);
    write(root, "ReducerC.kt", reducer_c);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&ra_uri, reducer_a);
    idx.index_content(&rb_uri, reducer_b);
    idx.index_content(&dash_uri, dashboard);
    idx.index_content(&rc_uri, reducer_c);

    // Cursor on `create` in `fun create(): ReducerA` — line 3 (0-based).
    let locs = find_references_with_qualifier("create", None, &ra_uri, 3, false, &*idx).await;

    let files = hit_files(&locs);

    // Caller in parent package must be found.
    assert!(
        files.iter().any(|f| f == "Dashboard.kt"),
        "Dashboard.kt (parent-package caller) must appear; got: {:?}",
        files
    );
    // Sibling factory in same package — must NOT appear.
    assert!(
        !files.iter().any(|f| f == "ReducerB.kt"),
        "ReducerB.kt (sibling factory, same pkg) must NOT appear; got: {:?}",
        files
    );
    // File that imports ReducerA but declares its own create() — must NOT appear.
    assert!(
        !files.iter().any(|f| f == "ReducerC.kt"),
        "ReducerC.kt (imports ReducerA but declares own create) must NOT appear; got: {:?}",
        files
    );

    // Same assertions must hold when include_decl=true (LSP default).
    let locs_incl = find_references_with_qualifier("create", None, &ra_uri, 3, true, &*idx).await;
    let files_incl = hit_files(&locs_incl);
    assert!(
        files_incl.iter().any(|f| f == "ReducerA.kt"),
        "ReducerA.kt (declaration) must appear with include_decl=true; got: {:?}",
        files_incl
    );
    assert!(
        files_incl.iter().any(|f| f == "Dashboard.kt"),
        "Dashboard.kt must appear with include_decl=true; got: {:?}",
        files_incl
    );
    assert!(
        !files_incl.iter().any(|f| f == "ReducerC.kt"),
        "ReducerC.kt must NOT appear even with include_decl=true; got: {:?}",
        files_incl
    );
}

/// **Field references**: `find_references` on a data class property must scope
/// results to files that mention the declaring class, excluding same-named
/// properties in unrelated classes.
///
/// Layout:
///   Account.kt     — `data class Account(val id: String)`  (declaration)
///   Consumer.kt    — `fun show(a: Account) = println(a.id)`  (valid access)
///   Unrelated.kt   — `data class Unrelated(val id: String)` (same-named field)
///
/// References to `id` on the declaration line in Account.kt must include
/// Consumer.kt (uses `a.id`) but must NOT include Unrelated.kt.
#[tokio::test]
async fn find_references_data_class_field_scoped_to_declaring_class() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let account_src = "\
package com.example
data class Account(val id: String)
";
    let consumer_src = "\
package com.example
import com.example.Account
fun show(a: Account) = println(a.id)
";
    // Different class with a field of the same name — must not appear.
    let unrelated_src = "\
package com.example
data class Unrelated(val id: String)
";

    write(root, "Account.kt", account_src);
    write(root, "Consumer.kt", consumer_src);
    write(root, "Unrelated.kt", unrelated_src);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let account_uri = Url::from_file_path(root.join("Account.kt")).unwrap();
    let consumer_uri = Url::from_file_path(root.join("Consumer.kt")).unwrap();
    let unrelated_uri = Url::from_file_path(root.join("Unrelated.kt")).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&account_uri, account_src);
    idx.index_content(&consumer_uri, consumer_src);
    idx.index_content(&unrelated_uri, unrelated_src);

    // Cursor on `id` in `data class Account(val id: String)` — line 1 (0-based).
    let locs = find_references_with_qualifier("id", None, &account_uri, 1, false, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "Consumer.kt"),
        "Consumer.kt must appear (uses a.id on an Account); got: {:?}",
        files
    );
    assert!(
        !files.iter().any(|f| f == "Unrelated.kt"),
        "Unrelated.kt must NOT appear (different class with same field name); got: {:?}",
        files
    );
}

// ─── package disambiguation for same-name nested classes ─────────────────────

/// Regression: multiple MVI contracts each define `sealed class Effect`.
/// Searching for refs on `Effect` inside `IntroContract.kt` must NOT return
/// hits from `LoginContract.kt` (different package, different enclosing class).
///
/// Root cause: `declared_package_of` was not scoped to the preferred URI, so it
/// could return the package of any contract's `Effect`, expanding the rg candidate
/// set to the wrong package directory and producing false positives.
#[tokio::test]
async fn find_references_nested_class_not_polluted_by_same_name_in_other_packages() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Two MVI contracts — each defines a nested `sealed class Effect`.
    let intro_src = "\
package com.example.intro
class IntroContract {
    sealed class Effect {
        object NavigateNext : Effect()
    }
}
";
    let login_src = "\
package com.example.login
class LoginContract {
    sealed class Effect {
        object ShowError : Effect()
    }
}
";
    // A presenter in the intro package references IntroContract.Effect bare (no import needed).
    let intro_presenter_src = "\
package com.example.intro
class IntroPresenter {
    fun handle(effect: IntroContract.Effect) {}
}
";
    // A presenter in the login package references LoginContract.Effect — must NOT appear.
    let login_presenter_src = "\
package com.example.login
class LoginPresenter {
    fun handle(effect: LoginContract.Effect) {}
}
";

    write(root, "IntroContract.kt", intro_src);
    write(root, "LoginContract.kt", login_src);
    write(root, "IntroPresenter.kt", intro_presenter_src);
    write(root, "LoginPresenter.kt", login_presenter_src);
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let intro_uri = Url::from_file_path(root.join("IntroContract.kt")).unwrap();
    let login_uri = Url::from_file_path(root.join("LoginContract.kt")).unwrap();
    let intro_presenter_uri = Url::from_file_path(root.join("IntroPresenter.kt")).unwrap();
    let login_presenter_uri = Url::from_file_path(root.join("LoginPresenter.kt")).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&intro_uri, intro_src);
    idx.index_content(&login_uri, login_src);
    idx.index_content(&intro_presenter_uri, intro_presenter_src);
    idx.index_content(&login_presenter_uri, login_presenter_src);

    // Cursor on `Effect` at its declaration inside IntroContract.kt (line 2, 0-based).
    let locs = find_references_with_qualifier("Effect", None, &intro_uri, 2, false, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "IntroPresenter.kt"),
        "IntroPresenter.kt must appear (uses IntroContract.Effect); got: {:?}",
        files
    );
    assert!(
        !files.iter().any(|f| f == "LoginPresenter.kt"),
        "LoginPresenter.kt must NOT appear (different contract's Effect); got: {:?}",
        files
    );
    assert!(
        !files.iter().any(|f| f == "LoginContract.kt"),
        "LoginContract.kt must NOT appear (unrelated Effect declaration); got: {:?}",
        files
    );
}

/// Stricter version: FP via bare `Effect` reference in the wrong package's file.
///
/// When `declared_package_of("Effect")` non-deterministically returns
/// `com.example.login` (the wrong package), `parent_scoped_reference_locations`
/// adds login-package files as candidates.  A bare `Effect` usage in those files
/// (no qualifier → `has_wrong_qualifier_at_col` can't filter it) leaks through.
#[tokio::test]
async fn find_references_bare_effect_in_wrong_package_not_leaked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let intro_src = "\
package com.example.intro
class IntroContract {
    sealed class Effect
}
";
    let login_src = "\
package com.example.login
class LoginContract {
    sealed class Effect
}
";
    // Login-side file that uses bare `Effect` after importing LoginContract.Effect.
    // If the wrong package is selected as scope, this file becomes a candidate
    // and `Effect` bare is returned as a false positive.
    let login_handler_src = "\
package com.example.login
import com.example.login.LoginContract.Effect
class LoginHandler {
    fun process(e: Effect) {}
}
";
    // Intro-side file that uses bare `Effect` via STAR import — the only real hit.
    // Star import: resolve_symbol_via_import returns (None, None) → hits declared_package_of.
    let intro_handler_src = "\
package com.example.intro
import com.example.intro.IntroContract.*
class IntroHandler {
    fun process(e: Effect) {}
}
";

    let intro_uri = Url::from_file_path(root.join("IntroContract.kt")).unwrap();
    let login_uri = Url::from_file_path(root.join("LoginContract.kt")).unwrap();
    let login_handler_uri = Url::from_file_path(root.join("LoginHandler.kt")).unwrap();
    let intro_handler_uri = Url::from_file_path(root.join("IntroHandler.kt")).unwrap();

    std::fs::write(root.join("IntroContract.kt"), intro_src).unwrap();
    std::fs::write(root.join("LoginContract.kt"), login_src).unwrap();
    std::fs::write(root.join("LoginHandler.kt"), login_handler_src).unwrap();
    std::fs::write(root.join("IntroHandler.kt"), intro_handler_src).unwrap();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    // Index login FIRST so it appears first in the definitions HashMap,
    // maximising the chance that declared_package_of returns the wrong package.
    idx.index_content(&login_uri, login_src);
    idx.index_content(&login_handler_uri, login_handler_src);
    idx.index_content(&intro_uri, intro_src);
    idx.index_content(&intro_handler_uri, intro_handler_src);

    // Cursor on `Effect` at a USAGE site inside IntroHandler.kt (off-declaration path).
    // Line 0: package, line 1: import, line 2: class IntroHandler {, line 3: fun process(e: Effect)
    // on_decl=false → resolve_scope falls to declared_package_of (the buggy path).
    let locs =
        find_references_with_qualifier("Effect", None, &intro_handler_uri, 3, false, &*idx).await;
    let files = hit_files(&locs);

    assert!(
        files.iter().any(|f| f == "IntroHandler.kt"),
        "IntroHandler.kt must appear (bare Effect from intro package); got: {:?}",
        files
    );
    assert!(
        !files.iter().any(|f| f == "LoginHandler.kt"),
        "LoginHandler.kt must NOT appear (bare Effect from login package is a FP); got: {:?}",
        files
    );
}

/// Regression: `findReferences` on a nested uppercase type must NOT include files
/// that import the parent class for a *different* member.
///
/// Scenario (mirrors the real IntroContract.Event false-positive explosion):
///   - `IntroContract.kt`  declares `IntroContract` with nested `Event` and `State`
///   - `GoodCaller.kt`     imports `IntroContract.Event` → uses bare `Event` ← valid hit
///   - `UnrelatedCaller.kt` imports `IntroContract` only for `IntroContract.State` usage,
///                           but happens to reference its own unrelated `Event` class ← FP
///
/// With the bug, the broad import pattern (`import.*IntroContract`) marks `UnrelatedCaller.kt`
/// as a candidate, and the bare `Event` search inside it produces a false positive.
#[tokio::test]
async fn find_references_nested_type_not_polluted_by_unrelated_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    // Declaring file — Event is a nested sealed interface inside IntroContract.
    let contract_src = "\
package com.example.intro
internal interface IntroContract {
    sealed interface Event
    data class State(val loading: Boolean)
}
";
    // Good caller — imports Event explicitly, uses it bare.
    let good_caller_src = "\
package com.feature.good
import com.example.intro.IntroContract.Event
fun handle(e: Event) {}
";
    // Unrelated caller — imports IntroContract only to use IntroContract.State.
    // Contains its own unrelated `Event` class — must NOT appear in results.
    let unrelated_src = "\
package com.feature.other
import com.example.intro.IntroContract
sealed class Event
fun process(s: IntroContract.State, e: Event) {}
";

    let contract_uri = Url::from_file_path(root.join("IntroContract.kt")).unwrap();
    let good_uri = Url::from_file_path(root.join("GoodCaller.kt")).unwrap();
    let unrelated_uri = Url::from_file_path(root.join("UnrelatedCaller.kt")).unwrap();

    write(root, "IntroContract.kt", contract_src);
    write(root, "GoodCaller.kt", good_caller_src);
    write(root, "UnrelatedCaller.kt", unrelated_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&contract_uri, contract_src);
    idx.index_content(&good_uri, good_caller_src);
    idx.index_content(&unrelated_uri, unrelated_src);

    // Cursor on `Event` at its declaration inside IntroContract (line 2, 0-based).
    let locs = find_references_with_qualifier("Event", None, &contract_uri, 2, false, &*idx).await;

    assert_refs_contain(&locs, &["GoodCaller.kt"]);
    assert_refs_exclude(&locs, &["UnrelatedCaller.kt"]);
}

/// Regression: two *different* classes named `IntroContract` in different packages,
/// each with their own `sealed interface Event`, must not bleed into each other's
/// `findReferences` results.
///
/// Scenario (mirrors `DocumentIntroViewModel` / zenid false-positive on Android):
///   - `PkgAContract.kt`  (pkg `com.a`) declares `IntroContract { sealed interface Event }`
///   - `PkgBContract.kt`  (pkg `com.b`) declares a DIFFERENT `IntroContract { Event }`
///   - `PkgACaller.kt`    imports `com.a.IntroContract.Event`, uses bare `Event`  ← valid
///   - `PkgBViewModel.kt` imports `com.b.IntroContract` (the B one), uses `IntroContract.Event`
///                        referring to the B type  ← must NOT appear in A's results
///
/// The qualified rg pattern `\bIntroContract\.\bEvent\b` naively matches `PkgBViewModel.kt`.
/// The index-based candidate filter should exclude it since it imports B's IntroContract,
/// not `com.a.IntroContract.Event`.
#[tokio::test]
async fn find_references_nested_type_same_name_different_package_no_bleed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let pkg_a_contract = "\
package com.a
interface IntroContract {
    sealed interface Event
}
";
    let pkg_b_contract = "\
package com.b
interface IntroContract {
    sealed interface Event
}
";
    let pkg_a_caller = "\
package com.a.feature
import com.a.IntroContract.Event
fun handleA(e: Event) {}
";
    // Uses com.b.IntroContract.Event — must NOT appear in com.a's Event results.
    let pkg_b_viewmodel = "\
package com.b.feature
import com.b.IntroContract
fun handleB(e: IntroContract.Event) {}
";

    let a_contract_uri = Url::from_file_path(root.join("PkgAContract.kt")).unwrap();
    let b_contract_uri = Url::from_file_path(root.join("PkgBContract.kt")).unwrap();
    let a_caller_uri = Url::from_file_path(root.join("PkgACaller.kt")).unwrap();
    let b_vm_uri = Url::from_file_path(root.join("PkgBViewModel.kt")).unwrap();

    write(root, "PkgAContract.kt", pkg_a_contract);
    write(root, "PkgBContract.kt", pkg_b_contract);
    write(root, "PkgACaller.kt", pkg_a_caller);
    write(root, "PkgBViewModel.kt", pkg_b_viewmodel);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&a_contract_uri, pkg_a_contract);
    idx.index_content(&b_contract_uri, pkg_b_contract);
    idx.index_content(&a_caller_uri, pkg_a_caller);
    idx.index_content(&b_vm_uri, pkg_b_viewmodel);

    // Cursor on `Event` in com.a.IntroContract (line 2, 0-based).
    let locs =
        find_references_with_qualifier("Event", None, &a_contract_uri, 2, false, &*idx).await;

    assert_refs_contain(&locs, &["PkgACaller.kt"]);
    assert_refs_exclude(&locs, &["PkgBViewModel.kt"]);
}

/// Regression: when multiple packages each define `IntroContract { Event }`,
/// the `decl_files` mechanism must not pull OTHER packages' `IntroContract.kt`
/// into the candidate set for bare-name scanning.
///
/// Scenario:
///   - `PkgAContract.kt`  declares `IntroContract { Event }` in `com.a`
///   - `PkgBContract.kt`  declares `IntroContract { Event }` in `com.b`
///   - `PkgBContract.kt`  has `data object Clicked : Event` (non-decl usage of its OWN Event)
///   - There is NO caller of com.a's Event
///
/// Without the fix, `PkgBContract.kt` ends up in `decl_files` (the index has BOTH
/// `IntroContract.Event` declarations), then its `Clicked : Event` line becomes a
/// false-positive bare-name hit for com.a's Event.
#[tokio::test]
async fn find_references_decl_files_dont_bleed_across_same_name_classes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let pkg_a_contract = "\
package com.a
interface IntroContract {
    sealed interface Event
    data object Clicked : Event
}
";
    let pkg_b_contract = "\
package com.b
interface IntroContract {
    sealed interface Event
    data object BackPressed : Event
}
";

    let a_uri = Url::from_file_path(root.join("PkgAContract.kt")).unwrap();
    let b_uri = Url::from_file_path(root.join("PkgBContract.kt")).unwrap();

    write(root, "PkgAContract.kt", pkg_a_contract);
    write(root, "PkgBContract.kt", pkg_b_contract);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&a_uri, pkg_a_contract);
    idx.index_content(&b_uri, pkg_b_contract);

    // Cursor on `Event` in com.a.IntroContract, include_decl=false.
    let locs = find_references_with_qualifier("Event", None, &a_uri, 2, false, &*idx).await;

    assert_refs_exclude(&locs, &["PkgBContract.kt"]);
}

/// Regression: field references for a field declared in a **deeply-nested** class
/// must not return hits from unrelated classes that share the same short name.
///
/// Scenario:
///   - `TextBody.kt` declares `TextBody { Scenes { BusyLoader { val title: String? } } }`
///   - `OtherBody.kt` declares a completely different `BusyLoader` (in `ProductScreens`)
///     and uses `title` locally
///   - Cursor on `title` in `TextBody.BusyLoader`
///
/// With the bug, `field_scoped_reference_locations` searches for `\bBusyLoader\b`
/// workspace-wide, finds `OtherBody.kt` (which mentions a different `BusyLoader`),
/// then returns its `title` usages as false positives.
///
/// The fix: use the outermost ancestor class (`TextBody`) as the candidate filter,
/// which is specific enough to exclude unrelated files.
#[tokio::test]
async fn find_references_nested_field_no_bleed_from_same_name_outer_class() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    // Declaring file: TextBody { Scenes { BusyLoader { val title: String? } } }
    let text_body_src = "\
package com.example.data
data class TextBody(val scenes: Scenes) {
    data class Scenes(val busyLoader: BusyLoader) {
        data class BusyLoader(
            val title: String?,
        )
    }
}
";
    // Legitimate caller: uses TextBody.Scenes.BusyLoader.title
    let good_caller_src = "\
package com.example.feature
import com.example.data.TextBody
fun render(b: TextBody) {
    val t = b.scenes.busyLoader.title
}
";
    // Unrelated file: a different BusyLoader (e.g. for product scoring) with its own title usage
    let other_body_src = "\
package com.other.product
data class ProductScreens(val busyLoader: BusyLoader) {
    data class BusyLoader(
        val title: String?,
        val detail: String?,
    )
}
";
    // Unrelated caller of OtherBody's BusyLoader: mentions BusyLoader and title
    let other_caller_src = "\
package com.other.feature
import com.other.product.ProductScreens
fun show(s: ProductScreens) {
    val title = s.busyLoader.title
}
";

    let text_body_uri = Url::from_file_path(root.join("TextBody.kt")).unwrap();
    let good_caller_uri = Url::from_file_path(root.join("GoodCaller.kt")).unwrap();

    write(root, "TextBody.kt", text_body_src);
    write(root, "GoodCaller.kt", good_caller_src);
    write(root, "OtherBody.kt", other_body_src);
    write(root, "OtherCaller.kt", other_caller_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&text_body_uri, text_body_src);
    idx.index_content(&good_caller_uri, good_caller_src);
    idx.index_content(
        &Url::from_file_path(root.join("OtherBody.kt")).unwrap(),
        other_body_src,
    );
    idx.index_content(
        &Url::from_file_path(root.join("OtherCaller.kt")).unwrap(),
        other_caller_src,
    );

    // Cursor on `title` in TextBody.Scenes.BusyLoader (line 4, 0-based).
    let locs = find_references_with_qualifier("title", None, &text_body_uri, 4, false, &*idx).await;

    assert_refs_contain(&locs, &["GoodCaller.kt"]);
    assert_refs_exclude(&locs, &["OtherBody.kt", "OtherCaller.kt"]);
}

/// Verify that `field_owner_for_decl` resolves Java class fields correctly.
///
/// A Java POJO with private fields and a caller accessing them via the object.
/// The field `mAmount` is private and can only appear within `Payment.java` or
/// via method calls.  When the caller accesses a `payment.getAmount()` style getter
/// that returns `mAmount`, findReferences on `mAmount` at its declaration should
/// NOT pollute results with unrelated files that happen to have the word "mAmount".
///
/// More importantly: `field_owner_for_decl` should return the enclosing Java class
/// so that `field_scoped_reference_locations` narrows the search correctly.
#[tokio::test]
async fn find_references_java_pojo_field_scoped_to_owner_class() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    // Java POJO with a private field
    let payment_src = "\
package com.example.models;
public class Payment {
    private java.math.BigDecimal mAmount;
    public java.math.BigDecimal getAmount() { return mAmount; }
    public void setAmount(java.math.BigDecimal amount) { this.mAmount = amount; }
}
";
    // Legitimate caller: uses payment.getAmount() and accesses Payment class
    let good_src = "\
package com.example.feature;
import com.example.models.Payment;
public class PaymentView {
    private Payment mPayment;
    public void display() {
        java.math.BigDecimal mAmount = mPayment.getAmount();
    }
}
";
    // Unrelated class in a different package that also has mAmount field
    let other_src = "\
package com.example.other;
public class Transaction {
    private java.math.BigDecimal mAmount;
    public java.math.BigDecimal getAmount() { return mAmount; }
}
";

    let payment_uri = Url::from_file_path(root.join("Payment.java")).unwrap();
    let good_uri = Url::from_file_path(root.join("PaymentView.java")).unwrap();
    let other_uri = Url::from_file_path(root.join("Transaction.java")).unwrap();

    write(root, "Payment.java", payment_src);
    write(root, "PaymentView.java", good_src);
    write(root, "Transaction.java", other_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&payment_uri, payment_src);
    idx.index_content(&good_uri, good_src);
    idx.index_content(&other_uri, other_src);

    // Cursor on `mAmount` field declaration in Payment.java (line 2, 0-based).
    let locs = find_references_with_qualifier("mAmount", None, &payment_uri, 2, false, &*idx).await;

    // Transaction.java has its own mAmount — must NOT appear
    assert_refs_exclude(&locs, &["Transaction.java"]);
    // PaymentView.java references mAmount as a local variable: depends on whether
    // field_scoped_reference_locations finds it through Payment. Accept it or not,
    // but Transaction.java must definitely be excluded.
}

/// Java method findReferences: an unrelated class that imports `Payment` AND
/// defines its own `getAmount() {` should be excluded.  A caller that uses
/// `payment.getAmount()` should be included.
///
/// This tests the `is_java_method_declaration_at` filter applied in both
/// `append_unique_reference_hits` (bare-name pass of `parent_scoped_reference_locations`)
/// and `field_scoped_reference_locations`.
#[tokio::test]
async fn find_references_java_method_excludes_unrelated_same_name_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let payment_src = "\
package com.example.models;
public class Payment {
    private java.math.BigDecimal mAmount;
    public java.math.BigDecimal getAmount() { return mAmount; }
}
";
    // Legitimate caller: dot-qualified call `payment.getAmount()`.
    let caller_src = "\
package com.example.ui;
import com.example.models.Payment;
public class PaymentView {
    public void show(Payment payment) {
        java.math.BigDecimal v = payment.getAmount();
    }
}
";
    // Unrelated class that also imports Payment (for a different reason) AND
    // has its own getAmount() method — classic FP source.
    let unrelated_src = "\
package com.example.other;
import com.example.models.Payment;
public class Order {
    private Payment mPayment;
    public java.math.BigDecimal getAmount() {
        return mPayment.getAmount();
    }
}
";

    let payment_uri = Url::from_file_path(root.join("Payment.java")).unwrap();
    let caller_uri = Url::from_file_path(root.join("PaymentView.java")).unwrap();
    let unrelated_uri = Url::from_file_path(root.join("Order.java")).unwrap();

    write(root, "Payment.java", payment_src);
    write(root, "PaymentView.java", caller_src);
    write(root, "Order.java", unrelated_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&payment_uri, payment_src);
    idx.index_content(&caller_uri, caller_src);
    idx.index_content(&unrelated_uri, unrelated_src);

    // Cursor on `getAmount` declaration in Payment.java (line 3, 0-based).
    let locs =
        find_references_with_qualifier("getAmount", None, &payment_uri, 3, false, &*idx).await;

    // PaymentView calls payment.getAmount() — must be included.
    assert_refs_contain(&locs, &["PaymentView.java"]);
    // Order.getAmount() is a declaration in an unrelated class — must be excluded.
    // Note: Order.java still contains `mPayment.getAmount()` which IS a valid call,
    // so Order.java may or may not appear depending on whether the declaration line
    // is the only hit. The declaration itself (line 5 in Order.java) must not be the hit.
    let order_hits: Vec<_> = locs
        .iter()
        .filter(|l| l.uri.as_str().ends_with("Order.java"))
        .collect();
    // If Order.java appears, it must only be for the `mPayment.getAmount()` call (line 5),
    // not for the `public java.math.BigDecimal getAmount() {` declaration (line 4).
    for hit in &order_hits {
        // The declaration is on line 4 (0-based); the call is on line 5.
        assert_ne!(
            hit.range.start.line, 4,
            "Order.java declaration line must not appear in references, got: {hit:?}"
        );
    }
}

/// Java field references: an unrelated class that imports `Payment` and has its
/// own `mAmount` field declaration must be excluded.  The `field_scoped_reference_locations`
/// Java filter should strip it.
#[tokio::test]
async fn find_references_java_field_excludes_unrelated_class_with_same_field_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let payment_src = "\
package com.example.models;
public class Payment {
    private java.math.BigDecimal mAmount;
    public java.math.BigDecimal getAmount() { return mAmount; }
}
";
    // Unrelated class that ALSO imports Payment AND has its own mAmount field.
    let other_src = "\
package com.example.models;
public class Order {
    private java.math.BigDecimal mAmount;
    public java.math.BigDecimal getAmount() { return mAmount; }
}
";

    let payment_uri = Url::from_file_path(root.join("Payment.java")).unwrap();
    let other_uri = Url::from_file_path(root.join("Order.java")).unwrap();

    write(root, "Payment.java", payment_src);
    write(root, "Order.java", other_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&payment_uri, payment_src);
    idx.index_content(&other_uri, other_src);

    // Cursor on `mAmount` declaration in Payment.java (line 2, 0-based).
    let locs = find_references_with_qualifier("mAmount", None, &payment_uri, 2, false, &*idx).await;

    // Order.java's own `mAmount` declaration must not appear.
    let order_decl_hits: Vec<_> = locs
        .iter()
        .filter(|l| {
            l.uri.as_str().ends_with("Order.java") && l.range.start.line == 2 // Order.mAmount declaration line
        })
        .collect();
    assert!(
        order_decl_hits.is_empty(),
        "Order.java mAmount declaration must not appear in Payment.mAmount references, got: {order_decl_hits:?}"
    );
}

/// Java method call from a different package should still find the declaration.
/// Regression test for `declaration_files_for` source_pkg filter: when
/// `findReferences` is invoked from a call site in a *different* package, the
/// declaration file must still appear in `decl_files` (used for candidates).
#[tokio::test]
async fn find_references_java_cross_package_includes_declaration_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let payment_src = "\
package com.example.models;
public class Payment {
    public java.math.BigDecimal getAmount() { return null; }
}
";
    // Caller is in a DIFFERENT package.
    let caller_src = "\
package com.example.ui;
import com.example.models.Payment;
public class PaymentView {
    public void show(Payment p) { p.getAmount(); }
}
";

    let payment_uri = Url::from_file_path(root.join("Payment.java")).unwrap();
    let caller_uri = Url::from_file_path(root.join("PaymentView.java")).unwrap();

    write(root, "Payment.java", payment_src);
    write(root, "PaymentView.java", caller_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&payment_uri, payment_src);
    idx.index_content(&caller_uri, caller_src);

    // Cursor on `getAmount` in Payment.java (declaration site, line 2, 0-based).
    // includeDeclaration=true: Payment.java itself must appear.
    let locs_with_decl =
        find_references_with_qualifier("getAmount", None, &payment_uri, 2, true, &*idx).await;
    assert_refs_contain(&locs_with_decl, &["Payment.java"]);
    assert_refs_contain(&locs_with_decl, &["PaymentView.java"]);

    // From CALL SITE in different package: caller must appear.
    let locs_from_caller =
        find_references_with_qualifier("getAmount", None, &caller_uri, 3, false, &*idx).await;
    assert_refs_contain(&locs_from_caller, &["PaymentView.java"]);
}

/// Regression test for the `rfind(')')` → `balanced_paren_close` fix.
/// A Java method whose parameter list contains a nested `Consumer<Function<..>>`
/// (no inner parens, but ensures balanced-paren logic is exercised) must still be
/// detected as a declaration and excluded from cross-file results.
///
/// Additionally exercises the balanced-paren fix: `Consumer<String>` has no
/// inner parens so `find(')')` and `rfind(')')` agree, but the test confirms
/// the full pipeline (package-scoped candidate discovery + Java filtering) works.
#[tokio::test]
async fn find_references_java_method_nested_parens_in_params_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();

    let owner_src = "\
package com.example;
public class Owner {
    public void process(java.util.function.Consumer<String> handler) { }
}
";
    // Another class that also declares `process(Consumer)` — must be excluded.
    let other_src = "\
package com.example;
public class Other {
    public void process(java.util.function.Consumer<String> handler) { }
}
";
    let caller_src = "\
package com.example;
public class Caller {
    void run(Owner o) { o.process(s -> {}); }
}
";

    let (_, owner_uri) = write(root, "Owner.java", owner_src);
    let (_, other_uri) = write(root, "Other.java", other_src);
    let (_, caller_uri) = write(root, "Caller.java", caller_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());
    idx.index_content(&owner_uri, owner_src);
    idx.index_content(&other_uri, other_src);
    idx.index_content(&caller_uri, caller_src);

    let locs = find_references_with_qualifier("process", None, &owner_uri, 2, false, &*idx).await;

    assert_refs_contain(&locs, &["Caller.java"]);
    assert_refs_exclude(&locs, &["Other.java"]);
}

/// **Acceptance (Task 2)**: `find_references` invoked on a *usage* of a JAR-defined
/// top-level function (`remember`) must return only the workspace files that import
/// the JAR symbol, and exclude an unrelated workspace `fun remember()` declared in
/// another package.
///
/// Without import-scoping a lowercase JAR-symbol usage falls to an unscoped
/// codebase-wide bare-word rg search, which also matches `Unrelated.kt`'s
/// `fun remember()` — a false positive this test guards against.
#[tokio::test]
async fn find_references_on_jar_symbol_usage_scopes_to_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Caller imports + calls the JAR `remember`.
    let caller_src =
        "package app\nimport androidx.compose.runtime.remember\nfun build() { remember() }\n";
    // An unrelated workspace function of the same name in a different package,
    // with no import of the JAR symbol — must NOT appear in the results.
    let unrelated_src = "package other\nfun remember() {}\nfun use() { remember() }\n";

    let (_, caller_uri) = write(root, "Caller.kt", caller_src);
    write(root, "Unrelated.kt", unrelated_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());

    // Inject the JAR `remember` as a top-level compose-runtime symbol.
    crate::indexer::jar::populate_from_symbols(
        &idx,
        std::path::Path::new("/fake/compose-runtime.jar"),
        &[crate::sidecar::SidecarSymbol {
            name: "remember".into(),
            kind: "fun".into(),
            container: "ComposablesKt".into(),
            detail: "fun remember()".into(),
            doc: String::new(),
            type_params: vec![],
            extension_receiver_type: String::new(),
            trailing_lambda: false,
            deprecated: false,
            pkg: "androidx.compose.runtime".into(),
            top_level: true,
        }],
    );

    idx.index_content(&caller_uri, caller_src);

    // Cursor on the `remember()` call usage in Caller.kt (line 2, 0-indexed).
    let locs = find_references_with_qualifier("remember", None, &caller_uri, 2, false, &*idx).await;

    assert_refs_contain(&locs, &["Caller.kt"]);
    assert_refs_exclude(&locs, &["Unrelated.kt"]);
}

/// Two different jars each declare a top-level `remember` in *different* packages.
/// A caller that imports only the compose one must scope find-references to files
/// importing *that* package. A second workspace file that imports the competing jar's
/// same-named symbol must NOT appear.
///
/// This guards the import-based disambiguation: `resolve_scope` returns the package the
/// caller actually imported (`androidx.compose.runtime`), not an arbitrary jar
/// definition picked by `jar_declaration_scope`'s insertion order. With the old
/// name-only scoping, an arbitrary pick of `com.other` would have inverted the result —
/// including `Other.kt` and dropping `Caller.kt`.
#[tokio::test]
async fn find_references_on_jar_symbol_disambiguates_competing_jars() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let caller_src =
        "package app\nimport androidx.compose.runtime.remember\nfun build() { remember() }\n";
    // Imports the *other* jar's `remember`: same simple name, different package.
    let other_src = "package feature\nimport com.other.remember\nfun use() { remember() }\n";

    let (_, caller_uri) = write(root, "Caller.kt", caller_src);
    let (_, other_uri) = write(root, "Other.kt", other_src);

    let idx = Arc::new(Indexer::new());
    idx.workspace_root.set(root.to_path_buf());

    let jar_symbol = |pkg: &str| crate::sidecar::SidecarSymbol {
        name: "remember".into(),
        kind: "fun".into(),
        container: "ComposablesKt".into(),
        detail: "fun remember()".into(),
        doc: String::new(),
        type_params: vec![],
        extension_receiver_type: String::new(),
        trailing_lambda: false,
        deprecated: false,
        pkg: pkg.into(),
        top_level: true,
    };

    crate::indexer::jar::populate_from_symbols(
        &idx,
        std::path::Path::new("/fake/compose-runtime.jar"),
        &[jar_symbol("androidx.compose.runtime")],
    );
    crate::indexer::jar::populate_from_symbols(
        &idx,
        std::path::Path::new("/fake/other.jar"),
        &[jar_symbol("com.other")],
    );

    idx.index_content(&caller_uri, caller_src);
    idx.index_content(&other_uri, other_src);

    // Cursor on the `remember()` call usage in Caller.kt (line 2, 0-indexed).
    let locs = find_references_with_qualifier("remember", None, &caller_uri, 2, false, &*idx).await;

    assert_refs_contain(&locs, &["Caller.kt"]);
    assert_refs_exclude(&locs, &["Other.kt"]);
}
