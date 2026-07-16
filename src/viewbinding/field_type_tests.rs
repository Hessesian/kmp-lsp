//! Tests for layout-XML-first ViewBinding field type resolution.

use std::fs;
use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use super::binding_field_type;
use crate::indexer::Indexer;
use crate::resolver::complete::complete_dot;
use crate::resolver::infer::{find_field_type_in_class_from, infer_receiver_type_at};
use crate::viewbinding::{binding_layout_completion_fields, infer_bare_binding_field_type};
use tower_lsp::lsp_types::Position;

const FOO_BAR_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/my_view"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />

    <include
        android:id="@+id/header"
        layout="@layout/view_header" />
</LinearLayout>
"#;

const FOO_BAR_LAYOUT_LAND: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <android.widget.Button
        android:id="@+id/my_view"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const FQ_TAG_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <androidx.recyclerview.widget.RecyclerView
        android:id="@+id/list"
        android:layout_width="match_parent"
        android:layout_height="match_parent" />
</LinearLayout>
"#;

const VIEW_HEADER_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const STALE_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.view.View;

public final class FooBarBinding {
    public final View myView;
    public final ViewHeaderBinding header;
    private final LinearLayout rootView;

    private FooBarBinding(View myView, ViewHeaderBinding header, LinearLayout rootView) {
        this.myView = myView;
        this.header = header;
        this.rootView = rootView;
    }
}
"#;

struct BindingFieldTypeFixture {
    _temp: tempfile::TempDir,
    kotlin_uri: Url,
    indexer: Arc<Indexer>,
}

impl BindingFieldTypeFixture {
    fn build(layout: &str, land_layout: Option<&str>, binding_java: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");

        let default_layout_path = layout_dir.join("foo_bar.xml");
        fs::write(&default_layout_path, layout).expect("write layout");

        if let Some(land) = land_layout {
            let layout_land_dir = module_root.join("src/main/res/layout-land");
            fs::create_dir_all(&layout_land_dir).expect("mkdir layout-land");
            fs::write(layout_land_dir.join("foo_bar.xml"), land).expect("write land layout");
        }

        let header_layout_path = layout_dir.join("view_header.xml");
        fs::write(&header_layout_path, VIEW_HEADER_LAYOUT).expect("write header layout");

        if let Some(java_source) = binding_java {
            let binding_java_path = module_root.join(
                "build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java",
            );
            fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
            fs::write(&binding_java_path, java_source).expect("write binding java");
        }

        let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        binding.myView
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let default_layout_uri = Url::from_file_path(&default_layout_path).expect("layout uri");
        let header_layout_uri = Url::from_file_path(&header_layout_path).expect("header uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");

        indexer.index_layout_content(&default_layout_uri, layout);
        if let Some(land) = land_layout {
            let land_path = module_root.join("src/main/res/layout-land/foo_bar.xml");
            let land_uri = Url::from_file_path(&land_path).expect("land uri");
            indexer.index_layout_content(&land_uri, land);
        }
        indexer.index_layout_content(&header_layout_uri, VIEW_HEADER_LAYOUT);
        if binding_java.is_some() {
            indexer.index_generated_bindings(&module_root, None);
        }
        indexer.index_content(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            kotlin_uri,
            indexer,
        }
    }
}

#[test]
fn binding_field_type_resolves_text_view_from_layout() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "myView"
        ),
        Some("TextView".to_string())
    );
    assert_eq!(
        find_field_type_in_class_from(
            &fixture.indexer,
            "FooBarBinding",
            "myView",
            &fixture.kotlin_uri
        ),
        Some("TextView".to_string())
    );
}

#[test]
fn binding_field_type_resolves_fq_tag_to_leaf_name() {
    let fixture = BindingFieldTypeFixture::build(FQ_TAG_LAYOUT, None, None);
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "list"
        ),
        Some("RecyclerView".to_string())
    );
}

#[test]
fn binding_field_type_resolves_include_chain_to_nested_binding() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "header"
        ),
        Some("ViewHeaderBinding".to_string())
    );
    assert_eq!(
        find_field_type_in_class_from(
            &fixture.indexer,
            "ViewHeaderBinding",
            "title",
            &fixture.kotlin_uri
        ),
        Some("TextView".to_string()),
        "include chain should resolve nested binding field type"
    );
}

#[test]
fn binding_field_type_variant_conflict_returns_view() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, Some(FOO_BAR_LAYOUT_LAND), None);
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "myView"
        ),
        Some("View".to_string())
    );
}

#[test]
fn binding_field_type_root_uses_root_tag() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "root"
        ),
        Some("LinearLayout".to_string())
    );
}

#[test]
fn binding_field_type_ignores_competing_module_layout() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);

    let other_module = fixture._temp.path().join("other");
    let other_layout_dir = other_module.join("src/main/res/layout");
    fs::create_dir_all(&other_layout_dir).expect("mkdir other layout");
    let other_layout_path = other_layout_dir.join("foo_bar.xml");
    let other_layout = r#"<?xml version="1.0" encoding="utf-8"?>
<FrameLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <ImageView
        android:id="@+id/my_view"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</FrameLayout>
"#;
    fs::write(&other_layout_path, other_layout).expect("write other layout");
    let other_layout_uri = Url::from_file_path(&other_layout_path).expect("other layout uri");
    fixture
        .indexer
        .index_layout_content(&other_layout_uri, other_layout);

    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "myView"
        ),
        Some("TextView".to_string()),
        "source file's own module layout must win over a same-named layout elsewhere"
    );
}

#[test]
fn binding_field_type_xml_wins_over_stale_generated_java() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, Some(STALE_BINDING_JAVA));
    assert_eq!(
        binding_field_type(
            &fixture.indexer,
            Some(&fixture.kotlin_uri),
            "FooBarBinding",
            "myView"
        ),
        Some("TextView".to_string()),
        "layout XML must not be overridden by stale generated Java declaring View"
    );
}

#[test]
fn binding_layout_completion_fields_without_generated_java() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let fields =
        binding_layout_completion_fields(&fixture.indexer, &fixture.kotlin_uri, "FooBarBinding");
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    assert!(names.contains(&"myView"), "expected myView in {names:?}");
    assert!(names.contains(&"header"), "expected header in {names:?}");
    assert!(names.contains(&"root"), "expected root in {names:?}");
}

#[test]
fn complete_dot_binding_lists_layout_fields_without_java() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let items = complete_dot(
        &fixture.indexer,
        "FooBarBinding",
        &fixture.kotlin_uri,
        false,
        None,
    );
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(
        labels.contains(&"myView"),
        "layout-only binding dot completion should include myView; got {labels:?}"
    );
    assert!(
        labels.contains(&"header"),
        "layout-only binding dot completion should include header; got {labels:?}"
    );
    assert!(
        labels.contains(&"root"),
        "layout-only binding dot completion should include root; got {labels:?}"
    );
}

#[test]
fn complete_dot_binding_dedups_layout_and_java_fields() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, Some(STALE_BINDING_JAVA));
    let items = complete_dot(
        &fixture.indexer,
        "FooBarBinding",
        &fixture.kotlin_uri,
        false,
        None,
    );
    let my_view_items: Vec<_> = items.iter().filter(|item| item.label == "myView").collect();
    assert_eq!(
        my_view_items.len(),
        1,
        "duplicate myView entries from layout and Java: {:?}",
        items
            .iter()
            .map(|item| (&item.label, &item.detail))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        my_view_items[0].detail.as_deref(),
        Some("TextView"),
        "layout XML detail should win over stale Java View"
    );
}

fn index_kotlin_with_source(indexer: &Indexer, uri: &Url, source: &str) {
    indexer.index_content(uri, source);
    indexer.set_live_lines(uri, source);
    indexer.store_live_tree(uri, source);
}

fn position_on_word(source: &str, word: &str) -> Position {
    let offset = source.find(word).expect("word in source");
    let mut line = 0_u32;
    let mut character = 0_u32;
    for (index, character_value) in source.char_indices() {
        if index == offset {
            break;
        }
        if character_value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += character_value.len_utf16() as u32;
        }
    }
    Position { line, character }
}

#[test]
fn bare_binding_field_type_inside_with_block() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        with(binding) {
            myView
        }
    }
}
"#;
    index_kotlin_with_source(&fixture.indexer, &fixture.kotlin_uri, source);
    let position = position_on_word(source, "myView");
    assert_eq!(
        infer_bare_binding_field_type(&fixture.indexer, &fixture.kotlin_uri, position, "myView"),
        Some("TextView".to_string())
    );
    assert_eq!(
        infer_receiver_type_at(&fixture.indexer, "myView", &fixture.kotlin_uri, position)
            .map(|receiver_type| receiver_type.leaf),
        Some("TextView".to_string())
    );
}

#[test]
fn bare_binding_field_type_inside_apply_block() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        binding.apply {
            myView
        }
    }
}
"#;
    index_kotlin_with_source(&fixture.indexer, &fixture.kotlin_uri, source);
    let position = position_on_word(source, "myView");
    assert_eq!(
        infer_bare_binding_field_type(&fixture.indexer, &fixture.kotlin_uri, position, "myView"),
        Some("TextView".to_string())
    );
}

#[test]
fn bare_binding_field_type_local_shadowing_wins() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        with(binding) {
            val title = "shadow"
            title
        }
    }
}
"#;
    index_kotlin_with_source(&fixture.indexer, &fixture.kotlin_uri, source);
    let shadow_use = source.rfind("title").expect("shadowed title use");
    let mut line = 0_u32;
    let mut character = 0_u32;
    for (index, character_value) in source.char_indices() {
        if index == shadow_use {
            break;
        }
        if character_value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += character_value.len_utf16() as u32;
        }
    }
    let position = Position { line, character };
    assert!(
        infer_bare_binding_field_type(&fixture.indexer, &fixture.kotlin_uri, position, "title")
            .is_none(),
        "local val shadowing must suppress binding-field inference"
    );
}

#[test]
fn bare_completion_inside_with_lists_binding_layout_fields() {
    let fixture = BindingFieldTypeFixture::build(FOO_BAR_LAYOUT, None, None);
    let source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        with(binding) {
            myV
        }
    }
}
"#;
    index_kotlin_with_source(&fixture.indexer, &fixture.kotlin_uri, source);
    let line = source
        .lines()
        .position(|line| line.contains("myV"))
        .expect("completion line") as u32;
    let character = source.lines().nth(line as usize).expect("line text").len() as u32;
    let (items, _) =
        fixture
            .indexer
            .completions(&fixture.kotlin_uri, Position::new(line, character), false);
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(
        labels.contains(&"myView"),
        "with(binding) bare completion should suggest layout fields; got {labels:?}"
    );
}
