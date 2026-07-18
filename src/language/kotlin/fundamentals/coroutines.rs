use super::{assert_source_has_syntax_error, assert_source_parses};

#[test]
#[ignore = "KS-COROUTINES-0005: kmp-lsp does not diagnose suspend calls from non-suspending contexts"]
fn ks_coroutines_0005_only_suspending_context_may_call_suspending_function() {
    assert_source_parses(
        "suspend fun loadSpec(): String = \"value\"\nsuspend fun validSpec(): String = loadSpec()\n",
    );
    assert_source_has_syntax_error(
        "suspend fun loadSpec(): String = \"value\"\nfun invalidSpec(): String = loadSpec()\n",
    );
}
