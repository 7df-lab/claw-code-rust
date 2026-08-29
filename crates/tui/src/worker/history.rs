//! Session history projection for restore and legacy transcript items.

use std::collections::HashMap;

use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::SessionPlanStepStatus;

use crate::events::TranscriptItem;
use crate::events::TranscriptItemKind;

use super::typed_events;

pub(crate) fn project_history_items(items: &[SessionHistoryItem]) -> Vec<TranscriptItem> {
    use std::collections::{HashMap, HashSet};

    let mut paired_result_by_call_id = HashMap::new();
    let mut consumed_result_indexes = HashSet::new();

    for (index, item) in items.iter().enumerate() {
        if matches!(
            item.kind,
            SessionHistoryItemKind::ToolResult | SessionHistoryItemKind::Error
        ) && let Some(tool_call_id) = item.tool_call_id.as_deref()
        {
            paired_result_by_call_id
                .entry(tool_call_id.to_string())
                .or_insert(index);
        }
    }

    let metadata_owned_ids = items
        .iter()
        .filter_map(|item| {
            item.tool_call_id
                .clone()
                .filter(|_| item.metadata.is_some())
        })
        .collect::<HashSet<_>>();
    let mut transcript = Vec::new();
    let mut index = 0usize;

    while index < items.len() {
        let item = &items[index];
        if let Some(metadata) = &item.metadata {
            if let Some(tool_call_id) = item.tool_call_id.as_deref()
                && let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied()
                && result_index != index
            {
                consumed_result_indexes.insert(result_index);
            }
            match metadata {
                SessionHistoryMetadata::PlanUpdate { explanation, steps } => {
                    transcript.push(TranscriptItem::new(
                        TranscriptItemKind::System,
                        explanation.clone().unwrap_or_default(),
                        steps
                            .iter()
                            .map(|step| {
                                let status = match step.status {
                                    SessionPlanStepStatus::Pending => "pending",
                                    SessionPlanStepStatus::InProgress => "in_progress",
                                    SessionPlanStepStatus::Completed => "completed",
                                    SessionPlanStepStatus::Cancelled => "cancelled",
                                };
                                format!("{status}: {}", step.text)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ));
                    index += 1;
                    continue;
                }
                SessionHistoryMetadata::ProposedPlan => {
                    transcript.push(TranscriptItem::new(
                        TranscriptItemKind::Assistant,
                        "Proposed Plan".to_string(),
                        item.body.clone(),
                    ));
                    index += 1;
                    continue;
                }
                SessionHistoryMetadata::TurnSummary { .. }
                | SessionHistoryMetadata::Edited { .. } => {}
                SessionHistoryMetadata::Explored { actions } => {
                    let title = item.title.clone();
                    let body = actions
                        .iter()
                        .map(|action| format!("{action:?}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    transcript.push(TranscriptItem::restored_tool_result(title, body));
                    index += 1;
                    continue;
                }
            }
        }
        if item.kind == SessionHistoryItemKind::ToolCall
            && let Some(tool_call_id) = item.tool_call_id.as_deref()
        {
            if metadata_owned_ids.contains(tool_call_id) {
                index += 1;
                continue;
            }
            if let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied() {
                let result_item = &items[result_index];
                consumed_result_indexes.insert(result_index);
                let mut ti = if result_item.kind == SessionHistoryItemKind::Error {
                    TranscriptItem::tool_error(item.title.clone(), result_item.body.clone())
                } else {
                    TranscriptItem::restored_tool_result(
                        item.title.clone(),
                        result_item.body.clone(),
                    )
                };
                if let Some(duration_ms) = result_item.duration_ms {
                    ti = ti.with_duration(duration_ms);
                }
                transcript.push(ti);
                index += 1;
                continue;
            }
        }

        if consumed_result_indexes.contains(&index) {
            index += 1;
            continue;
        }

        let kind = match item.kind {
            SessionHistoryItemKind::User => TranscriptItemKind::User,
            SessionHistoryItemKind::Assistant => TranscriptItemKind::Assistant,
            SessionHistoryItemKind::Reasoning => TranscriptItemKind::Reasoning,
            SessionHistoryItemKind::ToolCall => TranscriptItemKind::ToolCall,
            SessionHistoryItemKind::ToolResult => TranscriptItemKind::ToolResult,
            SessionHistoryItemKind::CommandExecution => TranscriptItemKind::ToolResult,
            SessionHistoryItemKind::Error => TranscriptItemKind::Error,
            SessionHistoryItemKind::TurnSummary => TranscriptItemKind::TurnSummary,
            SessionHistoryItemKind::ContextCompaction => TranscriptItemKind::System,
        };
        let mut transcript_item = match item.kind {
            SessionHistoryItemKind::ToolCall => TranscriptItem::tool_call(item.title.clone()),
            SessionHistoryItemKind::ToolResult => {
                TranscriptItem::restored_tool_result(item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::CommandExecution => {
                TranscriptItem::restored_tool_result(item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::Error => {
                if item.tool_call_id.is_some() {
                    TranscriptItem::tool_error(item.title.clone(), item.body.clone())
                } else {
                    TranscriptItem::new(kind, String::new(), item.body.clone())
                }
            }
            SessionHistoryItemKind::TurnSummary => {
                // TurnSummary uses title for model name, duration_ms for duration in seconds
                TranscriptItem::new(kind, item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::ContextCompaction => {
                let title = if item.title.is_empty() {
                    "Context compacted".to_string()
                } else {
                    item.title.clone()
                };
                TranscriptItem::new(kind, title, String::new())
            }
            SessionHistoryItemKind::User
            | SessionHistoryItemKind::Assistant
            | SessionHistoryItemKind::Reasoning => {
                TranscriptItem::new(kind, item.title.clone(), item.body.clone())
            }
        };
        if let Some(duration_ms) = item.duration_ms {
            transcript_item = transcript_item.with_duration(duration_ms);
        }
        transcript.push(transcript_item);
        index += 1;
    }

    transcript
}
pub(crate) fn restored_history_items(
    turns: Vec<devo_protocol::native::turn::Turn>,
    items: Vec<devo_protocol::native::item::ItemEnvelope>,
    fallback_mode: devo_protocol::CollaborationMode,
) -> Vec<devo_protocol::SessionHistoryItem> {
    let mut items_by_turn = HashMap::<String, Vec<_>>::new();
    for item in items {
        items_by_turn
            .entry(item.turn_id.as_str().to_string())
            .or_default()
            .push(item);
    }
    let mut history_items = Vec::new();
    for turn in &turns {
        if let Some(turn_items) = items_by_turn.remove(turn.id.as_str()) {
            history_items.extend(
                turn_items
                    .iter()
                    .filter_map(typed_events::history_item_from_native_item),
            );
        }
        if let Some(summary) = typed_events::history_item_from_native_turn(turn, fallback_mode) {
            history_items.push(summary);
        }
    }
    let mut orphan_items = items_by_turn.into_values().flatten().collect::<Vec<_>>();
    orphan_items.sort_by_key(|item| item.seq);
    history_items.extend(
        orphan_items
            .iter()
            .filter_map(typed_events::history_item_from_native_item),
    );
    history_items
}
