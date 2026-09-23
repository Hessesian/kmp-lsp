//! Tests for the `rg` module — extracted from `indexer.rs` tests.
//!
//! Referenced from `src/rg.rs` via:
//! ```rust
//! #[cfg(test)]
//! #[path = "rg_tests.rs"]
//! mod tests;
//! ```

use tower_lsp::lsp_types::{SymbolKind, Url};

use crate::rg::{
    declared_type_from_detail, declared_type_from_raw_lines, file_uri_under_source_paths,
    is_declaration_occurrence_at, is_declaration_of, is_unusable_producer_name, parse_rg_line,
    rg_find_definition, rg_find_references, IgnoreMatcher, ProducerCandidate, RgSearchRequest,
};

// ─── parse_rg_line ────────────────────────────────────────────────────────────

#[test]
fn parse_rg_line_basic() {
    // needs a drive prefix on Windows
    #[cfg(not(windows))]
    let line = "/home/user/project/Foo.kt:10:5:class Foo {";
    #[cfg(windows)]
    let line = r"C:\home\user\project\Foo.kt:10:5:class Foo {";

    let loc = parse_rg_line(line).unwrap();
    assert_eq!(loc.range.start.line, 9); // 1-indexed → 0-indexed
    assert_eq!(loc.range.start.character, 4);
    #[cfg(not(windows))]
    assert_eq!(loc.uri.path(), "/home/user/project/Foo.kt");
    #[cfg(windows)]
    assert!(
        loc.uri.path().ends_with("/Foo.kt"),
        "got: {}",
        loc.uri.path()
    );
}

#[test]
fn rg_line_relative_path_ignored() {
    // Before the fix this would panic / produce a wrong URI
    let line = "src/Foo.kt:10:5:class Foo {";
    assert!(
        parse_rg_line(line).is_none(),
        "relative paths must be ignored"
    );
}

// ─── rg_find_references scoping ──────────────────────────────────────────────

/// Write `content` to `dir/rel_path` and return the absolute path as String.
fn write_temp(dir: &std::path::Path, rel_path: &str, content: &str) -> String {
    let p = dir.join(rel_path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, content).unwrap();
    p.to_str().unwrap().to_owned()
}

/// `rg_find_references` must not bleed references across sealed interfaces
/// that share the same inner name (`Event`) but belong to different contracts.
///
/// Layout:
///   activate_contract.kt   — declares interface ActivateUpdateAppContract { sealed interface Event }
///   other_contract.kt      — declares interface OtherContract             { sealed interface Event }
///   activate_vm.kt         — imports ActivateUpdateAppContract.Event, uses bare `Event`
///   other_vm.kt            — imports OtherContract.Event,             uses bare `Event`
///
/// Finding refs for ActivateUpdateAppContract.Event must return hits in
/// activate_contract.kt and activate_vm.kt ONLY — not other_vm.kt.
#[test]
fn refs_inner_class_does_not_bleed_across_contracts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    write_temp(
        root,
        "activate_contract.kt",
        concat!(
            "package com.example.activate\n",
            "interface ActivateUpdateAppContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "other_contract.kt",
        concat!(
            "package com.example.other\n",
            "interface OtherContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "activate_vm.kt",
        concat!(
            "package com.example.activate\n",
            "import com.example.activate.ActivateUpdateAppContract.Event\n",
            "class ActivateViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "other_vm.kt",
        concat!(
            "package com.example.other\n",
            "import com.example.other.OtherContract.Event\n",
            "class OtherViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );

    let activate_uri = Url::from_file_path(root.join("activate_contract.kt")).unwrap();
    let activate_decl = root
        .join("activate_contract.kt")
        .to_str()
        .unwrap()
        .to_owned();

    // Simulate: cursor on declaration of Event inside ActivateUpdateAppContract.
    // parent_class = "ActivateUpdateAppContract", declared_pkg = "com.example.activate"
    let decl_files = [activate_decl];
    let request = RgSearchRequest::new(
        "Event",
        Some("ActivateUpdateAppContract"),
        Some("com.example.activate"), // declared_pkg
        Some(root),
        true, // include_declaration
        &activate_uri,
        &decl_files,
    );
    let locs = rg_find_references(&request, None);

    let hit_files: std::collections::HashSet<String> = locs
        .iter()
        .map(|l| {
            l.uri
                .to_file_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    assert!(
        hit_files.contains("activate_contract.kt"),
        "should include declaration file; got: {hit_files:?}"
    );
    assert!(
        hit_files.contains("activate_vm.kt"),
        "should include file that imports ActivateUpdateAppContract.Event; got: {hit_files:?}"
    );
    assert!(
        !hit_files.contains("other_vm.kt"),
        "must NOT include file that only imports OtherContract.Event; got: {hit_files:?}"
    );
    assert!(
        !hit_files.contains("other_contract.kt"),
        "must NOT include OtherContract declaration; got: {hit_files:?}"
    );
}

/// When cursor is on `Event` inside a file that imports `OtherContract.Event`,
/// refs must not include files that only import `ActivateUpdateAppContract.Event`.
#[test]
fn refs_inner_class_resolved_from_import_in_reference_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    write_temp(
        root,
        "activate_contract.kt",
        concat!(
            "package com.example.activate\n",
            "interface ActivateUpdateAppContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "other_contract.kt",
        concat!(
            "package com.example.other\n",
            "interface OtherContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "activate_vm.kt",
        concat!(
            "package com.example.activate\n",
            "import com.example.activate.ActivateUpdateAppContract.Event\n",
            "class ActivateViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "other_vm.kt",
        concat!(
            "package com.example.other\n",
            "import com.example.other.OtherContract.Event\n",
            "class OtherViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );

    // Simulate: cursor on `Event` inside other_vm.kt (a reference, not declaration).
    // resolve_symbol_via_import on other_vm.kt → parent=OtherContract, pkg=com.example.other
    let other_vm_uri = Url::from_file_path(root.join("other_vm.kt")).unwrap();
    let other_decl = root.join("other_contract.kt").to_str().unwrap().to_owned();

    let decl_files = [other_decl];
    let request = RgSearchRequest::new(
        "Event",
        Some("OtherContract"),
        Some("com.example.other"),
        Some(root),
        true,
        &other_vm_uri,
        &decl_files,
    );
    let locs = rg_find_references(&request, None);

    let hit_files: std::collections::HashSet<String> = locs
        .iter()
        .map(|l| {
            l.uri
                .to_file_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    assert!(
        hit_files.contains("other_contract.kt"),
        "should include OtherContract declaration; got: {hit_files:?}"
    );
    assert!(
        hit_files.contains("other_vm.kt"),
        "should include file importing OtherContract.Event; got: {hit_files:?}"
    );
    assert!(
        !hit_files.contains("activate_vm.kt"),
        "must NOT include file importing ActivateUpdateAppContract.Event; got: {hit_files:?}"
    );
}

/// Regression: when `decl_files` is unfiltered it includes ALL contracts that
/// declare a `sealed interface Event`, causing every consumer ViewModel to appear
/// in results for an unrelated contract's Event.
///
/// Layout: two contracts each with `sealed interface Event`, two ViewModels each
/// importing their own contract's Event.  Finding refs for DashboardContract.Event
/// must NOT return VisitBranchViewModel even though both are in `decl_files` when
/// unfiltered by enclosing-class.
#[test]
fn refs_decl_files_filtered_by_enclosing_class() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    write_temp(
        root,
        "DashboardContract.kt",
        concat!(
            "package com.example.dashboard\n",
            "interface DashboardContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "VisitBranchContract.kt",
        concat!(
            "package com.example.visitbranch\n",
            "interface VisitBranchContract {\n",
            "  sealed interface Event\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "DashboardViewModel.kt",
        concat!(
            "package com.example.dashboard\n",
            "import com.example.dashboard.DashboardContract.Event\n",
            "class DashboardViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );
    write_temp(
        root,
        "VisitBranchViewModel.kt",
        concat!(
            "package com.example.visitbranch\n",
            "import com.example.visitbranch.VisitBranchContract.Event\n",
            "class VisitBranchViewModel {\n",
            "  fun handle(e: Event) {}\n",
            "}\n",
        ),
    );

    let dashboard_uri = Url::from_file_path(root.join("DashboardContract.kt")).unwrap();
    // decl_files filtered to only DashboardContract.kt (enclosing = DashboardContract)
    let dashboard_decl = root
        .join("DashboardContract.kt")
        .to_str()
        .unwrap()
        .to_owned();

    let decl_files = [dashboard_decl];
    let request = RgSearchRequest::new(
        "Event",
        Some("DashboardContract"),
        Some("com.example.dashboard"),
        Some(root),
        true,
        &dashboard_uri,
        &decl_files, // NOT including VisitBranchContract.kt
    );
    let locs = rg_find_references(&request, None);

    let hit_files: std::collections::HashSet<String> = locs
        .iter()
        .map(|l| {
            l.uri
                .to_file_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    assert!(
        hit_files.contains("DashboardContract.kt"),
        "should include DashboardContract declaration; got: {hit_files:?}"
    );
    assert!(
        hit_files.contains("DashboardViewModel.kt"),
        "should include DashboardViewModel; got: {hit_files:?}"
    );
    assert!(
        !hit_files.contains("VisitBranchViewModel.kt"),
        "must NOT include VisitBranchViewModel; got: {hit_files:?}"
    );
    assert!(
        !hit_files.contains("VisitBranchContract.kt"),
        "must NOT include VisitBranchContract; got: {hit_files:?}"
    );
}

// ─── rg_find_definition / rg_find_references ignore-pattern filtering ─────────

/// `rg_find_definition` must not return results from ignored directories.
#[test]
fn rg_find_definition_filters_ignored_dirs() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/Real.kt"),
        "package com.example\nclass MyClass\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("buildSrc/generated")).unwrap();
    std::fs::write(
        root.join("buildSrc/generated/MyClass.kt"),
        "package com.example\nclass MyClass\n",
    )
    .unwrap();

    let matcher = IgnoreMatcher::new(vec!["buildSrc".to_owned()], root);
    let locs = rg_find_definition("MyClass", Some(root), &[], Some(&matcher));
    let files: Vec<String> = locs
        .iter()
        .map(|l| {
            l.uri
                .to_file_path()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert!(
        files.iter().any(|f| f.contains("src/Real.kt")),
        "must include real source; got: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains("buildSrc")),
        "must not include buildSrc results; got: {files:?}"
    );
}

/// `rg_find_references` must exclude candidate files from ignored directories.
#[test]
fn rg_find_references_filters_ignored_dirs() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/Contract.kt"),
        "package com.example\nclass Contract {\n  class Event\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/User.kt"),
        "package com.example\nimport com.example.Contract.Event\nfun use(e: Event) {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("buildSrc")).unwrap();
    std::fs::write(
        root.join("buildSrc/Contract.kt"),
        "package com.example\nclass Contract {\n  class Event\n}\n",
    )
    .unwrap();

    let uri = Url::from_file_path(root.join("src/Contract.kt")).unwrap();
    let decl = root.join("src/Contract.kt").to_str().unwrap().to_owned();
    let matcher = IgnoreMatcher::new(vec!["buildSrc".to_owned()], root);

    let decl_files = [decl];
    let request = RgSearchRequest::new(
        "Event",
        Some("Contract"),
        Some("com.example"),
        Some(root),
        true,
        &uri,
        &decl_files,
    );
    let locs = rg_find_references(&request, Some(&matcher));
    let files: Vec<String> = locs
        .iter()
        .map(|l| l.uri.to_file_path().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(
        !files.iter().any(|f| f.contains("buildSrc")),
        "must not include buildSrc in references; got: {files:?}"
    );
}

/// `rg_find_definition` with non-empty `source_paths` must only return results
/// from within those directories, not from the full workspace root.
#[test]
fn rg_find_definition_scoped_to_source_paths() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("app/src/main/kotlin")).unwrap();
    std::fs::write(root.join("app/src/main/kotlin/Foo.kt"), "class Foo\n").unwrap();

    // A second directory that should NOT be searched when source_paths is set.
    std::fs::create_dir_all(root.join("generated")).unwrap();
    std::fs::write(root.join("generated/Foo.kt"), "class Foo\n").unwrap();

    let source_path = root
        .join("app/src/main/kotlin")
        .to_string_lossy()
        .into_owned();
    let source_paths = vec![source_path.clone()];

    let locs = rg_find_definition("Foo", Some(root), &source_paths, None);
    let files: Vec<String> = locs
        .iter()
        .map(|l| {
            l.uri
                .to_file_path()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert!(
        !files.is_empty(),
        "must find Foo inside the configured source_path; got nothing"
    );
    assert!(
        files.iter().all(|f| f.contains("app/src/main/kotlin")),
        "must only return results from source_paths; got: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains("generated")),
        "must not include files outside source_paths; got: {files:?}"
    );
}

/// `rg_find_definition` with multiple source_paths searches ALL of them.
/// Regression test for GitHub issue #78: rg only searched one source root.
#[test]
fn rg_find_definition_searches_all_source_paths() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    // Two separate source roots
    std::fs::create_dir_all(root.join("frameworks/base/src")).unwrap();
    std::fs::write(
        root.join("frameworks/base/src/PolicyHandle.kt"),
        "class PolicyHandle\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("cts/src")).unwrap();
    std::fs::write(
        root.join("cts/src/PolicyIdentifier.kt"),
        "class PolicyIdentifier\n",
    )
    .unwrap();

    let source_paths = vec![
        root.join("frameworks/base/src")
            .to_string_lossy()
            .into_owned(),
        root.join("cts/src").to_string_lossy().into_owned(),
    ];

    // Search for a symbol in source root #1
    let locs = rg_find_definition("PolicyHandle", Some(root), &source_paths, None);
    assert!(
        !locs.is_empty(),
        "must find PolicyHandle in frameworks/base"
    );

    // Search for a symbol in source root #2
    let locs = rg_find_definition("PolicyIdentifier", Some(root), &source_paths, None);
    assert!(
        !locs.is_empty(),
        "must find PolicyIdentifier in cts (second source root)"
    );
}

/// `rg_find_definition` with empty `source_paths` falls back to searching the
/// entire workspace root (backward-compatible behavior).
#[test]
fn rg_find_definition_empty_source_paths_falls_back_to_root() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("app/src/main/kotlin")).unwrap();
    std::fs::write(root.join("app/src/main/kotlin/Bar.kt"), "class Bar\n").unwrap();

    // With empty source_paths, should find via workspace root scan.
    let locs = rg_find_definition("Bar", Some(root), &[], None);
    assert!(
        !locs.is_empty(),
        "must find Bar when source_paths is empty (fallback to root)"
    );
}

/// `rg_find_references` with `with_source_paths` must limit candidate-file
/// discovery and reference search to the configured source root.
#[test]
fn rg_find_references_scoped_to_source_paths() {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let root = dir.path();

    // Source root: contains the declaration and a legitimate reference.
    std::fs::create_dir_all(root.join("app/src/main/kotlin/com/example")).unwrap();
    std::fs::write(
        root.join("app/src/main/kotlin/com/example/Contract.kt"),
        "package com.example\nclass Contract {\n  class Event\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/src/main/kotlin/com/example/User.kt"),
        "package com.example\nfun use(e: Contract.Event) {}\n",
    )
    .unwrap();

    // Outside source root: has a usage of Contract.Event — must NOT appear in scoped results.
    // Without scoping, rg_find_references would return this file; with scoping it must be excluded.
    std::fs::create_dir_all(root.join("generated/com/example")).unwrap();
    std::fs::write(
        root.join("generated/com/example/OutsideUser.kt"),
        "package com.example\nfun outsideUse(e: Contract.Event) {}\n",
    )
    .unwrap();

    let source_path = root
        .join("app/src/main/kotlin")
        .to_string_lossy()
        .into_owned();
    let source_paths = vec![source_path];

    let decl_uri =
        Url::from_file_path(root.join("app/src/main/kotlin/com/example/Contract.kt")).unwrap();
    let decl_file = root
        .join("app/src/main/kotlin/com/example/Contract.kt")
        .to_string_lossy()
        .into_owned();
    let decl_files = [decl_file];

    let request = RgSearchRequest::new(
        "Event",
        Some("Contract"),
        Some("com.example"),
        Some(root),
        true,
        &decl_uri,
        &decl_files,
    )
    .with_source_paths(&source_paths);

    let locs = rg_find_references(&request, None);
    let files: Vec<String> = locs
        .iter()
        .map(|l| l.uri.to_file_path().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(
        !files.is_empty(),
        "must find references inside configured source_paths; got nothing"
    );
    assert!(
        !files.iter().any(|f| f.contains("generated")),
        "must not include files outside source_paths (generated/); got: {files:?}"
    );
}

// ─── is_declaration_of ────────────────────────────────────────────────────────

#[test]
fn is_declaration_of_matches_exact_name() {
    assert!(is_declaration_of("    fun create(): Foo", "create"));
    assert!(is_declaration_of("    fun create(x: Int): Foo", "create"));
    assert!(is_declaration_of("val create: Factory", "create"));
}

#[test]
fn is_declaration_of_rejects_longer_name_with_same_prefix() {
    // "fun createWidget" must NOT be treated as a declaration of "create"
    assert!(!is_declaration_of(
        "    fun createWidget(): Widget",
        "create"
    ));
    assert!(!is_declaration_of(
        "    fun createReducer() = factory.create()",
        "create"
    ));
    assert!(!is_declaration_of(
        "    fun createAccount(name: String): Account",
        "create"
    ));
}

#[test]
fn is_declaration_of_rejects_call_site_in_non_declaration() {
    assert!(!is_declaration_of("    val x = factory.create()", "create"));
    assert!(!is_declaration_of(
        "    fun build() = factory.create()",
        "create"
    ));
}

// ─── is_declaration_occurrence_at ────────────────────────────────────────────

#[test]
fn decl_occurrence_at_fun_declaration() {
    // "    fun getRate()" — col of 'g' is 8 (after "    fun ")
    let line = "    fun getRate(): Int";
    let col = line.find("getRate").unwrap() as u32;
    assert!(is_declaration_occurrence_at(line, col));
}

#[test]
fn decl_occurrence_at_override_fun_declaration() {
    // "    override fun getRate()" — the 'g' is a declaration
    let line = "    override fun getRate(): Int";
    let col = line.find("getRate").unwrap() as u32;
    assert!(is_declaration_occurrence_at(line, col));
}

#[test]
fn decl_occurrence_at_call_site_is_not_declaration() {
    // "    return delegate.getRate()" — the 'g' in 'getRate' is a call site
    let line = "    return delegate.getRate()";
    let col = line.find("getRate").unwrap() as u32;
    assert!(!is_declaration_occurrence_at(line, col));
}

#[test]
fn decl_occurrence_at_second_occurrence_is_call_site() {
    // Expression-body override: first `getRate` is the declaration, second is a call.
    let line = "    override fun getRate() = delegate.getRate()";
    let first_col = line.find("getRate").unwrap() as u32;
    let second_col = line.rfind("getRate").unwrap() as u32;
    assert!(
        is_declaration_occurrence_at(line, first_col),
        "first occurrence should be a declaration"
    );
    assert!(
        !is_declaration_occurrence_at(line, second_col),
        "second occurrence (call site) should not be a declaration"
    );
}

#[test]
fn decl_occurrence_at_does_not_match_longer_prefixed_identifier() {
    // "    fun notafun getRate()" — prefix ends with "notafun", not keyword "fun"
    let line = "    val notafunRate = 1";
    let col = line.find("Rate").unwrap() as u32;
    assert!(!is_declaration_occurrence_at(line, col));
}

/// Regression: `findReferences` on an interface method should return both
/// override declarations (implementations) AND same-package call sites.
/// Previously only overrides were returned (call sites were missing due to
/// wrong package-based scoping instead of class-import-based scoping).
#[test]
fn rg_find_references_includes_overrides_and_call_sites() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Interface declaration file
    let iface_content =
        "package com.example\ninterface IGoldConversionRepository {\n    fun getRate(): Int\n}\n";
    let impl_content = "package com.example\nclass GoldConversionRepositoryImpl : IGoldConversionRepository {\n    override fun getRate(): Int = 42\n}\n";
    let interactor_content = "package com.example\nclass Interactor(val repo: IGoldConversionRepository) {\n    fun run() = repo.getRate()\n}\n";

    let iface_path = write_temp(root, "IGoldConversionRepository.kt", iface_content);
    write_temp(root, "GoldConversionRepositoryImpl.kt", impl_content);
    write_temp(root, "Interactor.kt", interactor_content);

    let iface_uri = Url::from_file_path(&iface_path).unwrap();
    let decl_files = vec![iface_path.clone()];

    let request = RgSearchRequest::new(
        "getRate",
        Some("IGoldConversionRepository"),
        Some("com.example"),
        Some(root),
        false, // include_decl = false
        &iface_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let lines: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            let path = l.uri.to_file_path().ok()?;
            let content = std::fs::read_to_string(&path).ok()?;
            content
                .lines()
                .nth(l.range.start.line as usize)
                .map(|s| s.to_owned())
        })
        .collect();

    assert!(
        lines.iter().any(|l| l.contains("override fun getRate()")),
        "override declaration (implementation) must be included as a valid reference; got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("repo.getRate()")),
        "same-package call site in Interactor.kt must be included; got: {lines:?}"
    );
}

/// Cross-package callers that import the interface via a simple class import
/// (`import com.example.IGoldConversionRepository`) must be found even though
/// the import doesn't contain the method name.
#[test]
fn rg_find_references_finds_cross_package_callers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let iface_content =
        "package com.example\ninterface IGoldConversionRepository {\n    fun getRate(): Int\n}\n";
    // Cross-package caller: imports the interface, calls through a variable.
    let cross_pkg_content = "package com.feature\nimport com.example.IGoldConversionRepository\nclass FeatureInteractor(val repo: IGoldConversionRepository) {\n    fun compute() = repo.getRate() * 2\n}\n";

    let iface_path = write_temp(root, "IGoldConversionRepository.kt", iface_content);
    write_temp(root, "FeatureInteractor.kt", cross_pkg_content);

    let iface_uri = Url::from_file_path(&iface_path).unwrap();
    let decl_files = vec![iface_path.clone()];

    let request = RgSearchRequest::new(
        "getRate",
        Some("IGoldConversionRepository"),
        Some("com.example"),
        Some(root),
        false,
        &iface_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let lines: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            let path = l.uri.to_file_path().ok()?;
            let content = std::fs::read_to_string(&path).ok()?;
            content
                .lines()
                .nth(l.range.start.line as usize)
                .map(|s| s.to_owned())
        })
        .collect();

    assert!(
        lines.iter().any(|l| l.contains("repo.getRate()")),
        "cross-package call site in FeatureInteractor.kt must be found; got: {lines:?}"
    );
}

/// Regression: nested uppercase types (e.g. `IntroContract.Event`) must NOT produce false
/// positives from files that import the parent class for an unrelated reason.
///
/// A file that imports `IntroContract` only to use `IntroContract.State` may contain its
/// own `Event` class. Previously the broad import-pattern (`import.*IntroContract`) included
/// it as a candidate, causing bare `Event` hits from the wrong class to appear.
#[test]
fn rg_find_references_nested_type_excludes_unrelated_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Declaring file — `Event` is a nested sealed class inside `IntroContract`.
    let contract = "package com.example.intro\ninterface IntroContract {\n    sealed class Event\n    sealed class State\n}\n";
    // Legitimate caller — imports `IntroContract.Event` and uses bare `Event`.
    let good_caller = "package com.feature\nimport com.example.intro.IntroContract.Event\nfun handle(e: Event) {}\n";
    // Unrelated importer — imports `IntroContract` only for `State`; contains a different `Event`.
    let unrelated = "package com.other\nimport com.example.intro.IntroContract\nsealed class Event\nfun use(s: IntroContract.State, e: Event) {}\n";

    let contract_path = write_temp(root, "IntroContract.kt", contract);
    write_temp(root, "GoodCaller.kt", good_caller);
    write_temp(root, "Unrelated.kt", unrelated);

    let contract_uri = Url::from_file_path(&contract_path).unwrap();
    let decl_files = vec![contract_path.clone()];

    let request = RgSearchRequest::new(
        "Event",
        Some("IntroContract"),
        Some("com.example.intro"),
        Some(root),
        false,
        &contract_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let paths: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            l.uri
                .to_file_path()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .collect();

    assert!(
        paths.iter().any(|p| p == "GoodCaller.kt"),
        "legitimate caller via explicit import must be found; got: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p == "Unrelated.kt"),
        "file that imports IntroContract for unrelated member must NOT appear; got: {paths:?}"
    );
}

/// Regression: nested uppercase types found via `Parent.*` star-import must still be included.
///
/// A file that has `import com.example.intro.IntroContract.*` can use bare `Event`, and
/// its occurrences must be reported.  The star-import branch uses `\.\*` (no word-boundary
/// after `*`) — this test guards against a broken regex.
#[test]
fn rg_find_references_nested_type_star_import_included() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let contract =
        "package com.example.intro\ninterface IntroContract {\n    sealed class Event\n}\n";
    // Caller using star-import of the parent class.
    let star_caller =
        "package com.feature\nimport com.example.intro.IntroContract.*\nfun handle(e: Event) {}\n";

    let contract_path = write_temp(root, "IntroContract.kt", contract);
    write_temp(root, "StarCaller.kt", star_caller);

    let contract_uri = Url::from_file_path(&contract_path).unwrap();
    let decl_files = vec![contract_path.clone()];

    let request = RgSearchRequest::new(
        "Event",
        Some("IntroContract"),
        Some("com.example.intro"),
        Some(root),
        false,
        &contract_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let paths: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            l.uri
                .to_file_path()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .collect();

    assert!(
        paths.iter().any(|p| p == "StarCaller.kt"),
        "caller via `IntroContract.*` star-import must be found; got: {paths:?}"
    );
}

/// Regression: `import ...Parent.Name.Companion` must NOT qualify a file as a candidate
/// for bare `Name` scanning.
///
/// `\b` is true before `.`, so without an explicit terminator the pattern
/// `import.*Parent\.Name\b` would match `import...IntroContract.Event.Companion`,
/// wrongly treating the file as able to use bare `Event`.
/// The fix uses `(?:\s|;|$)` instead of `\b` after the name segment.
#[test]
fn rg_find_references_nested_type_companion_import_not_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let contract =
        "package com.example.intro\ninterface IntroContract {\n    sealed class Event\n}\n";
    // Imports Event.Companion — does NOT make bare `Event` available.
    // Body only uses `Companion` directly (aliased by the import) and its own `Event`.
    let companion_caller = "package com.other\nimport com.example.intro.IntroContract.Event.Companion\nsealed class Event\nfun foo(e: Event) {}\n";

    let contract_path = write_temp(root, "IntroContract.kt", contract);
    write_temp(root, "CompanionCaller.kt", companion_caller);

    let contract_uri = Url::from_file_path(&contract_path).unwrap();
    let decl_files = vec![contract_path.clone()];

    let request = RgSearchRequest::new(
        "Event",
        Some("IntroContract"),
        Some("com.example.intro"),
        Some(root),
        false,
        &contract_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let paths: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            l.uri
                .to_file_path()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .collect();

    assert!(
        !paths.iter().any(|p| p == "CompanionCaller.kt"),
        "file importing Event.Companion must NOT be a bare-Event candidate; got: {paths:?}"
    );
}

#[test]
fn java_method_declaration_recognises_semicolon_form() {
    use crate::rg::is_java_method_declaration_at;
    // Interface / abstract method — no body, ends with `;`
    assert!(is_java_method_declaration_at(
        "    void process(String input);",
        "process",
        9
    ));
    // Normal concrete method — ends with `{`
    assert!(is_java_method_declaration_at(
        "    void process(String input) {",
        "process",
        9
    ));
    // Abstract with throws clause
    assert!(is_java_method_declaration_at(
        "    void process(String input) throws IOException;",
        "process",
        9
    ));
    // Call site — ends with `);` should NOT be treated as a declaration
    assert!(!is_java_method_declaration_at(
        "        obj.process(input);",
        "process",
        12
    ));
    // Dot-qualified call
    assert!(!is_java_method_declaration_at(
        "    helper.process(x);",
        "process",
        11
    ));
}

// ─── declared_type_from_detail ─────────────────────────────────────────────────
//
// `detail` inputs below are real `SymbolEntry::detail` shapes (as produced by
// `extract_detail_from_node` at parse time) — not raw source lines. See
// `docs/superpowers/plans/2026-09-22b-producer-detection-cst-follow-up-plan.md`.

#[test]
fn declared_type_from_detail_extracts_kotlin_function_return_type() {
    assert_eq!(
        declared_type_from_detail("fun openBody(): Body", SymbolKind::FUNCTION).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_kotlin_val_type() {
    assert_eq!(
        declared_type_from_detail("val cachedBody: Body", SymbolKind::PROPERTY).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_kotlin_var_type() {
    // `var` is indexed as `SymbolKind::VARIABLE`, not `PROPERTY` — see
    // `src/queries.rs`. Both must be handled the same way.
    assert_eq!(
        declared_type_from_detail("var count: Int", SymbolKind::VARIABLE).as_deref(),
        Some("Int")
    );
}

#[test]
fn declared_type_from_detail_extracts_type_past_override_modifier() {
    assert_eq!(
        declared_type_from_detail(
            "override val factory: Reducer.Factory",
            SymbolKind::PROPERTY
        )
        .as_deref(),
        Some("Reducer.Factory")
    );
}

#[test]
fn declared_type_from_detail_extracts_type_past_an_annotated_lateinit_var() {
    // The canonical Dagger/Hilt field-injection shape this whole feature
    // targets — real regression: delegating straight to
    // `extract_property_type_from_detail` (which strips only a leading
    // visibility modifier) silently returned `None` here, since neither
    // `@Inject` nor `lateinit` is a visibility modifier it knows about.
    assert_eq!(
        declared_type_from_detail(
            "@Inject lateinit var repository: Repository",
            SymbolKind::VARIABLE
        )
        .as_deref(),
        Some("Repository")
    );
}

#[test]
fn declared_type_from_detail_extracts_type_past_const_modifier() {
    assert_eq!(
        declared_type_from_detail("const val cachedBody: Body", SymbolKind::PROPERTY).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_kotlin_nested_method_return_type() {
    // A Kotlin member function nested inside a class/interface/object is
    // indexed as `SymbolKind::METHOD` (nesting demotes it from `FUNCTION`,
    // see `parser.rs`'s `push_def_symbols`) — the SAME `SymbolKind` a Java
    // method uses, but the detail shape stays Kotlin's `"fun ...): Type"`.
    // Found via a real fixture (`field_reference_found_through_inferred_receiver_type`)
    // that failed until `METHOD` disambiguated by the literal `fun` keyword.
    assert_eq!(
        declared_type_from_detail("fun openBody(): Body", SymbolKind::METHOD).as_deref(),
        Some("Body")
    );
    assert_eq!(
        declared_type_from_detail("override fun openBody(): Body", SymbolKind::METHOD).as_deref(),
        Some("Body"),
        "a modifier (override/public/private/…) before `fun` must not break \
         the `fun`-keyword disambiguation against the Java shape"
    );
}

#[test]
fn declared_type_from_detail_extracts_java_method_return_type() {
    // Real shape confirmed via a throwaway `parse_java` probe: no trailing
    // `{` (detail is already body-truncated) and the return type comes
    // BEFORE the method name, unlike Kotlin.
    assert_eq!(
        declared_type_from_detail("public Body getBody()", SymbolKind::METHOD).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_unwraps_generic_return_type() {
    assert_eq!(
        declared_type_from_detail("fun openBodies(): List<Body>", SymbolKind::FUNCTION).as_deref(),
        Some("List<Body>")
    );
}

#[test]
fn declared_type_from_detail_unwraps_java_generic_with_internal_space() {
    // `Map<String, Object>` contains a space (after the comma) that must NOT
    // be treated as the boundary before the method name.
    assert_eq!(
        declared_type_from_detail(
            "@Nullable public Map<String, Object> getMap()",
            SymbolKind::METHOD
        )
        .as_deref(),
        Some("Map<String, Object>")
    );
}

#[test]
fn declared_type_from_detail_none_for_unit_function() {
    assert_eq!(
        declared_type_from_detail("fun consume(body: Body)", SymbolKind::FUNCTION),
        None,
        "no `: Type` suffix at all (Unit-returning) must not be treated as a \
         producer declaration"
    );
}

#[test]
fn declared_type_from_detail_none_for_inferred_local_val() {
    assert_eq!(
        declared_type_from_detail("val body = Body()", SymbolKind::PROPERTY),
        None,
        "a local `val` with only an initializer expression (no explicit type \
         annotation) must not be treated as a producer declaration"
    );
}

#[test]
fn declared_type_from_detail_none_for_non_declaration_kind() {
    assert_eq!(
        declared_type_from_detail("class Foo", SymbolKind::CLASS),
        None,
        "a class/constructor/field detail is never a producer declaration \
         shape, gated via SymbolKind rather than sniffing the string"
    );
}

#[test]
fn declared_type_from_detail_extracts_multiline_function_return_type() {
    // The return type sits on a line AFTER the closing paren — only possible
    // to detect once `detail` is already the CST-joined single-line text
    // (multi-line raw source, single-line scanning, would miss this).
    assert_eq!(
        declared_type_from_detail("fun openBody( x: Int ): Body", SymbolKind::FUNCTION).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_extension_receiver_function_return_type() {
    assert_eq!(
        declared_type_from_detail("fun Foo.openBody(): Body", SymbolKind::FUNCTION).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_leading_type_param_function_return_type() {
    assert_eq!(
        declared_type_from_detail("fun <T> openBody(): Body", SymbolKind::FUNCTION).as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_kotlin_return_type_past_a_parenthesized_annotation() {
    // A leading annotation with its own argument list (`@Named("body")`) must
    // not make the annotation's `(` the one this function anchors on — it
    // would land on `"body"`'s closing `)` instead of the parameter list's,
    // and never find the `: Body` suffix.
    assert_eq!(
        declared_type_from_detail(
            "@Named(\"body\") fun provideBody(): Body",
            SymbolKind::FUNCTION
        )
        .as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_java_return_type_past_a_parenthesized_annotation() {
    assert_eq!(
        declared_type_from_detail(
            "@Named(\"body\") public Body provideBody()",
            SymbolKind::METHOD
        )
        .as_deref(),
        Some("Body")
    );
}

#[test]
fn declared_type_from_detail_extracts_kotlin_return_type_past_a_paren_in_a_default_value() {
    // A `)` inside a string default value (`separator: String = ")"`) would
    // defeat a forward `(`-then-matching-`)` scan — this is exactly why
    // `declared_type_from_detail` delegates to the shared resolver parser
    // (`extract_return_type_from_detail`) instead of hand-rolling a second
    // one: an earlier from-scratch version of this function had this bug,
    // found only by differential-testing it against the resolver's existing
    // parser, which never had it (it scans backward for a `):` pattern,
    // retrying past any `)` that isn't followed by `:`).
    assert_eq!(
        declared_type_from_detail(
            "fun split(separator: String = \")\"): Body",
            SymbolKind::FUNCTION
        )
        .as_deref(),
        Some("Body")
    );
}

// ─── declared_type_from_raw_lines ──────────────────────────────────────────────
//
// 2026-09-22b fix round: `SymbolEntry::detail` is capped at `MAX_DETAIL_CHARS`
// (120 chars, `src/parser.rs`). A Dagger/Hilt-shaped producer with a long
// enough parameter list can exceed that before its `": Type"` return-type
// suffix, so `declared_type_from_detail` alone silently loses the type this
// whole feature depends on — this is the fallback: re-derive the type from
// the symbol's own raw source lines instead. Uses the REAL parser (not a
// hand-written truncated string) so the fixture genuinely exercises
// `parser.rs`'s truncation, not an assumption about its exact shape.

#[test]
fn declared_type_from_raw_lines_recovers_truncated_producer_return_type() {
    let src = "fun provideNetworkRepository(context: ApplicationContext, \
               apiClient: RetrofitApiClient, cache: DiskLruCache, logger: EventLogger): \
               NetworkRepository";
    assert!(
        src.chars().count() > 120,
        "fixture must actually exceed MAX_DETAIL_CHARS to exercise truncation"
    );

    let data = crate::parser::parse_kotlin(src);
    let symbol = data
        .symbols
        .iter()
        .find(|s| s.name == "provideNetworkRepository")
        .expect("provideNetworkRepository must be indexed");

    assert!(
        symbol.detail.ends_with('…'),
        "sanity check: the fixture must actually get truncated by the parser; \
         detail={:?}",
        symbol.detail
    );
    assert_eq!(
        declared_type_from_detail(&symbol.detail, symbol.kind),
        None,
        "sanity check (red before the fix): the truncated detail alone must \
         NOT recover the return type — this is exactly the gap the fallback \
         exists for; got detail={:?}",
        symbol.detail
    );

    let recovered = declared_type_from_raw_lines(&data.lines, symbol.range, symbol.kind);
    assert_eq!(
        recovered.as_deref(),
        Some("NetworkRepository"),
        "the fallback must recover the return type from raw source lines \
         when `detail` was truncated"
    );
}

// ─── type_annotation_matches_owner ─────────────────────────────────────────────

#[test]
fn type_annotation_matches_owner_accepts_nested_type_shape() {
    use crate::rg::type_annotation_matches_owner;
    assert!(
        type_annotation_matches_owner("Reducer.Factory", "Reducer"),
        "a member declared to return `Owner.Nested` explicitly mentions `Owner` \
         as its first segment and must count as a producer of `Owner`"
    );
}

#[test]
fn type_annotation_matches_owner_accepts_fully_qualified_shape() {
    use crate::rg::type_annotation_matches_owner;
    assert!(
        type_annotation_matches_owner("a.Body", "Body"),
        "a member declared to return a fully-qualified `pkg.Owner` explicitly \
         mentions `Owner` as its last segment and must count as a producer of `Owner`"
    );
}

#[test]
fn type_annotation_matches_owner_rejects_unrelated_dotted_type() {
    use crate::rg::type_annotation_matches_owner;
    assert!(
        !type_annotation_matches_owner("a.Other", "Body"),
        "a dotted type whose segments do not include `Body` at all must not match"
    );
}

// ─── producer-scoped hop 2 gating (uppercase nested types) ────────────────────

/// Regression for the hop-2 "producer" widening in `parent_scoped_reference_locations`:
/// it must not run for uppercase nested-type searches. An uppercase nested type can
/// never be referenced bare without an explicit import (same reasoning as the
/// same-package exclusion), so widening the bare-name candidate set for one can only
/// ever surface an unrelated textual coincidence in a hop-2-discovered file, never a
/// real reference.
///
/// `IntroContract` self-produces via its own `create(): IntroContract` companion
/// factory method — mirroring the exact shape that made the analogous owner-scoped
/// fix's test fixture accidentally self-producing. `Caller.kt` never imports or
/// mentions `IntroContract`/`Event` as code, calls the generically-named `create()`
/// producer of an unrelated type, and only contains the word `Event` inside a comment.
/// Without the gate, hop 2 would still add `Caller.kt` as a bare-name candidate (the
/// producer name `create` is searched project-wide) and its comment's bare `Event`
/// would leak through — there's no dot-qualifier for `has_wrong_qualifier_at_col` to
/// reject.
#[test]
fn parent_scoped_reference_locations_does_not_widen_via_hop_two_for_uppercase_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let contract = "package com.example.intro\ninterface IntroContract {\n    \
                     sealed class Event\n    companion object {\n        \
                     fun create(): IntroContract = TODO()\n    }\n}\n";
    let caller = "package com.other\n\nfun use() {\n    \
                   val instance = SomeUnrelatedFactory().create()\n    \
                   // Event: coincidental word, no relation to IntroContract.\n}\n";

    let contract_path = write_temp(root, "IntroContract.kt", contract);
    write_temp(root, "Caller.kt", caller);

    let contract_uri = Url::from_file_path(&contract_path).unwrap();
    let decl_files = vec![contract_path.clone()];

    let request = RgSearchRequest::new(
        "Event",
        Some("IntroContract"),
        Some("com.example.intro"),
        Some(root),
        false,
        &contract_uri,
        &decl_files,
    );

    let locs = rg_find_references(&request, None);
    let paths: Vec<String> = locs
        .iter()
        .filter_map(|l| {
            l.uri
                .to_file_path()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .collect();

    assert!(
        !paths.iter().any(|p| p == "Caller.kt"),
        "a file reached only through hop 2's producer widening must not surface a \
         coincidental bare-word match for an uppercase nested type; got: {paths:?}"
    );
}

/// Regression for the hop-2 file-discovery call in `producer_scoped_candidate_files`:
/// it must issue a single scoped `rg` call over an alternation of every producer
/// name, not one call per name — and every name's callers must still be found.
#[test]
fn producer_scoped_candidate_files_finds_callers_of_every_producer_name() {
    use crate::rg::{producer_scoped_candidate_files, ProducerExpansion};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let hop1_src = "package a\n\nclass Hop1 {\n    fun produceOne(): Owner = TODO()\n    \
                     fun produceTwo(): Owner = TODO()\n}\n";
    let caller_one_src = "package b\n\nfun use(hop1: Hop1) {\n    hop1.produceOne()\n}\n";
    let caller_two_src = "package b\n\nfun use(hop1: Hop1) {\n    hop1.produceTwo()\n}\n";

    let hop1_path = write_temp(root, "Hop1.kt", hop1_src);
    write_temp(root, "CallerOne.kt", caller_one_src);
    write_temp(root, "CallerTwo.kt", caller_two_src);

    let dummy_uri = Url::from_file_path(&hop1_path).unwrap();
    let decl_files: Vec<String> = vec![];
    // `producer_scoped_candidate_files` no longer reads `hop1_files` off disk —
    // it intersects `hop1_files` against pre-computed `(file_uri, member_name)`
    // producer candidates (see `RgSearchRequest::producer_candidates`), which
    // in production are built from the `Indexer` before the callers-search rg
    // pass this test exercises. Supply them directly here.
    let hop1_uri = dummy_uri.to_string();
    let request = RgSearchRequest::new(
        "produceOne",
        None,
        None,
        Some(root),
        false,
        &dummy_uri,
        &decl_files,
    )
    .with_producer_candidates(vec![
        ProducerCandidate {
            file_uri: hop1_uri.clone(),
            member_name: "produceOne".to_string(),
        },
        ProducerCandidate {
            file_uri: hop1_uri,
            member_name: "produceTwo".to_string(),
        },
    ]);

    let hop1_files = vec![hop1_path];
    let result = producer_scoped_candidate_files(&request, None, &hop1_files);

    let ProducerExpansion::Found(files) = result else {
        panic!(
            "expected ProducerExpansion::Found with two producer names, got a different variant"
        );
    };
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        names.contains(&"CallerOne.kt".to_string()),
        "caller of the first producer name must be found; got: {names:?}"
    );
    assert!(
        names.contains(&"CallerTwo.kt".to_string()),
        "caller of the second producer name must be found; got: {names:?}"
    );
}

/// 2026-09-22b fix round (finding 4, from PR #324's own review):
/// `MAX_PRODUCER_CANDIDATE_FILES` must be compared against the count of
/// NEWLY discovered files (filtered, minus hop-1 overlap), not the raw `rg`
/// match count — a file hop 1 already found isn't new widening, so counting
/// it toward the cap could disable widening entirely even when the real
/// new-and-usable file count is comfortably under the cap.
///
/// 260 files match the producer name (`Hop1.kt`, which declares it, plus 259
/// callers) — over the 256 cap on raw count alone. 10 of those (`Hop1.kt` +
/// 9 callers) are already counted as hop 1. The correctly-computed newly
/// discovered count is 260 - 10 = 250, under the cap — so this must return
/// `Found`, not `SkippedTooBroad`.
#[test]
fn producer_scoped_candidate_files_caps_only_the_newly_discovered_count() {
    use crate::rg::{producer_scoped_candidate_files, ProducerExpansion};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let hop1_src = "package a\n\nclass Hop1 {\n    fun produce(): Owner = TODO()\n}\n";
    let hop1_path = write_temp(root, "Hop1.kt", hop1_src);

    const TOTAL_CALLERS: usize = 259;
    const HOP1_OVERLAP_CALLERS: usize = 9;
    let mut hop1_files: Vec<String> = vec![hop1_path.clone()];
    for i in 0..TOTAL_CALLERS {
        let src = format!("package b\n\nfun use{i}(hop1: Hop1) {{\n    hop1.produce()\n}}\n");
        let path = write_temp(root, &format!("Caller{i}.kt"), &src);
        if i < HOP1_OVERLAP_CALLERS {
            hop1_files.push(path);
        }
    }
    // 260 total files match `\bproduce\b` (Hop1.kt + 259 callers) — over 256 —
    // but only 250 are newly discovered (minus the 10-file hop-1 overlap
    // built above) — under 256.
    assert_eq!(
        hop1_files.len(),
        HOP1_OVERLAP_CALLERS + 1,
        "sanity check on the fixture's own hop-1 overlap count"
    );

    let dummy_uri = Url::from_file_path(&hop1_path).unwrap();
    let decl_files: Vec<String> = vec![];
    let hop1_uri = dummy_uri.to_string();
    let request = RgSearchRequest::new(
        "produce",
        None,
        None,
        Some(root),
        false,
        &dummy_uri,
        &decl_files,
    )
    .with_producer_candidates(vec![ProducerCandidate {
        file_uri: hop1_uri,
        member_name: "produce".to_string(),
    }]);

    let result = producer_scoped_candidate_files(&request, None, &hop1_files);

    assert!(
        matches!(result, ProducerExpansion::Found(_)),
        "expected Found (newly-discovered count is under the cap), got a \
         different variant — the cap must be checked AFTER filtering and \
         subtracting hop-1 overlap, not on the raw rg match count: {:?}",
        match result {
            ProducerExpansion::NoProducerFound => "NoProducerFound",
            ProducerExpansion::SkippedTooBroad => "SkippedTooBroad",
            ProducerExpansion::Found(_) => "Found",
        }
    );
}

// ─── file_uri_under_source_paths ───────────────────────────────────────────────
//
// 2026-09-22b fix round: keeps `rg_locations`'s pre-`spawn_blocking` producer
// scan (see `references.rs`) scoped to configured source roots, the same way
// every other rg pass in this module already is.

#[test]
fn file_uri_under_source_paths_empty_scope_keeps_everything() {
    let uri = Url::from_file_path("/workspace/module/Foo.kt")
        .unwrap()
        .to_string();
    assert!(
        file_uri_under_source_paths(&uri, &[], None),
        "no configured source paths means no additional scoping"
    );
}

#[test]
fn file_uri_under_source_paths_accepts_file_under_an_absolute_source_root() {
    let uri = Url::from_file_path("/workspace/app/src/Foo.kt")
        .unwrap()
        .to_string();
    let source_paths = vec!["/workspace/app/src".to_string()];
    assert!(file_uri_under_source_paths(&uri, &source_paths, None));
}

#[test]
fn file_uri_under_source_paths_rejects_file_outside_every_source_root() {
    let uri = Url::from_file_path("/workspace/other-module/Foo.kt")
        .unwrap()
        .to_string();
    let source_paths = vec!["/workspace/app/src".to_string()];
    assert!(
        !file_uri_under_source_paths(&uri, &source_paths, None),
        "a file outside every configured source root must not pass"
    );
}

#[test]
fn file_uri_under_source_paths_resolves_relative_entries_against_workspace_root() {
    let uri = Url::from_file_path("/workspace/app/src/Foo.kt")
        .unwrap()
        .to_string();
    let source_paths = vec!["app/src".to_string()];
    assert!(
        file_uri_under_source_paths(
            &uri,
            &source_paths,
            Some(std::path::Path::new("/workspace"))
        ),
        "a relative source path must resolve against workspace_root, matching \
         RgTarget::SourcePaths's own resolution idiom"
    );
}

// ─── is_unusable_producer_name ─────────────────────────────────────────────────
//
// Real-world regression: a data class's synthesized `copy(): Self` was being
// treated as a producer-discovery candidate. `copy` as a bare-word `rg`
// pattern matches over a thousand files in a real ~18k-file Android
// monorepo, which alone exceeded `MAX_PRODUCER_CANDIDATE_FILES` and
// discarded hop 2's ENTIRE merged alternation search — silently reverting
// the whole feature (see `field_reference_found_through_inferred_receiver_type`'s
// sibling integration test below for the end-to-end proof) for exactly the
// data-class-field shape the reported bug was about.
//
// Widened (same day, same real corpus) after review found the identical
// mechanism unaddressed for enum-synthesized `values`/`valueOf`/`entries`
// (measured: 557 files, 2.2x the cap, silently discarding hop 2 for every
// field on any of the corpus's enum classes) and for a hand-written `copy`
// class *method* (`SymbolKind::METHOD`, not `FUNCTION` — nesting demotes the
// kind, so the original kind-gated exclusion missed it; a real instance of
// exactly this shape exists in the same corpus this was measured against).
// The exclusion is now name-only, regardless of kind, on the same reasoning
// that already justified excluding `copy`: a name this common as a bare-word
// rg pattern is an unusable discovery signal no matter who wrote it.

#[test]
fn is_unusable_producer_name_detects_the_synthesized_data_class_copy() {
    assert!(is_unusable_producer_name("copy"));
}

#[test]
fn is_unusable_producer_name_detects_the_synthesized_enum_members() {
    assert!(is_unusable_producer_name("values"));
    assert!(is_unusable_producer_name("valueOf"));
    assert!(is_unusable_producer_name("entries"));
}

#[test]
fn is_unusable_producer_name_does_not_match_a_differently_named_producer() {
    assert!(!is_unusable_producer_name("openBody"));
}

// ─── resolve_effective_source_paths ────────────────────────────────────────────
//
// Real-world regression (GitHub Copilot review, PR #324): `build_command`
// falls back to the whole workspace root when every configured `sourceRoots`
// entry is missing/stale, so `rg` itself never returns zero results in that
// scenario — but `file_uri_under_source_paths` had no equivalent fallback,
// silently disabling the producer precompute (and therefore hop 2) whenever
// `sourceRoots` pointed at directories that don't actually exist.

#[test]
fn resolve_effective_source_paths_falls_back_to_empty_when_every_path_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let source_paths = vec!["app/src".to_string(), "lib/src".to_string()];
    let effective = crate::rg::resolve_effective_source_paths(&source_paths, Some(root));
    assert!(
        effective.is_empty(),
        "every configured source path is missing on disk, so the effective \
         list must be empty — matching build_command's own \
         all-paths-missing -> whole-workspace-root fallback"
    );
}

#[test]
fn resolve_effective_source_paths_keeps_paths_when_at_least_one_exists() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    let source_paths = vec!["app/src".to_string(), "lib/src".to_string()];
    let effective = crate::rg::resolve_effective_source_paths(&source_paths, Some(root));
    assert_eq!(
        effective, source_paths,
        "at least one configured source path exists, so the original list is \
         kept unchanged (including the missing one — matching rg's own \
         per-path `is_dir()` skip, not an all-or-nothing decision at this level)"
    );
}
