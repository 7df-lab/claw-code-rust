//! Params/result types for turn/queue/task/agent methods.
//! Truth source: `devo-api-design/01-native-api.md` §4.3/§4.6.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::ItemId;
use super::ids::QueueItemId;
use super::ids::SessionId;
use super::ids::TurnId;
use super::item::ItemEnvelope;
use super::item::UserInput;
use super::queue::QueueEntry;
use super::turn::Turn;

// ── turn/start ──

/// Precondition: the session is idle; otherwise `TURN_ALREADY_ACTIVE`. What
/// "send while busy" means (queue vs. steer) is a client-side choice, not a
/// server policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub session_id: SessionId,
    pub input: Vec<UserInput>,
    /// Domain-level dedup key for the materialized user message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    pub turn: Turn,
}

// ── turn/steer ──

/// Injects into the running turn: the item is persisted immediately
/// (`entry = steer`) and takes effect at the next injection boundary. If the
/// turn ended before injection, the input degrades back into the queue
/// (message is never lost) — the result says which happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub session_id: SessionId,
    /// Precondition guard against steering the wrong turn.
    pub expected_turn_id: TurnId,
    pub input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "outcome", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TurnSteerResult {
    Injected { item_id: ItemId },
    /// Turn ended before the injection boundary; input was queued instead.
    DegradedToQueue { entry: QueueEntry },
}

// ── turn/interrupt / turn/read ──

/// Idempotent; cascades into sub-agents; tool terminal states are written as
/// `interrupted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadParams {
    pub session_id: SessionId,
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadResult {
    pub turn: Turn,
}

// ── session/queue/* ──

/// If the session is idle the input immediately executes as a new turn
/// (`started`); otherwise it is queued as an editable pre-item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueuePushParams {
    pub session_id: SessionId,
    pub input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "outcome", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SessionQueuePushResult {
    Started { turn: Box<Turn> },
    Queued { entry: Box<QueueEntry> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueListParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueListResult {
    pub entries: Vec<QueueEntry>,
}

/// Queue entries are pre-items and freely editable; `input` is replaced
/// wholesale, `queueItemId` is stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueUpdateParams {
    pub session_id: SessionId,
    pub queue_item_id: QueueItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<UserInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueUpdateResult {
    pub entry: QueueEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueRemoveParams {
    pub session_id: SessionId,
    pub queue_item_id: QueueItemId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueRemoveResult {}

/// Promotes a queued entry into the running turn as a steer; fails with
/// `TURN_NOT_STEERABLE` when no steerable turn exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueSteerParams {
    pub session_id: SessionId,
    pub queue_item_id: QueueItemId,
    pub expected_turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueSteerResult {
    pub item_id: ItemId,
}

// ── task/* ──

/// Background tasks are addressed by their item id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadParams {
    pub item_id: ItemId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadResult {
    pub item: ItemEnvelope,
    /// Tail of captured output for quick display; full output streams on the
    /// `task:<item-id>` stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskWriteStdinParams {
    pub item_id: ItemId,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskWriteStdinResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskInterruptParams {
    pub item_id: ItemId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskInterruptResult {}

// ── agent/* ──

/// Sub-agents are created by tools/internal orchestration only; there is no
/// public `agent/spawn`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentListResult {
    /// `SubAgent` items linking to the agent sessions.
    pub agents: Vec<ItemEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadParams {
    pub item_id: ItemId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadResult {
    pub item: ItemEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_progress: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageParams {
    pub item_id: ItemId,
    pub input: Vec<UserInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelParams {
    pub item_id: ItemId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelResult {}
