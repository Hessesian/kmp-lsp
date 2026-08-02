//! Workspace feature contract — the complete public surface of the workspace module.
//!
//! Mirrors the `*Contract` interface pattern from Moneta's MVI layer: one file
//! that a contributor can read to understand every input, state, and output of
//! the workspace subsystem.
//!
//! # Layout
//!
//! | Type | Role |
//! |---|---|
//! | [`Event`] | Inputs — every write to workspace state |
//! | [`State`] | State — `Uninitialized` or `Ready(ReadyState)` |
//!
//! # Compiler enforcement
//!
//! * Adding a [`Event`] variant → compile error in `Actor::run`
//!   until the handler is implemented.
//! * Accessing [`State::Ready`] data requires an explicit `match` or
//!   a call to [`State::ready`] — there is no way to get a
//!   `&ReadyState` without acknowledging the `Uninitialized` case.

// Items re-exported here are the single source of truth for the workspace
// public surface.  Individual types are unused at the re-export site until
// Wave 2/3 wires backend and CLI through this contract.
#[allow(unused_imports)]
pub(crate) use super::event::Event;
#[allow(unused_imports)]
pub(crate) use super::phase::{ReadyState, State};
