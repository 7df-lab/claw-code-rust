//! Process spawning and control primitives shared by tools and runtimes.
//!
//! This crate owns the transport-level distinction between pipe-backed commands,
//! PTY-backed interactive sessions, and externally driven process adapters so
//! higher-level crates can work with a single `ProcessHandle` abstraction.

mod pty;
pub mod sandbox;
#[cfg(windows)]
pub mod windows_job;

pub use pty::process_group;
pub use pty::*;
