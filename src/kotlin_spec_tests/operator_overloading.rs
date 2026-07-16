use super::{assert_source_has_syntax_error, assert_source_parses};

#[test]
fn ks_9_001_operator_functions_support_regular_calls_and_operator_conventions() {
    assert_source_parses(
        "class NumberSpec(val valueSpec: Int) {\n    operator fun plus(otherSpec: NumberSpec): NumberSpec = NumberSpec(valueSpec + otherSpec.valueSpec)\n}\nfun addSpec(firstSpec: NumberSpec, secondSpec: NumberSpec) {\n    val regularSpec = firstSpec.plus(secondSpec)\n    val operatorSpec = firstSpec + secondSpec\n}\n",
    );
}

#[test]
fn ks_9_002_operator_functions_may_be_members_extensions_or_suspending() {
    assert_source_parses(
        "class NumberSpec {\n    operator fun unaryPlus(): NumberSpec = this\n    suspend operator fun unaryMinus(): NumberSpec = this\n}\noperator fun NumberSpec.plus(otherSpec: NumberSpec): NumberSpec = this\nsuspend operator fun NumberSpec.minus(otherSpec: NumberSpec): NumberSpec = this\n",
    );
}

#[test]
#[ignore = "KS-9-003: kmp-lsp does not require operator modifiers for conventions"]
fn ks_9_003_operator_convention_requires_the_operator_modifier() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun plus(otherSpec: ValidSpec): ValidSpec = this\n}\nfun validSpec() = ValidSpec() + ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun plus(otherSpec: InvalidSpec): InvalidSpec = this\n}\nfun invalidSpec() = InvalidSpec() + InvalidSpec()\n",
    );
}

#[test]
fn ks_9_1_001_destructuring_convention_applies_to_locals_lambdas_and_for_loops() {
    assert_source_parses(
        "data class PairSpec(val numberSpec: Int, val textSpec: String)\nfun destructureSpec(valuesSpec: List<PairSpec>) {\n    val (numberSpec, textSpec) = PairSpec(1, \"one\")\n    valuesSpec.forEach { (lambdaNumberSpec, lambdaTextSpec) -> println(\"$lambdaNumberSpec$lambdaTextSpec\") }\n    for ((loopNumberSpec, loopTextSpec) in valuesSpec) println(\"$loopNumberSpec$loopTextSpec\")\n}\n",
    );
}

#[test]
fn ks_9_1_002_destructuring_placeholders_accept_types_and_ignore_markers() {
    assert_source_parses(
        "data class TripleSpec(val firstSpec: Int, val secondSpec: String, val thirdSpec: Long)\nfun destructureSpec() { val (firstSpec: Int, _, thirdSpec: Long) = TripleSpec(1, \"ignored\", 3L) }\n",
    );
}

#[test]
#[ignore = "KS-9.1-003: kmp-lsp does not require operator component functions"]
fn ks_9_1_003_destructuring_requires_operator_component_functions() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun component1(): Int = 1\n}\nfun validSpec() { val (valueSpec) = ValidSpec() }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun component1(): Int = 1\n}\nfun invalidSpec() { val (valueSpec) = InvalidSpec() }\n",
    );
}
