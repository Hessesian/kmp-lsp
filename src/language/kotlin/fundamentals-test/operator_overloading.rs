use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

#[test]
fn ks_operators_0008_operator_functions_support_regular_calls_and_operator_conventions() {
    assert_source_parses(
        "class NumberSpec(val valueSpec: Int) {\n    operator fun plus(otherSpec: NumberSpec): NumberSpec = NumberSpec(valueSpec + otherSpec.valueSpec)\n}\nfun addSpec(firstSpec: NumberSpec, secondSpec: NumberSpec) {\n    val regularSpec = firstSpec.plus(secondSpec)\n    val operatorSpec = firstSpec + secondSpec\n}\n",
    );
}

#[test]
fn ks_operators_0009_operator_functions_may_be_members_extensions_or_suspending() {
    assert_source_parses(
        "class NumberSpec {\n    operator fun unaryPlus(): NumberSpec = this\n    suspend operator fun unaryMinus(): NumberSpec = this\n}\noperator fun NumberSpec.plus(otherSpec: NumberSpec): NumberSpec = this\nsuspend operator fun NumberSpec.minus(otherSpec: NumberSpec): NumberSpec = this\n",
    );
}

#[test]
#[ignore = "KS-OPERATORS-0007: kmp-lsp does not require operator modifiers for conventions"]
fn ks_operators_0007_operator_convention_requires_the_operator_modifier() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun plus(otherSpec: ValidSpec): ValidSpec = this\n}\nfun validSpec() = ValidSpec() + ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun plus(otherSpec: InvalidSpec): InvalidSpec = this\n}\nfun invalidSpec() = InvalidSpec() + InvalidSpec()\n",
    );
}

#[test]
fn ks_operators_0019_destructuring_convention_applies_to_locals_lambdas_and_for_loops() {
    assert_source_parses(
        "data class PairSpec(val numberSpec: Int, val textSpec: String)\nfun destructureSpec(valuesSpec: List<PairSpec>) {\n    val (numberSpec, textSpec) = PairSpec(1, \"one\")\n    valuesSpec.forEach { (lambdaNumberSpec, lambdaTextSpec) -> println(\"$lambdaNumberSpec$lambdaTextSpec\") }\n    for ((loopNumberSpec, loopTextSpec) in valuesSpec) println(\"$loopNumberSpec$loopTextSpec\")\n}\n",
    );
}

#[test]
fn ks_operators_0020_destructuring_introduces_one_or_more_properties() {
    let source = "class SingleSpec {\n    operator fun component1(): Int = 1\n}\ndata class TripleSpec(val firstSpec: Int, val secondSpec: Int, val thirdSpec: Int)\nfun destructureSpec() {\n    val (onlySpec) = SingleSpec()\n    val (firstSpec, secondSpec, thirdSpec) = TripleSpec(1, 2, 3)\n    println(onlySpec + firstSpec + secondSpec + thirdSpec)\n}\n";
    assert_source_parses(source);

    let specification_uri = Url::parse("file:///kotlin-spec/OperatorDestructuring.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for property_name in ["onlySpec", "firstSpec", "secondSpec", "thirdSpec"] {
        assert!(
            symbols.iter().any(|symbol| symbol.name == property_name),
            "every destructured property must be indexed"
        );
    }
}

#[test]
#[ignore = "KS-OPERATORS-0022: tree-sitter-kotlin accepts underscore as a standalone property identifier"]
fn ks_operators_0022_standalone_underscore_is_not_an_identifier() {
    let source = "fun destructureSpec() { val (firstSpec, _, thirdSpec) = Triple(1, 2, 3); println(firstSpec + thirdSpec) }\n";
    assert_source_parses(source);
    assert_source_has_syntax_error("fun invalidSpec() { val _ = 1 }\n");
}

#[test]
#[ignore = "KS-OPERATORS-0022: kmp-lsp indexes a destructuring ignore marker as a property"]
fn ks_operators_0022_ignore_marker_introduces_no_property() {
    let source = "fun destructureSpec() { val (firstSpec, _, thirdSpec) = Triple(1, 2, 3); println(firstSpec + thirdSpec) }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/OperatorIgnoreMarker.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    assert!(symbols.iter().any(|symbol| symbol.name == "firstSpec"));
    assert!(symbols.iter().any(|symbol| symbol.name == "thirdSpec"));
    assert!(symbols.iter().all(|symbol| symbol.name != "_"));
}

#[test]
#[ignore = "KS-OPERATORS-0023: kmp-lsp does not require operator component functions"]
fn ks_operators_0023_destructuring_requires_operator_component_functions() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun component1(): Int = 1\n}\nfun validSpec() { val (valueSpec) = ValidSpec() }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun component1(): Int = 1\n}\nfun invalidSpec() { val (valueSpec) = InvalidSpec() }\n",
    );
}

#[test]
fn ks_operators_0026_destructuring_placeholders_accept_optional_types() {
    assert_source_parses(
        "data class TripleSpec(val firstSpec: Int, val secondSpec: String, val thirdSpec: Long)\nfun destructureSpec() { val (firstSpec: Int, _: String, thirdSpec: Long) = TripleSpec(1, \"ignored\", 3L) }\n",
    );
}
