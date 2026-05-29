use super::{param_names_from_sig, split_prefix};
use crate::resolver::complete::ReceiverExpr;

fn recv(chain: &str, is_call: bool) -> Option<ReceiverExpr> {
    Some(ReceiverExpr {
        chain: chain.to_string(),
        is_call,
    })
}

#[test]
fn dot_receiver_nested_chains() {
    assert_eq!(
        ReceiverExpr::parse("MaterialTheme.colorScheme."),
        recv("MaterialTheme.colorScheme", false),
        "Failed to capture a standard nested dot receiver chain"
    );
    assert_eq!(
        ReceiverExpr::parse("val x = MaterialTheme.colorScheme."),
        recv("MaterialTheme.colorScheme", false),
        "Failed to capture a nested chain inside an assignment"
    );
    assert_eq!(
        ReceiverExpr::parse("myVar."),
        recv("myVar", false),
        "Failed to capture a simple single variable receiver"
    );
    assert_eq!(
        ReceiverExpr::parse("Outer.Inner."),
        recv("Outer.Inner", false),
        "Failed to capture nested class dot receivers"
    );
    assert_eq!(
        ReceiverExpr::parse("val y = myVar."),
        recv("myVar", false),
        "Backward scan did not correctly stop at spaces"
    );
    assert_eq!(
        ReceiverExpr::parse("{ myVar."),
        recv("myVar", false),
        "Backward scan did not correctly stop at curly braces"
    );
    assert_eq!(
        ReceiverExpr::parse("(myVar."),
        recv("myVar", false),
        "Backward scan did not correctly stop at parentheses"
    );
    assert_eq!(
        ReceiverExpr::parse("no_dot_at_end"),
        None,
        "Expected None when there is no trailing dot"
    );
    assert_eq!(
        ReceiverExpr::parse("trailing.dot.."),
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
    assert_eq!(ReceiverExpr::parse("foo."), recv("foo", false));
}

#[test]
fn dot_receiver_qualified() {
    assert_eq!(
        ReceiverExpr::parse("Outer.Inner."),
        recv("Outer.Inner", false)
    );
}

#[test]
fn dot_receiver_chained_lowercase() {
    assert_eq!(
        ReceiverExpr::parse("refreshDashboardInteractor.triggers."),
        recv("refreshDashboardInteractor.triggers", false)
    );
}

#[test]
fn dot_receiver_three_segment_chain() {
    assert_eq!(ReceiverExpr::parse("a.b.c."), recv("a.b.c", false));
}

#[test]
fn dot_receiver_call_expression_no_args() {
    assert_eq!(
        ReceiverExpr::parse("productFlow()."),
        recv("productFlow", true)
    );
}

#[test]
fn dot_receiver_call_expression_with_args() {
    assert_eq!(
        ReceiverExpr::parse("getFlow(arg1, arg2)."),
        recv("getFlow", true)
    );
}

#[test]
fn dot_receiver_call_expression_nested_args() {
    assert_eq!(
        ReceiverExpr::parse("productFlow(trigger.isRefresh())."),
        recv("productFlow", true)
    );
}

#[test]
fn dot_receiver_none() {
    assert_eq!(ReceiverExpr::parse("foo"), None);
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
