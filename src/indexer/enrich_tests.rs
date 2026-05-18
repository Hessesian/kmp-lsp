use super::*;

#[test]
fn noop_handle_does_not_panic() {
    let handle = EnrichmentHandle::noop();
    handle.submit("SomeClass", 1);
    handle.submit("SomeClass", 1); // duplicate — should be no-op
    handle.clear();
}

#[test]
fn dedup_prevents_double_submit() {
    let handle = EnrichmentHandle::noop();
    handle.seen.insert("Foo".to_owned(), None);
    // Should not insert a second time.
    handle.submit("Foo", 1);
    assert_eq!(handle.seen.len(), 1);
}

#[test]
fn miss_cooldown_prevents_retry() {
    let handle = EnrichmentHandle::noop();
    // Simulate a recent miss.
    handle.seen.insert("Bar".to_owned(), Some(Instant::now()));
    handle.submit("Bar", 1);
    // Should still be 1 entry (not re-enqueued).
    assert_eq!(handle.seen.len(), 1);
}

#[test]
fn clear_resets_seen() {
    let handle = EnrichmentHandle::noop();
    handle.seen.insert("Baz".to_owned(), None);
    handle.clear();
    assert!(handle.seen.is_empty());
}
