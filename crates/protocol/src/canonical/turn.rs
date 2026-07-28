//! Canonical Turn structure. Truth source: `devo-api-design/07-session-turn.md`.
//!
//! A turn is the lifecycle of one user intent. Items are not embedded; they
//! are the append-only item sequence (see the item module) read through the
//! paged history APIs.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::error::AgentError;
use super::ids::SessionId;
use super::ids::TurnId;
use super::model::ModelBinding;
use super::usage::TurnUsage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: TurnId,
    pub session_id: SessionId,
    /// Nth turn within the session.
    pub sequence: u32,
    pub kind: TurnKind,
    pub status: TurnStatus,
    /// Snapshot: the binding this turn actually ran with. `session.model` may
    /// change afterwards; history must faithfully answer "what did this turn
    /// run with".
    pub model: ModelBinding,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
}

/// Completion invariants are defined per kind (07 §4.3): all kinds require
/// foreground items terminal and no pending approval/question; `Regular` and
/// `GoalContinuation` successes additionally require a final assistant
/// message; `Compaction` requires a terminal `ContextCompaction` instead.
///
/// v1 keeps exactly three variants: the legacy `Review` kind was dead code
/// and an open `Other(String)` variant breaks exhaustive client matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum TurnKind {
    Regular,
    Compaction,
    /// Goal-driven autonomous turn. Has no user message by design and is not
    /// steerable; only `Regular + InProgress` turns may be steered.
    GoalContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Interrupted,
    Failed,
}
