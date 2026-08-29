//! Applies [`ItemLifecycleEvent`] values to a single transcript projection.

use std::collections::HashMap;

use crate::transcript::lifecycle::ItemLifecycleEvent;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::model::LiveTextCellModel;
use crate::transcript::model::TextCellModel;
use crate::transcript::model::ToolCellModel;
use crate::transcript::model::ToolPhase;
use crate::transcript::tool_state::initial_phase;

use super::file_change::has_visible_file_changes;
use super::model::ToolCellModel as ToolModel;
use super::stream_text::apply_stream_text_delta;

/// Single source of truth for transcript lifecycle (text + tools).
#[derive(Debug, Default)]
pub(crate) struct TranscriptProjector {
    tools: HashMap<String, ToolCellModel>,
    tool_order: Vec<String>,
    live_text: HashMap<devo_core::ItemId, LiveTextCellModel>,
    text_order: Vec<devo_core::ItemId>,
    next_seq: u64,
    committed: Vec<CommittedCellModel>,
    synced_committed: usize,
}

impl TranscriptProjector {
    pub(crate) fn apply(&mut self, event: ItemLifecycleEvent) {
        match event {
            ItemLifecycleEvent::ToolOpened {
                tool_use_id,
                tool_name,
                input,
                command,
                command_source,
                parsed_commands,
            } => {
                if let Some(tool) = self.tools.get_mut(&tool_use_id) {
                    tool.refresh_opened(tool_name, input, command, command_source, parsed_commands);
                    return;
                }
                let seq = self.reserve_seq();
                let tool = ToolModel::new_opened(
                    tool_use_id.clone(),
                    seq,
                    tool_name,
                    input,
                    command,
                    command_source,
                    parsed_commands,
                );
                self.tool_order.push(tool_use_id.clone());
                self.tools.insert(tool_use_id, tool);
            }
            ItemLifecycleEvent::ToolInputChunk { tool_use_id, chunk } => {
                if let Some(tool) = self.tools.get_mut(&tool_use_id) {
                    tool.input_partial_json.push_str(&chunk);
                    if let Ok(parsed) =
                        serde_json::from_str::<serde_json::Value>(&tool.input_partial_json)
                    {
                        tool.input = Some(parsed.clone());
                        if tool.phase == ToolPhase::Preparing
                            && let Some(name) = tool.tool_name.clone()
                            && !super::tool_state::input_is_incomplete(&parsed)
                        {
                            tool.phase = initial_phase(&name, &parsed);
                        }
                    }
                }
            }
            ItemLifecycleEvent::ToolOutputChunk { tool_use_id, chunk } => {
                if let Some(tool) = self.tools.get_mut(&tool_use_id) {
                    tool.output_preview.push_str(&chunk);
                    tool.output_delta_lines.push(chunk);
                    if tool.phase == ToolPhase::Preparing {
                        tool.phase = ToolPhase::Running;
                    }
                }
            }
            ItemLifecycleEvent::ToolClosed {
                tool_use_id,
                tool_name,
                input,
                output,
                display_content,
                file_changes,
                is_error,
                truncated,
            } => {
                if let Some(changes) = file_changes.as_ref()
                    && !has_visible_file_changes(changes)
                {
                    self.tools.remove(&tool_use_id);
                    self.tool_order.retain(|id| id != &tool_use_id);
                    return;
                }
                if let Some(mut tool) = self.tools.remove(&tool_use_id) {
                    self.tool_order.retain(|id| id != &tool_use_id);
                    if tool_name != "tool" {
                        tool.tool_name = Some(tool_name);
                    }
                    if !input.is_null() {
                        tool.input = Some(input);
                    }
                    tool.tool_output = output;
                    tool.tool_display_content = display_content.clone();
                    if let Some(preview) = display_content {
                        tool.output_preview = preview;
                    }
                    if let Some(changes) = file_changes {
                        tool.file_changes = Some(changes);
                    }
                    tool.is_error = is_error;
                    tool.truncated = truncated;
                    tool.phase = if is_error {
                        ToolPhase::Failed
                    } else {
                        ToolPhase::Completed
                    };
                    self.committed.push(CommittedCellModel::Tool(tool));
                } else {
                    let seq = self.reserve_seq();
                    let phase = if is_error {
                        ToolPhase::Failed
                    } else {
                        ToolPhase::Completed
                    };
                    self.committed.push(CommittedCellModel::Tool(ToolModel {
                        tool_use_id,
                        seq,
                        phase,
                        summary: String::new(),
                        tool_name: Some(tool_name),
                        input: Some(input),
                        input_partial_json: String::new(),
                        parsed_commands: Vec::new(),
                        exec_like: false,
                        start_time: None,
                        output_preview: display_content.clone().unwrap_or_default(),
                        output_delta_lines: Vec::new(),
                        file_changes,
                        command: None,
                        command_source: None,
                        command_output: None,
                        command_duration: None,
                        tool_output: output,
                        tool_display_content: display_content,
                        is_error,
                        truncated,
                    }));
                }
            }
            ItemLifecycleEvent::TurnLiveToolsCleared => {
                self.tools.clear();
                self.tool_order.clear();
                self.live_text.clear();
                self.text_order.clear();
            }
            ItemLifecycleEvent::TextStarted { item_id, kind } => {
                if self.live_text.contains_key(&item_id) {
                    return;
                }
                let seq = self.reserve_seq();
                self.text_order.push(item_id);
                self.live_text.insert(
                    item_id,
                    LiveTextCellModel {
                        item_id,
                        kind,
                        seq,
                        text: String::new(),
                    },
                );
            }
            ItemLifecycleEvent::TextDelta {
                item_id,
                kind,
                delta,
            } => {
                if delta.is_empty() {
                    return;
                }
                if let Some(live) = self.live_text.get_mut(&item_id) {
                    apply_stream_text_delta(&mut live.text, &delta);
                    return;
                }
                let seq = self.reserve_seq();
                self.text_order.push(item_id);
                self.live_text.insert(
                    item_id,
                    LiveTextCellModel {
                        item_id,
                        kind,
                        seq,
                        text: delta,
                    },
                );
            }
            ItemLifecycleEvent::TextCompleted {
                item_id,
                kind,
                final_text,
            } => {
                self.live_text.remove(&item_id);
                self.text_order.retain(|id| *id != item_id);
                if final_text.trim().is_empty() {
                    return;
                }
                self.committed.push(CommittedCellModel::Text(TextCellModel {
                    item_id,
                    kind,
                    text: final_text,
                }));
            }
            ItemLifecycleEvent::ProposedPlanStarted { .. }
            | ItemLifecycleEvent::ProposedPlanDelta { .. }
            | ItemLifecycleEvent::ProposedPlanCompleted { .. }
            | ItemLifecycleEvent::PlanUpdated { .. } => {}
        }
    }

    pub(crate) fn live_tools(&self) -> impl Iterator<Item = &ToolCellModel> {
        self.tool_order
            .iter()
            .filter_map(|id| self.tools.get(id))
            .filter(|tool| tool.is_live())
    }

    pub(crate) fn live_tool(&self, tool_use_id: &str) -> Option<&ToolCellModel> {
        self.tools.get(tool_use_id).filter(|tool| tool.is_live())
    }

    pub(crate) fn live_text_items(&self) -> impl Iterator<Item = &LiveTextCellModel> {
        self.text_order
            .iter()
            .filter_map(|item_id| self.live_text.get(item_id))
    }

    pub(crate) fn live_text_for(&self, item_id: devo_core::ItemId) -> Option<&str> {
        self.live_text.get(&item_id).map(|live| live.text.as_str())
    }

    pub(crate) fn has_live_text(&self, item_id: devo_core::ItemId) -> bool {
        self.live_text.contains_key(&item_id)
    }

    pub(crate) fn drop_live_text(&mut self, item_id: devo_core::ItemId) {
        self.live_text.remove(&item_id);
        self.text_order.retain(|id| *id != item_id);
    }

    pub(crate) fn drain_unsynced_committed(&mut self) -> Vec<CommittedCellModel> {
        let start = self.synced_committed;
        let end = self.committed.len();
        self.synced_committed = end;
        self.committed[start..end].to_vec()
    }

    pub(crate) fn reset_sync_cursor(&mut self) {
        self.synced_committed = 0;
        self.committed.clear();
        self.tools.clear();
        self.tool_order.clear();
        self.live_text.clear();
        self.text_order.clear();
    }

    pub(crate) fn restore_committed(&mut self, cells: Vec<CommittedCellModel>) {
        self.committed = cells;
        self.synced_committed = 0;
    }

    fn reserve_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use devo_protocol::protocol::FileChange;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn tool_close_preserves_opened_metadata_when_result_omits_tool_name() {
        use crate::transcript::presentation::tool_title_line;
        use crate::transcript::presentation::tool_title_parts;

        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "grep-1".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({"pattern": "plan", "path": "crates"}),
            command: None,
            command_source: None,
            parsed_commands: Vec::new(),
        });
        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "grep-1".into(),
            tool_name: "tool".into(),
            input: serde_json::Value::Null,
            output: Some(serde_json::json!("matches")),
            display_content: Some("matches".into()),
            file_changes: None,
            is_error: false,
            truncated: false,
        });

        let committed = projector.drain_unsynced_committed();
        assert_eq!(committed.len(), 1);
        let CommittedCellModel::Tool(tool) = &committed[0] else {
            panic!("expected committed tool cell");
        };
        assert_eq!(tool.tool_name.as_deref(), Some("grep"));
        assert_eq!(
            tool.input,
            Some(serde_json::json!({"pattern": "plan", "path": "crates"}))
        );
        let parts = tool_title_parts(
            tool.phase,
            tool.tool_name.as_deref(),
            tool.input.as_ref(),
            &tool.parsed_commands,
            false,
            "",
        );
        assert_eq!(parts.verb, "Grepped");
        assert_eq!(parts.detail, "'plan' in crates");
        let title = tool_title_line(tool.phase, &parts);
        let title_text: String = title
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(title_text, "Grepped 'plan' in crates");
    }

    #[test]
    fn file_change_closes_running_tool() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "edit-1".into(),
            tool_name: "edit".into(),
            input: serde_json::json!({"filePath": "a.rs"}),
            command: None,
            command_source: None,
            parsed_commands: Vec::new(),
        });
        let mut changes = HashMap::new();
        changes.insert(
            PathBuf::from("a.rs"),
            FileChange::Update {
                unified_diff: "@@\n".into(),
                old_text: None,
                new_text: None,
                move_path: None,
            },
        );
        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "edit-1".into(),
            tool_name: "edit".into(),
            input: serde_json::json!({"filePath": "a.rs"}),
            output: None,
            display_content: None,
            file_changes: Some(changes),
            is_error: false,
            truncated: false,
        });

        assert_eq!(projector.live_tools().count(), 0);
        assert_eq!(projector.committed.len(), 1);
    }

    #[test]
    fn text_delta_accepts_incremental_and_cumulative_chunks() {
        let mut projector = TranscriptProjector::default();
        let item_id = devo_core::ItemId::new();
        projector.apply(ItemLifecycleEvent::TextStarted {
            item_id,
            kind: crate::events::TextItemKind::Reasoning,
        });
        projector.apply(ItemLifecycleEvent::TextDelta {
            item_id,
            kind: crate::events::TextItemKind::Reasoning,
            delta: "I".to_string(),
        });
        projector.apply(ItemLifecycleEvent::TextDelta {
            item_id,
            kind: crate::events::TextItemKind::Reasoning,
            delta: "'ll".to_string(),
        });
        let text = projector
            .live_text_items()
            .next()
            .map(|live| live.text.clone())
            .expect("live text");
        assert_eq!(text, "I'll");

        projector.apply(ItemLifecycleEvent::TextDelta {
            item_id,
            kind: crate::events::TextItemKind::Reasoning,
            delta: "I'll create".to_string(),
        });
        let text = projector
            .live_text_items()
            .next()
            .map(|live| live.text.clone())
            .expect("live text");
        assert_eq!(text, "I'll create");
    }

    #[test]
    fn fragmented_line_splits_preserve_full_text() {
        let mut projector = TranscriptProjector::default();
        let item_id = devo_core::ItemId::new();
        projector.apply(ItemLifecycleEvent::TextStarted {
            item_id,
            kind: crate::events::TextItemKind::Assistant,
        });

        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut expected = String::new();
        for index in 0..=3 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let line = format!("line-{index:02}-{seed:016x}");
            let streamed_line = format!("{line}\n");
            let split_at = 1 + (seed as usize % (streamed_line.len() - 1));
            for delta in [&streamed_line[..split_at], &streamed_line[split_at..]] {
                projector.apply(ItemLifecycleEvent::TextDelta {
                    item_id,
                    kind: crate::events::TextItemKind::Assistant,
                    delta: delta.to_string(),
                });
            }
            expected.push_str(&streamed_line);
            let text = projector
                .live_text_items()
                .next()
                .map(|live| live.text.clone())
                .expect("live text");
            assert_eq!(text, expected, "projector text mismatch after line {index}");
        }
    }
}
