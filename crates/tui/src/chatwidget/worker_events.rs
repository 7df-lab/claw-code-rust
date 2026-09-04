//! Worker event dispatch for `ChatWidget`.
//!
//! This module keeps server/worker event handling out of the main chat surface
//! while preserving the existing state transitions and rendering side effects.

use std::path::Path;

use devo_protocol::CollaborationMode;
use devo_protocol::ProviderRetryPhase;
use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;

use crate::bottom_pane::ApprovalOverlay;
use crate::bottom_pane::ApprovalOverlayRequest;
use crate::bottom_pane::InputMode;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::ExecCell;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell;
use crate::transcript::lifecycle::TurnToolOutcome;
use crate::transcript::model::ToolPhase;

use super::ActiveToolCall;
use super::ChatWidget;
use super::DotStatus;
use super::PendingApprovalRequest;

fn format_retry_status_message(attempt: usize, backoff_ms: u64) -> String {
    let seconds = (backoff_ms as f64 / 1000.0).max(0.1);
    format!("Retrying provider request in {seconds:.1}s (attempt {attempt})")
}

fn normalize_approval_action_summary(action_summary: String) -> String {
    if action_summary == "apply_patch" {
        return "Patch".to_string();
    }
    if let Some(command) = action_summary.strip_prefix("shell_command: ") {
        return format!("Shell: {command}");
    }
    if let Some(command) = action_summary.strip_prefix("bash: ") {
        return format!("Shell: {command}");
    }
    action_summary
}

fn exec_call_is_unfinished(cell: &crate::exec_cell::ExecCell, tool_use_id: &str) -> bool {
    cell.iter_calls()
        .any(|call| call.call_id == tool_use_id && call.output.is_none())
}

impl ChatWidget {
    fn exec_tool_result_targets_unfinished_call(&self, tool_use_id: &str) -> bool {
        let is_unfinished = |cell: &ExecCell| exec_call_is_unfinished(cell, tool_use_id);
        self.active_cell
            .as_ref()
            .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
            .is_some_and(is_unfinished)
            || self
                .history
                .iter()
                .rev()
                .filter_map(|cell| cell.as_any().downcast_ref::<ExecCell>())
                .any(is_unfinished)
    }

    fn start_command_execution_cell(
        &mut self,
        tool_use_id: String,
        title: String,
        command: Vec<String>,
        parsed: Vec<ParsedCommand>,
        source: ExecCommandSource,
        input: Option<serde_json::Value>,
    ) {
        if matches!(source, ExecCommandSource::UserShell) {
            self.current_turn_has_user_shell_command = true;
            self.current_turn_mode = InputMode::Shell;
        }

        let already_rendered = self
            .active_cell
            .as_ref()
            .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
            .is_some_and(|cell| cell.contains_call(&tool_use_id))
            || self.history.iter().any(|cell| {
                cell.as_any()
                    .downcast_ref::<ExecCell>()
                    .is_some_and(|cell| cell.contains_call(&tool_use_id))
            });
        if already_rendered {
            return;
        }

        self.active_tool_calls.remove(&tool_use_id);
        self.pending_tool_calls
            .retain(|pending| pending.tool_use_id != tool_use_id);

        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && let Some(mut grouped) = cell.with_added_call(
                tool_use_id.clone(),
                command.clone(),
                parsed.clone(),
                source,
                None,
            )
        {
            if let Some(input) = input.clone() {
                grouped.set_tool_io_input(&tool_use_id, "exec_command".to_string(), input);
            }
            *cell = grouped;
            let seq = self.reserve_seq();
            self.active_tool_calls.insert(
                tool_use_id.clone(),
                ActiveToolCall {
                    tool_use_id,
                    seq,
                    tool_name: Some("exec_command".to_string()),
                    input: input.clone(),
                    title,
                    lines: Vec::new(),
                    output: String::new(),
                    parsed_commands: parsed.clone(),
                    exec_like: true,
                    owned_by_active_cell: true,
                    start_time: None,
                    phase: ToolPhase::Running,
                },
            );
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.frame_requester.schedule_frame();
            self.set_status_message("Tool started");
            return;
        }

        self.flush_active_cell();
        let parsed_commands = parsed.clone();
        let mut cell =
            new_active_exec_command(tool_use_id.clone(), command, parsed, source, None, true);
        if let Some(input) = input.clone() {
            cell.set_tool_io_input(&tool_use_id, "exec_command".to_string(), input);
        }
        self.active_cell = Some(Box::new(cell));
        let seq = self.reserve_seq();
        self.active_tool_calls.insert(
            tool_use_id.clone(),
            ActiveToolCall {
                tool_use_id,
                seq,
                tool_name: Some("exec_command".to_string()),
                input,
                title,
                lines: Vec::new(),
                output: String::new(),
                parsed_commands,
                exec_like: true,
                owned_by_active_cell: true,
                start_time: None,
                phase: ToolPhase::Running,
            },
        );
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.frame_requester.schedule_frame();
        self.set_status_message("Tool started");
    }

    fn exec_cell_has_call(&self, tool_use_id: &str) -> bool {
        self.active_cell
            .as_ref()
            .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
            .is_some_and(|cell| cell.contains_call(tool_use_id))
            || self.history.iter().rev().any(|cell| {
                cell.as_any()
                    .downcast_ref::<ExecCell>()
                    .is_some_and(|cell| cell.contains_call(tool_use_id))
            })
    }

    fn exec_command_parts_for_tool(
        tool: &crate::transcript::model::ToolCellModel,
        cwd: &Path,
    ) -> (String, Vec<String>, Vec<ParsedCommand>) {
        let command = tool
            .command
            .clone()
            .or_else(|| {
                tool.input.as_ref().and_then(|input| {
                    input
                        .get("command")
                        .or_else(|| input.get("cmd"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
            })
            .unwrap_or_else(|| {
                tool.tool_name
                    .clone()
                    .unwrap_or_else(|| "exec_command".into())
            });
        let command_parts = crate::exec_command::split_command_string(&command);
        let mut parsed = tool.parsed_commands.clone();
        crate::read_display::normalize_read_actions(&mut parsed, cwd);
        (command, command_parts, parsed)
    }

    fn update_exec_cell_call(
        &mut self,
        tool_use_id: &str,
        command_parts: Vec<String>,
        parsed: Vec<ParsedCommand>,
    ) -> bool {
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && cell.update_call(tool_use_id, command_parts.clone(), parsed.clone())
        {
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.frame_requester.schedule_frame();
            return true;
        }
        self.history[self.next_history_flush_index..]
            .iter_mut()
            .rev()
            .any(|cell| {
                cell.as_any_mut()
                    .downcast_mut::<ExecCell>()
                    .is_some_and(|cell| {
                        cell.update_call(tool_use_id, command_parts.clone(), parsed.clone())
                    })
            })
    }

    pub(super) fn complete_exec_tool_from_committed(
        &mut self,
        tool: &crate::transcript::model::ToolCellModel,
    ) -> bool {
        if !tool.exec_like {
            return false;
        }
        let tool_use_id = tool.tool_use_id.as_str();
        let preview = tool
            .tool_display_content
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| tool.output_preview.clone());
        let output = tool.tool_output.clone().unwrap_or_else(|| {
            if preview.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(preview.clone())
            }
        });
        let command_output = CommandOutput {
            exit_code: if tool.is_error { 1 } else { 0 },
            aggregated_output: preview.clone(),
            formatted_output: preview,
        };
        let duration = std::time::Duration::from_millis(0);

        if let (Some(tool_name), Some(input)) = (&tool.tool_name, &tool.input)
            && let Some(cell) = self
                .active_cell
                .as_mut()
                .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        {
            cell.set_tool_io_input(tool_use_id, tool_name.clone(), input.clone());
            cell.complete_tool_io(
                tool_use_id,
                output.clone(),
                tool.tool_display_content.clone(),
            );
        }

        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && cell.complete_call(tool_use_id, command_output.clone(), duration)
        {
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.frame_requester.schedule_frame();
            self.set_status_message(if tool.is_error {
                "Tool returned an error"
            } else {
                "Tool completed"
            });
            return true;
        }

        for cell in self
            .history
            .iter_mut()
            .skip(self.next_history_flush_index)
            .rev()
            .filter_map(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        {
            if cell.complete_call(tool_use_id, command_output.clone(), duration) {
                self.frame_requester.schedule_frame();
                return true;
            }
        }

        false
    }

    pub(super) fn sync_exec_cells_from_projector(&mut self) {
        use devo_protocol::protocol::ExecCommandSource;

        let exec_tools: Vec<_> = self
            .transcript_projector
            .live_tools()
            .filter(|tool| {
                crate::chatwidget::history_commit::tool_uses_exec_cell(tool)
                    && !self.detached_exec_tool_ids.contains(&tool.tool_use_id)
            })
            .cloned()
            .collect();
        for tool in exec_tools {
            let tool_use_id = tool.tool_use_id.clone();
            let (_command, command_parts, parsed) =
                Self::exec_command_parts_for_tool(&tool, &self.session.cwd);
            if self.exec_cell_has_call(&tool_use_id) {
                let _ = self.update_exec_cell_call(&tool_use_id, command_parts, parsed);
                if !tool.output_preview.is_empty()
                    && let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                {
                    let existing = cell
                        .iter_calls()
                        .find(|call| call.call_id == tool_use_id)
                        .and_then(|call| call.output.as_ref())
                        .map(|output| output.aggregated_output.len())
                        .unwrap_or(0);
                    if tool.output_preview.len() > existing {
                        let delta = tool.output_preview[existing..].to_string();
                        let _ = cell.append_output(&tool_use_id, &delta);
                        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                    }
                }
            } else if !self.exec_tool_result_targets_unfinished_call(&tool_use_id)
                && !self.exec_cell_has_call(&tool_use_id)
            {
                let (command, command_parts, parsed) =
                    Self::exec_command_parts_for_tool(&tool, &self.session.cwd);
                self.start_command_execution_cell(
                    tool_use_id.clone(),
                    command,
                    command_parts,
                    parsed,
                    tool.command_source.unwrap_or(ExecCommandSource::Agent),
                    tool.input.clone(),
                );
            } else if let Some(call) = self.active_tool_calls.get_mut(&tool_use_id)
                && tool.output_preview.len() > call.output.len()
            {
                let delta = tool.output_preview[call.output.len()..].to_string();
                call.output.push_str(&delta);
                if let Some(cell) = self
                    .active_cell
                    .as_mut()
                    .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                {
                    let _ = cell.append_output(&tool_use_id, &delta);
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                }
            }
            if tool.phase.is_terminal() {
                let _ = self.complete_exec_tool_from_committed(&tool);
            }
        }
    }

    pub(crate) fn handle_worker_event(&mut self, event: WorkerEvent) {
        if self.route_worker_event_through_projector(&event) {
            return;
        }
        match event {
            WorkerEvent::SessionActivated { .. } => {}
            WorkerEvent::TurnStarted {
                model,
                model_binding_id,
                reasoning_effort_selection,
                reasoning_effort,
                turn_id,
                ..
            } => {
                self.active_turn_id = Some(turn_id);
                self.failed_turn_visually_finalized = false;
                if let Some(input_mode) = self.promoted_input_modes.pop_front() {
                    self.current_turn_mode = input_mode;
                }
                self.committed_server_assistant_in_turn = false;
                self.boundary_committed_assistant_items.clear();
                self.pending_proposed_plan_actions = false;
                self.current_turn_has_user_shell_command = false;
                self.update_session_model_selection(model, model_binding_id);
                self.reasoning_effort_selection = reasoning_effort_selection;
                self.session.reasoning_effort = reasoning_effort;
                self.refresh_header_box();
                self.busy = true;
                self.active_text_items.clear();
                self.detached_exec_tool_ids.clear();
                self.active_proposed_plan = None;
                self.bottom_pane.set_task_running(true);
            }
            WorkerEvent::InterruptFailed { message } => {
                self.interrupt_failed(message);
            }
            WorkerEvent::ProviderRetryStatus {
                turn_id,
                attempt,
                backoff_ms,
                provider: _,
                model: _,
                phase,
                message,
            } => {
                if self.active_turn_id != Some(turn_id) {
                    return;
                }
                match phase {
                    ProviderRetryPhase::Scheduled => {
                        let retry_message = if message.trim().is_empty() {
                            format_retry_status_message(attempt, backoff_ms)
                        } else {
                            message
                        };
                        self.bottom_pane.ensure_status_indicator();
                        if let Some(status) = self.bottom_pane.status_widget_mut() {
                            status.update_inline_message(Some(retry_message.clone()));
                        }
                        self.set_status_message(&retry_message);
                    }
                    ProviderRetryPhase::Resumed => {
                        if let Some(status) = self.bottom_pane.status_widget_mut() {
                            status.update_inline_message(None);
                        }
                        self.set_status_message("Retrying provider request");
                    }
                }
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::ProposedPlanStarted { item_id } => {
                self.start_proposed_plan(item_id);
            }
            WorkerEvent::ProposedPlanDelta { item_id, delta } => {
                self.push_proposed_plan_delta(item_id, delta);
            }
            WorkerEvent::ProposedPlanCompleted {
                item_id,
                final_text,
            } => {
                self.complete_proposed_plan(item_id, final_text);
            }
            WorkerEvent::TextDelta(text) => {
                self.apply_legacy_text_delta(TextItemKind::Assistant, text);
                self.set_status_message("Generating");
            }
            WorkerEvent::ReasoningDelta(text) => {
                self.apply_legacy_text_delta(TextItemKind::Reasoning, text);
                self.set_status_message("Thinking");
            }
            WorkerEvent::AssistantMessageCompleted(text) => {
                if !self.committed_server_assistant_in_turn
                    && !self.has_native_text_item(TextItemKind::Assistant)
                    && !self
                        .active_text_items
                        .iter()
                        .any(|item| item.kind == TextItemKind::Assistant)
                {
                    self.apply_legacy_text_completed(TextItemKind::Assistant, text);
                }
                self.set_status_message("Generating");
            }
            WorkerEvent::ReasoningCompleted(text) => {
                if !self.has_native_text_item(TextItemKind::Reasoning) {
                    self.apply_legacy_text_completed(TextItemKind::Reasoning, text);
                }
                self.set_status_message("Thought");
            }
            WorkerEvent::ShellCommandFinished { exit_code } => {
                let standalone_shell = self.active_turn_id.is_none();
                let interrupted = exit_code.is_none();
                if standalone_shell {
                    // A standalone shell command runs outside any agent turn,
                    // so this event is its commit boundary: flush the live
                    // exec cell into scrollback before the summary row lands.
                    let outcome = if interrupted {
                        TurnToolOutcome::Interrupted
                    } else {
                        TurnToolOutcome::Completed
                    };
                    self.clear_turn_live_projection(outcome);
                }
                let accent_color = self.active_accent_color();
                let cell = if interrupted {
                    history_cell::TurnSummaryCell::new_interrupted(
                        InputMode::Shell,
                        "Shell".to_string(),
                        accent_color,
                    )
                } else {
                    history_cell::TurnSummaryCell::new(
                        InputMode::Shell,
                        "Shell".to_string(),
                        None,
                        accent_color,
                    )
                };
                self.add_to_history(cell);
                self.set_status_message("Shell command completed");
                self.current_turn_has_user_shell_command = false;
                self.current_turn_mode = InputMode::Build;
                if standalone_shell {
                    self.busy = false;
                    self.bottom_pane.set_task_running(false);
                }
            }
            WorkerEvent::PlanUpdated { explanation, steps } => {
                self.on_plan_updated(explanation, steps);
                self.set_status_message("Plan updated");
            }
            WorkerEvent::ApprovalRequest {
                session_id,
                turn_id,
                approval_id,
                action_summary,
                justification,
                resource,
                available_scopes,
                path,
                host,
                target,
                command_pattern,
                command_prefix,
            } => {
                self.commit_active_streams(DotStatus::Completed);
                let action_summary = normalize_approval_action_summary(action_summary);
                self.seen_approval_decisions.remove(&approval_id);
                let overlay_request = ApprovalOverlayRequest {
                    session_id,
                    turn_id,
                    approval_id: approval_id.clone(),
                    action_summary: action_summary.clone(),
                    justification,
                    resource,
                    available_scopes,
                    path,
                    host,
                    target,
                    command_pattern,
                    command_prefix,
                };
                let duplicate = self
                    .pending_approval
                    .as_ref()
                    .is_some_and(|pending| pending.approval_id == approval_id)
                    || self
                        .queued_approvals
                        .iter()
                        .any(|queued| queued.approval_id == approval_id);
                if !duplicate {
                    if self.pending_approval.is_none() {
                        self.pending_approval = Some(PendingApprovalRequest {
                            session_id,
                            turn_id,
                            approval_id,
                            action_summary,
                        });
                        self.bottom_pane
                            .open_popup_view(Box::new(ApprovalOverlay::new(
                                overlay_request,
                                self.app_event_tx.clone(),
                                self.active_accent_color(),
                            )));
                    } else {
                        self.queued_approvals.push_back(overlay_request);
                    }
                }
                self.busy = true;
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Approval required");
            }
            WorkerEvent::RequestUserInput {
                session_id,
                turn_id,
                request_id,
                questions,
            } => {
                self.commit_active_streams(DotStatus::Completed);
                self.bottom_pane
                    .open_request_user_input(session_id, turn_id, request_id, questions);
                self.busy = true;
                self.bottom_pane.set_task_running(true);
                self.set_status_message("Input requested");
            }
            WorkerEvent::UserInputResolved { request_id } => {
                self.bottom_pane.dismiss_user_input(&request_id);
            }
            WorkerEvent::ApprovalDecision {
                approval_id,
                decision,
                scope,
                tool_name,
                rationale,
            } => {
                self.bottom_pane.dismiss_approval(&approval_id);
                let resolved_active = self
                    .pending_approval
                    .as_ref()
                    .is_some_and(|pending| pending.approval_id == approval_id);
                if resolved_active {
                    self.pending_approval = None;
                    if let Some(next) = self.queued_approvals.pop_front() {
                        self.pending_approval = Some(PendingApprovalRequest {
                            session_id: next.session_id,
                            turn_id: next.turn_id,
                            approval_id: next.approval_id.clone(),
                            action_summary: next.action_summary.clone(),
                        });
                        self.bottom_pane
                            .open_popup_view(Box::new(ApprovalOverlay::new(
                                next,
                                self.app_event_tx.clone(),
                                self.active_accent_color(),
                            )));
                    }
                } else {
                    self.queued_approvals
                        .retain(|queued| queued.approval_id != approval_id);
                }
                if !self.seen_approval_decisions.insert(approval_id) {
                    self.bottom_pane.set_task_running(self.busy);
                    return;
                }
                if scope == "auto_review" {
                    let summary = tool_name.unwrap_or_else(|| "tool request".to_string());
                    let cell = if decision == "approve" {
                        history_cell::new_guardian_approved_action_request(summary)
                    } else {
                        history_cell::new_guardian_denied_action_request(summary)
                    };
                    self.add_to_history(cell);
                    if let Some(rationale) = rationale {
                        self.add_to_history(history_cell::new_info_event(
                            format!("Auto-reviewer rationale: {rationale}"),
                            None,
                        ));
                    }
                } else {
                    let symbol = if decision == "approve" { "→ " } else { "✗" };
                    self.add_to_history(history_cell::new_info_event(
                        format!("{symbol} Permission request {decision} ({scope})"),
                        None,
                    ));
                }
                self.bottom_pane.set_task_running(self.busy);
            }
            WorkerEvent::UsageUpdated {
                total_input_tokens,
                total_output_tokens,
                total_tokens: _,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
            } => {
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                // Context length uses latest-query totals, not cumulative session
                // total_input_tokens.
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = last_query_input_tokens;
                self.sync_bottom_pane_summary();
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::ContextUsageUpdated { occupancy } => {
                self.last_query_total_tokens = occupancy.total_tokens as usize;
                self.last_context_occupancy = Some(occupancy);
                self.sync_bottom_pane_summary();
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::TurnFinished {
                stop_reason,
                turn_count,
                total_input_tokens,
                total_output_tokens,
                total_tokens: _,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
                prompt_token_estimate,
            } => {
                let was_interrupted = stop_reason.contains("Interrupted");
                let was_failed = stop_reason == "Failed";
                let failed_turn_was_finalized = was_failed && self.failed_turn_visually_finalized;
                if !failed_turn_was_finalized {
                    let stream_status = if was_failed {
                        DotStatus::Failed
                    } else {
                        DotStatus::Completed
                    };
                    self.commit_active_streams(stream_status);
                }
                if !failed_turn_was_finalized {
                    let tool_outcome = if was_failed {
                        TurnToolOutcome::Failed
                    } else if was_interrupted {
                        TurnToolOutcome::Interrupted
                    } else {
                        TurnToolOutcome::Completed
                    };
                    self.clear_turn_live_projection(tool_outcome);
                }
                if !failed_turn_was_finalized
                    && (was_interrupted || was_failed)
                    && let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                {
                    cell.mark_failed();
                }
                if !failed_turn_was_finalized {
                    self.flush_active_cell();
                }
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                if let Some(pending) = self.pending_approval.as_ref() {
                    self.bottom_pane.dismiss_approval(&pending.approval_id);
                }
                self.pending_approval = None;
                self.queued_approvals.clear();
                self.bottom_pane.dismiss_all_user_inputs();
                self.committed_server_assistant_in_turn = false;
                self.busy = false;
                self.active_turn_id = None;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                let elapsed = if failed_turn_was_finalized {
                    None
                } else {
                    self.bottom_pane
                        .status_widget()
                        .map(|status| status.elapsed_seconds())
                        .filter(|&secs| secs > 0)
                };
                self.bottom_pane.set_task_running(false);
                if !failed_turn_was_finalized {
                    let input_mode = if self.current_turn_has_user_shell_command {
                        InputMode::Shell
                    } else {
                        self.current_turn_mode
                    };
                    let model_name = if self.current_turn_has_user_shell_command {
                        "Shell".to_string()
                    } else {
                        self.session
                            .model
                            .as_ref()
                            .map(|m| m.display_name.clone())
                            .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                            .unwrap_or_default()
                    };
                    let accent_color = self.active_accent_color();
                    let cell = if was_failed {
                        self.set_status_message("Query failed");
                        history_cell::TurnSummaryCell::new_failed(
                            input_mode,
                            model_name,
                            accent_color,
                        )
                    } else if was_interrupted {
                        self.set_status_message("Ready");
                        history_cell::TurnSummaryCell::new_interrupted(
                            input_mode,
                            model_name,
                            accent_color,
                        )
                    } else {
                        self.set_status_message("Ready");
                        history_cell::TurnSummaryCell::new(
                            input_mode,
                            model_name,
                            elapsed,
                            accent_color,
                        )
                    };
                    self.add_to_history(cell);
                    if was_failed {
                        self.failed_turn_visually_finalized = true;
                    }
                }
                self.current_turn_has_user_shell_command = false;
                self.current_turn_mode = InputMode::Build;
                if was_failed {
                    self.pending_proposed_plan_actions = false;
                } else {
                    self.maybe_open_proposed_plan_actions();
                }
            }
            WorkerEvent::TurnFailed {
                message,
                hint,
                turn_count,
                total_input_tokens,
                total_output_tokens,
                total_tokens: _,
                total_cache_read_tokens,
                prompt_token_estimate,
                last_query_input_tokens,
            } => {
                self.finish_session_resume();
                let failed_turn_was_finalized = self.failed_turn_visually_finalized;
                if !failed_turn_was_finalized {
                    self.commit_active_streams(DotStatus::Failed);
                    self.clear_turn_live_projection(TurnToolOutcome::Failed);
                    if let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                    {
                        cell.mark_failed();
                    }
                    self.flush_active_cell();
                }
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                if let Some(pending) = self.pending_approval.as_ref() {
                    self.bottom_pane.dismiss_approval(&pending.approval_id);
                }
                self.pending_approval = None;
                self.queued_approvals.clear();
                self.bottom_pane.dismiss_all_user_inputs();
                self.committed_server_assistant_in_turn = false;
                self.busy = false;
                self.active_turn_id = None;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                if !failed_turn_was_finalized {
                    let input_mode = if self.current_turn_has_user_shell_command {
                        InputMode::Shell
                    } else {
                        self.current_turn_mode
                    };
                    let model_name = if self.current_turn_has_user_shell_command {
                        "Shell".to_string()
                    } else {
                        self.session
                            .model
                            .as_ref()
                            .map(|m| m.display_name.clone())
                            .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                            .unwrap_or_default()
                    };
                    let accent_color = self.active_accent_color();
                    self.add_to_history(history_cell::new_error_event_with_hint(message, hint));
                    self.add_to_history(history_cell::TurnSummaryCell::new_failed(
                        input_mode,
                        model_name,
                        accent_color,
                    ));
                    self.failed_turn_visually_finalized = true;
                }
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Query failed; see error above");
                self.current_turn_has_user_shell_command = false;
                self.current_turn_mode = InputMode::Build;
                self.pending_proposed_plan_actions = false;
            }
            WorkerEvent::ProviderValidationSucceeded { reply_preview } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_succeeded(reply_preview.clone());
                }
                self.drain_onboarding_transcript_events();
                self.add_to_history(history_cell::new_info_event(
                    format!("Validation reply: {reply_preview}"),
                    Some("provider validation succeeded".to_string()),
                ));
                self.busy = false;
                self.set_status_message("Saving provider");
            }
            WorkerEvent::ProviderValidationFailed { message, hint } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_failed(message.clone(), hint.clone());
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                let transcript_hint =
                    hint.unwrap_or_else(|| "provider validation failed".to_string());
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some(transcript_hint),
                ));
                self.set_status_message("Provider validation failed");
            }
            WorkerEvent::ProvidersListed {
                providers,
                template_provider_ids,
                connected_provider_ids,
                connection_models,
            } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_providers_listed_with_status_and_models(
                        providers,
                        template_provider_ids,
                        connected_provider_ids,
                        connection_models,
                    );
                }
                self.drain_onboarding_transcript_events();
            }
            WorkerEvent::ProviderUpserted {
                provider,
                default_model,
            } => {
                let onboarding_was_active = self.onboarding.is_some();
                if self.onboarding.is_some() {
                    if let Some(onboarding) = self.onboarding.as_mut() {
                        onboarding.on_provider_upserted(&provider, default_model.as_deref());
                    }
                    self.drain_onboarding_transcript_events();
                    if let Some(result) = self
                        .onboarding
                        .as_mut()
                        .and_then(crate::onboarding_widget::OnboardingWidget::take_result)
                    {
                        self.handle_onboarding_result(result);
                    }
                }
                if !onboarding_was_active {
                    self.add_to_history(history_cell::new_info_event(
                        format!("Provider saved: {}", provider.name),
                        Some("provider upserted".to_string()),
                    ));
                }
            }
            WorkerEvent::ProviderUpsertFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_save_failed(message.clone());
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("provider upsert failed".to_string()),
                ));
                self.set_status_message("Provider save failed");
            }
            WorkerEvent::ProviderDisconnected { provider_id } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_disconnected(&provider_id);
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.set_status_message("Provider disconnected");
            }
            WorkerEvent::ProviderDisconnectFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_disconnect_failed();
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("provider disconnect failed".to_string()),
                ));
                self.set_status_message("Provider disconnect failed");
            }
            WorkerEvent::ProviderModelRemoved {
                provider_id,
                model_id,
            } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_model_removed(&provider_id, &model_id);
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.set_status_message("Model removed");
            }
            WorkerEvent::ProviderModelRemoveFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_model_remove_failed();
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("provider model removal failed".to_string()),
                ));
                self.set_status_message("Model removal failed");
            }
            WorkerEvent::SessionsListed { sessions } => {
                self.bottom_pane.update_resume_sessions(sessions);
                self.set_status_message("Resume session");
            }
            WorkerEvent::SessionsListFailed { message } => {
                self.bottom_pane.update_resume_list_error(message);
                self.set_status_message("Failed to load sessions");
            }
            WorkerEvent::SessionPreviewLoaded {
                session_id,
                messages,
            } => {
                self.bottom_pane
                    .update_resume_preview(session_id, Ok(messages));
            }
            WorkerEvent::SessionPreviewFailed {
                session_id,
                message,
            } => {
                self.bottom_pane
                    .update_resume_preview(session_id, Err(message));
            }
            WorkerEvent::SubagentDiscovered { agent } => {
                self.on_subagent_discovered(agent);
            }
            WorkerEvent::SubagentMonitor { event } => {
                self.on_subagent_monitor_event(event);
            }
            WorkerEvent::SkillsListed {
                skills,
                picker_skills,
                open_picker,
            } => {
                self.bottom_pane.set_skill_mentions(Some(skills));
                if open_picker {
                    self.on_skills_listed_for_picker(picker_skills);
                } else {
                    self.skills_snapshot = Some(picker_skills);
                    if let Some(name) = self.skills_reopen_detail.take() {
                        self.open_skill_detail(&name);
                    }
                }
            }
            WorkerEvent::McpServersListed { servers } => {
                self.on_mcp_servers_listed(servers);
            }
            WorkerEvent::McpToolsListed { name, tools } => {
                self.on_mcp_tools_listed(name, tools);
            }
            WorkerEvent::McpServerEnabled {
                name,
                enabled,
                servers,
            } => {
                let action = if enabled { "enabled" } else { "disabled" };
                let status = servers
                    .iter()
                    .find(|server| server.name == name)
                    .map(|server| server.status.as_str())
                    .unwrap_or("unknown");
                self.set_mcp_reopen_detail(Some(name.clone()));
                if status == "failed" {
                    self.set_status_message(format!(
                        "MCP `{name}` {action} in config but runtime startup failed"
                    ));
                } else {
                    self.set_status_message(format!("MCP `{name}` {action}"));
                }
                self.on_mcp_servers_listed(servers);
            }
            WorkerEvent::McpServerEnableFailed { name, message } => {
                self.set_mcp_reopen_detail(None);
                self.add_to_history(crate::history_cell::new_error_event_with_hint(
                    format!("Failed to update MCP server `{name}`: {message}"),
                    Some("mcp enable/disable failed".to_string()),
                ));
                self.set_status_message(format!("Failed to update MCP `{name}`"));
            }
            WorkerEvent::AcpAvailableCommandsUpdated { commands } => {
                self.acp_available_commands = commands;
                let count = self.acp_available_commands.len();
                self.set_status_message(format!("ACP commands updated: {count}"));
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::AcpCurrentModeUpdated { current_mode_id } => {
                self.acp_current_mode_id = Some(current_mode_id);
                let current_mode_id = self.acp_current_mode_id.as_deref().unwrap_or("unknown");
                self.set_status_message(format!("ACP mode: {current_mode_id}"));
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::AcpConfigOptionsUpdated { config_options } => {
                self.acp_config_options = config_options;
                let count = self.acp_config_options.len();
                self.set_status_message(format!("ACP config options updated: {count}"));
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::AcpUsageUpdated { used, size, cost } => {
                self.acp_usage = Some((used, size, cost));
                let (used, size, _) = self.acp_usage.as_ref().expect("ACP usage was just stored");
                self.set_status_message(format!("ACP context: {used}/{size} tokens"));
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::ReferenceSearchUpdated { snapshot } => {
                self.bottom_pane.on_reference_search_result(snapshot);
            }
            WorkerEvent::NewSessionPrepared {
                cwd,
                model,
                model_binding_id,
                reasoning_effort_selection,
                reasoning_effort,
                permission_preset,
                collaboration_mode,
                active_agent_label,
                last_query_total_tokens: _,
                last_query_input_tokens: _,
                total_cache_read_tokens: _,
            } => {
                self.finish_session_resume();
                self.session.cwd = cwd;
                self.update_session_model_selection(model, model_binding_id);
                self.reasoning_effort_selection = reasoning_effort_selection;
                self.session.reasoning_effort = reasoning_effort;
                self.session.active_agent_label = active_agent_label.clone();
                self.bottom_pane.set_active_agent_label(active_agent_label);
                self.reset_subagent_monitor();
                let should_append_header = self.history_has_non_header_content();
                self.active_cell = None;
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                if let Some(pending) = self.pending_approval.as_ref() {
                    self.bottom_pane.dismiss_approval(&pending.approval_id);
                }
                self.pending_approval = None;
                self.queued_approvals.clear();
                self.bottom_pane.dismiss_all_user_inputs();
                self.active_text_items.clear();
                self.committed_server_assistant_in_turn = false;
                let restored_mode = InputMode::from_collaboration_mode(collaboration_mode);
                self.current_turn_mode = restored_mode;
                self.bottom_pane.set_input_mode(restored_mode);
                self.permission_preset = permission_preset;
                self.queued_count = 0;
                self.queued_input_modes.clear();
                self.promoted_input_modes.clear();
                self.editing_queue_item_id = None;
                self.bottom_pane.clear_pending_cells();
                self.seen_approval_decisions.clear();
                self.busy = false;
                self.turn_count = 0;
                self.total_input_tokens = 0;
                self.total_output_tokens = 0;
                self.total_cache_read_tokens = 0;
                self.last_query_total_tokens = 0;
                self.last_query_input_tokens = 0;
                self.prompt_token_estimate = 0;
                self.last_context_occupancy = None;
                self.effective_context_window = self.default_compaction_token_limit;
                if should_append_header {
                    self.push_session_header(/*is_first_run*/ false, None);
                } else {
                    self.refresh_header_box();
                }
                self.set_status_message("New session ready; send a prompt to start it");
            }
            WorkerEvent::SessionSwitched {
                session_id,
                cwd,
                title,
                model,
                model_binding_id,
                reasoning_effort_selection,
                reasoning_effort,
                active_agent_label,
                total_input_tokens,
                total_output_tokens,
                total_tokens: _,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
                prompt_token_estimate,
                history_items,
                rich_history_items,
                loaded_item_count,
                pending_texts: _,
                collaboration_mode,
                permission_preset,
                effective_context_window,
                last_context_occupancy,
            } => {
                self.finish_session_resume();
                self.session.cwd = cwd;
                if let Some(model) = model {
                    self.update_session_model_selection(model, model_binding_id);
                }
                self.reasoning_effort_selection = reasoning_effort_selection;
                self.session.reasoning_effort = reasoning_effort;
                self.session.active_agent_label = active_agent_label.clone();
                self.bottom_pane.set_active_agent_label(active_agent_label);
                self.reset_subagent_monitor();
                if let Some(pending) = self.pending_approval.as_ref() {
                    self.bottom_pane.dismiss_approval(&pending.approval_id);
                }
                self.pending_approval = None;
                self.queued_approvals.clear();
                self.bottom_pane.dismiss_all_user_inputs();
                self.history.clear();
                self.next_history_flush_index = 0;
                self.seen_approval_decisions.clear();
                self.active_text_items.clear();
                self.committed_server_assistant_in_turn = false;
                self.active_proposed_plan = None;
                self.pending_proposed_plan_actions = false;
                let restored_mode = InputMode::from_collaboration_mode(collaboration_mode);
                self.current_turn_mode = restored_mode;
                self.bottom_pane.set_input_mode(restored_mode);
                if let Some(preset) = permission_preset {
                    self.permission_preset = preset;
                }
                self.queued_input_modes.clear();
                self.promoted_input_modes.clear();
                self.editing_queue_item_id = None;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                self.last_context_occupancy = last_context_occupancy;
                self.effective_context_window = effective_context_window;
                if !self.rebuild_restored_session_history_from_rich_items(
                    &rich_history_items,
                    loaded_item_count,
                    &session_id,
                    title.as_deref(),
                ) {
                    self.rebuild_restored_session_history(
                        history_items,
                        loaded_item_count,
                        &session_id,
                        title.as_deref(),
                    );
                }
                // Queue entries arrive via QueueUpdated after subscription/create.
                self.queued_count = 0;
                self.queued_input_modes.clear();
                self.bottom_pane.clear_pending_cells();
                self.busy = false;
                if collaboration_mode == CollaborationMode::Plan
                    && history_awaits_proposed_plan_decision(&rich_history_items)
                {
                    self.pending_proposed_plan_actions = true;
                    self.maybe_open_proposed_plan_actions();
                } else {
                    self.set_status_message("Session switched");
                }
                self.sync_bottom_pane_summary();
                self.refresh_header_box();
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::GoalStatusLoaded { goal } => {
                self.show_goal_status(goal);
            }
            WorkerEvent::GoalUpdated { goal } => {
                self.show_goal_updated(goal);
            }
            WorkerEvent::GoalReplaceConfirmationRequested {
                current_goal,
                objective,
            } => {
                self.show_goal_replace_confirmation(current_goal, objective);
            }
            WorkerEvent::GoalEditLoaded { goal } => {
                self.show_goal_edit_prompt(goal);
            }
            WorkerEvent::GoalCleared { cleared } => {
                self.show_goal_cleared(cleared);
            }
            WorkerEvent::GoalOperationFailed { message } => {
                self.show_goal_operation_failed(message);
            }
            WorkerEvent::BtwStarted { question } => {
                self.set_status_message(format!("Asking side question: {question}"));
            }
            WorkerEvent::BtwCompleted {
                question: _,
                answer,
            } => {
                self.add_markdown_history("BTW", &answer);
                self.set_status_message("Side question answered");
            }
            WorkerEvent::BtwFailed { message } => {
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("BTW failed".to_string()),
                ));
                self.set_status_message("Side question failed");
            }
            WorkerEvent::SessionRenamed { session_id, title } => {
                let parsed_session_id = devo_core::SessionId::try_from(session_id.as_str()).ok();
                self.bottom_pane
                    .update_resume_rename(parsed_session_id, Ok(title.clone()));
                self.add_to_history(history_cell::new_info_event(
                    format!("renamed {session_id} to {title}"),
                    None,
                ));
                self.set_status_message("Session renamed");
            }
            WorkerEvent::SessionRenameFailed {
                session_id,
                message,
            } => {
                self.bottom_pane
                    .update_resume_rename(session_id, Err(message.clone()));
                self.set_status_message("Session rename failed");
            }
            WorkerEvent::SessionDeleted { session_id } => {
                let parsed_session_id = devo_core::SessionId::try_from(session_id.as_str()).ok();
                self.bottom_pane
                    .update_resume_delete(parsed_session_id, Ok(()));
                self.add_to_history(history_cell::new_info_event(
                    format!("deleted session {session_id}"),
                    None,
                ));
                self.set_status_message("Session deleted");
            }
            WorkerEvent::SessionDeleteFailed {
                session_id,
                message,
            } => {
                self.bottom_pane
                    .update_resume_delete(session_id, Err(message.clone()));
                self.set_status_message("Session delete failed");
            }
            WorkerEvent::EffectiveContextWindowUpdated {
                effective_context_window,
            } => {
                self.note_effective_context_window_updated(effective_context_window);
            }
            WorkerEvent::SessionCompactionStarted => {
                if self.status_message != "Session compaction in progress" {
                    self.flush_active_cell();
                    self.add_to_history(history_cell::new_live_aligned_info_event(
                        "Compaction started".to_string(),
                        None,
                    ));
                }
                self.busy = true;
                self.bottom_pane.set_task_running(true);
                if let Some(status) = self.bottom_pane.status_widget_mut() {
                    status.update_header("Compacting session".to_string());
                }
                self.set_status_message("Session compaction in progress");
            }
            WorkerEvent::SessionCompacted {
                total_input_tokens,
                total_output_tokens,
                total_tokens: _,
                last_query_total_tokens,
                last_query_input_tokens,
                prompt_token_estimate,
            } => {
                self.busy = false;
                self.active_turn_id = None;
                self.bottom_pane.set_task_running(false);
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                if self.status_message != "Context compacted" {
                    self.add_to_history(history_cell::new_live_aligned_info_event(
                        "Context compacted".to_string(),
                        None,
                    ));
                }
                self.set_status_message("Session compacted");
            }
            WorkerEvent::ContextCompactionCompleted { title: _ } => {
                // Mid-turn auto-compaction only emits item lifecycle events (not
                // session/compaction/completed). Clear the compacting indicator here.
                if self.active_turn_id.is_some() {
                    if let Some(status) = self.bottom_pane.status_widget_mut() {
                        status.update_header("Working".to_string());
                    }
                } else {
                    self.busy = false;
                    self.bottom_pane.set_task_running(false);
                }
                if self.status_message != "Session compacted"
                    && self.status_message != "Context compacted"
                {
                    self.add_to_history(history_cell::new_live_aligned_info_event(
                        "Context compacted".to_string(),
                        None,
                    ));
                }
                self.set_status_message("Context compacted");
            }
            WorkerEvent::SessionCompactionFailed { message } => {
                self.busy = false;
                self.active_turn_id = None;
                self.bottom_pane.set_task_running(false);
                if self.status_message != "Session compaction failed" {
                    self.add_to_history(history_cell::new_live_aligned_error_event_with_hint(
                        message,
                        Some("session compaction failed".to_string()),
                    ));
                }
                self.set_status_message("Session compaction failed");
            }
            WorkerEvent::SessionTitleUpdated {
                session_id: _,
                title,
            } => {
                self.set_status_message(format!("Session: {title}"));
            }
            WorkerEvent::InputHistoryLoaded { direction: _, text } => {
                self.bottom_pane.restore_input_from_history(text);
            }
            WorkerEvent::QueueUpdated {
                change, entries, ..
            } => {
                self.apply_native_queue_snapshot(change, entries);
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::SteerAccepted { .. } => {
                self.set_status_message("Steer accepted");
            }
            WorkerEvent::Transcript(_) => {}
        }
    }

    fn apply_native_queue_snapshot(
        &mut self,
        change: devo_protocol::native::queue::QueueChange,
        entries: Vec<devo_protocol::native::queue::QueueEntry>,
    ) {
        use crate::bottom_pane::PendingQueueItem;
        use crate::queue_ops::queue_entry_text;
        use std::collections::HashMap;

        let old_items = self.bottom_pane.pending_queue_items().to_vec();
        let new_items: Vec<PendingQueueItem> = entries
            .iter()
            .map(|entry| PendingQueueItem {
                queue_item_id: entry.queue_item_id.to_string(),
                text: queue_entry_text(entry),
            })
            .collect();
        let new_ids: std::collections::HashSet<&str> = new_items
            .iter()
            .map(|item| item.queue_item_id.as_str())
            .collect();

        // The item being edited was deleted or drained meanwhile; fall back to
        // pushing a fresh entry on the next busy submit.
        if self
            .editing_queue_item_id
            .as_deref()
            .is_some_and(|id| !new_ids.contains(id))
        {
            self.editing_queue_item_id = None;
        }

        let mut mode_by_id: HashMap<String, InputMode> = HashMap::new();
        for (item, mode) in old_items.iter().zip(self.queued_input_modes.iter()) {
            mode_by_id.insert(item.queue_item_id.clone(), *mode);
        }
        // Modes queued ahead of known ids (busy push before snapshot returned).
        let mut pending_new_modes: std::collections::VecDeque<InputMode> = self
            .queued_input_modes
            .iter()
            .skip(old_items.len())
            .copied()
            .collect();

        for old in &old_items {
            if new_ids.contains(old.queue_item_id.as_str()) {
                continue;
            }
            let mode = mode_by_id
                .remove(&old.queue_item_id)
                .unwrap_or(InputMode::Build);
            match change {
                devo_protocol::native::queue::QueueChange::Drained => {
                    if self.queued_count > new_items.len() {
                        self.commit_active_streams(DotStatus::Completed);
                    }
                    self.promoted_input_modes.push_back(mode);
                    self.add_to_history(history_cell::new_user_prompt(
                        old.text.clone(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        self.active_accent_color(),
                        mode,
                    ));
                }
                devo_protocol::native::queue::QueueChange::Removed
                | devo_protocol::native::queue::QueueChange::Promoted => {}
                _ => {}
            }
        }

        let mut next_modes = std::collections::VecDeque::new();
        for item in &new_items {
            let mode = mode_by_id
                .remove(&item.queue_item_id)
                .or_else(|| pending_new_modes.pop_front())
                .unwrap_or(InputMode::Build);
            next_modes.push_back(mode);
        }
        self.queued_input_modes = next_modes;
        self.bottom_pane.replace_pending_queue(new_items);
        self.queued_count = self.bottom_pane.pending_queue_items().len();
    }
}

fn history_awaits_proposed_plan_decision(items: &[SessionHistoryItem]) -> bool {
    for item in items.iter().rev() {
        if matches!(
            item.kind,
            SessionHistoryItemKind::TurnSummary | SessionHistoryItemKind::Error
        ) {
            continue;
        }
        return matches!(item.metadata, Some(SessionHistoryMetadata::ProposedPlan));
    }
    false
}
