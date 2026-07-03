use super::*;

#[test]
fn lambda_type_nth_input_test() {
    assert_eq!(
        lambda_type_nth_input("(String, Boolean) -> Unit", 0),
        Some("String".into())
    );
    assert_eq!(
        lambda_type_nth_input("(String, Boolean) -> Unit", 1),
        Some("Boolean".into())
    );
    assert_eq!(lambda_type_nth_input("() -> Unit", 0), None);
    assert_eq!(
        lambda_type_nth_input("(SaveInfo) -> Unit", 0),
        Some("SaveInfo".into())
    );
    // suspend function type as whole outer type:
    assert_eq!(
        lambda_type_nth_input("suspend (T) -> Unit", 0),
        Some("T".into())
    );
    assert_eq!(
        lambda_type_nth_input("suspend (LoanDetail) -> Unit", 0),
        Some("LoanDetail".into())
    );
    assert_eq!(lambda_type_nth_input("suspend () -> Unit", 0), None);
}

#[test]
fn lambda_type_input_preserves_qualified_type_names() {
    assert_eq!(
        lambda_type_nth_input("(Contract.Effect) -> Unit", 0),
        Some("Contract.Effect".into())
    );
    assert_eq!(
        lambda_type_nth_input("(State, Contract.Effect) -> Unit", 1),
        Some("Contract.Effect".into())
    );
}

#[test]
fn lambda_type_receiver_strips_suspend_before_receiver_type() {
    // `suspend` modifier directly before the receiver type (coroutine builders like
    // `callbackFlow { … }` whose block is `suspend ProducerScope<E>.() -> Unit`).
    assert_eq!(
        lambda_type_receiver("suspend ProducerScope<E>.() -> Unit").as_deref(),
        Some("ProducerScope")
    );
    // Plain receiver lambda still resolves.
    assert_eq!(
        lambda_type_receiver("LazyListScope.() -> Unit").as_deref(),
        Some("LazyListScope")
    );
    // `suspend` as a prefix of a longer identifier must NOT be stripped.
    assert_eq!(
        lambda_type_receiver("suspendableScope.() -> Unit").as_deref(),
        Some("suspendableScope")
    );
}

#[test]
fn lambda_type_receiver_strips_nullable_wrapper() {
    // Nullable receiver lambda `(Receiver.() -> R)?` (Compose slot params like
    // `content: (LazyListScope.() -> Unit)? = null`).
    assert_eq!(
        lambda_type_receiver("(LazyListScope.() -> Unit)?").as_deref(),
        Some("LazyListScope")
    );
    // Non-nullable receiver lambda still resolves.
    assert_eq!(
        lambda_type_receiver("LazyListScope.() -> Unit").as_deref(),
        Some("LazyListScope")
    );
    // A regular (non-receiver) lambda has no `this` receiver.
    assert_eq!(lambda_type_receiver("(String) -> Unit"), None);
}
