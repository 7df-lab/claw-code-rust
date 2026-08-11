//! Client-side transport API for talking to a Devo server.
//!
//! Protocol logic (JSON-RPC routing, pending response maps, ACP client handlers)
//! lives in `client_core`. [`stdio::StdioServerClient`] and
//! [`websocket::WebSocketServerClient`] are thin transport adapters.

mod acp_fs;
mod acp_permissions;
mod client_core;
mod events;
mod protocol_trace;
mod stdio;
mod websocket;

pub use client_core::ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD;
pub use client_core::ACP_PROMPT_STARTED_NOTIFICATION_METHOD;
pub use client_core::ServerNotificationMessage;
pub use events::{ClientEvent, client_event_from_notification};
pub use stdio::*;
pub use websocket::*;
