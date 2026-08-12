use std::path::PathBuf;

use chrono::Utc;

use super::ACP_SESSION_UPDATE_METHOD;
use super::AcpMeta;
use super::DEVO_ACTIVITY_AT_META;
use super::DEVO_ITEM_ID_META;
use super::DEVO_ORIGINAL_EVENT_META;
use super::DEVO_ORIGINAL_METHOD_META;
use super::DEVO_SESSION_META;
use super::DEVO_TURN_ID_META;
use super::DEVO_TURN_USAGE_META;
use super::content::*;
use super::session_update::*;
use crate::CommandExecutionPayload;
use crate::EventContext;
use crate::FileChangePayload;
use crate::ItemDeltaKind;
use crate::ItemEventPayload;
use crate::ItemKind;
use crate::ServerEvent;
use crate::SessionEventPayload;
use crate::ToolCallPayload;
use crate::ToolResultPayload;
use crate::TurnPlanStepPayload;

pub fn acp_notification_from_server_event(
    method: &str,
    event: &ServerEvent,
) -> (String, serde_json::Value) {
    let Some(session_id) = event.session_id() else {
        return (
            method.to_string(),
            serde_json::to_value(event).expect("serialize devo event"),
        );
    };
    let (update, meta) = if let Some(update) = acp_update_from_server_event(event) {
        // Tool item events keep the ACP surface for ACP clients, but also embed
        // the original server event so TUI/legacy clients can unwrap
        // `item/started` / `item/completed` with full ToolCallPayload
        // (parameters + command_actions) instead of the lossy ACP title.
        let meta = if should_preserve_original_tool_event(event) {
            Some(original_event_meta(method, event))
        } else {
            None
        };
        (update, meta)
    } else {
        (
            AcpSessionUpdate::SessionInfoUpdate {
                title: None,
                updated_at: None,
                meta: None,
            },
            Some(original_event_meta(method, event)),
        )
    };
    (
        ACP_SESSION_UPDATE_METHOD.to_string(),
        serde_json::to_value(AcpSessionNotification {
            session_id,
            update,
            meta,
        })
        .expect("serialize ACP session update"),
    )
}

fn original_event_meta(method: &str, event: &ServerEvent) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_ORIGINAL_METHOD_META.to_string(),
        serde_json::Value::String(method.to_string()),
    );
    meta.insert(
        DEVO_ORIGINAL_EVENT_META.to_string(),
        serde_json::to_value(event).expect("serialize original server event"),
    );
    meta
}

fn should_preserve_original_tool_event(event: &ServerEvent) -> bool {
    match event {
        ServerEvent::ItemStarted(payload) => matches!(
            payload.item.item_kind,
            ItemKind::ToolCall | ItemKind::CommandExecution
        ),
        ServerEvent::ItemCompleted(payload) => matches!(
            payload.item.item_kind,
            ItemKind::ToolCall
                | ItemKind::ToolResult
                | ItemKind::CommandExecution
                | ItemKind::FileChange
        ),
        // Keep status updates on the ACP surface; TUI ignores title-less status updates.
        _ => false,
    }
}

pub fn original_event_from_acp_notification(
    notification: &AcpSessionNotification,
) -> Option<(String, ServerEvent)> {
    let meta = notification.meta.as_ref()?;
    let method = meta.get(DEVO_ORIGINAL_METHOD_META)?.as_str()?.to_string();
    let event = serde_json::from_value(meta.get(DEVO_ORIGINAL_EVENT_META)?.clone()).ok()?;
    Some((method, event))
}

fn acp_meta_from_context(context: &EventContext) -> Option<AcpMeta> {
    let mut meta = AcpMeta::new();
    if let Some(turn_id) = &context.turn_id {
        meta.insert(
            DEVO_TURN_ID_META.to_string(),
            serde_json::Value::String(turn_id.to_string()),
        );
    }
    if let Some(item_id) = &context.item_id {
        meta.insert(
            DEVO_ITEM_ID_META.to_string(),
            serde_json::Value::String(item_id.to_string()),
        );
    }
    (!meta.is_empty()).then_some(meta)
}

fn add_activity_at(meta: &mut AcpMeta) {
    meta.insert(
        DEVO_ACTIVITY_AT_META.to_string(),
        serde_json::Value::String(Utc::now().to_rfc3339()),
    );
}

fn acp_activity_meta_from_context(context: &EventContext) -> AcpMeta {
    let mut meta = acp_meta_from_context(context).unwrap_or_default();
    add_activity_at(&mut meta);
    meta
}

fn acp_activity_meta_from_turn_id(turn_id: &crate::TurnId) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_TURN_ID_META.to_string(),
        serde_json::Value::String(turn_id.to_string()),
    );
    add_activity_at(&mut meta);
    meta
}

pub(crate) fn acp_update_from_server_event(event: &ServerEvent) -> Option<AcpSessionUpdate> {
    match event {
        ServerEvent::SessionStarted(SessionEventPayload { session })
        | ServerEvent::SessionTitleUpdated(SessionEventPayload { session }) => {
            let mut meta = AcpMeta::new();
            meta.insert(
                DEVO_SESSION_META.to_string(),
                serde_json::to_value(session).expect("serialize session metadata"),
            );
            Some(AcpSessionUpdate::SessionInfoUpdate {
                title: session.title.clone(),
                updated_at: Some(session.last_activity_at.to_rfc3339()),
                meta: Some(meta),
            })
        }
        ServerEvent::TurnPlanUpdated(payload) => Some(AcpSessionUpdate::Plan {
            entries: payload
                .plan
                .iter()
                .map(acp_plan_entry_from_turn_plan_step)
                .collect(),
            meta: None,
        }),
        ServerEvent::TurnUsageUpdated(payload) => {
            let used = (payload.total_input_tokens + payload.total_output_tokens) as u64;
            let mut meta = AcpMeta::new();
            meta.insert(
                DEVO_TURN_USAGE_META.to_string(),
                serde_json::to_value(payload).expect("serialize turn usage payload"),
            );
            Some(AcpSessionUpdate::UsageUpdate {
                used,
                size: payload.context_window.unwrap_or_else(|| used.max(1)),
                cost: None,
                meta: Some(meta),
            })
        }
        ServerEvent::ToolCallStatusUpdated(payload) => Some(AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: payload.tool_call_id.clone(),
            title: None,
            kind: None,
            status: acp_tool_call_status_from_str(payload.status.as_str()),
            raw_input: None,
            raw_output: None,
            content: None,
            locations: None,
            meta: Some(acp_activity_meta_from_turn_id(&payload.turn_id)),
        }),
        ServerEvent::ItemDelta {
            delta_kind,
            payload,
        } => acp_update_from_item_delta(delta_kind.clone(), payload),
        ServerEvent::ItemStarted(payload) => acp_update_from_item_started(payload),
        ServerEvent::ItemCompleted(payload) => acp_update_from_item_completed(payload),
        _ => None,
    }
}

fn acp_update_from_item_delta(
    delta_kind: ItemDeltaKind,
    payload: &crate::ItemDeltaPayload,
) -> Option<AcpSessionUpdate> {
    let content = AcpContentBlock::text(payload.delta.clone());
    let message_id = payload.context.item_id.map(|item_id| item_id.to_string());
    let meta = Some(acp_activity_meta_from_context(&payload.context));
    match delta_kind {
        ItemDeltaKind::AgentMessageDelta => Some(AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
            meta,
        }),
        ItemDeltaKind::ReasoningSummaryTextDelta | ItemDeltaKind::ReasoningTextDelta => {
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                meta,
            })
        }
        _ => None,
    }
}

fn acp_update_from_item_started(payload: &ItemEventPayload) -> Option<AcpSessionUpdate> {
    let meta = Some(acp_activity_meta_from_context(&payload.context));
    match payload.item.item_kind {
        ItemKind::ToolCall => {
            let tool =
                serde_json::from_value::<ToolCallPayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: tool.tool_call_id,
                title: tool_title(tool.tool_name.as_str(), &tool.parameters),
                kind: Some(tool_kind_from_name(tool.tool_name.as_str())),
                status: Some(AcpToolCallStatus::Pending),
                locations: tool_locations_from_value(&tool.parameters),
                raw_input: Some(tool.parameters),
                raw_output: None,
                content: Vec::new(),
                meta,
            })
        }
        ItemKind::CommandExecution => {
            let command =
                serde_json::from_value::<CommandExecutionPayload>(payload.item.payload.clone())
                    .ok()?;
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: command.tool_call_id,
                title: command.command,
                kind: Some(AcpToolKind::Execute),
                status: Some(AcpToolCallStatus::Pending),
                locations: command
                    .input
                    .as_ref()
                    .map(tool_locations_from_value)
                    .unwrap_or_default(),
                raw_input: command.input,
                raw_output: None,
                content: Vec::new(),
                meta,
            })
        }
        _ => None,
    }
}

fn acp_update_from_item_completed(payload: &ItemEventPayload) -> Option<AcpSessionUpdate> {
    let meta = Some(acp_activity_meta_from_context(&payload.context));
    match payload.item.item_kind {
        ItemKind::UserMessage => {
            let text = payload.item.payload.get("text")?.as_str()?.to_string();
            Some(AcpSessionUpdate::UserMessageChunk {
                content: AcpContentBlock::text(text),
                message_id: payload.context.item_id.map(|item_id| item_id.to_string()),
                meta,
            })
        }
        ItemKind::ToolResult => {
            let result =
                serde_json::from_value::<ToolResultPayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: result.tool_call_id,
                title: Some(
                    (!result.summary.is_empty())
                        .then_some(result.summary)
                        .or(result.tool_name.clone())
                        .unwrap_or_else(|| "Tool result".to_string()),
                ),
                kind: result.tool_name.as_deref().map(tool_kind_from_name),
                status: Some(if result.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: result.input.clone(),
                raw_output: Some(result.content.clone()),
                locations: Some(
                    result
                        .input
                        .as_ref()
                        .map(tool_locations_from_value)
                        .unwrap_or_default(),
                ),
                content: Some(tool_result_content(result.display_content, result.content)),
                meta,
            })
        }
        ItemKind::CommandExecution => {
            let command =
                serde_json::from_value::<CommandExecutionPayload>(payload.item.payload.clone())
                    .ok()?;
            let content = command
                .output
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .map(|text| vec![AcpToolCallContent::content(AcpContentBlock::text(text))]);
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: command.tool_call_id,
                title: Some(command.command),
                kind: Some(AcpToolKind::Execute),
                status: Some(if command.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: command.input,
                raw_output: command.output,
                content,
                locations: None,
                meta,
            })
        }
        ItemKind::FileChange => {
            let change =
                serde_json::from_value::<FileChangePayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: change.tool_call_id.clone(),
                title: change.tool_name.clone(),
                kind: Some(AcpToolKind::Edit),
                status: Some(if change.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: change.input.clone(),
                raw_output: Some(payload.item.payload.clone()),
                content: Some(file_change_tool_content(&change)),
                locations: Some(file_change_locations(&change)),
                meta,
            })
        }
        _ => None,
    }
}

fn acp_plan_entry_from_turn_plan_step(step: &TurnPlanStepPayload) -> AcpPlanEntry {
    AcpPlanEntry {
        content: step.step.clone(),
        priority: AcpPlanEntryPriority::Medium,
        status: match step.status.as_str() {
            "completed" => AcpPlanEntryStatus::Completed,
            "in_progress" => AcpPlanEntryStatus::InProgress,
            "pending" | "cancelled" => AcpPlanEntryStatus::Pending,
            _ => AcpPlanEntryStatus::Pending,
        },
        meta: None,
    }
}

fn tool_title(tool_name: &str, parameters: &serde_json::Value) -> String {
    if let Some(command) = parameters
        .get("command")
        .or_else(|| parameters.get("cmd"))
        .and_then(serde_json::Value::as_str)
    {
        return command.to_string();
    }
    if matches!(
        tool_name,
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url"
    ) && let Some(url) = parameters
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty())
    {
        return format!("web_fetch: {url}");
    }
    if matches!(tool_name, "web_search" | "websearch" | "web-search")
        && let Some(query) = parameters
            .get("query")
            .and_then(serde_json::Value::as_str)
            .filter(|query| !query.is_empty())
    {
        return format!("web_search: {query}");
    }
    if tool_name == "spawn_agent"
        && let Some(message) = parameters
            .get("message")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
    {
        return format!("spawn_agent: {message}");
    }
    tool_name.to_string()
}

fn tool_kind_from_name(tool_name: &str) -> AcpToolKind {
    match tool_name {
        "read" | "grep" | "glob" | "lsp" => AcpToolKind::Read,
        "apply_patch" | "edit" | "write" => AcpToolKind::Edit,
        "bash" | "shell_command" | "exec_command" => AcpToolKind::Execute,
        "web_search" | "websearch" | "web_fetch" | "webfetch" | "web-fetch" | "fetch_url"
        | "fetch-url" | "websearch_query" => AcpToolKind::Fetch,
        "agent" => AcpToolKind::Think,
        _ => AcpToolKind::Other,
    }
}

fn acp_tool_call_status_from_str(status: &str) -> Option<AcpToolCallStatus> {
    Some(match status {
        "pending" => AcpToolCallStatus::Pending,
        "in_progress" => AcpToolCallStatus::InProgress,
        "completed" => AcpToolCallStatus::Completed,
        "failed" => AcpToolCallStatus::Failed,
        // ACP v1 has no cancelled tool-call status. Cancellation is represented
        // by the surrounding session/turn lifecycle, so do not emit an invalid
        // enum value to ACP clients.
        "cancelled" => return None,
        _ => return None,
    })
}

pub(crate) fn file_change_tool_content(change: &FileChangePayload) -> Vec<AcpToolCallContent> {
    change
        .changes
        .iter()
        .map(|(path, change)| match change {
            crate::protocol::FileChange::Add { content } if path.is_absolute() => {
                AcpToolCallContent::Diff {
                    path: path.clone(),
                    old_text: None,
                    new_text: content.clone(),
                    meta: None,
                }
            }
            crate::protocol::FileChange::Delete { content } if path.is_absolute() => {
                AcpToolCallContent::Diff {
                    path: path.clone(),
                    old_text: Some(content.clone()),
                    new_text: String::new(),
                    meta: None,
                }
            }
            crate::protocol::FileChange::Update {
                unified_diff,
                old_text,
                new_text,
                ..
            } if path.is_absolute() => {
                if let (Some(old_text), Some(new_text)) = (old_text, new_text) {
                    AcpToolCallContent::Diff {
                        path: path.clone(),
                        old_text: Some(old_text.clone()),
                        new_text: new_text.clone(),
                        meta: None,
                    }
                } else {
                    AcpToolCallContent::content(AcpContentBlock::text(unified_diff.clone()))
                }
            }
            crate::protocol::FileChange::Add { content }
            | crate::protocol::FileChange::Delete { content } => {
                AcpToolCallContent::content(AcpContentBlock::text(content.clone()))
            }
            crate::protocol::FileChange::Update { unified_diff, .. } => {
                AcpToolCallContent::content(AcpContentBlock::text(unified_diff.clone()))
            }
        })
        .collect()
}

fn file_change_locations(change: &FileChangePayload) -> Vec<AcpToolCallLocation> {
    change
        .changes
        .iter()
        .filter_map(|(path, _)| {
            path.is_absolute().then_some(AcpToolCallLocation {
                path: path.clone(),
                line: None,
                meta: None,
            })
        })
        .collect()
}

fn tool_locations_from_value(value: &serde_json::Value) -> Vec<AcpToolCallLocation> {
    let mut locations = Vec::new();
    for key in ["path", "filePath", "file_path"] {
        if let Some(path) = value.get(key).and_then(serde_json::Value::as_str) {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                locations.push(AcpToolCallLocation {
                    path,
                    line: value
                        .get("line")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|line| u32::try_from(line).ok()),
                    meta: None,
                });
            }
        }
    }
    for key in ["paths", "files"] {
        if let Some(items) = value.get(key).and_then(serde_json::Value::as_array) {
            for item in items {
                if let Some(path) = item.as_str() {
                    let path = PathBuf::from(path);
                    if path.is_absolute() {
                        locations.push(AcpToolCallLocation {
                            path,
                            line: None,
                            meta: None,
                        });
                    }
                } else {
                    push_location_from_object(item, &mut locations);
                }
            }
        }
    }
    locations
}

fn push_location_from_object(value: &serde_json::Value, locations: &mut Vec<AcpToolCallLocation>) {
    let Some(object) = value.as_object() else {
        return;
    };
    let path = object
        .get("path")
        .or_else(|| object.get("filePath"))
        .or_else(|| object.get("file_path"))
        .and_then(serde_json::Value::as_str);
    if let Some(path) = path {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            locations.push(AcpToolCallLocation {
                path,
                line: object
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|line| u32::try_from(line).ok()),
                meta: None,
            });
        }
    }
}

pub(crate) fn tool_result_content(
    display_content: Option<String>,
    content: serde_json::Value,
) -> Vec<AcpToolCallContent> {
    if let Some(display_content) = display_content {
        return vec![AcpToolCallContent::content(AcpContentBlock::text(
            display_content,
        ))];
    }

    if let Some(content) = acp_tool_content_from_value(&content) {
        return content;
    }

    let text = match content {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    vec![AcpToolCallContent::content(AcpContentBlock::text(text))]
}

fn acp_tool_content_from_value(value: &serde_json::Value) -> Option<Vec<AcpToolCallContent>> {
    if let Ok(content) = serde_json::from_value::<AcpToolCallContent>(value.clone()) {
        return Some(vec![content]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpToolCallContent>>(value.clone()) {
        return Some(contents);
    }

    if let Ok(content) = serde_json::from_value::<AcpContentBlock>(value.clone()) {
        return Some(vec![AcpToolCallContent::content(content)]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpContentBlock>>(value.clone()) {
        return Some(
            contents
                .into_iter()
                .map(AcpToolCallContent::content)
                .collect(),
        );
    }

    let mcp_contents = value.get("content")?;
    if let Ok(contents) = serde_json::from_value::<Vec<AcpToolCallContent>>(mcp_contents.clone()) {
        return Some(contents);
    }
    let contents = serde_json::from_value::<Vec<AcpContentBlock>>(mcp_contents.clone()).ok()?;
    Some(
        contents
            .into_iter()
            .map(AcpToolCallContent::content)
            .collect(),
    )
}
