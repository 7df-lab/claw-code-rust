//! Wire-level projector from the legacy item envelope (`ItemKind` + untyped
//! `serde_json::Value` payload bag) to the native typed `Item`.
//!
//! Truth source: `devo-api-design/06-item-model.md` migration step 2 (P2):
//! the live protocol switches to typed items *before* persistence does, so
//! this projector is the wire-side counterpart of the core `LegacyProjector`
//! (which converts rollout files). It is used only for connections that
//! opted in to typed items; on any payload mismatch it returns `None` and
//! the caller falls back to the legacy envelope.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::ids::{ItemId, SessionId, TurnId};
use super::item::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalScope, ApprovalTarget, CompactionTrigger,
    ContextUsage, ExecOrigin, ExecutionMode, FileChangeEntry, FileChangeKind, Item, ItemEnvelope,
    ItemState, PlanEntry, PlanStepStatus, ToolSource, UserInput, UserMessageEntry,
};
use super::model::ModelBinding;
use crate::protocol::ExecCommandSource;
use crate::{
    ApprovalDecisionPayload, ApprovalRequestPayload, CommandExecutionPayload, EventContext,
    FileChangePayload, ItemKind, ServerEvent, ToolCallPayload, ToolResultPayload,
    TypedItemEventPayload,
};

/// Projects one legacy wire payload into the native `Item` for its kind.
///
/// `decided_at` fills `ApprovalDecision.decided_at`: legacy decision events
/// carry no timestamp, so the caller supplies one (the fan-out stamps the
/// projection time; there is no honest earlier value).
///
/// Returns `None` on any payload that does not match the expected legacy
/// shape — the caller must then keep the legacy envelope for that event.
pub fn project_wire_item(
    kind: &ItemKind,
    payload: &serde_json::Value,
    decided_at: DateTime<Utc>,
) -> Option<Item> {
    match kind {
        ItemKind::UserMessage => {
            let text = payload_text(payload)?;
            Some(Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text { text }],
                // Steered messages are indistinguishable on the wire (there
                // is no SteerInput kind; `steer/accepted` is a separate
                // event), so every wire user message projects as TurnStart.
                entry: UserMessageEntry::TurnStart,
            })
        }
        ItemKind::AgentMessage => {
            let text = payload_text(payload)?;
            Some(Item::AssistantMessage { text, phase: None })
        }
        ItemKind::Reasoning => {
            // Empty text is valid on `item/started` before the first delta.
            let text = payload_text(payload).unwrap_or_default();
            Some(Item::Reasoning {
                text,
                provider_payload_ref: None,
            })
        }
        ItemKind::Plan => {
            let text = payload_text(payload)?;
            // Same caveat as the rollout projector: the legacy plan is one
            // rendered text blob, preserved verbatim in a single entry.
            Some(Item::Plan {
                entries: vec![PlanEntry {
                    step: text,
                    status: PlanStepStatus::Completed,
                }],
            })
        }
        ItemKind::ToolCall => {
            let call = serde_json::from_value::<ToolCallPayload>(payload.clone()).ok()?;
            Some(Item::ToolCall {
                call_id: call.tool_call_id,
                tool_name: call.tool_name,
                // Legacy wire calls all went through the builtin dispatcher.
                source: ToolSource::Builtin,
                server_name: None,
                // `command_actions` (UI parse info) is intentionally dropped.
                input: Some(call.parameters),
            })
        }
        ItemKind::McpToolCall => {
            // Never emitted by the server (dead wire variant); the payload
            // shape follows `ToolCallPayload`, which carries no server name.
            let call = serde_json::from_value::<ToolCallPayload>(payload.clone()).ok()?;
            Some(Item::ToolCall {
                call_id: call.tool_call_id,
                tool_name: call.tool_name,
                source: ToolSource::Mcp,
                server_name: None,
                input: Some(call.parameters),
            })
        }
        ItemKind::ToolResult => {
            let result = serde_json::from_value::<ToolResultPayload>(payload.clone()).ok()?;
            Some(Item::ToolResult {
                call_id: result.tool_call_id,
                output: result.content,
                display_content: result.display_content,
                is_error: result.is_error,
                truncated: false,
            })
        }
        ItemKind::CommandExecution => {
            let command =
                serde_json::from_value::<CommandExecutionPayload>(payload.clone()).ok()?;
            let origin = match command.source {
                ExecCommandSource::Agent
                | ExecCommandSource::UnifiedExecStartup
                | ExecCommandSource::UnifiedExecInteraction => ExecOrigin::AgentTool,
                ExecCommandSource::UserShell => ExecOrigin::UserShell,
            };
            Some(Item::CommandExecution {
                call_id: command.tool_call_id,
                command: command.command,
                argv: None,
                // The cwd is not carried on the wire.
                cwd: PathBuf::new(),
                input: command.input,
                output: command.output,
                exit_code: None,
                execution_handle: None,
                is_error: command.is_error,
                execution_mode: ExecutionMode::Foreground,
                origin,
                sandbox: None,
            })
        }
        ItemKind::FileChange => {
            let change = serde_json::from_value::<FileChangePayload>(payload.clone()).ok()?;
            let changes = change
                .changes
                .into_iter()
                .map(|(path, change)| {
                    let change = match change {
                        crate::protocol::FileChange::Add { content } => {
                            FileChangeKind::Add { content }
                        }
                        crate::protocol::FileChange::Delete { content } => {
                            FileChangeKind::Delete { content }
                        }
                        crate::protocol::FileChange::Update {
                            unified_diff,
                            move_path,
                            // `old_text`/`new_text` are UI diff material; the
                            // unified diff is the native form.
                            ..
                        } => FileChangeKind::Update {
                            unified_diff,
                            move_path,
                        },
                    };
                    FileChangeEntry { path, change }
                })
                .collect();
            Some(Item::FileChange {
                call_id: change.tool_call_id,
                changes,
                sandbox: None,
            })
        }
        ItemKind::WebSearch => {
            // Never emitted by the server (dead wire variant); hosted tool
            // payloads carry no call id, so the id stays explicitly empty.
            Some(Item::HostedToolCall {
                call_id: String::new(),
                tool_name: "web_search".into(),
                input: None,
                output: Some(hosted_tool_output(payload)),
            })
        }
        ItemKind::ImageView => {
            // Never emitted by the server (dead wire variant); named after
            // the wire kind (the persisted sibling is `image_generation`).
            Some(Item::HostedToolCall {
                call_id: String::new(),
                tool_name: "image_view".into(),
                input: None,
                output: Some(hosted_tool_output(payload)),
            })
        }
        ItemKind::ContextCompaction => {
            let failed = payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| {
                    status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error")
                })
                || payload
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|title| title.eq_ignore_ascii_case("Compaction failed"));
            let message = payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty());
            let summary = if failed {
                Some(match message {
                    Some(message) => format!("Compaction failed: {message}"),
                    None => "Compaction failed".to_string(),
                })
            } else {
                payload
                    .get("text")
                    .or_else(|| payload.get("title"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            };
            let trigger = match payload
                .get("trigger")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
            {
                "manual" => CompactionTrigger::Manual,
                "providerRetry" | "provider_retry" => CompactionTrigger::ProviderRetry,
                _ => CompactionTrigger::AutoThreshold,
            };
            Some(Item::ContextCompaction {
                trigger,
                before: ContextUsage {
                    measured: false,
                    ..ContextUsage::default()
                },
                after: None,
                summary,
            })
        }
        ItemKind::ApprovalRequest => {
            let request = serde_json::from_value::<ApprovalRequestPayload>(payload.clone()).ok()?;
            Some(Item::Approval {
                approval_id: request.approval_id.to_string(),
                target_item_id: None,
                action_summary: request.action_summary,
                justification: request.justification,
                resource: request.resource,
                available_scopes: request.available_scopes,
                command_pattern: request.command_pattern,
                command_prefix: request.command_prefix,
                target: approval_target(request.path, request.host, request.target),
                decision: None,
            })
        }
        ItemKind::ApprovalDecision => {
            let decision =
                serde_json::from_value::<ApprovalDecisionPayload>(payload.clone()).ok()?;
            Some(Item::Approval {
                approval_id: decision.approval_id.to_string(),
                target_item_id: None,
                action_summary: payload
                    .get("action_summary")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                justification: payload
                    .get("justification")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                resource: payload
                    .get("resource")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                available_scopes: payload
                    .get("available_scopes")
                    .and_then(serde_json::Value::as_array)
                    .map(|scopes| {
                        scopes
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                command_pattern: string_array_field(payload, "command_pattern"),
                command_prefix: string_array_field(payload, "command_prefix"),
                target: approval_target(
                    payload
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    payload
                        .get("host")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    payload
                        .get("target")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                ),
                decision: Some(ApprovalDecision {
                    // Same string mapping as the rollout projector: legacy
                    // decisions were free-form ("Allow" appears in
                    // historical data), anything not clearly approve/deny is
                    // cancelled.
                    decision: match decision.decision.to_ascii_lowercase().as_str() {
                        "approve" | "approved" | "allow" => ApprovalDecisionKind::Approved,
                        "deny" | "denied" => ApprovalDecisionKind::Denied,
                        _ => ApprovalDecisionKind::Cancelled,
                    },
                    decision_source: decision.decision_source.unwrap_or_default(),
                    // Unknown legacy scope strings fall back to the
                    // narrowest scope.
                    scope: approval_scope_from_str(&decision.scope),
                    decided_at,
                }),
            })
        }
    }
}

fn string_array_field(payload: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

/// Builds the native typed envelope for one legacy item event.
///
/// Returns `None` when the payload does not project (caller falls back to
/// the legacy envelope) or when the event has no turn id (the native
/// envelope requires one). `projected_at` stamps `updated_at` (and
/// `created_at` when the payload has no earlier `startedAt`). Legacy item
/// events otherwise carry no timestamp, so the fan-out time is the fallback.
pub fn typed_item_envelope(
    context: &EventContext,
    item: &crate::ItemEnvelope,
    state: ItemState,
    projected_at: DateTime<Utc>,
) -> Option<ItemEnvelope> {
    let native_item = project_wire_item(&item.item_kind, &item.payload, projected_at)?;
    let created_at = payload_started_at(&item.payload).unwrap_or(projected_at);
    Some(ItemEnvelope {
        id: ItemId::from_legacy_uuid(Uuid::from(item.item_id)),
        session_id: SessionId::from_legacy_uuid(Uuid::from(context.session_id)),
        turn_id: TurnId::from_legacy_uuid(Uuid::from(context.turn_id?)),
        // The item's own sequence when the emitter threaded it through;
        // otherwise the connection event sequence is the only ordering left.
        seq: context.item_seq.unwrap_or(context.seq),
        revision: 1,
        created_at,
        updated_at: projected_at,
        state,
        item: native_item,
    })
}

/// Projects the runtime `TurnMetadata` carried by `turn/*` server events
/// into the native turn snapshot (L2-DES-APP-009 DD-3). Mirrors the
/// persisted-record mapping (`native_turn_from_record` in devo-core) so
/// live events and replayed history agree.
pub fn native_turn_from_metadata(metadata: &crate::TurnMetadata) -> crate::native::turn::Turn {
    use crate::native::turn::{TurnKind, TurnStatus};

    let kind = match &metadata.kind {
        crate::TurnKind::Regular | crate::TurnKind::Review | crate::TurnKind::Other(_) => {
            TurnKind::Regular
        }
        crate::TurnKind::ManualCompaction => TurnKind::Compaction,
    };
    let status = match metadata.status {
        crate::TurnStatus::Pending
        | crate::TurnStatus::Running
        | crate::TurnStatus::WaitingApproval => TurnStatus::InProgress,
        crate::TurnStatus::Completed => TurnStatus::Completed,
        crate::TurnStatus::Interrupted => TurnStatus::Interrupted,
        crate::TurnStatus::Failed => TurnStatus::Failed,
    };
    let usage = metadata
        .usage
        .as_ref()
        .map(|usage| crate::native::usage::TurnUsage {
            query: crate::native::usage::UsageTotals {
                total_tokens: u64::from(
                    usage
                        .total_tokens
                        .unwrap_or(usage.input_tokens + usage.output_tokens),
                ),
                input_tokens: u64::from(usage.input_tokens),
                output_tokens: u64::from(usage.output_tokens),
                reasoning_tokens: u64::from(usage.reasoning_output_tokens.unwrap_or(0)),
                cache_read_input_tokens: u64::from(usage.cache_read_input_tokens.unwrap_or(0)),
                cache_creation_input_tokens: u64::from(
                    usage.cache_creation_input_tokens.unwrap_or(0),
                ),
                call_count: 0,
                metered_call_count: 1,
                ..Default::default()
            },
            overhead: crate::native::usage::UsageTotals::default(),
        });
    crate::native::turn::Turn {
        id: TurnId::from_legacy_uuid(Uuid::from(metadata.turn_id)),
        session_id: SessionId::from_legacy_uuid(Uuid::from(metadata.session_id)),
        sequence: metadata.sequence,
        kind,
        status,
        model: ModelBinding {
            provider: metadata
                .model_binding_id
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            model: if metadata.request_model.is_empty() {
                metadata.model.clone()
            } else {
                metadata.request_model.clone()
            },
            reasoning_effort: metadata
                .reasoning_effort_selection
                .as_deref()
                .and_then(|selection| selection.parse().ok())
                .or(metadata.reasoning_effort),
        },
        collaboration_mode: None,
        started_at: metadata.started_at,
        completed_at: metadata.completed_at,
        error: None,
        usage,
    }
}

fn native_session_value(metadata: &crate::SessionMetadata) -> serde_json::Value {
    let status = match metadata.status {
        crate::SessionRuntimeStatus::ActiveTurn | crate::SessionRuntimeStatus::WaitingClient => {
            "active"
        }
        crate::SessionRuntimeStatus::Idle
        | crate::SessionRuntimeStatus::Archived
        | crate::SessionRuntimeStatus::Unloaded => "idle",
    };
    let permission_profile = match metadata.permission_preset {
        Some(crate::PermissionPreset::AutoReview) => "autoReview",
        Some(crate::PermissionPreset::FullAccess) => "fullAccess",
        Some(crate::PermissionPreset::Default) | None => "default",
    };
    let parent = metadata.parent_session_id.map(|session_id| {
        let session_id = SessionId::from_legacy_uuid(Uuid::from(session_id));
        if metadata.agent_path.is_some() {
            serde_json::json!({
                "kind": "agent",
                "sessionId": session_id,
                "role": metadata.agent_role,
            })
        } else {
            serde_json::json!({
                "kind": "fork",
                "sessionId": session_id,
            })
        }
    });
    serde_json::json!({
        "id": SessionId::from_legacy_uuid(Uuid::from(metadata.session_id)),
        "version": 1,
        "cwd": metadata.cwd,
        "additionalDirectories": metadata.additional_directories,
        "parent": parent,
        "ephemeral": metadata.ephemeral,
        "createdAt": metadata.created_at,
        "status": status,
        "flags": [],
        "archived": matches!(metadata.status, crate::SessionRuntimeStatus::Archived),
        "queuedCount": 0,
        "title": metadata.title,
        "model": {
            "provider": metadata.model_binding_id.as_deref().unwrap_or("unknown"),
            "model": metadata.model.as_deref().unwrap_or_default(),
            "reasoningEffort": metadata.reasoning_effort_selection,
        },
        "settings": {
            "permissionProfile": permission_profile,
            "reasoningEffort": metadata.reasoning_effort_selection,
            "mode": serde_json::to_value(metadata.collaboration_mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string)),
            "effectiveContextWindow": metadata.effective_context_window,
        },
        "preview": "",
        "lastActivityAt": metadata.last_activity_at,
        "usage": {
            "total": {
                "totalTokens": metadata.total_tokens,
                "inputTokens": metadata.total_input_tokens,
                "outputTokens": metadata.total_output_tokens,
                "reasoningTokens": 0,
                "cacheReadInputTokens": metadata.total_cache_read_tokens,
                "cacheCreationInputTokens": metadata.total_cache_creation_tokens,
                "callCount": 0,
                "meteredCallCount": 0,
                "failedCallCount": 0,
                "cancelledCallCount": 0,
            },
            "byPurpose": [],
            "updatedAt": metadata.updated_at,
        },
    })
}

/// Projects internal server events into their Native live-notification shape.
/// native typed notification (`{"context": ..., "item": <native
/// envelope>}`) for connections that opted in to typed items. All other
/// events — and item events whose payload does not project — return `None`
/// and keep the legacy ACP-wrapped path.
pub fn typed_item_notification_from_server_event(
    event: &ServerEvent,
) -> Option<(String, serde_json::Value)> {
    // Delta family (L2-DES-APP-009 DD-3): mapped onto the native delta
    // notifications; the per-item `chunk_index` is assigned at the emit site.
    // `FileChangeOutputDelta` has no native kind yet and stays on the legacy path.
    if let ServerEvent::ItemDelta {
        delta_kind,
        payload,
    } = event
    {
        let method = match delta_kind {
            crate::ItemDeltaKind::AgentMessageDelta => "item/assistantMessage/delta",
            crate::ItemDeltaKind::ReasoningSummaryTextDelta
            | crate::ItemDeltaKind::ReasoningTextDelta => "item/reasoning/delta",
            crate::ItemDeltaKind::CommandExecutionOutputDelta => {
                "item/commandExecution/outputDelta"
            }
            crate::ItemDeltaKind::ToolCallInputDelta => "item/toolCall/inputDelta",
            crate::ItemDeltaKind::PlanDelta => "item/plan/delta",
            crate::ItemDeltaKind::FileChangeOutputDelta => {
                return None;
            }
        };
        let delta = crate::native::event::ItemDelta {
            item_id: ItemId::from_legacy_uuid(Uuid::from(payload.context.item_id?)),
            session_id: crate::native::ids::SessionId::from_legacy_uuid(Uuid::from(
                payload.context.session_id,
            )),
            // Text deltas today always apply to the item's birth snapshot;
            // `item/updated` revisions are not emitted for them yet.
            base_revision: 1,
            chunk_index: payload.chunk_index.unwrap_or(0),
            delta: payload.delta.clone(),
        };
        let value = serde_json::to_value(delta).expect("serialize typed delta payload");
        return Some((method.to_string(), value));
    }
    match event {
        ServerEvent::SessionStarted(payload) => {
            return Some((
                "session/created".to_string(),
                serde_json::json!({ "session": native_session_value(&payload.session) }),
            ));
        }
        ServerEvent::SessionTitleUpdated(payload) => {
            return Some((
                "session/metadataUpdated".to_string(),
                serde_json::json!({ "session": native_session_value(&payload.session) }),
            ));
        }
        ServerEvent::SessionStatusChanged(payload) => {
            let status = match payload.status {
                crate::SessionRuntimeStatus::ActiveTurn
                | crate::SessionRuntimeStatus::WaitingClient => "active",
                crate::SessionRuntimeStatus::Idle
                | crate::SessionRuntimeStatus::Archived
                | crate::SessionRuntimeStatus::Unloaded => "idle",
            };
            return Some((
                "session/statusChanged".to_string(),
                serde_json::json!({
                    "sessionId": SessionId::from_legacy_uuid(Uuid::from(payload.session_id)),
                    "status": status,
                    "flags": [],
                    "activeTurnId": null,
                }),
            ));
        }
        ServerEvent::SessionArchived(payload) | ServerEvent::SessionUnarchived(payload) => {
            return Some((
                "session/archived".to_string(),
                serde_json::json!({
                    "sessionId": SessionId::from_legacy_uuid(Uuid::from(
                        payload.session.session_id,
                    )),
                    "archived": matches!(event, ServerEvent::SessionArchived(_)),
                }),
            ));
        }
        ServerEvent::SessionClosed(payload) => {
            return Some((
                "session/closed".to_string(),
                serde_json::json!({
                    "sessionId": SessionId::from_legacy_uuid(Uuid::from(
                        payload.session.session_id,
                    )),
                }),
            ));
        }
        ServerEvent::SessionDeleted(payload) => {
            return Some((
                "session/deleted".to_string(),
                serde_json::json!({
                    "sessionId": SessionId::from_legacy_uuid(Uuid::from(payload.session_id)),
                    "deletedSessionIds": payload.deleted_session_ids.iter().map(|session_id| {
                        SessionId::from_legacy_uuid(Uuid::from(*session_id))
                    }).collect::<Vec<_>>(),
                }),
            ));
        }
        ServerEvent::WorkspaceChangesUpdated(payload) => {
            return Some((
                "workspace/changes/updated".to_string(),
                serde_json::json!({
                    "sessionId": SessionId::from_legacy_uuid(Uuid::from(payload.session_id)),
                    "turnId": TurnId::from_legacy_uuid(Uuid::from(payload.turn_id)),
                    "scope": payload.scope,
                    "status": payload.status,
                    "coverage": payload.coverage,
                    "changeSetStatus": payload.change_set_status,
                    "stats": {
                        "filesChanged": payload.stats.files_changed,
                        "additions": payload.stats.additions,
                        "deletions": payload.stats.deletions,
                    },
                    "version": payload.version,
                    "generatedAt": payload.generated_at,
                }),
            ));
        }
        _ => {}
    }
    // Turn lifecycle (L2-DES-APP-009 DD-3): all terminal states flow through
    // the single native `turn/completed` notification.
    match event {
        ServerEvent::TurnStarted(payload) => {
            let turn = native_turn_from_metadata(&payload.turn);
            let value = serde_json::json!({ "turn": turn });
            return Some(("turn/started".to_string(), value));
        }
        ServerEvent::TurnCompleted(payload) => {
            let turn = native_turn_from_metadata(&payload.turn);
            let value = serde_json::json!({ "turn": turn });
            return Some(("turn/completed".to_string(), value));
        }
        ServerEvent::TurnInterrupted(payload) => {
            let turn = native_turn_from_metadata(&payload.turn);
            let value = serde_json::json!({ "turn": turn });
            return Some(("turn/completed".to_string(), value));
        }
        ServerEvent::TurnFailed(payload) => {
            let mut turn = native_turn_from_metadata(&payload.turn);
            turn.error = payload.error.as_ref().map(|error| {
                let mut projected = crate::native::error::AgentError::new(
                    error.code.clone(),
                    error.message.clone(),
                );
                if let Some(hint) = &error.recovery_hint {
                    projected.details = Some(serde_json::json!({ "recoveryHint": hint }));
                }
                projected
            });
            let value = serde_json::json!({ "turn": turn });
            return Some(("turn/completed".to_string(), value));
        }
        // Context occupancy is already a native type; the projection is a
        // field rename. The mid-turn query-level usage meter
        // (`TurnUsageUpdated`) has no native event kind yet and stays on
        // the legacy path (straggler, L2-DES-APP-009 DD-3).
        ServerEvent::ContextUsageUpdated(payload) => {
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session_id),
                ),
                "occupancy": payload.occupancy,
            });
            return Some(("context/usageUpdated".to_string(), value));
        }
        // Provider retry status (ratified, L2-DES-APP-009 DD-3): projects
        // with provider/model/phase carried through. A legacy payload
        // without `max_attempts` predates the emit-chain threading and stays
        // on the legacy path.
        ServerEvent::TurnProviderRetryStatus(payload) => {
            let max_attempts = payload.max_attempts?;
            let mut error = crate::native::error::AgentError::new(
                crate::native::error::codes::PROVIDER_TEMPORARY_FAILURE.to_string(),
                payload.message.clone(),
            );
            error.retryable = true;
            error.retry_after_ms = Some(payload.backoff_ms);
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session_id),
                ),
                "turnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.turn_id),
                ),
                "attempt": u32::try_from(payload.attempt).unwrap_or(u32::MAX),
                "maxAttempts": max_attempts,
                "nextDelayMs": payload.backoff_ms,
                "error": error,
                "provider": payload.provider,
                "model": payload.model,
                "phase": match payload.phase {
                    crate::ProviderRetryPhase::Scheduled => "scheduled",
                    crate::ProviderRetryPhase::Resumed => "resumed",
                },
            });
            return Some(("model/queryRetrying".to_string(), value));
        }
        // Mid-turn usage meter (ratified, L2-DES-APP-009 DD-3): the live
        // per-query meter as a typed notification.
        ServerEvent::TurnUsageUpdated(payload) => {
            let usage = &payload.usage;
            let query = crate::native::usage::UsageTotals {
                total_tokens: usage
                    .total_tokens
                    .map(|total| total as u64)
                    .unwrap_or((usage.input_tokens + usage.output_tokens) as u64),
                input_tokens: usage.input_tokens as u64,
                output_tokens: usage.output_tokens as u64,
                reasoning_tokens: usage.reasoning_output_tokens.unwrap_or(0) as u64,
                cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0) as u64,
                cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as u64,
                call_count: 0,
                metered_call_count: 0,
                failed_call_count: 0,
                cancelled_call_count: 0,
                estimated_cost: None,
            };
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session_id),
                ),
                "turnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.turn_id),
                ),
                "usage": crate::native::usage::TurnUsage {
                    query,
                    overhead: crate::native::usage::UsageTotals {
                        total_tokens: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        reasoning_tokens: 0,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        call_count: 0,
                        metered_call_count: 0,
                        failed_call_count: 0,
                        cancelled_call_count: 0,
                        estimated_cost: None,
                    },
                },
                "lastQueryInputTokens": payload.last_query_input_tokens as u64,
                "sessionTotals": crate::native::usage::UsageTotals {
                    total_tokens: payload.total_tokens as u64,
                    input_tokens: payload.total_input_tokens as u64,
                    output_tokens: payload.total_output_tokens as u64,
                    reasoning_tokens: 0,
                    cache_read_input_tokens: payload.total_cache_read_tokens as u64,
                    cache_creation_input_tokens: 0,
                    call_count: 0,
                    metered_call_count: 0,
                    failed_call_count: 0,
                    cancelled_call_count: 0,
                    estimated_cost: None,
                },
                "contextWindow": payload.context_window,
            });
            return Some(("turn/usage/updated".to_string(), value));
        }
        // Plan updates (ratified, L2-DES-APP-009 DD-3): the full `Plan` item
        // rides `item/updated` with replace-by-revision semantics — no
        // plan-delta granularity. The legacy emit site carries no revision
        // counter yet, so the envelope is stamped at revision 1.
        ServerEvent::TurnPlanUpdated(payload) => {
            let entries: Vec<crate::native::item::PlanEntry> = payload
                .plan
                .iter()
                .map(|step| crate::native::item::PlanEntry {
                    step: step.step.clone(),
                    status: match step.status.as_str() {
                        "in_progress" => crate::native::item::PlanStepStatus::InProgress,
                        "completed" => crate::native::item::PlanStepStatus::Completed,
                        _ => crate::native::item::PlanStepStatus::Pending,
                    },
                })
                .collect();
            let now = Utc::now();
            let envelope = ItemEnvelope {
                id: ItemId::from_legacy_uuid(Uuid::from(payload.turn.turn_id)),
                session_id: SessionId::from_legacy_uuid(Uuid::from(payload.session_id)),
                turn_id: TurnId::from_legacy_uuid(Uuid::from(payload.turn.turn_id)),
                seq: 0,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Running,
                item: crate::native::item::Item::Plan { entries },
            };
            let value = serde_json::to_value(TypedItemEventPayload {
                context: crate::EventContext {
                    session_id: payload.session_id,
                    turn_id: Some(payload.turn.turn_id),
                    item_id: None,
                    seq: 0,
                    item_seq: None,
                },
                item: envelope,
            })
            .expect("serialize typed plan event payload");
            return Some(("item/updated".to_string(), value));
        }
        // Turn superseded by an accepted message edit (ratified #10): the
        // legacy payload carries everything the native event needs.
        ServerEvent::TurnSuperseded(payload) => {
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session_id),
                ),
                "supersededTurnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.superseded_turn_id),
                ),
                "replacementTurnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.replacement_turn_id),
                ),
                "editId": payload.edit_id,
                "reason": payload.reason,
            });
            return Some(("turn/superseded".to_string(), value));
        }
        // Compaction lifecycle (L2-DES-APP-009 DD-3): the emit site threads
        // the compaction turn id and trigger, so the native events project
        // without degradation. A completion without a persisted
        // `ContextCompaction` item (Skipped outcome) stays legacy-only.
        ServerEvent::SessionCompactionStarted(payload) => {
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session.session_id),
                ),
                "turnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.turn_id),
                ),
                "trigger": payload.trigger,
            });
            return Some(("context/compactionStarted".to_string(), value));
        }
        ServerEvent::SessionCompactionCompleted(payload) => {
            let item_id = payload.item_id?;
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session.session_id),
                ),
                "turnId": crate::native::ids::TurnId::from_legacy_uuid(
                    Uuid::from(payload.turn_id),
                ),
                "itemId": crate::native::ids::ItemId::from_legacy_uuid(Uuid::from(item_id)),
            });
            return Some(("context/compactionCompleted".to_string(), value));
        }
        ServerEvent::SessionCompactionFailed(payload) => {
            let value = serde_json::json!({
                "sessionId": crate::native::ids::SessionId::from_legacy_uuid(
                    Uuid::from(payload.session_id),
                ),
                "message": payload.message,
            });
            return Some(("context/compactionFailed".to_string(), value));
        }
        // `model/queryRetrying` is deferred: the native shape lacks the
        // provider/model/phase fields the TUI renders, so projecting now
        // would silently degrade the retry display (straggler,
        // L2-DES-APP-009 DD-3). The emit chain already threads max_attempts
        // for when the vocabulary decision lands.
        _ => {}
    }
    let (payload, state) = match event {
        ServerEvent::ItemStarted(payload) => (
            payload,
            if payload.item.item_kind == ItemKind::ApprovalRequest {
                ItemState::Waiting
            } else {
                ItemState::Running
            },
        ),
        ServerEvent::ItemCompleted(payload) => (payload, ItemState::Completed),
        _ => return None,
    };
    // No timestamp travels with legacy item events; the envelope is stamped
    // with the fan-out time (see `typed_item_envelope`).
    let mut envelope = typed_item_envelope(&payload.context, &payload.item, state, Utc::now())?;
    envelope.revision = payload
        .item
        .payload
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .and_then(|revision| u32::try_from(revision).ok())
        .unwrap_or(1);
    let value = serde_json::to_value(TypedItemEventPayload {
        context: payload.context.clone(),
        item: envelope,
    })
    .expect("serialize typed item event payload");
    Some((event.method_name().to_string(), value))
}

/// Legacy text payloads are `{"title": ..., "text": ...}` display objects;
/// the text is the only semantically meaningful field.
fn payload_text(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Optional wall-clock start injected by the server on `item/completed` so
/// clients can recover `created_at` when the legacy event itself is untimed.
fn payload_started_at(payload: &serde_json::Value) -> Option<DateTime<Utc>> {
    payload
        .get("startedAt")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

/// Hosted-tool payloads are text-like display objects; keep the text when
/// present, otherwise pass the raw payload through unchanged.
fn hosted_tool_output(payload: &serde_json::Value) -> serde_json::Value {
    payload
        .get("text")
        .cloned()
        .unwrap_or_else(|| payload.clone())
}

/// Builds the approval target from the legacy request's optional path, host,
/// or free-form target string, in that priority order.
fn approval_target(
    path: Option<String>,
    host: Option<String>,
    target: Option<String>,
) -> Option<ApprovalTarget> {
    if let Some(path) = path {
        Some(ApprovalTarget::Path {
            path: PathBuf::from(path),
        })
    } else if let Some(host) = host {
        Some(ApprovalTarget::Host { host })
    } else {
        target.map(|command| ApprovalTarget::Command { command })
    }
}

fn approval_scope_from_str(scope: &str) -> ApprovalScope {
    match scope.to_ascii_lowercase().as_str() {
        "once" => ApprovalScope::Once,
        "turn" => ApprovalScope::Turn,
        "session" => ApprovalScope::Session,
        "path_prefix" => ApprovalScope::PathPrefix,
        "host" => ApprovalScope::Host,
        "tool" => ApprovalScope::Tool,
        "command_prefix" => ApprovalScope::CommandPrefix,
        "command_prefix_persist" => ApprovalScope::CommandPrefixPersist,
        _ => ApprovalScope::Once,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;
    use smol_str::SmolStr;

    use super::*;
    use crate::native::item::ApprovalDecisionSource;
    use crate::parse_command::ParsedCommand;
    use crate::{ApprovalRequestPayload, PendingServerRequestContext, ServerRequestKind};

    fn decided_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
    }

    fn project(kind: ItemKind, payload: serde_json::Value) -> Option<Item> {
        project_wire_item(&kind, &payload, decided_at())
    }

    #[test]
    fn user_message_projects_as_turn_start_text() {
        let item = project(
            ItemKind::UserMessage,
            serde_json::json!({ "title": "You", "text": "hello" }),
        );
        assert_eq!(
            item,
            Some(Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "hello".into()
                }],
                entry: UserMessageEntry::TurnStart,
            })
        );
    }

    #[test]
    fn agent_message_projects_with_no_phase() {
        let item = project(
            ItemKind::AgentMessage,
            serde_json::json!({ "title": "Assistant", "text": "done" }),
        );
        assert_eq!(
            item,
            Some(Item::AssistantMessage {
                text: "done".into(),
                phase: None,
            })
        );
    }

    #[test]
    fn reasoning_projects_without_provider_payload() {
        let item = project(
            ItemKind::Reasoning,
            serde_json::json!({ "title": "Reasoning", "text": "thinking" }),
        );
        assert_eq!(
            item,
            Some(Item::Reasoning {
                text: "thinking".into(),
                provider_payload_ref: None,
            })
        );
    }

    #[test]
    fn empty_reasoning_projects_for_item_started() {
        let item = project(
            ItemKind::Reasoning,
            serde_json::json!({ "title": "Reasoning", "text": "" }),
        );
        assert_eq!(
            item,
            Some(Item::Reasoning {
                text: String::new(),
                provider_payload_ref: None,
            })
        );
    }

    #[test]
    fn typed_item_envelope_preserves_started_at_as_created_at() {
        let started = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let completed = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 14).unwrap();
        let session = Uuid::nil();
        let turn = Uuid::from_u128(1);
        let item_id = Uuid::from_u128(2);
        let context = EventContext {
            session_id: session.into(),
            turn_id: Some(turn.into()),
            item_id: Some(item_id.into()),
            seq: 1,
            item_seq: Some(1),
        };
        let item = crate::ItemEnvelope {
            item_id: item_id.into(),
            item_kind: ItemKind::Reasoning,
            payload: serde_json::json!({
                "title": "Reasoning",
                "text": "thinking",
                "startedAt": started.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            }),
        };
        let envelope = typed_item_envelope(&context, &item, ItemState::Completed, completed)
            .expect("typed envelope");
        assert_eq!(envelope.created_at, started);
        assert_eq!(envelope.updated_at, completed);
    }

    #[test]
    fn plan_projects_as_single_completed_entry() {
        let item = project(
            ItemKind::Plan,
            serde_json::json!({ "title": "Plan", "text": "1. do\n2. done" }),
        );
        assert_eq!(
            item,
            Some(Item::Plan {
                entries: vec![PlanEntry {
                    step: "1. do\n2. done".into(),
                    status: PlanStepStatus::Completed,
                }],
            })
        );
    }

    #[test]
    fn tool_call_projects_as_builtin_and_drops_command_actions() {
        let payload = serde_json::to_value(ToolCallPayload {
            tool_call_id: "call-1".into(),
            tool_name: "read_file".into(),
            parameters: serde_json::json!({ "path": "src/lib.rs" }),
            command_actions: vec![ParsedCommand::Unknown { cmd: "ls".into() }],
        })
        .expect("serialize payload");
        let item = project(ItemKind::ToolCall, payload);
        assert_eq!(
            item,
            Some(Item::ToolCall {
                call_id: "call-1".into(),
                tool_name: "read_file".into(),
                source: ToolSource::Builtin,
                server_name: None,
                input: Some(serde_json::json!({ "path": "src/lib.rs" })),
            })
        );
    }

    #[test]
    fn mcp_tool_call_projects_with_mcp_source() {
        let payload = serde_json::to_value(ToolCallPayload {
            tool_call_id: "call-2".into(),
            tool_name: "mcp__docs__search".into(),
            parameters: serde_json::json!({ "query": "serde" }),
            command_actions: Vec::new(),
        })
        .expect("serialize payload");
        let item = project(ItemKind::McpToolCall, payload);
        assert_eq!(
            item,
            Some(Item::ToolCall {
                call_id: "call-2".into(),
                tool_name: "mcp__docs__search".into(),
                source: ToolSource::Mcp,
                server_name: None,
                input: Some(serde_json::json!({ "query": "serde" })),
            })
        );
    }

    #[test]
    fn tool_result_projects_content_and_display() {
        let payload = serde_json::to_value(ToolResultPayload {
            tool_call_id: "call-1".into(),
            tool_name: Some("read_file".into()),
            input: None,
            content: serde_json::json!({ "content": "fn main() {}" }),
            display_content: Some("fn main() {}".into()),
            is_error: false,
            summary: String::new(),
        })
        .expect("serialize payload");
        let item = project(ItemKind::ToolResult, payload);
        assert_eq!(
            item,
            Some(Item::ToolResult {
                call_id: "call-1".into(),
                output: serde_json::json!({ "content": "fn main() {}" }),
                display_content: Some("fn main() {}".into()),
                is_error: false,
                truncated: false,
            })
        );
    }

    #[test]
    fn command_execution_projects_agent_tool_origin() {
        let payload = serde_json::to_value(CommandExecutionPayload {
            tool_call_id: "call-3".into(),
            tool_name: "exec_command".into(),
            command: "cargo test".into(),
            input: Some(serde_json::json!({ "command": "cargo test" })),
            source: ExecCommandSource::Agent,
            command_actions: Vec::new(),
            output: Some(serde_json::json!({ "stdout": "ok" })),
            is_error: false,
        })
        .expect("serialize payload");
        let item = project(ItemKind::CommandExecution, payload);
        assert_eq!(
            item,
            Some(Item::CommandExecution {
                call_id: "call-3".into(),
                command: "cargo test".into(),
                argv: None,
                cwd: PathBuf::new(),
                input: Some(serde_json::json!({ "command": "cargo test" })),
                output: Some(serde_json::json!({ "stdout": "ok" })),
                exit_code: None,
                execution_handle: None,
                is_error: false,
                execution_mode: ExecutionMode::Foreground,
                origin: ExecOrigin::AgentTool,
                sandbox: None,
            })
        );
    }

    #[test]
    fn command_execution_projects_user_shell_origin() {
        let payload = serde_json::json!({
            "tool_call_id": "call-4",
            "tool_name": "exec_command",
            "command": "ls",
            "source": "user_shell",
        });
        let item = project(ItemKind::CommandExecution, payload);
        assert!(matches!(
            item,
            Some(Item::CommandExecution {
                origin: ExecOrigin::UserShell,
                ..
            })
        ));
    }

    #[test]
    fn file_change_projects_all_change_kinds() {
        let payload = serde_json::to_value(FileChangePayload {
            tool_call_id: "call-5".into(),
            tool_name: Some("apply_patch".into()),
            input: None,
            changes: vec![
                (
                    PathBuf::from("a.rs"),
                    crate::protocol::FileChange::Add {
                        content: "new".into(),
                    },
                ),
                (
                    PathBuf::from("b.rs"),
                    crate::protocol::FileChange::Delete {
                        content: "old".into(),
                    },
                ),
                (
                    PathBuf::from("c.rs"),
                    crate::protocol::FileChange::Update {
                        unified_diff: "@@".into(),
                        old_text: Some("o".into()),
                        new_text: Some("n".into()),
                        move_path: Some(PathBuf::from("d.rs")),
                    },
                ),
            ],
            is_error: false,
        })
        .expect("serialize payload");
        let item = project(ItemKind::FileChange, payload);
        assert_eq!(
            item,
            Some(Item::FileChange {
                call_id: "call-5".into(),
                changes: vec![
                    FileChangeEntry {
                        path: PathBuf::from("a.rs"),
                        change: FileChangeKind::Add {
                            content: "new".into()
                        },
                    },
                    FileChangeEntry {
                        path: PathBuf::from("b.rs"),
                        change: FileChangeKind::Delete {
                            content: "old".into()
                        },
                    },
                    FileChangeEntry {
                        path: PathBuf::from("c.rs"),
                        change: FileChangeKind::Update {
                            unified_diff: "@@".into(),
                            move_path: Some(PathBuf::from("d.rs")),
                        },
                    },
                ],
                sandbox: None,
            })
        );
    }

    #[test]
    fn web_search_and_image_view_project_as_hosted_tool_calls() {
        let search = project(
            ItemKind::WebSearch,
            serde_json::json!({ "title": "Web Search", "text": "results" }),
        );
        assert_eq!(
            search,
            Some(Item::HostedToolCall {
                call_id: String::new(),
                tool_name: "web_search".into(),
                input: None,
                output: Some(serde_json::Value::String("results".into())),
            })
        );

        let image = project(
            ItemKind::ImageView,
            serde_json::json!({ "title": "Image", "text": "artifact://1" }),
        );
        assert_eq!(
            image,
            Some(Item::HostedToolCall {
                call_id: String::new(),
                tool_name: "image_view".into(),
                input: None,
                output: Some(serde_json::Value::String("artifact://1".into())),
            })
        );
    }

    #[test]
    fn context_compaction_projects_title_as_summary_on_wire() {
        let item = project(
            ItemKind::ContextCompaction,
            serde_json::json!({ "title": "Context compacted" }),
        );
        assert_eq!(
            item,
            Some(Item::ContextCompaction {
                trigger: CompactionTrigger::AutoThreshold,
                before: ContextUsage {
                    measured: false,
                    ..ContextUsage::default()
                },
                after: None,
                summary: Some("Context compacted".to_string()),
            })
        );
    }

    #[test]
    fn context_compaction_failed_projects_message_into_summary() {
        let item = project(
            ItemKind::ContextCompaction,
            serde_json::json!({
                "title": "Compaction failed",
                "status": "failed",
                "message": "boom",
            }),
        );
        assert_eq!(
            item,
            Some(Item::ContextCompaction {
                trigger: CompactionTrigger::AutoThreshold,
                before: ContextUsage {
                    measured: false,
                    ..ContextUsage::default()
                },
                after: None,
                summary: Some("Compaction failed: boom".to_string()),
            })
        );
    }

    #[test]
    fn approval_request_projects_undecided_approval() {
        let payload = serde_json::to_value(ApprovalRequestPayload {
            request: PendingServerRequestContext {
                request_id: SmolStr::new("req-1"),
                request_kind: ServerRequestKind::ItemCommandExecutionRequestApproval,
                session_id: crate::SessionId::new(),
                turn_id: None,
                item_id: None,
            },
            approval_id: SmolStr::new("appr-1"),
            action_summary: "Run cargo test".into(),
            justification: "Need to verify".into(),
            resource: Some("ShellExec".into()),
            available_scopes: vec!["Once".into()],
            path: None,
            host: None,
            target: Some("cargo test".into()),
            command_pattern: None,
            command_prefix: None,
        })
        .expect("serialize payload");
        let item = project(ItemKind::ApprovalRequest, payload);
        assert_eq!(
            item,
            Some(Item::Approval {
                approval_id: "appr-1".into(),
                target_item_id: None,
                action_summary: "Run cargo test".into(),
                justification: "Need to verify".into(),
                resource: Some("ShellExec".into()),
                available_scopes: vec!["Once".into()],
                command_pattern: None,
                command_prefix: None,
                target: Some(ApprovalTarget::Command {
                    command: "cargo test".into()
                }),
                decision: None,
            })
        );
    }

    #[test]
    fn approval_decision_projects_with_supplied_decided_at() {
        let payload = serde_json::to_value(ApprovalDecisionPayload {
            approval_id: SmolStr::new("appr-1"),
            decision: "Allow".into(),
            scope: "Session".into(),
            decision_source: Some(ApprovalDecisionSource::User),
        })
        .expect("serialize payload");
        let item = project(ItemKind::ApprovalDecision, payload);
        assert_eq!(
            item,
            Some(Item::Approval {
                approval_id: "appr-1".into(),
                target_item_id: None,
                action_summary: String::new(),
                justification: String::new(),
                resource: None,
                available_scopes: Vec::new(),
                command_pattern: None,
                command_prefix: None,
                target: None,
                decision: Some(ApprovalDecision {
                    decision: ApprovalDecisionKind::Approved,
                    scope: ApprovalScope::Session,
                    decision_source: ApprovalDecisionSource::User,
                    decided_at: decided_at(),
                }),
            })
        );
    }

    #[test]
    fn malformed_payload_returns_none_for_fallback() {
        assert_eq!(
            project(ItemKind::ToolCall, serde_json::json!({ "bogus": true })),
            None
        );
        assert_eq!(
            project(ItemKind::UserMessage, serde_json::json!({ "title": "You" })),
            None
        );
    }

    #[test]
    fn typed_notification_projects_item_started_and_completed() {
        let session_id = crate::SessionId::new();
        let turn_id = crate::TurnId::new();
        let item_id = crate::ItemId::new();
        let payload = crate::ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq: Some(7),
            },
            item: crate::ItemEnvelope {
                item_id,
                item_kind: ItemKind::AgentMessage,
                payload: serde_json::json!({ "title": "Assistant", "text": "hi" }),
            },
        };

        let (method, value) =
            typed_item_notification_from_server_event(&ServerEvent::ItemCompleted(payload))
                .expect("projects");
        assert_eq!(method, "item/completed");
        let notification: TypedItemEventPayload =
            serde_json::from_value(value).expect("deserialize typed payload");
        let envelope = notification.item;
        assert_eq!(envelope.id.as_str(), item_id.to_string());
        assert_eq!(envelope.session_id.as_str(), session_id.to_string());
        assert_eq!(envelope.turn_id.as_str(), turn_id.to_string());
        assert_eq!((envelope.seq, envelope.revision), (7, 1));
        assert_eq!(envelope.state, ItemState::Completed);
        assert_eq!(
            envelope.item,
            Item::AssistantMessage {
                text: "hi".into(),
                phase: None
            }
        );
    }

    #[test]
    fn typed_notification_skips_non_item_events() {
        let event = ServerEvent::ReferenceSearchUpdated(crate::ReferenceSearchSnapshot {
            search_id: crate::ReferenceSearchId::new(),
            query: "q".to_string(),
            results: Vec::new(),
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: false,
        });
        assert_eq!(typed_item_notification_from_server_event(&event), None);
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: agent-message and command-output deltas project to the
    /// native typed delta notifications carrying the emit-site
    /// `chunk_index`; delta kinds without a native counterpart stay
    /// unprojected.
    #[test]
    fn deltas_project_to_typed_notifications_with_chunk_index() {
        let context = crate::EventContext {
            session_id: crate::SessionId::new(),
            turn_id: Some(crate::TurnId::new()),
            item_id: Some(crate::ItemId::new()),
            seq: 0,
            item_seq: None,
        };
        let delta_event = |kind, chunk_index| ServerEvent::ItemDelta {
            delta_kind: kind,
            payload: crate::ItemDeltaPayload {
                context: context.clone(),
                delta: "chunk".to_string(),
                stream_index: None,
                channel: None,
                chunk_index,
            },
        };

        let (method, value) = typed_item_notification_from_server_event(&delta_event(
            crate::ItemDeltaKind::AgentMessageDelta,
            Some(7),
        ))
        .expect("assistant delta projects");
        assert_eq!(method, "item/assistantMessage/delta");
        let delta: crate::native::event::ItemDelta =
            serde_json::from_value(value).expect("typed delta payload");
        assert_eq!(delta.chunk_index, 7);
        assert_eq!(delta.base_revision, 1);
        assert_eq!(delta.delta, "chunk");

        let (method, _) = typed_item_notification_from_server_event(&delta_event(
            crate::ItemDeltaKind::CommandExecutionOutputDelta,
            Some(0),
        ))
        .expect("command output delta projects");
        assert_eq!(method, "item/commandExecution/outputDelta");

        let (method, _) = typed_item_notification_from_server_event(&delta_event(
            crate::ItemDeltaKind::ReasoningTextDelta,
            Some(3),
        ))
        .expect("reasoning delta projects");
        assert_eq!(method, "item/reasoning/delta");

        let (method, _) = typed_item_notification_from_server_event(&delta_event(
            crate::ItemDeltaKind::PlanDelta,
            Some(0),
        ))
        .expect("plan delta projects");
        assert_eq!(method, "item/plan/delta");

        assert!(
            typed_item_notification_from_server_event(&delta_event(
                crate::ItemDeltaKind::FileChangeOutputDelta,
                Some(0),
            ))
            .is_none(),
            "file-change output deltas stay on the legacy path until a native kind exists"
        );
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: turn lifecycle events project to native turn
    /// notifications; all terminal states flow through `turn/completed`.
    #[test]
    fn turn_lifecycle_events_project_to_native_turns() {
        let turn = crate::TurnMetadata {
            turn_id: crate::TurnId::new(),
            session_id: crate::SessionId::new(),
            sequence: 3,
            status: crate::TurnStatus::Running,
            kind: crate::TurnKind::Regular,
            model: "kimi-k3".into(),
            model_binding_id: Some("binding-1".into()),
            reasoning_effort_selection: Some("high".into()),
            reasoning_effort: None,
            request_model: "kimi-k3".into(),
            request_thinking: None,
            started_at: decided_at(),
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };

        let (method, value) = typed_item_notification_from_server_event(&ServerEvent::TurnStarted(
            crate::TurnEventPayload {
                session_id: turn.session_id,
                turn: turn.clone(),
            },
        ))
        .expect("turn/started projects");
        assert_eq!(method, "turn/started");
        let projected: crate::native::turn::Turn =
            serde_json::from_value(value["turn"].clone()).expect("native turn");
        assert_eq!(projected.sequence, 3);
        assert_eq!(
            projected.status,
            crate::native::turn::TurnStatus::InProgress
        );
        assert_eq!(projected.model.provider, "binding-1");
        assert_eq!(
            projected.model.reasoning_effort,
            Some(crate::ReasoningEffort::High)
        );

        let mut failed_turn = turn.clone();
        failed_turn.status = crate::TurnStatus::Failed;
        let (method, value) = typed_item_notification_from_server_event(&ServerEvent::TurnFailed(
            crate::TurnFailedPayload {
                session_id: turn.session_id,
                turn: failed_turn,
                error: Some(crate::TurnErrorPayload {
                    code: "E_BROKE".into(),
                    message: "it broke".into(),
                    recovery_hint: None,
                }),
            },
        ))
        .expect("turn/failed projects as turn/completed");
        assert_eq!(method, "turn/completed");
        let projected: crate::native::turn::Turn =
            serde_json::from_value(value["turn"].clone()).expect("native turn");
        assert_eq!(projected.status, crate::native::turn::TurnStatus::Failed);
        assert_eq!(
            projected.error.expect("error").message,
            "it broke".to_string()
        );
    }

    #[test]
    fn live_session_status_and_delete_events_use_native_shapes() {
        let session_id = crate::SessionId::new();
        let (method, value) = typed_item_notification_from_server_event(
            &ServerEvent::SessionStatusChanged(crate::SessionStatusChangedPayload {
                session_id,
                status: crate::SessionRuntimeStatus::WaitingClient,
            }),
        )
        .expect("session status projects");
        assert_eq!(method, "session/statusChanged");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "status": "active",
                "flags": [],
                "activeTurnId": null,
            })
        );

        let child_id = crate::SessionId::new();
        let (method, value) = typed_item_notification_from_server_event(
            &ServerEvent::SessionDeleted(crate::SessionDeletedPayload {
                session_id,
                deleted_session_ids: vec![session_id, child_id],
            }),
        )
        .expect("session deletion projects");
        assert_eq!(method, "session/deleted");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "deletedSessionIds": [session_id.to_string(), child_id.to_string()],
            })
        );
    }

    /// Trace: L2-DES-APP-009
    /// Trace: L2-DES-APP-009
    /// Verifies: context usage events project to the native
    /// `context/usageUpdated` notification carrying the occupancy unchanged.
    #[test]
    fn context_usage_projects_to_native_notification() {
        let occupancy = crate::native::item::ContextOccupancy {
            total_tokens: 12345,
            context_window_tokens: 200_000,
            categories: Vec::new(),
        };
        let (method, value) = typed_item_notification_from_server_event(
            &ServerEvent::ContextUsageUpdated(crate::ContextUsageUpdatedPayload {
                session_id: crate::SessionId::new(),
                occupancy: occupancy.clone(),
            }),
        )
        .expect("context usage projects");
        assert_eq!(method, "context/usageUpdated");
        let projected: crate::native::item::ContextOccupancy =
            serde_json::from_value(value["occupancy"].clone()).expect("occupancy payload");
        assert_eq!(projected, occupancy);
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: the mid-turn usage meter projects to native
    /// `turn/usage/updated` with per-query totals, the last-query meter, and
    /// the context window (ratified vocabulary).
    #[test]
    fn turn_usage_meter_projects_to_native_notification() {
        let payload = crate::TurnUsageUpdatedPayload {
            session_id: crate::SessionId::new(),
            turn_id: crate::TurnId::new(),
            usage: crate::TurnUsage {
                input_tokens: 100,
                output_tokens: 40,
                cache_creation_input_tokens: Some(5),
                cache_read_input_tokens: Some(10),
                reasoning_output_tokens: Some(7),
                total_tokens: Some(162),
            },
            total_input_tokens: 500,
            total_output_tokens: 200,
            total_tokens: 700,
            total_cache_read_tokens: 50,
            last_query_input_tokens: 96,
            context_window: Some(200_000),
        };
        let turn_id = payload.turn_id;
        let (method, value) =
            typed_item_notification_from_server_event(&ServerEvent::TurnUsageUpdated(payload))
                .expect("turn usage meter projects");
        assert_eq!(method, "turn/usage/updated");
        assert_eq!(value["turnId"].as_str(), Some(turn_id.to_string().as_str()));
        assert_eq!(value["usage"]["query"]["inputTokens"].as_u64(), Some(100));
        assert_eq!(value["usage"]["query"]["totalTokens"].as_u64(), Some(162));
        assert_eq!(value["usage"]["query"]["reasoningTokens"].as_u64(), Some(7));
        assert_eq!(value["lastQueryInputTokens"].as_u64(), Some(96));
        assert_eq!(value["contextWindow"].as_u64(), Some(200_000));
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: provider retry status projects to native
    /// `model/queryRetrying` with provider/model/phase carried through, and
    /// payloads predating the max_attempts threading stay on the legacy path.
    #[test]
    fn provider_retry_projects_with_provider_model_phase() {
        let payload = crate::TurnProviderRetryStatusPayload {
            session_id: crate::SessionId::new(),
            turn_id: crate::TurnId::new(),
            attempt: 2,
            max_attempts: Some(5),
            backoff_ms: 1500,
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            phase: crate::ProviderRetryPhase::Scheduled,
            message: "rate limited".to_string(),
        };
        let (method, value) = typed_item_notification_from_server_event(
            &ServerEvent::TurnProviderRetryStatus(payload),
        )
        .expect("retry status projects");
        assert_eq!(method, "model/queryRetrying");
        assert_eq!(value["attempt"].as_u64(), Some(2));
        assert_eq!(value["maxAttempts"].as_u64(), Some(5));
        assert_eq!(value["nextDelayMs"].as_u64(), Some(1500));
        assert_eq!(value["provider"].as_str(), Some("openai"));
        assert_eq!(value["model"].as_str(), Some("gpt-5"));
        assert_eq!(value["phase"].as_str(), Some("scheduled"));
        assert_eq!(
            value["error"]["errorCode"].as_str(),
            Some("PROVIDER_TEMPORARY_FAILURE")
        );
        assert_eq!(value["error"]["message"].as_str(), Some("rate limited"));
        assert_eq!(value["error"]["retryable"].as_bool(), Some(true));

        let mut legacy_payload = crate::TurnProviderRetryStatusPayload {
            session_id: crate::SessionId::new(),
            turn_id: crate::TurnId::new(),
            attempt: 1,
            max_attempts: None,
            backoff_ms: 100,
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            phase: crate::ProviderRetryPhase::Resumed,
            message: "boom".to_string(),
        };
        assert!(
            typed_item_notification_from_server_event(&ServerEvent::TurnProviderRetryStatus(
                legacy_payload.clone(),
            ))
            .is_none(),
            "payloads without max_attempts stay on the legacy path"
        );
        legacy_payload.attempt = 2;
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: plan updates project as a full native `Plan` item on
    /// `item/updated` (replace-by-revision, no plan-delta granularity).
    #[test]
    fn plan_update_projects_as_full_plan_item() {
        let turn_id = crate::TurnId::new();
        let payload = crate::TurnPlanUpdatedPayload {
            session_id: crate::SessionId::new(),
            turn: crate::TurnMetadata {
                turn_id,
                session_id: crate::SessionId::new(),
                sequence: 1,
                status: crate::TurnStatus::Running,
                kind: crate::TurnKind::Regular,
                model: "test-model".to_string(),
                model_binding_id: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                request_model: "test-model".to_string(),
                request_thinking: None,
                started_at: Utc::now(),
                completed_at: None,
                usage: None,
                stop_reason: None,
                failure_reason: None,
            },
            explanation: None,
            plan: vec![
                crate::TurnPlanStepPayload {
                    step: "explore".to_string(),
                    status: "completed".to_string(),
                },
                crate::TurnPlanStepPayload {
                    step: "implement".to_string(),
                    status: "in_progress".to_string(),
                },
                crate::TurnPlanStepPayload {
                    step: "verify".to_string(),
                    status: "pending".to_string(),
                },
            ],
        };
        let (method, value) =
            typed_item_notification_from_server_event(&ServerEvent::TurnPlanUpdated(payload))
                .expect("plan update projects");
        assert_eq!(method, "item/updated");
        let item = &value["item"];
        assert_eq!(
            item["id"].as_str(),
            Some(crate::native::ids::ItemId::from_legacy_uuid(turn_id.into()).as_str())
        );
        let entries = item["item"]["entries"].as_array().expect("plan entries");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["status"].as_str(), Some("completed"));
        assert_eq!(entries[1]["status"].as_str(), Some("inProgress"));
        assert_eq!(entries[2]["status"].as_str(), Some("pending"));
        assert_eq!(entries[1]["step"].as_str(), Some("implement"));
    }
}
