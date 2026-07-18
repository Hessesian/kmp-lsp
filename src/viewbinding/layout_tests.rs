use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use super::{
    build_layout_file_data, element_tag_at_layout_position, id_attribute_position_for_view_id,
    is_layout_xml_path, layout_path_components, parse_layout_xml, view_id_at_layout_position,
    LayoutPathComponents,
};
use crate::indexer::test_helpers::with_xdg_cache;
use crate::indexer::{save_cache, try_load_cache, Indexer, CACHE_VERSION};

const SAMPLE_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<androidx.constraintlayout.widget.ConstraintLayout
    xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:tools="http://schemas.android.com/tools"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />

    <include
        android:id="@+id/header"
        layout="@layout/view_header" />
</androidx.constraintlayout.widget.ConstraintLayout>
"#;

#[test]
fn parse_layout_xml_extracts_ids_includes_and_root_tag() {
    let parsed = parse_layout_xml(SAMPLE_LAYOUT);
    let root_tag = parsed.root_tag.expect("root tag");
    assert!(
        root_tag.tag_name.contains("ConstraintLayout"),
        "unexpected root tag: {}",
        root_tag.tag_name
    );
    assert!(!parsed.view_binding_ignore);

    let title = parsed
        .view_ids
        .iter()
        .find(|view_id| view_id.id == "title")
        .expect("title id");
    assert_eq!(title.tag_name, "TextView");
    assert!(title.id_attribute_range.start.line >= 7);

    let include = parsed.includes.first().expect("include");
    assert_eq!(include.id.as_deref(), Some("header"));
    assert_eq!(include.included_layout_name, "view_header");
}

#[test]
fn parse_layout_xml_view_binding_ignore_flag() {
    let content = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:tools="http://schemas.android.com/tools"
    tools:viewBindingIgnore="true"
    android:layout_width="match_parent"
    android:layout_height="match_parent" />
"#;
    let parsed = parse_layout_xml(content);
    assert!(parsed.view_binding_ignore);
}

#[test]
fn parse_layout_xml_include_without_id() {
    let content = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout>
    <include layout="@layout/view_header" />
</LinearLayout>
"#;
    let parsed = parse_layout_xml(content);
    let include = parsed.includes.first().expect("include");
    assert!(include.id.is_none());
    assert_eq!(include.included_layout_name, "view_header");
}

#[test]
fn parse_layout_xml_malformed_lone_quote_does_not_panic() {
    let content = r#"<TextView android:id="" />"#;
    let parsed = parse_layout_xml(content);
    assert!(parsed.view_ids.is_empty());
}

#[test]
fn parse_layout_xml_id_range_uses_utf16_columns_with_multibyte_prefix() {
    let content = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android">
    <TextView android:label="标题" android:id="@+id/title" />
</LinearLayout>
"#;
    let parsed = parse_layout_xml(content);
    let title = parsed
        .view_ids
        .iter()
        .find(|view_id| view_id.id == "title")
        .expect("title id");

    let line_index = title.id_attribute_range.start.line as usize;
    let line = content.lines().nth(line_index).expect("layout line");
    let value_start = line
        .find("\"@+id/title\"")
        .or_else(|| line.find("@+id/title"))
        .expect("id value in line");
    let expected_utf16 = line[..value_start]
        .chars()
        .map(|character| character.len_utf16())
        .sum::<usize>() as u32;

    assert_eq!(
        title.id_attribute_range.start.character, expected_utf16,
        "id attribute range must use UTF-16 columns (line: {line})"
    );
}

#[test]
fn layout_path_components_uses_last_src_not_parent_src_directory() {
    let path = PathBuf::from("home/user/src/myproject/app/src/main/res/layout/foo_bar.xml");
    let components = layout_path_components(&path).expect("layout under nested src dirs");
    assert_eq!(
        components.module_root,
        PathBuf::from("home/user/src/myproject/app")
    );
    assert_eq!(components.layout_name, "foo_bar");
}

#[test]
fn layout_path_components_default_and_qualifier_variants() {
    let default_path = PathBuf::from("app/src/main/res/layout/foo_bar.xml");
    let components = layout_path_components(&default_path).expect("default layout");
    assert_eq!(components.module_root, PathBuf::from("app"));
    assert_eq!(components.layout_name, "foo_bar");
    assert!(components.variant_qualifier.is_empty());

    let land_path = PathBuf::from("app/src/main/res/layout-land/foo_bar.xml");
    let land = layout_path_components(&land_path).expect("land layout");
    assert_eq!(land.variant_qualifier, "land");
}

#[test]
fn layout_path_components_rejects_non_layout_directories() {
    let rejected = PathBuf::from("app/src/main/res/layouts_backup/foo_bar.xml");
    assert!(layout_path_components(&rejected).is_none());
    assert!(!is_layout_xml_path(&rejected));
}

#[test]
fn layout_path_components_rejects_non_res_parent() {
    let rejected = PathBuf::from("app/src/main/assets/layout/foo_bar.xml");
    assert!(layout_path_components(&rejected).is_none());
}

#[test]
fn layout_path_components_nested_module_root() {
    let path = PathBuf::from("project/app/src/androidMain/res/layout-sw600dp/screen.xml");
    let components = layout_path_components(&path).expect("nested module");
    assert_eq!(components.module_root, PathBuf::from("project/app"));
    assert_eq!(components.variant_qualifier, "sw600dp");
}

#[test]
fn index_layout_content_populates_side_index() {
    let indexer = Indexer::new();
    let layout_path = PathBuf::from("app/src/main/res/layout/activity_main.xml");
    let uri = Url::from_file_path(std::env::current_dir().unwrap().join(&layout_path)).unwrap();
    indexer.index_layout_content(&uri, SAMPLE_LAYOUT);

    let data = indexer.layout_for_uri(uri.as_str()).expect("layout data");
    assert_eq!(data.layout_name, "activity_main");
    assert!(data.view_ids.iter().any(|view_id| view_id.id == "title"));
}

#[test]
fn remove_layout_clears_side_index_entry() {
    let indexer = Indexer::new();
    let layout_path = PathBuf::from("app/src/main/res/layout/activity_main.xml");
    let uri = Url::from_file_path(std::env::current_dir().unwrap().join(&layout_path)).unwrap();
    indexer.index_layout_content(&uri, SAMPLE_LAYOUT);
    indexer.remove_layout(&uri);
    assert!(indexer.layout_for_uri(uri.as_str()).is_none());
}

#[test]
fn layout_cache_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("workspace");
    fs::create_dir_all(&root).expect("mkdir workspace");

    let components = LayoutPathComponents {
        module_root: PathBuf::from("app"),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: String::new(),
    };
    let parsed = parse_layout_xml(SAMPLE_LAYOUT);
    let data = build_layout_file_data(&components, &parsed).expect("layout data");
    let layout_path = tmp.path().join("app/src/main/res/layout/foo_bar.xml");
    std::fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout dir");
    std::fs::write(&layout_path, SAMPLE_LAYOUT).expect("write layout");

    let indexer = Indexer::new();
    let uri = Url::from_file_path(&layout_path).expect("layout uri");
    indexer
        .viewbinding
        .layouts
        .insert(uri.to_string(), Arc::new(data));

    with_xdg_cache(tmp.path(), || {
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
        let entry = loaded
            .layouts
            .get(&layout_path.to_string_lossy().to_string())
            .expect("layout cache entry");
        assert_eq!(entry.data.layout_name, "foo_bar");
        assert!(entry
            .data
            .view_ids
            .iter()
            .any(|view_id| view_id.id == "title"));
    });
}

#[test]
fn layout_cache_restore_populates_secondary_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, SAMPLE_LAYOUT).expect("write layout");

    let warm_indexer = Indexer::new();
    let uri = Url::from_file_path(&layout_path).expect("layout uri");
    warm_indexer.index_layout_content(&uri, SAMPLE_LAYOUT);

    with_xdg_cache(temp.path(), || {
        save_cache(
            root,
            &warm_indexer.files,
            &warm_indexer.content_hashes,
            &warm_indexer.library_uris,
            &warm_indexer.viewbinding.layouts,
            &warm_indexer.viewbinding.generated_bindings,
            true,
            true,
        );

        let loaded = try_load_cache(root).expect("cache loaded");
        let path_string = layout_path.to_string_lossy().to_string();
        let cache_entry = loaded
            .layouts
            .get(&path_string)
            .expect("layout cache entry");

        let restored_indexer = Indexer::new();
        let uri_string = uri.to_string();
        restored_indexer
            .viewbinding
            .layouts
            .insert(uri_string.clone(), Arc::clone(&cache_entry.data));
        restored_indexer
            .viewbinding
            .insert_layout_secondary_index(&uri_string, &cache_entry.data);

        let key = (
            cache_entry.data.module_root.clone(),
            cache_entry.data.layout_name.clone(),
        );
        let uris = restored_indexer
            .viewbinding
            .layouts_by_module_and_name
            .get(&key)
            .expect("secondary index populated on warm restore");
        assert_eq!(uris.len(), 1);
        assert_eq!(uris[0], uri_string);

        let layouts = restored_indexer
            .layouts_for_binding_class("FooBarBinding", cache_entry.data.module_root.as_path());
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].layout_name, "foo_bar");
    });
}

#[test]
fn insert_layout_secondary_index_dedups_reindex() {
    let indexer = Indexer::new();
    let components = LayoutPathComponents {
        module_root: PathBuf::from("app"),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: String::new(),
    };
    let parsed = parse_layout_xml(SAMPLE_LAYOUT);
    let data = build_layout_file_data(&components, &parsed).expect("layout data");
    let uri = "file:///app/src/main/res/layout/foo_bar.xml";

    indexer
        .viewbinding
        .insert_layout_secondary_index(uri, &data);
    indexer
        .viewbinding
        .insert_layout_secondary_index(uri, &data);

    let key = (PathBuf::from("app"), "foo_bar".to_string());
    let uris = indexer
        .viewbinding
        .layouts_by_module_and_name
        .get(&key)
        .expect("secondary index entry");
    assert_eq!(uris.len(), 1);
}

#[test]
fn ensure_module_layouts_indexed_retries_until_layouts_exist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    fs::create_dir_all(module_root.join("src/main/kotlin")).expect("mkdir kotlin");

    let indexer = Indexer::new();
    assert_eq!(
        indexer.index_module_layouts_blocking(&module_root),
        0,
        "no layout dirs yet"
    );
    assert!(
        !indexer
            .viewbinding
            .layouts_indexed_modules
            .contains(&module_root),
        "must not mark module done when walk found zero layout files"
    );

    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, SAMPLE_LAYOUT).expect("write layout");

    assert_eq!(
        indexer.index_module_layouts_blocking(&module_root),
        1,
        "layout file appeared — on-demand path must index it"
    );
    assert!(
        indexer
            .viewbinding
            .layouts_indexed_modules
            .contains(&module_root),
        "module marked done once layout files exist on disk"
    );
    assert_eq!(
        indexer.index_module_layouts_blocking(&module_root),
        0,
        "second call is a no-op"
    );
}

#[test]
fn layout_position_helpers_use_side_index_ranges() {
    let parsed = parse_layout_xml(SAMPLE_LAYOUT);
    let components = LayoutPathComponents {
        module_root: PathBuf::from("app"),
        layout_name: "foo_bar".to_string(),
        variant_qualifier: String::new(),
    };
    let layout_data = build_layout_file_data(&components, &parsed).expect("layout data");
    let title = parsed
        .view_ids
        .iter()
        .find(|view_id| view_id.id == "title")
        .expect("title");

    let view_id = view_id_at_layout_position(&layout_data, title.id_attribute_range.start)
        .expect("view id at attribute");
    assert_eq!(view_id, "title");

    let tag_name =
        element_tag_at_layout_position(&layout_data, title.tag_range.start).expect("tag name");
    assert_eq!(tag_name, "TextView");

    let decl_position =
        id_attribute_position_for_view_id(&layout_data, "title").expect("decl position");
    assert_eq!(decl_position, title.id_attribute_range.start);
}
