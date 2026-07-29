use chrono::{DateTime, Utc};
use devo_protocol::{ItemId, SessionId, TurnId, TurnKind, TurnStatus, TurnUsage};
use serde::{Deserialize, Serialize};

// ── DurableRecord Enum ────────────────────────────────────────────────

/// Every append-only JSONL record. One variant per record type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_kind", rename_all = "snake_case")]
pub enum DurableRecord {
    // Session lifecyle
    SessionCreated(SessionCreatedRecord),
    SessionForked(SessionForkedRecord),
    SessionMetadataUpdated(SessionMetadataUpdatedRecord),
    SessionDeleted(SessionDeletedRecord),

    // Transcript — turn lifecyle
    TurnStarted(TurnStartedRecord),
    TurnCompleted(TurnCompletedRecord),
    TurnFailed(TurnFailedRecord),
    TurnInterrupted(TurnInterruptedRecord),

    // Transcript — item lifecyle
    ItemStarted(ItemStartedRecord),
    ItemContentAppended(ItemContentAppendedRecord),
    ItemCompleted(ItemCompletedRecord),
    ItemFailed(ItemFailedRecord),

    // Active-turn messages
    SteerRecorded(SteerRecordedRecord),
    QueueItemRecorded(QueueItemRecordedRecord),
    QueueItemResolved(QueueItemResolvedRecord),

    // Interrupt / resume
    TurnInterruptRequested(TurnInterruptRequestedRecord),
    TurnResumeStarted(TurnResumeStartedRecord),

    // Usage
    UsageRecorded(UsageRecordedRecord),

    // Message editing
    MessageEditRecorded(MessageEditRecordedRecord),
    TurnSuperseded(TurnSupersededRecord),

    // Workspace
    TurnWorkspaceCheckpointRecorded(TurnWorkspaceCheckpointRecordedRecord),
    TurnWorkspaceChangeRecorded(TurnWorkspaceChangeRecordedRecord),
    TurnWorkspaceRestoreStarted(TurnWorkspaceRestoreStartedRecord),
    TurnWorkspaceRestoreCompleted(TurnWorkspaceRestoreCompletedRecord),

    // Plan
    PlanCreated(PlanCreatedRecord),
    PlanUpdated(PlanUpdatedRecord),

    // Goal
    GoalCreated(GoalCreatedRecord),
    GoalReplaced(GoalReplacedRecord),
    GoalStatusChanged(GoalStatusChangedRecord),
    GoalBudgetAccounted(GoalBudgetAccountedRecord),
    GoalProgressRecorded(GoalProgressRecordedRecord),
    GoalContextSnapshotRecorded(GoalContextSnapshotRecordedRecord),
    GoalCleared(GoalClearedRecord),

    // Context
    ContextSnapshotRecorded(ContextSnapshotRecordedRecord),
    ContextCompactionStarted(ContextCompactionStartedRecord),
    ContextCompactionCompleted(ContextCompactionCompletedRecord),

    // Memory (internal)
    MemoryLinkRecorded(MemoryLinkRecordedRecord),

    // Subagents
    SubagentSpawned(SubagentSpawnedRecord),
    SubagentClosed(SubagentClosedRecord),
    SubagentMailRecorded(SubagentMailRecordedRecord),
    SubagentStatusChanged(SubagentStatusChangedRecord),
    SubagentNotificationRecorded(SubagentNotificationRecordedRecord),

    // Background process
    BackgroundProcessUpdated(BackgroundProcessUpdatedRecord),
}

impl DurableRecord {
    pub fn record_kind(&self) -> &'static str {
        match self {
            Self::SessionCreated(_) => "session_created",
            Self::SessionForked(_) => "session_forked",
            Self::SessionMetadataUpdated(_) => "session_metadata_updated",
            Self::SessionDeleted(_) => "session_deleted",
            Self::TurnStarted(_) => "turn_started",
            Self::TurnCompleted(_) => "turn_completed",
            Self::TurnFailed(_) => "turn_failed",
            Self::TurnInterrupted(_) => "turn_interrupted",
            Self::ItemStarted(_) => "item_started",
            Self::ItemContentAppended(_) => "item_content_appended",
            Self::ItemCompleted(_) => "item_completed",
            Self::ItemFailed(_) => "item_failed",
            Self::SteerRecorded(_) => "steer_recorded",
            Self::QueueItemRecorded(_) => "queue_item_recorded",
            Self::QueueItemResolved(_) => "queue_item_resolved",
            Self::TurnInterruptRequested(_) => "turn_interrupt_requested",
            Self::TurnResumeStarted(_) => "turn_resume_started",
            Self::UsageRecorded(_) => "usage_recorded",
            Self::MessageEditRecorded(_) => "message_edit_recorded",
            Self::TurnSuperseded(_) => "turn_superseded",
            Self::TurnWorkspaceCheckpointRecorded(_) => "turn_workspace_checkpoint_recorded",
            Self::TurnWorkspaceChangeRecorded(_) => "turn_workspace_change_recorded",
            Self::TurnWorkspaceRestoreStarted(_) => "turn_workspace_restore_started",
            Self::TurnWorkspaceRestoreCompleted(_) => "turn_workspace_restore_completed",
            Self::PlanCreated(_) => "plan_created",
            Self::PlanUpdated(_) => "plan_updated",
            Self::GoalCreated(_) => "goal_created",
            Self::GoalReplaced(_) => "goal_replaced",
            Self::GoalStatusChanged(_) => "goal_status_changed",
            Self::GoalBudgetAccounted(_) => "goal_budget_accounted",
            Self::GoalProgressRecorded(_) => "goal_progress_recorded",
            Self::GoalContextSnapshotRecorded(_) => "goal_context_snapshot_recorded",
            Self::GoalCleared(_) => "goal_cleared",
            Self::ContextSnapshotRecorded(_) => "context_snapshot_recorded",
            Self::ContextCompactionStarted(_) => "context_compaction_started",
            Self::ContextCompactionCompleted(_) => "context_compaction_completed",
            Self::MemoryLinkRecorded(_) => "memory_link_recorded",
            Self::SubagentSpawned(_) => "subagent_spawned",
            Self::SubagentClosed(_) => "subagent_closed",
            Self::SubagentMailRecorded(_) => "subagent_mail_recorded",
            Self::SubagentStatusChanged(_) => "subagent_status_changed",
            Self::SubagentNotificationRecorded(_) => "subagent_notification_recorded",
            Self::BackgroundProcessUpdated(_) => "background_process_updated",
        }
    }
}

// ── Session Lifecycle Records ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCreatedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub workspace_root: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionForkedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub fork_origin: ForkOrigin,
    pub inherited_segment: InheritedHistorySegmentDescriptor,
    pub workspace_root: String,
    pub fork_label: Option<String>,
    pub created_by: ForkCreator,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMetadataUpdatedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDeletedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub deleted_at: DateTime<Utc>,
}

// ── Turn Lifecycle Records ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnStartedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub kind: TurnKind,
    pub resume_of_turn_id: Option<TurnId>,
    pub submitted_by_client_id: Option<String>,
    pub model: Option<String>,
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    pub reasoning_effort: Option<devo_protocol::ReasoningEffort>,
    pub started_at: DateTime<Utc>,
}

/// Shared terminal fields for Completed/Failed/Interrupted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnTerminalFields {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub status: TurnStatus,
    pub usage: Option<TurnUsage>,
    pub workspace_change_set_id: Option<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnCompletedRecord {
    pub schema_version: u32,
    #[serde(flatten)]
    pub terminal: TurnTerminalFields,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnFailedRecord {
    pub schema_version: u32,
    #[serde(flatten)]
    pub terminal: TurnTerminalFields,
    pub error: Option<TurnExecutionError>,
}

/// Normalized turn-scoped error payload for durable records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnExecutionError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptedRecord {
    pub schema_version: u32,
    #[serde(flatten)]
    pub terminal: TurnTerminalFields,
}

// ── Item Lifecycle Records ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStartedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub item_id: ItemId,
    pub kind: ItemRecordKind,
    pub role: RecordRole,
    pub content_parts: Vec<ContentPart>,
    pub mentions: Vec<Mention>,
    pub visibility: ItemVisibility,
    pub created_at: DateTime<Utc>,
}

/// A content part within an item, carrying typed payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "content_type", content = "value", rename_all = "snake_case")]
pub enum ContentPart {
    Text(String),
    ImageRef {
        artifact_id: String,
    },
    FileRef {
        path: String,
        artifact_id: Option<String>,
    },
    ToolCallJson(serde_json::Value),
    ToolResultText(String),
    ProviderMetadata(serde_json::Value),
}

/// A structured mention attached to a user input item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mention {
    pub mention_id: String,
    pub kind: MentionKind,
    pub display_text: String,
    pub target: String,
    pub source_range: Option<SourceRange>,
    pub resolution_status: MentionResolutionStatus,
    pub visibility: MentionVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionKind {
    Skill,
    File,
    Directory,
    McpResource,
    McpTemplate,
    ToolOrConnector,
    Session,
    Turn,
    Transcript,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionResolutionStatus {
    Resolved,
    Unresolved,
    Stale,
    PermissionBlocked,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionVisibility {
    Visible,
    Hidden,
}

/// Byte or grapheme range occupied by a mention token in submitted text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemRecordKind {
    UserInput,
    AssistantText,
    AssistantReasoning,
    ToolCall,
    ToolResult,
    ApprovalRequest,
    QuestionRequest,
    SteerMessage,
    QueueMessage,
    Error,
    ContextSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemVisibility {
    Visible,
    Hidden,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemContentAppendedRecord {
    pub schema_version: u32,
    pub item_id: ItemId,
    pub content_part_index: u32,
    pub offset: u64,
    pub content_kind: ContentAppendKind,
    pub content: String,
    pub byte_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentAppendKind {
    Text,
    Reasoning,
    ToolCallJson,
    ToolResultText,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemCompletedRecord {
    pub schema_version: u32,
    pub item_id: ItemId,
    pub turn_id: TurnId,
    pub final_status: ItemStatus,
    pub content_hash: Option<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemFailedRecord {
    pub schema_version: u32,
    pub item_id: ItemId,
    pub turn_id: TurnId,
    pub final_status: ItemStatus,
    pub error: Option<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Completed,
    Failed,
    Interrupted,
    Denied,
    Blocked,
    Canceled,
}

// ── Active-Turn Message Records ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteerRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub item_id: ItemId,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueItemRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub item_id: ItemId,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueItemResolvedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub item_id: ItemId,
    pub resolved_at: DateTime<Utc>,
}

// ── Interrupt / Resume Records ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptRequestedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub reason: Option<String>,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnResumeStartedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub interrupted_turn_id: TurnId,
    pub resume_turn_id: TurnId,
    pub started_at: DateTime<Utc>,
}

// ── Usage Record ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub invocation_id: InvocationId,
    pub model_binding_id: ModelBindingId,
    pub canonical_model_slug: String,
    pub provider_id: ProviderId,
    pub invocation_method: InvocationMethod,
    pub reasoning_effort: Option<devo_protocol::ReasoningEffort>,
    pub metrics: Vec<UsageMetric>,
    pub context_pressure: ContextPressure,
    pub recorded_at: DateTime<Utc>,
}

/// Identifies a single model invocation within a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvocationId(pub uuid::Uuid);

impl Default for InvocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl InvocationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for InvocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies a model-provider binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelBindingId(pub uuid::Uuid);

impl Default for ModelBindingId {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelBindingId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for ModelBindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies a provider instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub uuid::Uuid);

impl Default for ProviderId {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Normalized provider SDK/method used for an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationMethod {
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
}

/// A single normalized usage metric with source labeling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageMetric {
    pub metric_kind: UsageMetricKind,
    pub value: i64,
    pub source: MetricSource,
    pub confidence: MetricConfidence,
    pub inclusion: MetricInclusion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageMetricKind {
    InputTokens,
    OutputTokens,
    CacheCreationInputTokens,
    CacheReadInputTokens,
    ReasoningOutputTokens,
    TotalTokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricSource {
    ProviderReported,
    LocallyEstimated,
    Unavailable,
    Redacted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricConfidence {
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricInclusion {
    Included,
    Excluded,
    Unknown,
}

/// Context pressure snapshot recorded with each invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPressure {
    pub context_size: u64,
    pub effective_limit: u64,
    pub pressure_state: ContextPressureState,
    pub compaction_status: CompactionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPressureState {
    Normal,
    High,
    NearLimit,
    OverLimit,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    NotNeeded,
    InProgress,
    Completed,
    Failed,
}

// ── Fork / Inherited History Types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForkOrigin {
    pub parent_session_id: SessionId,
    pub fork_turn_id: TurnId,
    pub fork_created_at: DateTime<Utc>,
    pub parent_display_label: String,
    pub fork_turn_display_label: String,
    pub fork_turn_digest: String,
    pub origin_snapshot_hash: String,
    pub parent_availability: ParentAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentAvailability {
    Available,
    Archived,
    Deleted,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InheritedHistorySegmentDescriptor {
    pub inherited_segment_id: String,
    pub source_parent_session_id: SessionId,
    pub source_range: SegmentSourceRange,
    pub storage_strategy: StorageStrategy,
    pub record_refs: Vec<RecordRef>,
    pub segment_hash: String,
    pub availability_state: SegmentAvailability,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentSourceRange {
    pub start_offset: u64,
    pub end_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageStrategy {
    ProtectedSharedSegment,
    MaterializedForkSegment,
    ProtectedRetainedSourceRecords,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentAvailability {
    Available,
    Materialized,
    Protected,
    Missing,
    Corrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordRef {
    pub source_session_id: SessionId,
    pub record_sequence: u64,
    pub record_offset: u64,
    pub record_kind: String,
    pub record_hash: String,
    pub materialized_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkCreator {
    User,
    Subagent,
    System,
}

// ── Message Edit / Turn Superseded Records ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageEditRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub edit_id: EditId,
    pub target_message_id: ItemId,
    pub replacement_message_id: ItemId,
    pub target_turn_id: Option<TurnId>,
    pub replacement_turn_id: Option<TurnId>,
    pub queue_item_id: Option<String>,
    pub edited_content_parts: Vec<ContentPart>,
    pub edited_mentions: Vec<Mention>,
    pub workspace_restore_policy: WorkspaceRestorePolicy,
    pub edit_state: EditState,
    pub requested_by_client_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditId(pub uuid::Uuid);

impl Default for EditId {
    fn default() -> Self {
        Self::new()
    }
}

impl EditId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRestorePolicy {
    Safe,
    Skip,
    ConfiguredRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditState {
    Accepted,
    RestorePending,
    ReplacementStarted,
    QueuedUpdated,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnSupersededRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub superseded_turn_id: TurnId,
    pub replacement_turn_id: TurnId,
    pub edit_id: EditId,
    pub restore_id: Option<RestoreId>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreId(pub uuid::Uuid);

impl Default for RestoreId {
    fn default() -> Self {
        Self::new()
    }
}

impl RestoreId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

// ── Workspace Records ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnWorkspaceCheckpointRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub checkpoint_id: String,
    pub pre_turn_hash: String,
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<ChangeSetCoverage>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    /// Untracked paths that existed at checkpoint creation. Git rollback uses
    /// this manifest to delete only paths created after the checkpoint.
    /// Legacy checkpoints omit it and therefore restore tracked files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preexisting_untracked_files: Option<Vec<String>>,
    /// Pre-existing untracked directories paired with
    /// `preexisting_untracked_files`; see that field for legacy semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preexisting_untracked_dirs: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnWorkspaceChangeRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub change_id: String,
    pub file_path: String,
    pub pre_hash: String,
    pub post_hash: String,
    pub inverse_ref: Option<String>,
    pub display_diff_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<ChangeSetCoverage>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_set_status: Option<ChangeSetStatus>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnWorkspaceRestoreStartedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub restore_id: RestoreId,
    pub candidate_files: Vec<String>,
    pub policy: WorkspaceRestorePolicy,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnWorkspaceRestoreCompletedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub restore_id: RestoreId,
    pub outcomes: Vec<FileRestoreOutcome>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRestoreOutcome {
    pub file_path: String,
    pub status: RestoreFileStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreFileStatus {
    Restored,
    Skipped,
    Unsupported,
    Failed,
}

// ── Plan Records ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanCreatedRecord {
    pub schema_version: u32,
    pub plan_id: PlanId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub objective: String,
    pub items: Vec<PlanItemRecord>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanUpdatedRecord {
    pub schema_version: u32,
    pub plan_id: PlanId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub objective: Option<String>,
    pub status: Option<PlanStatus>,
    pub changed_item_ids: Vec<String>,
    pub items: Vec<PlanItemRecord>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanId(pub uuid::Uuid);

impl Default for PlanId {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    Completed,
    Blocked,
    Abandoned,
    Superseded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItemRecord {
    pub plan_item_id: String,
    pub text: String,
    pub status: PlanItemStatus,
    pub details: Option<String>,
    pub parent_item_id: Option<String>,
    pub parallel_group_id: Option<String>,
    pub source_turn_id: TurnId,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Canceled,
}

// ── Goal Records ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalCreatedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub prompt: String,
    pub description: Option<String>,
    pub max_iterations: Option<u32>,
    pub budget: Option<GoalBudget>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalReplacedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub previous_goal_id: GoalId,
    pub prompt: String,
    pub description: Option<String>,
    pub replaced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalStatusChangedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub previous_status: GoalStatus,
    pub new_status: GoalStatus,
    pub reason: Option<String>,
    pub changed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalBudgetAccountedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub budget_delta: GoalBudget,
    pub remaining_budget: GoalBudget,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalProgressRecordedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub summary: String,
    pub progress_type: GoalProgressType,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalContextSnapshotRecordedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub snapshot_id: String,
    pub summary: String,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalClearedRecord {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub reason: Option<String>,
    pub cleared_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GoalId(pub uuid::Uuid);

impl Default for GoalId {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Completed,
    Failed,
    Blocked,
    Canceled,
    Cleared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalBudget {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<i64>,
    pub max_duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalProgressType {
    Milestone,
    PhaseComplete,
    Blocked,
    Note,
}

// ── Context Records ────────────────────────────────────────────────────

/// Work around `serde_json` `arbitrary_precision` breaking `f64` fields inside
/// internally tagged enums (`DurableRecord`): buffered number content can appear
/// as a map to the nested deserializer.
fn deserialize_f64_via_json_value<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| D::Error::custom("expected finite f64")),
        other => serde_json::from_value(other).map_err(D::Error::custom),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSnapshotRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub snapshot_id: String,
    pub context_size: u64,
    pub token_estimate: u64,
    pub entry_count: u32,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextCompactionStartedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub compaction_id: String,
    pub pre_compaction_context_size: u64,
    /// Compaction trigger ratio. Custom deserialize works around `serde_json`
    /// `arbitrary_precision` breaking `f64` inside internally tagged enums.
    #[serde(deserialize_with = "deserialize_f64_via_json_value")]
    pub threshold_ratio: f64,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextCompactionCompletedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub compaction_id: String,
    pub summary_item_id: ItemId,
    pub post_compaction_context_size: u64,
    pub preserved_item_ids: Vec<ItemId>,
    pub completed_at: DateTime<Utc>,
}

// ── Memory Record ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryLinkRecordedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub memory_id: String,
    pub memory_file: String,
    pub link_type: MemoryLinkType,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLinkType {
    Extraction,
    Consolidation,
    AdHocNote,
    SkillGenerated,
}

// ── Subagent Records ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentSpawnedRecord {
    pub schema_version: u32,
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub agent_nickname: String,
    pub agent_role: String,
    pub agent_path: Option<String>,
    pub spawned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentClosedRecord {
    pub schema_version: u32,
    pub child_session_id: SessionId,
    pub parent_session_id: SessionId,
    pub final_status: SubagentFinalStatus,
    pub closed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentFinalStatus {
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentMailRecordedRecord {
    pub schema_version: u32,
    pub from_session_id: SessionId,
    pub to_session_id: SessionId,
    pub mail_id: String,
    pub content: String,
    pub sequence: u64,
    pub sent_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentStatusChangedRecord {
    pub schema_version: u32,
    pub child_session_id: SessionId,
    pub previous_status: SubagentRunStatus,
    pub new_status: SubagentRunStatus,
    pub changed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentRunStatus {
    Spawning,
    Running,
    WaitingForInput,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentNotificationRecordedRecord {
    pub schema_version: u32,
    pub child_session_id: SessionId,
    pub parent_session_id: SessionId,
    pub notification_id: String,
    pub notification_type: SubagentNotificationType,
    pub delivered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentNotificationType {
    Completion,
    Error,
    Progress,
}

// ── Background Process Record ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundProcessUpdatedRecord {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub process_id: String,
    pub status: BackgroundProcessStatus,
    pub exit_code: Option<i32>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundProcessStatus {
    Running,
    Completed,
    Failed,
    Stopped,
    Detached,
}

// ── Workspace Change Tracking (L3-BEH-CORE-006 B6) ────────────────────

/// Tracks file changes made by structured mutating tools during a turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceChangeSet {
    pub change_set_id: String,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub checkpoint_id: Option<String>,
    pub structured_tool_coverage: ChangeSetCoverage,
    pub shell_change_coverage: ChangeSetCoverage,
    pub file_change_refs: Vec<String>,
    pub display_diff_ref: Option<String>,
    pub restore_data_ref: Option<String>,
    pub change_set_status: ChangeSetStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetCoverage {
    Full,
    Partial,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetStatus {
    Accumulating,
    Finalized,
    Restored,
    Discarded,
}

/// A single file change attributed to a tool call.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileChange {
    pub file_change_id: String,
    pub turn_id: TurnId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub path: String,
    pub change_kind: FileChangeKind,
    pub pre_state_ref: Option<String>,
    pub pre_state_hash: Option<String>,
    pub post_state_ref: Option<String>,
    pub post_state_hash: Option<String>,
    pub inverse_ref: Option<String>,
    pub display_diff_hunk_ref: Option<String>,
    pub attribution_confidence: AttributionConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Create,
    Modify,
    Delete,
    Rename,
    ModeChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionConfidence {
    Exact,
    High,
    Medium,
    Low,
    Inferred,
}

// ── Turn Execution Phase State Machine ─────────────────────────────────

/// The server-visible execution phase of a turn.
/// Drives orchestration: server checks phase to decide what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    /// Turn accepted but context assembly hasn't started.
    Admitted,
    /// Context assembly in progress.
    AssemblingContext,
    /// Model invocation in progress (provider call active).
    ModelInvocation,
    /// Executing tool calls requested by the model.
    ToolDispatch,
    /// Waiting for user approval on one or more tool calls.
    WaitingApproval,
    /// Recording durable state and preparing terminal status.
    Finalizing,
    /// Turn ended successfully.
    Completed,
    /// Turn ended with an unrecoverable error.
    Failed,
    /// Turn was interrupted by user or system.
    Interrupted,
}

impl ExecutionPhase {
    /// Returns `true` if this phase is terminal (no further transitions allowed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Validate a transition from `self` to `next`.
    /// Returns `Ok(())` if legal, `Err(reason)` if illegal.
    pub fn can_transition_to(&self, next: ExecutionPhase) -> Result<(), &'static str> {
        use ExecutionPhase::*;
        match (self, next) {
            // Legal transitions per L3-BEH-CORE-001 turn state machine
            (Admitted, AssemblingContext) => Ok(()),
            (Admitted, Failed) => Ok(()),
            (AssemblingContext, ModelInvocation) => Ok(()),
            (AssemblingContext, Failed) => Ok(()),
            (ModelInvocation, ToolDispatch) => Ok(()),
            (ModelInvocation, Finalizing) => Ok(()),
            (ModelInvocation, Failed) => Ok(()),
            (ToolDispatch, ModelInvocation) => Ok(()),
            (ToolDispatch, WaitingApproval) => Ok(()),
            (ToolDispatch, Finalizing) => Ok(()),
            (ToolDispatch, Failed) => Ok(()),
            (WaitingApproval, ToolDispatch) => Ok(()),
            (WaitingApproval, Finalizing) => Ok(()),
            (WaitingApproval, Failed) => Ok(()),
            (Finalizing, Completed) => Ok(()),
            (Finalizing, Failed) => Ok(()),

            // Interrupt can happen from any non-terminal phase
            (Admitted, Interrupted) => Ok(()),
            (AssemblingContext, Interrupted) => Ok(()),
            (ModelInvocation, Interrupted) => Ok(()),
            (ToolDispatch, Interrupted) => Ok(()),
            (WaitingApproval, Interrupted) => Ok(()),
            (Finalizing, Interrupted) => Ok(()),

            // Interrupted turns are terminal; resume creates a new turn.
            (Interrupted, _) => Err("interrupted turns are terminal"),

            // All other transitions are illegal.
            _ => Err("illegal transition"),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn session_created_roundtrip() {
        let record = DurableRecord::SessionCreated(SessionCreatedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            workspace_root: "/home/user/project".into(),
            created_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), restored.record_kind());
        assert_eq!(record.record_kind(), "session_created");
    }

    #[test]
    fn turn_started_roundtrip() {
        let record = DurableRecord::TurnStarted(TurnStartedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            sequence: 0,
            status: TurnStatus::Running,
            kind: TurnKind::Regular,
            resume_of_turn_id: None,
            submitted_by_client_id: Some("tui-1".into()),
            model: Some("deepseek-v4-pro".into()),
            reasoning_effort_selection: Some("high".into()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::High),
            started_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), restored.record_kind());
    }

    #[test]
    fn turn_started_reads_legacy_thinking_field() {
        let expected = TurnStartedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            sequence: 0,
            status: TurnStatus::Running,
            kind: TurnKind::Regular,
            resume_of_turn_id: None,
            submitted_by_client_id: Some("tui-1".into()),
            model: Some("deepseek-v4-pro".into()),
            reasoning_effort_selection: Some("high".into()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::High),
            started_at: now(),
        };
        let mut value =
            serde_json::to_value(DurableRecord::TurnStarted(expected.clone())).expect("serialize");
        let object = value.as_object_mut().expect("turn-started json object");
        object.remove("reasoning_effort_selection");
        object.insert("thinking".to_string(), serde_json::json!("high"));

        let restored: DurableRecord = serde_json::from_value(value).expect("deserialize legacy");
        let DurableRecord::TurnStarted(restored) = restored else {
            panic!("expected turn started record");
        };
        assert_eq!(restored, expected);

        let serialized =
            serde_json::to_value(DurableRecord::TurnStarted(restored)).expect("serialize restored");
        assert_eq!(serialized["reasoning_effort_selection"], "high");
        assert_eq!(serialized.get("thinking"), None);
    }

    #[test]
    fn item_started_roundtrip() {
        let record = DurableRecord::ItemStarted(ItemStartedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            item_id: ItemId::new(),
            kind: ItemRecordKind::AssistantText,
            role: RecordRole::Assistant,
            content_parts: vec![],
            mentions: vec![],
            visibility: ItemVisibility::Visible,
            created_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), restored.record_kind());
        assert_eq!(record.record_kind(), "item_started");
    }

    #[test]
    fn item_content_appended_roundtrip() {
        let record = DurableRecord::ItemContentAppended(ItemContentAppendedRecord {
            schema_version: 1,
            item_id: ItemId::new(),
            content_part_index: 0,
            offset: 0,
            content_kind: ContentAppendKind::Text,
            content: "Hello, world!".into(),
            byte_count: 13,
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), restored.record_kind());
    }

    #[test]
    fn turn_completed_with_usage_roundtrip() {
        let usage = TurnUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
            reasoning_output_tokens: None,
            total_tokens: None,
        };
        let record = DurableRecord::TurnCompleted(TurnCompletedRecord {
            schema_version: 1,
            terminal: TurnTerminalFields {
                turn_id: TurnId::new(),
                session_id: SessionId::new(),
                status: TurnStatus::Completed,
                usage: Some(usage),
                workspace_change_set_id: None,
                completed_at: now(),
            },
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), "turn_completed");
        assert_eq!(restored.record_kind(), "turn_completed");
    }

    #[test]
    fn item_failed_with_error_roundtrip() {
        let record = DurableRecord::ItemFailed(ItemFailedRecord {
            schema_version: 1,
            item_id: ItemId::new(),
            turn_id: TurnId::new(),
            final_status: ItemStatus::Failed,
            error: Some("permission denied".into()),
            completed_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), "item_failed");
        assert_eq!(restored.record_kind(), "item_failed");
    }

    #[test]
    fn all_record_kinds_unique() {
        let kinds = vec![
            DurableRecord::SessionCreated(SessionCreatedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                workspace_root: "/tmp".into(),
                created_at: now(),
            })
            .record_kind(),
            DurableRecord::TurnStarted(TurnStartedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                sequence: 0,
                status: TurnStatus::Running,
                kind: TurnKind::Regular,
                resume_of_turn_id: None,
                submitted_by_client_id: None,
                model: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                started_at: now(),
            })
            .record_kind(),
            DurableRecord::ItemStarted(ItemStartedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                item_id: ItemId::new(),
                kind: ItemRecordKind::UserInput,
                role: RecordRole::User,
                content_parts: vec![],
                mentions: vec![],
                visibility: ItemVisibility::Visible,
                created_at: now(),
            })
            .record_kind(),
            DurableRecord::ItemContentAppended(ItemContentAppendedRecord {
                schema_version: 1,
                item_id: ItemId::new(),
                content_part_index: 0,
                offset: 0,
                content_kind: ContentAppendKind::Text,
                content: String::new(),
                byte_count: 0,
            })
            .record_kind(),
            DurableRecord::TurnCompleted(TurnCompletedRecord {
                schema_version: 1,
                terminal: TurnTerminalFields {
                    turn_id: TurnId::new(),
                    session_id: SessionId::new(),
                    status: TurnStatus::Completed,
                    usage: None,
                    workspace_change_set_id: None,
                    completed_at: now(),
                },
            })
            .record_kind(),
            DurableRecord::ItemCompleted(ItemCompletedRecord {
                schema_version: 1,
                item_id: ItemId::new(),
                turn_id: TurnId::new(),
                final_status: ItemStatus::Completed,
                content_hash: None,
                completed_at: now(),
            })
            .record_kind(),
        ];

        let mut deduped = kinds.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(kinds.len(), deduped.len(), "record kinds must be unique");
    }

    #[test]
    fn item_status_serde() {
        let statuses = [
            ItemStatus::Completed,
            ItemStatus::Failed,
            ItemStatus::Interrupted,
            ItemStatus::Denied,
            ItemStatus::Blocked,
            ItemStatus::Canceled,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: ItemStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }

    // ── ExecutionPhase state machine tests ──

    #[test]
    fn admitted_to_assembling_context_is_legal() {
        assert!(
            ExecutionPhase::Admitted
                .can_transition_to(ExecutionPhase::AssemblingContext)
                .is_ok()
        );
    }

    #[test]
    fn admitted_to_model_invocation_is_illegal() {
        assert!(
            ExecutionPhase::Admitted
                .can_transition_to(ExecutionPhase::ModelInvocation)
                .is_err()
        );
    }

    #[test]
    fn model_invocation_to_tool_dispatch_is_legal() {
        assert!(
            ExecutionPhase::ModelInvocation
                .can_transition_to(ExecutionPhase::ToolDispatch)
                .is_ok()
        );
    }

    #[test]
    fn tool_dispatch_back_to_model_invocation_is_legal() {
        assert!(
            ExecutionPhase::ToolDispatch
                .can_transition_to(ExecutionPhase::ModelInvocation)
                .is_ok()
        );
    }

    #[test]
    fn finalizing_to_completed_is_legal() {
        assert!(
            ExecutionPhase::Finalizing
                .can_transition_to(ExecutionPhase::Completed)
                .is_ok()
        );
    }

    #[test]
    fn completed_is_terminal() {
        assert!(ExecutionPhase::Completed.is_terminal());
        assert!(ExecutionPhase::Failed.is_terminal());
        assert!(!ExecutionPhase::Admitted.is_terminal());
        assert!(!ExecutionPhase::ModelInvocation.is_terminal());
    }

    #[test]
    fn completed_cannot_transition() {
        assert!(
            ExecutionPhase::Completed
                .can_transition_to(ExecutionPhase::Admitted)
                .is_err()
        );
        assert!(
            ExecutionPhase::Failed
                .can_transition_to(ExecutionPhase::Completed)
                .is_err()
        );
    }

    #[test]
    fn interrupt_from_any_non_terminal_phase() {
        for phase in &[
            ExecutionPhase::Admitted,
            ExecutionPhase::AssemblingContext,
            ExecutionPhase::ModelInvocation,
            ExecutionPhase::ToolDispatch,
            ExecutionPhase::WaitingApproval,
            ExecutionPhase::Finalizing,
        ] {
            assert!(
                phase.can_transition_to(ExecutionPhase::Interrupted).is_ok(),
                "interrupt should be legal from {phase:?}"
            );
        }
    }

    #[test]
    fn execution_phase_serde_roundtrip() {
        let phases = [
            ExecutionPhase::Admitted,
            ExecutionPhase::AssemblingContext,
            ExecutionPhase::ModelInvocation,
            ExecutionPhase::ToolDispatch,
            ExecutionPhase::WaitingApproval,
            ExecutionPhase::Finalizing,
            ExecutionPhase::Completed,
            ExecutionPhase::Failed,
            ExecutionPhase::Interrupted,
        ];
        for phase in &phases {
            let json = serde_json::to_string(phase).expect("serialize");
            let restored: ExecutionPhase = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *phase);
        }
    }

    // ── ContentPart & Mention ─────────────────────────────────────

    #[test]
    fn content_part_all_variants_roundtrip() {
        let parts = vec![
            ContentPart::Text("hello".into()),
            ContentPart::ImageRef {
                artifact_id: "img-1".into(),
            },
            ContentPart::FileRef {
                path: "src/main.rs".into(),
                artifact_id: None,
            },
            ContentPart::ToolCallJson(serde_json::json!({"name": "read"})),
            ContentPart::ToolResultText("file content".into()),
            ContentPart::ProviderMetadata(serde_json::json!({"model": "opus"})),
        ];
        for part in &parts {
            let json = serde_json::to_string(part).expect("serialize");
            let restored: ContentPart = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *part);
        }
    }

    #[test]
    fn mention_all_kinds_and_statuses_roundtrip() {
        let mention = Mention {
            mention_id: "m1".into(),
            kind: MentionKind::File,
            display_text: "src/main.rs".into(),
            target: "src/main.rs".into(),
            source_range: Some(SourceRange { start: 0, end: 12 }),
            resolution_status: MentionResolutionStatus::Resolved,
            visibility: MentionVisibility::Visible,
        };
        let json = serde_json::to_string(&mention).expect("serialize");
        let restored: Mention = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.mention_id, "m1");
        assert_eq!(restored.kind, MentionKind::File);
        assert_eq!(
            restored.resolution_status,
            MentionResolutionStatus::Resolved
        );
    }

    #[test]
    fn mention_unresolved_is_preserved() {
        let mention = Mention {
            mention_id: "m2".into(),
            kind: MentionKind::Skill,
            display_text: "unknown-skill".into(),
            target: "unknown-skill".into(),
            source_range: None,
            resolution_status: MentionResolutionStatus::Unresolved,
            visibility: MentionVisibility::Visible,
        };
        let json = serde_json::to_string(&mention).expect("serialize");
        let restored: Mention = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            restored.resolution_status,
            MentionResolutionStatus::Unresolved
        );
    }

    // ── New DurableRecord variants ────────────────────────────────

    #[test]
    fn plan_created_roundtrip() {
        let record = DurableRecord::PlanCreated(PlanCreatedRecord {
            schema_version: 1,
            plan_id: PlanId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            objective: "Implement feature X".into(),
            items: vec![PlanItemRecord {
                plan_item_id: "1".into(),
                text: "Write tests".into(),
                status: PlanItemStatus::Pending,
                details: None,
                parent_item_id: None,
                parallel_group_id: None,
                source_turn_id: TurnId::new(),
                updated_at: now(),
            }],
            created_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.record_kind(), restored.record_kind());
        assert_eq!(restored.record_kind(), "plan_created");
    }

    #[test]
    fn goal_created_roundtrip() {
        let record = DurableRecord::GoalCreated(GoalCreatedRecord {
            schema_version: 1,
            goal_id: GoalId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            prompt: "Refactor the auth module".into(),
            description: Some("Make it more testable".into()),
            max_iterations: Some(10),
            budget: Some(GoalBudget {
                max_turns: Some(5),
                max_tokens: Some(100000),
                max_duration_seconds: None,
            }),
            created_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "goal_created");
    }

    #[test]
    fn subagent_spawned_roundtrip() {
        let record = DurableRecord::SubagentSpawned(SubagentSpawnedRecord {
            schema_version: 1,
            parent_session_id: SessionId::new(),
            child_session_id: SessionId::new(),
            agent_nickname: "code-reviewer".into(),
            agent_role: "reviewer".into(),
            agent_path: Some("builtin".into()),
            spawned_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "subagent_spawned");
    }

    #[test]
    fn context_compaction_records_roundtrip() {
        let started = DurableRecord::ContextCompactionStarted(ContextCompactionStartedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            compaction_id: "comp-1".into(),
            pre_compaction_context_size: 100000,
            threshold_ratio: 0.8,
            started_at: now(),
        });
        let json = serde_json::to_string(&started).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "context_compaction_started");

        let completed =
            DurableRecord::ContextCompactionCompleted(ContextCompactionCompletedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                compaction_id: "comp-1".into(),
                summary_item_id: ItemId::new(),
                post_compaction_context_size: 30000,
                preserved_item_ids: vec![ItemId::new(), ItemId::new()],
                completed_at: now(),
            });
        let json = serde_json::to_string(&completed).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "context_compaction_completed");
    }

    #[test]
    fn usage_recorded_with_metrics_roundtrip() {
        let record = DurableRecord::UsageRecorded(UsageRecordedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            invocation_id: InvocationId::new(),
            model_binding_id: ModelBindingId::new(),
            canonical_model_slug: "claude-sonnet-4-6".into(),
            provider_id: ProviderId::new(),
            invocation_method: InvocationMethod::AnthropicMessages,
            reasoning_effort: Some(devo_protocol::ReasoningEffort::High),
            metrics: vec![
                UsageMetric {
                    metric_kind: UsageMetricKind::InputTokens,
                    value: 5000,
                    source: MetricSource::ProviderReported,
                    confidence: MetricConfidence::High,
                    inclusion: MetricInclusion::Included,
                },
                UsageMetric {
                    metric_kind: UsageMetricKind::OutputTokens,
                    value: 800,
                    source: MetricSource::ProviderReported,
                    confidence: MetricConfidence::High,
                    inclusion: MetricInclusion::Included,
                },
            ],
            context_pressure: ContextPressure {
                context_size: 5000,
                effective_limit: 200000,
                pressure_state: ContextPressureState::Normal,
                compaction_status: CompactionStatus::NotNeeded,
            },
            recorded_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "usage_recorded");
    }

    #[test]
    fn message_edit_recorded_roundtrip() {
        let record = DurableRecord::MessageEditRecorded(MessageEditRecordedRecord {
            schema_version: 1,
            session_id: SessionId::new(),
            edit_id: EditId::new(),
            target_message_id: ItemId::new(),
            replacement_message_id: ItemId::new(),
            target_turn_id: Some(TurnId::new()),
            replacement_turn_id: None,
            queue_item_id: None,
            edited_content_parts: vec![ContentPart::Text("fixed message".into())],
            edited_mentions: vec![],
            workspace_restore_policy: WorkspaceRestorePolicy::Safe,
            edit_state: EditState::Accepted,
            requested_by_client_id: Some("tui-1".into()),
            created_at: now(),
        });
        let json = serde_json::to_string(&record).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "message_edit_recorded");
    }

    #[test]
    fn workspace_restore_records_roundtrip() {
        let restore_id = RestoreId::new();
        let started =
            DurableRecord::TurnWorkspaceRestoreStarted(TurnWorkspaceRestoreStartedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                restore_id,
                candidate_files: vec!["src/main.rs".into()],
                policy: WorkspaceRestorePolicy::Safe,
                started_at: now(),
            });
        let json = serde_json::to_string(&started).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "turn_workspace_restore_started");

        let completed =
            DurableRecord::TurnWorkspaceRestoreCompleted(TurnWorkspaceRestoreCompletedRecord {
                schema_version: 1,
                session_id: SessionId::new(),
                restore_id,
                outcomes: vec![FileRestoreOutcome {
                    file_path: "src/main.rs".into(),
                    status: RestoreFileStatus::Restored,
                }],
                completed_at: now(),
            });
        let json = serde_json::to_string(&completed).expect("serialize");
        let restored: DurableRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.record_kind(), "turn_workspace_restore_completed");
    }

    // ── WorkspaceChangeSet & FileChange ───────────────────────────

    #[test]
    fn workspace_change_set_roundtrip() {
        let cs = WorkspaceChangeSet {
            change_set_id: "cs-1".into(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            checkpoint_id: Some("cp-1".into()),
            structured_tool_coverage: ChangeSetCoverage::Full,
            shell_change_coverage: ChangeSetCoverage::None,
            file_change_refs: vec!["fc-1".into()],
            display_diff_ref: Some("diff-1".into()),
            restore_data_ref: None,
            change_set_status: ChangeSetStatus::Finalized,
        };
        let json = serde_json::to_string(&cs).expect("serialize");
        let restored: WorkspaceChangeSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.change_set_id, "cs-1");
        assert_eq!(restored.change_set_status, ChangeSetStatus::Finalized);
    }

    #[test]
    fn file_change_all_kinds_roundtrip() {
        for kind in &[
            FileChangeKind::Create,
            FileChangeKind::Modify,
            FileChangeKind::Delete,
            FileChangeKind::Rename,
            FileChangeKind::ModeChange,
        ] {
            let fc = FileChange {
                file_change_id: "fc-1".into(),
                turn_id: TurnId::new(),
                tool_call_id: "call-1".into(),
                tool_name: "write".into(),
                path: "src/lib.rs".into(),
                change_kind: *kind,
                pre_state_ref: None,
                pre_state_hash: None,
                post_state_ref: Some("ref-1".into()),
                post_state_hash: Some("abc123".into()),
                inverse_ref: None,
                display_diff_hunk_ref: Some("diff-1".into()),
                attribution_confidence: AttributionConfidence::Exact,
            };
            let json = serde_json::to_string(&fc).expect("serialize");
            let restored: FileChange = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored.change_kind, *kind);
        }
    }

    // ── ForkOrigin & InheritedHistory ─────────────────────────────

    #[test]
    fn fork_origin_roundtrip() {
        let origin = ForkOrigin {
            parent_session_id: SessionId::new(),
            fork_turn_id: TurnId::new(),
            fork_created_at: now(),
            parent_display_label: "Main session".into(),
            fork_turn_display_label: "Turn 3".into(),
            fork_turn_digest: "Added tests for auth".into(),
            origin_snapshot_hash: "abc123def".into(),
            parent_availability: ParentAvailability::Available,
        };
        let json = serde_json::to_string(&origin).expect("serialize");
        let restored: ForkOrigin = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.parent_display_label, "Main session");
        assert_eq!(restored.parent_availability, ParentAvailability::Available);
    }

    #[test]
    fn inherited_history_segment_roundtrip() {
        let segment = InheritedHistorySegmentDescriptor {
            inherited_segment_id: "seg-1".into(),
            source_parent_session_id: SessionId::new(),
            source_range: SegmentSourceRange {
                start_offset: 0,
                end_offset: 5000,
            },
            storage_strategy: StorageStrategy::ProtectedSharedSegment,
            record_refs: vec![RecordRef {
                source_session_id: SessionId::new(),
                record_sequence: 1,
                record_offset: 100,
                record_kind: "turn_started".into(),
                record_hash: "hash123".into(),
                materialized_ref: None,
            }],
            segment_hash: "seg-hash".into(),
            availability_state: SegmentAvailability::Available,
            created_at: now(),
        };
        let json = serde_json::to_string(&segment).expect("serialize");
        let restored: InheritedHistorySegmentDescriptor =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.inherited_segment_id, "seg-1");
        assert_eq!(
            restored.storage_strategy,
            StorageStrategy::ProtectedSharedSegment
        );
    }

    // ── Enum serde coverage ───────────────────────────────────────

    #[test]
    fn all_new_enums_serde_roundtrip() {
        // Usage metric enums
        for (src, conf, incl) in [
            (
                MetricSource::ProviderReported,
                MetricConfidence::High,
                MetricInclusion::Included,
            ),
            (
                MetricSource::LocallyEstimated,
                MetricConfidence::Medium,
                MetricInclusion::Excluded,
            ),
            (
                MetricSource::Unavailable,
                MetricConfidence::Unknown,
                MetricInclusion::Unknown,
            ),
        ] {
            let m = UsageMetric {
                metric_kind: UsageMetricKind::InputTokens,
                value: 100,
                source: src,
                confidence: conf,
                inclusion: incl,
            };
            let json = serde_json::to_string(&m).expect("serialize");
            let restored: UsageMetric = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored.source, src);
        }

        // Goal statuses
        for status in &[
            GoalStatus::Active,
            GoalStatus::Paused,
            GoalStatus::Completed,
            GoalStatus::Failed,
            GoalStatus::Blocked,
            GoalStatus::Canceled,
            GoalStatus::Cleared,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: GoalStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }

        // Subagent statuses
        for status in &[
            SubagentRunStatus::Spawning,
            SubagentRunStatus::Running,
            SubagentRunStatus::WaitingForInput,
            SubagentRunStatus::Completed,
            SubagentRunStatus::Failed,
            SubagentRunStatus::Interrupted,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: SubagentRunStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }

        // Background process statuses
        for status in &[
            BackgroundProcessStatus::Running,
            BackgroundProcessStatus::Completed,
            BackgroundProcessStatus::Failed,
            BackgroundProcessStatus::Stopped,
            BackgroundProcessStatus::Detached,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: BackgroundProcessStatus =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }
}
