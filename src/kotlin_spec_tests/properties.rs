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

#[test]
#[ignore = "KS-4.3.4-001: kmp-lsp does not validate getter return type equality"]
fn ks_4_3_4_001_getter_return_type_must_equal_property_type() {
    assert_source_parses("val validSpec: Int get(): Int = 1\n");
    assert_source_has_syntax_error("val invalidSpec: Int get(): String = \"value\"\n");
}

#[test]
#[ignore = "KS-4.3.4-002: kmp-lsp does not validate setter parameter type equality"]
fn ks_4_3_4_002_setter_parameter_type_must_equal_property_type() {
    assert_source_parses(
        "var validSpec: Int = 1 set(newValueSpec: Int) { field = newValueSpec }\n",
    );
    assert_source_has_syntax_error(
        "var invalidSpec: Int = 1 set(newValueSpec: String) { field = newValueSpec.length }\n",
    );
}

#[test]
#[ignore = "KS-4.3.4-003: kmp-lsp does not require Unit setter return type"]
fn ks_4_3_4_003_setter_return_type_must_be_unit() {
    assert_source_parses(
        "var validSpec: Int = 1 set(newValueSpec: Int): Unit { field = newValueSpec }\n",
    );
    assert_source_has_syntax_error(
        "var invalidSpec: Int = 1 set(newValueSpec: Int): String { field = newValueSpec; return \"wrong\" }\n",
    );
}

#[test]
fn ks_4_3_4_004_accessor_types_may_be_omitted() {
    assert_source_parses(
        "var valueSpec: Int = 1\n    get() = field\n    set(newValueSpec) { field = newValueSpec }\n",
    );
}

#[test]
#[ignore = "KS-4.3.4-005: kmp-lsp does not diagnose setters on read-only properties"]
fn ks_4_3_4_005_read_only_property_cannot_have_setter() {
    assert_source_parses("val validSpec: Int get() = 1\n");
    assert_source_has_syntax_error(
        "val invalidSpec: Int\n    get() = 1\n    set(newValueSpec) {}\n",
    );
}

#[test]
fn ks_4_3_4_006_mutable_property_accepts_any_accessor_combination() {
    assert_source_parses(
        "var getterOnlySpec: Int = 1 get() = field\nvar setterOnlySpec: Int = 1 set(newValueSpec) { field = newValueSpec }\nvar bothSpec: Int = 1\n    get() = field\n    set(newValueSpec) { field = newValueSpec }\n",
    );
}

#[test]
fn ks_4_3_4_007_setter_parameter_accepts_any_valid_identifier() {
    assert_source_parses(
        "var valueSpec: Int = 1 set(replacementSpec) { field = replacementSpec }\n",
    );
}

#[test]
fn ks_4_3_4_008_accessor_body_may_be_omitted_for_default_implementation() {
    assert_source_parses("var valueSpec: Int = 1\n    get\n    set\n");
}

#[test]
fn ks_4_3_4_009_default_accessor_may_change_visibility() {
    assert_source_parses("var valueSpec: Int = 1\n    private set\n");
}

#[test]
#[ignore = "KS-4.3.4-011: kmp-lsp does not diagnose assignment to field inside getters"]
fn ks_4_3_4_011_backing_field_is_read_only_inside_getter() {
    assert_source_parses("val validSpec: Int = 1 get() = field\n");
    assert_source_has_syntax_error("val invalidSpec: Int = 1 get() { field = 2; return field }\n");
}

#[test]
fn ks_4_3_4_012_backing_field_is_mutable_inside_setter() {
    assert_source_parses("var valueSpec: Int = 1 set(newValueSpec) { field = newValueSpec }\n");
}

#[test]
#[ignore = "KS-4.3.4-018: kmp-lsp does not diagnose initializers on field-free properties"]
fn ks_4_3_4_018_property_without_backing_field_cannot_have_initializer() {
    assert_source_parses(
        "var validSpec: Int\n    get() = 1\n    set(newValueSpec) { println(newValueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "var invalidSpec: Int = 1\n    get() = 1\n    set(newValueSpec) { println(newValueSpec) }\n",
    );
}

#[test]
fn ks_4_3_4_020_accessor_accepts_function_modifiers() {
    assert_source_parses(
        "var valueSpec: Int = 1\n    inline get() = field\n    inline set(newValueSpec) { field = newValueSpec }\n",
    );
}

#[test]
fn ks_4_3_4_021_property_accepts_inline_modifier_for_both_accessors() {
    let source = "inline val valueSpec: Int get() = 1\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/InlineProperty.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let property = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "valueSpec")
        .expect("inline property must be indexed");
    assert!(property.detail.starts_with("inline val valueSpec"));
}

#[test]
#[ignore = "KS-4.3.4-022: kmp-lsp does not diagnose backing fields on inline properties"]
fn ks_4_3_4_022_inline_property_cannot_have_backing_field() {
    assert_source_parses("inline val validSpec: Int get() = 1\n");
    assert_source_has_syntax_error("inline val initializedSpec: Int = 1\n");
    assert_source_has_syntax_error("inline val fieldSpec: Int get() = field\n");
}

#[test]
fn ks_4_3_5_001_read_only_and_mutable_properties_accept_delegates() {
    let source = "class DelegateSpec\nval readOnlySpec: Int by DelegateSpec()\nvar mutableSpec: Int by DelegateSpec()\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/DelegatedProperties.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    let read_only = symbols
        .iter()
        .find(|symbol| symbol.name == "readOnlySpec")
        .expect("delegated read-only property must be indexed");
    assert_eq!(read_only.kind, SymbolKind::PROPERTY);
    assert!(read_only.detail.contains("by DelegateSpec()"));
    let mutable = symbols
        .iter()
        .find(|symbol| symbol.name == "mutableSpec")
        .expect("delegated mutable property must be indexed");
    assert_eq!(mutable.kind, SymbolKind::VARIABLE);
    assert!(mutable.detail.contains("by DelegateSpec()"));
}

#[test]
fn ks_4_3_5_002_delegated_property_type_may_be_omitted() {
    assert_source_parses("class DelegateSpec\nval inferredSpec by DelegateSpec()\n");
}

#[test]
fn ks_4_3_5_003_delegate_expression_is_allowed_in_every_property_scope() {
    assert_source_parses(
        "class DelegateSpec\nval topLevelSpec by DelegateSpec()\nclass HostSpec { val memberSpec by DelegateSpec(); }\nfun localSpec() { val localValueSpec by DelegateSpec() }\n",
    );
}

#[test]
fn ks_4_3_5_004_provide_delegate_operator_declaration_and_use_parse() {
    assert_source_parses(
        "import kotlin.reflect.KProperty\nclass ValueDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; }\nclass ProviderSpec { operator fun provideDelegate(thisReferenceSpec: Any?, propertySpec: KProperty<*>): ValueDelegateSpec = ValueDelegateSpec(); }\nval valueSpec: Int by ProviderSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.3.5-005: kmp-lsp does not validate delegated getValue availability"]
fn ks_4_3_5_005_read_only_delegate_requires_suitable_get_value() {
    assert_source_parses(
        "import kotlin.reflect.KProperty\nclass ValidDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; }\nval validSpec: Int by ValidDelegateSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidDelegateSpec\nval invalidSpec: Int by InvalidDelegateSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.3.5-006: kmp-lsp does not validate delegated setValue availability"]
fn ks_4_3_5_006_mutable_delegate_requires_suitable_set_value() {
    assert_source_parses(
        "import kotlin.reflect.KProperty\nclass ValidDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; operator fun setValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>, newValueSpec: Int) {}; }\nvar validSpec: Int by ValidDelegateSpec()\n",
    );
    assert_source_has_syntax_error(
        "import kotlin.reflect.KProperty\nclass InvalidDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; }\nvar invalidSpec: Int by InvalidDelegateSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.3.5-007: kmp-lsp does not diagnose failed delegated type inference"]
fn ks_4_3_5_007_omitted_delegated_type_must_be_inferable() {
    assert_source_parses(
        "import kotlin.reflect.KProperty\nclass ValidDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; }\nval validSpec by ValidDelegateSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidDelegateSpec\nval invalidSpec by InvalidDelegateSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.3.5-008: kmp-lsp does not validate provided-delegate accessors"]
fn ks_4_3_5_008_provided_delegate_must_supply_suitable_accessors() {
    assert_source_parses(
        "import kotlin.reflect.KProperty\nclass ValueDelegateSpec { operator fun getValue(thisReferenceSpec: Any?, propertySpec: KProperty<*>): Int = 1; }\nclass ValidProviderSpec { operator fun provideDelegate(thisReferenceSpec: Any?, propertySpec: KProperty<*>): ValueDelegateSpec = ValueDelegateSpec(); }\nval validSpec: Int by ValidProviderSpec()\n",
    );
    assert_source_has_syntax_error(
        "import kotlin.reflect.KProperty\nclass EmptyDelegateSpec\nclass InvalidProviderSpec { operator fun provideDelegate(thisReferenceSpec: Any?, propertySpec: KProperty<*>): EmptyDelegateSpec = EmptyDelegateSpec(); }\nval invalidSpec: Int by InvalidProviderSpec()\n",
    );
}
