use super::*;
use std::fs;
use tempfile::TempDir;

fn make_workspace_json(dir: &TempDir, json: &str) {
    fs::write(dir.path().join("workspace.json"), json).unwrap();
}

// ─── workspace.json tests ─────────────────────────────────────────────────────

#[test]
fn missing_file_returns_empty() {
    let dir = TempDir::new().unwrap();
    let paths = load_source_paths(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn malformed_json_returns_empty() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(&dir, "{ not valid json }}}");
    let paths = load_source_paths(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn extracts_java_source_and_java_test() {
    let dir = TempDir::new().unwrap();
    let json = r#"{
            "modules": [{
                "contentRoots": [{
                    "sourceRoots": [
                        {"path": "<WORKSPACE>/src/main/kotlin", "type": "java-source"},
                        {"path": "<WORKSPACE>/src/test/kotlin", "type": "java-test"},
                        {"path": "<WORKSPACE>/src/main/resources", "type": "java-resource"},
                        {"path": "<WORKSPACE>/src/test/resources", "type": "java-test-resource"}
                    ]
                }]
            }]
        }"#;
    make_workspace_json(&dir, json);

    let paths = load_source_paths(dir.path());
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], dir.path().join("src/main/kotlin"));
    assert_eq!(paths[1], dir.path().join("src/test/kotlin"));
    // resources excluded
    assert!(!paths.iter().any(|p| p.ends_with("resources")));
}

#[test]
fn deduplicates_paths_across_modules() {
    let dir = TempDir::new().unwrap();
    let json = r#"{
        "modules": [
            {"contentRoots": [{"sourceRoots": [{"path": "<WORKSPACE>/src/main/kotlin", "type": "java-source"}]}]},
            {"contentRoots": [{"sourceRoots": [{"path": "<WORKSPACE>/src/main/kotlin", "type": "java-source"}]}]}
        ]
    }"#;
    make_workspace_json(&dir, json);

    let paths = load_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
}

#[test]
fn resolves_workspace_placeholder() {
    let dir = TempDir::new().unwrap();
    let json = r#"{
        "modules": [{"contentRoots": [{"sourceRoots": [
            {"path": "<WORKSPACE>/app/src/main/kotlin", "type": "java-source"}
        ]}]}]
    }"#;
    make_workspace_json(&dir, json);

    let paths = load_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(paths[0].is_absolute());
    assert!(paths[0].ends_with("app/src/main/kotlin"));
}

#[test]
fn empty_modules_returns_empty() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(&dir, r#"{"modules": []}"#);
    let paths = load_source_paths(dir.path());
    assert!(paths.is_empty());
}

// ─── build-layout detection tests ────────────────────────────────────────────

#[test]
fn no_build_file_returns_empty() {
    let dir = TempDir::new().unwrap();
    let paths = detect_build_layout_source_paths(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn gradle_kts_probes_standard_dirs() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("build.gradle.kts"), "").unwrap();
    let src = dir.path().join("src/main/kotlin");
    fs::create_dir_all(&src).unwrap();
    let test = dir.path().join("src/test/kotlin");
    fs::create_dir_all(&test).unwrap();

    let paths = detect_build_layout_source_paths(dir.path());
    assert!(paths.contains(&src));
    assert!(paths.contains(&test));
}

#[test]
fn nonexistent_candidates_excluded() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("build.gradle.kts"), "").unwrap();
    // No source dirs created.
    let paths = detect_build_layout_source_paths(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn maven_pom_triggers_detection() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("pom.xml"), "<project/>").unwrap();
    let src = dir.path().join("src/main/java");
    fs::create_dir_all(&src).unwrap();

    let paths = detect_build_layout_source_paths(dir.path());
    assert!(paths.contains(&src));
}

#[test]
fn settings_gradle_multimodule() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        r#"include(":app", ":core")"#,
    )
    .unwrap();
    let app_src = dir.path().join("app/src/main/kotlin");
    let core_src = dir.path().join("core/src/main/kotlin");
    fs::create_dir_all(&app_src).unwrap();
    fs::create_dir_all(&core_src).unwrap();

    let paths = detect_build_layout_source_paths(dir.path());
    assert!(paths.contains(&app_src));
    assert!(paths.contains(&core_src));
}

#[test]
fn kmp_source_sets_discovered_structurally() {
    // probe_source_set_roots() must discover non-standard KMP source sets by
    // checking which src/<set>/{kotlin,java} directories actually exist on disk,
    // rather than relying on a hardcoded allowlist.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("build.gradle.kts"), "").unwrap();

    // Standard sets
    let common_main = dir.path().join("src/commonMain/kotlin");
    let android_main = dir.path().join("src/androidMain/kotlin");
    // Non-standard custom set — must also be discovered
    let custom_set = dir.path().join("src/myCustomSet/kotlin");
    // Java-only set
    let jvm_java = dir.path().join("src/jvmMain/java");
    fs::create_dir_all(&common_main).unwrap();
    fs::create_dir_all(&android_main).unwrap();
    fs::create_dir_all(&custom_set).unwrap();
    fs::create_dir_all(&jvm_java).unwrap();

    let paths = detect_build_layout_source_paths(dir.path());
    assert!(
        paths.contains(&common_main),
        "commonMain/kotlin must be discovered; got {paths:?}"
    );
    assert!(
        paths.contains(&android_main),
        "androidMain/kotlin must be discovered; got {paths:?}"
    );
    assert!(
        paths.contains(&custom_set),
        "user-defined myCustomSet/kotlin must be discovered; got {paths:?}"
    );
    assert!(
        paths.contains(&jvm_java),
        "jvmMain/java must be discovered; got {paths:?}"
    );
}

// ─── parse_include_calls unit tests ──────────────────────────────────────────

#[test]
fn parses_colon_prefixed_includes() {
    let content = r#"include(":app", ":core", ":data")"#;
    let result = parse_include_calls(content);
    assert_eq!(result, vec!["app", "core", "data"]);
}

#[test]
fn parses_nested_module_paths() {
    let content = r#"include(":feature:login", ":feature:home")"#;
    let result = parse_include_calls(content);
    let sep = std::path::MAIN_SEPARATOR_STR;
    assert_eq!(result[0], format!("feature{sep}login"));
    assert_eq!(result[1], format!("feature{sep}home"));
}

#[test]
fn deduplicates_include_entries() {
    let content = "include(\":app\")\ninclude(\":app\")";
    let result = parse_include_calls(content);
    assert_eq!(result.len(), 1);
}

#[test]
fn parses_single_quoted_includes() {
    let content = "include(':app', ':core')";
    let result = parse_include_calls(content);
    assert_eq!(result, vec!["app", "core"]);
}

#[test]
fn ignores_include_build_lines() {
    let content = "includeBuild(\"../other-project\")\ninclude(\":app\")";
    let result = parse_include_calls(content);
    assert_eq!(result, vec!["app"]);
}

// ─── Android SDK detection tests ─────────────────────────────────────────────

#[test]
fn no_sdk_returns_empty() {
    let dir = TempDir::new().unwrap();
    // No local.properties, no env vars set in test env
    let paths = detect_android_sdk_source_paths(dir.path());
    // Either empty (no SDK) or points to a real SDK — either is valid in CI.
    // We just verify the function returns without panic.
    let _ = paths;
}

#[test]
fn sdk_dir_from_local_properties_finds_sdk_dot_dir() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    fs::create_dir_all(fake_sdk.join("sources").join("android-34")).unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!(
            "# generated\nsdk.dir={}\nndk.version=25.0.0\n",
            fake_sdk.display()
        ),
    )
    .unwrap();
    let paths = detect_android_sdk_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("android-34"));
}

#[test]
fn picks_highest_api_level() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    for api in [31_u32, 33, 34] {
        fs::create_dir_all(fake_sdk.join("sources").join(format!("android-{api}"))).unwrap();
    }
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();
    let paths = detect_android_sdk_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].ends_with("android-34"),
        "expected android-34, got {:?}",
        paths[0]
    );
}

#[test]
fn sdk_dir_from_local_properties_with_whitespace() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    fs::create_dir_all(fake_sdk.join("sources").join("android-35")).unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir = {} \n", fake_sdk.display()),
    )
    .unwrap();
    let paths = detect_android_sdk_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("android-35"));
}

// ─── Android SDK compiled-JAR detection tests ────────────────────────────────

#[test]
fn jar_path_picks_highest_api_level_with_jar_present() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    for api in ["android-33", "android-34"] {
        let platform_dir = fake_sdk.join("platforms").join(api);
        fs::create_dir_all(&platform_dir).unwrap();
        fs::write(platform_dir.join("android.jar"), b"fake jar").unwrap();
    }
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].ends_with("android-34/android.jar")
            || paths[0].ends_with("android-34\\android.jar")
    );
}

#[test]
fn jar_path_skips_platform_dir_missing_the_jar() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    // android-34 is the higher API level but its android.jar is missing
    // (partial install) — android-33 must win instead.
    fs::create_dir_all(fake_sdk.join("platforms").join("android-34")).unwrap();
    let platform_33 = fake_sdk.join("platforms").join("android-33");
    fs::create_dir_all(&platform_33).unwrap();
    fs::write(platform_33.join("android.jar"), b"fake jar").unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].ends_with("android-33/android.jar")
            || paths[0].ends_with("android-33\\android.jar")
    );
}

#[test]
fn jar_path_independent_of_sources_only_install() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    // Sources are present for android-35, but there is no matching
    // platforms/android-35/android.jar — the two detectors must not
    // cross-contaminate each other's result.
    fs::create_dir_all(fake_sdk.join("sources").join("android-35")).unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let jar_paths = detect_android_sdk_jar_path(dir.path());
    assert!(jar_paths.is_empty());
    let source_paths = detect_android_sdk_source_paths(dir.path());
    assert_eq!(source_paths.len(), 1);
    assert!(source_paths[0].ends_with("android-35"));
}

#[test]
fn jar_path_no_sdk_root_returns_empty() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    // sdk.dir points at a directory with no platforms/ subdirectory at all.
    fs::create_dir_all(&fake_sdk).unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn jar_path_platforms_dir_with_no_valid_jar_returns_empty() {
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    // platforms/ exists, and even has an android-XX-named directory, but
    // no android.jar inside it — a different empty shape than "no SDK at all".
    fs::create_dir_all(fake_sdk.join("platforms").join("android-33")).unwrap();
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert!(paths.is_empty());
}

#[test]
fn jar_path_handles_extension_level_directory_names() {
    // Real Android SDK installs (confirmed on this machine) name newer
    // platform directories with a decimal extension level, e.g.
    // `android-37.0`/`android-36.1`, not just plain integers. The highest
    // level must still be picked correctly across mixed naming styles.
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    for api in ["android-36", "android-36.1", "android-37.0"] {
        let platform_dir = fake_sdk.join("platforms").join(api);
        fs::create_dir_all(&platform_dir).unwrap();
        fs::write(platform_dir.join("android.jar"), b"fake jar").unwrap();
    }
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].ends_with("android-37.0/android.jar")
            || paths[0].ends_with("android-37.0\\android.jar")
    );
}

#[test]
fn source_paths_handles_extension_level_directory_names() {
    // Regression guard for the shared version-comparison helper: the
    // pre-existing source-path detector must also correctly rank a decimal
    // extension-level directory as higher than a plain-integer one.
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    for api in ["android-36", "android-36.1"] {
        fs::create_dir_all(fake_sdk.join("sources").join(api)).unwrap();
    }
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_source_paths(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("android-36.1"));
}

#[test]
fn jar_path_handles_extension_platform_directory_names() {
    // Real Android SDK installs can also have Extension platform directories
    // named `android-<major>-ext<N>` (e.g. `android-36-ext14`) alongside a
    // plain `android-<major>` base directory — a different naming scheme
    // from the decimal-minor one above. Both must be recognized, and the
    // extension directory (additive to its base level) must outrank the
    // plain base of the same major version.
    let dir = TempDir::new().unwrap();
    let fake_sdk = dir.path().join("sdk");
    for api in ["android-36", "android-36-ext14"] {
        let platform_dir = fake_sdk.join("platforms").join(api);
        fs::create_dir_all(&platform_dir).unwrap();
        fs::write(platform_dir.join("android.jar"), b"fake jar").unwrap();
    }
    fs::write(
        dir.path().join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();

    let paths = detect_android_sdk_jar_path(dir.path());
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].ends_with("android-36-ext14/android.jar")
            || paths[0].ends_with("android-36-ext14\\android.jar"),
        "expected the extension-level directory to outrank the plain base, got: {:?}",
        paths[0]
    );
}

// ─── jarPaths ─────────────────────────────────────────────────────────────────

#[test]
fn jar_paths_resolves_files_dirs_placeholder() {
    let dir = TempDir::new().unwrap();
    // A directory of jars + a standalone jar, plus sources/javadoc that must be excluded.
    fs::create_dir_all(dir.path().join("libs")).unwrap();
    for file_rel in [
        "libs/foo.jar",
        "libs/bar.aar",
        "libs/foo-sources.jar",
        "libs/foo-javadoc.jar",
        // Legit compiled jar that merely *contains* "-sources" — must be kept
        // (exclusion is suffix-based, not substring).
        "libs/my-sources-helper.jar",
        "extra.jar",
    ] {
        fs::write(dir.path().join(file_rel), b"x").unwrap();
    }
    make_workspace_json(
        &dir,
        r#"{"jarPaths": ["<WORKSPACE>/libs", "extra.jar", "missing.jar"]}"#,
    );

    let jars = load_configured_jar_paths(dir.path());
    let names: Vec<String> = jars
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(
        names.contains(&"foo.jar".to_owned()),
        "dir jar missing: {names:?}"
    );
    assert!(
        names.contains(&"bar.aar".to_owned()),
        "dir aar missing: {names:?}"
    );
    assert!(
        names.contains(&"extra.jar".to_owned()),
        "relative jar missing: {names:?}"
    );
    assert!(
        !names.contains(&"foo-sources.jar".to_owned()),
        "sources jar leaked: {names:?}"
    );
    assert!(
        !names.contains(&"foo-javadoc.jar".to_owned()),
        "javadoc jar leaked: {names:?}"
    );
    // Suffix-based exclusion: a jar merely containing "-sources" is kept.
    assert!(
        names.contains(&"my-sources-helper.jar".to_owned()),
        "suffix exclusion wrongly dropped a legit jar: {names:?}"
    );
    // A nonexistent file spec is skipped (a warning is logged).
    assert!(!names.contains(&"missing.jar".to_owned()));
}

#[test]
fn jar_paths_absent_returns_empty() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(&dir, r#"{"sourcePaths": []}"#);
    assert!(load_configured_jar_paths(dir.path()).is_empty());
}

// ─── real workspace.json schema: libraries[] + dependencies[] ─────────────────

/// Mirrors the design doc's cited PetClinic excerpt: a module with a
/// `library`-type dependency, and a matching `libraries[]` entry whose
/// `properties.attributes` carries structured GAV coordinates.
#[test]
fn real_schema_deserialization_extracts_gav_from_properties() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(
        &dir,
        r#"{
            "modules": [{
                "name": "PetClinic.main",
                "contentRoots": [{"path": "<WORKSPACE>/app"}],
                "dependencies": [
                    {"type": "library", "name": "Gradle: ch.qos.logback:logback-classic:1.5.16", "scope": "compile"}
                ]
            }],
            "libraries": [{
                "name": "Gradle: ch.qos.logback:logback-classic:1.5.16",
                "type": "COMPILE",
                "roots": [{"path": "<GRADLE_REPO>/logback-classic-1.5.16.jar"}],
                "properties": {"attributes": {
                    "groupId": "ch.qos.logback",
                    "artifactId": "logback-classic",
                    "version": "1.5.16",
                    "baseVersion": "1.5.16"
                }}
            }]
        }"#,
    );

    let workspace_data = parse_workspace_data(dir.path()).expect("real schema fixture must parse");
    assert_eq!(workspace_data.libraries.len(), 1);
    let gradle_meta =
        library_gradle_meta(&workspace_data.libraries[0]).expect("properties path must resolve");
    assert_eq!(gradle_meta.group, "ch.qos.logback");
    assert_eq!(gradle_meta.artifact, "logback-classic");
    assert_eq!(gradle_meta.version, "1.5.16");
}

#[test]
fn library_gradle_meta_falls_back_to_name_string_when_properties_absent() {
    let library = LibraryData {
        name: "Gradle: org.jetbrains.kotlin:kotlin-stdlib:2.0.0".to_owned(),
        roots: Vec::new(),
        properties: None,
    };
    let gradle_meta = library_gradle_meta(&library).expect("name-string fallback must resolve");
    assert_eq!(gradle_meta.group, "org.jetbrains.kotlin");
    assert_eq!(gradle_meta.artifact, "kotlin-stdlib");
    assert_eq!(gradle_meta.version, "2.0.0");
}

/// The third-party plugin's synthetic Android SDK library is also
/// `"Gradle: "`-prefixed — it must parse like any other 3-segment name.
#[test]
fn library_gradle_meta_parses_synthetic_android_sdk_name() {
    let library = LibraryData {
        name: "Gradle: android:android:36".to_owned(),
        roots: Vec::new(),
        properties: None,
    };
    let gradle_meta = library_gradle_meta(&library).expect("synthetic name must still parse");
    assert_eq!(gradle_meta.group, "android");
    assert_eq!(gradle_meta.artifact, "android");
    assert_eq!(gradle_meta.version, "36");
}

#[test]
fn library_gradle_meta_returns_none_for_malformed_library() {
    let library = LibraryData {
        name: "a hand-added local jar".to_owned(),
        roots: Vec::new(),
        properties: None,
    };
    assert!(library_gradle_meta(&library).is_none());
}

/// An unrecognized `type` value on a dependency entry (simulating a future
/// schema addition) must be ignored, not fail the whole module's parse.
#[test]
fn unknown_dependency_type_is_ignored_not_a_parse_error() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(
        &dir,
        r#"{
            "modules": [{
                "name": "app.main",
                "contentRoots": [{"path": "<WORKSPACE>/app"}],
                "dependencies": [
                    {"type": "futureKind", "name": "something-unknown"},
                    {"type": "library", "name": "Gradle: com.example:known:1.0"}
                ]
            }],
            "libraries": [{
                "name": "Gradle: com.example:known:1.0",
                "roots": []
            }]
        }"#,
    );

    let workspace_data =
        parse_workspace_data(dir.path()).expect("unknown dependency type must not fail parsing");
    assert_eq!(workspace_data.modules.len(), 1);
    assert_eq!(workspace_data.modules[0].dependencies.len(), 2);
}

/// Matches the third-party plugin's documented deviations: `type` is the
/// non-standard `"java-imported"`, no `module`/`sdk`-type dependency entries
/// ever appear, and `externalProjectId` is never emitted.
#[test]
fn third_party_plugin_shape_does_not_break_parsing() {
    let dir = TempDir::new().unwrap();
    make_workspace_json(
        &dir,
        r#"{
            "modules": [{
                "name": "app.main",
                "contentRoots": [{"path": "<WORKSPACE>/app"}],
                "dependencies": [
                    {"type": "library", "name": "Gradle: com.example:known:1.0", "scope": "compile"},
                    {"type": "moduleSource"},
                    {"type": "inheritedSdk"}
                ]
            }],
            "libraries": [{
                "name": "Gradle: com.example:known:1.0",
                "type": "java-imported",
                "roots": [{"path": "<GRADLE_REPO>/known-1.0.jar"}]
            }]
        }"#,
    );

    let workspace_data =
        parse_workspace_data(dir.path()).expect("third-party plugin shape must parse");
    assert!(workspace_data.modules[0].external_project_id.is_none());
    let gradle_meta = library_gradle_meta(&workspace_data.libraries[0])
        .expect("name-string fallback must still resolve despite java-imported type");
    assert_eq!(gradle_meta.artifact, "known");
}

#[test]
fn load_module_dependencies_scopes_each_module_to_its_own_content_roots() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("app")).unwrap();
    fs::create_dir_all(dir.path().join("core")).unwrap();
    make_workspace_json(
        &dir,
        r#"{
            "modules": [
                {
                    "name": "app.main",
                    "contentRoots": [{"path": "<WORKSPACE>/app"}],
                    "dependencies": [
                        {"type": "library", "name": "Gradle: com.example:shared:1.0"},
                        {"type": "library", "name": "Gradle: com.example:app-only:2.0"}
                    ]
                },
                {
                    "name": "core.main",
                    "contentRoots": [{"path": "<WORKSPACE>/core"}],
                    "dependencies": [
                        {"type": "library", "name": "Gradle: com.example:shared:1.0"},
                        {"type": "library", "name": "Gradle: com.example:core-only:3.0"}
                    ]
                }
            ],
            "libraries": [
                {"name": "Gradle: com.example:shared:1.0", "roots": []},
                {"name": "Gradle: com.example:app-only:2.0", "roots": []},
                {"name": "Gradle: com.example:core-only:3.0", "roots": []}
            ]
        }"#,
    );

    let dependencies_by_content_root = load_module_dependencies(dir.path());

    let app_dependencies = dependencies_by_content_root
        .get(&dir.path().join("app"))
        .expect("app content root must be present");
    let artifacts: std::collections::HashSet<&str> = app_dependencies
        .iter()
        .map(|gradle_meta| gradle_meta.artifact.as_str())
        .collect();
    assert!(artifacts.contains("shared"));
    assert!(artifacts.contains("app-only"));
    assert!(
        !artifacts.contains("core-only"),
        "core-only dependency leaked into app's scope: {artifacts:?}"
    );

    let core_dependencies = dependencies_by_content_root
        .get(&dir.path().join("core"))
        .expect("core content root must be present");
    let artifacts: std::collections::HashSet<&str> = core_dependencies
        .iter()
        .map(|gradle_meta| gradle_meta.artifact.as_str())
        .collect();
    assert!(artifacts.contains("shared"));
    assert!(artifacts.contains("core-only"));
    assert!(
        !artifacts.contains("app-only"),
        "app-only dependency leaked into core's scope: {artifacts:?}"
    );
}

// ─── Android R.jar detection tests ───────────────────────────────────────────

/// Real, observed AGP output shape: `<module>/build/intermediates/
/// compile_r_class_jar/<variant>/generate<Variant>RFile/R.jar`.
fn write_r_jar(module_dir: &std::path::Path, task_dir: &str, variant: &str, task_output: &str) {
    let dir = module_dir
        .join("build/intermediates")
        .join(task_dir)
        .join(variant)
        .join(task_output);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("R.jar"), b"fake r.jar").unwrap();
}

#[test]
fn r_class_jars_found_across_multiple_modules() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "include(\":core:common\")\ninclude(\":feature:accident\")\n",
    )
    .unwrap();
    write_r_jar(
        &dir.path().join("core/common"),
        "compile_r_class_jar",
        "debug",
        "generateDebugRFile",
    );
    write_r_jar(
        &dir.path().join("feature/accident"),
        "compile_r_class_jar",
        "debug",
        "generateDebugRFile",
    );

    let jars = detect_android_r_class_jars(dir.path());
    assert_eq!(jars.len(), 2, "expected one R.jar per module; got {jars:?}");
    assert!(jars
        .iter()
        .any(|j| j.starts_with(dir.path().join("core/common"))));
    assert!(jars
        .iter()
        .any(|j| j.starts_with(dir.path().join("feature/accident"))));
}

#[test]
fn r_class_jar_prefers_debug_shaped_variant_over_custom_flavor() {
    // Real, observed shape: a custom product-flavor setup produces variant
    // names like `tst1Debug`/`ppeDebug`/`prodDebug` — none literally named
    // "debug", but "prodDebug" (say) still CONTAINS "debug" and should win
    // over a variant that doesn't (e.g. a pure "release"-only entry, or an
    // unrelated non-debug flavor combination).
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "include(\":app\")\n",
    )
    .unwrap();
    let module_dir = dir.path().join("app");
    write_r_jar(
        &module_dir,
        "compile_and_runtime_r_class_jar",
        "release",
        "processReleaseResources",
    );
    write_r_jar(
        &module_dir,
        "compile_and_runtime_r_class_jar",
        "prodDebug",
        "processProdDebugResources",
    );

    let jars = detect_android_r_class_jars(dir.path());
    assert_eq!(jars.len(), 1);
    assert!(
        jars[0].to_string_lossy().contains("prodDebug"),
        "must prefer the debug-shaped variant over release; got {:?}",
        jars[0]
    );
}

#[test]
fn r_class_jar_falls_back_to_compile_and_runtime_task_dir() {
    // App/test modules use a differently-named AGP task output dir than
    // library modules (`compile_and_runtime_r_class_jar`, not
    // `compile_r_class_jar`) — both must be checked.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "include(\":mobile\")\n",
    )
    .unwrap();
    write_r_jar(
        &dir.path().join("mobile"),
        "compile_and_runtime_r_class_jar",
        "tst1Debug",
        "processTst1DebugResources",
    );

    let jars = detect_android_r_class_jars(dir.path());
    assert_eq!(jars.len(), 1);
}

#[test]
fn r_class_jar_skips_module_never_built() {
    // A module with no `build/` output at all (never built, or pure-Kotlin
    // with no `res/`) must be silently skipped, never an error.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("settings.gradle.kts"),
        "include(\":core:common\")\ninclude(\":core:pure-kotlin\")\n",
    )
    .unwrap();
    write_r_jar(
        &dir.path().join("core/common"),
        "compile_r_class_jar",
        "debug",
        "generateDebugRFile",
    );
    // core/pure-kotlin has no build/ dir at all.

    let jars = detect_android_r_class_jars(dir.path());
    assert_eq!(
        jars.len(),
        1,
        "only the built module's R.jar should be found; got {jars:?}"
    );
}
