pub mod event_projection;
pub mod history;
pub mod legacy_projector;
pub mod rollout_v2;
pub mod v2_inverse;

mod records;

pub use devo_protocol::{ItemId, SessionId, SessionTitleState, TurnId, TurnStatus, TurnUsage};
pub use event_projection::{
    DerivedEvent, EVENT_SCHEMA_VERSION, events_from_v2_line, session_stream_id, sessions_stream_id,
    source_fact_id,
};
pub use history::{CanonicalHistory, HistoryReadError, read_canonical_history};
pub use legacy_projector::{LegacyProjectError, LegacyProjector, canonical_turn_from_record};
pub use records::{
    ApprovalDecisionItem, ApprovalRequestItem, CommandExecutionItem, CompactionSnapshotLine,
    ItemLine, ItemRecord, MessageEditRecordedLine, RolloutLine, SessionContextUpdatedLine,
    SessionMetaLine, SessionRecord, SessionRollbackLine, SessionTitleUpdatedLine, TextItem,
    ToolCallItem, ToolProgressItem, ToolResultItem, TurnError, TurnItem, TurnLine, TurnRecord,
    TurnSupersededLine, TurnWorkspaceChangeRecordedLine, TurnWorkspaceCheckpointRecordedLine,
    TurnWorkspaceRestoreCompletedLine, TurnWorkspaceRestoreStartedLine, Worklog,
};
pub use rollout_v2::{
    InternalRecordV2, ParsedRolloutLine, ROLLOUT_FORMAT_VERSION, RolloutLineReadError,
    RolloutLineV2, SessionPersistenceExtras, TurnPersistenceExtras, parse_rollout_line,
};
pub use v2_inverse::{V2InverseError, V2InverseProjector};
