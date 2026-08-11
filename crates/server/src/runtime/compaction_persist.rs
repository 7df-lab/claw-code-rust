//! Shared compaction summary + snapshot persistence helpers.
//!
//! Manual `/compact` and query-loop auto/proactive compaction both need to:
//! - derive preserved item ids from the compacted response suffix
//! - build a durable `ContextCompaction` summary item
//! - append a `CompactionSnapshot` so resume can rebuild `prompt_messages`

use std::sync::Arc;

use chrono::Utc;
use devo_core::CommandExecutionItem;
use devo_core::CompactionSnapshotLine;
use devo_core::ItemId;
use devo_core::Message;
use devo_core::ResponseItem;
use devo_core::SessionId;
use devo_core::SessionRecord;
use devo_core::TextItem;
use devo_core::ToolCallItem;
use devo_core::ToolResultItem;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnKind;
use devo_protocol::approx_tokens_from_byte_count;
use devo_protocol::canonical::item::ContextOccupancy;

use super::ServerRuntime;
use crate::execution::PersistedTurnItem;
use crate::persistence::RolloutStore;
use crate::persistence::build_item_record;
use crate::projection::history_item_from_turn_item;

/// Match the compacted preserve suffix against the prompt-visible journal tail.
pub(crate) fn preserved_item_ids_from_compacted(
    persisted_turn_items: &[PersistedTurnItem],
    compacted_items: &[ResponseItem],
) -> Vec<ItemId> {
    let mut normalized_persisted_items = Vec::new();
    for item in persisted_turn_items {
        if !crate::persistence::prompt_visible_persisted_turn_item(item) {
            continue;
        }

        // The compactor returns a summary followed by the prompt-visible suffix it
        // kept verbatim. Normalize persisted items into that same response shape
        // without allocating a short intermediate Vec for every journal item.
        match &item.turn_item {
            TurnItem::UserMessage(TextItem { text }) | TurnItem::SteerInput(TextItem { text }) => {
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::Message(Message::user(text.clone())),
                ));
            }
            TurnItem::AgentMessage(TextItem { text })
            | TurnItem::Plan(TextItem { text })
            | TurnItem::WebSearch(TextItem { text })
            | TurnItem::ImageGeneration(TextItem { text })
            | TurnItem::ContextCompaction(TextItem { text })
            | TurnItem::HookPrompt(TextItem { text }) => {
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::Message(Message::assistant_text(text.clone())),
                ));
            }
            TurnItem::Reasoning(_) => {}
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id,
                tool_name,
                input,
            }) => {
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::ToolCall {
                        id: tool_call_id.clone(),
                        name: tool_name.clone(),
                        input: input.clone(),
                    },
                ));
            }
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id,
                output,
                is_error,
                ..
            }) => {
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::ToolCallOutput {
                        tool_use_id: tool_call_id.clone(),
                        content: match output {
                            serde_json::Value::String(text) => text.clone(),
                            other => other.to_string(),
                        },
                        is_error: *is_error,
                    },
                ));
            }
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id,
                tool_name,
                input,
                output,
                is_error,
                ..
            }) => {
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::ToolCall {
                        id: tool_call_id.clone(),
                        name: tool_name.clone(),
                        input: input.clone(),
                    },
                ));
                normalized_persisted_items.push((
                    item.item_id,
                    ResponseItem::ToolCallOutput {
                        tool_use_id: tool_call_id.clone(),
                        content: match output {
                            serde_json::Value::String(text) => text.clone(),
                            other => other.to_string(),
                        },
                        is_error: *is_error,
                    },
                ));
            }
            TurnItem::ToolProgress(_)
            | TurnItem::ApprovalRequest(_)
            | TurnItem::ApprovalDecision(_)
            | TurnItem::TurnSummary(_) => {}
        }
    }
    let preserved = compacted_items.get(1..).unwrap_or(&[]);
    if preserved.is_empty() {
        return Vec::new();
    }
    let preserved_len = preserved.len();
    if normalized_persisted_items.len() < preserved_len {
        return Vec::new();
    }
    let suffix = &normalized_persisted_items[normalized_persisted_items.len() - preserved_len..];
    if suffix.iter().map(|(_, item)| item).eq(preserved.iter()) {
        suffix.iter().map(|(item_id, _)| *item_id).collect()
    } else {
        Vec::new()
    }
}

/// Build the durable summary turn item from compacted history.
pub(crate) fn summary_turn_item_from_compacted(compacted_items: &[ResponseItem]) -> TurnItem {
    let summary_text = compacted_items
        .first()
        .and_then(|item| match item {
            ResponseItem::Message(message) => {
                message.content.iter().find_map(|block| match block {
                    devo_core::ContentBlock::Text { text } => Some(text.clone()),
                    devo_core::ContentBlock::Reasoning { .. }
                    | devo_core::ContentBlock::ProviderReasoning { .. }
                    | devo_core::ContentBlock::ToolUse { .. }
                    | devo_core::ContentBlock::HostedToolUse { .. }
                    | devo_core::ContentBlock::ToolResult { .. } => None,
                })
            }
            ResponseItem::Reason { text } => Some(text.clone()),
            ResponseItem::ToolCall { .. } | ResponseItem::ToolCallOutput { .. } => None,
        })
        .unwrap_or_default();
    TurnItem::ContextCompaction(TextItem { text: summary_text })
}

/// Construct the rollout compaction snapshot line.
pub(crate) fn build_compaction_snapshot_line(
    session_id: SessionId,
    turn_id: TurnId,
    summary_item_id: ItemId,
    preserved_item_ids: Vec<ItemId>,
    context_occupancy: Option<ContextOccupancy>,
) -> CompactionSnapshotLine {
    CompactionSnapshotLine {
        timestamp: Utc::now(),
        session_id,
        turn_id,
        summary_item_id,
        preserved_item_ids,
        context_occupancy,
    }
}

/// Inputs needed to append a compaction summary item and its snapshot.
pub(crate) struct CompactionSummaryPersist {
    pub(crate) session_id: SessionId,
    pub(crate) turn_id: TurnId,
    pub(crate) summary_item_id: ItemId,
    pub(crate) item_seq: u64,
    pub(crate) summary_turn_item: TurnItem,
    pub(crate) snapshot: CompactionSnapshotLine,
}

/// Append the summary item and compaction snapshot to the durable rollout.
pub(crate) fn append_compaction_summary_and_snapshot(
    rollout_store: &RolloutStore,
    record: &SessionRecord,
    persist: CompactionSummaryPersist,
) {
    let CompactionSummaryPersist {
        session_id,
        turn_id,
        summary_item_id,
        item_seq,
        summary_turn_item,
        snapshot,
    } = persist;
    let item_record = build_item_record(
        session_id,
        turn_id,
        summary_item_id,
        item_seq,
        summary_turn_item,
        None,
        None,
    );
    if let Err(error) = rollout_store.append_item(record, item_record) {
        tracing::warn!(
            session_id = %session_id,
            error = %error,
            "failed to persist compaction summary item"
        );
    }
    if let Err(error) = rollout_store.append_compaction_snapshot(record, snapshot) {
        tracing::warn!(
            session_id = %session_id,
            error = %error,
            "failed to persist compaction snapshot"
        );
    }
}

/// Build the in-memory journal entry for a compaction summary item.
pub(crate) fn compaction_persisted_turn_item(
    turn_id: TurnId,
    turn_kind: TurnKind,
    item_id: ItemId,
    summary_turn_item: TurnItem,
) -> PersistedTurnItem {
    PersistedTurnItem {
        turn_id,
        turn_kind,
        item_id,
        turn_item: summary_turn_item,
    }
}

impl ServerRuntime {
    /// Persist an in-turn (auto/proactive) compaction summary + snapshot.
    ///
    /// Must not block on the session-actor mailbox: the actor is waiting on the
    /// turn event stream. Mutate inline scratch under the stream lock, then write
    /// rollout after releasing the lock.
    pub(crate) async fn persist_in_turn_compaction(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        summary_item_id: ItemId,
        compacted_items: &[ResponseItem],
    ) -> Option<u64> {
        let Some(stream) = self.active_stream_state(session_id).await else {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn_id,
                "in-turn compaction persist skipped: no active stream"
            );
            return None;
        };

        let spawn_stable_items = self
            .active_turns
            .spawn_snapshot_for_session(session_id)
            .await
            .map(|snapshot| snapshot.stable_items)
            .unwrap_or_default();

        let rollout = {
            let mut stream = stream.lock().await;
            let Some(inline) = stream.turn_inline.as_mut() else {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    "in-turn compaction persist skipped: no inline state"
                );
                return None;
            };
            if inline.turn_id != turn_id {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    inline_turn_id = %inline.turn_id,
                    "in-turn compaction persist skipped: turn mismatch"
                );
                return None;
            }

            let mut journal = spawn_stable_items;
            journal.extend(inline.persisted_turn_items.iter().cloned());
            let preserved_item_ids = preserved_item_ids_from_compacted(&journal, compacted_items);
            let summary_turn_item = summary_turn_item_from_compacted(compacted_items);

            let prompt_bytes = compacted_items
                .iter()
                .map(|item| serde_json::to_string(item).map_or(0, |json| json.len()))
                .sum::<usize>();
            let conversation_tokens = approx_tokens_from_byte_count(prompt_bytes);

            let global = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned")
                .effective_config()
                .compaction_token_limit;
            let model = inline
                .summary
                .model
                .as_deref()
                .and_then(|slug| {
                    inline
                        .hook_context
                        .runtime_context
                        .model_catalog
                        .get(slug)
                        .or_else(|| self.deps.model_catalog.get(slug))
                })
                .or_else(|| {
                    inline
                        .summary
                        .model_binding_id
                        .as_deref()
                        .and_then(|binding| {
                            inline
                                .hook_context
                                .runtime_context
                                .model_catalog
                                .get(binding)
                                .or_else(|| self.deps.model_catalog.get(binding))
                        })
                });
            let window = inline
                .summary
                .effective_context_window
                .or_else(|| {
                    model.map(|model| {
                        super::context_occupancy::resolved_compaction_limit(global, model)
                    })
                })
                .unwrap_or(0);
            let previous_occupancy = inline.summary.last_context_occupancy.clone();
            let occupancy = super::context_occupancy::occupancy_after_compaction(
                window,
                previous_occupancy.as_ref(),
                conversation_tokens,
                None,
            );
            inline.summary.last_context_occupancy = Some(occupancy.clone());
            inline.summary.last_query_total_tokens = occupancy.total_tokens as usize;
            inline.summary.prompt_token_estimate =
                conversation_tokens.try_into().unwrap_or(usize::MAX);

            let item_seq = inline.allocate_item_seq();
            let snapshot = build_compaction_snapshot_line(
                session_id,
                turn_id,
                summary_item_id,
                preserved_item_ids,
                Some(occupancy),
            );
            inline.latest_compaction_snapshot = Some(snapshot.clone());
            inline
                .persisted_turn_items
                .push(compaction_persisted_turn_item(
                    turn_id,
                    inline.turn_kind.clone(),
                    summary_item_id,
                    summary_turn_item.clone(),
                ));
            if let Some(history_item) = history_item_from_turn_item(&summary_turn_item) {
                inline.history_items.push(history_item);
            }

            inline
                .record
                .clone()
                .map(|record| (record, item_seq, summary_turn_item, snapshot))
        };

        let (record, item_seq, summary_turn_item, snapshot) = rollout?;
        append_compaction_summary_and_snapshot(
            &self.rollout_store,
            &record,
            CompactionSummaryPersist {
                session_id,
                turn_id,
                summary_item_id,
                item_seq,
                summary_turn_item,
                snapshot,
            },
        );
        Some(item_seq)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn preserved_item_ids_match_complete_command_execution_pair() {
        let command_item_id = ItemId::new();
        let command_input = serde_json::json!({ "cmd": "printf ok" });
        let command_output = serde_json::Value::String("ok".to_string());
        let persisted_turn_items = vec![PersistedTurnItem {
            turn_id: TurnId::new(),
            turn_kind: TurnKind::Regular,
            item_id: command_item_id,
            turn_item: TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: "call-1".to_string(),
                tool_name: "exec_command".to_string(),
                command: "printf ok".to_string(),
                input: command_input.clone(),
                output: command_output.clone(),
                is_error: false,
            }),
        }];
        let compacted_items = vec![
            ResponseItem::Message(Message::assistant_text("summary")),
            ResponseItem::ToolCall {
                id: "call-1".to_string(),
                name: "exec_command".to_string(),
                input: command_input,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
            },
        ];

        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![command_item_id, command_item_id]
        );
    }
}
