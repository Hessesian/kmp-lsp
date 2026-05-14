use super::{dot_receiver, split_prefix};

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
fn dot_receiver_none() {
    assert_eq!(dot_receiver("foo"), None);
}
