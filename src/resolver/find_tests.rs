use super::*;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{Position, Range};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

const SIBLING_FIELDS_SRC: &str = "\
sealed interface Event {
    data class RegularInput(val event: RegularEvent) : Event
    data class OverdraftInput(val event: OverdraftEvent) : Event
}
sealed interface RegularEvent
sealed interface OverdraftEvent
";

#[test]
fn scoped_to_a_specific_variant_finds_that_variants_own_field_not_a_sibling() {
    let idx = Indexer::new();
    let file_uri = uri("/Reducer.kt");
    idx.index_content(&file_uri, SIBLING_FIELDS_SRC);

    let overdraft_input = find_name_in_uri(&idx, "OverdraftInput", file_uri.as_str())
        .into_iter()
        .next()
        .expect("OverdraftInput should be indexed");

    let found = find_name_scoped_to_container(&idx, "event", &overdraft_input)
        .expect("event field should be found within OverdraftInput");

    assert_eq!(
        found.range.start.line, 2,
        "must resolve to OverdraftInput's own `event` field, not RegularInput's: {:?}",
        found.range
    );
}

#[test]
fn scoped_to_the_other_variant_finds_its_own_field_instead() {
    let idx = Indexer::new();
    let file_uri = uri("/Reducer.kt");
    idx.index_content(&file_uri, SIBLING_FIELDS_SRC);

    let regular_input = find_name_in_uri(&idx, "RegularInput", file_uri.as_str())
        .into_iter()
        .next()
        .expect("RegularInput should be indexed");

    let found = find_name_scoped_to_container(&idx, "event", &regular_input)
        .expect("event field should be found within RegularInput");

    assert_eq!(
        found.range.start.line, 1,
        "must resolve to RegularInput's own `event` field, not OverdraftInput's: {:?}",
        found.range
    );
}

#[test]
fn falls_back_to_closest_after_line_when_container_symbol_is_not_found() {
    let idx = Indexer::new();
    let file_uri = uri("/Reducer.kt");
    idx.index_content(&file_uri, SIBLING_FIELDS_SRC);

    let fake_container = Location {
        uri: file_uri.clone(),
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
    };

    let found = find_name_scoped_to_container(&idx, "event", &fake_container);
    let fallback = find_name_in_uri_after_line(&idx, "event", file_uri.as_str(), 0)
        .into_iter()
        .next();
    assert_eq!(found, fallback);
}

/// Documents pick-first: scoping to the outer `Event` (which encloses both
/// variants) has two matches, and the first-declared one wins.
#[test]
fn scoping_to_an_outer_container_with_multiple_matches_picks_the_first() {
    let idx = Indexer::new();
    let file_uri = uri("/Reducer.kt");
    idx.index_content(&file_uri, SIBLING_FIELDS_SRC);

    let event_interface = find_name_in_uri(&idx, "Event", file_uri.as_str())
        .into_iter()
        .next()
        .expect("Event should be indexed");

    let found = find_name_scoped_to_container(&idx, "event", &event_interface)
        .expect("at least one event field is enclosed by Event");

    assert_eq!(found.range.start.line, 1);
}
