//! Applies [`ItemLifecycleEvent`] values to a single transcript projection.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::transcript::lifecycle::ItemLifecycleEvent;
use crate::transcript::lifecycle::TurnToolOutcome;
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
    /// Call ids this projector has already committed (turn boundary or
    /// restore). Late lifecycle events for these ids must not re-materialize
    /// live rows: after the boundary nothing would ever flush them, so a
    /// refreshed row would stay rendered at the bottom of the live viewport
    /// next to the composer.
    committed_tool_ids: HashSet<String>,
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
                item_seq,
                command,
                command_source,
                parsed_commands,
            } => {
                if let Some(tool) = self.tools.get_mut(&tool_use_id) {
                    tool.refresh_opened(tool_name, input, command, command_source, parsed_commands);
                    return;
                }
                if self.committed_tool_ids.contains(&tool_use_id) {
                    // A refresh (`item/started` re-broadcast or the completed
                    // `ToolCall` item) for a row the boundary already committed.
                    // Re-materializing it would pin a live row to the bottom of
                    // the viewport that no future boundary would ever flush.
                    return;
                }
                let seq = self.reserve_seq(item_seq);
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
                    } else if let Some(partial) =
                        super::tool_state::partial_object_members(&tool.input_partial_json)
                    {
                        // Display-only fill while the JSON is still streaming:
                        // the running row shows its parameters (filePath,
                        // pattern, command, …) as soon as each member
                        // completes instead of only at the tool result.
                        tool.input = Some(partial);
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
                if let Some(tool) = self.tools.get_mut(&tool_use_id) {
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
                } else if self.committed_tool_ids.contains(&tool_use_id) {
                    // The boundary already committed this row; a late close
                    // (a notification that raced past the turn's terminal
                    // event) must not re-materialize it in the live viewport.
                } else {
                    // A close for a call this projector never saw open
                    // (recovery sweep, missed open) is already terminal:
                    // commit it directly instead of parking it live, where
                    // only the next turn boundary would flush it.
                    let exec_like = super::tool_state::is_shell_tool_name(&tool_name);
                    let phase = if is_error {
                        ToolPhase::Failed
                    } else {
                        ToolPhase::Completed
                    };
                    let tool = ToolModel {
                        tool_use_id: tool_use_id.clone(),
                        seq: 0,
                        phase,
                        summary: String::new(),
                        tool_name: Some(tool_name),
                        input: Some(input),
                        input_partial_json: String::new(),
                        parsed_commands: Vec::new(),
                        exec_like,
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
                    };
                    self.commit_tool_model(tool);
                }
            }
            ItemLifecycleEvent::TurnLiveToolsCleared { outcome } => {
                for tool_use_id in std::mem::take(&mut self.tool_order) {
                    if let Some(mut tool) = self.tools.remove(&tool_use_id) {
                        if tool.phase == ToolPhase::Preparing && tool.input.is_none() {
                            // A row that never received any payload carries no
                            // renderable facts; drop it rather than commit
                            // an empty cell.
                            continue;
                        }
                        if matches!(tool.phase, ToolPhase::Preparing | ToolPhase::Running) {
                            tool.phase = match outcome {
                                TurnToolOutcome::Completed => ToolPhase::Degraded,
                                TurnToolOutcome::Failed | TurnToolOutcome::Interrupted => {
                                    ToolPhase::Failed
                                }
                            };
                            if tool.phase == ToolPhase::Failed {
                                tool.is_error = true;
                            }
                        }
                        self.commit_tool_model(tool);
                    }
                }
                self.tools.clear();
                self.live_text.clear();
                self.text_order.clear();
            }
            ItemLifecycleEvent::TextStarted {
                item_id,
                kind,
                item_seq,
            } => {
                if self.live_text.contains_key(&item_id) {
                    return;
                }
                let seq = self.reserve_seq(item_seq);
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
                let seq = self.reserve_seq(/*item_seq*/ None);
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
                let text_seq = self.live_text.get(&item_id).map(|live| live.seq);
                self.live_text.remove(&item_id);
                self.text_order.retain(|id| *id != item_id);
                if final_text.trim().is_empty() {
                    return;
                }
                // The TUI commits finished text to scrollback immediately,
                // while tools stay in the live viewport until the turn
                // boundary. Text that follows tools in event order must not
                // overtake them: flush terminal tools that ran before this
                // text so `committed` stays in transcript order (otherwise
                // the tools would land in history below the text they
                // preceded).
                if let Some(text_seq) = text_seq {
                    self.commit_terminal_tools_older_than(text_seq);
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
        self.tool_order.iter().filter_map(|id| self.tools.get(id))
    }

    pub(crate) fn live_tool(&self, tool_use_id: &str) -> Option<&ToolCellModel> {
        self.tools.get(tool_use_id)
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
        for cell in &cells {
            if let CommittedCellModel::Tool(tool) = cell {
                self.committed_tool_ids.insert(tool.tool_use_id.clone());
            }
        }
        self.committed = cells;
        self.synced_committed = 0;
    }

    fn reserve_seq(&mut self, item_seq: Option<u64>) -> u64 {
        if let Some(item_seq) = item_seq {
            self.next_seq = self.next_seq.max(item_seq.saturating_add(1));
            return item_seq;
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    fn commit_tool_model(&mut self, tool: ToolModel) {
        self.committed_tool_ids.insert(tool.tool_use_id.clone());
        self.committed.push(CommittedCellModel::Tool(tool));
    }

    /// Commits live tools that already finished before `seq`, preserving
    /// transcript order when a text cell commits mid-turn (see
    /// `ItemLifecycleEvent::TextCompleted`).
    fn commit_terminal_tools_older_than(&mut self, seq: u64) {
        let older: Vec<String> = self
            .tool_order
            .iter()
            .filter(|id| {
                self.tools
                    .get(*id)
                    .is_some_and(|tool| tool.seq < seq && tool.phase.is_terminal())
            })
            .cloned()
            .collect();
        for tool_use_id in older {
            let Some(tool) = self.tools.remove(&tool_use_id) else {
                continue;
            };
            self.tool_order.retain(|id| *id != tool_use_id);
            self.commit_tool_model(tool);
        }
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
    fn tool_input_chunks_fill_parameters_progressively_while_streaming() {
        use crate::transcript::presentation::tool_title_parts;

        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "read-1".into(),
            tool_name: "read".into(),
            input: serde_json::Value::Null,
            item_seq: None,
            command: None,
            command_source: None,
            parsed_commands: Vec::new(),
        });

        // First fragment: the key is still streaming — nothing displayable yet.
        projector.apply(ItemLifecycleEvent::ToolInputChunk {
            tool_use_id: "read-1".into(),
            chunk: r#"{"filePa"#.to_string(),
        });
        let tool = projector.live_tool("read-1").expect("live tool");
        assert!(
            tool.input.is_none() || tool.input.as_ref().is_some_and(serde_json::Value::is_null)
        );

        // filePath completes: the running row can render it immediately.
        projector.apply(ItemLifecycleEvent::ToolInputChunk {
            tool_use_id: "read-1".into(),
            chunk: r#"th": "src/lib.rs", "offs"#.to_string(),
        });
        let tool = projector.live_tool("read-1").expect("live tool");
        let input = tool.input.clone().expect("partial input");
        assert_eq!(input["filePath"], serde_json::json!("src/lib.rs"));
        let parts = tool_title_parts(
            tool.phase,
            tool.tool_name.as_deref(),
            tool.input.as_ref(),
            &tool.parsed_commands,
            false,
            tool.summary.as_str(),
        );
        assert_eq!(parts.verb, "Reading");
        assert!(parts.detail.contains("src/lib.rs"));

        // Full JSON arrives: the authoritative input replaces the partial view.
        projector.apply(ItemLifecycleEvent::ToolInputChunk {
            tool_use_id: "read-1".into(),
            chunk: r#"et": 10}"#.to_string(),
        });
        let tool = projector.live_tool("read-1").expect("live tool");
        assert_eq!(
            tool.input,
            Some(serde_json::json!({"filePath": "src/lib.rs", "offset": 10})),
        );
    }

    #[test]
    fn tool_close_preserves_opened_metadata_when_result_omits_tool_name() {
        use crate::transcript::presentation::tool_title_line;
        use crate::transcript::presentation::tool_title_parts;

        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "grep-1".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({"pattern": "plan", "path": "crates"}),
            item_seq: None,
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

        let live = projector.live_tool("grep-1").expect("completed live tool");
        assert_eq!(live.phase, ToolPhase::Completed);
        assert!(projector.drain_unsynced_committed().is_empty());
        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: TurnToolOutcome::Completed,
        });
        let committed = projector.drain_unsynced_committed();
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
    fn late_close_after_boundary_does_not_rematerialize_live_row() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "bash-1".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "cargo test"}),
            item_seq: None,
            command: Some("cargo test".into()),
            command_source: None,
            parsed_commands: Vec::new(),
        });
        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: TurnToolOutcome::Completed,
        });
        assert!(projector.live_tool("bash-1").is_none());
        assert_eq!(projector.drain_unsynced_committed().len(), 1);

        // A `ToolResult` notification that raced past the turn's terminal
        // event arrives after the boundary. It must not re-open a live row:
        // nothing would ever flush it back into history.
        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "bash-1".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "cargo test"}),
            output: Some(serde_json::json!("ok")),
            display_content: Some("ok".into()),
            file_changes: None,
            is_error: false,
            truncated: false,
        });

        assert!(
            projector.live_tool("bash-1").is_none(),
            "late close must not re-materialize a committed row"
        );
        assert!(
            projector.drain_unsynced_committed().is_empty(),
            "late close for a committed row must not append a duplicate cell"
        );

        // Same for the completed `ToolCall` refresh: it must not reopen the
        // row as a live "Running" entry.
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "bash-1".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "cargo test"}),
            item_seq: None,
            command: Some("cargo test".into()),
            command_source: None,
            parsed_commands: Vec::new(),
        });
        assert!(
            projector.live_tool("bash-1").is_none(),
            "late open refresh must not re-materialize a committed row"
        );
    }

    #[test]
    fn text_completion_commits_older_tools_before_text() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "grep-1".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({"pattern": "plan"}),
            item_seq: None,
            command: None,
            command_source: None,
            parsed_commands: Vec::new(),
        });
        let item_id = devo_core::ItemId::new();
        projector.apply(ItemLifecycleEvent::TextStarted {
            item_id,
            kind: crate::events::TextItemKind::Assistant,
            item_seq: None,
        });
        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "grep-1".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({"pattern": "plan"}),
            output: Some(serde_json::json!("matches")),
            display_content: Some("matches".into()),
            file_changes: None,
            is_error: false,
            truncated: false,
        });
        // Tools stay live until the turn boundary, so the finished grep must
        // still be parked in the live projection here.
        assert!(projector.live_tool("grep-1").is_some());
        assert!(projector.drain_unsynced_committed().is_empty());

        projector.apply(ItemLifecycleEvent::TextCompleted {
            item_id,
            kind: crate::events::TextItemKind::Assistant,
            final_text: "Here is what I found.".into(),
        });

        // Text commits mid-turn; the grep that ran before it must commit
        // first or scrollback would order [text, tool] against event order.
        let committed = projector.drain_unsynced_committed();
        let labels: Vec<&str> = committed
            .iter()
            .map(|cell| match cell {
                CommittedCellModel::Tool(_) => "tool",
                CommittedCellModel::Text(_) => "text",
            })
            .collect();
        assert_eq!(labels, vec!["tool", "text"]);
        assert!(projector.live_tool("grep-1").is_none());
    }

    #[test]
    fn text_completion_leaves_older_running_tools_live() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "bash-1".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "cargo build"}),
            item_seq: None,
            command: Some("cargo build".into()),
            command_source: None,
            parsed_commands: Vec::new(),
        });
        projector.apply(ItemLifecycleEvent::ToolOutputChunk {
            tool_use_id: "bash-1".into(),
            chunk: "Compiling\n".into(),
        });
        let item_id = devo_core::ItemId::new();
        projector.apply(ItemLifecycleEvent::TextStarted {
            item_id,
            kind: crate::events::TextItemKind::Assistant,
            item_seq: None,
        });
        projector.apply(ItemLifecycleEvent::TextCompleted {
            item_id,
            kind: crate::events::TextItemKind::Assistant,
            final_text: "Still building.".into(),
        });

        // A tool that has not reached a terminal phase is not force-finished
        // by the text commit; the turn boundary still owns its outcome.
        let committed = projector.drain_unsynced_committed();
        assert_eq!(committed.len(), 1);
        assert!(matches!(committed[0], CommittedCellModel::Text(_)));
        assert!(projector.live_tool("bash-1").is_some());
    }

    #[test]
    fn close_for_unknown_call_commits_directly_instead_of_parking_live() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "bash-9".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
            output: Some(serde_json::json!("src\n")),
            display_content: Some("src\n".into()),
            file_changes: None,
            is_error: false,
            truncated: false,
        });

        assert!(
            projector.live_tool("bash-9").is_none(),
            "terminal facts must not park a live row"
        );
        let committed = projector.drain_unsynced_committed();
        let CommittedCellModel::Tool(tool) = &committed[0] else {
            panic!("expected directly committed tool cell");
        };
        assert_eq!(tool.phase, ToolPhase::Completed);
        assert_eq!(tool.tool_name.as_deref(), Some("bash"));
    }

    #[test]
    fn restored_committed_ids_block_late_rematerialization() {
        let restored = CommittedCellModel::Tool(ToolModel {
            tool_use_id: "bash-1".into(),
            seq: 0,
            phase: ToolPhase::Completed,
            summary: String::new(),
            tool_name: Some("bash".into()),
            input: Some(serde_json::json!({"command": "ls"})),
            input_partial_json: String::new(),
            parsed_commands: Vec::new(),
            exec_like: true,
            start_time: None,
            output_preview: String::new(),
            output_delta_lines: Vec::new(),
            file_changes: None,
            command: Some("ls".into()),
            command_source: None,
            command_output: None,
            command_duration: None,
            tool_output: None,
            tool_display_content: None,
            is_error: false,
            truncated: false,
        });
        let mut projector = TranscriptProjector::default();
        projector.restore_committed(vec![restored]);
        assert_eq!(projector.drain_unsynced_committed().len(), 1);

        projector.apply(ItemLifecycleEvent::ToolClosed {
            tool_use_id: "bash-1".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
            output: Some(serde_json::json!("src\n")),
            display_content: Some("src\n".into()),
            file_changes: None,
            is_error: false,
            truncated: false,
        });

        assert!(projector.live_tool("bash-1").is_none());
        assert!(
            projector.drain_unsynced_committed().is_empty(),
            "late duplicate for a restored row must not append a second cell"
        );
    }

    #[test]
    fn file_change_closes_running_tool() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "edit-1".into(),
            tool_name: "edit".into(),
            input: serde_json::json!({"filePath": "a.rs"}),
            item_seq: None,
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

        assert_eq!(projector.live_tools().count(), 1);
        assert!(projector.committed.is_empty());
        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: TurnToolOutcome::Completed,
        });
        assert_eq!(projector.live_tools().count(), 0);
        assert_eq!(projector.committed.len(), 1);
    }

    #[test]
    fn duplicate_open_refreshes_one_tool_owner() {
        let mut projector = TranscriptProjector::default();
        for input in [
            serde_json::Value::Null,
            serde_json::json!({"command": "cargo check"}),
        ] {
            projector.apply(ItemLifecycleEvent::ToolOpened {
                tool_use_id: "exec-1".into(),
                tool_name: "exec_command".into(),
                input,
                item_seq: None,
                command: Some("cargo check".into()),
                command_source: None,
                parsed_commands: Vec::new(),
            });
        }

        let tools: Vec<_> = projector.live_tools().cloned().collect();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].input,
            Some(serde_json::json!({"command": "cargo check"}))
        );
    }

    #[test]
    fn parallel_tools_complete_in_place_and_commit_in_open_order() {
        let mut projector = TranscriptProjector::default();
        for id in ["first", "second"] {
            projector.apply(ItemLifecycleEvent::ToolOpened {
                tool_use_id: id.into(),
                tool_name: "exec_command".into(),
                input: serde_json::json!({"command": id}),
                item_seq: None,
                command: Some(id.into()),
                command_source: None,
                parsed_commands: Vec::new(),
            });
        }
        for id in ["second", "first"] {
            projector.apply(ItemLifecycleEvent::ToolClosed {
                tool_use_id: id.into(),
                tool_name: "exec_command".into(),
                input: serde_json::Value::Null,
                output: Some(serde_json::json!(id)),
                display_content: Some(id.into()),
                file_changes: None,
                is_error: id == "second",
                truncated: false,
            });
        }

        let live: Vec<_> = projector.live_tools().cloned().collect();
        assert_eq!(
            live.iter()
                .map(|tool| (tool.tool_use_id.as_str(), tool.phase))
                .collect::<Vec<_>>(),
            vec![
                ("first", ToolPhase::Completed),
                ("second", ToolPhase::Failed),
            ]
        );
        assert!(projector.drain_unsynced_committed().is_empty());

        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: TurnToolOutcome::Completed,
        });
        let committed = projector.drain_unsynced_committed();
        let ids: Vec<_> = committed
            .iter()
            .map(|cell| match cell {
                CommittedCellModel::Tool(tool) => tool.tool_use_id.as_str(),
                CommittedCellModel::Text(_) => panic!("expected tool"),
            })
            .collect();
        assert_eq!(ids, vec!["first", "second"]);
    }

    #[test]
    fn successful_turn_without_tool_result_commits_degraded_row() {
        let mut projector = TranscriptProjector::default();
        projector.apply(ItemLifecycleEvent::ToolOpened {
            tool_use_id: "missing-result".into(),
            tool_name: "exec_command".into(),
            input: serde_json::json!({"command": "echo hi"}),
            item_seq: None,
            command: Some("echo hi".into()),
            command_source: None,
            parsed_commands: Vec::new(),
        });
        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: TurnToolOutcome::Completed,
        });

        let committed = projector.drain_unsynced_committed();
        let CommittedCellModel::Tool(tool) = &committed[0] else {
            panic!("expected tool");
        };
        assert_eq!(tool.phase, ToolPhase::Degraded);
        let parts = crate::transcript::presentation::tool_title_parts(
            tool.phase,
            tool.tool_name.as_deref(),
            tool.input.as_ref(),
            &tool.parsed_commands,
            false,
            &tool.summary,
        );
        let title = crate::transcript::presentation::tool_title_line(tool.phase, &parts);
        let text = title
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, "Ran echo hi · result unavailable");
    }

    #[test]
    fn text_delta_accepts_incremental_and_cumulative_chunks() {
        let mut projector = TranscriptProjector::default();
        let item_id = devo_core::ItemId::new();
        projector.apply(ItemLifecycleEvent::TextStarted {
            item_id,
            kind: crate::events::TextItemKind::Reasoning,
            item_seq: None,
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
            item_seq: None,
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
