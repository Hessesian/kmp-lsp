use super::{dot_receiver, param_names_from_sig, split_prefix};

#[test]
fn dot_receiver_nested_chains() {
    // Test that nested chain dot receiver is captured completely (e.g., MaterialTheme.colorScheme.)
    assert_eq!(
        dot_receiver("MaterialTheme.colorScheme."),
        Some("MaterialTheme.colorScheme".to_string()),
        "Failed to capture a standard nested dot receiver chain"
    );

    // Test capturing inside an assignment expression
    assert_eq!(
        dot_receiver("val x = MaterialTheme.colorScheme."),
        Some("MaterialTheme.colorScheme".to_string()),
        "Failed to capture a nested chain inside an assignment"
    );

    // Test that standard single receiver remains unaffected (e.g., myVar.)
    assert_eq!(
        dot_receiver("myVar."),
        Some("myVar".to_string()),
        "Failed to capture a simple single variable receiver"
    );

    // Test nested class paths starting with uppercase letters
    assert_eq!(
        dot_receiver("Outer.Inner."),
        Some("Outer.Inner".to_string()),
        "Failed to capture nested class dot receivers"
    );

    // Test that spaces bound the backward scan correctly
    assert_eq!(
        dot_receiver("val y = myVar."),
        Some("myVar".to_string()),
        "Backward scan did not correctly stop at spaces"
    );

    // Test that curly braces bound the backward scan correctly
    assert_eq!(
        dot_receiver("{ myVar."),
        Some("myVar".to_string()),
        "Backward scan did not correctly stop at curly braces"
    );

    // Test that parentheses bound the backward scan correctly
    assert_eq!(
        dot_receiver("(myVar."),
        Some("myVar".to_string()),
        "Backward scan did not correctly stop at parentheses"
    );

    // Test fallback behavior when there is no trailing dot
    assert_eq!(
        dot_receiver("no_dot_at_end"),
        None,
        "Expected None when there is no trailing dot"
    );

    // Test fallback behavior with duplicate trailing dots
    assert_eq!(
        dot_receiver("trailing.dot.."),
        None,
        "Expected None for duplicate trailing dots"
    );
}

#[test]
fn split_prefix_after_dot() {
    let (prefix, before_prefix) = split_prefix("foo.bar");
    assert_eq!(prefix, "bar");
    assert_eq!(before_prefix, "foo.");
}

#[test]
fn split_prefix_bare() {
    let (prefix, before_prefix) = split_prefix("someIdent");
    assert_eq!(prefix, "someIdent");
    assert_eq!(before_prefix, "");
}

#[test]
fn dot_receiver_simple() {
    assert_eq!(dot_receiver("foo."), Some("foo".to_string()));
}

#[test]
fn dot_receiver_qualified() {
    assert_eq!(
        dot_receiver("Outer.Inner."),
        Some("Outer.Inner".to_string())
    );
}

#[test]
fn dot_receiver_chained_lowercase() {
    // "foo.bar." → full chain "foo.bar" for cross-file resolution
    assert_eq!(
        dot_receiver("refreshDashboardInteractor.triggers."),
        Some("refreshDashboardInteractor.triggers".to_string())
    );
}

#[test]
fn dot_receiver_three_segment_chain() {
    assert_eq!(dot_receiver("a.b.c."), Some("a.b.c".to_string()));
}

#[test]
fn dot_receiver_call_expression_no_args() {
    // call args are stripped; resolver handles return-type lookup
    assert_eq!(
        dot_receiver("productFlow()."),
        Some("productFlow".to_string())
    );
}

#[test]
fn dot_receiver_call_expression_with_args() {
    assert_eq!(
        dot_receiver("getFlow(arg1, arg2)."),
        Some("getFlow".to_string())
    );
}

#[test]
fn dot_receiver_call_expression_nested_args() {
    assert_eq!(
        dot_receiver("productFlow(trigger.isRefresh())."),
        Some("productFlow".to_string())
    );
}

#[test]
fn dot_receiver_none() {
    assert_eq!(dot_receiver("foo"), None);
}

// ── param_names_from_sig ──────────────────────────────────────────────────────

#[test]
fn param_names_basic() {
    assert_eq!(
        param_names_from_sig("name: String, age: Int"),
        vec!["name", "age"]
    );
}

#[test]
fn param_names_with_defaults() {
    assert_eq!(
        param_names_from_sig(
            "text: String, modifier: Modifier = Modifier, color: Color = Color.Unspecified"
        ),
        vec!["text", "modifier", "color"]
    );
}

#[test]
fn param_names_with_annotation() {
    assert_eq!(
        param_names_from_sig("@Composable content: @Composable () -> Unit"),
        vec!["content"]
    );
}

#[test]
fn param_names_vararg() {
    assert_eq!(param_names_from_sig("vararg items: String"), vec!["items"]);
}

#[test]
fn param_names_skips_this() {
    // Extension receiver `this@Foo` should not produce a named arg
    assert_eq!(param_names_from_sig("this: Foo, value: Int"), vec!["value"]);
}

#[test]
fn param_names_empty() {
    let result = param_names_from_sig("");
    assert!(result.is_empty());
}
