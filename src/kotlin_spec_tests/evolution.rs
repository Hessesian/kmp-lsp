use std::sync::Arc;

use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::features::fill_when::when_diagnostics;
use crate::indexer::{live_tree::parse_live, Indexer};
use crate::inlay_hints::compute_inlay_hints;
use crate::semantic_tokens::{full_tokens, TOKEN_MODIFIERS};
use crate::Language;
use tower_lsp::lsp_types::{
    GotoDefinitionResponse, InlayHintLabel, Location, Position, Range, SemanticTokenModifier,
    SemanticTokens, Url,
};

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
        Url::parse("file:///kotlin-spec/Evolution.kt").expect("specification URI must be valid");
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

async fn cross_file_definition_location(
    declaration_source: &str,
    usage_source: &str,
    needle: &str,
    occurrence: usize,
) -> Option<Location> {
    let declaration_uri = Url::parse("file:///kotlin-spec/RootDeclarations.kt")
        .expect("declaration URI must be valid");
    let usage_uri = Url::parse("file:///kotlin-spec/Usage.kt").expect("usage URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&declaration_uri, declaration_source);
    indexer.index_content(&usage_uri, usage_source);
    let position = position_of_occurrence(usage_source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &usage_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &usage_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => Some(location),
        Some(GotoDefinitionResponse::Array(locations)) if locations.len() == 1 => {
            locations.into_iter().next()
        }
        Some(GotoDefinitionResponse::Array(_)) | Some(GotoDefinitionResponse::Link(_)) | None => {
            None
        }
    }
}

fn inlay_hint_labels(source: &str) -> Vec<String> {
    let specification_uri = Url::parse("file:///kotlin-spec/EvolutionInference.kt")
        .expect("specification URI must be valid");
    let indexer = Arc::new(Indexer::new());
    indexer.index_content(&specification_uri, source);
    let line_count = source.lines().count() as u32;
    compute_inlay_hints(
        &indexer,
        &specification_uri,
        Range::new(Position::new(0, 0), Position::new(line_count, 0)),
    )
    .into_iter()
    .filter_map(|hint| match hint.label {
        InlayHintLabel::String(label) => Some(label),
        InlayHintLabel::LabelParts(_) => None,
    })
    .collect()
}

fn when_diagnostic_messages(source: &str) -> Vec<String> {
    let specification_uri = Url::parse("file:///kotlin-spec/EvolutionWhen.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    indexer.store_live_tree(&specification_uri, source);
    indexer.set_live_lines(&specification_uri, source);
    when_diagnostics(&indexer, &specification_uri)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

fn decode_semantic_tokens(tokens: &SemanticTokens) -> Vec<(Position, u32)> {
    let mut line = 0;
    let mut character = 0;
    tokens
        .data
        .iter()
        .map(|token| {
            line += token.delta_line;
            if token.delta_line > 0 {
                character = token.delta_start;
            } else {
                character += token.delta_start;
            }
            (Position::new(line, character), token.token_modifiers_bitset)
        })
        .collect()
}

fn semantic_token_modifiers_at(source: &str, needle: &str, occurrence: usize) -> u32 {
    let specification_uri = Url::parse("file:///kotlin-spec/EvolutionSemanticTokens.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let document = parse_live(source, tree_sitter_kotlin::language())
        .expect("evolution fixture must produce a live Kotlin document");
    let position = position_of_occurrence(source, needle, occurrence);
    decode_semantic_tokens(&full_tokens(
        &indexer,
        &specification_uri,
        &document,
        Language::Kotlin,
    ))
    .into_iter()
    .find_map(|(token_position, modifiers)| (token_position == position).then_some(modifiers))
    .expect("fixture identifier must receive a semantic token")
}

fn deprecated_modifier_bit() -> u32 {
    let modifier_index = TOKEN_MODIFIERS
        .iter()
        .position(|modifier| modifier == &SemanticTokenModifier::DEPRECATED)
        .expect("semantic-token legend must contain the deprecated modifier");
    1 << modifier_index
}

#[test]
#[ignore = "KL-1-9-0001: tree-sitter-kotlin accepts class literals without a left-hand side"]
fn kl_1_9_0001_class_literal_requires_a_left_hand_side() {
    assert_source_parses("val validSpec = String::class\n");
    assert_source_has_syntax_error("val invalidSpec = ::class\n");
}

#[tokio::test]
async fn kl_1_9_0002_callable_reference_keeps_target_when_expected_type_conflicts() {
    let source = "class ReferencedSpec\nclass ExpectedSpec\nval invalidSpec: ExpectedSpec = ::ReferencedSpec\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "ReferencedSpec", 1).await,
        Some(position_of_occurrence(source, "ReferencedSpec", 0))
    );
}

#[test]
#[ignore = "KL-1-9-0003: kmp-lsp does not diagnose callable references to enum entries"]
fn kl_1_9_0003_enum_entry_cannot_be_used_as_a_callable_reference() {
    assert_source_parses(
        "class ValidSpec {\n    fun memberSpec(): Unit {}\n}\nval validReferenceSpec = ValidSpec::memberSpec\n",
    );
    assert_source_has_syntax_error(
        "enum class StateSpec { ReadySpec }\nval invalidReferenceSpec = StateSpec::ReadySpec\n",
    );
}

#[tokio::test]
#[ignore = "KL-1-9-0008: kmp-lsp resolves synthetic enum entries to the competing companion property"]
async fn kl_1_9_0008_synthetic_enum_entries_precedes_companion_entries() {
    let source = "enum class StateSpec {\n    ReadySpec;\n    companion object {\n        val entries: String = \"companion\"\n    }\n}\nval selectedSpec = StateSpec.entries\nval companionSpec = StateSpec.Companion.entries\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "entries", 2).await,
        Some(position_of_occurrence(source, "entries", 0)),
        "the explicit companion path must resolve to the companion declaration"
    );
    assert_eq!(
        definition_position(source, "entries", 1).await,
        None,
        "the synthetic enum property must not resolve to the companion declaration"
    );
}

#[test]
#[ignore = "KL-1-9-0004: kmp-lsp does not propagate deprecation to enum-entry reference tokens"]
fn kl_1_9_0004_deprecated_enum_entry_reference_has_deprecated_semantic_token() {
    let source = "enum class StateSpec {\n    @Deprecated(\"legacy\") LegacySpec,\n    CurrentSpec\n}\nval legacySpec = StateSpec.LegacySpec\nval currentSpec = StateSpec.CurrentSpec\n";
    assert_source_parses(source);
    let deprecated_bit = deprecated_modifier_bit();
    let legacy_modifiers = semantic_token_modifiers_at(source, "LegacySpec", 1);
    let current_modifiers = semantic_token_modifiers_at(source, "CurrentSpec", 1);
    assert_ne!(legacy_modifiers & deprecated_bit, 0);
    assert_eq!(current_modifiers & deprecated_bit, 0);
}

#[test]
#[ignore = "KL-1-9-0005: kmp-lsp does not diagnose named arguments on function-type calls"]
fn kl_1_9_0005_function_type_call_forbids_named_arguments() {
    assert_source_parses(
        "fun validSpec(callbackSpec: (valueSpec: String) -> Unit) { callbackSpec(\"value\") }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(callbackSpec: (valueSpec: String) -> Unit) { callbackSpec(valueSpec = \"value\") }\n",
    );
}

#[test]
fn kl_1_9_0006_extension_function_type_is_forbidden_as_a_supertype() {
    assert_source_parses("class ValidSpec : () -> Unit {\n    override fun invoke(): Unit {}\n}\n");
    assert_source_has_syntax_error(
        "class InvalidSpec : String.() -> Unit { override fun invoke(): Unit {} }\n",
    );
}

#[tokio::test]
#[ignore = "KL-1-9-0007: kmp-lsp does not resolve a companion property competing with a type parameter"]
async fn kl_1_9_0007_type_parameter_name_is_not_a_value_expression() {
    let source = "class OwnerSpec<valueSpec> {\n    companion object {\n        val valueSpec: Int = 1\n    }\n    inner class InnerSpec<valueSpec> {\n        val selectedSpec = valueSpec\n    }\n}\nval valueSpec: Int = 2\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 3).await,
        Some(position_of_occurrence(source, "valueSpec", 1))
    );
}

#[tokio::test]
async fn kl_2_0_0001_root_package_declaration_requires_an_import_in_a_named_package() {
    let declaration_source = "class RootSpec\n";
    let unimported_source = "package nested\nval unimportedSpec: RootSpec? = null\n";
    assert_eq!(
        cross_file_definition_location(declaration_source, unimported_source, "RootSpec", 0).await,
        None,
        "a named package must not see a root-package declaration implicitly"
    );

    let imported_source = "package nested\nimport RootSpec\nval importedSpec: RootSpec? = null\n";
    let imported_location =
        cross_file_definition_location(declaration_source, imported_source, "RootSpec", 1)
            .await
            .expect("an explicit root-package import must resolve");
    assert_eq!(
        imported_location.uri,
        Url::parse("file:///kotlin-spec/RootDeclarations.kt")
            .expect("declaration URI must be valid")
    );
    assert_eq!(
        imported_location.range.start,
        position_of_occurrence(declaration_source, "RootSpec", 0)
    );
}

#[test]
#[ignore = "KL-2-0-0008: tree-sitter-kotlin does not parse multi-dollar interpolation"]
fn kl_2_0_0008_multi_dollar_interpolation_uses_the_selected_prefix_length() {
    assert_source_parses(
        "val valueSpec = 42\nval textSpec = $$\"literal $valueSpec and interpolated $$valueSpec\"\n",
    );
}

#[tokio::test]
#[ignore = "KL-2-0-0009: tree-sitter-kotlin does not parse when guards"]
async fn kl_2_0_0009_when_guard_accepts_a_boolean_condition_after_a_primary_condition() {
    let source = "sealed interface StateSpec\ndata class ReadySpec(val enabledSpec: Boolean) : StateSpec\ndata object DoneSpec : StateSpec\nfun renderSpec(stateSpec: StateSpec) = when (stateSpec) {\n    is ReadySpec if stateSpec.enabledSpec -> \"ready\"\n    is ReadySpec -> \"disabled\"\n    DoneSpec -> \"done\"\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "enabledSpec", 1).await,
        Some(position_of_occurrence(source, "enabledSpec", 0))
    );
}

#[test]
fn kl_2_0_0002_elvis_condition_smart_casts_its_safe_call_receiver() {
    let source = "interface OrderSpec {\n    val expiredSpec: Boolean?\n    val numberSpec: Int\n}\nfun readSpec(valueSpec: Any) {\n    val orderSpec = valueSpec as? OrderSpec\n    if (orderSpec?.expiredSpec ?: false) {\n        val numberSpec = orderSpec.numberSpec\n    }\n}\n";
    assert_source_parses(source);
    assert!(inlay_hint_labels(source)
        .iter()
        .any(|label| label == ": Int"));
}

#[test]
fn kl_2_0_0003_disjunction_smart_casts_to_the_common_supertype() {
    let source = "sealed interface StateSpec {\n    val labelSpec: String\n}\nclass ReadySpec : StateSpec {\n    override val labelSpec = \"ready\"\n}\nclass DoneSpec : StateSpec {\n    override val labelSpec = \"done\"\n}\nfun readSpec(valueSpec: Any?) {\n    if (valueSpec is ReadySpec || valueSpec is DoneSpec) {\n        val labelSpec = valueSpec.labelSpec\n    }\n}\n";
    assert_source_parses(source);
    assert!(inlay_hint_labels(source)
        .iter()
        .any(|label| label == ": String"));
}

#[test]
#[ignore = "KL-2-0-0004: kmp-lsp does not infer member result types after boolean early exits"]
fn kl_2_0_0004_boolean_early_exit_smart_casts_the_surviving_path() {
    let source = "fun readSpec(valueSpec: String?) {\n    valueSpec != null || return\n    val lengthSpec = valueSpec.length\n}\n";
    assert_source_parses(source);
    assert!(inlay_hint_labels(source)
        .iter()
        .any(|label| label == ": Int"));
}

#[test]
#[ignore = "KL-2-0-0005: kmp-lsp does not infer the getter type of prefix increment"]
fn kl_2_0_0005_prefix_increment_has_the_getter_return_type() {
    let source = "open class CounterSpec {\n    operator fun inc(): AdvancedCounterSpec = AdvancedCounterSpec()\n}\nclass AdvancedCounterSpec : CounterSpec()\nvar counterSpec: CounterSpec = CounterSpec()\nfun updateSpec() {\n    val updatedSpec = ++counterSpec\n}\n";
    assert_source_parses(source);
    assert!(inlay_hint_labels(source)
        .iter()
        .any(|label| label == ": CounterSpec"));
}

#[tokio::test]
#[ignore = "KL-2-0-0006: kmp-lsp does not resolve inherited annotations on companion objects"]
async fn kl_2_0_0006_companion_annotation_ignores_the_companion_scope() {
    let source = "open class ParentSpec {\n    annotation class MarkerSpec\n}\nclass ChildSpec : ParentSpec() {\n    @MarkerSpec\n    companion object {\n        annotation class MarkerSpec\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "MarkerSpec", 1).await,
        Some(position_of_occurrence(source, "MarkerSpec", 0))
    );
}

#[test]
#[ignore = "KL-2-0-0007: kmp-lsp treats empty sealed and enum when expressions as exhaustive"]
fn kl_2_0_0007_empty_bounded_type_when_expression_is_not_exhaustive() {
    let sealed_source = "sealed interface EmptyStateSpec\nfun readSealedSpec(stateSpec: EmptyStateSpec) = when (stateSpec) {}\n";
    assert_source_parses(sealed_source);
    assert!(!when_diagnostic_messages(sealed_source).is_empty());

    let enum_source = "enum class EmptyEnumSpec {}\nfun readEnumSpec(stateSpec: EmptyEnumSpec?) = when (stateSpec) {}\n";
    assert_source_parses(enum_source);
    assert!(!when_diagnostic_messages(enum_source).is_empty());
}

#[tokio::test]
async fn kl_2_1_0001_root_package_object_requires_an_import_in_a_named_package() {
    let declaration_source = "object RootObjectSpec\n";
    let unimported_source = "package nested\nval unimportedSpec = RootObjectSpec\n";
    assert_eq!(
        cross_file_definition_location(declaration_source, unimported_source, "RootObjectSpec", 0,)
            .await,
        None,
        "a named package must not see a root-package object implicitly"
    );

    let imported_source =
        "package nested\nimport RootObjectSpec\nval importedSpec = RootObjectSpec\n";
    let imported_location =
        cross_file_definition_location(declaration_source, imported_source, "RootObjectSpec", 1)
            .await
            .expect("an explicit root-package object import must resolve");
    assert_eq!(
        imported_location.uri,
        Url::parse("file:///kotlin-spec/RootDeclarations.kt")
            .expect("declaration URI must be valid")
    );
    assert_eq!(
        imported_location.range.start,
        position_of_occurrence(declaration_source, "RootObjectSpec", 0)
    );
}

#[tokio::test]
#[ignore = "KL-2-1-0002: tree-sitter-kotlin does not parse named context parameters"]
async fn kl_2_1_0002_context_parameter_is_in_scope_in_the_declaration_body() {
    let source = "class LoggerSpec {\n    fun messageSpec(): String = \"ready\"\n}\ncontext(loggerSpec: LoggerSpec)\nfun renderSpec(): String = loggerSpec.messageSpec()\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "loggerSpec", 1).await,
        Some(position_of_occurrence(source, "loggerSpec", 0))
    );
}

#[test]
#[ignore = "KL-2-1-0003: kmp-lsp does not inspect sealed upper bounds for when exhaustiveness"]
fn kl_2_1_0003_generic_sealed_upper_bound_makes_when_exhaustive() {
    let exhaustive_source = "sealed interface StateSpec\nclass ReadySpec : StateSpec\nobject DoneSpec : StateSpec\nfun <ValueSpec : StateSpec> renderSpec(valueSpec: ValueSpec) = when (valueSpec) {\n    is ReadySpec -> \"ready\"\n    DoneSpec -> \"done\"\n}\n";
    assert_source_parses(exhaustive_source);
    assert!(when_diagnostic_messages(exhaustive_source).is_empty());

    let non_exhaustive_source = "sealed interface StateSpec\nclass ReadySpec : StateSpec\nobject DoneSpec : StateSpec\nfun <ValueSpec : StateSpec> renderSpec(valueSpec: ValueSpec) = when (valueSpec) {\n    is ReadySpec -> \"ready\"\n}\n";
    assert_source_parses(non_exhaustive_source);
    assert!(!when_diagnostic_messages(non_exhaustive_source).is_empty());
}

#[tokio::test]
async fn kl_2_1_0004_legacy_keywords_are_valid_enum_entry_names() {
    let source = "enum class StateSpec { header, impl }\nval headerSpec = StateSpec.header\nval implementationSpec = StateSpec.impl\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "header", 2).await,
        Some(position_of_occurrence(source, "header", 0))
    );
    assert_eq!(
        definition_position(source, "impl", 2).await,
        Some(position_of_occurrence(source, "impl", 0))
    );
}

#[test]
fn kl_2_1_0005_package_declaration_rejects_modifiers() {
    assert_source_parses("package valid.packageSpec\nclass ValidSpec\n");
    assert_source_has_syntax_error("public package invalid.packageSpec\nclass InvalidSpec\n");
}

#[test]
#[ignore = "KL-2-1-0006: tree-sitter-kotlin does not parse the all annotation use-site target"]
fn kl_2_1_0006_all_annotation_use_site_target_is_accepted() {
    assert_source_parses(
        "annotation class MarkerSpec\ndata class ModelSpec(@all:MarkerSpec val valueSpec: String)\n",
    );
}

#[tokio::test]
async fn kl_2_1_0007_inherited_nested_type_alias_resolves_in_a_derived_class() {
    let source = "class EntitySpec\nopen class BaseSpec {\n    typealias EntityAliasSpec = EntitySpec\n}\nclass DerivedSpec : BaseSpec() {\n    val entitySpec: EntityAliasSpec? = null\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "EntityAliasSpec", 1).await,
        Some(position_of_occurrence(source, "EntityAliasSpec", 0))
    );
}

#[test]
fn kl_2_2_0001_underscore_declares_an_unnamed_local_variable() {
    assert_source_parses(
        "fun saveSpec(): Boolean = true\nfun recordSpec() {\n    val _ = saveSpec()\n    for (_ in 1..3) { saveSpec() }\n}\n",
    );
}

#[tokio::test]
#[ignore = "KL-2-2-0002: kmp-lsp does not use expected types for unqualified member resolution"]
async fn kl_2_2_0002_context_sensitive_resolution_uses_expected_types() {
    let source = "enum class StateSpec { ReadySpec }\nenum class DecoyStateSpec { ReadySpec }\nannotation class MarkerSpec(val stateSpec: StateSpec)\nfun consumeSpec(stateSpec: StateSpec): Unit {}\n@MarkerSpec(ReadySpec)\nfun useSpec() {\n    val currentSpec: StateSpec = ReadySpec\n    consumeSpec(ReadySpec)\n}\nsealed interface ResultSpec {\n    class SuccessSpec : ResultSpec\n}\nclass DecoyResultSpec {\n    class SuccessSpec\n}\nfun renderSpec(resultSpec: ResultSpec): String = when (resultSpec) {\n    is SuccessSpec -> \"success\"\n}\n";
    assert_source_parses(source);
    let expected_enum_position = Some(position_of_occurrence(source, "ReadySpec", 0));
    assert_eq!(
        definition_position(source, "ReadySpec", 2).await,
        expected_enum_position
    );
    assert_eq!(
        definition_position(source, "ReadySpec", 3).await,
        expected_enum_position
    );
    assert_eq!(
        definition_position(source, "ReadySpec", 4).await,
        expected_enum_position
    );
    assert_eq!(
        definition_position(source, "SuccessSpec", 2).await,
        Some(position_of_occurrence(source, "SuccessSpec", 0))
    );
}

#[test]
#[ignore = "KL-2-2-0003: kmp-lsp when diagnostics do not use preceding data-flow facts"]
fn kl_2_2_0003_data_flow_facts_make_when_exhaustive() {
    let guarded_source = "enum class StateSpec {\n    ReadySpec,\n    DoneSpec\n}\nfun renderSpec(stateSpec: StateSpec): String {\n    if (stateSpec != StateSpec.DoneSpec) return \"ready\"\n    return when (stateSpec) {\n        StateSpec.DoneSpec -> \"done\"\n    }\n}\n";
    assert_source_parses(guarded_source);
    assert!(when_diagnostic_messages(guarded_source).is_empty());

    let assigned_source = "enum class StateSpec {\n    ReadySpec,\n    DoneSpec\n}\nfun renderSpec(stateSpec: StateSpec): String {\n    var currentSpec = stateSpec\n    currentSpec = StateSpec.ReadySpec\n    return when (currentSpec) {\n        StateSpec.ReadySpec -> \"ready\"\n    }\n}\n";
    assert_source_parses(assigned_source);
    assert!(when_diagnostic_messages(assigned_source).is_empty());

    let incomplete_source = "enum class StateSpec {\n    ReadySpec,\n    DoneSpec\n}\nfun renderSpec(stateSpec: StateSpec): String = when (stateSpec) {\n    StateSpec.DoneSpec -> \"done\"\n}\n";
    assert_source_parses(incomplete_source);
    assert!(!when_diagnostic_messages(incomplete_source).is_empty());
}

#[test]
#[ignore = "KL-2-2-0004: kmp-lsp does not smart-cast after an inline-lambda Elvis exit"]
fn kl_2_2_0004_inline_lambda_exit_smart_casts_after_elvis() {
    let source = "inline fun <ResultSpec> callSpec(blockSpec: () -> ResultSpec): ResultSpec = blockSpec()\nfun readSpec(valueSpec: String?) {\n    valueSpec ?: callSpec { return }\n    val lengthSpec = valueSpec.length\n}\n";
    assert_source_parses(source);
    assert!(inlay_hint_labels(source)
        .iter()
        .any(|label| label == ": Int"));
}

#[tokio::test]
#[ignore = "KL-2-2-0005: tree-sitter-kotlin does not parse local named context parameters"]
async fn kl_2_2_0005_local_context_parameter_is_in_scope() {
    let source = "class LoggerSpec {\n    fun messageSpec(): String = \"ready\"\n}\nfun renderSpec(loggerSpec: LoggerSpec): String {\n    context(localLoggerSpec: LoggerSpec)\n    fun localSpec(): String = localLoggerSpec.messageSpec()\n    return with(loggerSpec) { localSpec() }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "localLoggerSpec", 1).await,
        Some(position_of_occurrence(source, "localLoggerSpec", 0))
    );
}
