//! Syncs [`TranscriptProjector`] state into `ChatWidget` rendering containers.

use std::collections::HashSet;

use devo_core::ItemId;
use ratatui::text::Line;

use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::transcript::lifecycle::ItemLifecycleEvent;
use crate::transcript::lifecycle::TurnToolOutcome;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::model::ToolPhase;

use super::ActiveToolCall;
use super::ChatWidget;
use super::DotStatus;
use super::text_stream::ActiveTextItemId;

impl ChatWidget {
    /// Single entry point for transcript-affecting lifecycle events (P3).
    pub(super) fn apply_item_lifecycle(&mut self, event: ItemLifecycleEvent) {
        self.transcript_projector.apply(event);
        self.sync_transcript_projection();
    }

    /// Routes transcript lifecycle events from the worker bus.
    pub(super) fn route_worker_event_through_projector(&mut self, event: &WorkerEvent) -> bool {
        if let WorkerEvent::Transcript(lifecycle) = event {
            self.apply_item_lifecycle(lifecycle.clone());
            return true;
        }
        false
    }

    pub(super) fn clear_turn_live_projection(&mut self, outcome: TurnToolOutcome) {
        self.active_cell = None;
        self.detached_exec_tool_ids.clear();
        self.apply_item_lifecycle(ItemLifecycleEvent::TurnLiveToolsCleared { outcome });
    }

    pub(super) fn sync_transcript_projection(&mut self) {
        let committed: Vec<_> = self.transcript_projector.drain_unsynced_committed();
        for cell in committed {
            match cell {
                CommittedCellModel::Text(text) => {
                    let item_id = ActiveTextItemId(text.item_id);
                    if self
                        .active_text_items
                        .iter()
                        .any(|item| item.item_id == item_id)
                    {
                        self.complete_text_item(item_id, text.kind, text.text);
                        continue;
                    }
                    let skip_history = text.kind == TextItemKind::Assistant
                        && self
                            .boundary_committed_assistant_items
                            .contains(&text.item_id);
                    if text.kind == TextItemKind::Assistant {
                        self.committed_server_assistant_in_turn = true;
                    }
                    if skip_history {
                        continue;
                    }
                    let title = match text.kind {
                        TextItemKind::Assistant => "Assistant",
                        TextItemKind::Reasoning => "Reasoning",
                    };
                    self.add_markdown_history_without_redraw(title, &text.text);
                }
                CommittedCellModel::Tool(tool) => {
                    self.commit_committed_tool_to_history(tool);
                }
            }
        }

        self.sync_live_text_from_projector();

        self.active_tool_calls.clear();
        self.pending_tool_calls.clear();
        for tool in self.transcript_projector.live_tools() {
            let tool_call = ActiveToolCall {
                tool_use_id: tool.tool_use_id.clone(),
                seq: tool.seq,
                tool_name: tool.tool_name.clone(),
                input: tool.input.clone(),
                title: tool.summary.clone(),
                lines: tool
                    .output_delta_lines
                    .iter()
                    .map(|line| Line::from(line.clone()))
                    .collect(),
                output: tool.output_preview.clone(),
                parsed_commands: tool.parsed_commands.clone(),
                exec_like: tool.exec_like,
                owned_by_active_cell: crate::chatwidget::history_commit::tool_uses_exec_cell(tool)
                    && !self.detached_exec_tool_ids.contains(&tool.tool_use_id),
                start_time: tool.start_time,
                phase: tool.phase,
            };
            if tool.phase == ToolPhase::Preparing {
                self.pending_tool_calls.push(tool_call);
            } else {
                self.active_tool_calls
                    .insert(tool.tool_use_id.clone(), tool_call);
            }
        }

        self.sync_exec_cells_from_projector();

        if self
            .transcript_projector
            .live_text_items()
            .any(|text| text.kind == TextItemKind::Assistant)
        {
            self.set_status_message("Generating");
        } else if self
            .transcript_projector
            .live_text_items()
            .any(|text| text.kind == TextItemKind::Reasoning)
        {
            self.set_status_message("Thinking");
        }

        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.frame_requester.schedule_frame();
    }

    fn sync_live_text_from_projector(&mut self) {
        let live_items: Vec<_> = self
            .transcript_projector
            .live_text_items()
            .cloned()
            .collect();
        let live_ids: HashSet<ItemId> = live_items.iter().map(|live| live.item_id).collect();

        for live in live_items {
            let item_id = ActiveTextItemId(live.item_id);
            if !self
                .active_text_items
                .iter()
                .any(|item| item.item_id == item_id)
            {
                // A completed exploration group is already in its compact
                // display form and can safely remain alongside live text.
                // Only detach an unfinished group so later tools do not get
                // merged across the text boundary.
                if let Some(cell) = self
                    .active_cell
                    .as_ref()
                    .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
                    .filter(|cell| cell.is_exploring_cell() && cell.is_active())
                {
                    self.detached_exec_tool_ids
                        .extend(cell.iter_calls().map(|call| call.call_id.clone()));
                    self.active_cell = None;
                }
                self.start_text_item(item_id, live.kind, live.seq);
            }

            self.sync_live_text_item(item_id);
        }

        self.active_text_items.retain(|item| {
            live_ids.contains(&item.item_id.item_id()) || item.status == DotStatus::Completed
        });
    }

    pub(super) fn reset_transcript_projection(&mut self) {
        self.transcript_projector.reset_sync_cursor();
    }

    pub(super) fn restore_transcript_projection(
        &mut self,
        items: &[devo_protocol::SessionHistoryItem],
    ) {
        self.transcript_projector =
            crate::transcript::restore::restore_projector_from_history(items);
        self.sync_transcript_projection();
    }

    pub(super) fn append_restored_committed_cell(&mut self, cell: CommittedCellModel) {
        match cell {
            CommittedCellModel::Text(text) => {
                let title = match text.kind {
                    TextItemKind::Assistant => "Assistant",
                    TextItemKind::Reasoning => "Reasoning",
                };
                self.add_markdown_history_without_redraw(title, &text.text);
            }
            CommittedCellModel::Tool(tool) => {
                let dot_prefix = if tool.is_error {
                    Self::failed_dot_prefix()
                } else {
                    Self::tool_dot_prefix()
                };
                let history_cell = crate::transcript::render::committed_cell_to_history(
                    &CommittedCellModel::Tool(tool),
                    &self.session.cwd,
                    Self::ran_tool_line,
                    dot_prefix,
                    Self::tool_text_style(),
                );
                self.add_history_entry_without_redraw(history_cell);
            }
        }
    }
}
