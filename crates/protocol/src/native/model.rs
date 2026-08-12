//! Model binding: which provider/model a session or turn runs with.
//!
//! `Session.model` is the live current value (what the next turn will use);
//! `Turn.model` and `UsageRecord.model` are snapshots copied at event time.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::ReasoningEffort;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelBinding {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Permission presets selectable per session (`permission/profile/*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum PermissionProfile {
    Default,
    AutoReview,
    FullAccess,
}
