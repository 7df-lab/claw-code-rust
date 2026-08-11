use super::*;

const DEVO_TURN_DURATION_MS_META: &str = "devo/turnDurationMs";

pub(super) fn acp_update_from_history_item(
    index: usize,
    item: &SessionHistoryItem,
    parent_message_id: Option<&str>,
) -> Option<AcpSessionUpdate> {
    let meta = history_meta(index, parent_message_id);
    if let Some(SessionHistoryMetadata::PlanUpdate { steps, .. }) = &item.metadata {
        return Some(AcpSessionUpdate::Plan {
            entries: steps
                .iter()
                .map(|step| AcpPlanEntry {
                    content: step.text.clone(),
                    priority: AcpPlanEntryPriority::Medium,
                    status: match step.status {
                        SessionPlanStepStatus::Completed => AcpPlanEntryStatus::Completed,
                        SessionPlanStepStatus::InProgress => AcpPlanEntryStatus::InProgress,
                        SessionPlanStepStatus::Pending | SessionPlanStepStatus::Cancelled => {
                            AcpPlanEntryStatus::Pending
                        }
                    },
                })
                .collect(),
            meta: Some(meta),
        });
    }
    let content = AcpContentBlock::text(item.body.clone());
    let message_id = Some(format!("history-{index}"));
    match item.kind {
        SessionHistoryItemKind::User => Some(AcpSessionUpdate::UserMessageChunk {
            content,
            message_id,
            meta: Some(history_meta(index, None)),
        }),
        SessionHistoryItemKind::Assistant => Some(AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
            meta: Some(meta),
        }),
        SessionHistoryItemKind::Reasoning => Some(AcpSessionUpdate::AgentThoughtChunk {
            content,
            message_id,
            meta: Some(meta),
        }),
        SessionHistoryItemKind::TurnSummary => {
            let mut meta = meta;
            if let Some(duration_secs) = item.duration_ms {
                meta.insert(
                    DEVO_TURN_DURATION_MS_META.to_string(),
                    serde_json::json!(duration_secs.saturating_mul(1_000)),
                );
            }
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                meta: Some(meta),
            })
        }
        SessionHistoryItemKind::ToolCall => {
            let tool_call_id = history_tool_call_id(index, item);
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id,
                title: item.title.clone(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: Vec::new(),
                locations: Vec::new(),
                meta: Some(meta),
            })
        }
        SessionHistoryItemKind::ToolResult
        | SessionHistoryItemKind::CommandExecution
        | SessionHistoryItemKind::Error => {
            let tool_call_id = history_tool_call_id(index, item);
            let text = if item.body.is_empty() {
                item.title.clone()
            } else {
                item.body.clone()
            };
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title: Some(item.title.clone()),
                kind: None,
                status: Some(if item.kind == SessionHistoryItemKind::Error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: vec![AcpToolCallContent::content(AcpContentBlock::text(text))],
                locations: Vec::new(),
                meta: Some(meta),
            })
        }
        SessionHistoryItemKind::ContextCompaction => None,
    }
}

fn history_meta(index: usize, parent_message_id: Option<&str>) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_HISTORY_INDEX_META.to_string(),
        serde_json::json!(index),
    );
    if let Some(parent_message_id) = parent_message_id {
        meta.insert(
            DEVO_PARENT_MESSAGE_ID_META.to_string(),
            serde_json::Value::String(parent_message_id.to_string()),
        );
    }
    meta
}

fn history_tool_call_id(index: usize, item: &SessionHistoryItem) -> String {
    item.tool_call_id
        .clone()
        .unwrap_or_else(|| format!("history-{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn history_item(kind: SessionHistoryItemKind, title: &str, body: &str) -> SessionHistoryItem {
        SessionHistoryItem::new(None, kind, title.to_string(), body.to_string())
    }

    #[test]
    fn history_user_updates_include_stable_order_metadata_without_parent() {
        let item = history_item(SessionHistoryItemKind::User, "User", "hello");

        let update = acp_update_from_history_item(3, &item, None).expect("history update");

        let AcpSessionUpdate::UserMessageChunk {
            message_id, meta, ..
        } = update
        else {
            panic!("expected user message chunk");
        };
        assert_eq!(message_id, Some("history-3".to_string()));
        assert_eq!(meta, Some(history_meta(3, None)));
    }

    #[test]
    fn history_tool_updates_include_stable_order_and_parent_metadata() {
        let mut item = history_item(SessionHistoryItemKind::ToolCall, "Read", "");
        item.tool_call_id = Some("read-real-a".to_string());

        let update =
            acp_update_from_history_item(4, &item, Some("history-0")).expect("history update");

        let AcpSessionUpdate::ToolCall {
            tool_call_id, meta, ..
        } = update
        else {
            panic!("expected tool call");
        };
        assert_eq!(tool_call_id, "read-real-a");
        assert_eq!(meta, Some(history_meta(4, Some("history-0"))));
    }

    #[test]
    fn history_turn_summary_includes_duration_metadata() {
        let mut item = history_item(SessionHistoryItemKind::TurnSummary, "gpt-5", "");
        item.duration_ms = Some(42);

        let update =
            acp_update_from_history_item(5, &item, Some("history-0")).expect("history update");

        let AcpSessionUpdate::AgentThoughtChunk {
            message_id, meta, ..
        } = update
        else {
            panic!("expected agent thought chunk");
        };
        let mut expected = history_meta(5, Some("history-0"));
        expected.insert(
            DEVO_TURN_DURATION_MS_META.to_string(),
            serde_json::json!(42_000_u64),
        );
        assert_eq!(message_id, Some("history-5".to_string()));
        assert_eq!(meta, Some(expected));
    }
}
