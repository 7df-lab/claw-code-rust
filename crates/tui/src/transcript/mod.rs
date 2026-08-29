//! Transcript projection: one model for live and restored sessions (L2-DES-TUI-007).

pub(crate) mod file_change;
pub(crate) mod lifecycle;
pub(crate) mod model;
pub(crate) mod presentation;
pub(crate) mod projector;
pub(crate) mod render;
pub(crate) mod restore;
pub(crate) mod restore_session;
pub(crate) mod stream_text;
pub(crate) mod tool_state;

pub(crate) use projector::TranscriptProjector;
