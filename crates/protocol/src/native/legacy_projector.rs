//! Inverse wire projector: native typed `Item` → legacy `(ItemKind,
//! serde_json::Value)` envelope for ACP and other legacy consumers.
//!
//! Companion of [`super::wire_projector::project_wire_item`]. Used by the
//! server emit path to construct native items first while still broadcasting
//! legacy `ItemStarted` / `ItemCompleted` events.

use std::path::PathBuf;

use super::item::{
    ApprovalDecisionKind, ApprovalScope, ApprovalTarget, CompactionTrigger, ExecOrigin,
    FileChangeEntry, FileChangeKind, Item, PlanEntry, ToolSource, UserInput,
};
use crate::protocol::ExecCommandSource;
use crate::protocol::FileChange;
use crate::{
    ApprovalDecisionPayload, ApprovalRequestPayload, CommandExecutionPayload, FileChangePayload,
    ItemKind, PendingServerRequestContext, ServerRequestKind, ToolCallPayload, ToolResultPayload,
};

/// Converts one native item into the legacy wire `(ItemKind, payload)` pair.
///
/// Returns `None` for item variants that have no legacy wire counterpart
/// (sub-agents, warnings, user-input requests, …).
pub fn legacy_wire_from_native_item(item: &Item) -> Option<(ItemKind, serde_json::Value)> {
    match item {
        Item::UserMessage { content, .. } => {
            let text = user_message_text(content);
            Some((ItemKind::UserMessage, text_display_payload("You", &text)))
        }
        Item::AssistantMessage { text, .. } => Some((
            ItemKind::AgentMessage,
            text_display_payload("Assistant", text),
        )),
        Item::Reasoning { text, .. } => {
            Some((ItemKind::Reasoning, text_display_payload("Reasoning", text)))
        }
        Item::Plan { entries } => Some((
            ItemKind::Plan,
            text_display_payload("Plan", &plan_entries_text(entries)),
        )),
        Item::ToolCall {
            call_id,
            tool_name,
            source,
            input,
            ..
        } => {
            let kind = match source {
                ToolSource::Mcp => ItemKind::McpToolCall,
                ToolSource::Builtin | ToolSource::Plugin => ItemKind::ToolCall,
            };
            let payload = serde_json::to_value(ToolCallPayload {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                parameters: input.clone().unwrap_or(serde_json::Value::Null),
                command_actions: Vec::new(),
            })
            .expect("serialize tool call payload");
            Some((kind, payload))
        }
        Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            truncated: _,
        } => {
            let payload = serde_json::to_value(ToolResultPayload {
                tool_call_id: call_id.clone(),
                tool_name: None,
                input: None,
                content: output.clone(),
                display_content: display_content.clone(),
                is_error: *is_error,
                summary: String::new(),
            })
            .expect("serialize tool result payload");
            Some((ItemKind::ToolResult, payload))
        }
        Item::CommandExecution {
            call_id,
            command,
            input,
            output,
            is_error,
            origin,
            ..
        } => {
            let source = match origin {
                ExecOrigin::UserShell => ExecCommandSource::UserShell,
                ExecOrigin::AgentTool => ExecCommandSource::Agent,
            };
            let payload = serde_json::to_value(CommandExecutionPayload {
                tool_call_id: call_id.clone(),
                tool_name: "exec_command".to_string(),
                command: command.clone(),
                input: input.clone(),
                source,
                command_actions: Vec::new(),
                output: output.clone(),
                is_error: *is_error,
            })
            .expect("serialize command execution payload");
            Some((ItemKind::CommandExecution, payload))
        }
        Item::FileChange {
            call_id,
            changes,
            sandbox: _,
        } => {
            let payload = serde_json::to_value(FileChangePayload {
                tool_call_id: call_id.clone(),
                tool_name: None,
                input: None,
                changes: changes.iter().map(file_change_entry_to_legacy).collect(),
                is_error: false,
            })
            .expect("serialize file change payload");
            Some((ItemKind::FileChange, payload))
        }
        Item::HostedToolCall {
            tool_name, output, ..
        } => match tool_name.as_str() {
            "web_search" => Some((
                ItemKind::WebSearch,
                hosted_tool_display_payload("Web Search", output),
            )),
            "image_view" => Some((
                ItemKind::ImageView,
                hosted_tool_display_payload("Image", output),
            )),
            _ => None,
        },
        Item::ContextCompaction {
            trigger, summary, ..
        } => Some((
            ItemKind::ContextCompaction,
            context_compaction_payload(*trigger, summary.as_deref()),
        )),
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
            let (path, host, target_command) = approval_target_fields(target.as_ref());
            if decision.is_none() {
                let payload = serde_json::to_value(ApprovalRequestPayload {
                    request: PendingServerRequestContext {
                        request_id: approval_id.clone().into(),
                        request_kind: approval_request_kind(resource.as_deref()),
                        session_id: crate::SessionId::new(),
                        turn_id: None,
                        item_id: None,
                    },
                    approval_id: approval_id.clone().into(),
                    action_summary: action_summary.clone(),
                    justification: justification.clone(),
                    resource: resource.clone(),
                    available_scopes: available_scopes.clone(),
                    path,
                    host,
                    target: target_command,
                    command_pattern: command_pattern.clone(),
                    command_prefix: command_prefix.clone(),
                })
                .expect("serialize approval request payload");
                Some((ItemKind::ApprovalRequest, payload))
            } else {
                let decision = decision.as_ref().expect("checked above");
                let mut payload = serde_json::to_value(ApprovalDecisionPayload {
                    approval_id: approval_id.clone().into(),
                    decision: legacy_decision_label(decision.decision).to_string(),
                    scope: legacy_scope_label(decision.scope).to_string(),
                    decision_source: Some(decision.decision_source),
                })
                .expect("serialize approval decision payload");
                if let Some(payload) = payload.as_object_mut() {
                    payload.insert("revision".into(), serde_json::json!(2));
                    payload.insert(
                        "action_summary".into(),
                        serde_json::json!(action_summary.clone()),
                    );
                    payload.insert(
                        "justification".into(),
                        serde_json::json!(justification.clone()),
                    );
                    payload.insert("resource".into(), serde_json::json!(resource.clone()));
                    payload.insert(
                        "available_scopes".into(),
                        serde_json::json!(available_scopes.clone()),
                    );
                    payload.insert("path".into(), serde_json::json!(path.clone()));
                    payload.insert("host".into(), serde_json::json!(host.clone()));
                    payload.insert("target".into(), serde_json::json!(target_command.clone()));
                    payload.insert(
                        "command_pattern".into(),
                        serde_json::json!(command_pattern.clone()),
                    );
                    payload.insert(
                        "command_prefix".into(),
                        serde_json::json!(command_prefix.clone()),
                    );
                    payload.insert("decided_at".into(), serde_json::json!(decision.decided_at));
                }
                Some((ItemKind::ApprovalDecision, payload))
            }
        }
        Item::UserInputRequest { .. }
        | Item::SubAgent { .. }
        | Item::BackgroundTask { .. }
        | Item::GoalProgress { .. }
        | Item::Warning { .. } => None,
    }
}

fn text_display_payload(title: &str, text: &str) -> serde_json::Value {
    serde_json::json!({ "title": title, "text": text })
}

fn hosted_tool_display_payload(
    title: &str,
    output: &Option<serde_json::Value>,
) -> serde_json::Value {
    let text = output
        .as_ref()
        .and_then(|value| match value {
            serde_json::Value::String(text) => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default();
    text_display_payload(title, text)
}

fn user_message_text(content: &[UserInput]) -> String {
    content
        .iter()
        .filter_map(|input| match input {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn plan_entries_text(entries: &[PlanEntry]) -> String {
    if entries.len() == 1 {
        entries[0].step.clone()
    } else {
        entries
            .iter()
            .map(|entry| entry.step.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn file_change_entry_to_legacy(entry: &FileChangeEntry) -> (PathBuf, FileChange) {
    let change = match &entry.change {
        FileChangeKind::Add { content } => FileChange::Add {
            content: content.clone(),
        },
        FileChangeKind::Delete { content } => FileChange::Delete {
            content: content.clone(),
        },
        FileChangeKind::Update {
            unified_diff,
            move_path,
        } => FileChange::Update {
            unified_diff: unified_diff.clone(),
            old_text: None,
            new_text: None,
            move_path: move_path.clone(),
        },
    };
    (entry.path.clone(), change)
}

fn context_compaction_payload(
    trigger: CompactionTrigger,
    summary: Option<&str>,
) -> serde_json::Value {
    let trigger = match trigger {
        CompactionTrigger::Manual => "manual",
        CompactionTrigger::ProviderRetry => "providerRetry",
        CompactionTrigger::AutoThreshold => "autoThreshold",
    };
    let summary = summary.unwrap_or_default();
    let failed = summary.starts_with("Compaction failed");
    if failed {
        let message = summary
            .strip_prefix("Compaction failed: ")
            .or_else(|| summary.strip_prefix("Compaction failed"))
            .unwrap_or(summary)
            .trim();
        serde_json::json!({
            "title": "Compaction failed",
            "text": summary,
            "status": "failed",
            "message": message,
            "trigger": trigger,
        })
    } else {
        serde_json::json!({
            "title": summary,
            "text": summary,
            "trigger": trigger,
        })
    }
}

fn approval_target_fields(
    target: Option<&ApprovalTarget>,
) -> (Option<String>, Option<String>, Option<String>) {
    match target {
        Some(ApprovalTarget::Path { path }) => (Some(path.display().to_string()), None, None),
        Some(ApprovalTarget::Host { host }) => (None, Some(host.clone()), None),
        Some(ApprovalTarget::Command { command }) => (None, None, Some(command.clone())),
        None => (None, None, None),
    }
}

fn approval_request_kind(resource: Option<&str>) -> ServerRequestKind {
    match resource {
        Some(resource) if resource.contains("ShellExec") => {
            ServerRequestKind::ItemCommandExecutionRequestApproval
        }
        Some(resource) if resource.contains("FileWrite") => {
            ServerRequestKind::ItemFileChangeRequestApproval
        }
        _ => ServerRequestKind::ItemPermissionsRequestApproval,
    }
}

fn legacy_decision_label(decision: ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approved => "approve",
        ApprovalDecisionKind::Denied => "deny",
        ApprovalDecisionKind::Cancelled => "cancel",
    }
}

fn legacy_scope_label(scope: ApprovalScope) -> &'static str {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::DateTime;
    use chrono::TimeZone;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use smol_str::SmolStr;

    use super::*;
    use crate::native::item::{ApprovalDecisionSource, UserMessageEntry};
    use crate::native::wire_projector::project_wire_item;
    use crate::parse_command::ParsedCommand;
    use crate::{ApprovalRequestPayload, PendingServerRequestContext, ServerRequestKind};

    fn decided_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
    }

    fn round_trip(kind: ItemKind, payload: serde_json::Value) {
        let native = project_wire_item(&kind, &payload, decided_at()).expect("forward project");
        let (reverse_kind, reverse_payload) =
            legacy_wire_from_native_item(&native).expect("reverse project");
        assert_eq!(reverse_kind, kind);
        let re_native =
            project_wire_item(&reverse_kind, &reverse_payload, decided_at()).expect("re-forward");
        assert_eq!(re_native, native);
    }

    #[test]
    fn round_trips_user_message() {
        round_trip(
            ItemKind::UserMessage,
            serde_json::json!({ "title": "You", "text": "hello" }),
        );
    }

    #[test]
    fn round_trips_agent_message() {
        round_trip(
            ItemKind::AgentMessage,
            serde_json::json!({ "title": "Assistant", "text": "done" }),
        );
    }

    #[test]
    fn round_trips_reasoning() {
        round_trip(
            ItemKind::Reasoning,
            serde_json::json!({ "title": "Reasoning", "text": "thinking" }),
        );
    }

    #[test]
    fn round_trips_plan() {
        round_trip(
            ItemKind::Plan,
            serde_json::json!({ "title": "Plan", "text": "1. do\n2. done" }),
        );
    }

    #[test]
    fn round_trips_tool_call() {
        round_trip(
            ItemKind::ToolCall,
            serde_json::to_value(ToolCallPayload {
                tool_call_id: "call-1".into(),
                tool_name: "read_file".into(),
                parameters: serde_json::json!({ "path": "src/lib.rs" }),
                command_actions: vec![ParsedCommand::Unknown { cmd: "ls".into() }],
            })
            .expect("serialize payload"),
        );
    }

    #[test]
    fn round_trips_tool_result() {
        round_trip(
            ItemKind::ToolResult,
            serde_json::to_value(ToolResultPayload {
                tool_call_id: "call-1".into(),
                tool_name: Some("read_file".into()),
                input: None,
                content: serde_json::json!({ "content": "fn main() {}" }),
                display_content: Some("fn main() {}".into()),
                is_error: false,
                summary: String::new(),
            })
            .expect("serialize payload"),
        );
    }

    #[test]
    fn round_trips_command_execution() {
        round_trip(
            ItemKind::CommandExecution,
            serde_json::to_value(CommandExecutionPayload {
                tool_call_id: "call-3".into(),
                tool_name: "exec_command".into(),
                command: "cargo test".into(),
                input: Some(serde_json::json!({ "command": "cargo test" })),
                source: ExecCommandSource::Agent,
                command_actions: Vec::new(),
                output: Some(serde_json::json!({ "stdout": "ok" })),
                is_error: false,
            })
            .expect("serialize payload"),
        );
    }

    #[test]
    fn round_trips_file_change() {
        round_trip(
            ItemKind::FileChange,
            serde_json::to_value(FileChangePayload {
                tool_call_id: "call-5".into(),
                tool_name: Some("apply_patch".into()),
                input: None,
                changes: vec![(
                    PathBuf::from("a.rs"),
                    FileChange::Add {
                        content: "new".into(),
                    },
                )],
                is_error: false,
            })
            .expect("serialize payload"),
        );
    }

    #[test]
    fn round_trips_context_compaction_success() {
        round_trip(
            ItemKind::ContextCompaction,
            serde_json::json!({ "title": "Context compacted", "trigger": "autoThreshold" }),
        );
    }

    #[test]
    fn round_trips_context_compaction_failure() {
        round_trip(
            ItemKind::ContextCompaction,
            serde_json::json!({
                "title": "Compaction failed",
                "status": "failed",
                "message": "boom",
            }),
        );
    }

    #[test]
    fn round_trips_approval_request() {
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
        let native = project_wire_item(&ItemKind::ApprovalRequest, &payload, decided_at())
            .expect("forward project");
        let (reverse_kind, reverse_payload) =
            legacy_wire_from_native_item(&native).expect("reverse project");
        assert_eq!(reverse_kind, ItemKind::ApprovalRequest);
        let restored = serde_json::from_value::<ApprovalRequestPayload>(reverse_payload)
            .expect("approval request payload");
        assert_eq!(restored.approval_id, SmolStr::new("appr-1"));
        assert_eq!(restored.action_summary, "Run cargo test");
        assert_eq!(restored.justification, "Need to verify");
        assert_eq!(restored.resource.as_deref(), Some("ShellExec"));
        assert_eq!(restored.target.as_deref(), Some("cargo test"));
    }

    #[test]
    fn round_trips_approval_decision() {
        round_trip(
            ItemKind::ApprovalDecision,
            serde_json::to_value(ApprovalDecisionPayload {
                approval_id: SmolStr::new("appr-1"),
                decision: "Allow".into(),
                scope: "Session".into(),
                decision_source: Some(ApprovalDecisionSource::User),
            })
            .expect("serialize payload"),
        );
    }

    #[test]
    fn assistant_message_projects_from_native_item() {
        let item = Item::AssistantMessage {
            text: "hi".into(),
            phase: None,
        };
        let (kind, payload) = legacy_wire_from_native_item(&item).expect("reverse");
        assert_eq!(kind, ItemKind::AgentMessage);
        assert_eq!(
            payload,
            serde_json::json!({ "title": "Assistant", "text": "hi" })
        );
    }

    #[test]
    fn user_message_projects_from_native_item() {
        let item = Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "hello".into(),
            }],
            entry: UserMessageEntry::TurnStart,
        };
        let (kind, payload) = legacy_wire_from_native_item(&item).expect("reverse");
        assert_eq!(kind, ItemKind::UserMessage);
        assert_eq!(payload["text"].as_str(), Some("hello"));
    }
}
