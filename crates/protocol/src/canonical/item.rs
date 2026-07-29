//! The canonical `Item` tagged union: the single domain definition shared by
//! persistence (rollout JSONL) and the wire (`item/*` events).
//!
//! Truth source: `devo-api-design/06-item-model.md`.
//!
//! Hard rules (from the design):
//! - exactly one `Item` definition crate-wide; events carry typed items, never
//!   a `serde_json::Value` payload bag;
//! - `Item <-> ResponseItem` conversion exists only at the model boundary
//!   (ContextBuilder / TurnFinalizer), not here;
//! - `InternalEntry` is rollout-only and never appears in the public schema.

use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

use super::ids::GoalId;
use super::ids::ItemId;
use super::ids::SessionId;
use super::ids::TurnId;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Persistence-and-event shared header for every item. The item payload does
/// not embed an id; identity, ordering, revision and the common lifecycle
/// state live here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ItemEnvelope {
    pub id: ItemId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    /// Sequence position assigned on first appearance; strictly increasing
    /// within a session. Later updates reuse the same `seq`.
    pub seq: u64,
    /// Strictly increasing per `id`; `1` on first appearance. Readers fold
    /// updates by `(id, revision)`.
    pub revision: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: ItemState,
    pub item: Item,
}

/// Common delivery lifecycle of an item, owned by the envelope. Variants must
/// not duplicate their own status field (linked-work state on
/// `SubAgent`/`BackgroundTask` describes the external work, not delivery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Running,
    Waiting,
    Completed,
    Failed,
    Interrupted,
    Lost,
}

// ---------------------------------------------------------------------------
// Item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Item {
    // ── Conversation ──
    UserMessage {
        /// Domain-level dedup key: the same logical message materializes only
        /// once across steer/queue races and RPC retries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_user_message_id: Option<String>,
        content: Vec<UserInput>,
        /// Records the path this message actually took (absorbs the legacy
        /// `SteerInput` variant). Input still sitting in the queue is not yet
        /// an item of any turn.
        #[serde(default)]
        entry: UserMessageEntry,
    },
    AssistantMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<AssistantPhase>,
    },
    /// The provider's encrypted reasoning payload is stored by reference and
    /// re-attached when building outbound context.
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_payload_ref: Option<String>,
    },
    /// Typed projection of the `update_plan` tool: the tool call/result pair
    /// remains the replay truth (hidden from display); this variant is the
    /// single UI-facing plan truth and evolves over the whole turn.
    Plan { entries: Vec<PlanEntry> },

    // ── Local tools (call/result pairing + approval/sandbox) ──
    ToolCall {
        call_id: String,
        tool_name: String,
        source: ToolSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<JsonValue>,
    },
    ToolResult {
        call_id: String,
        output: JsonValue,
        /// Compressed UI rendering; does not change replay semantics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_content: Option<String>,
        is_error: bool,
        truncated: bool,
    },
    /// Specialized result variant for the exec tool family
    /// (`exec_command`/`write_stdin`). Model-initiated shell calls are the
    /// majority; user `!` commands land in the same variant with
    /// `origin = userShell`. Displaying the command plus prompt replay needs
    /// the original model input, hence a dedicated variant.
    CommandExecution {
        call_id: String,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argv: Option<Vec<String>>,
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<JsonValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<JsonValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_handle: Option<String>,
        is_error: bool,
        execution_mode: ExecutionMode,
        origin: ExecOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sandbox: Option<SandboxExecutionSummary>,
    },

    // ── Hosted tools (executed provider-side, passed through as a block) ──
    /// Generalizes the legacy `WebSearch`/`ImageGeneration` variants: future
    /// provider-hosted tools (code_interpreter, ...) need zero protocol
    /// changes; clients render by `tool_name`.
    HostedToolCall {
        call_id: String,
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<JsonValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<JsonValue>,
    },

    // ── File changes (closes the persistence hole) ──
    /// Specialized result for the write/edit/patch tool family. Per-file
    /// granularity (one apply_patch may touch many files), approval is per
    /// change, and a stable item lets diff-review UIs update in place.
    FileChange {
        call_id: String,
        changes: Vec<FileChangeEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sandbox: Option<SandboxExecutionSummary>,
    },

    // ── Interaction ──
    /// One logical approval interaction = one item (merges the legacy
    /// `ApprovalRequest` + `ApprovalDecision` pair); `decision = None` is the
    /// waiting state, filled in place on response.
    Approval {
        approval_id: String,
        /// Points at the action item being approved
        /// (CommandExecution/FileChange/ToolCall).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_item_id: Option<ItemId>,
        action_summary: String,
        justification: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ApprovalTarget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<ApprovalDecision>,
    },
    /// Structured questions from a tool (options/forms). Must be persisted so
    /// pending questions survive reconnect/resume and remain answerable.
    UserInputRequest {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_item_id: Option<ItemId>,
        questions: Vec<UserQuestion>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<JsonValue>,
    },

    // ── Linked (spawned async work, keeps the parent history complete) ──
    /// Not merged into `BackgroundTask`: the control planes are disjoint
    /// (process stdin/stdout vs. a full session's message channel and
    /// permission inheritance). Both share `SpawnedWorkState`.
    SubAgent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_call_id: Option<String>,
        agent_session_id: SessionId,
        parent_session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        task: String,
        state: SpawnedWorkState,
    },
    BackgroundTask {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_call_id: Option<String>,
        task_kind: BackgroundTaskKind,
        state: SpawnedWorkState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_handle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    // ── System ──
    ContextCompaction {
        trigger: CompactionTrigger,
        before: ContextUsage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ContextUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Human-readable milestone summary only; no percentage — a value that
    /// cannot be honestly computed does not enter the protocol.
    GoalProgress { goal_id: GoalId, summary: String },
    /// Non-fatal events (model retry, capability downgrade, quota pressure)
    /// that must leave a trace without failing the turn.
    Warning {
        code: String,
        message: String,
        retryable: bool,
    },
}

/// Client-side decode layer around `Item`: unknown variants (from a newer
/// server) degrade to `Unknown` with the raw JSON preserved, instead of
/// failing the whole decode. The server validates inbound items strictly and
/// never uses this wrapper.
///
/// Caveat of the untagged fallback: a *malformed* known item also degrades to
/// `Unknown`; clients should surface that, not silently drop it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItemOrUnknown {
    Known(Box<Item>),
    Unknown(JsonValue),
}

impl ItemOrUnknown {
    /// Returns the raw JSON for `Unknown` items (or serializes the known
    /// item) so nothing is lost on the client.
    pub fn raw(&self) -> JsonValue {
        match self {
            Self::Known(item) => serde_json::to_value(item).unwrap_or(JsonValue::Null),
            Self::Unknown(raw) => raw.clone(),
        }
    }
}

/// Rollout-only internal records. These are not public items: they never
/// appear in `item/*` events or the public schema, and the rollout reader
/// hands them straight to the recovery pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InternalEntry {
    TurnSummary { text: String },
    ToolProgress { call_id: String, message: String },
    HookPrompt { text: String },
}

// ---------------------------------------------------------------------------
// User input
// ---------------------------------------------------------------------------

/// One submission = one `UserMessage` item whose content is a list of parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UserInput {
    Text {
        text: String,
    },
    Image {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    LocalImage {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    /// Experimental; not part of the v1 guaranteed modalities.
    Audio {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    Skill {
        name: String,
    },
    Mention {
        uri: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImageDetail {
    Low,
    High,
    Auto,
}

/// How a user message entered the system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum UserMessageEntry {
    /// Submitted while idle; immediately started a new turn.
    #[default]
    TurnStart,
    /// Submitted while busy into the queue; started its own turn when drained.
    Queue,
    /// Injected into a running turn (including promotion from the queue).
    Steer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum AssistantPhase {
    Commentary,
    Final,
}

// ---------------------------------------------------------------------------
// Tooling
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub step: String,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToolSource {
    Builtin,
    Mcp,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionMode {
    Foreground,
    Background,
}

/// Who initiated a command execution: a model tool call, or the user's `!`
/// shell escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ExecOrigin {
    AgentTool,
    UserShell,
}

/// Compact description of how a sandboxed execution ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SandboxExecutionSummary {
    /// Sandbox backend, e.g. `seatbelt` / `landlock` / `windows` / `none`.
    pub backend: String,
    pub network_access: bool,
    /// Whether the execution ran outside the sandbox after approval.
    pub escalated: bool,
}

/// One file touched by a write/edit/patch tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeEntry {
    pub path: PathBuf,
    pub change: FileChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileChangeKind {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        move_path: Option<PathBuf>,
    },
}

// ---------------------------------------------------------------------------
// Approval & questions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecision {
    pub decision: ApprovalDecisionKind,
    pub scope: ApprovalScope,
    /// The authority that produced this decision. Legacy records predate this
    /// field and deserialize as `user`, which is the only decision source they
    /// could persist.
    #[serde(default)]
    pub decision_source: ApprovalDecisionSource,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecisionSource {
    StaticPolicy,
    ExecPolicy,
    #[default]
    User,
    AutoReview,
    Hook,
    ExternalPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecisionKind {
    Approved,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalScope {
    Once,
    Turn,
    Session,
    PathPrefix,
    Host,
    Tool,
    CommandPrefix,
    CommandPrefixPersist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApprovalTarget {
    Path { path: PathBuf },
    Host { host: String },
    Command { command: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<UserQuestionOption>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionOption {
    pub label: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Linked work & system
// ---------------------------------------------------------------------------

/// State of spawned external work referenced by `SubAgent`/`BackgroundTask`.
/// Distinct from the parent item envelope's delivery lifecycle. `Lost` makes
/// spawned work whose terminal state cannot be confirmed after a runtime
/// restart recognizable instead of pretending it completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SpawnedWorkState {
    Running,
    Completed,
    Failed,
    Cancelled,
    Lost,
}

/// v1 only has shell background tasks; new kinds are backward-compatible
/// variant additions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundTaskKind {
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum CompactionTrigger {
    AutoThreshold,
    Manual,
    ProviderRetry,
}

/// Context-window occupancy snapshot (distinct from billing usage, see the
/// usage module). `measured = false` marks parts that were not precisely
/// metered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub measured: bool,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn user_message_serializes_with_camel_case_tag_and_defaults() {
        let item = Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "hello".to_owned(),
            }],
            entry: UserMessageEntry::default(),
        };
        let json = serde_json::to_value(&item).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "type": "userMessage",
                "content": [{"type": "text", "text": "hello"}],
                "entry": "turnStart"
            })
        );
        let back: Item = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, item);
    }

    #[test]
    fn legacy_user_message_without_entry_defaults_to_turn_start() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "type": "userMessage",
            "content": [{"type": "text", "text": "hi"}]
        }))
        .expect("deserialize");
        assert_eq!(
            item,
            Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "hi".to_owned()
                }],
                entry: UserMessageEntry::TurnStart,
            }
        );
    }

    #[test]
    fn approval_waiting_state_has_no_decision_field() {
        let item = Item::Approval {
            approval_id: "appr_1".to_owned(),
            target_item_id: None,
            action_summary: "run cargo test".to_owned(),
            justification: "tests needed".to_owned(),
            resource: None,
            available_scopes: vec![],
            target: None,
            decision: None,
        };
        let json = serde_json::to_value(&item).expect("serialize");
        assert_eq!(json.get("decision"), None);
        let back: Item = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, item);
    }
}
