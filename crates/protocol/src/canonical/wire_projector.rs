//! Wire-level projector from the legacy item envelope (`ItemKind` + untyped
//! `serde_json::Value` payload bag) to the canonical typed `Item`.
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
use crate::protocol::ExecCommandSource;
use crate::{
    ApprovalDecisionPayload, ApprovalRequestPayload, CommandExecutionPayload, EventContext,
    FileChangePayload, ItemKind, ServerEvent, ToolCallPayload, ToolResultPayload,
    TypedItemEventPayload,
};

/// Projects one legacy wire payload into the canonical `Item` for its kind.
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
            let text = payload_text(payload)?;
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
                            // unified diff is the canonical form.
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
        ItemKind::ContextCompaction => Some(Item::ContextCompaction {
            // The wire payload carries only a display title, never the
            // trigger or the summary text.
            trigger: CompactionTrigger::AutoThreshold,
            before: ContextUsage {
                measured: false,
                ..ContextUsage::default()
            },
            after: None,
            summary: payload
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        }),
        ItemKind::ApprovalRequest => {
            let request = serde_json::from_value::<ApprovalRequestPayload>(payload.clone()).ok()?;
            Some(Item::Approval {
                approval_id: request.approval_id.to_string(),
                target_item_id: None,
                action_summary: request.action_summary,
                justification: request.justification,
                resource: request.resource,
                available_scopes: request.available_scopes,
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

/// Builds the canonical typed envelope for one legacy item event.
///
/// Returns `None` when the payload does not project (caller falls back to
/// the legacy envelope) or when the event has no turn id (the canonical
/// envelope requires one). `projected_at` stamps `created_at`/`updated_at`:
/// legacy item events carry no timestamp, so the fan-out time is the only
/// honest value.
pub fn typed_item_envelope(
    context: &EventContext,
    item: &crate::ItemEnvelope,
    state: ItemState,
    projected_at: DateTime<Utc>,
) -> Option<ItemEnvelope> {
    let canonical_item = project_wire_item(&item.item_kind, &item.payload, projected_at)?;
    Some(ItemEnvelope {
        id: ItemId::from_legacy_uuid(Uuid::from(item.item_id)),
        session_id: SessionId::from_legacy_uuid(Uuid::from(context.session_id)),
        turn_id: TurnId::from_legacy_uuid(Uuid::from(context.turn_id?)),
        // The item's own sequence when the emitter threaded it through;
        // otherwise the connection event sequence is the only ordering left.
        seq: context.item_seq.unwrap_or(context.seq),
        revision: 1,
        created_at: projected_at,
        updated_at: projected_at,
        state,
        item: canonical_item,
    })
}

/// Projects an `item/started` / `item/completed` server event into its
/// native typed notification (`{"context": ..., "item": <canonical
/// envelope>}`) for connections that opted in to typed items. All other
/// events — and item events whose payload does not project — return `None`
/// and keep the legacy ACP-wrapped path.
pub fn typed_item_notification_from_server_event(
    event: &ServerEvent,
) -> Option<(String, serde_json::Value)> {
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
    use crate::canonical::item::ApprovalDecisionSource;
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
    fn context_compaction_projects_without_summary_on_wire() {
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
                summary: None,
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
        let event = ServerEvent::InputQueueUpdated(crate::InputQueueUpdatedPayload {
            session_id: crate::SessionId::new(),
            pending_count: 0,
            pending_texts: Vec::new(),
        });
        assert_eq!(typed_item_notification_from_server_event(&event), None);
    }
}
