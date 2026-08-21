//! Native protocol types: the single domain definition shared by
//! persistence (rollout JSONL) and all four wire surfaces (Native, ACP,
//! External, A2A).
//!
//! These types are the schema truth source (`devo-api-design/README.md` §3):
//! wire JSON is camelCase, times are RFC 3339 UTC, IDs are opaque strings.
//! They are introduced alongside the legacy protocol types (05 P0/P1) and do
//! not replace them until the migration phases land.

pub mod error;
pub mod event;
pub mod goal;
pub mod ids;
pub mod item;
pub mod methods;
pub mod model;
pub mod page;
pub mod patch;
pub mod queue;
pub mod rpc_admin;
pub mod rpc_search;
pub mod rpc_session;
pub mod rpc_turn;
pub mod rpc_workspace;
pub mod session;
pub mod turn;
pub mod usage;
pub mod wire_projector;
