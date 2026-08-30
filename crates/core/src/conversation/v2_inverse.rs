//! Inverse projector: canonical v2 rollout lines → frozen legacy
//! `RolloutLine`s.
//!
//! Exists so rollout files written in the v2 envelope keep replaying through
//! the legacy `ReplayState` pipeline (`crates/server/src/persistence.rs`)
//! until that pipeline is retired. Companion of
//! [`crate::conversation::legacy_projector::LegacyProjector`]; the tested
//! invariant is: for every record shape the v2 writer can produce,
//! `inverse(forward(x))` replays equivalently to `x`.
//!
//! Honest-loss points are commented inline; each one is a field the
//! canonical model deliberately does not carry.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use devo_protocol::native::error::AgentError;
use devo_protocol::native::ids::{
    ItemId as CanonicalItemId, SessionId as CanonicalSessionId, TurnId as CanonicalTurnId,
};
use devo_protocol::native::item::{
    ApprovalDecisionKind, ApprovalScope, ApprovalTarget, InternalEntry, Item, ItemEnvelope,
    UserInput, UserMessageEntry,
};
use devo_protocol::native::model::PermissionProfile;
use devo_protocol::native::session::{Session, SessionParent};
use devo_protocol::native::turn::{Turn, TurnKind, TurnStatus};
use uuid::Uuid;

use crate::conversation::rollout_v2::{
    InternalRecordV2, RolloutLineV2, SessionPersistenceExtras, TurnPersistenceExtras,
};
use crate::conversation::{
    ApprovalDecisionItem, ApprovalRequestItem, CommandExecutionItem, CompactionSnapshotLine,
    ItemId, ItemLine, ItemRecord, RolloutLine, SessionContextUpdatedLine, SessionId,
    SessionMetaLine, SessionRecord, SessionRollbackLine, SessionTitleState,
    SessionTitleUpdatedLine, TextItem, ToolCallItem, ToolProgressItem, ToolResultItem, TurnError,
    TurnId, TurnItem, TurnLine, TurnRecord, TurnStatus as LegacyTurnStatus,
};
use crate::{SessionTitleFinalSource, TurnKind as LegacyTurnKind, TurnUsage};

/// Schema versions the live legacy write path stamps today
/// (`crates/server/src/persistence.rs`). The inverse writes the CURRENT
/// versions, not whatever the original record carried: replay only
/// understands the current layout, and the legacy schema is frozen.
const CURRENT_SESSION_SCHEMA_VERSION: u32 = 2;
const CURRENT_TURN_SCHEMA_VERSION: u32 = 4;
const CURRENT_ITEM_SCHEMA_VERSION: u32 = 1;
const CURRENT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Errors from projecting a v2 line back into the legacy format.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum V2InverseError {
    /// A canonical id is not a bare UUID (e.g. `item_…`-prefixed). Only
    /// reachable for resources created natively by the v2 runtime — and by
    /// then the legacy replay path this projector feeds is gone.
    #[error("canonical id is not a legacy bare UUID: {0}")]
    NonLegacyId(String),
    /// A turn-scoped internal entry arrived without its turn id (the v2
    /// writer always sets one for `Entry` records).
    #[error("internal entry line is missing its turn id")]
    MissingTurnId,
}

/// v2 → legacy line converter. Stateless: every v2 line carries the
/// identity the inverse needs (internal lines got explicit
/// session/turn/seq fields in the same change that introduced the write
/// switch).
#[derive(Debug, Default)]
pub struct V2InverseProjector;

impl V2InverseProjector {
    pub fn new() -> Self {
        Self
    }

    /// Projects one v2 line into zero or more legacy lines. Most lines map
    /// 1:1; items the legacy format cannot represent at all (`Warning` and
    /// the new-only canonical variants) produce no line — see the comment on
    /// the item mapping.
    pub fn project_line(&self, line: &RolloutLineV2) -> Result<Vec<RolloutLine>, V2InverseError> {
        match line {
            RolloutLineV2::SessionMeta {
                timestamp,
                session,
                extras,
                ..
            } => self.project_session_meta(*timestamp, session.as_ref(), extras.as_deref()),
            RolloutLineV2::Turn {
                timestamp,
                turn,
                extras,
                ..
            } => self.project_turn(*timestamp, turn, extras.as_deref()),
            RolloutLineV2::Item { item, .. } => {
                Ok(self.project_item_envelope(item)?.into_iter().collect())
            }
            RolloutLineV2::Internal {
                timestamp,
                session_id,
                turn_id,
                seq,
                entry,
                ..
            } => self.project_internal(*timestamp, session_id, turn_id.as_ref(), *seq, entry),
            RolloutLineV2::SessionTitleUpdated {
                timestamp,
                session_id,
                title,
                previous_title,
                ..
            } => Ok(vec![RolloutLine::SessionTitleUpdated(
                SessionTitleUpdatedLine {
                    timestamp: *timestamp,
                    session_id: legacy_session_id(session_id)?,
                    title: title.clone(),
                    // The title lifecycle is a derived cache in the canonical
                    // model; any Final variant is honest here because it only
                    // suppresses later regeneration of a recorded title.
                    title_state: SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate),
                    previous_title: previous_title.clone(),
                },
            )]),
            RolloutLineV2::CompactionSnapshot {
                timestamp,
                session_id,
                turn_id,
                summary_item_id,
                preserved_item_ids,
                context_occupancy,
                ..
            } => Ok(vec![RolloutLine::CompactionSnapshot(Box::new(
                CompactionSnapshotLine {
                    timestamp: *timestamp,
                    session_id: legacy_session_id(session_id)?,
                    turn_id: legacy_turn_id(turn_id)?,
                    summary_item_id: legacy_item_id(summary_item_id)?,
                    preserved_item_ids: preserved_item_ids
                        .iter()
                        .map(legacy_item_id)
                        .collect::<Result<_, _>>()?,
                    context_occupancy: context_occupancy.clone(),
                },
            ))]),
            RolloutLineV2::SessionRollback {
                timestamp,
                session_id,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
                ..
            } => Ok(vec![RolloutLine::SessionRollback(Box::new(
                SessionRollbackLine {
                    timestamp: *timestamp,
                    session_id: legacy_session_id(session_id)?,
                    retained_turn_ids: retained_turn_ids
                        .iter()
                        .map(legacy_turn_id)
                        .collect::<Result<_, _>>()?,
                    retained_item_ids: retained_item_ids
                        .iter()
                        .map(legacy_item_id)
                        .collect::<Result<_, _>>()?,
                    latest_turn_id: latest_turn_id.as_ref().map(legacy_turn_id).transpose()?,
                    schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
                },
            ))]),
            RolloutLineV2::WorkspaceCheckpoint {
                timestamp, record, ..
            } => Ok(vec![RolloutLine::TurnWorkspaceCheckpointRecorded(
                Box::new(crate::conversation::TurnWorkspaceCheckpointRecordedLine {
                    timestamp: *timestamp,
                    record: record.clone(),
                }),
            )]),
            RolloutLineV2::WorkspaceChange {
                timestamp, record, ..
            } => Ok(vec![RolloutLine::TurnWorkspaceChangeRecorded(Box::new(
                crate::conversation::TurnWorkspaceChangeRecordedLine {
                    timestamp: *timestamp,
                    record: record.clone(),
                },
            ))]),
            RolloutLineV2::WorkspaceRestoreStarted {
                timestamp, record, ..
            } => Ok(vec![RolloutLine::TurnWorkspaceRestoreStarted(Box::new(
                crate::conversation::TurnWorkspaceRestoreStartedLine {
                    timestamp: *timestamp,
                    record: record.clone(),
                },
            ))]),
            RolloutLineV2::WorkspaceRestoreCompleted {
                timestamp, record, ..
            } => Ok(vec![RolloutLine::TurnWorkspaceRestoreCompleted(Box::new(
                crate::conversation::TurnWorkspaceRestoreCompletedLine {
                    timestamp: *timestamp,
                    record: record.clone(),
                },
            ))]),
        }
    }

    fn project_session_meta(
        &self,
        timestamp: DateTime<Utc>,
        session: &Session,
        extras: Option<&SessionPersistenceExtras>,
    ) -> Result<Vec<RolloutLine>, V2InverseError> {
        let id = legacy_session_id(&session.id)?;

        let (parent_session_id, agent_role, fork_from_id, fork_at_turn_id) =
            match (&session.parent, &session.fork_from_id, &session.at_turn_id) {
                (Some(SessionParent::Agent { session_id, role }), _, _) => (
                    Some(legacy_session_id(session_id)?),
                    role.clone(),
                    None,
                    None,
                ),
                (None, Some(fork_from), at_turn) => (
                    None,
                    None,
                    Some(legacy_session_id(fork_from)?),
                    at_turn.as_ref().map(legacy_turn_id).transpose()?,
                ),
                (None, None, _) => (None, None, None, None),
            };

        // Lossy: the legacy approval mode was a free-form string
        // ("on-request", "untrusted", "never", ...); only the mapped profile
        // survives, so it maps back to the canonical spellings.
        let approval_mode = match session.settings.permission_profile {
            PermissionProfile::Default => "on-request",
            PermissionProfile::AutoReview => "auto-review",
            PermissionProfile::FullAccess => "full-access",
        };

        let (git_sha, git_branch, git_origin_url) =
            session.git_info.as_ref().map_or((None, None, None), |git| {
                (git.sha.clone(), git.branch.clone(), git.origin_url.clone())
            });

        let record = SessionRecord {
            id,
            // Unknown at this layer: replay takes the real rollout path from
            // the file it is reading.
            rollout_path: PathBuf::new(),
            created_at: session.created_at,
            // The canonical model keeps only `last_activity_at`; it stands in
            // for the metadata update time as well.
            updated_at: session.last_activity_at,
            last_activity_at: Some(session.last_activity_at),
            source: extras
                .map(|extras| extras.source.clone())
                .unwrap_or_default(),
            // Nickname/path are not modeled on canonical `SessionParent`.
            agent_nickname: None,
            agent_role,
            agent_path: None,
            model_provider: session.model.provider.clone(),
            model: (!session.model.model.is_empty()).then(|| session.model.model.clone()),
            // Not modeled canonically; only the provider string survives.
            model_binding_id: None,
            // The settings snapshot carries the raw selection literal
            // (toggle keywords included); the `ModelBinding` enum only holds
            // the request-parameter subset, so prefer settings when present.
            reasoning_effort_selection: session.settings.reasoning_effort.clone().or_else(|| {
                session
                    .model
                    .reasoning_effort
                    .map(|effort| effort.to_string())
            }),
            cwd: session.cwd.clone(),
            additional_directories: session.additional_directories.clone(),
            cli_version: extras
                .map(|extras| extras.cli_version.clone())
                .unwrap_or_default(),
            title: session.title.clone(),
            title_state: if session.title.is_some() {
                SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate)
            } else {
                SessionTitleState::Unset
            },
            sandbox_policy: session.settings.sandbox_profile.clone().unwrap_or_default(),
            approval_mode: approval_mode.into(),
            effective_context_window: session.settings.effective_context_window,
            tokens_used: session
                .usage
                .legacy
                .as_ref()
                .map_or(session.usage.total.total_tokens, |legacy| {
                    legacy.total_tokens
                }) as i64,
            first_user_message: (!session.preview.is_empty()).then(|| session.preview.clone()),
            // The exact archive time is not modeled; the creation time is
            // the only timestamp known to precede it.
            archived_at: session.archived.then_some(session.created_at),
            git_sha,
            git_branch,
            git_origin_url,
            parent_session_id,
            fork_from_id,
            fork_at_turn_id,
            session_context: extras.and_then(|extras| extras.session_context.clone()),
            // Internal prefix-cache cache, not carried even in the extras.
            latest_turn_context: None,
            collaboration_mode: extras.and_then(|extras| extras.collaboration_mode),
            permission_preset: extras.and_then(|extras| extras.permission_preset),
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
        };
        Ok(vec![RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp,
            session: record,
        }))])
    }

    fn project_turn(
        &self,
        timestamp: DateTime<Utc>,
        turn: &Turn,
        extras: Option<&TurnPersistenceExtras>,
    ) -> Result<Vec<RolloutLine>, V2InverseError> {
        let id = legacy_turn_id(&turn.id)?;

        let status = match turn.status {
            // Pending/WaitingApproval are not distinguishable after the
            // forward mapping collapsed them into InProgress; Running is the
            // honest in-flight value.
            TurnStatus::InProgress => LegacyTurnStatus::Running,
            TurnStatus::Completed => LegacyTurnStatus::Completed,
            TurnStatus::Interrupted => LegacyTurnStatus::Interrupted,
            TurnStatus::Failed => LegacyTurnStatus::Failed,
        };
        let kind = match turn.kind {
            TurnKind::Compaction => LegacyTurnKind::ManualCompaction,
            // GoalContinuation is new-only (never produced from legacy data),
            // and legacy Regular is the honest fallback for both.
            TurnKind::Regular | TurnKind::GoalContinuation => LegacyTurnKind::Regular,
        };
        let usage = turn.usage.as_ref().map(|usage| TurnUsage {
            input_tokens: saturating_u32(usage.query.input_tokens),
            output_tokens: saturating_u32(usage.query.output_tokens),
            // Forward mapped None → 0; 0 maps back to None. A genuine Some(0)
            // is indistinguishable and also becomes None.
            cache_creation_input_tokens: (usage.query.cache_creation_input_tokens > 0)
                .then(|| saturating_u32(usage.query.cache_creation_input_tokens)),
            cache_read_input_tokens: (usage.query.cache_read_input_tokens > 0)
                .then(|| saturating_u32(usage.query.cache_read_input_tokens)),
            reasoning_output_tokens: (usage.query.reasoning_tokens > 0)
                .then(|| saturating_u32(usage.query.reasoning_tokens)),
            // Always recorded in the canonical totals (forward fills it with
            // input+output when the legacy total was absent).
            total_tokens: Some(saturating_u32(usage.query.total_tokens)),
        });
        let error = turn.error.as_ref().map(|error| TurnError {
            code: error.error_code.clone(),
            message: error.message.clone(),
            recovery_hint: recovery_hint_from_details(error),
        });

        let record = TurnRecord {
            id,
            session_id: legacy_session_id(&turn.session_id)?,
            sequence: turn.sequence,
            started_at: turn.started_at,
            completed_at: turn.completed_at,
            status,
            kind,
            // The canonical snapshot keeps one model slug (the request model
            // when both existed); the logical model is not separately
            // recoverable.
            model: turn.model.model.clone(),
            model_binding_id: (turn.model.provider != "unknown")
                .then(|| turn.model.provider.clone()),
            reasoning_effort_selection: turn
                .model
                .reasoning_effort
                .map(|effort| effort.to_string()),
            request_model: turn.model.model.clone(),
            request_thinking: extras.and_then(|extras| extras.request_thinking.clone()),
            input_token_estimate: extras.and_then(|extras| extras.input_token_estimate),
            usage,
            latest_query_usage: extras.and_then(|extras| extras.latest_query_usage.clone()),
            context_occupancy: extras.and_then(|extras| extras.context_occupancy.clone()),
            stop_reason: extras.and_then(|extras| extras.stop_reason.clone()),
            failure_reason: extras.and_then(|extras| extras.failure_reason),
            error,
            session_context: extras.and_then(|extras| extras.session_context.clone()),
            turn_context: extras.and_then(|extras| extras.turn_context.clone()),
            schema_version: CURRENT_TURN_SCHEMA_VERSION,
        };
        Ok(vec![RolloutLine::Turn(Box::new(TurnLine {
            timestamp,
            turn: record,
        }))])
    }

    /// Projects one item envelope into one legacy `ItemRecord` (the v2
    /// writer emits exactly one payload per envelope), or `None` when the
    /// canonical variant has no legacy representation:
    ///
    /// - `Warning` is itself a migration artifact (orphan approval
    ///   decisions), so re-emitting it would fabricate a legacy kind that
    ///   was never written;
    /// - `FileChange`/`UserInputRequest`/`SubAgent`/`BackgroundTask`/
    ///   `GoalProgress` are new-only variants the forward projector never
    ///   produces from legacy records (and `FileChange` famously has no
    ///   legacy `TurnItem` — the persistence hole this redesign closes).
    fn project_item_envelope(
        &self,
        envelope: &ItemEnvelope,
    ) -> Result<Option<RolloutLine>, V2InverseError> {
        let payload = match &envelope.item {
            Item::UserMessage { content, entry, .. } => {
                let text = text_from_user_content(content);
                match entry {
                    UserMessageEntry::TurnStart | UserMessageEntry::Queue => {
                        TurnItem::UserMessage(TextItem { text })
                    }
                    UserMessageEntry::Steer => TurnItem::SteerInput(TextItem { text }),
                }
            }
            Item::AssistantMessage { text, .. } => {
                TurnItem::AgentMessage(TextItem { text: text.clone() })
            }
            Item::Reasoning { text, .. } => TurnItem::Reasoning(TextItem { text: text.clone() }),
            Item::Plan { entries } => TurnItem::Plan(TextItem {
                // The v2 writer produces exactly one entry (the legacy
                // rendered text); multiple entries join lossily with
                // newlines because the legacy plan is one text blob.
                text: entries
                    .iter()
                    .map(|entry| entry.step.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            Item::ToolCall {
                call_id,
                tool_name,
                input,
                ..
            } => TurnItem::ToolCall(ToolCallItem {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                input: input.clone().unwrap_or(serde_json::Value::Null),
            }),
            Item::ToolResult {
                call_id,
                output,
                display_content,
                is_error,
                ..
            } => TurnItem::ToolResult(ToolResultItem {
                tool_call_id: call_id.clone(),
                // The legacy tool name is not carried by the canonical
                // result variant.
                tool_name: None,
                output: output.clone(),
                display_content: display_content.clone(),
                is_error: *is_error,
            }),
            Item::CommandExecution {
                call_id,
                command,
                input,
                output,
                is_error,
                ..
            } => TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: call_id.clone(),
                // The legacy tool name was dropped by the forward mapping
                // (canonical carries origin/mode instead); replay only uses
                // it for display, and the exec family is the only producer
                // of this variant.
                tool_name: "exec_command".into(),
                command: command.clone(),
                input: input.clone().unwrap_or(serde_json::Value::Null),
                output: output.clone().unwrap_or(serde_json::Value::Null),
                is_error: *is_error,
            }),
            Item::HostedToolCall {
                tool_name, output, ..
            } => {
                let text = output
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                match tool_name.as_str() {
                    "image_generation" | "image_view" => {
                        TurnItem::ImageGeneration(TextItem { text })
                    }
                    // "web_search" and any other hosted tool name: the
                    // legacy format has no generic hosted variant, and the
                    // writer never produces others.
                    _ => TurnItem::WebSearch(TextItem { text }),
                }
            }
            Item::ContextCompaction { summary, .. } => TurnItem::ContextCompaction(TextItem {
                text: summary.clone().unwrap_or_default(),
            }),
            Item::Approval {
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
            } => {
                if let Some(decision) = decision {
                    // A decided approval becomes the legacy decision record.
                    // Legacy decisions lived in their own record with their
                    // own id, so a fresh bare UUID stands in (replay only
                    // needs uniqueness); the seq stays the shared fold seq so
                    // it sorts with its request.
                    let record = self.item_record(
                        ItemId::new(),
                        envelope,
                        TurnItem::ApprovalDecision(ApprovalDecisionItem {
                            approval_id: approval_id.clone(),
                            decision: legacy_decision_string(decision.decision).into(),
                            scope: legacy_scope_string(decision.scope).into(),
                            decision_source: (decision.decision_source
                                != devo_protocol::native::item::ApprovalDecisionSource::User)
                                .then_some(decision.decision_source),
                        }),
                    )?;
                    return Ok(Some(RolloutLine::Item(record)));
                }
                let (path, host, target) =
                    target.as_ref().map_or((None, None, None), |t| match t {
                        ApprovalTarget::Path { path } => {
                            (Some(path.display().to_string()), None, None)
                        }
                        ApprovalTarget::Host { host } => (None, Some(host.clone()), None),
                        ApprovalTarget::Command { command } => (None, None, Some(command.clone())),
                    });
                TurnItem::ApprovalRequest(ApprovalRequestItem {
                    approval_id: approval_id.clone(),
                    action_summary: action_summary.clone(),
                    justification: justification.clone(),
                    resource: resource.clone(),
                    available_scopes: available_scopes.clone(),
                    command_pattern: command_pattern.clone(),
                    command_prefix: command_prefix.clone(),
                    path,
                    host,
                    target,
                })
            }
            Item::FileChange { .. }
            | Item::UserInputRequest { .. }
            | Item::SubAgent { .. }
            | Item::BackgroundTask { .. }
            | Item::GoalProgress { .. }
            | Item::Warning { .. } => return Ok(None),
        };

        let record = self.item_record(legacy_item_id(&envelope.id)?, envelope, payload)?;
        Ok(Some(RolloutLine::Item(record)))
    }

    /// Builds one legacy `ItemRecord` mirroring the live write path
    /// (`build_item_record`): exactly one payload, placed in `output_items`
    /// (the live writer never fills `input_items`; replay reads both
    /// buckets).
    fn item_record(
        &self,
        id: ItemId,
        envelope: &ItemEnvelope,
        payload: TurnItem,
    ) -> Result<ItemLine, V2InverseError> {
        Ok(ItemLine {
            timestamp: envelope.updated_at,
            item: ItemRecord {
                id,
                session_id: legacy_session_id(&envelope.session_id)?,
                turn_id: legacy_turn_id(&envelope.turn_id)?,
                seq: envelope.seq,
                timestamp: envelope.updated_at,
                started_at: (envelope.created_at != envelope.updated_at)
                    .then_some(envelope.created_at),
                // Not modeled on the canonical envelope: orchestration
                // placement, the turn status at append time, sibling turns,
                // worklog and per-item errors.
                attempt_placement: None,
                turn_status: None,
                sibling_turn_ids: Vec::new(),
                input_items: Vec::new(),
                output_items: vec![payload],
                worklog: None,
                error: None,
                schema_version: CURRENT_ITEM_SCHEMA_VERSION,
            },
        })
    }

    fn project_internal(
        &self,
        timestamp: DateTime<Utc>,
        session_id: &CanonicalSessionId,
        turn_id: Option<&CanonicalTurnId>,
        seq: u64,
        entry: &InternalRecordV2,
    ) -> Result<Vec<RolloutLine>, V2InverseError> {
        match entry {
            InternalRecordV2::Entry { entry } => {
                let payload = match entry {
                    InternalEntry::TurnSummary { text } => {
                        TurnItem::TurnSummary(TextItem { text: text.clone() })
                    }
                    InternalEntry::ToolProgress { call_id, message } => {
                        TurnItem::ToolProgress(ToolProgressItem {
                            tool_call_id: call_id.clone(),
                            message: message.clone(),
                        })
                    }
                    InternalEntry::HookPrompt { text } => {
                        TurnItem::HookPrompt(TextItem { text: text.clone() })
                    }
                };
                // Identity and position travel on the line (exact); only the
                // record id is synthesized, since internal entries have no
                // item id of their own (replay only needs uniqueness).
                Ok(vec![RolloutLine::Item(ItemLine {
                    timestamp,
                    item: ItemRecord {
                        id: ItemId::new(),
                        session_id: legacy_session_id(session_id)?,
                        turn_id: legacy_turn_id(turn_id.ok_or(V2InverseError::MissingTurnId)?)?,
                        seq,
                        timestamp,
                        started_at: None,
                        attempt_placement: None,
                        turn_status: None,
                        sibling_turn_ids: Vec::new(),
                        input_items: Vec::new(),
                        output_items: vec![payload],
                        worklog: None,
                        error: None,
                        schema_version: CURRENT_ITEM_SCHEMA_VERSION,
                    },
                })])
            }
            InternalRecordV2::SessionContext(context) => {
                Ok(vec![RolloutLine::SessionContextUpdated(Box::new(
                    SessionContextUpdatedLine {
                        timestamp,
                        session_id: legacy_session_id(session_id)?,
                        session_context: (**context).clone(),
                        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
                    },
                ))])
            }
            InternalRecordV2::MessageEdit(record) => Ok(vec![RolloutLine::MessageEditRecorded(
                Box::new(crate::conversation::MessageEditRecordedLine {
                    timestamp,
                    record: record.clone(),
                }),
            )]),
            InternalRecordV2::TurnSuperseded(record) => Ok(vec![RolloutLine::TurnSuperseded(
                Box::new(crate::conversation::TurnSupersededLine {
                    timestamp,
                    record: record.clone(),
                }),
            )]),
            InternalRecordV2::SessionSettings {
                field,
                value,
                epoch,
                ..
            } => Ok(vec![RolloutLine::SessionSettings(
                crate::conversation::records::SessionSettingsLine {
                    timestamp,
                    session_id: legacy_session_id(session_id)?,
                    field: *field,
                    value: value.clone(),
                    epoch: *epoch,
                },
            )]),
            // There is no legacy session-rollout representation for Goal
            // snapshots; old builds continue to use the read-only
            // goal-records compatibility store.
            InternalRecordV2::GoalState { .. } | InternalRecordV2::UsageRecord { .. } => {
                Ok(Vec::new())
            }
        }
    }
}

/// Parses a canonical id back into the legacy UUID newtype. Prefixed
/// canonical ids (`ses_…`/`turn_…`/`item_…`) fail here — see
/// [`V2InverseError::NonLegacyId`].
fn legacy_uuid(id: &str) -> Result<Uuid, V2InverseError> {
    Uuid::parse_str(id).map_err(|_| V2InverseError::NonLegacyId(id.to_owned()))
}

fn legacy_session_id(id: &CanonicalSessionId) -> Result<SessionId, V2InverseError> {
    Ok(SessionId::from(legacy_uuid(id.as_str())?))
}

fn legacy_turn_id(id: &CanonicalTurnId) -> Result<TurnId, V2InverseError> {
    Ok(TurnId::from(legacy_uuid(id.as_str())?))
}

fn legacy_item_id(id: &CanonicalItemId) -> Result<ItemId, V2InverseError> {
    Ok(ItemId::from(legacy_uuid(id.as_str())?))
}

/// The v2 writer only ever produces a single `Text` part (the legacy user
/// message was plain text); multiple text parts join with newlines and
/// non-text parts are dropped because legacy `TextItem` cannot represent
/// them.
fn text_from_user_content(content: &[UserInput]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn legacy_decision_string(decision: ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approved => "approve",
        ApprovalDecisionKind::Denied => "deny",
        ApprovalDecisionKind::Cancelled => "cancel",
    }
}

fn legacy_scope_string(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Turn => "turn",
        ApprovalScope::Session => "session",
        ApprovalScope::PathPrefix => "path_prefix",
        ApprovalScope::Host => "host",
        ApprovalScope::Tool => "tool",
        ApprovalScope::CommandPrefix => "command_prefix",
        ApprovalScope::CommandPrefixPersist => "command_prefix_persist",
    }
}

fn recovery_hint_from_details(error: &AgentError) -> Option<String> {
    error
        .details
        .as_ref()?
        .get("recoveryHint")?
        .as_str()
        .map(str::to_owned)
}

fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
