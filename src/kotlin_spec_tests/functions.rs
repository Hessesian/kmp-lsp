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
    let specification_uri = Url::parse("file:///kotlin-spec/Functions.kt")
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
fn ks_4_2_001_simple_function_indexes_name_parameters_return_type_and_body_shape() {
    let source = "fun renderSpec(valueSpec: Int, labelSpec: String = \"item\"): String = labelSpec + valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/SimpleFunction.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let function = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "renderSpec")
        .expect("function must be indexed");
    assert_eq!(function.kind, SymbolKind::FUNCTION);
    assert_eq!(
        function.params,
        "valueSpec: Int, labelSpec: String = \"item\""
    );
    assert_eq!(function.param_counts, (1, 2));
    assert!(function.detail.ends_with(": String"));
}

#[test]
fn ks_4_2_002_function_signature_boundedly_represents_its_function_type() {
    let source = "fun transformSpec(valueSpec: Int, labelSpec: String): Boolean = valueSpec.toString() == labelSpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/FunctionTypeSignature.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let function = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "transformSpec")
        .expect("function must be indexed");
    assert_eq!(function.params, "valueSpec: Int, labelSpec: String");
    assert!(function.detail.ends_with(": Boolean"));
}

#[tokio::test]
#[ignore = "KS-4.2-003: kmp-lsp resolves a parameter use to a competing top-level name"]
async fn ks_4_2_003_function_parameters_bind_names_inside_the_body() {
    let source =
        "val valueSpec = 99\nfun renderSpec(valueSpec: Int): String = valueSpec.toString()\n";
    let position = definition_position(source, "valueSpec", 2).await;
    assert_eq!(position, Some(Position::new(1, 15)));
}

#[test]
#[ignore = "KS-4.2-004: kmp-lsp does not diagnose assignment to final function parameters"]
fn ks_4_2_004_function_parameters_are_final() {
    assert_source_parses("fun validSpec(valueSpec: Int): Int = valueSpec\n");
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int): Int { valueSpec = 2; return valueSpec }\n",
    );
}

#[test]
fn ks_4_2_005_function_accepts_zero_or_more_parameters() {
    let source = "fun zeroSpec(): Unit = Unit\nfun manySpec(firstSpec: Int, secondSpec: String, thirdSpec: Boolean): Unit = Unit\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/FunctionParameterCounts.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    let zero = symbols
        .iter()
        .find(|symbol| symbol.name == "zeroSpec")
        .expect("zero-parameter function must be indexed");
    assert_eq!(zero.param_counts, (0, 0));
    let many = symbols
        .iter()
        .find(|symbol| symbol.name == "manySpec")
        .expect("multi-parameter function must be indexed");
    assert_eq!(many.param_counts, (3, 3));
}

#[test]
fn ks_4_2_006_default_parameter_boundedly_allows_omitted_arguments() {
    let source = "fun labelSpec(valueSpec: Int, suffixSpec: String = \"px\"): String = valueSpec.toString() + suffixSpec\nval defaultedSpec = labelSpec(4)\nval explicitSpec = labelSpec(4, \"dp\")\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/DefaultFunctionParameter.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let function = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "labelSpec")
        .expect("defaulted function must be indexed");
    assert_eq!(function.param_counts, (1, 2));
}

#[test]
#[ignore = "KS-4.2-008: kmp-lsp does not infer top-level expression-body return types"]
fn ks_4_2_008_expression_body_infers_non_nothing_return_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/ExpressionReturn.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "fun inferredSpec() = \"value\"\nfun misleadingSpec(): Int = 1\n",
    );
    assert_eq!(
        indexer.find_fun_return_type("inferredSpec").as_deref(),
        Some("String")
    );
    assert_ne!(
        indexer.find_fun_return_type("misleadingSpec").as_deref(),
        Some("String")
    );
}

#[test]
#[ignore = "KS-4.2-009: kmp-lsp does not expose implicit Unit for block-body functions"]
fn ks_4_2_009_block_body_without_return_type_maps_to_unit() {
    let specification_uri = Url::parse("file:///kotlin-spec/BlockReturn.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "fun runSpec() { println(\"done\") }\nfun misleadingSpec(): String = \"done\"\n",
    );
    assert_eq!(
        indexer.find_fun_return_type("runSpec").as_deref(),
        Some("Unit")
    );
}

#[test]
#[ignore = "KS-4.2-010: kmp-lsp does not diagnose omitted non-inferable return types"]
fn ks_4_2_010_return_type_is_required_when_it_cannot_be_inferred() {
    assert_source_parses("abstract class BaseSpec { abstract fun validSpec(): String; }\n");
    assert_source_has_syntax_error("abstract class BaseSpec { abstract fun invalidSpec(); }\n");
}

#[test]
#[ignore = "KS-4.2-011: kmp-lsp does not require explicit Nothing return types"]
fn ks_4_2_011_nothing_return_type_must_be_explicit() {
    assert_source_parses("fun failSpec(): Nothing = throw IllegalStateException()\n");
    assert_source_has_syntax_error("fun invalidFailSpec() = throw IllegalStateException()\n");
}

#[test]
#[ignore = "KS-4.2-012: kmp-lsp does not diagnose bodyless concrete functions"]
fn ks_4_2_012_bodyless_function_is_allowed_only_as_abstract_member() {
    assert_source_parses(
        "abstract class BaseSpec { abstract fun classSpec(): String; }\ninterface ContractSpec { fun interfaceSpec(): String; }\n",
    );
    assert_source_has_syntax_error("fun invalidTopLevelSpec(): String\n");
    assert_source_has_syntax_error("class InvalidSpec { fun memberSpec(): String; }\n");
}

#[test]
fn ks_4_2_014_parameterized_function_indexes_type_parameters_and_signature() {
    let source =
        "fun <ElementSpec> identitySpec(valueSpec: ElementSpec): ElementSpec = valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ParameterizedFunction.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let function = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "identitySpec")
        .expect("parameterized function must be indexed");
    assert_eq!(function.params, "valueSpec: ElementSpec");
    assert!(function.detail.contains("<ElementSpec>"));
    assert!(function.detail.ends_with(": ElementSpec"));
}

#[test]
fn ks_4_2_1_001_function_signature_contains_name_type_parameters_and_parameter_types() {
    let source = "fun <ElementSpec> convertSpec(valueSpec: ElementSpec): String = valueSpec.toString()\nfun <ElementSpec> convertSpec(valueSpec: ElementSpec): Int = 1\n";
    let specification_uri = Url::parse("file:///kotlin-spec/FunctionSignatureParts.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let functions: Vec<_> = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| symbol.name == "convertSpec")
        .collect();
    assert_eq!(functions.len(), 2);
    for function in functions {
        assert_eq!(function.name, "convertSpec");
        assert_eq!(function.params, "valueSpec: ElementSpec");
        assert!(function.detail.contains("<ElementSpec>"));
    }
}

#[tokio::test]
async fn ks_4_2_2_001_named_argument_binds_to_declaration_parameter_name() {
    let source = "fun combineSpec(firstSpec: Int, secondSpec: String): String = secondSpec + firstSpec\nval resultSpec = combineSpec(secondSpec = \"value\", firstSpec = 1)\n";
    let second_position = definition_position(source, "secondSpec", 2).await;
    assert_eq!(second_position, Some(Position::new(0, 32)));
    let first_position = definition_position(source, "firstSpec", 2).await;
    assert_eq!(first_position, Some(Position::new(0, 16)));
}

#[test]
#[ignore = "KS-4.2.2-002: kmp-lsp does not diagnose duplicate named arguments"]
fn ks_4_2_2_002_named_parameter_cannot_be_bound_more_than_once() {
    assert_source_parses(
        "fun consumeSpec(valueSpec: Int): Unit = Unit\nval validSpec = consumeSpec(valueSpec = 1)\n",
    );
    assert_source_has_syntax_error(
        "fun consumeSpec(valueSpec: Int): Unit = Unit\nval invalidSpec = consumeSpec(valueSpec = 1, valueSpec = 2)\n",
    );
}

#[test]
#[ignore = "KS-4.2.2-003: kmp-lsp does not diagnose unknown named arguments"]
fn ks_4_2_2_003_named_argument_must_match_a_declared_parameter() {
    assert_source_parses(
        "fun consumeSpec(valueSpec: Int): Unit = Unit\nval validSpec = consumeSpec(valueSpec = 1)\n",
    );
    assert_source_has_syntax_error(
        "fun consumeSpec(valueSpec: Int): Unit = Unit\nval invalidSpec = consumeSpec(missingSpec = 1)\n",
    );
}

#[test]
#[ignore = "KS-4.2.2-004: kmp-lsp does not diagnose positional arguments after the named suffix begins"]
fn ks_4_2_2_004_mixed_arguments_have_positional_or_named_prefix_and_named_suffix() {
    assert_source_parses(
        "fun combineSpec(firstSpec: Int, secondSpec: Int, thirdSpec: Int): Int = firstSpec + secondSpec + thirdSpec\nval validSpec = combineSpec(firstSpec = 1, 2, thirdSpec = 3)\n",
    );
    assert_source_has_syntax_error(
        "fun combineSpec(firstSpec: Int, secondSpec: Int, thirdSpec: Int): Int = firstSpec + secondSpec + thirdSpec\nval invalidSpec = combineSpec(firstSpec = 1, thirdSpec = 3, 2)\n",
    );
}

#[test]
fn ks_4_2_2_005_named_vararg_accepts_regular_array_or_spread_array() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: Int): Unit = Unit\nval regularSpec = consumeSpec(valuesSpec = intArrayOf(1, 2))\nval spreadSpec = consumeSpec(valuesSpec = *intArrayOf(1, 2))\n",
    );
}

#[test]
fn ks_4_2_2_007_missing_arguments_boundedly_map_to_declared_defaults() {
    let source = "fun formatSpec(countSpec: Int = 1, scaleSpec: Double = 2.0, labelSpec: String = \"item\"): String = labelSpec\nval allDefaultsSpec = formatSpec()\nval suffixDefaultsSpec = formatSpec(2)\nval middleDefaultSpec = formatSpec(2, labelSpec = \"value\")\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/DefaultArgumentBinding.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let function = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "formatSpec")
        .expect("defaulted function must be indexed");
    assert_eq!(function.param_counts, (0, 3));
}

#[test]
#[ignore = "KS-4.2.2-008: kmp-lsp does not diagnose middle positional default ambiguity"]
fn ks_4_2_2_008_default_cannot_fill_middle_positional_parameter() {
    assert_source_parses(
        "fun formatSpec(countSpec: Int, scaleSpec: Double = 2.0, labelSpec: String): String = labelSpec\nval validSpec = formatSpec(1, labelSpec = \"item\")\n",
    );
    assert_source_has_syntax_error(
        "fun formatSpec(countSpec: Int, scaleSpec: Double = 2.0, labelSpec: String): String = labelSpec\nval invalidSpec = formatSpec(1, \"item\")\n",
    );
}
