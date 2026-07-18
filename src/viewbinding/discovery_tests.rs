use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use tower_lsp::lsp_types::Url;

use crate::indexer::test_helpers::with_xdg_cache;
use crate::indexer::{save_cache, try_load_cache, Indexer, CACHE_VERSION};
use crate::viewbinding::discovery::{
    binding_class_name_for_layout, binding_field_name_to_id, binding_id_to_field_name,
    discover_generated_bindings, import_triggers_binding_discovery,
    is_generated_binding_watcher_path, layout_name_for_binding_class,
    module_root_for_generated_file, module_root_for_source_file,
};
use crate::viewbinding::layout::{
    build_layout_file_data, layout_path_components, parse_layout_xml, LayoutPathComponents,
};

const SAMPLE_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.view.LayoutInflater;
import android.view.View;

public final class FooBarBinding {
    private FooBarBinding() {}
}
"#;

const WRONG_PACKAGE_BINDING_JAVA: &str = r#"package com.example.app.ui;

public final class FooBarBinding {
}
"#;

fn write_binding_java(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir binding dir");
    }
    fs::write(path, content).expect("write binding java");
}

#[test]
fn discover_generated_bindings_uses_databinding_dirs_when_provided() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let databinding_dir =
        module_root.join("build/generated/data_binding/base_builder_log/out/databinding");
    let binding_path = databinding_dir.join("com/example/app/databinding/FooBarBinding.java");
    let noise_path = module_root.join("build/tmp/noise/FooBarBinding.java");
    fs::create_dir_all(binding_path.parent().unwrap()).expect("mkdir binding");
    fs::create_dir_all(noise_path.parent().unwrap()).expect("mkdir noise");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);
    write_binding_java(&noise_path, WRONG_PACKAGE_BINDING_JAVA);

    let discovered = discover_generated_bindings(&module_root, Some(&[databinding_dir.clone()]));
    assert_eq!(discovered.len(), 1);
    assert!(discovered[0].file_uri.contains("data_binding"));
    assert!(!discovered[0].file_uri.contains("build/tmp/"));
}

#[test]
fn discover_generated_bindings_finds_nested_agp_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root.join(
        "build/generated/data_binding_base_class_source_out/debug/out/com/example/app/databinding/FooBarBinding.java",
    );
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let discovered = discover_generated_bindings(&module_root, None);
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].class_name, "FooBarBinding");
    assert!(discovered[0].file_uri.contains("FooBarBinding.java"));
}

#[test]
fn discover_generated_bindings_rejects_wrong_package() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path =
        module_root.join("build/generated/source/debug/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, WRONG_PACKAGE_BINDING_JAVA);

    let discovered = discover_generated_bindings(&module_root, None);
    assert!(discovered.is_empty());
}

#[test]
fn discover_generated_bindings_prefers_newer_mtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let debug_path = module_root.join(
        "build/intermediates/data_binding/debug/com/example/app/databinding/FooBarBinding.java",
    );
    let release_path = module_root.join(
        "build/intermediates/data_binding/release/com/example/app/databinding/FooBarBinding.java",
    );
    write_binding_java(&debug_path, SAMPLE_BINDING_JAVA);
    thread::sleep(Duration::from_millis(1100));
    write_binding_java(&release_path, SAMPLE_BINDING_JAVA);

    let discovered = discover_generated_bindings(&module_root, None);
    assert_eq!(discovered.len(), 1);
    assert!(discovered[0].file_uri.contains("release"));
    assert!(
        discovered[0].modified_at_secs
            >= fs::metadata(&debug_path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
    );
}

#[test]
fn binding_name_mapping_roundtrip_and_edge_cases() {
    assert_eq!(binding_class_name_for_layout("foo_bar"), "FooBarBinding");
    assert_eq!(
        layout_name_for_binding_class("FooBarBinding"),
        Some("foo_bar".to_string())
    );

    assert_eq!(binding_class_name_for_layout("screen"), "ScreenBinding");
    assert_eq!(
        layout_name_for_binding_class("ScreenBinding"),
        Some("screen".to_string())
    );

    assert_eq!(binding_class_name_for_layout("item2"), "Item2Binding");
    assert_eq!(
        layout_name_for_binding_class("Item2Binding"),
        Some("item2".to_string())
    );

    assert_eq!(layout_name_for_binding_class("NotBind"), None);
    assert_eq!(layout_name_for_binding_class("Binding"), None);
    assert_eq!(layout_name_for_binding_class("FooBar"), None);

    assert_eq!(binding_id_to_field_name("foo_bar"), "fooBar");
    assert_eq!(binding_field_name_to_id("fooBar"), "foo_bar");

    assert_eq!(binding_class_name_for_layout("url"), "UrlBinding");
    assert_eq!(
        layout_name_for_binding_class("UrlBinding"),
        Some("url".to_string())
    );
    assert_eq!(binding_class_name_for_layout("u_r_l"), "URLBinding");
    assert_eq!(
        layout_name_for_binding_class("URLBinding"),
        Some("u_r_l".to_string())
    );
    assert_eq!(binding_id_to_field_name("u_r_l"), "uRL");
    assert_eq!(binding_field_name_to_id("uRL"), "u_r_l");
}

#[test]
fn module_root_derivation_uses_last_src_or_build_segment() {
    let generated =
        PathBuf::from("home/user/src/myproject/app/build/generated/databinding/FooBarBinding.java");
    assert_eq!(
        module_root_for_generated_file(&generated),
        Some(PathBuf::from("home/user/src/myproject/app"))
    );

    let source =
        PathBuf::from("home/user/src/myproject/app/src/main/kotlin/com/example/MainActivity.kt");
    assert_eq!(
        module_root_for_source_file(&source),
        Some(PathBuf::from("home/user/src/myproject/app"))
    );
}

#[test]
fn module_root_derivation_for_generated_and_source_paths() {
    let generated = PathBuf::from("project/app/build/generated/databinding/FooBarBinding.java");
    assert_eq!(
        module_root_for_generated_file(&generated),
        Some(PathBuf::from("project/app"))
    );

    let source = PathBuf::from("project/app/src/main/kotlin/com/example/MainActivity.kt");
    assert_eq!(
        module_root_for_source_file(&source),
        Some(PathBuf::from("project/app"))
    );
}

#[test]
fn import_trigger_pattern_matches_databinding_only() {
    assert!(import_triggers_binding_discovery(
        "com.example.app.databinding.FooBarBinding"
    ));
    assert!(!import_triggers_binding_discovery(
        "com.example.app.ui.FooBarBinding"
    ));
    assert!(!import_triggers_binding_discovery(
        "com.example.app.databinding.*"
    ));
}

#[test]
fn import_triggered_indexing_makes_generated_class_resolvable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"
package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun bind(): FooBarBinding = throw NotImplementedError()
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Indexer::new();
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_content(&kotlin_uri, kotlin_source);
    indexer.index_generated_bindings(&module_root, None);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    assert!(
        indexer.qualified.contains_key(qualified_key),
        "generated binding should be indexed into qualified map"
    );
}

#[test]
fn is_generated_binding_uri_distinguishes_generated_from_handwritten() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let generated_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&generated_path, SAMPLE_BINDING_JAVA);

    let handwritten_path = module_root.join("src/main/java/com/example/FooBarBinding.java");
    write_binding_java(
        &handwritten_path,
        "package com.example;\npublic class FooBarBinding {}\n",
    );

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);

    let generated_uri = Url::from_file_path(&generated_path)
        .expect("generated uri")
        .to_string();
    let handwritten_uri = Url::from_file_path(&handwritten_path)
        .expect("handwritten uri")
        .to_string();

    assert!(indexer.is_generated_binding_uri(&generated_uri));
    assert!(!indexer.is_generated_binding_uri(&handwritten_uri));
}

#[test]
fn generated_binding_by_class_index_supports_import_lookup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);

    let locations = indexer.generated_binding_locations_for_class("FooBarBinding");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].module_root, module_root);
    assert_eq!(
        locations[0].package.as_deref(),
        Some("com.example.app.databinding")
    );
    assert!(locations[0].file_uri.contains("FooBarBinding.java"));
}

#[test]
fn workspace_files_importing_binding_class_lists_importers_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let importer_path = module_root.join("src/main/kotlin/com/example/Importer.kt");
    let non_importer_path = module_root.join("src/main/kotlin/com/example/Noise.kt");
    fs::create_dir_all(importer_path.parent().unwrap()).expect("mkdir sources");
    let importer_source =
        "package com.example\nimport com.example.app.databinding.FooBarBinding\nclass Importer\n";
    let noise_source = "package com.example\nclass Noise { val title = \"x\" }\n";
    fs::write(&importer_path, importer_source).expect("write importer");
    fs::write(&non_importer_path, noise_source).expect("write noise");

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);
    let importer_uri = Url::from_file_path(&importer_path).expect("importer uri");
    let noise_uri = Url::from_file_path(&non_importer_path).expect("noise uri");
    indexer.index_content(&importer_uri, importer_source);
    indexer.index_content(&noise_uri, noise_source);

    let scope_files =
        indexer.workspace_files_importing_binding_class("FooBarBinding", &importer_uri);
    assert_eq!(scope_files.len(), 1);
    assert!(scope_files[0].ends_with("Importer.kt"));
    assert!(!scope_files[0].ends_with("Noise.kt"));
}

#[test]
fn layouts_for_binding_class_pairs_by_module_and_orders_default_first() {
    let indexer = Indexer::new();
    let module_root = PathBuf::from("app");

    let default_components = LayoutPathComponents {
        module_root: module_root.clone(),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: String::new(),
    };
    let land_components = LayoutPathComponents {
        module_root: module_root.clone(),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: "land".to_string(),
    };
    let other_module = LayoutPathComponents {
        module_root: PathBuf::from("other"),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: String::new(),
    };

    let layout_xml = r#"<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent" />"#;
    let parsed = parse_layout_xml(layout_xml);

    for (components, uri_suffix) in [
        (default_components, "default"),
        (land_components, "land"),
        (other_module, "other"),
    ] {
        let data = build_layout_file_data(&components, &parsed).expect("layout data");
        let uri = format!("file:///layout/{uri_suffix}");
        indexer.viewbinding.layouts.insert(uri, Arc::new(data));
    }

    let layouts = indexer.layouts_for_binding_class("FooBarBinding", &module_root);
    assert_eq!(layouts.len(), 2);
    assert!(layouts[0].variant_qualifier.is_empty());
    assert_eq!(layouts[1].variant_qualifier, "land");
}

#[test]
fn watcher_path_matcher_requires_build_and_databinding_segments() {
    let valid = PathBuf::from("app/build/generated/databinding/com/example/FooBarBinding.java");
    assert!(is_generated_binding_watcher_path(&valid));

    let missing_databinding = PathBuf::from("app/build/generated/source/FooBarBinding.java");
    assert!(!is_generated_binding_watcher_path(&missing_databinding));
}

#[test]
fn restore_generated_bindings_from_cache_registers_watcher_module() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("mkdir workspace");

    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let watcher_state = Arc::new(crate::viewbinding::DatabindingWatcherState::new());
    let warm_indexer = Indexer::new();
    warm_indexer.set_databinding_watcher_handle(crate::viewbinding::DatabindingWatcherHandle::new(
        Arc::clone(&watcher_state),
    ));
    warm_indexer.index_generated_bindings(&module_root, None);

    with_xdg_cache(temp.path(), || {
        save_cache(
            &root,
            &warm_indexer.files,
            &warm_indexer.content_hashes,
            &warm_indexer.library_uris,
            &warm_indexer.viewbinding.layouts,
            &warm_indexer.viewbinding.generated_bindings,
            true,
            true,
        );

        let loaded = try_load_cache(&root).expect("cache loaded");
        let restored_indexer = Indexer::new();
        restored_indexer.set_databinding_watcher_handle(
            crate::viewbinding::DatabindingWatcherHandle::new(Arc::clone(&watcher_state)),
        );
        restored_indexer.restore_generated_bindings_from_cache(&loaded.generated_bindings);

        assert!(
            watcher_state
                .registered_module_roots()
                .contains(&module_root),
            "cache restore must register the module with the databinding watcher"
        );
    });
}

#[test]
fn view_id_lookup_prefers_exact_match_over_ambiguous_normalized() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let default_path = module_root.join("src/main/res/layout/conflict.xml");
    let land_path = module_root.join("src/main/res/layout-land/conflict.xml");
    fs::create_dir_all(default_path.parent().unwrap()).expect("mkdir layout");
    fs::create_dir_all(land_path.parent().unwrap()).expect("mkdir land");
    let layout = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android">
    <TextView android:id="@+id/fooBar" />
</LinearLayout>
"#;
    fs::write(&default_path, layout).expect("write default");
    fs::write(&land_path, layout).expect("write land");

    let indexer = Indexer::new();
    indexer.index_layout_content(
        &Url::from_file_path(&default_path).expect("default uri"),
        layout,
    );
    indexer.index_layout_content(&Url::from_file_path(&land_path).expect("land uri"), layout);

    let exact = indexer.layouts_declaring_view_id(&module_root, "conflict", "fooBar");
    assert_eq!(exact.len(), 2, "exact camelCase id matches both variants");

    let ambiguous = indexer.layouts_declaring_view_id(&module_root, "conflict", "foo_bar");
    assert!(
        ambiguous.is_empty(),
        "normalized fallback must not pick when multiple variants normalize-match"
    );
}

#[test]
fn generated_bindings_cache_roundtrip() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("mkdir workspace");

    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);

    let cache_base = temp.path().join("cache");
    with_xdg_cache(&cache_base, || {
        save_cache(
            &root,
            &indexer.files,
            &indexer.content_hashes,
            &indexer.library_uris,
            &indexer.viewbinding.layouts,
            &indexer.viewbinding.generated_bindings,
            true,
            true,
        );

        let loaded = try_load_cache(&root).expect("cache loaded");
        assert_eq!(loaded.version, CACHE_VERSION);
        let module_key = module_root.to_string_lossy().to_string();
        let cached = loaded
            .generated_bindings
            .get(&module_key)
            .expect("module cache entry");
        assert!(cached.entries.contains_key("FooBarBinding"));
    });
}

#[test]
fn index_layout_content_triggers_binding_discovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    let layout_xml = r#"<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent" />"#;
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, layout_xml).expect("write layout");

    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let indexer = Arc::new(Indexer::new());
        indexer.set_binding_discovery_handle(
            crate::viewbinding::discovery::spawn_binding_discovery_worker(Arc::clone(&indexer)),
        );

        let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
        indexer.index_layout_content(&layout_uri, layout_xml);

        let qualified_key = "com.example.app.databinding.FooBarBinding";
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if indexer.qualified.contains_key(qualified_key) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("layout index did not trigger binding discovery within timeout");
    });
}

#[test]
fn watcher_touch_reindexes_generated_binding_fixture() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);
    let qualified_key = "com.example.app.databinding.FooBarBinding";
    assert!(indexer.qualified.contains_key(qualified_key));

    let updated_source = format!("{SAMPLE_BINDING_JAVA}\n// touched\n");
    thread::sleep(Duration::from_millis(1100));
    fs::write(&binding_path, updated_source).expect("rewrite binding");
    indexer.index_generated_bindings(&module_root, None);

    assert!(indexer.qualified.contains_key(qualified_key));
    let binding_uri = Url::from_file_path(&binding_path)
        .expect("binding uri")
        .to_string();
    assert!(indexer.is_generated_binding_uri(&binding_uri));
}

#[test]
fn reindex_removes_index_entries_for_deleted_binding_files() {
    let temp = tempfile::tempdir().expect("tempdir");

    let app_module_root = temp.path().join("app");
    let app_binding_path = app_module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    write_binding_java(&app_binding_path, SAMPLE_BINDING_JAVA);

    // Competing binding with the same class name in another module — it must
    // survive a clean build of `app`.
    let other_module_root = temp.path().join("other");
    let other_binding_path = other_module_root
        .join("build/generated/databinding/com/example/other/databinding/FooBarBinding.java");
    let other_binding_java = SAMPLE_BINDING_JAVA.replace(
        "package com.example.app.databinding;",
        "package com.example.other.databinding;",
    );
    write_binding_java(&other_binding_path, &other_binding_java);

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&app_module_root, None);
    indexer.index_generated_bindings(&other_module_root, None);

    let app_qualified_key = "com.example.app.databinding.FooBarBinding";
    let other_qualified_key = "com.example.other.databinding.FooBarBinding";
    assert!(indexer.qualified.contains_key(app_qualified_key));
    assert!(indexer.qualified.contains_key(other_qualified_key));

    let app_binding_uri = Url::from_file_path(&app_binding_path)
        .expect("app binding uri")
        .to_string();
    assert!(indexer.is_generated_binding_uri(&app_binding_uri));

    // Simulate a clean build of `app`: the generated file disappears from disk.
    fs::remove_file(&app_binding_path).expect("delete app binding");
    indexer.index_generated_bindings(&app_module_root, None);

    assert!(
        !indexer.qualified.contains_key(app_qualified_key),
        "qualified entry must not survive a clean build"
    );
    assert!(!indexer.is_generated_binding_uri(&app_binding_uri));
    assert!(
        indexer
            .definition_locations("FooBarBinding")
            .iter()
            .all(|location| location.uri.as_str() != app_binding_uri),
        "definitions must not resolve to the deleted generated path"
    );

    // The other module's same-named binding is untouched.
    assert!(indexer.qualified.contains_key(other_qualified_key));
    let other_binding_uri = Url::from_file_path(&other_binding_path)
        .expect("other binding uri")
        .to_string();
    assert!(indexer.is_generated_binding_uri(&other_binding_uri));
}

#[test]
fn layout_path_components_and_module_root_stay_consistent() {
    let layout_path = PathBuf::from("app/src/main/res/layout/foo_bar.xml");
    let components = layout_path_components(&layout_path).expect("layout components");
    assert_eq!(components.module_root, PathBuf::from("app"));
    assert_eq!(
        binding_class_name_for_layout(&components.layout_name),
        "FooBarBinding"
    );
}
