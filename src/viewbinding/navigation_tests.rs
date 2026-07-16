//! ViewBinding navigation feature tests (PR 4).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Range, Url};

use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::features::hover::compute_hover;
use crate::features::implementation::find_implementation;
use crate::indexer::{Indexer, RequestParseCache};
use crate::parser::nullable_at_line;
use crate::viewbinding::{
    binding_field_hover_for_class, binding_field_in_generated_java, binding_field_in_live_layout,
    find_binding_field_definition, find_binding_field_references, find_binding_implementation,
    find_layout_xml_definition, find_layout_xml_implementation, find_layout_xml_references,
    format_binding_field_hover, java_field_type_from_detail,
    normalize_reference_location_to_utf16_for_test, remap_generated_binding_definitions,
    resolve_expected_binding_class, short_type_name,
};
use crate::viewbinding::{binding_field_name_to_id, binding_id_to_field_name};

async fn binding_field_references_for_test(
    indexer: &Indexer,
    expected_binding_class: &str,
    field_name: &str,
    uri: &Url,
    line: u32,
    include_decl: bool,
) -> Vec<Location> {
    let mut parse_cache = RequestParseCache::new();
    find_binding_field_references(
        indexer,
        &mut parse_cache,
        expected_binding_class,
        field_name,
        uri,
        line,
        include_decl,
    )
    .await
}

async fn layout_xml_references_for_test(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
    include_decl: bool,
) -> Option<Vec<Location>> {
    let mut parse_cache = RequestParseCache::new();
    find_layout_xml_references(indexer, &mut parse_cache, uri, position, include_decl).await
}

const FOO_BAR_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />

    <TextView
        android:id="@+id/subtitle"
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

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
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

const FOO_BAR_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.view.View;
import android.widget.TextView;
import androidx.annotation.Nullable;
import androidx.constraintlayout.widget.ConstraintLayout;

public final class FooBarBinding {
    public final TextView title;
    @Nullable
    public final TextView subtitle;
    public final ViewHeaderBinding header;
    private final ConstraintLayout rootView;

    private FooBarBinding(TextView title, TextView subtitle, ViewHeaderBinding header, ConstraintLayout rootView) {
        this.title = title;
        this.subtitle = subtitle;
        this.header = header;
        this.rootView = rootView;
    }

    public ConstraintLayout getRoot() {
        return rootView;
    }
}
"#;

const VIEW_HEADER_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.widget.TextView;

public final class ViewHeaderBinding {
    public final TextView title;
}
"#;

struct ViewBindingFixture {
    _temp: tempfile::TempDir,
    module_root: PathBuf,
    land_layout_uri: Url,
    binding_java_uri: Url,
    kotlin_uri: Url,
    kotlin_source: String,
    indexer: Arc<Indexer>,
}

impl ViewBindingFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        let layout_land_dir = module_root.join("src/main/res/layout-land");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");
        fs::create_dir_all(&layout_land_dir).expect("mkdir layout-land");

        let default_layout_path = layout_dir.join("foo_bar.xml");
        let land_layout_path = layout_land_dir.join("foo_bar.xml");
        let header_layout_path = layout_dir.join("view_header.xml");
        fs::write(&default_layout_path, FOO_BAR_LAYOUT).expect("write default layout");
        fs::write(&land_layout_path, FOO_BAR_LAYOUT_LAND).expect("write land layout");
        fs::write(&header_layout_path, VIEW_HEADER_LAYOUT).expect("write header layout");

        let binding_java_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java",
        );
        let header_binding_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/ViewHeaderBinding.java",
        );
        fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
        fs::write(&binding_java_path, FOO_BAR_BINDING_JAVA).expect("write binding java");
        fs::write(&header_binding_path, VIEW_HEADER_BINDING_JAVA).expect("write header binding");

        let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        binding.title
        binding.header.title
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let default_layout_uri = Url::from_file_path(&default_layout_path).expect("default uri");
        let land_layout_uri = Url::from_file_path(&land_layout_path).expect("land uri");
        let header_layout_uri = Url::from_file_path(&header_layout_path).expect("header uri");
        let binding_java_uri = Url::from_file_path(&binding_java_path).expect("binding uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");

        indexer.index_layout_content(&default_layout_uri, FOO_BAR_LAYOUT);
        indexer.index_layout_content(&land_layout_uri, FOO_BAR_LAYOUT_LAND);
        indexer.index_layout_content(&header_layout_uri, VIEW_HEADER_LAYOUT);
        indexer.index_generated_bindings(&module_root, None);
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            module_root,
            land_layout_uri,
            binding_java_uri,
            kotlin_uri,
            kotlin_source: kotlin_source.to_string(),
            indexer,
        }
    }

    fn cursor_context(word: &str, qualifier: Option<&str>) -> CursorContext {
        CursorContext {
            word: word.to_string(),
            qualifier: qualifier.map(str::to_string),
            contextual: None,
            lambda_decl: None,
        }
    }

    fn position_in(source: &str, needle: &str) -> Position {
        let offset = source.find(needle).expect("needle in source");
        let mut line = 0_u32;
        let mut character = 0_u32;
        for (index, ch) in source.char_indices() {
            if index == offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                character = 0;
            } else {
                character += ch.len_utf8() as u32;
            }
        }
        Position { line, character }
    }

    fn position_on_word_in_line(source: &str, line_needle: &str, word: &str) -> Position {
        let line_start = source.find(line_needle).expect("line needle in source");
        let word_start = source[line_start..].find(word).expect("word in line") + line_start;
        let mut line = 0_u32;
        let mut character = 0_u32;
        for (index, ch) in source.char_indices() {
            if index == word_start {
                break;
            }
            if ch == '\n' {
                line += 1;
                character = 0;
            } else {
                character += ch.len_utf8() as u32;
            }
        }
        Position { line, character }
    }
}

fn response_locations(response: Option<GotoDefinitionResponse>) -> Vec<Location> {
    match response {
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_range,
            })
            .collect(),
        None => Vec::new(),
    }
}

fn uri_path_string(location: &Location) -> String {
    location
        .uri
        .to_file_path()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

#[test]
fn binding_field_name_to_id_inverts_layout_id_mapping() {
    assert_eq!(binding_field_name_to_id("fooBar"), "foo_bar");
    assert_eq!(binding_id_to_field_name("foo_bar"), "fooBar");
    assert_eq!(binding_field_name_to_id("title"), "title");
    assert_eq!(binding_id_to_field_name("title"), "title");
    assert_eq!(binding_field_name_to_id("header"), "header");
}

#[test]
fn short_type_name_strips_package_prefix() {
    assert_eq!(short_type_name("android.widget.TextView"), "TextView");
    assert_eq!(short_type_name("ViewHeaderBinding"), "ViewHeaderBinding");
}

#[test]
fn format_binding_field_hover_renders_nullable_and_non_nullable() {
    let non_null = format_binding_field_hover("title", "android.widget.TextView", false);
    assert!(non_null.contains("val title: TextView"));
    assert!(!non_null.contains("android.widget"));
    let nullable = format_binding_field_hover("subtitle", "TextView", true);
    assert!(nullable.contains("val subtitle: TextView?"));
}

#[test]
fn java_field_type_from_detail_extracts_type() {
    assert_eq!(
        java_field_type_from_detail("public final TextView title", "title"),
        Some("TextView".to_string())
    );
    assert_eq!(
        java_field_type_from_detail("public final ViewHeaderBinding header", "header"),
        Some("ViewHeaderBinding".to_string())
    );
}

#[test]
fn nullable_at_line_detects_annotation_above_field() {
    let lines = vec![
        "import androidx.annotation.Nullable;".to_string(),
        "@Nullable".to_string(),
        "public final TextView subtitle;".to_string(),
    ];
    assert!(nullable_at_line(&lines, 2));
    assert!(!nullable_at_line(
        &["public final TextView title;".to_string()],
        0
    ));
}

#[tokio::test]
async fn hover_on_binding_field_renders_kotlin_style() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let context = ViewBindingFixture::cursor_context("title", Some("binding"));
    let hover = compute_hover(
        fixture.indexer.as_ref(),
        &context,
        &fixture.kotlin_uri,
        position,
    )
    .expect("hover");
    let markdown = match hover.contents {
        tower_lsp::lsp_types::HoverContents::Markup(content) => content.value,
        _ => panic!("expected markdown hover"),
    };
    assert!(markdown.contains("val title: TextView"));
    assert!(!markdown.contains("android.widget"));
}

#[tokio::test]
async fn hover_on_nullable_binding_field_shows_question_mark() {
    let fixture = ViewBindingFixture::build();
    let kotlin_source = fixture
        .kotlin_source
        .replace("binding.title", "binding.subtitle");
    fixture
        .indexer
        .index_content(&fixture.kotlin_uri, &kotlin_source);
    fixture
        .indexer
        .set_live_lines(&fixture.kotlin_uri, &kotlin_source);
    fixture
        .indexer
        .store_live_tree(&fixture.kotlin_uri, &kotlin_source);

    let position = ViewBindingFixture::position_in(&kotlin_source, "binding.subtitle");
    let context = ViewBindingFixture::cursor_context("subtitle", Some("binding"));
    let hover = compute_hover(
        fixture.indexer.as_ref(),
        &context,
        &fixture.kotlin_uri,
        position,
    )
    .expect("hover");
    let markdown = match hover.contents {
        tower_lsp::lsp_types::HoverContents::Markup(content) => content.value,
        _ => panic!("expected markdown hover"),
    };
    assert!(markdown.contains("val subtitle: TextView?"));
}

#[test]
fn binding_field_hover_for_class_reads_nullable_flag() {
    let fixture = ViewBindingFixture::build();
    let hover = binding_field_hover_for_class(
        &fixture.indexer,
        &fixture.kotlin_uri,
        "FooBarBinding",
        "subtitle",
    )
    .expect("binding hover");
    assert!(hover.contains("TextView?"));
}

#[test]
fn binding_field_hover_pairs_binding_class_to_importing_module() {
    let fixture = ViewBindingFixture::build();

    // Competing module with a same-named binding class whose `title` has a
    // DIFFERENT field type — hover must never leak it into the app module.
    let other_module = fixture.module_root.parent().unwrap().join("other");
    let other_binding_java = r#"package com.example.other.databinding;

import android.widget.Button;

public final class FooBarBinding {
    public final Button title;
}
"#;
    let other_binding_path = other_module
        .join("build/generated/databinding/com/example/other/databinding/FooBarBinding.java");
    fs::create_dir_all(other_binding_path.parent().unwrap()).expect("mkdir other binding");
    fs::write(&other_binding_path, other_binding_java).expect("write other binding");
    fixture
        .indexer
        .index_generated_bindings(&other_module, None);

    // App-module file importing the app-module binding: field type comes from
    // the app module's generated Java (TextView, not Button).
    let app_hover = binding_field_hover_for_class(
        &fixture.indexer,
        &fixture.kotlin_uri,
        "FooBarBinding",
        "title",
    )
    .expect("app module hover");
    assert!(app_hover.contains("TextView"), "got: {app_hover}");
    assert!(!app_hover.contains("Button"), "got: {app_hover}");

    // App-module file importing the OTHER module's binding: the import package
    // must win over the file's own module root.
    let cross_module_source = r#"package com.example

import com.example.other.databinding.FooBarBinding

fun crossModule(binding: FooBarBinding) {
    binding.title
}
"#;
    let cross_module_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/CrossModule.kt");
    fs::write(&cross_module_path, cross_module_source).expect("write cross module");
    let cross_module_uri = Url::from_file_path(&cross_module_path).expect("cross module uri");
    fixture
        .indexer
        .index_content(&cross_module_uri, cross_module_source);

    let cross_module_hover = binding_field_hover_for_class(
        &fixture.indexer,
        &cross_module_uri,
        "FooBarBinding",
        "title",
    )
    .expect("cross module hover");
    assert!(
        cross_module_hover.contains("Button"),
        "got: {cross_module_hover}"
    );
}

#[tokio::test]
async fn binding_field_references_find_qualified_usages() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let context = ViewBindingFixture::cursor_context("title", Some("binding"));
    let expected = resolve_expected_binding_class(
        &fixture.indexer,
        &fixture.kotlin_uri,
        position,
        &context,
        None,
    )
    .expect("expected binding class");
    assert_eq!(expected, "FooBarBinding");

    let references = binding_field_references_for_test(
        &fixture.indexer,
        &expected,
        "title",
        &fixture.kotlin_uri,
        position.line,
        false,
    )
    .await;
    assert!(!references.is_empty());
    assert!(references.iter().all(|location| !fixture
        .indexer
        .is_generated_binding_uri(location.uri.as_str())));
}

#[tokio::test]
async fn binding_field_references_exclude_misleading_competitor() {
    let fixture = ViewBindingFixture::build();
    let competitor_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/Competitor.kt");
    fs::create_dir_all(competitor_path.parent().unwrap()).expect("mkdir");
    let competitor_source = r#"package com.example

class Competitor {
    val title: String = "misleading"
}

fun use(competitor: Competitor) {
    competitor.title
}
"#;
    fs::write(&competitor_path, competitor_source).expect("write competitor");
    let competitor_uri = Url::from_file_path(&competitor_path).expect("competitor uri");
    fixture
        .indexer
        .index_content(&competitor_uri, competitor_source);

    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let references = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "title",
        &fixture.kotlin_uri,
        position.line,
        false,
    )
    .await;
    assert!(references
        .iter()
        .all(|location| !location.uri.as_str().contains("Competitor.kt")));
}

#[tokio::test]
async fn binding_field_references_scope_rg_to_binding_importers() {
    let fixture = ViewBindingFixture::build();
    let noise_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/TitleNoise.kt");
    fs::create_dir_all(noise_path.parent().unwrap()).expect("mkdir");
    let noise_source = r#"package com.example

class TitleNoise {
    fun noisy() {
        val title = "many title mentions without binding import"
        println(title)
    }
}
"#;
    fs::write(&noise_path, noise_source).expect("write noise");
    let noise_uri = Url::from_file_path(&noise_path).expect("noise uri");
    fixture.indexer.index_content(&noise_uri, noise_source);

    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let references = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "title",
        &fixture.kotlin_uri,
        position.line,
        false,
    )
    .await;

    assert!(
        references
            .iter()
            .all(|location| !location.uri.as_str().contains("TitleNoise.kt")),
        "rg must not search files outside the binding import graph: {references:?}"
    );
}

#[tokio::test]
async fn binding_field_references_still_verify_when_id_removed_from_layout() {
    let fixture = ViewBindingFixture::build();
    let stale_layout = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />

    <include
        android:id="@+id/header"
        layout="@layout/view_header" />
</LinearLayout>
"#;
    let default_layout_path = fixture.module_root.join("src/main/res/layout/foo_bar.xml");
    fs::write(&default_layout_path, stale_layout).expect("write stale layout");
    let default_layout_uri = Url::from_file_path(&default_layout_path).expect("layout uri");
    fixture
        .indexer
        .index_layout_content(&default_layout_uri, stale_layout);

    assert!(
        !binding_field_in_live_layout(
            &fixture.indexer,
            "FooBarBinding",
            "subtitle",
            &fixture.kotlin_uri,
        ),
        "subtitle id removed from layout"
    );
    assert!(
        binding_field_in_generated_java(
            &fixture.indexer,
            "FooBarBinding",
            "subtitle",
            &fixture.kotlin_uri,
        ),
        "subtitle still in generated Java"
    );

    let kotlin_with_stale_field = r#"package com.example

import com.example.app.databinding.FooBarBinding

class StaleFieldUsage {
    fun demo(binding: FooBarBinding) {
        binding.subtitle
    }
}
"#;
    let stale_usage_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/StaleFieldUsage.kt");
    fs::create_dir_all(stale_usage_path.parent().unwrap()).expect("mkdir");
    fs::write(&stale_usage_path, kotlin_with_stale_field).expect("write stale usage");
    let stale_usage_uri = Url::from_file_path(&stale_usage_path).expect("stale usage uri");
    fixture
        .indexer
        .index_content(&stale_usage_uri, kotlin_with_stale_field);

    let position = ViewBindingFixture::position_in(kotlin_with_stale_field, "binding.subtitle");
    let references = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "subtitle",
        &stale_usage_uri,
        position.line,
        false,
    )
    .await;
    assert!(
        !references.is_empty(),
        "references must still be receiver-verified when field is stale in layout but present in Java"
    );
    assert!(references.iter().any(|location| {
        location.uri.as_str().contains("StaleFieldUsage.kt")
            && location.range.start.line == position.line
    }));
}

#[tokio::test]
async fn binding_field_references_include_contextual_this_and_it_receivers() {
    let fixture = ViewBindingFixture::build();
    let scope_function_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/ScopeFunctions.kt");
    fs::create_dir_all(scope_function_path.parent().unwrap()).expect("mkdir scope functions");
    let scope_function_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun applyBlock(binding: FooBarBinding) {
    binding.apply { this.title }
}

fun alsoBlock(binding: FooBarBinding) {
    binding.also { it.title }
}

class NotABinding {
    val title: String = "misleading"
}

fun misleadingApply(competitor: NotABinding) {
    competitor.apply { this.title }
}
"#;
    fs::write(&scope_function_path, scope_function_source).expect("write scope functions");
    let scope_function_uri = Url::from_file_path(&scope_function_path).expect("scope uri");
    fixture
        .indexer
        .index_content(&scope_function_uri, scope_function_source);

    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let references = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "title",
        &fixture.kotlin_uri,
        position.line,
        false,
    )
    .await;

    let this_receiver_line =
        ViewBindingFixture::position_in(scope_function_source, "this.title").line;
    let it_receiver_line = ViewBindingFixture::position_in(scope_function_source, "it.title").line;
    let misleading_line =
        ViewBindingFixture::position_in(scope_function_source, "competitor.apply").line;

    let lines_in_scope_file: Vec<u32> = references
        .iter()
        .filter(|location| location.uri == scope_function_uri)
        .map(|location| location.range.start.line)
        .collect();
    assert!(
        lines_in_scope_file.contains(&this_receiver_line),
        "`this.title` inside `binding.apply` must verify as a binding reference, got lines: {lines_in_scope_file:?}"
    );
    assert!(
        lines_in_scope_file.contains(&it_receiver_line),
        "`it.title` inside `binding.also` must verify as a binding reference, got lines: {lines_in_scope_file:?}"
    );
    assert!(
        !lines_in_scope_file.contains(&misleading_line),
        "`this.title` on a non-binding receiver must be excluded, got lines: {lines_in_scope_file:?}"
    );
}

#[tokio::test]
async fn xml_references_match_kotlin_side() {
    let fixture = ViewBindingFixture::build();
    let default_layout_uri = fixture
        .indexer
        .layout_uris_for_binding_class("FooBarBinding", &fixture.module_root)
        .into_iter()
        .find(|(_uri, data)| data.variant_qualifier.is_empty())
        .map(|(uri, _data)| Url::parse(&uri).expect("layout uri"))
        .expect("default layout uri");

    let xml_position = ViewBindingFixture::position_in(FOO_BAR_LAYOUT, "@+id/title");
    let xml_refs =
        layout_xml_references_for_test(&fixture.indexer, &default_layout_uri, xml_position, false)
            .await
            .expect("xml references");

    let kotlin_position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let kotlin_refs = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "title",
        &fixture.kotlin_uri,
        kotlin_position.line,
        false,
    )
    .await;

    let mut xml_set: Vec<_> = xml_refs
        .iter()
        .map(|location| (location.uri.as_str(), location.range.start.line))
        .collect();
    let mut kotlin_set: Vec<_> = kotlin_refs
        .iter()
        .map(|location| (location.uri.as_str(), location.range.start.line))
        .collect();
    xml_set.sort();
    kotlin_set.sort();
    assert_eq!(xml_set, kotlin_set);
}

#[tokio::test]
async fn definition_on_binding_type_remaps_to_layout_variants_default_first() {
    let fixture = ViewBindingFixture::build();
    let binding_locs =
        fixture
            .indexer
            .find_definition_qualified("FooBarBinding", None, &fixture.kotlin_uri);
    assert!(
        !binding_locs.is_empty(),
        "FooBarBinding must resolve to generated Java before remap"
    );

    let context = ViewBindingFixture::cursor_context("FooBarBinding", None);
    let position =
        ViewBindingFixture::position_in(&fixture.kotlin_source, "binding: FooBarBinding");
    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 2);
    assert_eq!(uri_path_string(&locations[0]), "foo_bar.xml");
    assert_eq!(locations[0].range.start.line, 0);
    assert_eq!(locations[0].range.start.character, 0);
    assert!(locations[1].uri.as_str().contains("layout-land"));
}

#[tokio::test]
async fn definition_on_binding_field_remaps_to_view_id_in_all_variants() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");

    let context = ViewBindingFixture::cursor_context("title", Some("binding"));
    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 2);
    assert!(locations
        .iter()
        .all(|location| uri_path_string(location) == "foo_bar.xml"));
    assert!(locations
        .iter()
        .all(|location| location.range.start.line > 0));
}

#[tokio::test]
async fn definition_on_include_field_remaps_to_include_tag() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.header");

    let context = ViewBindingFixture::cursor_context("header", Some("binding"));
    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 1);
    assert_eq!(uri_path_string(&locations[0]), "foo_bar.xml");
}

#[tokio::test]
async fn definition_on_chained_include_field_remaps_recursively() {
    let fixture = ViewBindingFixture::build();
    let kotlin_source = r#"package com.example

import com.example.app.databinding.ViewHeaderBinding

fun onHeader(header: ViewHeaderBinding) {
    header.title
}
"#;
    let kotlin_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/HeaderUsage.kt");
    fs::write(&kotlin_path, kotlin_source).expect("write header usage");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("header usage uri");
    fixture.indexer.index_content(&kotlin_uri, kotlin_source);
    fixture.indexer.set_live_lines(&kotlin_uri, kotlin_source);
    fixture.indexer.store_live_tree(&kotlin_uri, kotlin_source);

    let position = ViewBindingFixture::position_in(kotlin_source, "header.title");
    let context = ViewBindingFixture::cursor_context("title", Some("header"));
    let response = find_definition(&context, &*fixture.indexer, &kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 1);
    assert_eq!(uri_path_string(&locations[0]), "view_header.xml");
}

#[tokio::test]
async fn definition_on_get_root_remaps_to_root_tag() {
    let fixture = ViewBindingFixture::build();
    let java_locations =
        fixture
            .indexer
            .find_definition_qualified("getRoot", None, &fixture.binding_java_uri);
    assert!(!java_locations.is_empty());

    let remapped = remap_generated_binding_definitions(&*fixture.indexer, java_locations);
    assert!(!remapped.is_empty());
    assert!(remapped
        .iter()
        .all(|location| uri_path_string(location) == "foo_bar.xml"));
    assert!(remapped[0].range.start.line <= 1);
}

#[tokio::test]
async fn implementation_on_binding_type_returns_raw_java() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "FooBarBinding");
    let context = ViewBindingFixture::cursor_context("FooBarBinding", None);

    let response = find_implementation(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("implementation response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 1);
    assert!(locations[0].uri.as_str().contains("FooBarBinding.java"));
    assert!(locations[0].uri.as_str().contains("build/"));
}

#[test]
fn hand_written_binding_class_is_not_remapped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let handwritten_path = module_root.join("src/main/java/com/example/FooBarBinding.java");
    fs::create_dir_all(handwritten_path.parent().unwrap()).expect("mkdir java");
    let source = "package com.example;\npublic class FooBarBinding {}\n";
    fs::write(&handwritten_path, source).expect("write handwritten");

    let indexer = Indexer::new();
    let uri = Url::from_file_path(&handwritten_path).expect("uri");
    indexer.index_content(&uri, source);

    let locations = indexer.find_definition_qualified("FooBarBinding", None, &uri);
    let remapped = remap_generated_binding_definitions(&indexer, locations.clone());
    assert_eq!(remapped, locations);
    assert!(!indexer.is_generated_binding_uri(uri.as_str()));
}

#[test]
fn wrong_module_pairing_does_not_remap_to_other_module_layout() {
    let fixture = ViewBindingFixture::build();
    let other_module = fixture.module_root.parent().unwrap().join("other");
    let other_layout = other_module.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(other_layout.parent().unwrap()).expect("mkdir other layout");
    fs::write(&other_layout, FOO_BAR_LAYOUT).expect("write other layout");
    let other_uri = Url::from_file_path(&other_layout).expect("other uri");
    fixture
        .indexer
        .index_layout_content(&other_uri, FOO_BAR_LAYOUT);

    let binding_locations =
        fixture
            .indexer
            .find_definition_qualified("FooBarBinding", None, &fixture.kotlin_uri);
    let remapped = remap_generated_binding_definitions(&*fixture.indexer, binding_locations);

    assert!(remapped
        .iter()
        .all(|location| !location.uri.as_str().contains("/other/")));
}

#[tokio::test]
async fn contextual_receiver_definition_remaps_identically() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("layout");

    let binding_path = module_root
        .join("build/generated/databinding/com/example/app/databinding/FooBarBinding.java");
    fs::create_dir_all(binding_path.parent().unwrap()).expect("mkdir binding");
    fs::write(&binding_path, FOO_BAR_BINDING_JAVA).expect("binding");

    let kotlin_path = module_root.join("src/main/kotlin/com/example/Context.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun withBlock(binding: FooBarBinding) {
    binding.title
}

fun applyBlock(binding: FooBarBinding) {
    binding.apply { it.title }
}

fun runBlock(binding: FooBarBinding) {
    binding.run { it.title }
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_generated_bindings(&module_root, None);
    indexer.index_content(&kotlin_uri, kotlin_source);
    indexer.set_live_lines(&kotlin_uri, kotlin_source);
    indexer.store_live_tree(&kotlin_uri, kotlin_source);

    for (needle, qualifier) in [("binding.title", Some("binding")), ("it.title", None)] {
        let position = ViewBindingFixture::position_in(kotlin_source, needle);
        let context = if let Some(qualifier) = qualifier {
            ViewBindingFixture::cursor_context("title", Some(qualifier))
        } else {
            CursorContext::build_with_cache(&indexer, &kotlin_uri, position, None)
                .expect("cursor context")
        };
        let response = find_definition(&context, &*indexer, &kotlin_uri, position)
            .await
            .expect("definition for contextual receiver");
        let locations = response_locations(Some(response));
        assert_eq!(locations.len(), 1, "expected remap for {needle}");
        assert_eq!(uri_path_string(&locations[0]), "foo_bar.xml");
    }
}

#[test]
fn xml_definition_on_view_id_returns_all_variants() {
    let fixture = ViewBindingFixture::build();
    let layout_source =
        fs::read_to_string(fixture.land_layout_uri.to_file_path().unwrap()).unwrap();
    let position = ViewBindingFixture::position_in(&layout_source, "@+id/title");

    let response =
        find_layout_xml_definition(&*fixture.indexer, &fixture.land_layout_uri, position);
    let locations = response_locations(response);

    assert_eq!(locations.len(), 2);
    assert!(locations
        .iter()
        .any(|location| location.uri.as_str().contains("layout-land")));
}

#[test]
fn xml_implementation_on_fqn_tag_resolves_custom_class() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/custom.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    let layout_source = r#"<?xml version="1.0" encoding="utf-8"?>
<com.example.widgets.CustomView xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent" />
"#;
    fs::write(&layout_path, layout_source).expect("layout");

    let custom_java = module_root.join("src/main/java/com/example/widgets/CustomView.java");
    fs::create_dir_all(custom_java.parent().unwrap()).expect("mkdir java");
    let custom_source = "package com.example.widgets;\npublic class CustomView {}\n";
    fs::write(&custom_java, custom_source).expect("custom java");

    let indexer = Indexer::new();
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let custom_uri = Url::from_file_path(&custom_java).expect("custom uri");
    indexer.index_layout_content(&layout_uri, layout_source);
    indexer.index_content(&custom_uri, custom_source);

    let position = ViewBindingFixture::position_in(layout_source, "com.example.widgets.CustomView");
    let response = find_layout_xml_implementation(&indexer, &layout_uri, position);
    let locations = response_locations(response);

    assert_eq!(locations.len(), 1);
    assert!(locations[0].uri.as_str().contains("CustomView.java"));
}

#[test]
fn xml_implementation_on_bare_tag_with_sdk_resolves_text_view() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/widget.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    let layout_source = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;
    fs::write(&layout_path, layout_source).expect("layout");

    let sdk_root = temp.path().join("android-sdk");
    let text_view_path = sdk_root.join("android/widget/TextView.java");
    fs::create_dir_all(text_view_path.parent().unwrap()).expect("mkdir sdk");
    let text_view_source = "package android.widget;\npublic class TextView {}\n";
    fs::write(&text_view_path, text_view_source).expect("textview");

    let indexer = Indexer::new();
    indexer.workspace_root.set(temp.path().to_path_buf());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let text_view_uri = Url::from_file_path(&text_view_path).expect("textview uri");
    indexer.index_layout_content(&layout_uri, layout_source);
    indexer.index_content(&text_view_uri, text_view_source);

    let position = ViewBindingFixture::position_in(layout_source, "TextView");
    let response = find_layout_xml_implementation(&indexer, &layout_uri, position);
    let locations = response_locations(response);

    assert_eq!(locations.len(), 1);
    assert!(locations[0].uri.as_str().contains("TextView.java"));
}

#[test]
fn xml_implementation_on_bare_tag_without_sdk_is_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/widget.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    let layout_source = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;
    fs::write(&layout_path, layout_source).expect("layout");

    let indexer = Indexer::new();
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    indexer.index_layout_content(&layout_uri, layout_source);

    let position = ViewBindingFixture::position_in(layout_source, "TextView");
    let response = find_layout_xml_implementation(&indexer, &layout_uri, position);
    assert!(response.is_none());
}

#[test]
fn open_layout_routes_to_layout_side_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("layout");

    let indexer = Indexer::new();
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    indexer.set_live_lines(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);

    let data = indexer
        .layout_data_for_uri(layout_uri.as_str())
        .expect("layout indexed");
    assert_eq!(data.layout_name, "foo_bar");
    assert!(!data.view_ids.is_empty());
}

#[tokio::test]
async fn definition_on_import_line_remaps_to_layout() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "FooBarBinding");
    let import_line = fixture
        .kotlin_source
        .lines()
        .nth(position.line as usize)
        .unwrap();
    assert!(
        import_line.contains("import"),
        "cursor must be on the import line: {import_line}"
    );

    let context = ViewBindingFixture::cursor_context("FooBarBinding", None);
    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert!(!locations.is_empty());
    assert_eq!(uri_path_string(&locations[0]), "foo_bar.xml");
    assert_eq!(locations[0].range.start.line, 0);
}

#[tokio::test]
async fn bare_unqualified_scope_function_definition_and_references() {
    let fixture = ViewBindingFixture::build();
    let scope_function_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/BareScopeAccess.kt");
    fs::create_dir_all(scope_function_path.parent().unwrap()).expect("mkdir bare scope");
    let scope_function_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun withBlock(binding: FooBarBinding) {
    with(binding) { title }
}

fun applyBlock(binding: FooBarBinding) {
    binding.apply { title }
}

fun runBlock(binding: FooBarBinding) {
    binding.run { title }
}

class NotABinding {
    val title: String = "misleading"
}

fun misleadingWith(competitor: NotABinding) {
    with(competitor) { title }
}
"#;
    fs::write(&scope_function_path, scope_function_source).expect("write bare scope");
    let scope_function_uri = Url::from_file_path(&scope_function_path).expect("scope uri");
    fixture
        .indexer
        .index_content(&scope_function_uri, scope_function_source);
    fixture
        .indexer
        .set_live_lines(&scope_function_uri, scope_function_source);
    fixture
        .indexer
        .store_live_tree(&scope_function_uri, scope_function_source);

    for line_needle in [
        "with(binding) { title }",
        "binding.apply { title }",
        "binding.run { title }",
    ] {
        let position = ViewBindingFixture::position_on_word_in_line(
            scope_function_source,
            line_needle,
            "title",
        );
        let context =
            CursorContext::build_with_cache(&fixture.indexer, &scope_function_uri, position, None)
                .expect("context");
        let response = find_definition(&context, &*fixture.indexer, &scope_function_uri, position)
            .await
            .expect("definition for bare scope access");
        let locations = response_locations(Some(response));
        assert_eq!(
            locations.len(),
            2,
            "expected layout remap for {line_needle}, got {locations:?}"
        );
        assert!(locations
            .iter()
            .all(|location| uri_path_string(location) == "foo_bar.xml"));
    }

    let anchor_position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title");
    let references = binding_field_references_for_test(
        &fixture.indexer,
        "FooBarBinding",
        "title",
        &fixture.kotlin_uri,
        anchor_position.line,
        false,
    )
    .await;

    let with_line = ViewBindingFixture::position_on_word_in_line(
        scope_function_source,
        "with(binding) { title }",
        "title",
    )
    .line;
    let apply_line = ViewBindingFixture::position_on_word_in_line(
        scope_function_source,
        "binding.apply { title }",
        "title",
    )
    .line;
    let run_line = ViewBindingFixture::position_on_word_in_line(
        scope_function_source,
        "binding.run { title }",
        "title",
    )
    .line;
    let misleading_line = ViewBindingFixture::position_on_word_in_line(
        scope_function_source,
        "with(competitor) { title }",
        "title",
    )
    .line;

    let lines_in_scope_file: Vec<u32> = references
        .iter()
        .filter(|location| location.uri == scope_function_uri)
        .map(|location| location.range.start.line)
        .collect();
    assert!(
        lines_in_scope_file.contains(&with_line),
        "bare `title` in `with(binding)` must verify, got {lines_in_scope_file:?}"
    );
    assert!(
        lines_in_scope_file.contains(&apply_line),
        "bare `title` in `apply` must verify, got {lines_in_scope_file:?}"
    );
    assert!(
        lines_in_scope_file.contains(&run_line),
        "bare `title` in `run` must verify, got {lines_in_scope_file:?}"
    );
    assert!(
        !lines_in_scope_file.contains(&misleading_line),
        "bare `title` on non-binding receiver must be excluded, got {lines_in_scope_file:?}"
    );
}

#[tokio::test]
async fn chained_include_field_references_from_nested_position() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_on_word_in_line(
        &fixture.kotlin_source,
        "binding.header.title",
        "title",
    );
    let context =
        CursorContext::build_with_cache(&fixture.indexer, &fixture.kotlin_uri, position, None)
            .expect("context");
    let expected_class = resolve_expected_binding_class(
        &fixture.indexer,
        &fixture.kotlin_uri,
        position,
        &context,
        None,
    )
    .expect("expected ViewHeaderBinding for chained include field");
    assert_eq!(expected_class, "ViewHeaderBinding");

    let references = binding_field_references_for_test(
        &fixture.indexer,
        &expected_class,
        "title",
        &fixture.kotlin_uri,
        position.line,
        false,
    )
    .await;
    assert!(
        references.iter().any(|location| {
            location.uri == fixture.kotlin_uri
                && location.range.start.line
                    == ViewBindingFixture::position_in(
                        &fixture.kotlin_source,
                        "binding.header.title",
                    )
                    .line
        }),
        "chained include field must find its own usage: {references:?}"
    );
    assert!(
        !references.iter().any(|location| {
            location.uri == fixture.kotlin_uri
                && location.range.start.line
                    == ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.title").line
        }),
        "must not conflate parent binding title with include title: {references:?}"
    );
}

#[tokio::test]
async fn hover_on_include_field_shows_binding_type() {
    let fixture = ViewBindingFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.header");
    let context = ViewBindingFixture::cursor_context("header", Some("binding"));
    let hover = compute_hover(
        fixture.indexer.as_ref(),
        &context,
        &fixture.kotlin_uri,
        position,
    )
    .expect("hover on include field");
    let markdown = match hover.contents {
        tower_lsp::lsp_types::HoverContents::Markup(content) => content.value,
        _ => panic!("expected markdown hover"),
    };
    assert!(
        markdown.contains("val header: ViewHeaderBinding"),
        "got: {markdown}"
    );
    assert!(!markdown.contains("com.example.app.databinding"));
}

#[test]
fn binding_implementation_helper_filters_non_generated_classes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let handwritten = temp.path().join("FooBarBinding.java");
    fs::write(&handwritten, "public class FooBarBinding {}\n").expect("write");
    let uri = Url::from_file_path(&handwritten).expect("uri");
    let indexer = Indexer::new();
    indexer.index_content(&uri, "public class FooBarBinding {}\n");

    let context = ViewBindingFixture::cursor_context("FooBarBinding", None);
    let response = find_binding_implementation(&indexer, &context, &uri, Position::new(0, 7));
    assert!(response.is_none());
}

/// UTF-16 column of the first char of `needle` in `source` (LSP position). The
/// fixture's byte-based `position_in` is wrong for lines with multi-byte chars.
fn utf16_position_in(source: &str, needle: &str) -> Position {
    let offset = source.find(needle).expect("needle in source");
    let mut line = 0_u32;
    let mut character = 0_u32;
    for (index, ch) in source.char_indices() {
        if index == offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    Position { line, character }
}

#[tokio::test]
async fn bare_scope_access_respects_local_shadowing() {
    let fixture = ViewBindingFixture::build();
    let shadow_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/Shadow.kt");
    fs::create_dir_all(shadow_path.parent().unwrap()).expect("mkdir shadow");
    // `title` is both a binding field (@+id/title) AND a local val here. Kotlin
    // resolves the bare name to the nearer local, so navigation must NOT remap
    // to the layout.
    let shadow_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

fun shadowed(binding: FooBarBinding) {
    with(binding) {
        val title = "local shadow"
        println(title)
    }
}
"#;
    fs::write(&shadow_path, shadow_source).expect("write shadow");
    let shadow_uri = Url::from_file_path(&shadow_path).expect("shadow uri");
    fixture.indexer.index_content(&shadow_uri, shadow_source);
    fixture.indexer.set_live_lines(&shadow_uri, shadow_source);
    fixture.indexer.store_live_tree(&shadow_uri, shadow_source);

    let position =
        ViewBindingFixture::position_on_word_in_line(shadow_source, "println(title)", "title");
    let context = CursorContext::build_with_cache(&fixture.indexer, &shadow_uri, position, None)
        .expect("cursor context");

    assert!(
        context.qualifier.as_deref() != Some("this"),
        "local val must shadow the binding member; got qualifier {:?}",
        context.qualifier
    );
    assert!(
        resolve_expected_binding_class(&fixture.indexer, &shadow_uri, position, &context, None)
            .is_none(),
        "a local `val title` must not resolve to a binding class"
    );

    let response = find_definition(&context, &*fixture.indexer, &shadow_uri, position).await;
    let locations = response_locations(response);
    assert!(
        locations
            .iter()
            .all(|location| uri_path_string(location) != "foo_bar.xml"),
        "shadowed local `title` must not navigate to the layout: {locations:?}"
    );
}

#[test]
fn xml_view_id_lookup_maps_utf16_column_to_bytes() {
    // A non-ASCII attribute value precedes `@+id/title` on the same line, so the
    // UTF-16 cursor column differs from the byte column tree-sitter expects.
    let content = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android">
    <TextView android:contentDescription="αααααααααα" android:id="@+id/title" />
</LinearLayout>
"#;
    let position = utf16_position_in(content, "id/title");
    let temp = tempfile::tempdir().expect("tempdir");
    let layout_path = temp.path().join("app/src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    fs::write(&layout_path, content).expect("write layout");
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");

    let indexer = Indexer::new();
    indexer.index_layout_content(&layout_uri, content);
    let layout_data = indexer
        .layout_data_for_uri(layout_uri.as_str())
        .expect("layout side index");
    assert_eq!(
        crate::viewbinding::view_id_at_layout_position(&layout_data, position),
        Some("title".to_string()),
        "side-index ranges are UTF-16, so the cursor column must match without re-parsing"
    );
}

const AVATAR_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <ImageView
        android:id="@+id/avatar"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const AVATAR_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.widget.ImageView;

public final class ProfileBinding {
    public final ImageView avatar;

    private ProfileBinding(ImageView avatar) {
        this.avatar = avatar;
    }
}
"#;

/// Layout files exist on disk but are absent from the layout side index — cold-start gap.
struct ColdStartFixture {
    _temp: tempfile::TempDir,
    module_root: PathBuf,
    kotlin_uri: Url,
    kotlin_source: String,
    layout_uri: Url,
    indexer: Arc<Indexer>,
}

impl ColdStartFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");

        let layout_path = layout_dir.join("profile.xml");
        fs::write(&layout_path, AVATAR_LAYOUT).expect("write layout");

        let binding_java_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/ProfileBinding.java",
        );
        fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
        fs::write(&binding_java_path, AVATAR_BINDING_JAVA).expect("write binding java");

        let kotlin_path = module_root.join("src/main/kotlin/com/example/ProfileScreen.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.ProfileBinding

class ProfileScreen {
    fun show(binding: ProfileBinding) {
        binding.avatar
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");

        // Deliberately skip index_layout_content — only bindings + Kotlin.
        indexer.index_generated_bindings(&module_root, None);
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            module_root,
            kotlin_uri,
            kotlin_source: kotlin_source.to_string(),
            layout_uri,
            indexer,
        }
    }
}

#[tokio::test]
async fn definition_on_binding_type_uses_on_demand_layout_indexing() {
    let fixture = ColdStartFixture::build();
    assert!(
        fixture
            .indexer
            .layout_uris_for_binding_class("ProfileBinding", &fixture.module_root)
            .is_empty(),
        "fixture must start without layout side index entries"
    );

    let context = ViewBindingFixture::cursor_context("ProfileBinding", None);
    let position =
        ViewBindingFixture::position_in(&fixture.kotlin_source, "binding: ProfileBinding");
    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 1);
    assert_eq!(uri_path_string(&locations[0]), "profile.xml");
    assert_eq!(locations[0].range.start.line, 0);
}

#[tokio::test]
async fn definition_on_binding_field_uses_on_demand_layout_indexing() {
    let fixture = ColdStartFixture::build();
    let position = ViewBindingFixture::position_in(&fixture.kotlin_source, "binding.avatar");
    let context = ViewBindingFixture::cursor_context("avatar", Some("binding"));

    let response = find_definition(&context, &*fixture.indexer, &fixture.kotlin_uri, position)
        .await
        .expect("definition response");
    let locations = response_locations(Some(response));

    assert_eq!(locations.len(), 1);
    assert_eq!(uri_path_string(&locations[0]), "profile.xml");
    assert!(locations[0].range.start.line > 0);
}

#[test]
fn remap_keeps_generated_java_when_field_has_no_layout_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_dir = module_root.join("src/main/res/layout");
    fs::create_dir_all(&layout_dir).expect("mkdir layout");
    let layout_path = layout_dir.join("foo_bar.xml");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("write layout");

    let binding_java = r#"package com.example.app.databinding;

import android.widget.TextView;

public final class FooBarBinding {
    public final TextView title;
    public final TextView orphanField;
}
"#;
    let binding_java_path = module_root
        .join("build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java");
    fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
    fs::write(&binding_java_path, binding_java).expect("write binding java");

    let indexer = Indexer::new();
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_generated_bindings(&module_root, None);

    let binding_java_uri = Url::from_file_path(&binding_java_path).expect("binding uri");
    let field_locations = indexer.find_definition_qualified("orphanField", None, &binding_java_uri);
    assert!(
        !field_locations.is_empty(),
        "orphanField must resolve in generated Java"
    );

    let remapped = remap_generated_binding_definitions(&indexer, field_locations.clone());
    assert_eq!(
        remapped, field_locations,
        "field with no matching @+id must not fall back to class/layout remap"
    );
    assert!(
        remapped
            .iter()
            .all(|location| location.uri.as_str().contains("FooBarBinding.java")),
        "expected generated Java to be kept, got: {remapped:?}"
    );
}

#[test]
fn remap_keeps_generated_java_when_no_layouts_exist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_java_path = module_root
        .join("build/generated/source/databinding/com/example/app/databinding/ProfileBinding.java");
    fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
    fs::write(&binding_java_path, AVATAR_BINDING_JAVA).expect("write binding java");

    let indexer = Indexer::new();
    indexer.index_generated_bindings(&module_root, None);
    let binding_java_uri = Url::from_file_path(&binding_java_path).expect("binding uri");
    let binding_locations =
        indexer.find_definition_qualified("ProfileBinding", None, &binding_java_uri);
    assert!(!binding_locations.is_empty());

    let remapped = remap_generated_binding_definitions(&indexer, binding_locations.clone());
    assert!(
        !remapped.is_empty(),
        "remap must keep generated Java when no layouts exist"
    );
    assert!(
        remapped
            .iter()
            .any(|location| location.uri.as_str().contains("ProfileBinding.java")),
        "expected at least one generated Java fallback, got: {remapped:?}"
    );
}

#[test]
fn reference_location_normalization_converts_rg_byte_columns() {
    let fixture = ViewBindingFixture::build();
    let line = fixture
        .kotlin_source
        .lines()
        .find(|line| line.contains("binding.title"))
        .expect("binding.title line");
    let prefixed = line.replace("binding.title", "println(\"标题\"); binding.title");
    let byte_column = prefixed.find("title").expect("title in line") as u32;
    let utf16_column = prefixed[..byte_column as usize]
        .chars()
        .map(|character| character.len_utf16())
        .sum::<usize>() as u32;
    assert_ne!(
        byte_column, utf16_column,
        "fixture must use a multibyte prefix"
    );

    let kotlin_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/MultibyteRef.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir multibyte");
    let source = format!("{}\n", fixture.kotlin_source.replace(line, &prefixed));
    fs::write(&kotlin_path, &source).expect("write multibyte");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    fixture.indexer.index_content(&kotlin_uri, &source);

    let rg_location = Location {
        uri: kotlin_uri.clone(),
        range: Range {
            start: Position {
                line: source
                    .lines()
                    .position(|l| l.contains("binding.title"))
                    .unwrap() as u32,
                character: byte_column,
            },
            end: Position {
                line: source
                    .lines()
                    .position(|l| l.contains("binding.title"))
                    .unwrap() as u32,
                character: byte_column + 5,
            },
        },
    };
    let normalized =
        normalize_reference_location_to_utf16_for_test(&fixture.indexer, &rg_location, "title");
    assert_eq!(
        normalized.range.start.character, utf16_column,
        "rg byte columns must be normalized to UTF-16 at ingestion"
    );
}

#[test]
fn view_id_matches_lookup_accepts_camel_case_ids() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/camel_case.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");
    let layout = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android">
    <TextView android:id="@+id/fooBar" />
</LinearLayout>
"#;
    fs::write(&layout_path, layout).expect("write layout");

    let indexer = Indexer::new();
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    indexer.index_layout_content(&layout_uri, layout);

    let snake_targets = indexer.layouts_declaring_view_id(&module_root, "camel_case", "foo_bar");
    assert_eq!(
        snake_targets.len(),
        1,
        "snake_case lookup must match camelCase @+id"
    );
}

#[tokio::test]
async fn xml_references_use_binding_verification_not_text_search() {
    let fixture = ColdStartFixture::build();

    let decoy_path = fixture
        .module_root
        .join("src/main/kotlin/com/example/Decoy.kt");
    fs::create_dir_all(decoy_path.parent().unwrap()).expect("mkdir decoy");
    let decoy_source = r#"package com.example

class Decoy {
    fun noise() {
        val avatar = "avatar"
        println(avatar)
    }
}
"#;
    fs::write(&decoy_path, decoy_source).expect("write decoy");
    let decoy_uri = Url::from_file_path(&decoy_path).expect("decoy uri");
    fixture.indexer.index_content(&decoy_uri, decoy_source);

    let xml_position = ViewBindingFixture::position_in(AVATAR_LAYOUT, "@+id/avatar");
    let references =
        layout_xml_references_for_test(&fixture.indexer, &fixture.layout_uri, xml_position, false)
            .await
            .expect("layout xml references");

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].uri, fixture.kotlin_uri);
    assert!(
        !references.iter().any(|location| location.uri == decoy_uri),
        "decoy bare `avatar` usages must not appear in binding-field references"
    );
}

/// Competing `binding` declarations in one class: a class property plus two
/// methods each taking a different `*Binding` parameter. File-global name lookup
/// returns the first `binding:` annotation — the wrong class for receiver-scoped
/// access inside the other methods.
struct CompetingBindingFixture {
    _temp: tempfile::TempDir,
    kotlin_uri: Url,
    kotlin_source: String,
    indexer: Arc<Indexer>,
}

impl CompetingBindingFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");

        let default_layout_path = layout_dir.join("foo_bar.xml");
        let header_layout_path = layout_dir.join("view_header.xml");
        let profile_layout_path = layout_dir.join("profile.xml");
        fs::write(&default_layout_path, FOO_BAR_LAYOUT).expect("write default layout");
        fs::write(&header_layout_path, VIEW_HEADER_LAYOUT).expect("write header layout");
        fs::write(&profile_layout_path, AVATAR_LAYOUT).expect("write profile layout");

        let binding_java_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java",
        );
        let header_binding_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/ViewHeaderBinding.java",
        );
        let profile_binding_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/ProfileBinding.java",
        );
        fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
        fs::write(&binding_java_path, FOO_BAR_BINDING_JAVA).expect("write binding java");
        fs::write(&header_binding_path, VIEW_HEADER_BINDING_JAVA).expect("write header binding");
        fs::write(&profile_binding_path, AVATAR_BINDING_JAVA).expect("write profile binding");

        let kotlin_path = module_root.join("src/main/kotlin/com/example/CompetingBindings.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir competing");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding
import com.example.app.databinding.ProfileBinding
import com.example.app.databinding.ViewHeaderBinding

class Delegate {
    private val binding: ViewHeaderBinding? = null

    fun bar(binding: FooBarBinding) {
        binding.title.toString()
        with(binding) {
            title.toString()
        }
        binding.apply {
            title.toString()
        }
        binding.let {
            it.title.toString()
        }
    }

    fun other(binding: ProfileBinding) {
        with(binding) {
            avatar.toString()
        }
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write competing");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let default_layout_uri = Url::from_file_path(&default_layout_path).expect("default uri");
        let header_layout_uri = Url::from_file_path(&header_layout_path).expect("header uri");
        let profile_layout_uri = Url::from_file_path(&profile_layout_path).expect("profile uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("competing uri");

        indexer.index_layout_content(&default_layout_uri, FOO_BAR_LAYOUT);
        indexer.index_layout_content(&header_layout_uri, VIEW_HEADER_LAYOUT);
        indexer.index_layout_content(&profile_layout_uri, AVATAR_LAYOUT);
        indexer.index_generated_bindings(&module_root, None);
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            kotlin_uri,
            kotlin_source: kotlin_source.to_string(),
            indexer,
        }
    }

    fn assert_resolves_to_foo_bar(&self, line_needle: &str, word: &str, qualifier: Option<&str>) {
        let position =
            ViewBindingFixture::position_on_word_in_line(&self.kotlin_source, line_needle, word);
        let context = if let Some(qualifier_name) = qualifier {
            ViewBindingFixture::cursor_context(word, Some(qualifier_name))
        } else {
            CursorContext::build_with_cache(&self.indexer, &self.kotlin_uri, position, None)
                .expect("context")
        };
        let expected_class = resolve_expected_binding_class(
            &self.indexer,
            &self.kotlin_uri,
            position,
            &context,
            None,
        )
        .unwrap_or_else(|| {
            panic!(
                "resolve_expected_binding_class returned None for {line_needle:?} word={word:?} qualifier={qualifier:?} contextual={:?}",
                context.contextual.as_ref().map(|receiver| receiver.leaf.as_str())
            )
        });
        assert_eq!(
            expected_class, "FooBarBinding",
            "wrong binding class for {line_needle:?} word={word:?}"
        );
    }

    async fn assert_definition_in_foo_bar(&self, line_needle: &str, word: &str) {
        let position =
            ViewBindingFixture::position_on_word_in_line(&self.kotlin_source, line_needle, word);
        let context =
            CursorContext::build_with_cache(&self.indexer, &self.kotlin_uri, position, None)
                .expect("context");
        let response = find_definition(&context, &*self.indexer, &self.kotlin_uri, position)
            .await
            .expect("definition for {line_needle}");
        let locations = response_locations(Some(response));
        assert!(
            !locations.is_empty(),
            "expected layout remap for {line_needle}, got empty"
        );
        assert!(
            locations
                .iter()
                .all(|location| uri_path_string(location) == "foo_bar.xml"),
            "expected foo_bar.xml for {line_needle}, got {locations:?}"
        );
    }
}

#[test]
fn resolve_expected_binding_class_scoped_to_enclosing_method_parameter() {
    let fixture = CompetingBindingFixture::build();
    fixture.assert_resolves_to_foo_bar("binding.title.toString()", "title", Some("binding"));
    fixture.assert_resolves_to_foo_bar(
        "with(binding) {\n            title.toString()",
        "title",
        None,
    );
    fixture.assert_resolves_to_foo_bar(
        "binding.apply {\n            title.toString()",
        "title",
        None,
    );
    fixture.assert_resolves_to_foo_bar(
        "binding.let {\n            it.title.toString()",
        "title",
        Some("it"),
    );
}

#[tokio::test]
async fn receiver_scoped_binding_field_definition_resolves_to_correct_layout() {
    let fixture = CompetingBindingFixture::build();
    fixture
        .assert_definition_in_foo_bar("binding.title.toString()", "title")
        .await;
    fixture
        .assert_definition_in_foo_bar("with(binding) {\n            title.toString()", "title")
        .await;
    fixture
        .assert_definition_in_foo_bar("binding.apply {\n            title.toString()", "title")
        .await;
    fixture
        .assert_definition_in_foo_bar("binding.let {\n            it.title.toString()", "title")
        .await;
}

#[test]
fn resolve_expected_binding_class_other_method_uses_profile_binding() {
    let fixture = CompetingBindingFixture::build();
    let position = ViewBindingFixture::position_on_word_in_line(
        &fixture.kotlin_source,
        "with(binding) {\n            avatar.toString()",
        "avatar",
    );
    let context =
        CursorContext::build_with_cache(&fixture.indexer, &fixture.kotlin_uri, position, None)
            .expect("context");
    let expected_class = resolve_expected_binding_class(
        &fixture.indexer,
        &fixture.kotlin_uri,
        position,
        &context,
        None,
    )
    .expect("expected ProfileBinding for other()");
    assert_eq!(expected_class, "ProfileBinding");
}

struct ChainedReceiverFixture {
    _temp: tempfile::TempDir,
    kotlin_uri: Url,
    kotlin_source: String,
    indexer: Arc<Indexer>,
}

impl ChainedReceiverFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");

        let default_layout_path = layout_dir.join("foo_bar.xml");
        let profile_layout_path = layout_dir.join("profile.xml");
        fs::write(&default_layout_path, FOO_BAR_LAYOUT).expect("write default layout");
        fs::write(&profile_layout_path, AVATAR_LAYOUT).expect("write profile layout");

        let binding_java_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/FooBarBinding.java",
        );
        let profile_binding_path = module_root.join(
            "build/generated/source/databinding/com/example/app/databinding/ProfileBinding.java",
        );
        fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
        fs::write(&binding_java_path, FOO_BAR_BINDING_JAVA).expect("write binding java");
        fs::write(&profile_binding_path, AVATAR_BINDING_JAVA).expect("write profile binding");

        // Index a competing ViewHolder first so a workspace-wide field lookup would pick
        // ProfileBinding for `binding` if scope-aware resolution is missing.
        let wrong_holder_path = module_root.join("src/main/kotlin/com/example/WrongViewHolder.kt");
        fs::create_dir_all(wrong_holder_path.parent().unwrap()).expect("mkdir wrong holder");
        let wrong_holder_source = r#"package com.example.other

import com.example.app.databinding.ProfileBinding

class ViewHolder(val binding: ProfileBinding)
"#;
        fs::write(&wrong_holder_path, wrong_holder_source).expect("write wrong holder");

        let kotlin_path = module_root.join("src/main/kotlin/com/example/ChainedReceiver.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir chained");
        let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class ViewHolder(val binding: FooBarBinding)

class Delegate {
    fun bar(holder: ViewHolder) {
        holder.binding.title.toString()
        with(holder.binding) {
            title.toString()
            this.title.toString()
        }
        holder.binding.apply {
            title.toString()
            this.title.toString()
        }
        holder.binding.also {
            it.title.toString()
        }
        holder.binding.let { bind ->
            bind.title.toString()
        }
        holder.binding.let {
            it.title.toString()
        }
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write chained");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let default_layout_uri = Url::from_file_path(&default_layout_path).expect("default uri");
        let profile_layout_uri = Url::from_file_path(&profile_layout_path).expect("profile uri");
        let wrong_holder_uri = Url::from_file_path(&wrong_holder_path).expect("wrong holder uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("chained uri");

        indexer.index_layout_content(&default_layout_uri, FOO_BAR_LAYOUT);
        indexer.index_layout_content(&profile_layout_uri, AVATAR_LAYOUT);
        indexer.index_generated_bindings(&module_root, None);
        indexer.index_content(&wrong_holder_uri, wrong_holder_source);
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            kotlin_uri,
            kotlin_source: kotlin_source.to_string(),
            indexer,
        }
    }

    fn assert_resolves_to_foo_bar(&self, line_needle: &str, word: &str, qualifier: Option<&str>) {
        let position =
            ViewBindingFixture::position_on_word_in_line(&self.kotlin_source, line_needle, word);
        let context = if let Some(qualifier_name) = qualifier {
            ViewBindingFixture::cursor_context(word, Some(qualifier_name))
        } else {
            CursorContext::build_with_cache(&self.indexer, &self.kotlin_uri, position, None)
                .expect("context")
        };
        let expected_class = resolve_expected_binding_class(
            &self.indexer,
            &self.kotlin_uri,
            position,
            &context,
            None,
        )
        .unwrap_or_else(|| {
            panic!(
                "resolve_expected_binding_class returned None for {line_needle:?} word={word:?} qualifier={qualifier:?} contextual={:?}",
                context.contextual.as_ref().map(|receiver| receiver.leaf.as_str())
            )
        });
        assert_eq!(
            expected_class, "FooBarBinding",
            "wrong binding class for {line_needle:?} word={word:?}"
        );
    }

    fn assert_definition_in_foo_bar(&self, line_needle: &str, word: &str) {
        let position =
            ViewBindingFixture::position_on_word_in_line(&self.kotlin_source, line_needle, word);
        let context =
            CursorContext::build_with_cache(&self.indexer, &self.kotlin_uri, position, None)
                .expect("context");
        let response =
            find_binding_field_definition(&self.indexer, &self.kotlin_uri, position, &context)
                .expect("definition for {line_needle}");
        let locations = response_locations(Some(response));
        assert!(
            !locations.is_empty(),
            "expected layout remap for {line_needle}, got empty"
        );
        assert!(
            locations
                .iter()
                .all(|location| uri_path_string(location) == "foo_bar.xml"),
            "expected foo_bar.xml for {line_needle}, got {locations:?}"
        );
    }
}

#[test]
fn resolve_expected_binding_class_chained_receiver_with_apply_also_let() {
    let fixture = ChainedReceiverFixture::build();
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.title.toString()",
        "title",
        Some("holder.binding"),
    );
    fixture.assert_resolves_to_foo_bar(
        "with(holder.binding) {\n            title.toString()",
        "title",
        None,
    );
    fixture.assert_resolves_to_foo_bar(
        "with(holder.binding) {\n            title.toString()\n            this.title.toString()",
        "title",
        Some("this"),
    );
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.apply {\n            title.toString()",
        "title",
        None,
    );
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.apply {\n            title.toString()\n            this.title.toString()",
        "title",
        Some("this"),
    );
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.also {\n            it.title.toString()",
        "title",
        Some("it"),
    );
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.let { bind ->\n            bind.title.toString()",
        "title",
        Some("bind"),
    );
    fixture.assert_resolves_to_foo_bar(
        "holder.binding.let {\n            it.title.toString()",
        "title",
        Some("it"),
    );
}

#[tokio::test]
async fn chained_receiver_binding_field_definition_resolves_to_correct_layout() {
    let fixture = ChainedReceiverFixture::build();
    fixture.assert_definition_in_foo_bar("holder.binding.title.toString()", "title");
    fixture.assert_definition_in_foo_bar(
        "with(holder.binding) {\n            title.toString()",
        "title",
    );
    fixture.assert_definition_in_foo_bar(
        "with(holder.binding) {\n            title.toString()\n            this.title.toString()",
        "title",
    );
    fixture.assert_definition_in_foo_bar(
        "holder.binding.apply {\n            title.toString()",
        "title",
    );
    fixture.assert_definition_in_foo_bar(
        "holder.binding.apply {\n            title.toString()\n            this.title.toString()",
        "title",
    );
    fixture.assert_definition_in_foo_bar(
        "holder.binding.also {\n            it.title.toString()",
        "title",
    );
    fixture.assert_definition_in_foo_bar(
        "holder.binding.let { bind ->\n            bind.title.toString()",
        "title",
    );
}

struct InheritedGenericBindingFixture {
    _temp: tempfile::TempDir,
    kotlin_uri: Url,
    kotlin_source: String,
    indexer: Arc<Indexer>,
}

impl InheritedGenericBindingFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let module_root = temp.path().join("app");
        let layout_dir = module_root.join("src/main/res/layout");
        fs::create_dir_all(&layout_dir).expect("mkdir layout");

        let default_layout_path = layout_dir.join("foo_bar.xml");
        fs::write(&default_layout_path, FOO_BAR_LAYOUT).expect("write default layout");

        let binding_java_path = module_root.join(
            "build/generated/data_binding_base_class_source_out/debug/out/com/example/app/databinding/FooBarBinding.java",
        );
        fs::create_dir_all(binding_java_path.parent().unwrap()).expect("mkdir binding");
        fs::write(&binding_java_path, FOO_BAR_BINDING_JAVA).expect("write binding java");

        let base_path = module_root.join("src/main/kotlin/framework/ViewBindingAdapter.kt");
        fs::create_dir_all(base_path.parent().unwrap()).expect("mkdir base");
        let base_source = r#"package framework

import androidx.viewbinding.ViewBinding

abstract class ViewBindingAdapter<V : ViewBinding> {
    private var _viewBinding: V? = null
    val viewBinding: V get() = _viewBinding ?: error("not init")
}
"#;
        fs::write(&base_path, base_source).expect("write base adapter");

        let intermediate_path = module_root.join("src/main/kotlin/shared/BaseIntermediate.kt");
        fs::create_dir_all(intermediate_path.parent().unwrap()).expect("mkdir intermediate");
        let intermediate_source = r#"package shared

import androidx.viewbinding.ViewBinding
import framework.ViewBindingAdapter

abstract class BaseIntermediate<V : ViewBinding> : ViewBindingAdapter<V>()
"#;
        fs::write(&intermediate_path, intermediate_source).expect("write intermediate");

        let decoy_path = module_root.join("src/main/kotlin/misleading/ViewBindingAdapter.kt");
        fs::create_dir_all(decoy_path.parent().unwrap()).expect("mkdir decoy");
        let decoy_source = r#"package misleading

abstract class ViewBindingAdapter<V> {
    val viewBinding: WrongBinding get() = error("wrong")
}
"#;
        fs::write(&decoy_path, decoy_source).expect("write decoy");

        let kotlin_path = module_root.join("src/main/kotlin/feature/FooFragment.kt");
        fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir fragment");
        let kotlin_source = r#"package feature

import com.example.app.databinding.FooBarBinding
import shared.BaseIntermediate

class FooFragment : BaseIntermediate<FooBarBinding>() {
    fun bar() {
        viewBinding.title.toString()
        with(viewBinding) {
            title.toString()
        }
        viewBinding.apply {
            title.toString()
        }
    }
}
"#;
        fs::write(&kotlin_path, kotlin_source).expect("write fragment");

        let indexer = Arc::new(Indexer::new());
        indexer.workspace_root.set(temp.path().to_path_buf());

        let default_layout_uri = Url::from_file_path(&default_layout_path).expect("default uri");
        let base_uri = Url::from_file_path(&base_path).expect("base uri");
        let intermediate_uri = Url::from_file_path(&intermediate_path).expect("intermediate uri");
        let decoy_uri = Url::from_file_path(&decoy_path).expect("decoy uri");
        let kotlin_uri = Url::from_file_path(&kotlin_path).expect("fragment uri");

        indexer.index_layout_content(&default_layout_uri, FOO_BAR_LAYOUT);
        indexer.index_generated_bindings(&module_root, None);
        indexer.index_content(&decoy_uri, decoy_source);
        indexer.index_content(&base_uri, base_source);
        indexer.index_content(&intermediate_uri, intermediate_source);
        indexer.index_content(&kotlin_uri, kotlin_source);
        indexer.set_live_lines(&kotlin_uri, kotlin_source);
        indexer.store_live_tree(&kotlin_uri, kotlin_source);

        Self {
            _temp: temp,
            kotlin_uri,
            kotlin_source: kotlin_source.to_string(),
            indexer,
        }
    }

    fn assert_resolves_to_foo_bar(&self, line_needle: &str, word: &str, qualifier: Option<&str>) {
        let position =
            ViewBindingFixture::position_on_word_in_line(&self.kotlin_source, line_needle, word);
        let context = if let Some(qualifier_name) = qualifier {
            ViewBindingFixture::cursor_context(word, Some(qualifier_name))
        } else {
            CursorContext::build_with_cache(&self.indexer, &self.kotlin_uri, position, None)
                .expect("context")
        };
        let expected_class = resolve_expected_binding_class(
            &self.indexer,
            &self.kotlin_uri,
            position,
            &context,
            None,
        )
        .unwrap_or_else(|| {
            panic!(
                "resolve_expected_binding_class returned None for {line_needle:?} word={word:?} qualifier={qualifier:?} contextual={:?}",
                context.contextual.as_ref().map(|receiver| receiver.leaf.as_str())
            )
        });
        assert_eq!(
            expected_class, "FooBarBinding",
            "wrong binding class for {line_needle:?} word={word:?}"
        );
    }
}

#[test]
fn resolve_expected_binding_class_inherited_generic_base() {
    let fixture = InheritedGenericBindingFixture::build();
    fixture.assert_resolves_to_foo_bar(
        "viewBinding.title.toString()",
        "title",
        Some("viewBinding"),
    );
    fixture.assert_resolves_to_foo_bar(
        "with(viewBinding) {\n            title.toString()",
        "title",
        None,
    );
    fixture.assert_resolves_to_foo_bar(
        "viewBinding.apply {\n            title.toString()",
        "title",
        None,
    );
}
