use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
use crate::resolver::resolve_symbol;
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
    let specification_uri =
        Url::parse("file:///kotlin-spec/Scopes.kt").expect("specification URI must be valid");
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
fn ks_6_001_declaration_scopes_bind_types_and_values() {
    let source = "class ModelSpec\nval valueSpec: ModelSpec = ModelSpec()\nfun createSpec(): ModelSpec = valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ScopeBindings.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for (name, kind) in [
        ("ModelSpec", SymbolKind::CLASS),
        ("valueSpec", SymbolKind::PROPERTY),
        ("createSpec", SymbolKind::FUNCTION),
    ] {
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.name == name && symbol.kind == kind),
            "{name} must introduce a {kind:?} binding"
        );
    }
}

#[tokio::test]
#[ignore = "KS-6-002: kmp-lsp does not resolve forward references in declaration scopes"]
async fn ks_6_002_declaration_scope_allows_forward_reference() {
    let source = "val valueSpec: Int = 99\nclass HostSpec {\n    fun readSpec(): Int = valueSpec\n    val valueSpec: Int = 3\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 1).await,
        Some(Position::new(3, 8))
    );
}

#[tokio::test]
#[ignore = "KS-6-003: kmp-lsp does not model statement-scope binding order"]
async fn ks_6_003_statement_scope_binds_values_in_appearance_order() {
    let source = "val valueSpec: Int = 99\nfun readSpec(): Int {\n    val beforeSpec = valueSpec\n    val valueSpec: Int = 3\n    return beforeSpec + valueSpec\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 1).await,
        Some(Position::new(0, 4))
    );
    assert_eq!(
        definition_position(source, "valueSpec", 3).await,
        Some(Position::new(3, 8))
    );
}

#[test]
#[ignore = "KS-6-004: kmp-lsp does not diagnose same-scope value redeclarations"]
fn ks_6_004_same_scope_value_redeclaration_is_forbidden() {
    assert_source_parses(
        "val valueSpec: Int = 1\nfun readSpec(): Int { val valueSpec: Int = 2; return valueSpec }\n",
    );
    assert_source_has_syntax_error("val valueSpec: Int = 1\nval valueSpec: Int = 2\n");
}

#[test]
fn ks_6_005_same_scope_function_overloads_are_allowed() {
    let source = "fun renderSpec(valueSpec: Int): Int = valueSpec\nfun renderSpec(valueSpec: String): String = valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ScopeOverloads.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let overloads = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| symbol.name == "renderSpec")
        .collect::<Vec<_>>();
    assert_eq!(overloads.len(), 2);
    assert!(overloads
        .iter()
        .all(|symbol| symbol.kind == SymbolKind::FUNCTION));
}

#[test]
#[ignore = "KS-6-006: kmp-lsp does not diagnose same-receiver property redeclarations"]
fn ks_6_006_same_receiver_property_redeclaration_is_forbidden() {
    assert_source_parses("val firstSpec: Int = 1\nval secondSpec: Int = 2\n");
    assert_source_has_syntax_error("val valueSpec: Int = 1\nval valueSpec: String = \"two\"\n");
}

#[test]
fn ks_6_007_top_level_import_introduces_a_binding() {
    let declaration_uri =
        Url::parse("file:///kotlin-spec/library/Values.kt").expect("declaration URI must be valid");
    let usage_uri =
        Url::parse("file:///kotlin-spec/client/Usage.kt").expect("usage URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package library\nval importedSpec: Int = 1\n",
    );
    indexer.index_content(
        &usage_uri,
        "package client\nimport library.importedSpec\nval copiedSpec: Int = importedSpec\n",
    );

    let locations = resolve_symbol(&indexer, "importedSpec", None, &usage_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, declaration_uri);
    assert_eq!(locations[0].range.start, Position::new(1, 4));
}
