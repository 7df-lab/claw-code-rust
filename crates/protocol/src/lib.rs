//! Shared wire and persistence types exchanged between Devo crates.
//!
//! The crate keeps provider requests, runtime events, sessions, permissions,
//! and tool payloads in one serialization boundary so other crates can depend
//! on stable protocol shapes instead of each other's internal models.

mod acp;
mod agent;
mod approval;
mod command_exec;
mod connection;
mod conversation;
mod event;
mod goal;
mod hosted_tools;
mod model;
pub mod native;
pub mod parse_command;
mod permissions;
pub mod protocol;
mod provider_vendor;
mod reasoning_effort;
mod reference_search;
mod request_normalize;
mod request_user_input;
mod response;
mod role;
mod session;
mod skill;
mod slash_command;
mod task;
mod truncation;
mod turn;
pub mod user_input;
mod workspace_changes;

pub use acp::ts as acp_ts;
pub use acp::*;
pub use agent::*;
pub use approval::*;
pub use command_exec::*;
pub use connection::*;
pub use conversation::*;
pub use event::*;
pub use goal::*;
pub use hosted_tools::*;
pub use model::*;
pub use permissions::*;
pub use protocol::*;
pub use provider_vendor::*;
pub use reasoning_effort::*;
pub use reference_search::*;
pub use request_normalize::*;
pub use request_user_input::*;
pub use response::*;
pub use role::*;
pub use session::*;
pub use skill::*;
pub use slash_command::*;
pub use task::*;
pub use truncation::*;
pub use turn::*;
pub use user_input::*;
pub use workspace_changes::*;
