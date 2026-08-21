//! Client-side transport API for talking to a Devo server.
//!
//! Protocol logic (JSON-RPC routing, pending response maps, ACP client handlers)
//! lives in `client_core`. [`stdio::StdioServerClient`] and
//! [`websocket::WebSocketServerClient`] are thin transport adapters.

mod client_core;
mod events;
mod native_approval;
mod protocol_trace;
mod stdio;
mod websocket;

pub use client_core::GoalLifecycleTransition;
pub use client_core::ServerNotificationMessage;
pub use client_core::native_turn_start_input;
pub use events::{ClientEvent, client_event_from_notification};
pub use stdio::*;
pub use websocket::*;
