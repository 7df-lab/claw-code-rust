//! Stateful pure converter from the frozen legacy rollout format (v1) to the
//! canonical v2 line stream.
//!
//! Truth source: `devo-api-design/05-migration.md` §2.2 and
//! `06-item-model.md` §4. One [`LegacyProjector`] instance converts one
//! session file: it owns the per-session `seq` counter, the approval
//! request/decision fold state, and the session cwd learned from the
//! SessionMeta line (used as the `CommandExecution` cwd fallback, which
//! legacy payloads never recorded).

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;

use devo_protocol::native::error::AgentError;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalScope, ApprovalTarget, CompactionTrigger,
    ContextUsage, ExecOrigin, ExecutionMode, InternalEntry, Item, ItemEnvelope, ItemState,
    PlanEntry, PlanStepStatus, ToolSource, UserInput, UserMessageEntry,
};
use devo_protocol::native::model::{ModelBinding, PermissionProfile};
use devo_protocol::native::session::{
    GitInfo, Session, SessionParent, SessionSettings, SessionStatus,
};
use devo_protocol::native::turn::{Turn, TurnKind, TurnStatus};
use devo_protocol::native::usage::{SessionUsage, TurnUsage as CanonicalTurnUsage, UsageTotals};
use uuid::Uuid;

use crate::TurnKind as LegacyTurnKind;
use crate::conversation::{
    ApprovalRequestItem, ItemLine, ItemRecord, RolloutLine, SessionMetaLine, TurnItem, TurnLine,
    TurnRecord, TurnStatus as LegacyTurnStatus,
};

use super::rollout_v2::{
    InternalRecordV2, ROLLOUT_FORMAT_VERSION, RolloutLineV2, SessionPersistenceExtras,
    TurnPersistenceExtras,
};

/// Errors from projecting a legacy rollout line. Every known legacy shape
/// projects successfully; this exists so genuinely unrecoverable data fails
/// loudly instead of being silently fabricated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyProjectError {
    /// A legacy identifier did not parse back into the UUID it wraps.
    #[error("legacy identifier is not a valid UUID: {0}")]
    InvalidLegacyId(String),
}

/// The state carried between the request and the decision of one approval.
#[derive(Debug)]
struct ApprovalFold {
    item_id: ItemId,
    seq: u64,
    revision: u32,
    /// The full original request payload, needed to reconstruct the complete
    /// `Item::Approval` when the matching decision arrives.
    request: ApprovalRequestItem,
}

/// Intermediate result of projecting one packed legacy payload: either a
/// normal item (fresh seq assigned by the caller), an approval-fold item
/// (id/seq/revision already fixed by the fold), or a non-item internal
/// record.
#[derive(Debug)]
enum Projected {
    Item {
        item: Item,
        state: ItemState,
    },
    FoldedItem {
        id: ItemId,
        seq: u64,
        revision: u32,
        item: Item,
        state: ItemState,
    },
    Internal(Box<InternalRecordV2>),
}

/// Pure legacy (v1) → canonical (v2) rollout converter. One instance per
/// session file being converted; not shared across sessions because `seq`,
/// approval folds, and the cwd fallback are all per-session.
#[derive(Debug)]
pub struct LegacyProjector {
    /// Next sequence number to assign on an item's first appearance. Starts
    /// at 1 and is strictly increasing within the session.
    next_seq: u64,
    /// Next epoch to assign to a field-level session settings line
    /// (L2-DES-CONV-002 DD-4). Hydrated from existing v2 lines and strictly
    /// increasing within the session.
    next_settings_epoch: u64,
    /// Session cwd learned from SessionMeta; the fallback `CommandExecution`
    /// cwd because legacy exec payloads never recorded one.
    session_cwd: Option<PathBuf>,
    /// Approval requests seen so far, keyed by `approval_id`, so a later
    /// decision folds into the same item id/seq with a bumped revision.
    approvals: HashMap<String, ApprovalFold>,
}

impl Default for LegacyProjector {
    fn default() -> Self {
        Self::new()
    }
}

impl LegacyProjector {
    pub fn new() -> Self {
        Self {
            next_seq: 1,
            next_settings_epoch: 1,
            session_cwd: None,
            approvals: HashMap::new(),
        }
    }

    /// The epoch the next settings write will receive; `1` when no settings
    /// line has been written yet.
    pub fn next_settings_epoch(&self) -> u64 {
        self.next_settings_epoch
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Re-syncs the write-path state (seq counter, approval folds, cwd) with
    /// a v2 line that is already on disk. Stores hydrating a projector for a
    /// pre-existing file replay legacy lines through [`Self::project_line`]
    /// and feed v2 lines through this method so subsequent appends never
    /// collide with or orphan the on-disk history.
    pub fn observe_v2_line(&mut self, line: &RolloutLineV2) {
        match line {
            RolloutLineV2::SessionMeta { session, .. } => {
                self.session_cwd = Some(session.cwd.clone());
            }
            RolloutLineV2::Item { item, .. } => {
                self.next_seq = self.next_seq.max(item.seq + 1);
                if let Item::Approval {
                    approval_id,
                    action_summary,
                    justification,
                    resource,
                    available_scopes,
                    command_pattern,
                    command_prefix,
                    target,
                    decision,
                    ..
                } = &item.item
                {
                    match decision {
                        None => {
                            self.approvals.insert(
                                approval_id.clone(),
                                ApprovalFold {
                                    item_id: item.id.clone(),
                                    seq: item.seq,
                                    revision: item.revision,
                                    request: approval_request_from_parts(
                                        approval_id,
                                        action_summary,
                                        justification,
                                        resource,
                                        available_scopes,
                                        command_pattern,
                                        command_prefix,
                                        target,
                                    ),
                                },
                            );
                        }
                        Some(_) => {
                            if let Some(fold) = self.approvals.get_mut(approval_id) {
                                fold.revision = fold.revision.max(item.revision);
                            }
                        }
                    }
                }
            }
            RolloutLineV2::Internal { seq, entry, .. } => {
                self.next_seq = self.next_seq.max(seq + 1);
                if let InternalRecordV2::SessionSettings { epoch, .. } = entry {
                    self.next_settings_epoch = self.next_settings_epoch.max(epoch + 1);
                }
            }
            RolloutLineV2::Turn { .. }
            | RolloutLineV2::SessionTitleUpdated { .. }
            | RolloutLineV2::CompactionSnapshot { .. }
            | RolloutLineV2::SessionRollback { .. }
            | RolloutLineV2::WorkspaceCheckpoint { .. }
            | RolloutLineV2::WorkspaceChange { .. }
            | RolloutLineV2::WorkspaceRestoreStarted { .. }
            | RolloutLineV2::WorkspaceRestoreCompleted { .. } => {}
        }
    }

    /// Projects one legacy rollout line into zero or more v2 lines. Most
    /// lines map 1:1; an `Item` record expands to one line per packed
    /// payload, and internal payloads (HookPrompt/TurnSummary/ToolProgress)
    /// become `Internal` lines instead of items.
    pub fn project_line(
        &mut self,
        line: &RolloutLine,
    ) -> Result<Vec<RolloutLineV2>, LegacyProjectError> {
        match line {
            RolloutLine::SessionMeta(line) => self.project_session_meta(line),
            RolloutLine::Turn(line) => self.project_turn(line),
            RolloutLine::Item(line) => self.project_item(line),
            RolloutLine::SessionTitleUpdated(line) => {
                Ok(vec![RolloutLineV2::SessionTitleUpdated {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    session_id: SessionId::from_legacy_uuid(legacy_uuid(line.session_id)?),
                    title: line.title.clone(),
                    previous_title: line.previous_title.clone(),
                }])
            }
            RolloutLine::SessionContextUpdated(line) => Ok(vec![RolloutLineV2::Internal {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: line.timestamp,
                session_id: SessionId::from_legacy_uuid(legacy_uuid(line.session_id)?),
                turn_id: None,
                seq: 0,
                entry: InternalRecordV2::SessionContext(Box::new(line.session_context.clone())),
            }]),
            RolloutLine::SessionSettings(line) => {
                // The projector is the per-file authority for settings epochs:
                // callers pass a placeholder and the assigned epoch is what
                // lands on disk.
                let epoch = self.next_settings_epoch;
                self.next_settings_epoch = self.next_settings_epoch.saturating_add(1);
                Ok(vec![RolloutLineV2::Internal {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    session_id: SessionId::from_legacy_uuid(legacy_uuid(line.session_id)?),
                    turn_id: None,
                    seq: 0,
                    entry: InternalRecordV2::SessionSettings {
                        schema_version: 1,
                        field: line.field,
                        value: line.value.clone(),
                        epoch,
                    },
                }])
            }
            RolloutLine::CompactionSnapshot(line) => Ok(vec![RolloutLineV2::CompactionSnapshot {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: line.timestamp,
                session_id: SessionId::from_legacy_uuid(legacy_uuid(line.session_id)?),
                turn_id: TurnId::from_legacy_uuid(legacy_uuid(line.turn_id)?),
                summary_item_id: ItemId::from_legacy_uuid(legacy_uuid(line.summary_item_id)?),
                preserved_item_ids: line
                    .preserved_item_ids
                    .iter()
                    .map(|id| legacy_uuid(id).map(ItemId::from_legacy_uuid))
                    .collect::<Result<_, _>>()?,
                context_occupancy: line.context_occupancy.clone(),
            }]),
            RolloutLine::MessageEditRecorded(line) => {
                let record = &line.record;
                Ok(vec![RolloutLineV2::Internal {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    session_id: SessionId::from_legacy_uuid(legacy_uuid(record.session_id)?),
                    turn_id: record
                        .replacement_turn_id
                        .or(record.target_turn_id)
                        .map(|id| legacy_uuid(id).map(TurnId::from_legacy_uuid))
                        .transpose()?,
                    seq: 0,
                    entry: InternalRecordV2::MessageEdit(line.record.clone()),
                }])
            }
            RolloutLine::TurnSuperseded(line) => {
                let record = &line.record;
                Ok(vec![RolloutLineV2::Internal {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    session_id: SessionId::from_legacy_uuid(legacy_uuid(record.session_id)?),
                    turn_id: Some(TurnId::from_legacy_uuid(legacy_uuid(
                        record.replacement_turn_id,
                    )?)),
                    seq: 0,
                    entry: InternalRecordV2::TurnSuperseded(line.record.clone()),
                }])
            }
            RolloutLine::TurnWorkspaceCheckpointRecorded(line) => {
                Ok(vec![RolloutLineV2::WorkspaceCheckpoint {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    record: line.record.clone(),
                }])
            }
            RolloutLine::TurnWorkspaceChangeRecorded(line) => {
                Ok(vec![RolloutLineV2::WorkspaceChange {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    record: line.record.clone(),
                }])
            }
            RolloutLine::TurnWorkspaceRestoreStarted(line) => {
                Ok(vec![RolloutLineV2::WorkspaceRestoreStarted {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    record: line.record.clone(),
                }])
            }
            RolloutLine::TurnWorkspaceRestoreCompleted(line) => {
                Ok(vec![RolloutLineV2::WorkspaceRestoreCompleted {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: line.timestamp,
                    record: line.record.clone(),
                }])
            }
            RolloutLine::SessionRollback(line) => Ok(vec![RolloutLineV2::SessionRollback {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: line.timestamp,
                session_id: SessionId::from_legacy_uuid(legacy_uuid(line.session_id)?),
                retained_turn_ids: line
                    .retained_turn_ids
                    .iter()
                    .map(|id| legacy_uuid(id).map(TurnId::from_legacy_uuid))
                    .collect::<Result<_, _>>()?,
                retained_item_ids: line
                    .retained_item_ids
                    .iter()
                    .map(|id| legacy_uuid(id).map(ItemId::from_legacy_uuid))
                    .collect::<Result<_, _>>()?,
                latest_turn_id: line
                    .latest_turn_id
                    .map(|id| legacy_uuid(id).map(TurnId::from_legacy_uuid))
                    .transpose()?,
            }]),
        }
    }

    fn project_session_meta(
        &mut self,
        line: &SessionMetaLine,
    ) -> Result<Vec<RolloutLineV2>, LegacyProjectError> {
        let record = &line.session;
        self.session_cwd = Some(record.cwd.clone());

        let parent = match record.parent_session_id {
            Some(parent_id)
                if record.agent_role.is_some()
                    || record.agent_nickname.is_some()
                    || record.agent_path.is_some() =>
            {
                Some(SessionParent::Agent {
                    session_id: SessionId::from_legacy_uuid(legacy_uuid(parent_id)?),
                    role: record.agent_role.clone(),
                })
            }
            Some(parent_id) => Some(SessionParent::Fork {
                session_id: SessionId::from_legacy_uuid(legacy_uuid(parent_id)?),
                at_turn_id: None,
            }),
            None => None,
        };

        // Legacy approval modes were free-form strings ("on-request",
        // "full-auto", ...); map by keyword, defaulting to the safest profile.
        let approval_mode = record.approval_mode.to_ascii_lowercase();
        let permission_profile = if approval_mode.contains("auto") {
            PermissionProfile::AutoReview
        } else if approval_mode.contains("full") {
            PermissionProfile::FullAccess
        } else {
            PermissionProfile::Default
        };

        let git_info = if record.git_sha.is_some()
            || record.git_branch.is_some()
            || record.git_origin_url.is_some()
        {
            Some(GitInfo {
                sha: record.git_sha.clone(),
                branch: record.git_branch.clone(),
                origin_url: record.git_origin_url.clone(),
                // Legacy never recorded the dirty flag; None = unknown.
                dirty: None,
                observed_at: record.updated_at,
            })
        } else {
            None
        };

        // The legacy lump cannot be decomposed into calls: it lands in
        // `legacy` and in `total.total_tokens` (total = legacy + ledger, and
        // the ledger starts empty).
        let legacy_totals = UsageTotals {
            total_tokens: record.tokens_used.max(0) as u64,
            ..UsageTotals::default()
        };

        let session = Session {
            id: SessionId::from_legacy_uuid(legacy_uuid(record.id)?),
            version: 1,
            cwd: record.cwd.clone(),
            additional_directories: record.additional_directories.clone(),
            parent,
            ephemeral: false,
            created_at: record.created_at,
            status: SessionStatus::Idle,
            flags: Vec::new(),
            archived: record.archived_at.is_some(),
            active_turn_id: None,
            queued_count: 0,
            title: record.title.clone(),
            model: ModelBinding {
                provider: record.model_provider.clone(),
                // Sessions that never recorded a resolved model keep an
                // explicitly empty slug: unknown, not fabricated.
                model: record.model.clone().unwrap_or_default(),
                reasoning_effort: record
                    .reasoning_effort_selection
                    .as_deref()
                    .and_then(|selection| selection.parse().ok()),
            },
            settings: SessionSettings {
                permission_profile,
                reasoning_effort: None,
                mode: None,
                sandbox_profile: (!record.sandbox_policy.is_empty())
                    .then(|| record.sandbox_policy.clone()),
                effective_context_window: record.effective_context_window,
            },
            git_info,
            preview: record.first_user_message.clone().unwrap_or_default(),
            last_activity_at: record.last_activity_at.unwrap_or(record.updated_at),
            transcript_size_bytes: None,
            usage: SessionUsage {
                total: legacy_totals.clone(),
                by_purpose: Vec::new(),
                legacy: Some(legacy_totals),
                updated_at: record.updated_at,
            },
        };
        Ok(vec![RolloutLineV2::SessionMeta {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: line.timestamp,
            session: Box::new(session),
            extras: Some(Box::new(SessionPersistenceExtras {
                session_context: record.session_context.clone(),
                cli_version: record.cli_version.clone(),
                source: record.source.clone(),
                collaboration_mode: record.collaboration_mode,
                permission_preset: record.permission_preset,
            })),
        }])
    }

    fn project_turn(&mut self, line: &TurnLine) -> Result<Vec<RolloutLineV2>, LegacyProjectError> {
        let record = &line.turn;
        Ok(vec![RolloutLineV2::Turn {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: line.timestamp,
            turn: canonical_turn_from_record(record)?,
            extras: Some(Box::new(TurnPersistenceExtras {
                session_context: record.session_context.clone(),
                turn_context: record.turn_context.clone(),
                request_thinking: record.request_thinking.clone(),
                input_token_estimate: record.input_token_estimate,
                latest_query_usage: record.latest_query_usage.clone(),
                context_occupancy: record.context_occupancy.clone(),
                stop_reason: record.stop_reason.clone(),
                failure_reason: record.failure_reason,
            })),
        }])
    }

    fn project_item(&mut self, line: &ItemLine) -> Result<Vec<RolloutLineV2>, LegacyProjectError> {
        let record = &line.item;
        let session_id = SessionId::from_legacy_uuid(legacy_uuid(record.session_id)?);
        let turn_id = TurnId::from_legacy_uuid(legacy_uuid(record.turn_id)?);
        let first_item_id = ItemId::from_legacy_uuid(legacy_uuid(record.id)?);

        let mut out = Vec::new();
        for (index, payload) in record
            .input_items
            .iter()
            .chain(&record.output_items)
            .enumerate()
        {
            // A legacy record packs N payloads under a single record id; the
            // first payload keeps that id, the rest get fresh canonical ids
            // because persistence is one-record-one-item in v2. Fresh ids are
            // bare UUIDs (not prefixed) so they still round-trip into legacy
            // UUID newtypes via the inverse projector; prefixed ids only
            // appear once the runtime natively creates canonical resources,
            // at which point the legacy replay path is gone.
            let item_id = if index == 0 {
                first_item_id.clone()
            } else {
                ItemId::from_legacy_uuid(Uuid::now_v7())
            };
            let (id, seq, revision, state, item) =
                match self.project_payload(record, &item_id, payload)? {
                    Projected::Item { item, state } => (item_id, self.next_seq(), 1, state, item),
                    Projected::FoldedItem {
                        id,
                        seq,
                        revision,
                        item,
                        state,
                    } => (id, seq, revision, state, item),
                    Projected::Internal(entry) => {
                        // Internal entries consume one sequence position, shared
                        // with the item stream, so their order among items is
                        // exactly recoverable by the inverse projector.
                        out.push(RolloutLineV2::Internal {
                            v: ROLLOUT_FORMAT_VERSION,
                            timestamp: line.timestamp,
                            session_id: session_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            seq: self.next_seq(),
                            entry: *entry,
                        });
                        continue;
                    }
                };
            out.push(RolloutLineV2::Item {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: line.timestamp,
                item: ItemEnvelope {
                    id,
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    seq,
                    revision,
                    created_at: record.timestamp,
                    updated_at: record.timestamp,
                    state,
                    item,
                },
            });
        }
        Ok(out)
    }

    fn project_payload(
        &mut self,
        record: &ItemRecord,
        item_id: &ItemId,
        payload: &TurnItem,
    ) -> Result<Projected, LegacyProjectError> {
        let projected = match payload {
            TurnItem::UserMessage(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: item.text.clone(),
                    }],
                    entry: UserMessageEntry::TurnStart,
                },
            },
            TurnItem::SteerInput(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: item.text.clone(),
                    }],
                    entry: UserMessageEntry::Steer,
                },
            },
            TurnItem::HookPrompt(item) => Projected::Internal(Box::new(InternalRecordV2::Entry {
                entry: InternalEntry::HookPrompt {
                    text: item.text.clone(),
                },
            })),
            TurnItem::AgentMessage(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::AssistantMessage {
                    text: item.text.clone(),
                    phase: None,
                },
            },
            TurnItem::Plan(item) => Projected::Item {
                state: ItemState::Completed,
                // The legacy plan is a plain rendered text blob; one entry
                // preserves it verbatim. Cold files are almost always finished
                // turns, so the step is marked completed.
                item: Item::Plan {
                    entries: vec![PlanEntry {
                        step: item.text.clone(),
                        status: PlanStepStatus::Completed,
                    }],
                },
            },
            TurnItem::Reasoning(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::Reasoning {
                    text: item.text.clone(),
                    provider_payload_ref: None,
                },
            },
            TurnItem::ToolCall(call) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ToolCall {
                    call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    // Legacy persisted calls all went through the builtin
                    // dispatcher.
                    source: ToolSource::Builtin,
                    server_name: None,
                    input: Some(call.input.clone()),
                },
            },
            TurnItem::ToolProgress(progress) => {
                Projected::Internal(Box::new(InternalRecordV2::Entry {
                    entry: InternalEntry::ToolProgress {
                        call_id: progress.tool_call_id.clone(),
                        message: progress.message.clone(),
                    },
                }))
            }
            TurnItem::ToolResult(result) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ToolResult {
                    call_id: result.tool_call_id.clone(),
                    output: result.output.clone(),
                    display_content: result.display_content.clone(),
                    is_error: result.is_error,
                    truncated: false,
                },
            },
            TurnItem::CommandExecution(command) => Projected::Item {
                state: ItemState::Completed,
                item: Item::CommandExecution {
                    call_id: command.tool_call_id.clone(),
                    command: command.command.clone(),
                    argv: None,
                    // Legacy exec payloads never recorded a cwd; fall back to
                    // the session cwd, or an explicitly empty path when the
                    // SessionMeta line has not been seen yet.
                    cwd: self.session_cwd.clone().unwrap_or_default(),
                    input: Some(command.input.clone()),
                    output: Some(command.output.clone()),
                    exit_code: None,
                    execution_handle: None,
                    is_error: command.is_error,
                    execution_mode: ExecutionMode::Foreground,
                    origin: ExecOrigin::AgentTool,
                    sandbox: None,
                },
            },
            TurnItem::WebSearch(item) => Projected::Item {
                state: ItemState::Completed,
                // Legacy hosted-tool payloads only kept their rendered text;
                // no call id was recorded, so the envelope item id stands in
                // as a stable identifier.
                item: Item::HostedToolCall {
                    call_id: item_id.as_str().to_owned(),
                    tool_name: "web_search".into(),
                    input: None,
                    output: Some(serde_json::Value::String(item.text.clone())),
                },
            },
            TurnItem::ImageGeneration(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::HostedToolCall {
                    call_id: item_id.as_str().to_owned(),
                    tool_name: "image_generation".into(),
                    input: None,
                    output: Some(serde_json::Value::String(item.text.clone())),
                },
            },
            TurnItem::ContextCompaction(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ContextCompaction {
                    // Legacy did not record the trigger; the conservative
                    // default is the automatic threshold.
                    trigger: CompactionTrigger::AutoThreshold,
                    before: ContextUsage {
                        measured: false,
                        ..ContextUsage::default()
                    },
                    after: None,
                    summary: Some(item.text.clone()),
                },
            },
            TurnItem::TurnSummary(item) => Projected::Internal(Box::new(InternalRecordV2::Entry {
                entry: InternalEntry::TurnSummary {
                    text: item.text.clone(),
                },
            })),
            TurnItem::ApprovalRequest(request) => {
                let seq = self.next_seq();
                self.approvals.insert(
                    request.approval_id.clone(),
                    ApprovalFold {
                        item_id: item_id.clone(),
                        seq,
                        revision: 1,
                        request: request.clone(),
                    },
                );
                Projected::FoldedItem {
                    id: item_id.clone(),
                    seq,
                    revision: 1,
                    state: ItemState::Waiting,
                    item: approval_request_item(request, None),
                }
            }
            TurnItem::ApprovalDecision(decision) => {
                match self.approvals.get_mut(&decision.approval_id) {
                    Some(fold) => {
                        fold.revision += 1;
                        // Legacy decisions were free-form strings; "allow"
                        // appears in historical files (records.rs tests) and
                        // anything not clearly approve/deny is cancelled.
                        let decision_kind = match decision.decision.to_ascii_lowercase().as_str() {
                            "approve" | "approved" | "allow" => ApprovalDecisionKind::Approved,
                            "deny" | "denied" => ApprovalDecisionKind::Denied,
                            _ => ApprovalDecisionKind::Cancelled,
                        };
                        // Unknown legacy scope strings fall back to the
                        // narrowest scope instead of failing the conversion.
                        let scope = match decision.scope.to_ascii_lowercase().as_str() {
                            "once" => ApprovalScope::Once,
                            "turn" => ApprovalScope::Turn,
                            "session" => ApprovalScope::Session,
                            "path_prefix" => ApprovalScope::PathPrefix,
                            "host" => ApprovalScope::Host,
                            "tool" => ApprovalScope::Tool,
                            "command_prefix" => ApprovalScope::CommandPrefix,
                            "command_prefix_persist" => ApprovalScope::CommandPrefixPersist,
                            _ => ApprovalScope::Once,
                        };
                        let request = fold.request.clone();
                        Projected::FoldedItem {
                            id: fold.item_id.clone(),
                            seq: fold.seq,
                            revision: fold.revision,
                            state: ItemState::Completed,
                            item: approval_request_item(
                                &request,
                                Some(ApprovalDecision {
                                    decision: decision_kind,
                                    scope,
                                    decision_source: decision.decision_source.unwrap_or_default(),
                                    decided_at: record.timestamp,
                                }),
                            ),
                        }
                    }
                    None => {
                        // Orphan decision (no matching request in this file):
                        // keep the information as a warning item with a fresh
                        // id/seq rather than dropping history. The fresh id is
                        // a bare UUID for the same round-trip reason as the
                        // expansion ids above.
                        Projected::FoldedItem {
                            id: ItemId::from_legacy_uuid(Uuid::now_v7()),
                            seq: self.next_seq(),
                            revision: 1,
                            state: ItemState::Completed,
                            item: Item::Warning {
                                code: "legacyOrphanApprovalDecision".into(),
                                message: format!(
                                    "approval decision '{}'/'{}' references unknown approval id {}",
                                    decision.decision, decision.scope, decision.approval_id
                                ),
                                retryable: false,
                            },
                        }
                    }
                }
            }
        };
        Ok(projected)
    }
}

/// Converts a legacy `TurnRecord` into the canonical `Turn`. This is the
/// exact mapping the rollout forward projector uses for Turn lines, shared
/// with the history read API (`session/turns/list`).
pub fn canonical_turn_from_record(record: &TurnRecord) -> Result<Turn, LegacyProjectError> {
    let kind = match &record.kind {
        // `Review` was dead code with no production data and `Other(_)`
        // was an open string; both collapse to Regular. Goal continuations
        // are never back-filled from content (05 §2.2): they stay Regular.
        LegacyTurnKind::Regular | LegacyTurnKind::Review | LegacyTurnKind::Other(_) => {
            TurnKind::Regular
        }
        LegacyTurnKind::ManualCompaction => TurnKind::Compaction,
    };

    let status = match record.status {
        // Waiting on an approval is still part of the turn, not a
        // separate state (07 §4.3).
        LegacyTurnStatus::Pending
        | LegacyTurnStatus::Running
        | LegacyTurnStatus::WaitingApproval => TurnStatus::InProgress,
        LegacyTurnStatus::Completed => TurnStatus::Completed,
        LegacyTurnStatus::Interrupted => TurnStatus::Interrupted,
        LegacyTurnStatus::Failed => TurnStatus::Failed,
    };

    let error = record.error.as_ref().map(|error| {
        let mut projected = AgentError::new(error.code.clone(), error.message.clone());
        if let Some(hint) = &error.recovery_hint {
            projected.details = Some(serde_json::json!({ "recoveryHint": hint }));
        }
        projected
    });

    let usage = record.usage.as_ref().map(|usage| CanonicalTurnUsage {
        query: UsageTotals {
            total_tokens: u64::from(
                usage
                    .total_tokens
                    .unwrap_or(usage.input_tokens + usage.output_tokens),
            ),
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            reasoning_tokens: u64::from(usage.reasoning_output_tokens.unwrap_or(0)),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens.unwrap_or(0)),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens.unwrap_or(0)),
            call_count: 0,
            // The provider reported usage, so the turn had at least one
            // metered call.
            metered_call_count: 1,
            ..UsageTotals::default()
        },
        overhead: UsageTotals::default(),
    });

    Ok(Turn {
        id: TurnId::from_legacy_uuid(legacy_uuid(record.id)?),
        session_id: SessionId::from_legacy_uuid(legacy_uuid(record.session_id)?),
        sequence: record.sequence,
        kind,
        status,
        model: ModelBinding {
            provider: record
                .model_binding_id
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            model: if record.request_model.is_empty() {
                record.model.clone()
            } else {
                record.request_model.clone()
            },
            reasoning_effort: record
                .reasoning_effort_selection
                .as_deref()
                .and_then(|selection| selection.parse().ok()),
        },
        collaboration_mode: Some(
            record
                .turn_context
                .as_ref()
                .map(|context| context.collaboration_mode)
                .unwrap_or_default(),
        ),
        started_at: record.started_at,
        completed_at: record.completed_at,
        error,
        usage,
    })
}

/// Legacy identifiers are `Display`-formatted UUIDs; converting back through
/// the string form keeps the bare-UUID textual representation so ids
/// round-trip unchanged.
fn legacy_uuid(id: impl Display) -> Result<Uuid, LegacyProjectError> {
    let text = id.to_string();
    Uuid::parse_str(&text).map_err(|_| LegacyProjectError::InvalidLegacyId(text))
}

/// Rebuilds a legacy approval request payload from the canonical approval
/// parts (the inverse of [`approval_request_item`]); used when hydrating the
/// fold map from an on-disk v2 approval envelope.
fn approval_request_from_parts(
    approval_id: &str,
    action_summary: &str,
    justification: &str,
    resource: &Option<String>,
    available_scopes: &[String],
    command_pattern: &Option<Vec<String>>,
    command_prefix: &Option<Vec<String>>,
    target: &Option<ApprovalTarget>,
) -> ApprovalRequestItem {
    let (path, host, target) = target
        .as_ref()
        .map_or((None, None, None), |target| match target {
            ApprovalTarget::Path { path } => (Some(path.display().to_string()), None, None),
            ApprovalTarget::Host { host } => (None, Some(host.clone()), None),
            ApprovalTarget::Command { command } => (None, None, Some(command.clone())),
        });
    ApprovalRequestItem {
        approval_id: approval_id.into(),
        action_summary: action_summary.into(),
        justification: justification.into(),
        resource: resource.clone(),
        available_scopes: available_scopes.into(),
        command_pattern: command_pattern.clone(),
        command_prefix: command_prefix.clone(),
        path,
        host,
        target,
    }
}

/// Builds the approval target from the legacy request's optional path, host,
/// or free-form target string, in that priority order.
fn approval_target(request: &ApprovalRequestItem) -> Option<ApprovalTarget> {
    if let Some(path) = &request.path {
        Some(ApprovalTarget::Path {
            path: PathBuf::from(path),
        })
    } else if let Some(host) = &request.host {
        Some(ApprovalTarget::Host { host: host.clone() })
    } else {
        request
            .target
            .clone()
            .map(|command| ApprovalTarget::Command { command })
    }
}

/// Reconstructs a full `Item::Approval` from a stored legacy request payload,
/// with or without the folded-in decision.
fn approval_request_item(
    request: &ApprovalRequestItem,
    decision: Option<ApprovalDecision>,
) -> Item {
    Item::Approval {
        approval_id: request.approval_id.clone(),
        target_item_id: None,
        action_summary: request.action_summary.clone(),
        justification: request.justification.clone(),
        resource: request.resource.clone(),
        available_scopes: request.available_scopes.clone(),
        command_pattern: request.command_pattern.clone(),
        command_prefix: request.command_prefix.clone(),
        target: approval_target(request),
        decision,
    }
}
