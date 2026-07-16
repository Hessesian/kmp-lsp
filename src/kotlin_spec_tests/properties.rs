use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::{Indexer, InferDeps};
use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, SymbolKind, Url};

fn position_of_occurrence(source: &str, needle: &str, occurrence: usize) -> Position {
    let byte_offset = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(byte_offset, _)| byte_offset)
        .expect("fixture occurrence must exist");
    let preceding_source = &source[..byte_offset];
    let line = preceding_source.matches('\n').count() as u32;
    let character = preceding_source
        .rsplit('\n')
        .next()
        .expect("split always yields one segment")
        .chars()
        .count() as u32;
    Position::new(line, character)
}

async fn definition_position(source: &str, needle: &str, occurrence: usize) -> Option<Position> {
    let specification_uri = Url::parse("file:///kotlin-spec/Properties.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = position_of_occurrence(source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &specification_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &specification_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => Some(location.range.start),
        Some(GotoDefinitionResponse::Array(locations)) if locations.len() == 1 => {
            Some(locations[0].range.start)
        }
        Some(GotoDefinitionResponse::Array(_)) | Some(GotoDefinitionResponse::Link(_)) | None => {
            None
        }
    }
}

#[test]
fn ks_4_3_001_property_declarations_create_top_level_member_and_local_entities() {
    let source = "val topSpec: Int = 1\nclass HostSpec { val memberSpec: String = \"member\"; }\nfun localSpec(): Int { val localValueSpec = 2; return localValueSpec }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/PropertyScopes.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for property_name in ["topSpec", "memberSpec", "localValueSpec"] {
        let property = symbols
            .iter()
            .find(|symbol| symbol.name == property_name)
            .expect("property entity must be indexed");
        assert_eq!(property.kind, SymbolKind::PROPERTY);
    }
}

#[test]
fn ks_4_3_002_val_and_var_create_read_only_and_mutable_symbol_kinds() {
    let source = "val readOnlySpec: Int = 1\nvar mutableSpec: Int = 2\n";
    let specification_uri = Url::parse("file:///kotlin-spec/PropertyMutability.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    let read_only = symbols
        .iter()
        .find(|symbol| symbol.name == "readOnlySpec")
        .expect("read-only property must be indexed");
    assert_eq!(read_only.kind, SymbolKind::PROPERTY);
    let mutable = symbols
        .iter()
        .find(|symbol| symbol.name == "mutableSpec")
        .expect("mutable property must be indexed");
    assert_eq!(mutable.kind, SymbolKind::VARIABLE);
}

#[test]
#[ignore = "KS-4.3-004: kmp-lsp does not diagnose direct accessor calls"]
fn ks_4_3_004_property_accessors_cannot_be_called_directly() {
    assert_source_parses(
        "class HostSpec { val valueSpec: Int get() = 1; }\nval validSpec = HostSpec().valueSpec\n",
    );
    assert_source_has_syntax_error(
        "class HostSpec { val valueSpec: Int get() = 1; }\nval invalidSpec = HostSpec().valueSpec.get()\n",
    );
}

#[tokio::test]
async fn ks_4_3_1_001_read_only_property_names_its_initializer_result() {
    let source = "val valueSpec: String = \"value\"\nval copiedSpec = valueSpec\n";
    let position = definition_position(source, "valueSpec", 1).await;
    assert_eq!(position, Some(Position::new(0, 4)));
}

#[test]
fn ks_4_3_1_002_read_only_property_accepts_block_or_expression_getter() {
    let source = "val blockSpec: Int\n    get(): Int { return 1 }\nval expressionSpec: String\n    get(): String = \"value\"\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ReadOnlyGetters.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for property_name in ["blockSpec", "expressionSpec"] {
        let property = symbols
            .iter()
            .find(|symbol| symbol.name == property_name)
            .expect("getter-backed property must be indexed");
        assert_eq!(property.kind, SymbolKind::PROPERTY);
    }
}

#[test]
#[ignore = "KS-4.3.1-003: kmp-lsp does not diagnose val declarations missing initializer type and getter"]
fn ks_4_3_1_003_read_only_property_requires_initializer_type_or_getter() {
    assert_source_parses("val initializedSpec = 1\nval typedSpec: Int\nval getterSpec get() = 1\n");
    assert_source_has_syntax_error("val invalidSpec\n");
}

#[test]
#[ignore = "KS-4.3.1-004: kmp-lsp does not infer top-level property types from initializers"]
fn ks_4_3_1_004_initializer_boundedly_infers_read_only_property_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/InitializerPropertyType.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, "val inferredSpec = \"value\"\n");
    assert_eq!(
        indexer
            .find_var_type("inferredSpec", &specification_uri)
            .as_deref(),
        Some("String")
    );
}

#[test]
#[ignore = "KS-4.3.1-005: kmp-lsp does not infer property types from expression getters"]
fn ks_4_3_1_005_expression_getter_boundedly_infers_read_only_property_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/GetterPropertyType.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, "val inferredSpec get() = \"value\"\n");
    assert_eq!(
        indexer
            .find_var_type("inferredSpec", &specification_uri)
            .as_deref(),
        Some("String")
    );
}

#[test]
#[ignore = "KS-4.3.1-006: kmp-lsp does not diagnose non-inferable untyped properties"]
fn ks_4_3_1_006_non_inferable_property_requires_explicit_type() {
    assert_source_parses("val validSpec: String get() { return \"value\" }\n");
    assert_source_has_syntax_error("val invalidSpec get() { return \"value\" }\n");
}

#[test]
#[ignore = "KS-4.3.1-009: kmp-lsp does not diagnose initializers without backing fields"]
fn ks_4_3_1_009_property_without_backing_field_cannot_have_initializer() {
    assert_source_parses("val validSpec: Int get() = 2\n");
    assert_source_has_syntax_error("val invalidSpec: Int = 1 get() = 2\n");
}

#[test]
#[ignore = "KS-4.3.1-010: kmp-lsp does not diagnose reassignment of read-only properties"]
fn ks_4_3_1_010_read_only_property_cannot_be_reassigned_after_initializer() {
    assert_source_parses("val validSpec: Int = 1\n");
    assert_source_has_syntax_error("val invalidSpec: Int = 1\ninvalidSpec = 2\n");
}

#[tokio::test]
async fn ks_4_3_2_001_mutable_property_names_assignable_typed_state() {
    let source = "var countSpec: Int = 1\ncountSpec = 2\nval copiedSpec = countSpec\n";
    assert_source_parses(source);
    for occurrence in [1, 2] {
        let position = definition_position(source, "countSpec", occurrence).await;
        assert_eq!(position, Some(Position::new(0, 4)));
    }
}

#[test]
#[ignore = "KS-4.3.2-002: kmp-lsp does not infer mutable property types from initializers"]
fn ks_4_3_2_002_initializer_boundedly_infers_mutable_property_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/MutablePropertyType.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, "var inferredSpec = \"value\"\n");
    assert_eq!(
        indexer
            .find_var_type("inferredSpec", &specification_uri)
            .as_deref(),
        Some("String")
    );
}

#[test]
fn ks_4_3_2_003_mutable_property_accepts_custom_getter_and_setter() {
    let source = "var valueSpec: Int = 1\n    get(): Int = field\n    set(newValueSpec: Int) { field = newValueSpec }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/MutableAccessors.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let property = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "valueSpec")
        .expect("mutable accessor property must be indexed");
    assert_eq!(property.kind, SymbolKind::VARIABLE);
}

#[test]
fn ks_4_3_3_001_local_property_creates_an_entity_in_function_scope() {
    let source = "fun buildSpec(): Int { val localSpec: Int = 1; return localSpec }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/LocalProperty.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let property = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "localSpec")
        .expect("local property must be indexed");
    assert_eq!(property.kind, SymbolKind::PROPERTY);
    assert_eq!(property.range.start.line, 0);
}

#[test]
#[ignore = "KS-4.3.3-002: kmp-lsp does not diagnose custom accessors on local properties"]
fn ks_4_3_3_002_local_property_cannot_have_custom_accessors() {
    assert_source_parses("fun validSpec(): Int { val valueSpec = 1; return valueSpec }\n");
    assert_source_has_syntax_error(
        "fun invalidGetterSpec(): Int { val valueSpec: Int get() = 1; return valueSpec }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSetterSpec(): Int { var valueSpec: Int = 1 set(newValueSpec) { field = newValueSpec }; return valueSpec }\n",
    );
}

#[test]
fn ks_4_3_3_004_destructuring_introduces_one_local_name_per_entry() {
    let source = "fun buildSpec(): Int { val (firstSpec, secondSpec) = Pair(1, 2); return firstSpec + secondSpec }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/LocalDestructuring.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for property_name in ["firstSpec", "secondSpec"] {
        assert!(
            symbols.iter().any(|symbol| symbol.name == property_name),
            "destructured local name must be indexed"
        );
    }
}

#[test]
#[ignore = "KS-4.3.3-005: kmp-lsp indexes destructuring ignore markers as symbols"]
fn ks_4_3_3_005_destructuring_ignore_marker_introduces_no_name() {
    let source = "fun buildSpec(): Int { val (_, valueSpec) = Pair(1, 2); return valueSpec }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/DestructuringIgnore.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    assert!(symbols.iter().any(|symbol| symbol.name == "valueSpec"));
    assert!(symbols.iter().all(|symbol| symbol.name != "_"));
}

#[test]
#[ignore = "KS-4.3.3-007: kmp-lsp does not diagnose accessors on destructuring declarations"]
fn ks_4_3_3_007_destructuring_declaration_cannot_use_accessor() {
    assert_source_parses(
        "fun validSpec(): Int { val (firstSpec, secondSpec) = Pair(1, 2); return firstSpec + secondSpec }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(): Int { val (firstSpec, secondSpec) = Pair(1, 2) get() = Pair(3, 4); return firstSpec + secondSpec }\n",
    );
}

#[test]
#[ignore = "KS-4.3.3-008: kmp-lsp does not diagnose delegated destructuring declarations"]
fn ks_4_3_3_008_destructuring_declaration_cannot_use_delegate() {
    assert_source_parses(
        "fun validSpec(): Int { val (firstSpec, secondSpec) = Pair(1, 2); return firstSpec + secondSpec }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(): Int { val (firstSpec, secondSpec) by lazy { Pair(1, 2) }; return firstSpec + secondSpec }\n",
    );
}

#[test]
#[ignore = "KS-4.3.3-009: kmp-lsp does not require in-place destructuring initialization"]
fn ks_4_3_3_009_destructuring_declaration_must_be_initialized_in_place() {
    assert_source_parses(
        "fun validSpec(): Int { val (firstSpec, secondSpec) = Pair(1, 2); return firstSpec + secondSpec }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(): Int { val (firstSpec, secondSpec); return 0 }\n",
    );
}
