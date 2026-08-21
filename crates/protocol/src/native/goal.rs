//! Native Goal: the thin public surface (objective/status/budget/progress
//! summary). Plan/acceptance details are server-internal orchestration and do
//! not enter the API.
//!
//! Truth source: `devo-api-design/01-native-api.md` §4.5.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::GoalId;
use super::ids::SessionId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: GoalId,
    pub session_id: SessionId,
    pub objective: String,
    pub status: GoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    /// Human-readable progress summary; no percentage (a value that cannot be
    /// honestly computed does not enter the protocol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// State machine: `active ⇄ paused / blocked / usageLimited / budgetLimited`
/// (the latter two are NOT terminal — topping up quota or resuming returns to
/// `active`); terminal states are `completed / failed / canceled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Completed,
    Failed,
    Canceled,
}
