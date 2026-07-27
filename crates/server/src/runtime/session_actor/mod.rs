// Per-session actor: single-writer for durable session state.
//
// Long-running turns still execute inside or beside this actor today. While a
// turn is in flight, transient execution state lives in ActiveTurnRegistry and
// merges back through actor commands when the turn completes.

pub(crate) mod approval_scope;
mod commands;
mod handle;
mod loop_;
pub(crate) mod registry;
pub(crate) mod snapshots;
pub(crate) mod state;
mod turn;
mod turn_inline;

pub(crate) use handle::SessionHandle;
pub(crate) use state::SessionActorState;
