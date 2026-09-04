//! Native Session structure. Truth source: `devo-api-design/07-session-turn.md`.
//!
//! State is split into three orthogonal dimensions — `status` (lifecycle,
//! derived from the active turn) × `flags` (stackable blocking reasons) ×
//! `archived` (user intent) — replacing the old single mixed enum.

use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::SessionId;
use super::ids::TurnId;
use super::model::ModelBinding;
use super::model::PermissionProfile;
use super::usage::SessionUsage;
use crate::SessionTitleState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: SessionId,
    /// Optimistic concurrency token for `expectedVersion` writes.
    pub version: u64,

    // ── Identity (immutable after creation; `cwd` moves only via an explicit
    // `session/cwd/change`) ──
    /// Execution scope of the session: normalized absolute path. It is part
    /// of the session's identity, not the client's location; resuming from a
    /// different cwd does not change it.
    pub cwd: PathBuf,
    /// Additional absolute workspace roots associated with the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    /// Sub-agent parent only. User forks use `fork_from_id` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionParent>,
    /// Source session when this session was created by user `session/fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_from_id: Option<SessionId>,
    /// Cut turn for a user fork (`through` that turn); absent for tip forks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn_id: Option<TurnId>,
    pub ephemeral: bool,
    pub created_at: DateTime<Utc>,

    // ── Three orthogonal state dimensions ──
    pub status: SessionStatus,
    /// Stackable blocking reasons; deduplicated and serialized in enum order
    /// so equivalent snapshots produce stable JSON.
    pub flags: Vec<SessionFlag>,
    pub archived: bool,

    // ── Runtime pointers ──
    /// Invariant: `status == Active` iff `active_turn_id.is_some()`; updated
    /// atomically by the single session actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<TurnId>,
    /// The queue is session state, not turn state.
    pub queued_count: u32,

    // ── Mutable configuration (current values) ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub title_state: SessionTitleState,
    /// Current binding: what the next turn will use.
    pub model: ModelBinding,
    pub settings: SessionSettings,

    // ── Snapshot: an observation that may be stale ──
    /// Computed at creation and recomputed on `session/cwd/change`; clients
    /// treat it as potentially stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_info: Option<GitInfo>,

    // ── Derived caches (server-maintained, client read-only) ──
    pub preview: String,
    pub last_activity_at: DateTime<Utc>,
    /// Current on-disk size of the durable JSONL transcript. This is a
    /// list-view cache: servers may omit it from non-list responses or when
    /// the session has no readable rollout file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_size_bytes: Option<u64>,
    /// Redundant aggregate of turn usages for list views; the ledger wins on
    /// any disagreement.
    pub usage: SessionUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Idle,
    Active,
}

/// Blocking reasons, stackable on top of `status`. "Waiting" is a flag, not a
/// status: clients can tell "working" apart from "blocked on you".
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "camelCase")]
pub enum SessionFlag {
    /// A pending approval request.
    WaitingApproval,
    /// An unanswered structured question.
    WaitingUserInput,
    /// Compaction in progress.
    Compacting,
    /// Goal-internal orchestration in progress.
    UpdatingGoal,
}

/// Sub-agent parentage only. User forks use `Session.fork_from_id` /
/// `Session.at_turn_id` so list/delete/resume treat them as independent
/// parallel history rather than child executors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionParent {
    Agent {
        #[schemars(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettings {
    pub permission_profile: PermissionProfile,
    /// User's reasoning-effort selection as persisted, including the toggle
    /// keywords `on`/`off` used by toggle/variant-style models —
    /// the typed `ReasoningEffort` enum cannot express those, so the snapshot
    /// carries the raw selection string (same contract as
    /// `SessionSettingsPatch.reasoning_effort`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// ACP-style session mode id, if one is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Active sandbox profile description; needed to restore the sandbox when
    /// resuming the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile: Option<String>,
    /// Session override for the absolute effective context window (tokens).
    /// When set, takes precedence over the model-derived effective window and
    /// is clamped to `model.context_window` at resolve time. Also drives the
    /// automatic-compaction boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window: Option<u64>,
}

/// Snapshot semantics: the current value is *copied* into the record at
/// observation time; later changes to the source do not affect the copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    /// `None` = unknown (e.g. converted from legacy data that never recorded
    /// the dirty flag); fresh snapshots always compute it.
    pub dirty: Option<bool>,
    pub observed_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings snapshot carries the raw selection literal — including
    /// the toggle keywords the `ReasoningEffort` enum cannot express — so a
    /// "wide write" (patch) always reads back equal from the "snapshot" side.
    #[test]
    fn session_settings_reasoning_effort_round_trips_raw_selection() {
        for literal in ["on", "off", "enabled", "disabled", "high", "xhigh"] {
            let settings = SessionSettings {
                permission_profile: PermissionProfile::Default,
                reasoning_effort: Some(literal.to_string()),
                mode: None,
                sandbox_profile: None,
                effective_context_window: None,
            };
            let json = serde_json::to_value(&settings).expect("serialize settings");
            assert_eq!(
                json["reasoningEffort"].as_str(),
                Some(literal),
                "wire name is reasoningEffort and the literal is preserved"
            );
            let back: SessionSettings = serde_json::from_value(json).expect("deserialize settings");
            assert_eq!(back.reasoning_effort.as_deref(), Some(literal));
        }
    }
}
