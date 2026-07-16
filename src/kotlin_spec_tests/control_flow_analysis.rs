use super::{assert_source_has_syntax_error, assert_source_parses};

#[test]
#[ignore = "KS-12.2.3-001: kmp-lsp does not diagnose path-sensitive uninitialized reads"]
fn ks_12_2_3_001_property_must_be_assigned_on_every_reaching_path() {
    assert_source_parses(
        "fun validSpec(conditionSpec: Boolean): Int {\n    val valueSpec: Int\n    if (conditionSpec) { valueSpec = 1 } else { valueSpec = 2 }\n    return valueSpec\n}\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(conditionSpec: Boolean): Int {\n    val valueSpec: Int\n    if (conditionSpec) { valueSpec = 1 }\n    return valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-12.2.5-001: kmp-lsp does not apply calls-in-place contracts to definite assignment"]
fn ks_12_2_5_001_run_exactly_once_contract_propagates_assignment() {
    assert_source_parses(
        "fun validSpec(): Int {\n    val valueSpec: Int\n    run { valueSpec = 1 }\n    return valueSpec\n}\n",
    );
    assert_source_has_syntax_error(
        "fun invokeSpec(blockSpec: () -> Unit) { blockSpec() }\nfun invalidSpec(): Int {\n    val valueSpec: Int\n    invokeSpec { valueSpec = 1 }\n    return valueSpec\n}\n",
    );
}
