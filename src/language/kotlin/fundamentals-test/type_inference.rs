use std::sync::Arc;

use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::indexer::Indexer;
use crate::inlay_hints::compute_inlay_hints;
use tower_lsp::lsp_types::{InlayHintLabel, Position, Range, Url};

fn inlay_hint_labels(source: &str) -> Vec<String> {
    let specification_uri = Url::parse("file:///kotlin-spec/TypeInference.kt")
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

#[test]
#[ignore = "KS-TYPE-INFERENCE-0003: kmp-lsp does not infer member result types through smart casts"]
fn ks_type_inference_0003_stable_type_check_enables_member_result_inference() {
    let labels = inlay_hint_labels(
        "fun inferSpec(valueSpec: Any?) {\n    if (valueSpec is String) {\n        val lengthSpec = valueSpec.length\n    }\n}\n",
    );
    assert!(labels.iter().any(|label| label == ": Int"));
}

#[test]
#[ignore = "KS-TYPE-INFERENCE-0011: kmp-lsp does not preserve the direct-property smart-cast inference exception"]
fn ks_type_inference_0011_direct_property_declaration_uses_the_declared_type() {
    let labels = inlay_hint_labels(
        "fun <ElementSpec> identitySpec(valueSpec: ElementSpec): ElementSpec = valueSpec\nfun inferSpec(valueSpec: Any?) {\n    if (valueSpec == null) return\n    val directSpec = valueSpec\n    val genericSpec = identitySpec(valueSpec)\n}\n",
    );
    assert!(labels.iter().any(|label| label == ": Any?"));
    assert!(labels.iter().any(|label| label == ": Any"));
}

#[test]
#[ignore = "KS-TYPE-INFERENCE-0013: kmp-lsp does not diagnose unstable captured smart-cast sinks"]
fn ks_type_inference_0013_captured_mutable_property_is_not_a_stable_smart_cast_sink() {
    assert_source_parses(
        "fun validSpec(valueSpec: Any?) {\n    if (valueSpec is String) println(valueSpec.length)\n}\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() {\n    var valueSpec: Any? = \"text\"\n    val mutateSpec = { valueSpec = null }\n    if (valueSpec is String) println(valueSpec.length)\n    mutateSpec()\n}\n",
    );
}

#[test]
#[ignore = "KS-TYPE-INFERENCE-0016: kmp-lsp does not diagnose invalid smart casts at direct and nested sinks"]
fn ks_type_inference_0016_effectively_immutable_rules_cover_direct_and_nested_sinks() {
    assert_source_parses(
        "fun directSinkValidSpec() {\n    var valueSpec: Int? = 42\n    if (valueSpec != null) valueSpec.inc()\n    run { valueSpec = null }\n}\nfun nestedSinkValidSpec() {\n    var valueSpec: Int? = 42\n    valueSpec = nullableIntSpec()\n    run { if (valueSpec != null) valueSpec.inc() }\n}\nfun nullableIntSpec(): Int? = null\n",
    );
    assert_source_has_syntax_error(
        "fun directSinkInvalidSpec() {\n    var valueSpec: Int? = 42\n    run { valueSpec = null }\n    if (valueSpec != null) valueSpec.inc()\n}\n",
    );
    assert_source_has_syntax_error(
        "fun nestedSinkInvalidSpec() {\n    var valueSpec: Int? = 42\n    run { if (valueSpec != null) valueSpec.inc() }\n    valueSpec = nullableIntSpec()\n}\nfun nullableIntSpec(): Int? = null\n",
    );
}

#[test]
#[ignore = "KS-TYPE-INFERENCE-0017: kmp-lsp does not propagate semantic smart-cast facts through definitely evaluated loops"]
fn ks_type_inference_0017_definitely_evaluated_loops_propagate_smart_cast_facts() {
    assert_source_parses(
        "fun whileSpec(valueSpec: String?) {\n    var currentSpec = valueSpec\n    while (true) {\n        if (currentSpec == null) return\n        break\n    }\n    println(currentSpec.length)\n}\nfun doWhileSpec(valueSpec: String?) {\n    var currentSpec = valueSpec\n    do {\n        if (currentSpec == null) return\n    } while (false)\n    println(currentSpec.length)\n}\n",
    );
    assert_source_has_syntax_error(
        "fun nonExactLoopSpec(valueSpec: String?) {\n    var currentSpec = valueSpec\n    while (true == true) {\n        if (currentSpec == null) return\n        break\n    }\n    println(currentSpec.length)\n}\n",
    );
}

#[test]
fn ks_type_inference_0020_local_property_type_is_inferred_from_initializer() {
    let labels = inlay_hint_labels("fun inferSpec() { val valueSpec = 42 }\n");
    assert!(labels.iter().any(|label| label == ": Int"));
}
