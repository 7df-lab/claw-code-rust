//! Client-side transport API for talking to a Devo server.
//!
//! Protocol logic (JSON-RPC routing, pending response maps, and Native reverse
//! requests) lives in `client_core`. [`stdio::StdioServerClient`] and
//! [`websocket::WebSocketServerClient`] are thin transport adapters.

mod client_core;
mod native_approval;
mod protocol_trace;
mod stdio;
mod websocket;

pub use client_core::GoalLifecycleTransition;
pub use client_core::ServerNotificationMessage;
pub use client_core::native_turn_start_input;
pub use stdio::*;
pub use websocket::*;
