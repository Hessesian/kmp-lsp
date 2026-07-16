//! ViewBinding diagnostics tests (PR 6).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::{DiagnosticSeverity, Url};

use crate::indexer::Indexer;
use crate::viewbinding::{stale_binding_field_diagnostics, viewbinding_import_diagnostics};

const FOO_BAR_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:tools="http://schemas.android.com/tools"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const FOO_BAR_LAYOUT_IGNORE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:tools="http://schemas.android.com/tools"
    tools:viewBindingIgnore="true"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const FOO_BAR_BINDING_WITH_STALE: &str = r#"package com.example.app.databinding;

import android.widget.TextView;

public final class FooBarBinding {
    public final TextView title;
    public final TextView oldField;
}
"#;

struct DiagnosticsFixture {
    _temp: tempfile::TempDir,
    module_root: PathBuf,
    kotlin_uri: Url,
    indexer: Arc<Indexer>,
}

impl DiagnosticsFixture {
    fn build(layout_xml: &str, include_binding_java: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");
        let layout_path = layout_dir.join("foo_bar.xml");
        fs::write(&layout_path, layout_xml).expect("write layout");

        if include_binding_java {
            let binding_path = module_root
                .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
            fs::create_dir_all(binding_path.parent().unwrap()).expect("mkdir binding");
            fs::write(&binding_path, FOO_BAR_BINDING_WITH_STALE).expect("write binding");
        }

        let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        binding.title
        binding.oldField
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

        let indexer = Arc::new(Indexer::new());
        let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");

        indexer.index_layout_content(&layout_uri, layout_xml);
        if include_binding_java {
            indexer.index_generated_bindings(&module_root, None);
        }
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            module_root,
            kotlin_uri,
            indexer,
        }
    }
}

#[test]
fn import_diagnostic_resolves_aliased_binding_import_by_full_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_dir = module_root.join("src/main/res/layout");
    fs::create_dir_all(&layout_dir).expect("mkdir layout");
    let layout_path = layout_dir.join("foo_bar.xml");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("write layout");

    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding as ScreenBinding

class MainActivity {
    fun demo(screen: ScreenBinding) {
        screen.title
    }
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_content(&kotlin_uri, kotlin_source);

    let diags = viewbinding_import_diagnostics(&indexer, &kotlin_uri);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("ViewBinding class not generated"));
}

#[test]
fn import_diagnostic_when_layout_exists_but_no_generated_class() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, false);
    let diags = viewbinding_import_diagnostics(&fixture.indexer, &fixture.kotlin_uri);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    assert!(diags[0].message.contains("ViewBinding class not generated"));
}

#[test]
fn import_diagnostic_view_binding_ignore_only_when_all_variants_opt_out() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let default_dir = module_root.join("src/main/res/layout");
    let land_dir = module_root.join("src/main/res/layout-land");
    fs::create_dir_all(&default_dir).expect("mkdir default");
    fs::create_dir_all(&land_dir).expect("mkdir land");
    fs::write(default_dir.join("foo_bar.xml"), FOO_BAR_LAYOUT).expect("write default");
    fs::write(land_dir.join("foo_bar.xml"), FOO_BAR_LAYOUT_IGNORE).expect("write land ignore");

    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) = binding.title
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let default_uri = Url::from_file_path(default_dir.join("foo_bar.xml")).expect("default uri");
    let land_uri = Url::from_file_path(land_dir.join("foo_bar.xml")).expect("land uri");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_layout_content(&default_uri, FOO_BAR_LAYOUT);
    indexer.index_layout_content(&land_uri, FOO_BAR_LAYOUT_IGNORE);
    indexer.index_content(&kotlin_uri, kotlin_source);

    let diags = viewbinding_import_diagnostics(&indexer, &kotlin_uri);
    assert!(
        diags
            .iter()
            .all(|diag| !diag.message.contains("viewBindingIgnore")),
        "partial variant opt-out must not emit viewBindingIgnore warning: {diags:?}"
    );
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("ViewBinding class not generated"));
}

#[test]
fn import_diagnostic_view_binding_ignore_takes_precedence() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT_IGNORE, false);
    let diags = viewbinding_import_diagnostics(&fixture.indexer, &fixture.kotlin_uri);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("viewBindingIgnore"));
    assert!(!diags[0].message.contains("build the project"));
}

#[test]
fn import_diagnostic_absent_when_generated_class_present() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    let diags = viewbinding_import_diagnostics(&fixture.indexer, &fixture.kotlin_uri);
    assert!(
        diags.is_empty(),
        "expected no import diagnostics: {diags:?}"
    );
}

#[test]
fn stale_field_diagnostic_when_id_removed_from_layout() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    let document = fixture
        .indexer
        .live_doc(&fixture.kotlin_uri)
        .expect("live doc");
    let diags = stale_binding_field_diagnostics(&fixture.indexer, &fixture.kotlin_uri, &document);
    let stale = diags
        .iter()
        .find(|diag| diag.message.contains("oldField"))
        .expect("stale diagnostic on oldField");
    assert_eq!(stale.severity, Some(DiagnosticSeverity::INFORMATION));
    assert!(!diags.iter().any(|diag| diag.message.contains("title")));
}

#[test]
fn stale_field_diagnostic_covers_bare_receiver_scope_member() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    let bare_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/BareUsage.kt");
    fs::create_dir_all(bare_path.parent().unwrap()).expect("mkdir bare usage");
    // Bare `oldField` inside `with(binding)` uses implicit `this` — its id was
    // removed from the layout, so it must be flagged just like `binding.oldField`.
    // `title` is still live and must stay silent.
    let bare_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun render(binding: FooBarBinding) {
    with(binding) {
        title
        oldField
    }
}
"#;
    fs::write(&bare_path, bare_source).expect("write bare usage");
    let bare_uri = Url::from_file_path(&bare_path).expect("bare uri");
    fixture.indexer.index_content(&bare_uri, bare_source);
    fixture.indexer.set_live_lines(&bare_uri, bare_source);
    fixture.indexer.store_live_tree(&bare_uri, bare_source);

    let document = fixture.indexer.live_doc(&bare_uri).expect("live doc");
    let diags = stale_binding_field_diagnostics(&fixture.indexer, &bare_uri, &document);

    let stale = diags
        .iter()
        .find(|diag| diag.message.contains("oldField"))
        .expect("bare stale field `oldField` inside with(binding) must be flagged");
    assert_eq!(stale.severity, Some(DiagnosticSeverity::INFORMATION));
    assert!(
        !diags.iter().any(|diag| diag.message.contains("title")),
        "live field `title` must not be flagged stale: {diags:?}"
    );
}

#[test]
fn stale_field_diagnostic_skipped_for_shadowed_bare_local() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    let shadow_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/ShadowUsage.kt");
    fs::create_dir_all(shadow_path.parent().unwrap()).expect("mkdir shadow usage");
    // A local `val oldField` shadows the binding field; the bare usage is the
    // local, so no stale-build diagnostic should fire.
    let shadow_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun render(binding: FooBarBinding) {
    with(binding) {
        val oldField = "local"
        println(oldField)
    }
}
"#;
    fs::write(&shadow_path, shadow_source).expect("write shadow usage");
    let shadow_uri = Url::from_file_path(&shadow_path).expect("shadow uri");
    fixture.indexer.index_content(&shadow_uri, shadow_source);
    fixture.indexer.set_live_lines(&shadow_uri, shadow_source);
    fixture.indexer.store_live_tree(&shadow_uri, shadow_source);

    let document = fixture.indexer.live_doc(&shadow_uri).expect("live doc");
    let diags = stale_binding_field_diagnostics(&fixture.indexer, &shadow_uri, &document);
    assert!(
        !diags.iter().any(|diag| diag.message.contains("oldField")),
        "a local val shadowing a binding field must not be flagged stale: {diags:?}"
    );
}

#[test]
fn stale_field_diagnostic_range_uses_utf16_with_multibyte_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("write layout");

    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    fs::create_dir_all(binding_path.parent().unwrap()).expect("mkdir binding");
    fs::write(&binding_path, FOO_BAR_BINDING_WITH_STALE).expect("write binding");

    let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        println("标题"); binding.oldField
    }
}
"#;
    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_generated_bindings(&module_root, None);
    indexer.index_content(&kotlin_uri, kotlin_source);
    indexer.set_live_lines(&kotlin_uri, kotlin_source);
    indexer.store_live_tree(&kotlin_uri, kotlin_source);

    let document = indexer.live_doc(&kotlin_uri).expect("live doc");
    let diags = stale_binding_field_diagnostics(&indexer, &kotlin_uri, &document);
    let stale = diags
        .iter()
        .find(|diag| diag.message.contains("oldField"))
        .expect("stale oldField diagnostic");

    let line_index = stale.range.start.line as usize;
    let line = kotlin_source.lines().nth(line_index).expect("source line");
    let field_start = line.find("oldField").expect("field on line");
    let expected_utf16 = line[..field_start]
        .chars()
        .map(|character| character.len_utf16())
        .sum::<usize>() as u32;
    assert_eq!(
        stale.range.start.character, expected_utf16,
        "stale diagnostic range must use UTF-16 columns (line: {line})"
    );
}

#[test]
fn stale_field_diagnostic_skipped_when_id_present() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    let document = fixture
        .indexer
        .live_doc(&fixture.kotlin_uri)
        .expect("live doc");
    let diags = stale_binding_field_diagnostics(&fixture.indexer, &fixture.kotlin_uri, &document);
    assert!(!diags.iter().any(|diag| diag.message.contains("title")));
}

#[test]
fn stale_field_diagnostic_pairs_generated_binding_to_current_module() {
    let temp = tempfile::tempdir().expect("tempdir");
    let indexer = Arc::new(Indexer::new());

    // Module `app`: layout without `oldField`, generated binding WITHOUT `oldField`.
    let app_module_root = temp.path().join("app");
    let app_layout_path = app_module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(app_layout_path.parent().unwrap()).expect("mkdir app layout");
    fs::write(&app_layout_path, FOO_BAR_LAYOUT).expect("write app layout");
    let app_binding_java = r#"package com.example.app.databinding;

import android.widget.TextView;

public final class FooBarBinding {
    public final TextView title;
}
"#;
    let app_binding_path = app_module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    fs::create_dir_all(app_binding_path.parent().unwrap()).expect("mkdir app binding");
    fs::write(&app_binding_path, app_binding_java).expect("write app binding");

    // Module `other`: same layout name and binding class name, but its generated
    // binding DOES declare `oldField` (misleading competitor for the app module).
    let other_module_root = temp.path().join("other");
    let other_layout_path = other_module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(other_layout_path.parent().unwrap()).expect("mkdir other layout");
    fs::write(&other_layout_path, FOO_BAR_LAYOUT).expect("write other layout");
    let other_binding_java = FOO_BAR_BINDING_WITH_STALE.replace(
        "package com.example.app.databinding;",
        "package com.example.other.databinding;",
    );
    let other_binding_path = other_module_root
        .join("build/generated/databinding/com/example/other/databinding/FooBarBinding.java");
    fs::create_dir_all(other_binding_path.parent().unwrap()).expect("mkdir other binding");
    fs::write(&other_binding_path, other_binding_java).expect("write other binding");

    let app_layout_uri = Url::from_file_path(&app_layout_path).expect("app layout uri");
    let other_layout_uri = Url::from_file_path(&other_layout_path).expect("other layout uri");
    indexer.index_layout_content(&app_layout_uri, FOO_BAR_LAYOUT);
    indexer.index_layout_content(&other_layout_uri, FOO_BAR_LAYOUT);
    indexer.index_generated_bindings(&app_module_root, None);
    indexer.index_generated_bindings(&other_module_root, None);

    let kotlin_source_template = |package_suffix: &str| {
        format!(
            r#"package com.example

import com.example.{package_suffix}.databinding.FooBarBinding

class MainActivity {{
    fun demo(binding: FooBarBinding) {{
        binding.title
        binding.oldField
    }}
}}
"#
        )
    };

    let app_kotlin_path = app_module_root.join("src/main/kotlin/com/example/AppActivity.kt");
    fs::create_dir_all(app_kotlin_path.parent().unwrap()).expect("mkdir app kotlin");
    let app_kotlin_source = kotlin_source_template("app");
    fs::write(&app_kotlin_path, &app_kotlin_source).expect("write app kotlin");
    let app_kotlin_uri = Url::from_file_path(&app_kotlin_path).expect("app kotlin uri");
    indexer.index_content(&app_kotlin_uri, &app_kotlin_source);
    indexer.set_live_lines(&app_kotlin_uri, &app_kotlin_source);
    indexer.store_live_tree(&app_kotlin_uri, &app_kotlin_source);

    let other_kotlin_path = other_module_root.join("src/main/kotlin/com/example/OtherActivity.kt");
    fs::create_dir_all(other_kotlin_path.parent().unwrap()).expect("mkdir other kotlin");
    let other_kotlin_source = kotlin_source_template("other");
    fs::write(&other_kotlin_path, &other_kotlin_source).expect("write other kotlin");
    let other_kotlin_uri = Url::from_file_path(&other_kotlin_path).expect("other kotlin uri");
    indexer.index_content(&other_kotlin_uri, &other_kotlin_source);
    indexer.set_live_lines(&other_kotlin_uri, &other_kotlin_source);
    indexer.store_live_tree(&other_kotlin_uri, &other_kotlin_source);

    // App module: `oldField` does not exist in ITS generated binding — the other
    // module's same-named binding must not trigger a stale-build diagnostic here.
    let app_document = indexer.live_doc(&app_kotlin_uri).expect("app live doc");
    let app_diagnostics = stale_binding_field_diagnostics(&indexer, &app_kotlin_uri, &app_document);
    assert!(
        app_diagnostics.is_empty(),
        "no stale diagnostic expected in app module: {app_diagnostics:?}"
    );

    // Other module: `oldField` exists in its own generated binding but the id is
    // gone from its layout — the stale diagnostic must still fire there.
    let other_document = indexer.live_doc(&other_kotlin_uri).expect("other live doc");
    let other_diagnostics =
        stale_binding_field_diagnostics(&indexer, &other_kotlin_uri, &other_document);
    assert!(
        other_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("oldField")),
        "stale diagnostic expected in other module: {other_diagnostics:?}"
    );
}

#[test]
fn xml_file_emits_no_diagnostics() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, false);
    let layout_uri =
        Url::from_file_path(fixture.module_root.join("src/main/res/layout/foo_bar.xml"))
            .expect("layout uri");
    assert!(
        crate::backend::helpers::is_xml_uri(&layout_uri),
        "layout URI must be classified as XML"
    );
    let import_diags = viewbinding_import_diagnostics(&fixture.indexer, &layout_uri);
    assert!(import_diags.is_empty());
}

#[test]
fn import_diagnostic_clears_after_binding_discovered() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, false);
    assert_eq!(
        viewbinding_import_diagnostics(&fixture.indexer, &fixture.kotlin_uri).len(),
        1
    );
    let binding_path = fixture
        .module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    fs::create_dir_all(binding_path.parent().unwrap()).expect("mkdir binding");
    fs::write(&binding_path, FOO_BAR_BINDING_WITH_STALE).expect("write binding");
    fixture
        .indexer
        .index_generated_bindings(&fixture.module_root, None);
    let diags = viewbinding_import_diagnostics(&fixture.indexer, &fixture.kotlin_uri);
    assert!(
        diags.is_empty(),
        "build-required should clear after discovery: {diags:?}"
    );
}

#[test]
fn import_diagnostic_absent_when_no_layout_and_no_generated_class() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"package com.example

import com.example.app.databinding.MissingBinding

class MainActivity {
    fun demo() {}
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_content(&kotlin_uri, kotlin_source);
    indexer.set_live_lines(&kotlin_uri, kotlin_source);
    indexer.store_live_tree(&kotlin_uri, kotlin_source);

    let diags = viewbinding_import_diagnostics(&indexer, &kotlin_uri);
    assert!(
        diags.is_empty(),
        "import with neither layout nor generated class must stay silent: {diags:?}"
    );
}

#[test]
fn generated_binding_discovered_read_helper() {
    let fixture = DiagnosticsFixture::build(FOO_BAR_LAYOUT, true);
    assert!(fixture
        .indexer
        .generated_binding_discovered(&fixture.module_root, "FooBarBinding"));
    assert!(!fixture
        .indexer
        .generated_binding_discovered(&fixture.module_root, "MissingBinding"));
}
