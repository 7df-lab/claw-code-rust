//! Restored-session transcript reconstruction for `ChatWidget`.
//!
//! Session resume can provide rich protocol history or older transcript items;
//! this module rebuilds the visible history cells for those restored sessions.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::bottom_pane::InputMode;
use crate::events::TranscriptItem;
use crate::history_cell;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::restore_session;
use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::SessionPlanStepStatus;
use serde_json::Value;

use super::ChatWidget;

impl ChatWidget {
    pub(super) fn rebuild_restored_session_history(
        &mut self,
        history_items: Vec<TranscriptItem>,
        loaded_item_count: u64,
        session_id: &str,
        title: Option<&str>,
    ) {
        self.history.clear();
        self.next_history_flush_index = 0;
        self.transcript_projector = crate::transcript::TranscriptProjector::default();
        self.active_tool_calls.clear();
        self.pending_tool_calls.clear();

        tracing::trace!(
            session_id,
            loaded_item_count,
            restored_items = history_items.len(),
            restored_preview = ?history_items
                .iter()
                .take(10)
                .map(|item| (format!("{:?}", item.kind), item.title.clone()))
                .collect::<Vec<_>>(),
            synthetic_header_inserted = true,
            "rebuilding restored session transcript"
        );

        let loaded_any_history = !history_items.is_empty();
        for item in &history_items {
            self.add_transcript_item_without_redraw(item.clone());
        }

        if !loaded_any_history {
            self.add_history_entry_without_redraw(Box::new(history_cell::new_info_event(
                format!(
                    "switched to {session_id}; title: {}; loaded items: {loaded_item_count}",
                    title.unwrap_or("(untitled)")
                ),
                None,
            )));
        }
        self.frame_requester.schedule_frame();
    }

    pub(super) fn rebuild_restored_session_history_from_rich_items(
        &mut self,
        history_items: &[SessionHistoryItem],
        loaded_item_count: u64,
        session_id: &str,
        title: Option<&str>,
    ) -> bool {
        self.history.clear();
        self.next_history_flush_index = 0;
        self.transcript_projector = crate::transcript::TranscriptProjector::default();
        self.active_tool_calls.clear();
        self.pending_tool_calls.clear();

        if history_items.is_empty() {
            self.add_history_entry_without_redraw(Box::new(history_cell::new_info_event(
                format!(
                    "switched to {session_id}; title: {}; loaded items: {loaded_item_count}",
                    title.unwrap_or("(untitled)")
                ),
                None,
            )));
            self.frame_requester.schedule_frame();
            return false;
        }

        let mut paired_result_by_call_id = HashMap::new();
        for (index, item) in history_items.iter().enumerate() {
            if matches!(
                item.kind,
                devo_protocol::SessionHistoryItemKind::ToolResult
                    | devo_protocol::SessionHistoryItemKind::Error
            ) && let Some(tool_call_id) = item.tool_call_id.as_deref()
            {
                paired_result_by_call_id
                    .entry(tool_call_id.to_string())
                    .or_insert(index);
            }
        }

        let metadata_owned_ids: HashSet<String> = history_items
            .iter()
            .filter_map(|item| {
                item.tool_call_id
                    .clone()
                    .filter(|_| item.metadata.is_some())
            })
            .collect();
        let mut consumed_indexes = HashSet::new();

        for (index, item) in history_items.iter().enumerate() {
            if consumed_indexes.contains(&index) {
                continue;
            }

            if let Some(metadata) = &item.metadata {
                let paired_result_index = if let Some(tool_call_id) = item.tool_call_id.as_deref()
                    && let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied()
                {
                    consumed_indexes.insert(result_index);
                    Some(result_index)
                } else {
                    None
                };
                let handled_metadata = match metadata {
                    SessionHistoryMetadata::PlanUpdate { explanation, steps } => {
                        self.on_plan_updated(
                            explanation.clone(),
                            steps
                                .iter()
                                .map(|step| crate::events::PlanStep {
                                    text: step.text.clone(),
                                    status: match step.status {
                                        SessionPlanStepStatus::Pending => {
                                            crate::events::PlanStepStatus::Pending
                                        }
                                        SessionPlanStepStatus::InProgress => {
                                            crate::events::PlanStepStatus::InProgress
                                        }
                                        SessionPlanStepStatus::Completed => {
                                            crate::events::PlanStepStatus::Completed
                                        }
                                        SessionPlanStepStatus::Cancelled => {
                                            crate::events::PlanStepStatus::Cancelled
                                        }
                                    },
                                })
                                .collect(),
                        );
                        true
                    }
                    SessionHistoryMetadata::ProposedPlan => {
                        self.add_history_entry_without_redraw(Box::new(
                            history_cell::new_proposed_plan(item.body.clone(), &self.session.cwd),
                        ));
                        true
                    }
                    SessionHistoryMetadata::TurnSummary { .. } => false,
                    SessionHistoryMetadata::Edited { changes } => {
                        let tool_cell =
                            restore_session::completed_tool_from_edit(item, changes.clone(), 0);
                        self.commit_committed_tool_to_history(tool_cell);
                        true
                    }
                    SessionHistoryMetadata::Explored { actions } => {
                        let result_item = paired_result_index
                            .map(|result_index| &history_items[result_index])
                            .or_else(|| {
                                (item.kind != devo_protocol::SessionHistoryItemKind::ToolCall)
                                    .then_some(item)
                            });
                        self.commit_exploration_tool_from_history_item(
                            item.tool_call_id
                                .clone()
                                .unwrap_or_else(|| "restored".to_string()),
                            item.title.clone(),
                            actions.clone(),
                            Self::restored_tool_io_name(item, result_item),
                            Self::restored_tool_io_input(item, result_item),
                            result_item.and_then(Self::restored_tool_io_output),
                            result_item.and_then(Self::restored_tool_io_display_content),
                            result_item.is_some_and(|item| {
                                item.kind == devo_protocol::SessionHistoryItemKind::Error
                            }),
                        );
                        true
                    }
                };
                if handled_metadata {
                    continue;
                }
            }

            if let Some(changes) = Self::edited_changes_from_history_item(item) {
                let tool_cell = restore_session::completed_tool_from_edit(item, changes, 0);
                self.commit_committed_tool_to_history(tool_cell);
                continue;
            }

            if item.kind == devo_protocol::SessionHistoryItemKind::ToolCall
                && let Some(tool_call_id) = item.tool_call_id.as_deref()
            {
                if metadata_owned_ids.contains(tool_call_id) {
                    continue;
                }
                if let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied() {
                    consumed_indexes.insert(result_index);
                    let result_item = &history_items[result_index];
                    if let Some(tool_cell) = restore_session::paired_tool_cell(item, result_item, 0)
                    {
                        self.commit_committed_tool_to_history(tool_cell);
                    }
                    continue;
                }
            }

            match item.kind {
                devo_protocol::SessionHistoryItemKind::User => {
                    self.add_restored_user_prompt(item.body.clone());
                }
                devo_protocol::SessionHistoryItemKind::Assistant
                | devo_protocol::SessionHistoryItemKind::Reasoning => {
                    if let Some(cell) = restore_session::restore_item_to_committed(item, 0) {
                        self.append_restored_committed_cell(cell);
                    }
                }
                devo_protocol::SessionHistoryItemKind::ToolCall => {}
                devo_protocol::SessionHistoryItemKind::ToolResult
                | devo_protocol::SessionHistoryItemKind::CommandExecution => {
                    if let Some(CommittedCellModel::Tool(tool)) =
                        restore_session::restore_item_to_committed(item, 0)
                    {
                        self.commit_committed_tool_to_history(tool);
                    }
                }
                devo_protocol::SessionHistoryItemKind::Error => {
                    if item.tool_call_id.is_none() {
                        self.add_history_entry_without_redraw(Box::new(
                            history_cell::new_error_event(item.body.clone()),
                        ));
                    } else if let Some(CommittedCellModel::Tool(tool)) =
                        restore_session::restore_item_to_committed(item, 0)
                    {
                        self.commit_committed_tool_to_history(tool);
                    }
                }
                devo_protocol::SessionHistoryItemKind::TurnSummary => {
                    let input_mode = turn_summary_input_mode(item);
                    let summary = match item.body.as_str() {
                        "failed" => history_cell::TurnSummaryCell::new_failed(
                            input_mode,
                            item.title.clone(),
                            self.active_accent_color(),
                        ),
                        "interrupted" => history_cell::TurnSummaryCell::new_interrupted(
                            input_mode,
                            item.title.clone(),
                            self.active_accent_color(),
                        ),
                        _ => history_cell::TurnSummaryCell::new(
                            input_mode,
                            item.title.clone(),
                            item.duration_ms,
                            self.active_accent_color(),
                        ),
                    };
                    self.add_history_entry_without_redraw(Box::new(summary));
                }
                devo_protocol::SessionHistoryItemKind::ContextCompaction => {
                    let title = if item.title.is_empty() {
                        "Context compacted".to_string()
                    } else {
                        item.title.clone()
                    };
                    self.add_history_entry_without_redraw(Box::new(
                        history_cell::new_live_aligned_info_event(title, None),
                    ));
                }
            }
        }

        self.frame_requester.schedule_frame();
        true
    }

    pub(super) fn add_restored_user_prompt(&mut self, body: String) {
        self.add_history_entry_without_redraw(Box::new(history_cell::new_user_prompt(
            body,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            self.active_accent_color(),
            InputMode::Build,
        )));
    }

    fn restored_tool_io_name(
        item: &SessionHistoryItem,
        result_item: Option<&SessionHistoryItem>,
    ) -> Option<String> {
        item.tool_io
            .as_ref()
            .map(|tool_io| tool_io.tool_name.clone())
            .or_else(|| {
                result_item
                    .and_then(|item| item.tool_io.as_ref())
                    .map(|tool_io| tool_io.tool_name.clone())
            })
            .filter(|tool_name| !tool_name.is_empty())
    }

    fn restored_tool_io_input(
        item: &SessionHistoryItem,
        result_item: Option<&SessionHistoryItem>,
    ) -> Option<Value> {
        item.tool_io
            .as_ref()
            .map(|tool_io| tool_io.input.clone())
            .filter(|input| !input.is_null())
            .or_else(|| {
                result_item
                    .and_then(|item| item.tool_io.as_ref())
                    .map(|tool_io| tool_io.input.clone())
                    .filter(|input| !input.is_null())
            })
    }

    fn restored_tool_io_output(item: &SessionHistoryItem) -> Option<Value> {
        item.tool_io
            .as_ref()
            .and_then(|tool_io| tool_io.output.clone())
            .or_else(|| (!item.body.is_empty()).then(|| Value::String(item.body.clone())))
    }

    fn restored_tool_io_display_content(item: &SessionHistoryItem) -> Option<String> {
        item.tool_io
            .as_ref()
            .and_then(|tool_io| tool_io.display_content.clone())
    }

    fn value_text(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }

    pub(super) fn edited_changes_from_history_item(
        item: &SessionHistoryItem,
    ) -> Option<HashMap<PathBuf, devo_protocol::protocol::FileChange>> {
        if item.kind != devo_protocol::SessionHistoryItemKind::ToolResult {
            return None;
        }
        if let Some(SessionHistoryMetadata::Edited { changes }) = &item.metadata {
            return (!changes.is_empty()).then(|| changes.clone());
        }
        let lower_title = item.title.to_ascii_lowercase();
        let body_for_parse = item
            .tool_io
            .as_ref()
            .and_then(|tool_io| tool_io.output.as_ref())
            .map(ToString::to_string)
            .filter(|text| text.contains("\"files\"") || text.contains("\"diff\""))
            .unwrap_or_else(|| item.body.clone());
        if !lower_title.contains("apply_patch")
            && !lower_title.contains("write")
            && !lower_title.contains("edit")
            && !body_for_parse.contains("\"files\"")
        {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(&body_for_parse).ok()?;
        let files = value.get("files")?.as_array()?;
        let diff = value
            .get("diff")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut changes = HashMap::new();
        for file in files {
            let path = PathBuf::from(file.get("path")?.as_str()?);
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
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| diff.clone()),
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
                        .map(PathBuf::from),
                },
                _ => continue,
            };
            changes.insert(path, change);
        }
        (!changes.is_empty()).then_some(changes)
    }
}

fn turn_summary_input_mode(item: &SessionHistoryItem) -> InputMode {
    match item.metadata {
        Some(SessionHistoryMetadata::TurnSummary { collaboration_mode }) => {
            InputMode::from_collaboration_mode(collaboration_mode)
        }
        _ => InputMode::Build,
    }
}
