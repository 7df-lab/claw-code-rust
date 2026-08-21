//! Native typed item events → legacy item payload conversion
//! (L2-DES-APP-009 DD-5). The existing `handle_started_item` /
//! `handle_completed_item` handlers consume the converted payloads
//! unchanged, so typed and legacy shapes render identically.

use devo_protocol::ItemEnvelope as LegacyItemEnvelope;
use devo_protocol::ItemEventPayload;
use devo_protocol::ItemKind;
use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::TypedItemEventPayload;
use devo_protocol::native::item::Item;
use devo_protocol::native::turn::Turn;
use devo_protocol::native::turn::TurnStatus;

/// Converts one canonical history item (from `session/items/list`) into the
/// legacy display model used for transcript restore (L2-DES-APP-008 Phase
/// C). The mapping is intentionally display-oriented: replay-only variants
/// yield `None`, and tool metadata that the canonical item does not carry
/// (parsed command actions, sandbox summaries) is left empty.
pub(super) fn history_item_from_native_item(
    item: &devo_protocol::native::item::ItemEnvelope,
) -> Option<SessionHistoryItem> {
    let item_id = item.id.as_str().to_string();
    let base = |kind: SessionHistoryItemKind, title: &str, body: String| {
        SessionHistoryItem::new(Some(item_id.clone()), kind, title.to_string(), body)
    };
    Some(match &item.item {
        Item::UserMessage { content, .. } => base(
            SessionHistoryItemKind::User,
            "User",
            content
                .iter()
                .filter_map(|input| match input {
                    devo_protocol::native::item::UserInput::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Item::AssistantMessage { text, .. } => {
            base(SessionHistoryItemKind::Assistant, "Assistant", text.clone())
        }
        Item::Reasoning { text, .. } => {
            base(SessionHistoryItemKind::Reasoning, "Reasoning", text.clone())
        }
        Item::Plan { entries } => {
            let mut history = base(
                SessionHistoryItemKind::Assistant,
                "Proposed Plan",
                entries
                    .iter()
                    .map(|entry| format!("- {}", entry.step))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            history = history.with_metadata(SessionHistoryMetadata::ProposedPlan);
            history
        }
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let mut history = SessionHistoryItem::new(
                Some(call_id.clone()),
                SessionHistoryItemKind::ToolCall,
                tool_name.clone(),
                String::new(),
            );
            history.tool_io = Some(devo_protocol::SessionHistoryToolIo {
                tool_name: tool_name.clone(),
                input: input.clone().unwrap_or(serde_json::Value::Null),
                output: None,
                display_content: None,
            });
            history
        }
        Item::ToolResult {
            call_id,
            output,
            display_content,
            ..
        } => {
            let mut history = SessionHistoryItem::new(
                Some(call_id.clone()),
                SessionHistoryItemKind::ToolResult,
                String::new(),
                display_content
                    .clone()
                    .unwrap_or_else(|| serde_json::to_string_pretty(output).unwrap_or_default()),
            );
            history.tool_io = Some(devo_protocol::SessionHistoryToolIo {
                tool_name: String::new(),
                input: serde_json::Value::Null,
                output: Some(output.clone()),
                display_content: display_content.clone(),
            });
            history
        }
        Item::CommandExecution {
            call_id,
            command,
            output,
            exit_code,
            ..
        } => {
            let mut history = SessionHistoryItem::new(
                Some(call_id.clone()),
                SessionHistoryItemKind::CommandExecution,
                command.clone(),
                output
                    .as_ref()
                    .map(|output| serde_json::to_string_pretty(output).unwrap_or_default())
                    .unwrap_or_default(),
            );
            history.tool_io = Some(devo_protocol::SessionHistoryToolIo {
                tool_name: "exec_command".to_string(),
                input: serde_json::json!({ "command": command }),
                output: output
                    .clone()
                    .or_else(|| exit_code.map(|code| serde_json::json!({ "exit_code": code }))),
                display_content: None,
            });
            history
        }
        Item::ContextCompaction { summary, .. } => base(
            SessionHistoryItemKind::ContextCompaction,
            "Context compacted",
            summary.clone().unwrap_or_default(),
        ),
        Item::HostedToolCall { .. }
        | Item::FileChange { .. }
        | Item::Approval { .. }
        | Item::UserInputRequest { .. }
        | Item::SubAgent { .. }
        | Item::BackgroundTask { .. }
        | Item::GoalProgress { .. }
        | Item::Warning { .. } => return None,
    })
}

/// Reconstructs the end-of-turn display row that is stored as rollout-only
/// metadata and therefore is not returned by `session/items/list`.
pub(super) fn history_item_from_native_turn(
    turn: &Turn,
    fallback_mode: devo_protocol::CollaborationMode,
) -> Option<SessionHistoryItem> {
    let body = match turn.status {
        TurnStatus::InProgress => return None,
        TurnStatus::Completed => String::new(),
        TurnStatus::Interrupted => "interrupted".to_string(),
        TurnStatus::Failed => "failed".to_string(),
    };
    let duration = turn.completed_at.and_then(|completed_at| {
        let seconds = completed_at
            .signed_duration_since(turn.started_at)
            .num_seconds();
        (seconds > 0).then_some(seconds as u64)
    });
    Some(SessionHistoryItem {
        tool_call_id: None,
        kind: SessionHistoryItemKind::TurnSummary,
        title: turn.model.model.clone(),
        body,
        tool_io: None,
        metadata: Some(SessionHistoryMetadata::TurnSummary {
            collaboration_mode: turn.collaboration_mode.unwrap_or(fallback_mode),
        }),
        duration_ms: duration,
    })
}

/// Converts a canonical typed item event into the legacy item-event shape
/// understood by the TUI's item handlers. Variants the TUI does not render
/// (approvals, hosted tools, background work, warnings, ...) yield `None`,
/// matching the legacy handler's ignore list.
pub(super) fn legacy_item_event_from_typed(
    payload: &TypedItemEventPayload,
) -> Option<ItemEventPayload> {
    let item_id = devo_protocol::ItemId::try_from(payload.item.id.as_str()).ok()?;
    let (item_kind, legacy_payload) = match &payload.item.item {
        Item::AssistantMessage { text, .. } => (
            ItemKind::AgentMessage,
            serde_json::json!({ "title": "Assistant", "text": text }),
        ),
        Item::Reasoning { text, .. } => (
            ItemKind::Reasoning,
            serde_json::json!({ "title": "Reasoning", "text": text }),
        ),
        Item::Plan { entries } => (
            ItemKind::Plan,
            serde_json::json!({
                "title": "Proposed Plan",
                "text": serde_json::to_value(entries).expect("serialize plan entries"),
            }),
        ),
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => (
            ItemKind::ToolCall,
            serde_json::to_value(devo_protocol::ToolCallPayload {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                parameters: input.clone().unwrap_or(serde_json::Value::Null),
                command_actions: Vec::new(),
            })
            .expect("serialize legacy tool call payload"),
        ),
        Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            ..
        } => (
            ItemKind::ToolResult,
            serde_json::to_value(devo_protocol::ToolResultPayload {
                tool_call_id: call_id.clone(),
                tool_name: None,
                input: None,
                content: output.clone(),
                display_content: display_content.clone(),
                is_error: *is_error,
                summary: String::new(),
            })
            .expect("serialize legacy tool result payload"),
        ),
        Item::CommandExecution {
            call_id,
            command,
            input,
            output,
            is_error,
            ..
        } => (
            ItemKind::CommandExecution,
            serde_json::to_value(devo_protocol::CommandExecutionPayload {
                tool_call_id: call_id.clone(),
                tool_name: "exec_command".to_string(),
                command: command.clone(),
                input: input.clone(),
                source: Default::default(),
                command_actions: Vec::new(),
                output: output.clone(),
                is_error: *is_error,
            })
            .expect("serialize legacy command execution payload"),
        ),
        Item::ContextCompaction { .. } => (ItemKind::ContextCompaction, serde_json::json!({})),
        Item::UserMessage { .. }
        | Item::HostedToolCall { .. }
        | Item::FileChange { .. }
        | Item::Approval { .. }
        | Item::UserInputRequest { .. }
        | Item::SubAgent { .. }
        | Item::BackgroundTask { .. }
        | Item::GoalProgress { .. }
        | Item::Warning { .. } => return None,
    };
    Some(ItemEventPayload {
        context: payload.context.clone(),
        item: LegacyItemEnvelope {
            item_id,
            item_kind,
            payload: legacy_payload,
        },
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::EventContext;
    use devo_protocol::native::ids::ItemId as NativeItemId;
    use devo_protocol::native::ids::SessionId as NativeSessionId;
    use devo_protocol::native::ids::TurnId as NativeTurnId;
    use devo_protocol::native::item::ItemEnvelope as NativeItemEnvelope;
    use devo_protocol::native::item::ItemState;

    fn typed_payload(item: Item) -> TypedItemEventPayload {
        let item_id = devo_protocol::ItemId::new();
        TypedItemEventPayload {
            context: EventContext {
                session_id: devo_protocol::SessionId::new(),
                turn_id: Some(devo_protocol::TurnId::new()),
                item_id: Some(item_id),
                seq: 0,
                item_seq: None,
            },
            item: NativeItemEnvelope {
                id: NativeItemId::from_legacy_uuid(item_id.into()),
                session_id: NativeSessionId::from_legacy_uuid(
                    devo_protocol::SessionId::new().into(),
                ),
                turn_id: NativeTurnId::from_legacy_uuid(devo_protocol::TurnId::new().into()),
                seq: 1,
                revision: 1,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                state: ItemState::Running,
                item,
            },
        }
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: typed assistant/tool-call items convert into the legacy
    /// payload shapes the TUI handlers render.
    #[test]
    fn typed_items_convert_to_legacy_payloads() {
        let legacy = legacy_item_event_from_typed(&typed_payload(Item::AssistantMessage {
            text: "hello".into(),
            phase: None,
        }))
        .expect("assistant message converts");
        assert_eq!(legacy.item.item_kind, ItemKind::AgentMessage);
        assert_eq!(
            legacy.item.payload.get("text").and_then(|v| v.as_str()),
            Some("hello")
        );

        let legacy = legacy_item_event_from_typed(&typed_payload(Item::ToolCall {
            call_id: "call-1".into(),
            tool_name: "exec_command".into(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: Some(serde_json::json!({ "cmd": "ls" })),
        }))
        .expect("tool call converts");
        assert_eq!(legacy.item.item_kind, ItemKind::ToolCall);
        let payload: devo_protocol::ToolCallPayload =
            serde_json::from_value(legacy.item.payload).expect("legacy tool call payload");
        assert_eq!(payload.tool_call_id, "call-1");
        assert_eq!(payload.tool_name, "exec_command");
        assert_eq!(payload.parameters, serde_json::json!({ "cmd": "ls" }));
    }

    #[test]
    fn native_turn_restores_plan_summary_row() {
        let started_at = chrono::Utc::now();
        let turn = Turn {
            id: NativeTurnId::from_legacy_uuid(devo_protocol::TurnId::new().into()),
            session_id: NativeSessionId::from_legacy_uuid(devo_protocol::SessionId::new().into()),
            sequence: 1,
            kind: devo_protocol::native::turn::TurnKind::Regular,
            status: TurnStatus::Completed,
            model: devo_protocol::native::model::ModelBinding {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                reasoning_effort: None,
            },
            collaboration_mode: Some(devo_protocol::CollaborationMode::Plan),
            started_at,
            completed_at: Some(started_at + chrono::Duration::seconds(65)),
            error: None,
            usage: None,
        };

        assert_eq!(
            history_item_from_native_turn(&turn, devo_protocol::CollaborationMode::Build),
            Some(SessionHistoryItem {
                tool_call_id: None,
                kind: SessionHistoryItemKind::TurnSummary,
                title: "test-model".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: Some(SessionHistoryMetadata::TurnSummary {
                    collaboration_mode: devo_protocol::CollaborationMode::Plan,
                }),
                duration_ms: Some(65),
            })
        );
    }
}
