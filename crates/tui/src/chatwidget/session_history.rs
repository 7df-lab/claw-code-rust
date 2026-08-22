//! Transcript and restored-session history handling for `ChatWidget`.
//!
//! This module converts protocol/session items into history cells and owns the
//! bookkeeping for active cells, scrollback flushes, and restored transcripts.

use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use devo_core::ItemId;
use devo_protocol::CollaborationMode;
use devo_protocol::InputItem;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::InputMode;
use crate::bottom_pane::ProposedPlanActionsView;
use crate::events::PlanStep;
use crate::events::PlanStepStatus;
use crate::events::TranscriptItem;
use crate::events::TranscriptItemKind;
use crate::exec_cell::truncated_tool_output_preview;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::markdown::append_markdown;
use crate::render::line_utils::prefix_lines;
use crate::tool_result_cell::ToolResultCell;

use super::ChatWidget;
use super::DotStatus;

impl ChatWidget {
    pub(super) fn clear_for_session_switch(&mut self) {
        self.history.clear();
        self.next_history_flush_index = 0;
        self.active_cell = None;
        self.active_cell_revision = 0;
        self.active_proposed_plan = None;
        self.pending_proposed_plan_actions = false;
        self.active_tool_calls.clear();
        self.pending_tool_calls.clear();
        self.active_text_items.clear();
        self.boundary_committed_assistant_items.clear();
        self.queued_input_modes.clear();
        self.promoted_input_modes.clear();
        self.current_turn_mode = InputMode::Build;
        self.bottom_pane.clear_composer();
        self.set_status_message("Resuming session");
    }

    pub(super) fn clear_transcript_view(&mut self) {
        self.history.clear();
        self.next_history_flush_index = 0;
        self.active_cell = None;
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.last_terminal_assistant_visible_hash = None;
        self.active_text_items.clear();
        self.boundary_committed_assistant_items.clear();
        self.active_proposed_plan = None;
        self.pending_proposed_plan_actions = false;
        self.stream_chunking_policy.reset();
        self.selection_mode = false;
        self.selected_user_cell_index = None;
        self.user_cell_history_indices.clear();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn set_default_placeholder(&mut self) {
        self.bottom_pane.set_default_placeholder();
    }

    pub(super) fn on_plan_updated(&mut self, explanation: Option<String>, steps: Vec<PlanStep>) {
        let total = steps.len();
        let completed = steps
            .iter()
            .filter(|step| matches!(step.status, PlanStepStatus::Completed))
            .count();
        self.last_plan_progress = (total > 0).then_some((completed, total));

        let mut lines = vec![Line::from(vec![
            Span::styled("▌", Style::default().fg(Color::Rgb(120, 220, 160))),
            " ".into(),
            "Updated Plan".bold(),
        ])];
        if let Some(explanation) = explanation
            && !explanation.trim().is_empty()
        {
            lines.push(Line::from(""));
            lines.push(Line::from(explanation.italic()));
            lines.push(Line::from(""));
        }
        for step in steps {
            let (prefix, style) = match step.status {
                PlanStepStatus::Completed => ("→ ", Style::default().green()),
                PlanStepStatus::InProgress => ("→ ", Style::default().cyan()),
                PlanStepStatus::Pending => ("□ ", Style::default().dim()),
                PlanStepStatus::Cancelled => ("✗ ", Style::default().red()),
            };
            lines.extend(prefix_lines(
                vec![Line::from(Span::styled(step.text, style))],
                Span::styled(format!("  {prefix}"), style),
                Span::from("    "),
            ));
        }
        if !lines.is_empty() {
            self.add_to_history(PlainHistoryCell::new(lines));
        }
        self.frame_requester.schedule_frame();
    }

    pub(super) fn start_proposed_plan(&mut self, item_id: ItemId) {
        self.flush_active_cell();
        self.commit_assistant_text_before_proposed_plan();
        self.active_proposed_plan = Some(super::ActiveProposedPlan {
            item_id,
            text: String::new(),
        });
        self.refresh_active_proposed_plan_cell();
        self.set_status_message("Planning");
    }

    pub(super) fn push_proposed_plan_delta(&mut self, item_id: ItemId, delta: String) {
        if self
            .active_proposed_plan
            .as_ref()
            .is_none_or(|plan| plan.item_id != item_id)
        {
            self.start_proposed_plan(item_id);
        }
        if let Some(plan) = self.active_proposed_plan.as_mut() {
            plan.text.push_str(&delta);
        }
        self.refresh_active_proposed_plan_cell();
        self.set_status_message("Planning");
    }

    pub(super) fn complete_proposed_plan(&mut self, item_id: ItemId, final_text: String) {
        let active_text = self
            .active_proposed_plan
            .take()
            .filter(|plan| plan.item_id == item_id)
            .map(|plan| plan.text)
            .unwrap_or_default();
        let text = if final_text.trim().is_empty() {
            active_text
        } else {
            final_text
        };
        self.active_cell = None;
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.add_to_history(history_cell::new_proposed_plan(text, &self.session.cwd));
        self.set_status_message("Planning");
        self.pending_proposed_plan_actions = true;
        self.maybe_open_proposed_plan_actions();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn maybe_open_proposed_plan_actions(&mut self) {
        if !self.pending_proposed_plan_actions || self.busy {
            return;
        }

        self.pending_proposed_plan_actions = false;
        let implement_cwd = Some(self.session.cwd.clone());
        let implement_model = self.user_turn_model();
        let implement_model_binding_id = self.user_turn_model_binding_id();
        let implement_reasoning = self.reasoning_effort_selection.clone();
        let revise_cwd = Some(self.session.cwd.clone());
        let revise_model = self.user_turn_model();
        let revise_model_binding_id = self.user_turn_model_binding_id();
        let revise_reasoning = self.reasoning_effort_selection.clone();

        self.bottom_pane
            .open_popup_view(Box::new(ProposedPlanActionsView::new(
                self.app_event_tx.clone(),
                self.active_accent_color(),
                Box::new(move |tx: &AppEventSender| {
                    tx.send(AppEvent::Command(
                        AppCommand::user_turn_with_collaboration_mode(
                            vec![InputItem::Text {
                                text: "Implement Plan".to_string(),
                            }],
                            implement_cwd.clone(),
                            implement_model.clone(),
                            implement_model_binding_id.clone(),
                            implement_reasoning.clone(),
                            /*sandbox*/ None,
                            /*approval_policy*/ None,
                            CollaborationMode::Build,
                        ),
                    ));
                }),
                Box::new(move |tx: &AppEventSender, text: String| {
                    tx.send(AppEvent::Command(
                        AppCommand::user_turn_with_collaboration_mode(
                            vec![InputItem::Text { text }],
                            revise_cwd.clone(),
                            revise_model.clone(),
                            revise_model_binding_id.clone(),
                            revise_reasoning.clone(),
                            /*sandbox*/ None,
                            /*approval_policy*/ None,
                            CollaborationMode::Plan,
                        ),
                    ));
                }),
            )));
        self.set_status_message("Choose plan action");
    }

    fn refresh_active_proposed_plan_cell(&mut self) {
        let Some(plan) = self.active_proposed_plan.as_ref() else {
            return;
        };
        self.active_cell = Some(Box::new(history_cell::new_proposed_plan(
            plan.text.clone(),
            &self.session.cwd,
        )));
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn add_markdown_history(&mut self, title: &str, body: &str) {
        self.add_markdown_history_with_status(title, body, DotStatus::Completed);
    }

    pub(crate) fn add_padded_markdown_history(&mut self, title: &str, body: &str) {
        let mut lines = vec![Line::from(title.to_string()).bold()];
        append_markdown(
            body,
            /*width*/ None,
            Some(&self.session.cwd),
            &mut lines,
        );
        let lines = prefix_lines(lines, Span::raw("  "), Span::raw("  "));
        self.add_to_history(PlainHistoryCell::new(lines));
    }

    pub(super) fn add_markdown_history_with_status(
        &mut self,
        title: &str,
        body: &str,
        status: DotStatus,
    ) {
        self.add_markdown_history_with_status_without_redraw(title, body, status);
        self.frame_requester.schedule_frame();
    }

    pub(super) fn add_markdown_history_without_redraw(&mut self, title: &str, body: &str) {
        self.add_markdown_history_with_status_without_redraw(title, body, DotStatus::Completed);
    }

    pub(super) fn add_markdown_history_with_status_without_redraw(
        &mut self,
        title: &str,
        body: &str,
        status: DotStatus,
    ) {
        let is_ai_message = title == "Assistant" || title == "Reasoning";
        let mut lines = if is_ai_message {
            Vec::new()
        } else {
            vec![Line::from(title.to_string()).bold()]
        };
        if title == "Reasoning" {
            if self.collapse_reasoning {
                self.add_history_entry_without_redraw(
                    super::reasoning_view::collapsed_reasoning_history_cell(
                        body.to_string(),
                        &self.session.cwd,
                        reasoning_heading_for_status(status),
                        Self::reasoning_completed_heading_style(),
                        Self::reasoning_text_style(),
                        Self::reasoning_completed_dot_prefix(),
                    ),
                );
                return;
            }
            let mut body_lines = Vec::new();
            append_markdown(
                body,
                /*width*/ None,
                Some(&self.session.cwd),
                &mut body_lines,
            );
            Self::patch_lines_style(&mut body_lines, Self::reasoning_text_style());
            if let Some(first_line) = body_lines.first_mut() {
                first_line.spans.insert(
                    0,
                    Span::styled(
                        reasoning_heading_for_status(status),
                        Self::reasoning_completed_heading_style(),
                    ),
                );
            }
            lines.extend(body_lines);
        } else {
            append_markdown(body, None, Some(&self.session.cwd), &mut lines);
        }
        if is_ai_message {
            let prefix = if title == "Reasoning" {
                Self::reasoning_completed_dot_prefix()
            } else {
                self.dot_prefix(status)
            };
            self.add_history_entry_without_redraw(Box::new(
                history_cell::AgentMessageCell::new_ai_response_with_prefix(
                    lines, prefix, "  ", false,
                ),
            ));
        } else {
            self.add_history_entry_without_redraw(Box::new(PlainHistoryCell::new(lines)));
        }
    }

    pub(super) fn bulleted_markdown_lines(
        &self,
        body: &str,
        width: u16,
        prefix: Line<'static>,
    ) -> Vec<Line<'static>> {
        self.bulleted_markdown_cell(body, prefix)
            .display_lines(width.max(1))
    }

    pub(super) fn bulleted_markdown_cell(
        &self,
        body: &str,
        prefix: Line<'static>,
    ) -> history_cell::AgentMessageCell {
        self.bulleted_markdown_cell_with_style(body, prefix, Style::default())
    }

    pub(super) fn bulleted_markdown_cell_with_style(
        &self,
        body: &str,
        prefix: Line<'static>,
        style: Style,
    ) -> history_cell::AgentMessageCell {
        let mut lines = Vec::new();
        append_markdown(
            body,
            /*width*/ None,
            Some(&self.session.cwd),
            &mut lines,
        );
        Self::patch_lines_style(&mut lines, style);
        history_cell::AgentMessageCell::new_ai_response_with_prefix(lines, prefix, "  ", false)
    }

    pub(super) fn add_transcript_item(&mut self, item: TranscriptItem) {
        self.add_transcript_item_without_redraw(item);
        self.frame_requester.schedule_frame();
    }

    pub(super) fn add_transcript_item_without_redraw(&mut self, item: TranscriptItem) {
        match item.kind {
            TranscriptItemKind::User => {
                self.add_restored_user_prompt(item.body);
            }
            TranscriptItemKind::Assistant => {
                self.add_markdown_history_without_redraw("Assistant", &item.body)
            }
            TranscriptItemKind::Reasoning => {
                self.add_markdown_history_without_redraw("Reasoning", &item.body);
            }
            TranscriptItemKind::ToolCall => {
                self.add_history_entry_without_redraw(Box::new(
                    history_cell::AgentMessageCell::new_with_prefix(
                        vec![Self::running_tool_line(&item.title)],
                        self.dot_prefix(DotStatus::Pending),
                        "  ",
                        false,
                    ),
                ));
            }
            TranscriptItemKind::ToolResult => {
                self.add_history_entry_without_redraw(Box::new(ToolResultCell::new(
                    (!item.title.is_empty()).then(|| Self::ran_tool_line(&item.title)),
                    item.body,
                    Self::tool_dot_prefix(),
                    Line::from("  "),
                    Self::tool_text_style(),
                    false,
                )));
            }
            TranscriptItemKind::Error => {
                let cell = if item.title.is_empty() {
                    history_cell::new_error_event(item.body)
                } else {
                    history_cell::new_error_event_with_hint(item.body, Some(item.title))
                };
                self.add_history_entry_without_redraw(Box::new(cell));
            }
            TranscriptItemKind::Approval => {}
            TranscriptItemKind::System => {
                self.add_history_entry_without_redraw(Box::new(history_cell::new_info_event(
                    item.title,
                    Some(item.body),
                )));
            }
            TranscriptItemKind::TurnSummary => {
                // item.title contains model name, item.duration_ms contains seconds
                let summary = match item.body.as_str() {
                    "failed" => history_cell::TurnSummaryCell::new_failed(
                        InputMode::Build,
                        item.title.clone(),
                        self.active_accent_color(),
                    ),
                    "interrupted" => history_cell::TurnSummaryCell::new_interrupted(
                        InputMode::Build,
                        item.title.clone(),
                        self.active_accent_color(),
                    ),
                    _ => history_cell::TurnSummaryCell::new(
                        InputMode::Build,
                        item.title.clone(),
                        item.duration_ms,
                        self.active_accent_color(),
                    ),
                };
                self.add_history_entry_without_redraw(Box::new(summary));
            }
        }
    }

    pub(super) fn tool_preview_lines(&self, preview: &str) -> Vec<Line<'static>> {
        let width = self.last_known_width().saturating_sub(2).max(1);
        let mut preview_lines =
            truncated_tool_output_preview(preview, width, 2, crate::exec_cell::TOOL_CALL_MAX_LINES);
        for line in &mut preview_lines {
            line.spans = line
                .spans
                .clone()
                .into_iter()
                .map(|span| span.patch_style(Self::tool_text_style()))
                .collect();
        }
        preview_lines
    }

    pub(crate) fn add_to_history(&mut self, cell: impl HistoryCell + 'static) {
        self.add_history_entry_without_redraw(Box::new(cell));
        self.frame_requester.schedule_frame();
    }

    pub(super) fn flush_active_cell(&mut self) {
        if let Some(active) = self.active_cell.take() {
            self.add_history_entry_without_redraw(active);
        }
    }

    pub(super) fn bump_active_cell_revision(&mut self) {
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
    }

    /// Pop the oldest pending cell from the bottom pane and add it to history
    /// as a normal user input cell.
    pub(super) fn unqueue_oldest_pending(&mut self) {
        if let Some(text) = self.bottom_pane.pop_oldest_pending_cell() {
            let input_mode = self
                .queued_input_modes
                .pop_front()
                .unwrap_or(InputMode::Build);
            self.promoted_input_modes.push_back(input_mode);
            self.add_to_history(history_cell::new_user_prompt(
                text,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                self.active_accent_color(),
                input_mode,
            ));
        }
        self.queued_count = self.queued_count.saturating_sub(1);
    }

    pub(super) fn add_history_entry_without_redraw(&mut self, cell: Box<dyn HistoryCell>) {
        self.history.push(cell);
    }

    pub(crate) fn truncate_history_to_user_turn_count(&mut self, user_turn_count: usize) {
        let mut remaining_users = user_turn_count;
        let mut new_len = 0usize;
        for (idx, cell) in self.history.iter().enumerate() {
            let is_user = cell
                .as_ref()
                .as_any()
                .downcast_ref::<history_cell::UserHistoryCell>()
                .is_some();
            if is_user {
                if remaining_users == 0 {
                    break;
                }
                remaining_users -= 1;
            }
            new_len = idx + 1;
        }
        self.history.truncate(new_len);
        self.next_history_flush_index = self.next_history_flush_index.min(self.history.len());
        self.refresh_user_cell_indices();
        self.exit_selection_mode();
        self.frame_requester.schedule_frame();
    }
}

fn reasoning_heading_for_status(status: DotStatus) -> &'static str {
    match status {
        DotStatus::Completed => "Thought: ",
        DotStatus::Pending | DotStatus::Failed => "Thinking: ",
    }
}
