use std::path::PathBuf;
use std::time::Instant;

use chrono::DateTime;
use chrono::Utc;

use crate::app_command::InputHistoryDirection;
use crate::bottom_pane::SkillMetadata;
use devo_core::ItemId;
use devo_core::SessionId;
use devo_protocol::AcpAvailableCommand;
use devo_protocol::AcpCost;
use devo_protocol::AcpSessionConfigOption;
use devo_protocol::CollaborationMode;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderRetryPhase;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReferenceSearchSnapshot;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionRuntimeStatus;
use devo_protocol::ThreadGoal;
use devo_protocol::native::item::ContextOccupancy;
const TOOL_RESULT_FOLD_FINAL_STAGE: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanStep {
    pub(crate) text: String,
    pub(crate) status: PlanStepStatus,
}

/// One persisted session entry shown in the interactive session picker panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionListEntry {
    /// Stable session identifier used when switching the active session.
    pub(crate) session_id: SessionId,
    /// Human-readable session title shown to the user.
    pub(crate) title: String,
    /// First user-visible prompt, used as a title fallback and search term.
    pub(crate) preview: String,
    /// Session workspace identity.
    pub(crate) cwd: PathBuf,
    /// Git branch captured by the canonical session snapshot.
    pub(crate) branch: Option<String>,
    /// Last user-visible activity timestamp.
    pub(crate) last_activity_at: DateTime<Utc>,
    /// Current durable JSONL transcript size.
    pub(crate) transcript_size_bytes: Option<u64>,
    /// Whether this entry is the currently active session.
    pub(crate) is_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPreviewRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPreviewMessage {
    pub(crate) role: SessionPreviewRole,
    pub(crate) text: String,
}

/// One direct child agent shown in the read-only sub-agent monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentMonitorAgent {
    pub(crate) session_id: SessionId,
    pub(crate) parent_session_id: SessionId,
    pub(crate) agent_path: String,
    pub(crate) nickname: String,
    pub(crate) role: String,
    pub(crate) status: String,
    pub(crate) last_task_message: Option<String>,
}

/// Live event routed to the sub-agent monitor instead of the active parent transcript.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SubagentMonitorEvent {
    TurnStarted {
        session_id: SessionId,
        turn_id: TurnId,
    },
    TextItemStarted {
        session_id: SessionId,
        item_id: ItemId,
        kind: TextItemKind,
    },
    TextItemDelta {
        session_id: SessionId,
        item_id: Option<ItemId>,
        kind: TextItemKind,
        delta: String,
    },
    TextItemCompleted {
        session_id: SessionId,
        item_id: Option<ItemId>,
        kind: TextItemKind,
        final_text: String,
    },
    ToolCall {
        session_id: SessionId,
        tool_use_id: String,
        summary: String,
    },
    ToolCallUpdated {
        session_id: SessionId,
        tool_use_id: String,
        summary: String,
    },
    ToolOutputDelta {
        session_id: SessionId,
        tool_use_id: String,
        delta: String,
    },
    ToolResult {
        session_id: SessionId,
        tool_use_id: String,
        title: String,
        preview: String,
        is_error: bool,
    },
    PlanUpdated {
        session_id: SessionId,
        explanation: Option<String>,
        steps: Vec<PlanStep>,
    },
    TurnFinished {
        session_id: SessionId,
        status: String,
    },
    TurnFailed {
        session_id: SessionId,
        message: String,
    },
    TaskMessage {
        session_id: SessionId,
        message: String,
    },
    SessionStatusChanged {
        session_id: SessionId,
        status: SessionRuntimeStatus,
    },
}

/// One persisted model profile available for switching in the interactive model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedModelEntry {
    /// Stable model binding id when the entry comes from `[model_bindings]`.
    pub binding_id: Option<String>,
    /// Stable catalog model slug or custom model name.
    pub model: String,
    /// Provider-specific model name used in requests when it differs from `model`.
    pub request_model: Option<String>,
    /// Persisted display label for the saved binding.
    pub display_name: Option<String>,
    /// Provider config id that owns this saved model entry.
    pub provider_id: Option<String>,
    /// Human-readable provider label shown alongside the model picker item.
    pub provider_name: Option<String>,
    /// Concrete wire protocol stored for this model's provider profile.
    pub wire_api: ProviderWireApi,
    /// Optional provider base URL override stored with the model.
    pub base_url: Option<String>,
    /// Optional API key override stored with the model.
    pub api_key: Option<String>,
}

use devo_protocol::TurnId;

/// One event emitted by the background query worker into the interactive UI.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WorkerEvent {
    /// A new assistant turn has started.
    TurnStarted {
        /// The model slug resolved by the server for this turn.
        model: String,
        /// Stable provider model binding id used by the server for this turn.
        model_binding_id: Option<String>,
        /// The logical reasoning effort selection used for this turn.
        reasoning_effort_selection: Option<String>,
        /// The effective reasoning effort observed for this turn.
        reasoning_effort: Option<ReasoningEffort>,
        /// The server-assigned turn identifier.
        turn_id: TurnId,
    },
    /// The active session identifier is now known.
    SessionActivated {
        session_id: SessionId,
    },
    /// Native session queue snapshot (`queue/updated` / list / push).
    QueueUpdated {
        change: devo_protocol::native::queue::QueueChange,
        queue_item_id: devo_protocol::native::ids::QueueItemId,
        started_turn_id: Option<TurnId>,
        entries: Vec<devo_protocol::native::queue::QueueEntry>,
    },
    /// A steer (/btw) was accepted by the server.
    SteerAccepted {
        turn_id: TurnId,
    },
    /// Provider retry status for the active turn.
    ProviderRetryStatus {
        turn_id: TurnId,
        attempt: usize,
        backoff_ms: u64,
        provider: String,
        model: String,
        phase: ProviderRetryPhase,
        message: String,
    },
    /// A streamed Plan Mode proposal item started.
    ProposedPlanStarted {
        item_id: ItemId,
    },
    /// Incremental Markdown for the streamed Plan Mode proposal.
    ProposedPlanDelta {
        item_id: ItemId,
        delta: String,
    },
    /// A streamed Plan Mode proposal item completed.
    ProposedPlanCompleted {
        item_id: ItemId,
        final_text: String,
    },
    /// Incremental assistant text.
    TextDelta(String),
    /// Incremental reasoning text.
    ReasoningDelta(String),
    /// Final assistant text for a completed item.
    AssistantMessageCompleted(String),
    /// Final reasoning text for a completed item.
    ReasoningCompleted(String),
    /// A user-shell command/process finished outside the model turn loop.
    ShellCommandFinished {
        /// Process exit code when known.
        exit_code: Option<i32>,
    },
    /// A structured plan or todo list update.
    PlanUpdated {
        explanation: Option<String>,
        steps: Vec<PlanStep>,
    },
    ApprovalRequest {
        session_id: SessionId,
        turn_id: TurnId,
        approval_id: String,
        action_summary: String,
        justification: String,
        resource: Option<String>,
        available_scopes: Vec<String>,
        path: Option<String>,
        host: Option<String>,
        target: Option<String>,
        command_pattern: Option<Vec<String>>,
        command_prefix: Option<Vec<String>>,
    },
    RequestUserInput {
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: Vec<RequestUserInputQuestion>,
    },
    UserInputResolved {
        request_id: String,
    },
    ApprovalDecision {
        approval_id: String,
        decision: String,
        scope: String,
        tool_name: Option<String>,
        rationale: Option<String>,
    },
    /// Live usage update for the active turn.
    UsageUpdated {
        /// Total input tokens accumulated in the session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the session.
        total_output_tokens: usize,
        /// Display total tokens accumulated in the session.
        total_tokens: usize,
        /// Total cached input tokens accumulated in the session.
        total_cache_read_tokens: usize,
        /// Latest completed query display total (not session cumulative totals).
        last_query_total_tokens: usize,
        /// Input tokens from the latest completed query.
        last_query_input_tokens: usize,
    },
    /// The current turn completed successfully.
    TurnFinished {
        /// Human-readable stop reason.
        stop_reason: String,
        /// Total turns completed in the session.
        turn_count: usize,
        /// Total input tokens accumulated in the session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the session.
        total_output_tokens: usize,
        /// Display total tokens accumulated in the session.
        total_tokens: usize,
        /// Total cached input tokens accumulated in the session.
        total_cache_read_tokens: usize,
        /// Latest completed query display total (not session cumulative totals).
        last_query_total_tokens: usize,
        /// Input tokens from the latest completed query.
        last_query_input_tokens: usize,
        /// Estimated prompt tokens for the just-completed request.
        prompt_token_estimate: usize,
    },
    /// Live context-window occupancy breakdown for the active session.
    ContextUsageUpdated {
        /// Category occupancy anchored to the latest query display total.
        occupancy: ContextOccupancy,
    },
    /// The interrupt request could not be delivered or accepted.
    InterruptFailed {
        /// Human-readable failure reason to restore into the working status.
        message: String,
    },
    /// The current turn failed.
    TurnFailed {
        /// Human-readable error text to surface in the transcript and status bar.
        message: String,
        /// Optional user-facing next step for recovering from this failure.
        hint: Option<String>,
        /// Total turns completed in the session so far.
        turn_count: usize,
        /// Total input tokens accumulated in the session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the session.
        total_output_tokens: usize,
        /// Display total tokens accumulated in the session.
        total_tokens: usize,
        /// Total cached input tokens accumulated in the session.
        total_cache_read_tokens: usize,
        /// Estimated prompt tokens for the last attempted request.
        prompt_token_estimate: usize,
        /// Input tokens consumed by the last attempted query.
        last_query_input_tokens: usize,
    },
    /// Provider validation succeeded during onboarding.
    ProviderValidationSucceeded {
        /// Short human-readable confirmation from the probe request.
        reply_preview: String,
    },
    /// Provider validation failed during onboarding.
    ProviderValidationFailed {
        /// Human-readable failure reason from the probe request.
        message: String,
        /// Optional user-facing next step for recovering from this failure.
        hint: Option<String>,
    },
    /// Current provider vendors were listed from the server.
    ProviderVendorsListed {
        /// Structured provider vendors returned by `provider/list`.
        provider_vendors: Vec<ProviderVendor>,
    },
    /// A provider vendor was upserted through the server.
    ProviderVendorUpserted {
        /// The provider vendor returned by `provider/upsert`.
        provider_vendor: ProviderVendor,
        /// Optional model binding returned by `provider/upsert`.
        model_binding: Option<ProviderModelBinding>,
    },
    /// Provider vendor upsert failed during onboarding or provider updates.
    ProviderVendorUpsertFailed {
        /// Human-readable failure reason from `provider/upsert`.
        message: String,
    },
    /// Current known sessions were listed from the server.
    SessionsListed {
        /// Structured sessions rendered into the bottom picker panel.
        sessions: Vec<SessionListEntry>,
    },
    SessionsListFailed {
        message: String,
    },
    SessionPreviewLoaded {
        session_id: SessionId,
        messages: Vec<SessionPreviewMessage>,
    },
    SessionPreviewFailed {
        session_id: SessionId,
        message: String,
    },
    /// Current goal status loaded from the server.
    GoalStatusLoaded {
        /// The current goal, if the active session has one.
        goal: Option<ThreadGoal>,
    },
    /// Goal mutation completed on the server.
    GoalUpdated {
        /// Updated goal projection.
        goal: ThreadGoal,
    },
    /// A `/goal <objective>` command found an existing goal and needs user confirmation.
    GoalReplaceConfirmationRequested {
        /// Existing goal that would be replaced.
        current_goal: ThreadGoal,
        /// New objective requested by the user.
        objective: String,
    },
    /// The current goal was loaded for `/goal edit`.
    GoalEditLoaded {
        /// Goal to edit.
        goal: ThreadGoal,
    },
    /// Goal clear completed on the server.
    GoalCleared {
        /// Whether a goal was actually removed.
        cleared: bool,
    },
    /// Goal operation failed before or during the server RPC.
    GoalOperationFailed {
        /// Human-readable failure message.
        message: String,
    },
    /// A `/btw` side question has started in a forked lightweight agent.
    BtwStarted {
        /// The question submitted through `/btw`.
        question: String,
    },
    /// A `/btw` side question completed with a temporary answer.
    BtwCompleted {
        /// The original side question.
        question: String,
        /// Assistant answer from the side agent.
        answer: String,
    },
    /// A `/btw` side question failed before producing an answer.
    BtwFailed {
        /// Human-readable failure message.
        message: String,
    },
    /// A new child agent session was observed from server metadata.
    SubagentDiscovered {
        agent: SubagentMonitorAgent,
    },
    /// A live child-agent event should update the read-only monitor.
    SubagentMonitor {
        event: SubagentMonitorEvent,
    },
    /// Current known skills were listed from the server.
    SkillsListed {
        /// Structured skill metadata used by the composer `@skill` popup.
        skills: Vec<SkillMetadata>,
        /// Full skill list used by the interactive `/skills` picker.
        picker_skills: Vec<crate::skills_picker::SkillPickerEntry>,
        /// Whether `/skills` should open the interactive picker.
        open_picker: bool,
    },
    /// MCP server runtime statuses from `mcp/list`.
    McpServersListed {
        servers: Vec<devo_protocol::native::rpc_admin::McpServerInfo>,
    },
    /// Tools for one MCP server from `mcp/tools`.
    McpToolsListed {
        name: String,
        tools: Vec<devo_protocol::native::rpc_admin::McpToolEntry>,
    },
    /// MCP enable/disable applied via `mcp/set_enabled`.
    McpServerEnabled {
        name: String,
        enabled: bool,
        servers: Vec<devo_protocol::native::rpc_admin::McpServerInfo>,
    },
    /// MCP enable/disable failed.
    McpServerEnableFailed {
        name: String,
        message: String,
    },
    /// ACP-native available commands changed for the active session.
    AcpAvailableCommandsUpdated {
        /// Commands advertised through `session/update`.
        commands: Vec<AcpAvailableCommand>,
    },
    /// ACP-native current session mode changed.
    AcpCurrentModeUpdated {
        /// Current ACP session mode id.
        current_mode_id: String,
    },
    /// ACP-native session configuration options changed.
    AcpConfigOptionsUpdated {
        /// Full set of ACP config options from the update.
        config_options: Vec<AcpSessionConfigOption>,
    },
    /// ACP-native context window usage changed.
    AcpUsageUpdated {
        /// Tokens currently used in the context window.
        used: u64,
        /// Total context window size in tokens.
        size: u64,
        /// Optional cumulative ACP cost.
        cost: Option<AcpCost>,
    },
    /// Server-owned `@` reference search results for the composer popup.
    ReferenceSearchUpdated {
        /// Correlated unified result snapshot returned by `search/*`.
        snapshot: ReferenceSearchSnapshot,
    },
    /// The interactive client cleared its active session and is waiting for the next prompt.
    NewSessionPrepared {
        /// Working directory for the next newly-created session.
        cwd: std::path::PathBuf,
        /// Model currently configured for the next newly-created session.
        model: String,
        /// Stable provider model binding id configured for the next session.
        model_binding_id: Option<String>,
        /// Reasoning effort selection currently configured for the next newly-created session.
        reasoning_effort_selection: Option<String>,
        /// Effective reasoning effort currently configured for the next session.
        reasoning_effort: Option<ReasoningEffort>,
        permission_preset: devo_protocol::PermissionPreset,
        collaboration_mode: CollaborationMode,
        /// Contextual footer label for the active child agent, when viewing one.
        active_agent_label: Option<String>,
        /// Latest completed query display total for the fresh session.
        last_query_total_tokens: usize,
        /// Latest completed query input tokens for the fresh session.
        last_query_input_tokens: usize,
        /// Total cached input tokens accumulated in the fresh session.
        total_cache_read_tokens: usize,
    },
    /// The active session changed.
    SessionSwitched {
        /// The new active session identifier.
        session_id: String,
        /// Working directory restored from the resumed session metadata.
        cwd: std::path::PathBuf,
        /// Optional human-readable session title.
        title: Option<String>,
        /// The model restored from the resumed session, when one exists.
        model: Option<String>,
        /// Stable provider model binding id restored from the resumed session.
        model_binding_id: Option<String>,
        /// The reasoning effort selection restored from the resumed session, when one exists.
        reasoning_effort_selection: Option<String>,
        /// The effective reasoning effort restored from session context, when one exists.
        reasoning_effort: Option<ReasoningEffort>,
        /// Contextual footer label for the active child agent, when viewing one.
        active_agent_label: Option<String>,
        /// Total input tokens accumulated for the resumed session.
        total_input_tokens: usize,
        /// Total output tokens accumulated for the resumed session.
        total_output_tokens: usize,
        /// Display total tokens accumulated for the resumed session.
        total_tokens: usize,
        /// Total cached input tokens accumulated for the resumed session.
        total_cache_read_tokens: usize,
        /// Latest completed query display total (not session cumulative totals).
        last_query_total_tokens: usize,
        /// Input tokens from the latest completed query.
        last_query_input_tokens: usize,
        /// Estimated prompt tokens currently visible to the model.
        prompt_token_estimate: usize,
        /// Replay-friendly transcript items loaded from the resumed session.
        history_items: Vec<TranscriptItem>,
        /// Rich persisted history items used to rebuild semantic cells on resume.
        rich_history_items: Vec<SessionHistoryItem>,
        /// Number of persisted items loaded for the resumed session.
        loaded_item_count: u64,
        /// Pending turn input texts queued for the next turn.
        pending_texts: Vec<String>,
        /// Collaboration mode restored from the resumed session metadata.
        collaboration_mode: CollaborationMode,
        /// Permission preset restored from the resumed session metadata.
        permission_preset: Option<devo_protocol::PermissionPreset>,
        /// Session auto-compaction token limit override, when one is set.
        effective_context_window: Option<u64>,
        /// Latest context-window occupancy restored from rollout or session stats.
        last_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    },
    /// The current session title changed.
    SessionRenamed {
        /// The renamed session identifier.
        session_id: String,
        /// The new session title.
        title: String,
    },
    SessionRenameFailed {
        session_id: Option<SessionId>,
        message: String,
    },
    /// The current session was deleted.
    SessionDeleted {
        /// The deleted session identifier.
        session_id: String,
    },
    SessionDeleteFailed {
        session_id: Option<SessionId>,
        message: String,
    },
    /// Server confirmed a compaction-threshold hot update.
    EffectiveContextWindowUpdated {
        /// Absolute token threshold applied for the active session (model-clamped).
        effective_context_window: u64,
    },
    /// The active session or its context-compaction transcript item started compaction.
    SessionCompactionStarted,
    /// The active session completed a proactive compaction request.
    SessionCompacted {
        /// Total input tokens accumulated in the compacted session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the compacted session.
        total_output_tokens: usize,
        /// Display total tokens accumulated in the compacted session.
        total_tokens: usize,
        /// Latest/context display total after compaction.
        last_query_total_tokens: usize,
        /// Input tokens currently visible to the model after compaction.
        last_query_input_tokens: usize,
        /// Estimated prompt tokens currently visible to the model.
        prompt_token_estimate: usize,
    },
    /// A context-compaction transcript item was completed.
    ContextCompactionCompleted {
        /// Server-provided title retained for event compatibility.
        title: String,
    },
    /// The active session compaction request failed.
    SessionCompactionFailed {
        /// Human-readable failure reason.
        message: String,
    },
    /// The current session title changed due to automatic or explicit server-side updates.
    SessionTitleUpdated {
        /// The updated session identifier.
        session_id: String,
        /// The new best-known title.
        title: String,
    },
    /// One input-history query completed.
    InputHistoryLoaded {
        /// Which direction was requested.
        direction: InputHistoryDirection,
        /// History entry text, or `None` if there is no matching entry.
        text: Option<String>,
    },
    /// Native-first transcript lifecycle event for [`crate::transcript::TranscriptProjector`].
    Transcript(crate::transcript::lifecycle::ItemLifecycleEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextItemKind {
    Assistant,
    Reasoning,
}

/// One rendered transcript item shown in the history pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptItem {
    /// Stable kind used for styling and incremental updates.
    pub kind: TranscriptItemKind,
    /// Short title rendered above or before the body.
    pub title: String,
    /// Main text body for the transcript item.
    pub body: String,
    /// Time when the tool output should start folding away.
    pub fold_next_at: Option<Instant>,
    /// Current fold stage for tool outputs.
    pub fold_stage: u8,
    /// Duration of the turn that produced this item (milliseconds), if known.
    pub duration_ms: Option<u64>,
}

impl TranscriptItem {
    /// Creates a new transcript item with the supplied title and body.
    pub(crate) fn new(
        kind: TranscriptItemKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            body: body.into(),
            fold_next_at: None,
            fold_stage: 0,
            duration_ms: None,
        }
    }

    /// Creates a compact tool-call transcript item that only keeps the title row.
    pub(crate) fn tool_call(title: impl Into<String>) -> Self {
        Self::new(TranscriptItemKind::ToolCall, title, String::new())
    }

    /// Creates a restored historical tool-result item in its already-compacted state.
    pub(crate) fn restored_tool_result(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new(TranscriptItemKind::ToolResult, title, body)
            .with_fold_stage(TOOL_RESULT_FOLD_FINAL_STAGE)
    }

    /// Creates a tool error item that stays expanded because errors should remain visible.
    pub(crate) fn tool_error(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new(TranscriptItemKind::Error, title, body)
    }

    /// Forces a specific fold stage without scheduling the animation.
    pub(crate) fn with_fold_stage(mut self, stage: u8) -> Self {
        self.fold_stage = stage;
        self.fold_next_at = None;
        self
    }

    /// Attaches turn duration metadata to this transcript item.
    pub(crate) fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

#[allow(dead_code)]
/// Visual category for one transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptItemKind {
    /// User-authored prompt text.
    User,
    /// Assistant-authored text.
    Assistant,
    /// Model reasoning text.
    Reasoning,
    /// Tool execution start marker.
    ToolCall,
    /// Successful tool result.
    ToolResult,
    /// Failed tool result or runtime error.
    Error,
    Approval,
    /// Local UI/system note that is not model-authored content.
    System,
    /// Turn summary with model name and duration.
    TurnSummary,
}
