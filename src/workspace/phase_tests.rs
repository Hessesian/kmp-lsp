//! Tests for [`State`] lifecycle transitions.

use std::path::PathBuf;

use super::{ReadyState, State};

fn stub_data(root: &str) -> ReadyState {
    ReadyState {
        root: PathBuf::from(root),
        source_paths: vec![format!("{root}/src")],
    }
}

#[test]
fn default_phase_is_uninitialized() {
    let phase = State::default();
    assert!(phase.ready().is_none());
}

#[test]
fn set_state_transitions_to_ready() {
    let mut phase = State::default();
    phase.set_state(stub_data("/workspace"));
    assert!(phase.ready().is_some());
    assert_eq!(phase.ready().unwrap().root, PathBuf::from("/workspace"));
}

#[test]
fn set_state_replaces_existing_ready() {
    let mut phase = State::default();
    phase.set_state(stub_data("/old-root"));
    phase.set_state(stub_data("/new-root"));
    assert_eq!(phase.ready().unwrap().root, PathBuf::from("/new-root"));
}

#[test]
fn source_paths_preserved_in_ready_data() {
    let mut phase = State::default();
    let mut data = stub_data("/ws");
    data.source_paths.push("/ws/lib".to_owned());
    phase.set_state(data);
    assert_eq!(
        phase.ready().unwrap().source_paths,
        vec!["/ws/src".to_owned(), "/ws/lib".to_owned()]
    );
}
