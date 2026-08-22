//! Event envelope, streams, subscriptions, and typed server notifications.
//!
//! Truth source: `devo-api-design/08-events-subscription.md`.
//! Core proposition: an event first becomes a replayable fact, then gets
//! delivered — delivery is consumption, not production.

use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::error::AgentError;
use super::goal::Goal;
use super::goal::GoalStatus;
use super::ids::EventId;
use super::ids::ItemId;
use super::ids::QueueItemId;
use super::ids::RestorePlanId;
use super::ids::SessionId;
use super::ids::SubscriptionId;
use super::ids::TurnId;
use super::item::ApprovalDecision;
use super::item::CompactionTrigger;
use super::item::ContextOccupancy;
use super::item::ItemEnvelope;
use super::item::SpawnedWorkState;
use super::queue::QueueChange;
use super::queue::QueueEntry;
use super::session::Session;
use super::session::SessionFlag;
use super::session::SessionStatus;
use super::turn::Turn;
use super::turn::TurnStatus;
use super::usage::SessionUsage;
use crate::workspace_changes::WorkspaceChangeCoverage;
use crate::workspace_changes::WorkspaceChangeScope;
use crate::workspace_changes::WorkspaceChangeSetStatus;
use crate::workspace_changes::WorkspaceChangeViewStatus;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct EventMeta {
    pub event_id: EventId,
    /// Whitelisted stream, e.g. `runtime:<instance-id>` /
    /// `sessions:<cwd-hash>` / `session:<session-id>` / `task:<item-id>`.
    pub stream_id: String,
    /// Present only when `persisted = true`; strictly increasing within the
    /// stream and usable as a cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub emitted_at: DateTime<Utc>,
    /// `false` = purely transient (e.g. high-frequency token deltas): no
    /// stream seq, cannot be acked, excluded from replay.
    pub persisted: bool,
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_client_id: Option<String>,
}

/// One event with its metadata; used both for live delivery and for replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    #[serde(rename = "event")]
    pub meta: EventMeta,
    pub notification: ServerNotification,
}

// ---------------------------------------------------------------------------
// Notifications (Server -> Client)
// ---------------------------------------------------------------------------

/// Delta channels for living items. Transient deltas carry
/// `itemId + baseRevision + chunkIndex + delta`, are ordered per
/// item/channel, may be coalesced, and never enter the cursor log.
/// `item/completed` is the delta barrier for an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum DeltaChannel {
    AssistantMessage,
    Reasoning,
    CommandExecutionOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ItemDelta {
    pub item_id: ItemId,
    /// Owning session: lets subscribers route child-session deltas (subagent
    /// monitor) instead of folding them into the main transcript.
    pub session_id: SessionId,
    pub base_revision: u32,
    pub chunk_index: u64,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeStatsNotification {
    pub files_changed: u64,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesUpdatedNotification {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub scope: WorkspaceChangeScope,
    pub status: WorkspaceChangeViewStatus,
    pub coverage: WorkspaceChangeCoverage,
    pub change_set_status: WorkspaceChangeSetStatus,
    pub stats: WorkspaceChangeStatsNotification,
    pub version: u64,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "method", content = "params", rename_all_fields = "camelCase")]
pub enum ServerNotification {
    // ── Connection ──
    #[serde(rename = "initialized")]
    Initialized {
        connection_id: String,
        protocol_version: String,
        server_instance_id: String,
    },
    #[serde(rename = "runtime/warning")]
    RuntimeWarning {
        code: String,
        message: String,
        retryable: bool,
    },
    #[serde(rename = "runtime/shutdown")]
    RuntimeShutdown { reason: Option<String> },

    // ── Session ──
    #[serde(rename = "session/created")]
    SessionCreated { session: Box<Session> },
    #[serde(rename = "session/metadataUpdated")]
    SessionMetadataUpdated { session: Box<Session> },
    #[serde(rename = "session/cwdChanged")]
    SessionCwdChanged { session_id: SessionId, cwd: PathBuf },
    #[serde(rename = "session/statusChanged")]
    SessionStatusChanged {
        session_id: SessionId,
        status: SessionStatus,
        flags: Vec<SessionFlag>,
        active_turn_id: Option<TurnId>,
    },
    #[serde(rename = "session/archived")]
    SessionArchived {
        session_id: SessionId,
        archived: bool,
    },
    #[serde(rename = "session/closed")]
    SessionClosed { session_id: SessionId },
    #[serde(rename = "session/deleted")]
    SessionDeleted {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        deleted_session_ids: Vec<SessionId>,
    },
    #[serde(rename = "workspace/restoreStarted")]
    WorkspaceRestoreStarted {
        session_id: SessionId,
        restore_plan_id: RestorePlanId,
    },
    #[serde(rename = "workspace/restoreCompleted")]
    WorkspaceRestoreCompleted {
        session_id: SessionId,
        restore_plan_id: RestorePlanId,
        succeeded: bool,
        error: Option<AgentError>,
    },
    #[serde(rename = "workspace/changes/updated")]
    WorkspaceChangesUpdated(WorkspaceChangesUpdatedNotification),

    // ── Turn / Item ──
    #[serde(rename = "turn/started")]
    TurnStarted { turn: Box<Turn> },
    #[serde(rename = "turn/statusChanged")]
    TurnStatusChanged { turn_id: TurnId, status: TurnStatus },
    #[serde(rename = "turn/completed")]
    TurnCompleted { turn: Box<Turn> },
    /// Item birth; carries the revision=1 full snapshot (delta baseline).
    #[serde(rename = "item/started")]
    ItemStarted { item: Box<ItemEnvelope> },
    /// Non-delta content change of a living item; carries a full snapshot
    /// with a strictly increasing revision; clients replace by id.
    #[serde(rename = "item/updated")]
    ItemUpdated { item: Box<ItemEnvelope> },
    #[serde(rename = "item/assistantMessage/delta")]
    ItemAssistantMessageDelta(ItemDelta),
    #[serde(rename = "item/reasoning/delta")]
    ItemReasoningDelta(ItemDelta),
    #[serde(rename = "item/commandExecution/outputDelta")]
    ItemCommandExecutionOutputDelta(ItemDelta),
    /// All terminal states (Completed/Failed/Interrupted/Lost) go through
    /// this one notification with the terminal full snapshot; no separate
    /// `item/failed` exists.
    #[serde(rename = "item/completed")]
    ItemCompleted { item: Box<ItemEnvelope> },
    /// `drained` carries `queueItemId + startedTurnId` and is generated in
    /// the same session-actor operation as the matching `turn/started`, so
    /// External/A2A handles bind atomically.
    #[serde(rename = "queue/updated")]
    QueueUpdated {
        session_id: SessionId,
        change: QueueChange,
        queue_item_id: QueueItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_turn_id: Option<TurnId>,
        queue: Vec<QueueEntry>,
    },

    // ── Goal ──
    #[serde(rename = "session/goal/created")]
    GoalCreated { goal: Goal },
    #[serde(rename = "session/goal/updated")]
    GoalUpdated { goal: Goal },
    #[serde(rename = "session/goal/statusChanged")]
    GoalStatusChanged {
        session_id: SessionId,
        goal_id: super::ids::GoalId,
        status: GoalStatus,
    },
    #[serde(rename = "session/goal/cleared")]
    GoalCleared {
        session_id: SessionId,
        goal_id: super::ids::GoalId,
    },

    // ── Model / Context / Usage ──
    #[serde(rename = "model/queryFailed")]
    ModelQueryFailed {
        session_id: SessionId,
        turn_id: TurnId,
        error: AgentError,
    },
    #[serde(rename = "model/queryRetrying")]
    ModelQueryRetrying {
        session_id: SessionId,
        turn_id: TurnId,
        attempt: u32,
        max_attempts: u32,
        next_delay_ms: u64,
        error: AgentError,
        /// Provider/model/phase the retry display renders (ratified
        /// vocabulary, L2-DES-APP-009 DD-3). Optional for backward
        /// compatibility with the pre-extension wire shape.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<ModelQueryRetryPhase>,
    },
    /// Live mid-turn usage meter (ratified, L2-DES-APP-009 DD-3): carries the
    /// per-query meter the session-level totals cannot express. Live typed
    /// notification, not a durable subscription fact. `session_totals` keeps
    /// the live session-level meter intact while the legacy event it
    /// replaces carried both (no silent degradation).
    #[serde(rename = "turn/usage/updated")]
    TurnUsageUpdated {
        session_id: SessionId,
        turn_id: TurnId,
        usage: super::usage::TurnUsage,
        last_query_input_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_totals: Option<super::usage::UsageTotals>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// A turn superseded by an accepted message edit (ratified #10): durable
    /// history fact — the original turn stays recoverable, the replacement
    /// continues from the edited message.
    #[serde(rename = "turn/superseded")]
    TurnSuperseded {
        session_id: SessionId,
        superseded_turn_id: TurnId,
        replacement_turn_id: TurnId,
        edit_id: String,
        reason: String,
    },
    #[serde(rename = "context/usageUpdated")]
    ContextUsageUpdated {
        session_id: SessionId,
        occupancy: ContextOccupancy,
    },
    #[serde(rename = "context/compactionStarted")]
    ContextCompactionStarted {
        session_id: SessionId,
        turn_id: TurnId,
        trigger: CompactionTrigger,
    },
    #[serde(rename = "context/compactionCompleted")]
    ContextCompactionCompleted {
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
    },
    #[serde(rename = "session/usage/updated")]
    SessionUsageUpdated {
        session_id: SessionId,
        usage: Box<SessionUsage>,
    },

    // ── Background ──
    #[serde(rename = "task/started")]
    TaskStarted { item_id: ItemId },
    #[serde(rename = "task/delta")]
    TaskDelta {
        item_id: ItemId,
        chunk_index: u64,
        delta: String,
    },
    #[serde(rename = "task/completed")]
    TaskCompleted {
        item_id: ItemId,
        exit_code: Option<i32>,
    },
    #[serde(rename = "task/lost")]
    TaskLost { item_id: ItemId },
    #[serde(rename = "agent/started")]
    AgentStarted {
        item_id: ItemId,
        agent_session_id: SessionId,
    },
    #[serde(rename = "agent/progress")]
    AgentProgress { item_id: ItemId, summary: String },
    #[serde(rename = "agent/completed")]
    AgentCompleted {
        item_id: ItemId,
        agent_session_id: SessionId,
        state: SpawnedWorkState,
    },

    // ── Security ──
    #[serde(rename = "permission/decision")]
    PermissionDecision {
        session_id: SessionId,
        approval_id: String,
        decision: ApprovalDecision,
    },
    #[serde(rename = "security/alert")]
    SecurityAlert { code: String, message: String },
    #[serde(rename = "credential/changed")]
    CredentialChanged {
        credential_id: String,
        provider: String,
        change: CredentialChange,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum CredentialChange {
    Added,
    Updated,
    Deleted,
}

/// Phase of a provider retry cycle (ratified vocabulary, L2-DES-APP-009
/// DD-3): a retry is first `Scheduled`, then `Resumed` when the next attempt
/// starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ModelQueryRetryPhase {
    Scheduled,
    Resumed,
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// A durable position within one stream; only persisted events are cursorable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct EventCursor {
    pub stream_id: String,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StreamSelector {
    SessionsByCwd {
        cwd: PathBuf,
    },
    Session {
        #[schemars(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: SessionId,
    },
    BackgroundTask {
        #[schemars(rename = "itemId")]
        #[ts(rename = "itemId")]
        item_id: ItemId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct StreamSnapshot {
    pub stream_id: String,
    /// The snapshot is consistent with this barrier seq: the server read the
    /// barrier, registered the subscription, and produced the snapshot in one
    /// critical section, so nothing between read and subscribe is lost.
    pub barrier_seq: u64,
    pub data: SnapshotData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SnapshotData {
    SessionsList {
        sessions: Vec<Session>,
    },
    Session {
        session: Box<Session>,
        #[schemars(rename = "activeTurn")]
        #[ts(rename = "activeTurn")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_turn: Option<Box<Turn>>,
        queue: Vec<QueueEntry>,
    },
    BackgroundTask {
        item: Box<ItemEnvelope>,
    },
}

/// Forced full content of an active item on resubscription, so transient
/// deltas lost during the disconnect are corrected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LiveItemSnapshot {
    pub item: ItemEnvelope,
    pub accumulated: Vec<ChannelAccumulation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAccumulation {
    pub channel: DeltaChannel,
    pub text: String,
    pub next_chunk_index: u64,
}

/// An unanswered server->client control request (approval / structured
/// question). The first valid response wins. A persisted waiting item left by
/// a process crash remains audit history, but is not advertised here unless
/// the runtime still owns a live response channel for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingControlRequest {
    pub request_id: String,
    pub kind: ControlRequestKind,
    /// The waiting-state item (Approval / UserInputRequest).
    pub item: ItemEnvelope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ControlRequestKind {
    ApprovalCommand,
    ApprovalFileChange,
    ApprovalPermission,
    UserInput,
    GoalCompletion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionCreateParams {
    pub selectors: Vec<StreamSelector>,
    pub include_snapshot: bool,
    /// Positions from the client's last acks when resubscribing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<EventCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionCreateResult {
    pub subscription_id: SubscriptionId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<StreamSnapshot>,
    /// Persisted events in `(after, barrier]`, ordered by stream/seq.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay: Vec<EventEnvelope>,
    /// Mandatory when `after` is present, even if `include_snapshot = false`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery_snapshots: Vec<LiveItemSnapshot>,
    pub cursors: Vec<EventCursor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_control_requests: Vec<PendingControlRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUpdateParams {
    pub subscription_id: SubscriptionId,
    pub selectors: Vec<StreamSelector>,
}

/// Monotonic ack of cursors; also the server's basis for truncating the
/// persisted event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionAckParams {
    pub subscription_id: SubscriptionId,
    pub cursors: Vec<EventCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUnsubscribeParams {
    pub subscription_id: SubscriptionId,
}
