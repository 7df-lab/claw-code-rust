//! Slash-command dispatch for `ChatWidget`.
//!
//! The bottom pane parses slash commands, and this module translates them into
//! chat-widget state changes or app commands sent back to the host loop.

use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_command::GoalObjectiveMode;
use crate::app_event::AppEvent;
use crate::get_git_diff::get_git_diff;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use crate::slash_command::SlashCommand;
use devo_protocol::MAX_THREAD_GOAL_OBJECTIVE_CHARS;
use devo_protocol::PermissionPreset;
use devo_protocol::ThreadGoalStatus;

use super::ChatWidget;

impl ChatWidget {
    pub(super) fn can_change_configuration(&self) -> bool {
        !self.busy
    }

    pub(super) fn add_busy_configuration_message(&mut self, command: SlashCommand) {
        let noun = match command {
            SlashCommand::Model => "model",
            SlashCommand::Theme => "theme",
            SlashCommand::Compact => "session",
            SlashCommand::New => "session",
            SlashCommand::Delete => "session",
            SlashCommand::Resume => "session",
            SlashCommand::Permissions => "permissions",
            SlashCommand::Diff => "diff",
            SlashCommand::Mcp
            | SlashCommand::Skills
            | SlashCommand::Goal
            | SlashCommand::Exit
            | SlashCommand::Status
            | SlashCommand::Clear
            | SlashCommand::ShowReasoning
            | SlashCommand::Rename
            | SlashCommand::Btw => {
                return;
            }
        };
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
            "Cannot change {noun} while generating"
        ))]));
        self.set_status_message(format!("Cannot change {noun} while generating"));
    }

    pub(crate) fn handle_slash_command(&mut self, command: SlashCommand, argument: String) {
        if !self.can_change_configuration() && !command.available_during_task() {
            self.add_busy_configuration_message(command);
            return;
        }

        match command {
            SlashCommand::Exit => {
                tracing::info!("slash /exit dispatched from chat widget");
                self.app_event_tx
                    .send(AppEvent::Exit(crate::app_event::ExitMode::ShutdownFirst));
            }
            SlashCommand::Clear => {
                self.clear_transcript_view();
                self.set_status_message("Transcript cleared");
            }
            SlashCommand::Status => {
                let model = self
                    .session
                    .model
                    .as_ref()
                    .map(|m| m.slug.as_str())
                    .unwrap_or("unknown");
                let reasoning_effort_selection = self
                    .reasoning_effort_selection
                    .as_deref()
                    .unwrap_or("default");
                let cwd = self.session.cwd.display().to_string();
                let turns = self.turn_count;
                let tokens_in = Self::format_token_count(self.total_input_tokens);
                let tokens_out = Self::format_token_count(self.total_output_tokens);
                let lines = history_cell::with_border(vec![
                    Line::from("Session Status".bold()),
                    Line::from(""),
                    Line::from(format!("  model:       {model}")),
                    Line::from(format!(
                        "  reasoning_effort_selection:    {reasoning_effort_selection}"
                    )),
                    Line::from(format!("  cwd:         {cwd}")),
                    Line::from(format!(
                        "  permissions: {}",
                        match self.permission_preset {
                            PermissionPreset::Default => "default",
                            PermissionPreset::AutoReview => "auto-review",
                            PermissionPreset::FullAccess => "full-access",
                        }
                    )),
                    Line::from(format!(
                        "  sandbox:     {}",
                        self.sandbox_profile.as_deref().unwrap_or("workspace")
                    )),
                    Line::from(format!(
                        "  reasoning:   {}",
                        super::reasoning_view::reasoning_view_label(self.collapse_reasoning)
                    )),
                    Line::from(format!("  turns:       {turns}")),
                    Line::from(format!(
                        "  tokens:      \u{2191}{tokens_in} \u{2193}{tokens_out}",
                    )),
                ]);
                self.add_to_history(PlainHistoryCell::new(lines));
                self.set_status_message("Session status shown");
            }
            SlashCommand::Permissions => {
                self.open_permissions_picker();
            }
            SlashCommand::ShowReasoning => {
                self.open_reasoning_view_picker();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Model => {
                if argument.is_empty() {
                    self.open_model_picker();
                } else {
                    self.apply_model_selection(argument);
                }
            }
            SlashCommand::Mcp => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "mcp list".to_string(),
                    }));
                self.set_status_message("Loading MCP servers");
            }
            SlashCommand::Skills => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "skills list".to_string(),
                    }));
                self.set_status_message("Loading skills");
            }
            SlashCommand::Compact => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::compact()));
            }
            SlashCommand::New => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "session new".to_string(),
                    }));
                self.set_status_message("New session requested");
            }
            SlashCommand::Rename => {
                let trimmed = argument.trim();
                if trimmed.is_empty() {
                    self.add_to_history(history_cell::new_info_event(
                        "Usage: /rename <new title>".to_string(),
                        None,
                    ));
                    self.set_status_message("Usage: /rename <new title>");
                    return;
                }
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::rename_session(
                        trimmed.to_string(),
                    )));
                self.set_status_message("Renaming session");
            }
            SlashCommand::Delete => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::delete_session()));
                self.set_status_message("Deleting session");
            }
            SlashCommand::Resume => {
                self.resume_browser = None;
                self.resume_browser_loading = true;
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "session list".to_string(),
                    }));
                self.set_status_message("Loading sessions");
            }
            SlashCommand::Btw => {
                let trimmed = argument.trim();
                if trimmed.is_empty() {
                    self.add_to_history(history_cell::new_info_event(
                        "Usage: /btw <your question>".to_string(),
                        None,
                    ));
                    self.set_status_message("Usage: /btw <your question>");
                    return;
                }
                self.add_to_history(history_cell::new_user_prompt(
                    format!("/btw {trimmed}"),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    self.active_accent_color(),
                    self.current_turn_mode,
                ));
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunBtwQuestion {
                        question: trimmed.to_string(),
                    }));
                self.set_status_message("Asking side question");
            }
            SlashCommand::Goal => {
                self.handle_goal_slash_command(argument);
            }
            SlashCommand::Diff => {
                self.set_status_message("Computing diff");
                let tx = self.app_event_tx.clone();
                tokio::spawn(async move {
                    let text = Self::format_git_diff_result(get_git_diff().await);
                    tx.send(AppEvent::DiffResult(text));
                });
            }
        }
    }

    fn handle_goal_slash_command(&mut self, argument: String) {
        let trimmed = argument.trim();
        if trimmed.is_empty() {
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::show_goal()));
            self.set_status_message("Loading goal");
            return;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            "clear" => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::clear_goal()));
                self.set_status_message("Clearing goal");
                return;
            }
            "edit" => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::edit_goal()));
                self.set_status_message("Loading goal editor");
                return;
            }
            "pause" => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::set_goal_status(
                        ThreadGoalStatus::Paused,
                    )));
                self.set_status_message("Pausing goal");
                return;
            }
            "resume" => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::set_goal_status(
                        ThreadGoalStatus::Active,
                    )));
                self.set_status_message("Resuming goal");
                return;
            }
            _ => {}
        }

        if trimmed.chars().count() > MAX_THREAD_GOAL_OBJECTIVE_CHARS {
            self.add_to_history(history_cell::new_error_event(format!(
                "Goal objective is too long: limit is {MAX_THREAD_GOAL_OBJECTIVE_CHARS} characters"
            )));
            self.set_status_message("Goal objective too long");
            return;
        }

        self.add_to_history(history_cell::new_user_prompt(
            format!("/goal {trimmed}"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            self.active_accent_color(),
            self.current_turn_mode,
        ));
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::set_goal_objective(
                trimmed.to_string(),
                GoalObjectiveMode::ConfirmIfExists,
            )));
        self.set_status_message("Setting goal");
    }
}
