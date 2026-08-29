use std::sync::Arc;

use devo_core::tools::ToolContent;
use devo_core::{
    CommandExecutionItem, SessionId, TextItem, ToolCallItem, ToolResultItem, TurnId, TurnItem,
};
use devo_protocol::native::item::{
    ExecOrigin, ExecutionMode, FileChangeEntry, FileChangeKind, Item, PlanEntry, PlanStepStatus,
};
use devo_util_git::extract_paths_from_patch;

use super::super::*;
use super::tool_display::{command_actions_from_tool_result, is_file_change_tool, is_plan_tool};
use super::types::PendingToolCall;
use crate::{ItemKind, ToolCallPayload, TurnPlanStepPayload, TurnPlanUpdatedPayload};

pub(super) fn tool_content_to_json(content: ToolContent) -> serde_json::Value {
    match content {
        ToolContent::Text(text) => serde_json::Value::String(text),
        ToolContent::Json(json) => json,
        // Object metadata (shell_exec): keep the original object shape and only
        // fill `output` from Mixed text when the producer omitted the duplicate.
        ToolContent::Mixed {
            text: Some(text),
            json: Some(serde_json::Value::Object(mut map)),
        } => {
            map.entry("output".to_string())
                .or_insert_with(|| serde_json::Value::String(text));
            serde_json::Value::Object(map)
        }
        // Preserve arrays/scalars/etc. exactly (e.g. hosted web_search hits).
        ToolContent::Mixed {
            text: _,
            json: Some(json),
        } => json,
        ToolContent::Mixed {
            text: Some(text),
            json: None,
        } => serde_json::Value::String(text),
        ToolContent::Mixed {
            text: None,
            json: None,
        } => serde_json::Value::Null,
    }
}

/// Completes a pending tool-call item when the tool has a specialized item kind.
///
/// Returns `true` when the tool result item should not be emitted separately.
#[allow(clippy::too_many_arguments)]
pub(super) async fn complete_pending_tool_call(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    turn_for_plan_updates: &crate::TurnMetadata,
    tool_use_id: &str,
    tool_name: Option<String>,
    pending: &PendingToolCall,
    content: &ToolContent,
    display_content: Option<String>,
    is_error: bool,
    summary: &str,
) -> bool {
    let pending_item_id = pending.item_id.expect("pending item id");
    let pending_item_seq = pending.item_seq.expect("pending item seq");
    if let Some(ref tool_name) = tool_name {
        if is_plan_tool(tool_name) {
            complete_plan_tool_call(
                runtime,
                session_id,
                turn_id,
                turn_for_plan_updates,
                pending_item_id,
                pending_item_seq,
                content,
            )
            .await;
            return true;
        }
        if is_file_change_tool(tool_name) {
            complete_file_change_tool_call(
                runtime,
                session_id,
                turn_id,
                tool_use_id,
                tool_name,
                pending,
                content,
                display_content,
                is_error,
                pending_item_id,
                pending_item_seq,
            )
            .await;
            return true;
        }
        if pending.display_kind.is_command_execution() {
            complete_command_execution_tool_call(
                runtime,
                session_id,
                turn_id,
                tool_use_id,
                tool_name,
                pending,
                content,
                is_error,
                summary,
                pending_item_id,
                pending_item_seq,
            )
            .await;
            return true;
        }
    }
    complete_generic_tool_call(
        runtime,
        session_id,
        turn_id,
        tool_use_id,
        tool_name.unwrap_or_default(),
        pending,
        summary,
        pending_item_id,
        pending_item_seq,
    )
    .await;
    false
}

async fn complete_plan_tool_call(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    turn_for_plan_updates: &crate::TurnMetadata,
    pending_item_id: devo_core::ItemId,
    pending_item_seq: u64,
    content: &ToolContent,
) {
    let output_json = tool_content_to_json(content.clone());
    let explanation = output_json
        .get("explanation")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let plan = output_json
        .get("plan")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    runtime
        .complete_native_item(
            session_id,
            turn_id,
            pending_item_id,
            pending_item_seq,
            Item::Plan {
                entries: vec![PlanEntry {
                    step: output_json.to_string(),
                    status: PlanStepStatus::Completed,
                }],
            },
            TurnItem::Plan(TextItem {
                text: output_json.to_string(),
            }),
        )
        .await;
    runtime
        .broadcast_event(crate::ServerEvent::TurnPlanUpdated(
            TurnPlanUpdatedPayload {
                session_id,
                turn: turn_for_plan_updates.clone(),
                explanation,
                plan: plan
                    .into_iter()
                    .filter_map(|item| {
                        Some(TurnPlanStepPayload {
                            step: item.get("step")?.as_str()?.to_string(),
                            status: item.get("status")?.as_str()?.to_string(),
                        })
                    })
                    .collect(),
            },
        ))
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn complete_file_change_tool_call(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: &str,
    tool_name: &str,
    _pending: &PendingToolCall,
    content: &ToolContent,
    display_content: Option<String>,
    is_error: bool,
    pending_item_id: devo_core::ItemId,
    pending_item_seq: u64,
) {
    let output_json = tool_content_to_json(content.clone());
    let changes = file_changes_from_output(&output_json);
    runtime
        .complete_native_item(
            session_id,
            turn_id,
            pending_item_id,
            pending_item_seq,
            Item::FileChange {
                call_id: tool_use_id.to_string(),
                changes: legacy_file_changes_to_native(&changes),
                sandbox: None,
            },
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: tool_use_id.to_string(),
                tool_name: Some(tool_name.to_string()),
                output: output_json.clone(),
                display_content: display_content.clone(),
                is_error,
            }),
        )
        .await;
    runtime
        .persist_file_change_item(
            session_id,
            turn_id,
            pending_item_id,
            pending_item_seq,
            tool_use_id.to_string(),
            &changes,
        )
        .await;
}

fn file_changes_from_output(
    output_json: &serde_json::Value,
) -> Vec<(std::path::PathBuf, devo_protocol::protocol::FileChange)> {
    let changes = output_json
        .get("files")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| {
            let path = std::path::PathBuf::from(file.get("path")?.as_str()?);
            let kind = file.get("kind")?.as_str()?;
            let additions = file
                .get("additions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let deletions = file
                .get("deletions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let change = match kind {
                "add" => devo_protocol::protocol::FileChange::Add {
                    content: file
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| "\n".repeat(additions as usize)),
                },
                "delete" => devo_protocol::protocol::FileChange::Delete {
                    content: file
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| "\n".repeat(deletions as usize)),
                },
                "update" | "move" => devo_protocol::protocol::FileChange::Update {
                    unified_diff: file
                        .get("diff")
                        .or_else(|| file.get("patch"))
                        .or_else(|| output_json.get("diff"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    old_text: file
                        .get("oldContent")
                        .or_else(|| file.get("preContent"))
                        .or_else(|| file.get("pre_content"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    new_text: file
                        .get("postContent")
                        .or_else(|| file.get("post_content"))
                        .or_else(|| file.get("content"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    move_path: file
                        .get("movePath")
                        .or_else(|| file.get("move_path"))
                        .and_then(serde_json::Value::as_str)
                        .map(std::path::PathBuf::from),
                },
                _ => return None,
            };
            Some((path, change))
        })
        .collect::<Vec<_>>();
    if changes.is_empty() {
        output_json
            .get("diff")
            .and_then(serde_json::Value::as_str)
            .map(extract_paths_from_patch)
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                (
                    std::path::PathBuf::from(path),
                    devo_protocol::protocol::FileChange::Update {
                        unified_diff: output_json
                            .get("diff")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        old_text: None,
                        new_text: None,
                        move_path: None,
                    },
                )
            })
            .collect()
    } else {
        changes
    }
}

#[allow(clippy::too_many_arguments)]
async fn complete_command_execution_tool_call(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: &str,
    tool_name: &str,
    pending: &PendingToolCall,
    content: &ToolContent,
    is_error: bool,
    _summary: &str,
    pending_item_id: devo_core::ItemId,
    pending_item_seq: u64,
) {
    let output = tool_content_to_json(content.clone());
    runtime
        .complete_native_item(
            session_id,
            turn_id,
            pending_item_id,
            pending_item_seq,
            Item::CommandExecution {
                call_id: tool_use_id.to_string(),
                command: pending.command.clone(),
                argv: None,
                cwd: std::path::PathBuf::new(),
                input: Some(pending.input.clone()),
                output: Some(output.clone()),
                exit_code: None,
                execution_handle: None,
                is_error,
                execution_mode: ExecutionMode::Foreground,
                origin: ExecOrigin::AgentTool,
                sandbox: None,
            },
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: tool_use_id.to_string(),
                tool_name: tool_name.to_string(),
                command: pending.command.clone(),
                input: pending.input.clone(),
                output,
                is_error,
            }),
        )
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn complete_generic_tool_call(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: &str,
    tool_name: String,
    pending: &PendingToolCall,
    summary: &str,
    pending_item_id: devo_core::ItemId,
    pending_item_seq: u64,
) {
    let completed_payload = serde_json::to_value(ToolCallPayload {
        tool_call_id: tool_use_id.to_string(),
        tool_name: tool_name.clone(),
        parameters: pending.input.clone(),
        command_actions: command_actions_from_tool_result(
            tool_name.as_str(),
            &pending.command,
            &pending.input,
            summary,
        ),
    })
    .expect("serialize tool call payload");
    runtime
        .complete_item(
            session_id,
            turn_id,
            pending_item_id,
            pending_item_seq,
            ItemKind::ToolCall,
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id: tool_use_id.to_string(),
                tool_name,
                input: pending.input.clone(),
            }),
            completed_payload,
        )
        .await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn emit_tool_result_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: String,
    tool_name: Option<String>,
    _result_input: Option<serde_json::Value>,
    content: ToolContent,
    display_content: Option<String>,
    is_error: bool,
    _summary: String,
) {
    runtime
        .emit_turn_native_item(
            session_id,
            turn_id,
            Item::ToolResult {
                call_id: tool_use_id.clone(),
                output: tool_content_to_json(content.clone()),
                display_content: display_content.clone(),
                is_error,
                truncated: false,
            },
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: tool_use_id,
                tool_name: tool_name.clone(),
                output: tool_content_to_json(content),
                display_content,
                is_error,
            }),
        )
        .await;
}

fn legacy_file_changes_to_native(
    changes: &[(std::path::PathBuf, devo_protocol::protocol::FileChange)],
) -> Vec<FileChangeEntry> {
    changes
        .iter()
        .map(|(path, change)| FileChangeEntry {
            path: path.clone(),
            change: match change {
                devo_protocol::protocol::FileChange::Add { content } => FileChangeKind::Add {
                    content: content.clone(),
                },
                devo_protocol::protocol::FileChange::Delete { content } => FileChangeKind::Delete {
                    content: content.clone(),
                },
                devo_protocol::protocol::FileChange::Update {
                    unified_diff,
                    move_path,
                    ..
                } => FileChangeKind::Update {
                    unified_diff: unified_diff.clone(),
                    move_path: move_path.clone(),
                },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tool_content_to_json;
    use devo_core::tools::ToolContent;
    use pretty_assertions::assert_eq;

    #[test]
    fn mixed_object_fills_missing_output_from_text() {
        let content = ToolContent::Mixed {
            text: Some("hello\nworld".into()),
            json: Some(serde_json::json!({
                "command": "echo hello",
                "exit": 0,
            })),
        };
        assert_eq!(
            tool_content_to_json(content),
            serde_json::json!({
                "command": "echo hello",
                "exit": 0,
                "output": "hello\nworld",
            })
        );
    }

    #[test]
    fn mixed_object_preserves_existing_output() {
        let content = ToolContent::Mixed {
            text: Some("stream".into()),
            json: Some(serde_json::json!({
                "exit": 0,
                "output": "already set",
            })),
        };
        assert_eq!(
            tool_content_to_json(content),
            serde_json::json!({
                "exit": 0,
                "output": "already set",
            })
        );
    }

    #[test]
    fn mixed_array_json_preserves_original_shape() {
        let hits = serde_json::json!([
            {"title": "a", "url": "https://a.example"},
            {"title": "b", "url": "https://b.example"},
        ]);
        let content = ToolContent::Mixed {
            text: Some("search summary".into()),
            json: Some(hits.clone()),
        };
        assert_eq!(tool_content_to_json(content), hits);
    }

    #[test]
    fn mixed_webfetch_image_object_keeps_image_fields() {
        let content = ToolContent::Mixed {
            text: Some("Image fetched successfully".into()),
            json: Some(serde_json::json!({
                "title": "https://example.com/a.png (image/png)",
                "mime": "image/png",
                "image_base64": "abc123",
            })),
        };
        let json = tool_content_to_json(content);
        assert_eq!(json["image_base64"], "abc123");
        assert_eq!(json["mime"], "image/png");
        assert_eq!(json["output"], "Image fetched successfully");
    }
}
