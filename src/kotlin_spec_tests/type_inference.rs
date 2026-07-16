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
#[ignore = "KS-14.1-001: kmp-lsp does not infer member result types through smart casts"]
fn ks_14_1_001_stable_type_check_enables_member_result_inference() {
    let labels = inlay_hint_labels(
        "fun inferSpec(valueSpec: Any?) {\n    if (valueSpec is String) {\n        val lengthSpec = valueSpec.length\n    }\n}\n",
    );
    assert!(labels.iter().any(|label| label == ": Int"));
}

#[test]
#[ignore = "KS-14.1.3-001: kmp-lsp does not diagnose unstable captured smart-cast sinks"]
fn ks_14_1_3_001_captured_mutable_property_is_not_a_stable_smart_cast_sink() {
    assert_source_parses(
        "fun validSpec(valueSpec: Any?) {\n    if (valueSpec is String) println(valueSpec.length)\n}\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() {\n    var valueSpec: Any? = \"text\"\n    val mutateSpec = { valueSpec = null }\n    if (valueSpec is String) println(valueSpec.length)\n    mutateSpec()\n}\n",
    );
}

#[test]
fn ks_14_2_001_local_property_type_is_inferred_from_initializer() {
    let labels = inlay_hint_labels("fun inferSpec() { val valueSpec = 42 }\n");
    assert!(labels.iter().any(|label| label == ": Int"));
}
