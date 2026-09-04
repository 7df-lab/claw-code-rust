//! Native typed item event projection for the TUI worker (L2-DES-APP-009).
//!
//! Transcript items route through [`crate::worker::native_items`] into
//! [`crate::transcript::lifecycle::ItemLifecycleEvent`]. Session history restore
//! uses [`history_item_from_native_item`] and [`history_item_from_native_turn`].

use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionHistoryMetadata;
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::native::ids::SessionId as NativeSessionId;
    use devo_protocol::native::ids::TurnId as NativeTurnId;
    /// Verifies: native FileChange items project into lifecycle events.
    #[test]
    fn native_file_change_projects_to_lifecycle_event() {
        use std::path::PathBuf;

        use devo_protocol::native::item::FileChangeEntry;
        use devo_protocol::native::item::FileChangeKind;

        use crate::transcript::lifecycle::ItemLifecycleEvent;
        use crate::worker::native_items;

        let events = native_items::completed_events(
            &Item::FileChange {
                call_id: "edit-1".into(),
                changes: vec![FileChangeEntry {
                    path: PathBuf::from("src/main.rs"),
                    change: FileChangeKind::Update {
                        unified_diff: "@@ -1 +1 @@\n-old\n+new\n".into(),
                        move_path: None,
                    },
                }],
                sandbox: None,
            },
            devo_core::ItemId::new(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            ItemLifecycleEvent::ToolClosed {
                tool_use_id,
                file_changes: Some(changes),
                ..
            } => {
                assert_eq!(tool_use_id, "edit-1");
                assert_eq!(changes.len(), 1);
            }
            other => panic!("unexpected lifecycle event: {other:?}"),
        }
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
                variant: None,
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
