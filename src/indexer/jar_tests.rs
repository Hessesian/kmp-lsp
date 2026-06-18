//! JAR indexing integration tests.
//!
//! Simulate the full JAR indexing pipeline without a real sidecar:
//! create an Indexer, inject sidecar symbols via `populate_from_symbols`,
//! then verify that resolution, completion, and file_data lookups find them.

use std::io::Write;

use tower_lsp::lsp_types::Url;
use zip::write::SimpleFileOptions;

use crate::indexer::jar::populate_from_symbols;
use crate::sidecar::SidecarSymbol;
use crate::types::SourceSet;

// ── helpers ───────────────────────────────────────────────────────────────────

fn jar_uri(path: &str) -> Url {
    Url::parse(&format!("jar:file://{}", path)).unwrap()
}

fn make_sidecar_symbol(name: &str, kind: &str, detail: &str, container: &str) -> SidecarSymbol {
    SidecarSymbol {
        name: name.to_owned(),
        kind: kind.to_owned(),
        container: container.to_owned(),
        detail: detail.to_owned(),
        doc: String::new(),
        type_params: Vec::new(),
        extension_receiver_type: String::new(),
        trailing_lambda: false,
        deprecated: false,
        pkg: String::new(),
        top_level: container.is_empty(),
    }
}

fn make_sidecar_extension(name: &str, receiver_type: &str, detail: &str) -> SidecarSymbol {
    SidecarSymbol {
        name: name.to_owned(),
        kind: "fun".to_owned(),
        container: String::new(),
        detail: detail.to_owned(),
        doc: String::new(),
        type_params: Vec::new(),
        extension_receiver_type: receiver_type.to_owned(),
        trailing_lambda: false,
        deprecated: false,
        pkg: String::new(),
        top_level: true,
    }
}

fn idx() -> crate::indexer::Indexer {
    crate::indexer::Indexer::new()
}

// ============================================================================
// Compiled JAR tests (sidecar path via populate_from_symbols)
// ============================================================================

#[test]
fn jar_symbol_resolves_via_lookup() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![
        make_sidecar_symbol(
            "launch",
            "fun",
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
            "CoroutineScopeKt",
        ),
        make_sidecar_symbol(
            "async",
            "fun",
            "fun CoroutineScope.async(block: suspend () -> Unit): Deferred<T>",
            "CoroutineScopeKt",
        ),
        make_sidecar_symbol("Job", "interface", "interface kotlinx.coroutines.Job", ""),
    ];

    let count = populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);
    assert_eq!(count, 3, "should have indexed 3 symbols");

    let launch_locs = indexer.lookup_definitions("launch");
    assert_eq!(
        launch_locs.len(),
        1,
        "launch should be found in JAR definitions"
    );
    assert!(
        launch_locs[0].uri.as_str().starts_with("jar:file://"),
        "launch location should be a JAR URI"
    );

    let job_locs = indexer.lookup_definitions("Job");
    assert_eq!(job_locs.len(), 1, "Job should be found");
}

#[test]
fn jar_symbol_resolves_via_import() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![make_sidecar_symbol(
        "Job",
        "interface",
        "interface kotlinx.coroutines.Job",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let user_uri = Url::parse("file:///src/main/Main.kt").unwrap();
    let user_source =
        "package com.example.main\n\nimport kotlinx.coroutines.Job\n\nfun example(): Job = TODO()";
    indexer.index_content(&user_uri, user_source);

    let locs = crate::resolver::resolve_symbol(&indexer, "Job", None, &user_uri);
    assert!(
        !locs.is_empty(),
        "Job should resolve from user file via import; got 0 locations"
    );
}

#[test]
fn jar_file_data_accessible_via_file_data_for() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/retrofit-2.9.0.jar";

    let symbols = vec![
        make_sidecar_symbol("Retrofit", "class", "class retrofit2.Retrofit", ""),
        make_sidecar_symbol(
            "Builder",
            "class",
            "class retrofit2.Retrofit.Builder",
            "Retrofit",
        ),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let jar_uri_str = format!("jar:file://{}", jar_path);
    let file_data = indexer.file_data_for(&jar_uri_str);
    assert!(
        file_data.is_some(),
        "jar_files should be accessible via file_data_for"
    );

    let fd = file_data.unwrap();
    assert_eq!(fd.symbols.len(), 2);
    assert_eq!(fd.package.as_deref(), Some("retrofit2"));
}

#[test]
fn jar_extension_visible_in_dot_completion() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![
        make_sidecar_extension(
            "launch",
            "CoroutineScope",
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
        ),
        make_sidecar_extension(
            "async",
            "CoroutineScope",
            "fun CoroutineScope.async(block: suspend () -> Unit): Deferred<T>",
        ),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let entries = indexer.extension_by_receiver.get("CoroutineScope");
    assert!(
        entries.is_some(),
        "CoroutineScope should be in extension_by_receiver"
    );
    let entries = entries.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"launch"),
        "launch should be in extension entries"
    );
    assert!(
        names.contains(&"async"),
        "async should be in extension entries"
    );
}

#[test]
fn jar_symbols_survive_reset_index_state() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![make_sidecar_symbol(
        "Job",
        "interface",
        "interface kotlinx.coroutines.Job",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    assert_eq!(indexer.lookup_definitions("Job").len(), 1);

    indexer.reset_index_state();

    let jar_locs = indexer.lookup_definitions("Job");
    assert_eq!(
        jar_locs.len(),
        1,
        "JAR symbols should survive reset_index_state"
    );
}

#[test]
fn clear_jar_index_removes_jar_symbols() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![make_sidecar_symbol(
        "Job",
        "interface",
        "interface kotlinx.coroutines.Job",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);
    assert_eq!(indexer.lookup_definitions("Job").len(), 1);

    indexer.clear_jar_index();

    assert!(
        indexer.lookup_definitions("Job").is_empty(),
        "JAR symbols should be gone after clear_jar_index"
    );
}

#[test]
fn jar_reindex_no_duplicates() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![make_sidecar_symbol(
        "Job",
        "interface",
        "interface kotlinx.coroutines.Job",
        "",
    )];

    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let locs = indexer.lookup_definitions("Job");
    assert_eq!(
        locs.len(),
        1,
        "re-indexing same JAR should not produce duplicate definitions"
    );
}

#[test]
fn jar_qualified_name_in_qualified_index() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/retrofit-2.9.0.jar";

    let symbols = vec![make_sidecar_symbol(
        "Retrofit",
        "class",
        "class retrofit2.Retrofit",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let loc = indexer.qualified.get("retrofit2.Retrofit");
    assert!(
        loc.is_some(),
        "retrofit2.Retrofit should be in qualified index"
    );
}

#[test]
fn empty_sidecar_symbols_no_crash() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/empty-1.0.jar";

    let symbols: Vec<SidecarSymbol> = vec![];
    let count = populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);
    assert_eq!(count, 0, "empty symbols should produce 0 count");

    assert!(indexer.jar_files.is_empty());
    assert!(indexer.jar_definitions.is_empty());
}

#[test]
fn jar_symbol_in_no_rg_resolution() {
    let indexer = idx();
    populate_from_symbols(
        &indexer,
        "/home/test/.gradle/caches/coroutines-core-1.7.3.jar".as_ref(),
        &[make_sidecar_symbol(
            "Job",
            "interface",
            "interface kotlinx.coroutines.Job",
            "",
        )],
    );

    let user_uri = Url::parse("file:///src/main/Main.kt").unwrap();
    indexer.index_content(&user_uri, "package com.example\nfun test() {}");

    let locs = crate::resolver::resolve_symbol_no_rg(&indexer, "Job", &user_uri);
    assert!(
        !locs.is_empty(),
        "JAR symbol Job should be found via no-rg fallback; got: {:?}",
        locs
    );
}

#[test]
fn multiple_jars_same_symbol_name() {
    let indexer = idx();

    let jar1 = "/home/test/.gradle/caches/lib-a-1.0.jar";
    let jar2 = "/home/test/.gradle/caches/lib-b-2.0.jar";

    populate_from_symbols(
        &indexer,
        jar1.as_ref(),
        &[make_sidecar_symbol(
            "Builder",
            "class",
            "class com.lib_a.Builder",
            "",
        )],
    );
    populate_from_symbols(
        &indexer,
        jar2.as_ref(),
        &[make_sidecar_symbol(
            "Builder",
            "class",
            "class com.lib_b.Builder",
            "",
        )],
    );

    let locs = indexer.lookup_definitions("Builder");
    assert_eq!(
        locs.len(),
        2,
        "two different JARs with same symbol name should produce 2 locations"
    );
}

#[test]
fn jar_uri_registered_as_library() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/retrofit-2.9.0.jar";

    populate_from_symbols(
        &indexer,
        jar_path.as_ref(),
        &[make_sidecar_symbol(
            "Retrofit",
            "class",
            "class retrofit2.Retrofit",
            "",
        )],
    );

    let jar_uri_str = format!("jar:file://{}", jar_path);
    assert!(
        indexer.is_library_uri(&Url::parse(&jar_uri_str).unwrap()),
        "JAR URI should be registered in library_uris"
    );
}

#[test]
fn ensure_file_data_returns_jar_file_data() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/retrofit-2.9.0.jar";

    populate_from_symbols(
        &indexer,
        jar_path.as_ref(),
        &[make_sidecar_symbol(
            "Retrofit",
            "class",
            "class retrofit2.Retrofit",
            "",
        )],
    );

    let jar_uri = jar_uri(jar_path);
    let file_data = crate::resolver::ensure_file_data(&indexer, &jar_uri);
    assert!(
        file_data.is_some(),
        "ensure_file_data should return JAR FileData"
    );
    let fd = file_data.unwrap();
    assert_eq!(fd.source_set, SourceSet::Library);
    assert_eq!(fd.symbols.len(), 1);
}

#[test]
fn jar_package_inferred_from_detail() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/compose-ui-1.5.0.jar";

    let symbols = vec![
        make_sidecar_symbol("Column", "fun", "fun Column(modifier: Modifier): Unit", ""),
        make_sidecar_symbol("Row", "fun", "fun Row(modifier: Modifier): Unit", ""),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let jar_uri_str = format!("jar:file://{}", jar_path);
    let fd = indexer.jar_files.get(&jar_uri_str).unwrap();

    assert!(
        fd.package.is_none(),
        "package should be None when detail has no dotted FQN; got: {:?}",
        fd.package
    );
}

#[test]
fn jar_package_inferred_from_class_symbol_only() {
    // Regression: package must come from class-like symbols, not functions whose
    // detail contains a member-access dot (e.g. "fun CoroutineScope.launch(...)").
    // Previously a function detail with a dot could poison package inference to
    // pick "CoroutineScope" as the package.
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![
        // Function detail has a dot that is NOT a package separator.
        make_sidecar_symbol(
            "launch",
            "fun",
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
            "CoroutineScopeKt",
        ),
        // Class detail has the real package.
        make_sidecar_symbol("Job", "interface", "interface kotlinx.coroutines.Job", ""),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let jar_uri_str = format!("jar:file://{}", jar_path);
    let fd = indexer.jar_files.get(&jar_uri_str).unwrap();
    assert_eq!(
        fd.package.as_deref(),
        Some("kotlinx.coroutines"),
        "package must be inferred from the class-like symbol, not the function"
    );

    // The qualified index must also reflect the correct package.
    let job_loc = indexer.qualified.get("kotlinx.coroutines.Job");
    assert!(
        job_loc.is_some(),
        "kotlinx.coroutines.Job must be in the qualified index"
    );
    // And the bad package candidate must NOT pollute the qualified index.
    let bad_loc = indexer.qualified.get("CoroutineScope.launch");
    assert!(
        bad_loc.is_none(),
        "CoroutineScope.launch must NOT be in the qualified index"
    );
}

#[test]
fn jar_package_none_when_only_function_details() {
    // If the JAR only exposes function symbols (no classes with FQN),
    // package inference should yield None — not a fake value from a function
    // detail that happens to contain dots.
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/anonymous-fn-only-1.0.jar";

    let symbols = vec![make_sidecar_symbol(
        "doWork",
        "fun",
        "fun Worker.doWork(): Result",
        "Worker",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let jar_uri_str = format!("jar:file://{}", jar_path);
    let fd = indexer.jar_files.get(&jar_uri_str).unwrap();
    assert!(
        fd.package.is_none(),
        "package must be None when only function/property details are present; got: {:?}",
        fd.package
    );
}

#[test]
fn jar_fqn_detail_populates_qualified_index() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/core-ktx-1.12.0.jar";

    let symbols = vec![make_sidecar_symbol(
        "ComponentActivity",
        "class",
        "class androidx.activity.ComponentActivity",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let loc = indexer.qualified.get("androidx.activity.ComponentActivity");
    assert!(
        loc.is_some(),
        "FQN from detail should be in qualified index"
    );
}

// ============================================================================
// End-to-end smoke tests (real I/O — kept small)
// ============================================================================

/// Create a sources JAR on disk in the standard Gradle cache layout
/// `caches/modules-2/files-2.1/group/artifact/version/hash/artifact-version-sources.jar`.
/// This is the I/O path the production `index_sources_jars` walks; the unit
/// tests above bypass it via `mock_jar_entries` for speed.
fn write_sources_jar(
    gradle_home: &std::path::Path,
    group: &str,
    artifact: &str,
    version: &str,
    sources: &[(&str, &str)],
) -> std::path::PathBuf {
    let jar_dir = gradle_home
        .join("caches")
        .join("modules-2")
        .join("files-2.1")
        .join(group)
        .join(artifact)
        .join(version)
        .join("abc123");
    std::fs::create_dir_all(&jar_dir).unwrap();

    let jar_path = jar_dir.join(format!("{artifact}-{version}-sources.jar"));
    let file = std::fs::File::create(&jar_path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (path, content) in sources {
        writer.start_file_from_path(path, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
    jar_path
}

#[test]
fn scan_gradle_jars_split_dedups_to_latest_version() {
    // Sets up a tempdir mimicking the Gradle module cache layout with two
    // versions of the same artifact (one compiled, one sources).  Asserts
    // that the scan keeps only the latest version of each.
    let tmpdir = tempfile::tempdir().unwrap();
    write_sources_jar(
        tmpdir.path(),
        "com.example",
        "lib-core",
        "1.0.0",
        &[("META-INF/foo", "ignored")],
    );
    write_sources_jar(
        tmpdir.path(),
        "com.example",
        "lib-core",
        "2.0.0",
        &[("META-INF/foo", "ignored")],
    );

    let (compiled, sources) = crate::indexer::jar::scan_gradle_jars_split(Some(tmpdir.path()));

    // Two sources JARs found total, but only the latest version survives.
    assert_eq!(
        sources.len(),
        1,
        "scan must dedup two versions of the same artifact to the latest"
    );
    assert!(
        sources[0].to_string_lossy().contains("2.0.0"),
        "latest version (2.0.0) should win, got: {}",
        sources[0].display()
    );
    assert!(
        compiled.is_empty(),
        "no compiled JARs were created in this test"
    );
}

#[test]
fn index_sources_jars_end_to_end_with_real_jar() {
    // End-to-end smoke: walk the real Gradle cache + extract + parse + insert.
    // One small JAR, one .kt file.  Exercises the slow I/O path that
    // `index_jar_entries` bypasses.
    let tmpdir = tempfile::tempdir().unwrap();
    write_sources_jar(
        tmpdir.path(),
        "com.example",
        "e2e",
        "1.0.0",
        &[(
            "com/example/e2e/Core.kt",
            "package com.example.e2e\n\nclass Core\n",
        )],
    );

    let indexer = idx();
    let cache_dir = tempfile::tempdir().unwrap();
    let total = crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(tmpdir.path()),
        Some(cache_dir.path()),
    );
    assert!(total > 0, "end-to-end index should parse the JAR");

    let core_locs = indexer.definitions.get("Core");
    assert!(
        core_locs.is_some(),
        "Core class from real JAR should be in definitions"
    );
}

// ============================================================================
// Mocked-entry unit tests (fast — no I/O, no zip crate)
// ============================================================================

/// Build a `Vec<(Url, String)>` representing sources-JAR entries — used to
/// exercise `index_jar_entries` without filesystem I/O.  Tests that need the
/// real Gradle cache walk + ZIP extraction should call `index_sources_jars`
/// directly (see the integration test at the bottom of this file).
fn mock_jar_entries(jar_path: &str, files: &[(&str, &str)]) -> Vec<(Url, String)> {
    let jar_uri = format!("jar:file://{}", jar_path);
    files
        .iter()
        .map(|(path, content)| {
            let entry_uri_str = format!("{}!/{}", jar_uri, path);
            let entry_uri = Url::parse(&entry_uri_str).expect("mock_jar_entries: valid URL");
            (entry_uri, content.to_string())
        })
        .collect()
}

#[test]
fn sources_jar_auto_mount_indexes_kotlin_files() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/lib-core-1.0.jar";
    let entries = mock_jar_entries(
        jar_path,
        &[
            (
                "com/example/lib/Core.kt",
                "package com.example.lib\n\nclass Core {\n    fun greet(): String = \"hello\"\n}\n",
            ),
            (
                "com/example/lib/Utils.kt",
                "package com.example.lib\n\nfun utility() {}\n",
            ),
        ],
    );

    let total = crate::indexer::jar::index_jar_entries(&indexer, entries);

    assert!(total > 0, "should have indexed some symbols");

    // Verify the symbols landed in the main definitions index (not jar_definitions).
    let core_locs = indexer.definitions.get("Core");
    assert!(
        core_locs.is_some(),
        "Core should be in main definitions index"
    );
    let core_locs_vec = core_locs.unwrap().clone();
    assert_eq!(core_locs_vec.len(), 1);

    let greet_locs = indexer.definitions.get("greet");
    assert!(
        greet_locs.is_some(),
        "greet function should be in main definitions index"
    );

    // Verify the URI scheme is jar:file://
    let core_loc = &core_locs_vec[0];
    assert!(
        core_loc.uri.as_str().starts_with("jar:file://"),
        "Core URI should be jar:file:// scheme; got: {}",
        core_loc.uri
    );
    // Verify it contains the !/ separator for the entry path.
    assert!(
        core_loc.uri.as_str().contains("!/"),
        "URI should contain !/ separator; got: {}",
        core_loc.uri
    );

    // Verify the file is a library file (not in jar_files).
    let uri_str = core_loc.uri.to_string();
    assert!(
        indexer.files.contains_key(&uri_str),
        "sources JAR entry should be in main files map"
    );
    assert!(
        !indexer.jar_files.contains_key(&uri_str),
        "sources JAR entry should NOT be in jar_files"
    );

    // Verify library_uris registration.
    assert!(
        indexer.is_library_uri(&core_loc.uri),
        "sources JAR entry URI should be in library_uris"
    );
}

#[test]
fn sources_jar_auto_mount_resolvable_via_import() {
    let indexer = idx();
    crate::indexer::jar::index_jar_entries(
        &indexer,
        mock_jar_entries(
            "/home/test/.gradle/caches/lib-core-1.0.jar",
            &[(
                "com/example/lib/Core.kt",
                "package com.example.lib\n\nclass Core {}\n",
            )],
        ),
    );

    // Write a user file that imports from the sources JAR.
    let user_uri = Url::parse("file:///src/main/Main.kt").unwrap();
    let user_source =
        "package com.example.main\n\nimport com.example.lib.Core\n\nfun example(): Core = Core()";
    indexer.index_content(&user_uri, user_source);

    let locs = crate::resolver::resolve_symbol(&indexer, "Core", None, &user_uri);
    assert!(
        !locs.is_empty(),
        "Core from sources JAR should resolve via import; got 0 locations"
    );

    // Verify the location points to the sources JAR entry.
    let loc = &locs[0];
    assert!(
        loc.uri.as_str().contains("!/com/example/lib/Core.kt"),
        "resolved location should point to the source file inside the JAR; got: {}",
        loc.uri
    );
}

#[test]
fn sources_jar_empty_entries_no_crash() {
    // No entries at all (the "empty JAR" case).  No .kt/.java files means
    // nothing to index.  We can express this directly with the mocked
    // helper without creating a real empty JAR.
    let indexer = idx();
    let total = crate::indexer::jar::index_jar_entries(&indexer, vec![]);
    assert_eq!(total, 0, "empty entries list should produce 0 symbols");
}

#[test]
fn sources_jar_dedup_latest_version() {
    // The dedup-to-latest-version happens in `scan_gradle_jars_split` at the
    // filesystem-discovery layer, not in the indexing layer.  This test
    // verifies that the indexing layer behaves correctly given the latest
    // version's entries (the older version's entries never reach it).
    //
    // For the scan-time dedup test, see `scan_gradle_jars_split` tests in
    // `indexer/scan_tests.rs` (or run `cargo test scan_gradle`).
    let indexer = idx();
    let total = crate::indexer::jar::index_jar_entries(
        &indexer,
        mock_jar_entries(
            "/home/test/.gradle/caches/lib-core-2.0.0-sources.jar",
            &[(
                "com/example/lib/New.kt",
                "package com.example.lib\n\nclass New {}\n",
            )],
        ),
    );
    assert!(total > 0);

    // Latest version is indexed.
    assert!(
        indexer.definitions.get("New").is_some(),
        "New class from v2.0.0 should be indexed"
    );
    // Older version never made it through scan, so it can't be in the index.
    assert!(
        indexer.definitions.get("Old").is_none(),
        "Old class from v1.0.0 should NOT be indexed (never reached this layer)"
    );
}

#[test]
fn sources_jar_extension_survives_reset_index_state() {
    // Regression F2: a sources-JAR extension entry must survive `reset_index_state`.
    //
    // Previously, `reset_index_state` checked `jar_files.contains_key(uri)` for
    // every `jar:file://` extension entry.  Sources-JAR entries live in the
    // main `files` map (not `jar_files`), so they were silently dropped, leaving
    // dot-completion for `viewModelScope.launch`-style extensions broken after
    // every workspace re-index.
    let indexer = idx();
    let total = crate::indexer::jar::index_jar_entries(
        &indexer,
        mock_jar_entries(
            "/home/test/.gradle/caches/lib-core-1.0.0-sources.jar",
            &[
                (
                    "com/example/lib/ViewModel.kt",
                    "package com.example.lib\n\nclass ViewModel\n",
                ),
                (
                    "com/example/lib/ViewModelExt.kt",
                    "package com.example.lib\n\nfun ViewModel.greet(): String = \"hi\"\n",
                ),
            ],
        ),
    );
    assert!(total > 0, "should have indexed some symbols");

    // Sanity check: extension entry for ViewModel exists with a jar:file://!/ URI.
    let entries = indexer
        .extension_by_receiver
        .get("ViewModel")
        .expect("ViewModel should have extension entries after index_sources_jars");
    let greet_entry = entries
        .iter()
        .find(|e| e.name == "greet")
        .expect("greet extension should be present");
    let greet_uri = greet_entry.file_uri.clone();
    assert!(
        greet_uri.starts_with("jar:file://") && greet_uri.contains("!/"),
        "greet entry URI should be a sources-JAR entry URI; got: {greet_uri}"
    );

    // The URI must live in the main `files` map (not `jar_files`) so that
    // `with_classified_source_set` + tree-sitter can re-derive metadata if needed.
    assert!(
        indexer.files.contains_key(&greet_uri),
        "sources-JAR entry URI must be in the main `files` map, not `jar_files`"
    );
    assert!(
        !indexer.jar_files.contains_key(&greet_uri),
        "sources-JAR entry URI must NOT be in `jar_files`"
    );

    // The actual reset: clear workspace-derived state.  JAR/sources entries
    // should survive (per the design intent: JAR symbols live in separate maps
    // and are only cleared on `clear_jar_index`).
    indexer.reset_index_state();

    let entries_after = indexer.extension_by_receiver.get("ViewModel");
    assert!(
        entries_after.is_some(),
        "sources-JAR extension entries must survive reset_index_state (F2 fix)"
    );
    let entries_after = entries_after.unwrap();
    let greet_entry = entries_after
        .iter()
        .find(|e| e.name == "greet")
        .expect("greet extension entry must still be present after reset");
    assert_eq!(
        greet_entry.file_uri, greet_uri,
        "greet entry URI must be unchanged after reset"
    );

    // The companion file_data must also be retained so the entry resolves
    // to real line numbers (not a synthetic 0,0).
    let fd = indexer
        .files
        .get(&greet_uri)
        .expect("sources-JAR FileData must still be in main `files` after reset");
    assert_eq!(
        fd.source_set,
        SourceSet::Library,
        "sources-JAR FileData must be classified as Library"
    );
    assert!(
        !fd.symbols.is_empty(),
        "sources-JAR FileData must still contain parsed symbols"
    );
}

#[test]
fn jar_pipeline_order_sources_wins_over_compiled() {
    // Regression F3: when both compiled-JAR and sources-JAR contribute the same
    // FQN to `qualified` / `extension_by_receiver`, the sources-JAR entry (real
    // line numbers from tree-sitter) must win over the compiled-JAR entry
    // (synthetic line indices from the sidecar).
    //
    // Simulates the order produced by `spawn_jar_indexing`: compiled-JAR first,
    // then sources-JAR.  Sources-JAR runs last and overwrites the synthetic
    // compiled-JAR entry with a real tree-sitter-parsed location.
    let indexer = idx();

    // Step 1: simulated compiled-JAR path.  In real code this is the sidecar
    // emitting a synthetic symbol with the same FQN.  We call
    // `populate_from_symbols` directly, which is the unit-level hook the
    // sidecar path uses to insert sidecar output.
    let compiled_jar_path = "/home/test/.gradle/caches/shared-compiled-1.0.jar";
    populate_from_symbols(
        &indexer,
        compiled_jar_path.as_ref(),
        &[make_sidecar_symbol(
            "Core",
            "class",
            "class com.example.shared.Core",
            "",
        )],
    );

    // Compiled path should have written a synthetic qualified entry.
    let compiled_uri = format!("jar:file://{}", compiled_jar_path);
    let compiled_loc = indexer
        .qualified
        .get("com.example.shared.Core")
        .expect("compiled-JAR should have populated qualified[com.example.shared.Core]")
        .clone();
    assert_eq!(
        compiled_loc.uri.as_str(),
        compiled_uri,
        "compiled-JAR qualified entry should use the compiled-JAR URI"
    );
    assert_eq!(
        compiled_loc.range.start.line, 0,
        "compiled entry has synthetic line 0"
    );

    // Step 2: sources-JAR path.  Real tree-sitter parse → real line numbers.
    crate::indexer::jar::index_jar_entries(
        &indexer,
        mock_jar_entries(
            "/home/test/.gradle/caches/shared-1.0.0-sources.jar",
            &[(
                "com/example/shared/Core.kt",
                "package com.example.shared\n\nclass Core {\n    fun hello(): String = \"hi\"\n}\n",
            )],
        ),
    );

    // Sources-JAR must have OVERWRITTEN the qualified entry.
    let sources_loc = indexer
        .qualified
        .get("com.example.shared.Core")
        .expect("qualified[com.example.shared.Core] must still exist after sources run")
        .clone();
    assert!(
        sources_loc.uri.as_str().contains("!/"),
        "qualified entry must now point to a sources-JAR entry URI (with !/); got: {}",
        sources_loc.uri
    );
    assert!(
        sources_loc.uri.as_str() != compiled_uri,
        "qualified entry must NOT still be the compiled-JAR URI"
    );
    // Real source has Core.kt starting at line 0 with `package`, then class on
    // line 2.  We don't assert exact line (tree-sitter indexing may vary) but
    // it must not be the synthetic "line 0, char 0..4" produced by the sidecar
    // adapter.
    assert_ne!(
        sources_loc.range.start.line, 0,
        "sources-JAR qualified entry must not have synthetic line 0"
    );
}

#[test]
fn sources_jar_reindex_no_duplicates() {
    // Regression F1: re-running `index_jar_entries` with the same set of
    // entries must not duplicate symbol entries.  Previously, the parallel
    // insert used `extend` on `definitions` / `packages` / `subtypes` /
    // `extension_by_receiver`, so a second run would double-count.
    let indexer = idx();
    let entries = mock_jar_entries(
        "/home/test/.gradle/caches/lib-core-1.0.0-sources.jar",
        &[
            (
                "com/example/lib/Core.kt",
                "package com.example.lib\n\nclass Core {\n    fun greet(): String = \"hi\"\n}\n",
            ),
            (
                "com/example/lib/Utils.kt",
                "package com.example.lib\n\nfun utility() {}\n",
            ),
        ],
    );

    // First run.
    let first = crate::indexer::jar::index_jar_entries(&indexer, entries.clone());
    assert!(first > 0, "first run should index some symbols");

    // Snapshot symbol counts after first run.
    let core_count_first = indexer
        .definitions
        .get("Core")
        .map(|v| v.len())
        .unwrap_or(0);
    let utils_pkg_count_first = indexer
        .packages
        .get("com.example.lib")
        .map(|v| v.len())
        .unwrap_or(0);
    let greet_count_first = indexer
        .definitions
        .get("greet")
        .map(|v| v.len())
        .unwrap_or(0);
    let utility_count_first = indexer
        .definitions
        .get("utility")
        .map(|v| v.len())
        .unwrap_or(0);

    assert_eq!(
        core_count_first, 1,
        "Core should have 1 location after first run"
    );
    assert_eq!(
        greet_count_first, 1,
        "greet should have 1 location after first run"
    );
    assert_eq!(
        utility_count_first, 1,
        "utility should have 1 location after first run"
    );
    assert_eq!(
        utils_pkg_count_first, 2,
        "com.example.lib package should have 2 URIs (Core.kt, Utils.kt)"
    );

    // Second run with the same entries — must not duplicate.
    let second = crate::indexer::jar::index_jar_entries(&indexer, entries);
    assert_eq!(
        second, first,
        "second run should re-index the same number of symbols"
    );

    let core_count_second = indexer
        .definitions
        .get("Core")
        .map(|v| v.len())
        .unwrap_or(0);
    let greet_count_second = indexer
        .definitions
        .get("greet")
        .map(|v| v.len())
        .unwrap_or(0);
    let utility_count_second = indexer
        .definitions
        .get("utility")
        .map(|v| v.len())
        .unwrap_or(0);
    let utils_pkg_count_second = indexer
        .packages
        .get("com.example.lib")
        .map(|v| v.len())
        .unwrap_or(0);

    assert_eq!(
        core_count_second, 1,
        "Core must still have exactly 1 location after re-run (was {core_count_first} before)"
    );
    assert_eq!(
        greet_count_second, 1,
        "greet must still have exactly 1 location after re-run"
    );
    assert_eq!(
        utility_count_second, 1,
        "utility must still have exactly 1 location after re-run"
    );
    assert_eq!(
        utils_pkg_count_second, 2,
        "com.example.lib package must still have 2 URIs after re-run"
    );
}

// ============================================================================
// Regression: package inference + import resolution
// ============================================================================

#[test]
fn jar_symbol_resolves_via_import_without_fqn_detail() {
    // Simulates a real JAR where sidecar details lack dotted FQNs.
    // Package inference should still work from other symbols.
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![
        make_sidecar_symbol(
            "launch",
            "fun",
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
            "CoroutineScopeKt",
        ),
        make_sidecar_symbol("Job", "interface", "interface kotlinx.coroutines.Job", ""),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let user_uri = Url::parse("file:///src/main/Main.kt").unwrap();
    let user_source =
        "package com.example.main\n\nimport kotlinx.coroutines.Job\n\nfun example(): Job = TODO()";
    indexer.index_content(&user_uri, user_source);

    let locs = crate::resolver::resolve_symbol(&indexer, "Job", None, &user_uri);
    assert!(
        !locs.is_empty(),
        "Job should resolve via import even when other symbols lack FQN details; got 0 locations"
    );
}

#[test]
fn jar_symbol_resolves_via_import_with_no_fqn_at_all() {
    // Worst case: NO symbol has a dotted FQN in detail.
    // Package will be None, but import resolution should still work.
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/mystery-lib-1.0.jar";

    let symbols = vec![make_sidecar_symbol(
        "doThing",
        "fun",
        "fun doThing(): Unit",
        "",
    )];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let user_uri = Url::parse("file:///src/main/Main.kt").unwrap();
    let user_source =
        "package com.example.main\n\nimport mystery_lib.doThing\n\nfun example() = doThing()";
    indexer.index_content(&user_uri, user_source);

    let locs = crate::resolver::resolve_symbol(&indexer, "doThing", None, &user_uri);
    assert!(
        !locs.is_empty(),
        "doThing should resolve via import even with no FQN in detail; got 0 locations"
    );
}

#[test]
fn jar_nested_class_fqn_in_qualified_index() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/retrofit-2.9.0.jar";

    let symbols = vec![
        make_sidecar_symbol("Retrofit", "class", "class retrofit2.Retrofit", ""),
        make_sidecar_symbol(
            "Builder",
            "class",
            "class retrofit2.Retrofit.Builder",
            "Retrofit",
        ),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let loc = indexer.qualified.get("retrofit2.Retrofit.Builder");
    assert!(
        loc.is_some(),
        "retrofit2.Retrofit.Builder should be in qualified index"
    );

    let wrong_loc = indexer.qualified.get("retrofit2.Builder");
    assert!(
        wrong_loc.is_none(),
        "retrofit2.Builder (without container) should NOT be in qualified index"
    );
}

#[test]
fn jar_extension_has_package_for_same_package_resolution() {
    let indexer = idx();
    let jar_path = "/home/test/.gradle/caches/coroutines-core-1.7.3.jar";

    let symbols = vec![
        make_sidecar_extension(
            "launch",
            "CoroutineScope",
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
        ),
        make_sidecar_symbol("Job", "interface", "interface kotlinx.coroutines.Job", ""),
    ];
    populate_from_symbols(&indexer, jar_path.as_ref(), &symbols);

    let entries = indexer.extension_by_receiver.get("CoroutineScope");
    assert!(entries.is_some(), "CoroutineScope should have extensions");
    let entries = entries.unwrap();
    assert!(!entries.is_empty());
    assert!(
        entries[0].package.is_some(),
        "extension entry should have package set for same-package resolution"
    );
}

// ============================================================================
// Sources-JAR parse cache integration
// ============================================================================

/// First run writes the cache file; the entry fingerprint matches the JAR.
#[test]
fn index_sources_jars_writes_parse_cache() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let jar_path = write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "cached",
        "1.0.0",
        &[(
            "com/example/cached/CachedCore.kt",
            "package com.example.cached\n\nclass CachedCore\n",
        )],
    );

    let indexer = idx();
    let total = crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );
    assert!(total > 0, "first run should parse and index");

    let cache = crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    let entry = cache
        .get(jar_path.to_string_lossy().as_ref())
        .expect("cache entry written for the JAR");
    assert!(!entry.files.is_empty(), "entry holds parsed files");
    assert_eq!(
        entry.files[0].file_data.source_set,
        crate::types::SourceSet::Library,
        "cached file data is pre-marked Library"
    );
    let current =
        crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path).expect("fingerprint");
    assert!(
        crate::indexer::sources_jar_cache::entry_is_fresh(entry, &current),
        "entry fingerprint matches the JAR on disk"
    );
}

/// A fresh cache entry is served WITHOUT touching the JAR contents: the file
/// on disk is garbage (not a ZIP), so any symbols in the index must have come
/// from the cache.
#[test]
fn index_sources_jars_serves_fresh_entry_from_cache_without_extraction() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");

    let jar_dir = gradle_dir
        .path()
        .join("caches")
        .join("modules-2")
        .join("files-2.1")
        .join("com.example")
        .join("garbage")
        .join("1.0.0")
        .join("abc123");
    std::fs::create_dir_all(&jar_dir).expect("mkdir");
    let jar_path = jar_dir.join("garbage-1.0.0-sources.jar");
    std::fs::write(&jar_path, b"definitely not a zip archive").expect("write garbage jar");

    let uri_text = format!(
        "jar:file://{}!/com/example/garbage/FromCache.kt",
        jar_path.display()
    );
    let uri = tower_lsp::lsp_types::Url::parse(&uri_text).expect("uri");
    let mut parsed = crate::indexer::Indexer::parse_file(
        &uri,
        "package com.example.garbage\n\nclass FromCache\n",
    );
    parsed.data.source_set = crate::types::SourceSet::Library;
    let fingerprint =
        crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path).expect("fingerprint");
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: fingerprint.mtime_secs,
            mtime_nanos: fingerprint.mtime_nanos,
            file_size: fingerprint.file_size,
            files: vec![crate::indexer::sources_jar_cache::SourcesFileEntry {
                uri: uri_text.clone(),
                content_hash: parsed.content_hash,
                file_data: std::sync::Arc::new(parsed.data),
            }],
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    let total = crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    assert!(total > 0, "cache hit should contribute symbols");
    assert!(
        indexer.definitions.get("FromCache").is_some(),
        "FromCache must be resolvable purely from the cache (the JAR is garbage)"
    );
    assert!(
        indexer.library_uris.contains(&uri_text),
        "cache-hit files are registered as library URIs"
    );
}

/// A stale entry (fingerprint mismatch) is re-parsed from the real JAR and
/// the cache is updated with the new content.
#[test]
fn index_sources_jars_stale_entry_reparses_and_updates_cache() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let jar_path = write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "staleness",
        "1.0.0",
        &[(
            "com/example/staleness/NewSymbol.kt",
            "package com.example.staleness\n\nclass NewSymbol\n",
        )],
    );

    let stale_uri_text = format!(
        "jar:file://{}!/com/example/staleness/OldSymbol.kt",
        jar_path.display()
    );
    let stale_uri = tower_lsp::lsp_types::Url::parse(&stale_uri_text).expect("uri");
    let stale_parsed = crate::indexer::Indexer::parse_file(
        &stale_uri,
        "package com.example.staleness\n\nclass OldSymbol\n",
    );
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: 1,
            mtime_nanos: 2,
            file_size: 3,
            files: vec![crate::indexer::sources_jar_cache::SourcesFileEntry {
                uri: stale_uri_text,
                content_hash: stale_parsed.content_hash,
                file_data: std::sync::Arc::new(stale_parsed.data),
            }],
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    assert!(
        indexer.definitions.get("NewSymbol").is_some(),
        "stale entry must be re-parsed from the real JAR"
    );
    assert!(
        indexer.definitions.get("OldSymbol").is_none(),
        "stale cached symbols must not leak into the index"
    );

    let reloaded =
        crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    let entry = reloaded
        .get(jar_path.to_string_lossy().as_ref())
        .expect("cache entry refreshed");
    let current =
        crate::indexer::sources_jar_cache::jar_fingerprint(&jar_path).expect("fingerprint");
    assert!(
        crate::indexer::sources_jar_cache::entry_is_fresh(entry, &current),
        "refreshed entry matches the real JAR"
    );
}

/// Entries for JARs that no longer exist are pruned from the cache on save.
#[test]
fn index_sources_jars_prunes_deleted_jar_entries() {
    let gradle_dir = tempfile::tempdir().expect("gradle dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_sources_jar(
        gradle_dir.path(),
        "com.example",
        "alive",
        "1.0.0",
        &[(
            "com/example/alive/Alive.kt",
            "package com.example.alive\n\nclass Alive\n",
        )],
    );

    let mut entries = std::collections::HashMap::new();
    entries.insert(
        "/nonexistent/path/gone-0.1-sources.jar".to_owned(),
        crate::indexer::sources_jar_cache::SourcesJarEntry {
            mtime_secs: 1,
            mtime_nanos: 2,
            file_size: 3,
            files: Vec::new(),
        },
    );
    crate::indexer::sources_jar_cache::save_sources_jar_cache(Some(cache_dir.path()), &entries);

    let indexer = idx();
    crate::indexer::jar::index_sources_jars(
        &indexer,
        Some(gradle_dir.path()),
        Some(cache_dir.path()),
    );

    let reloaded =
        crate::indexer::sources_jar_cache::load_sources_jar_cache(Some(cache_dir.path()));
    assert!(
        !reloaded.contains_key("/nonexistent/path/gone-0.1-sources.jar"),
        "entry for the deleted JAR is pruned"
    );
    assert_eq!(reloaded.len(), 1, "only the live JAR remains");
}

// ── params_from_detail: arity parsing from sidecar signature strings ──────────

#[test]
fn params_from_detail_counts_required_plus_total() {
    use crate::indexer::jar::params_from_detail;
    // No params.
    assert_eq!(params_from_detail("interface WindowInsets").1, (0, 0));
    assert_eq!(
        params_from_detail("fun WindowInsets(): WindowInsets").1,
        (0, 0)
    );
    // All required.
    assert_eq!(
        params_from_detail(
            "fun WindowInsets(left: Int, top: Int, right: Int, bottom: Int): WindowInsets"
        )
        .1,
        (4, 4)
    );
    // Defaults → optional (required < total).
    assert_eq!(
        params_from_detail("fun WindowInsets(left: Int = 0, top: Int = 0, right: Int = 0, bottom: Int = 0): WindowInsets").1,
        (0, 4)
    );
    // Function-type param must not terminate the list early.
    assert_eq!(
        params_from_detail("fun launch(context: CoroutineContext, block: suspend () -> Unit): Job")
            .1,
        (2, 2)
    );
    // Generic commas don't inflate the count.
    assert_eq!(
        params_from_detail("fun put(entry: Map<String, Int>): Unit").1,
        (1, 1)
    );
}

/// Regression: a JAR function (e.g. compose `WindowInsets`) must carry real
/// arities parsed from its sidecar detail, not the old hardcoded (0,0) that
/// produced call-arg false positives like `WindowInsets(0,0,0,0)`.
#[test]
fn jar_symbol_gets_real_param_counts() {
    let indexer = idx();
    crate::indexer::jar::populate_from_symbols(
        &indexer,
        std::path::Path::new("/fake/foundation-layout.jar"),
        &[SidecarSymbol {
            name: "WindowInsets".to_owned(),
            kind: "fun".to_owned(),
            container: String::new(),
            detail: "fun WindowInsets(left: Int, top: Int, right: Int, bottom: Int): WindowInsets"
                .to_owned(),
            doc: String::new(),
            type_params: vec![],
            extension_receiver_type: String::new(),
            trailing_lambda: false,
            deprecated: false,
            pkg: String::new(),
            top_level: true,
        }],
    );
    let found = indexer
        .jar_files
        .iter()
        .flat_map(|f| f.value().symbols.clone())
        .find(|s| s.name == "WindowInsets")
        .expect("WindowInsets indexed");
    assert_eq!(found.param_counts, (4, 4), "expected real arity, not (0,0)");
}

fn sym_with_pkg(name: &str, container: &str, pkg: &str, top_level: bool) -> SidecarSymbol {
    SidecarSymbol {
        name: name.to_owned(),
        kind: "fun".to_owned(),
        container: container.to_owned(),
        detail: format!("fun {name}()"),
        doc: String::new(),
        type_params: vec![],
        extension_receiver_type: String::new(),
        trailing_lambda: false,
        deprecated: false,
        pkg: pkg.to_owned(),
        top_level,
    }
}

/// A top-level JAR function registers in `qualified` as `pkg.name` (not
/// `pkg.Facade.name`), while a class member registers as `pkg.Container.name`.
/// This is what lets `import androidx.compose.runtime.remember` resolve to the
/// public top-level overload via the exact-FQN path.
#[test]
fn jar_top_level_plus_member_register_distinct_fqns() {
    let indexer = idx();
    populate_from_symbols(
        &indexer,
        std::path::Path::new("/fake/runtime.jar"),
        &[
            sym_with_pkg(
                "remember",
                "ComposablesKt",
                "androidx.compose.runtime",
                true,
            ),
            sym_with_pkg("remember", "Composer", "androidx.compose.runtime", false),
        ],
    );

    let top = indexer
        .qualified
        .get("androidx.compose.runtime.remember")
        .expect("top-level FQN registered");
    assert_eq!(top.range.start.line, 0, "top-level remember is symbol 0");

    let member = indexer
        .qualified
        .get("androidx.compose.runtime.Composer.remember")
        .expect("member FQN registered");
    assert_eq!(member.range.start.line, 1, "member remember is symbol 1");
}
