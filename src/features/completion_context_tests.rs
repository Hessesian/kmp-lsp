use super::{LambdaScope, ScopeContext};

#[test]
fn scope_resolve_it_returns_innermost_it_type() {
    let scope = ScopeContext {
        enclosing_class: None,
        lambda_scopes: vec![
            LambdaScope {
                it_type: Some("OuterType".into()),
                named_params: vec![],
                label: Some("map".into()),
            },
            LambdaScope {
                it_type: Some("InnerType".into()),
                named_params: vec![],
                label: Some("forEach".into()),
            },
        ],
        bare_this_type: None,
    };

    assert_eq!(scope.resolve_receiver("it"), Some("InnerType"));
}

#[test]
fn scope_resolve_this_at_label() {
    let scope = ScopeContext {
        enclosing_class: Some("MyClass".into()),
        lambda_scopes: vec![LambdaScope {
            it_type: Some("Element".into()),
            named_params: vec![],
            label: Some("forEach".into()),
        }],
        bare_this_type: Some("MyClass".into()),
    };

    assert_eq!(scope.resolve_receiver("this@forEach"), Some("Element"));
    assert_eq!(scope.resolve_receiver("this@MyClass"), Some("MyClass"));
}

#[test]
fn scope_is_scope_receiver() {
    let scope = ScopeContext {
        enclosing_class: None,
        lambda_scopes: vec![],
        bare_this_type: None,
    };

    assert!(scope.is_scope_receiver("it"));
    assert!(scope.is_scope_receiver("this"));
    assert!(scope.is_scope_receiver("this@Foo"));
    assert!(!scope.is_scope_receiver("someVar"));
    assert!(!scope.is_scope_receiver("Companion"));
}
