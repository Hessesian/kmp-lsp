//! [`State`] — lifecycle state of the workspace actor.
//!
//! Mirrors the `StoreState { Uninitialized | Ready<T> }` pattern from Moneta's
//! MVI layer.  Before the first [`super::Event::Initialize`] arrives
//! the actor is `Uninitialized`; after it, `Ready(ReadyState)`.
//!
//! All code that needs workspace state should call [`State::ready`]
//! and handle `None` (pre-init) explicitly — the named `Uninitialized` variant
//! makes the pre-init case visible at every call site rather than a generic
//! `Option<PathBuf>` whose meaning is unclear.

use std::path::PathBuf;

use super::Config;

// ─── ReadyState ────────────────────────────────────────────────────────────

/// Immutable snapshot of workspace state captured at initialisation time.
///
/// Produced from a [`Config`] the first time the actor processes a
/// [`super::Event::Initialize`] event, and updated on every
/// [`super::Event::ChangeRoot`].
///
/// Carries the resolved (not raw) source-path list so that no caller ever has
/// to re-run source discovery.
#[derive(Debug, Clone)]
pub(crate) struct ReadyState {
    /// Absolute path to the workspace root.
    pub root: PathBuf,

    /// Resolved, deduplicated list of source paths (output of
    /// [`Config::resolve_sources`]).
    pub source_paths: Vec<String>,
}

impl ReadyState {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            root: config.root.clone(),
            source_paths: config.resolve_sources(),
        }
    }
}

// ─── State ──────────────────────────────────────────────────────────

/// Lifecycle phase of the workspace actor.
///
/// ```text
/// ┌──────────────┐  Initialize  ┌────────────────────────┐
/// │ Uninitialized│─────────────▶│  Ready(ReadyState)  │
/// └──────────────┘              └────────────────────────┘
///                                         │  ChangeRoot
///                                         ▼
///                               ┌────────────────────────┐
///                               │  Ready(ReadyState') │
///                               └────────────────────────┘
/// ```
///
/// There is no transition back to `Uninitialized`.  A `ChangeRoot` replaces
/// `Ready` with a fresh `Ready` for the new root.
#[derive(Debug, Clone, Default)]
pub(crate) enum State {
    #[default]
    Uninitialized,
    Ready(ReadyState),
}

impl State {
    /// Return a reference to the inner [`ReadyState`], or `None` if the
    /// workspace has not been initialised yet.
    pub(crate) fn ready(&self) -> Option<&ReadyState> {
        match self {
            State::Ready(data) => Some(data),
            State::Uninitialized => None,
        }
    }

    /// Transition to `Ready` (or replace the current `Ready`).
    pub(crate) fn set_state(&mut self, data: ReadyState) {
        *self = State::Ready(data);
    }
}

#[cfg(test)]
#[path = "phase_tests.rs"]
mod tests;
