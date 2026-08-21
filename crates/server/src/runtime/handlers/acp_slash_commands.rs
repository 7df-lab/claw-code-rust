use super::super::*;

use std::sync::Arc;

use super::acp::legacy_error_to_acp;

use devo_protocol::GoalClearParams;
use devo_protocol::GoalClearResult;
use devo_protocol::GoalSetParams;
use devo_protocol::GoalSetResult;
use devo_protocol::GoalStatusParams;
use devo_protocol::GoalStatusResult;
use devo_protocol::SessionId;
use devo_protocol::SlashCommand;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::acp_available_slash_commands;

use crate::ACP_SESSION_UPDATE_METHOD;
use crate::AcpClientNotification;
use crate::AcpContentBlock;
use crate::AcpErrorCode;
use crate::AcpPromptResult;
use crate::AcpSessionNotification;
use crate::AcpSessionUpdate;
use crate::AcpStopReason;
use crate::CollaborationMode;
use crate::InputItem;
use crate::SuccessResponse;
use crate::TurnExecutionMode;
use crate::TurnStartParams;
use crate::TurnStartResult;
use crate::acp_error_response;
use crate::acp_success_response;

pub(super) enum AcpSlashCommandPromptResult {
    NotCommand,
    Response(serde_json::Value),
    Pending,
}

impl ServerRuntime {
    pub(crate) async fn send_acp_session_state_snapshot(
        &self,
        connection_id: u64,
        session_id: SessionId,
    ) {
        self.send_acp_session_update(
            connection_id,
            session_id,
            AcpSessionUpdate::AvailableCommandsUpdate {
                available_commands: acp_available_slash_commands(),
                meta: None,
            },
        )
        .await;
    }

    pub(super) async fn handle_acp_slash_command_prompt(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        prompt: &[AcpContentBlock],
    ) -> AcpSlashCommandPromptResult {
        let Some((command_name, argument)) = acp_slash_command_text(prompt) else {
            return AcpSlashCommandPromptResult::NotCommand;
        };
        // Desktop requirement: /plan is a composer trigger chip, not a user-defined
        // command. Keep it on the ACP prompt path so the client waits for the plan turn.
        if command_name == "plan" {
            return self
                .handle_acp_plan_slash_command(
                    connection_id,
                    request_id,
                    session_id,
                    argument,
                    prompt,
                )
                .await;
        }
        let Ok(command) = command_name.parse::<SlashCommand>() else {
            return AcpSlashCommandPromptResult::NotCommand;
        };

        match command {
            SlashCommand::Compact => {
                self.handle_acp_compact_slash_command(
                    connection_id,
                    request_id,
                    session_id,
                    argument,
                )
                .await
            }
            SlashCommand::Goal => {
                self.handle_acp_goal_slash_command(connection_id, request_id, session_id, argument)
                    .await
            }
            SlashCommand::Theme
            | SlashCommand::Model
            | SlashCommand::Skills
            | SlashCommand::Mcp
            | SlashCommand::Resume
            | SlashCommand::New
            | SlashCommand::Rename
            | SlashCommand::Delete
            | SlashCommand::Status
            | SlashCommand::Settings
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Diff
            | SlashCommand::Exit
            | SlashCommand::Btw => {
                let command_name = command.command();
                AcpSlashCommandPromptResult::Response(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("/{command_name} is a TUI command and is not available over ACP"),
                ))
            }
        }
    }

    async fn handle_acp_compact_slash_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        argument: &str,
    ) -> AcpSlashCommandPromptResult {
        if !argument.trim().is_empty() {
            return AcpSlashCommandPromptResult::Response(acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "/compact does not accept arguments",
            ));
        }

        let response = self
            .handle_native_session_compact_start(
                request_id.clone(),
                serde_json::json!({ "sessionId": session_id }),
            )
            .await;
        let Ok(_response) = serde_json::from_value::<
            SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult>,
        >(response.clone()) else {
            return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                request_id, response,
            ));
        };
        self.send_acp_agent_message(connection_id, session_id, "Session compaction started.")
            .await;
        AcpSlashCommandPromptResult::Response(acp_prompt_success_response(request_id))
    }

    async fn handle_acp_goal_slash_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        argument: &str,
    ) -> AcpSlashCommandPromptResult {
        let trimmed = argument.trim();
        if trimmed.is_empty() {
            return self
                .handle_acp_goal_status_command(connection_id, request_id, session_id)
                .await;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            "clear" => {
                self.handle_acp_goal_clear_command(connection_id, request_id, session_id)
                    .await
            }
            "pause" => {
                self.handle_acp_goal_status_update_command(
                    connection_id,
                    request_id,
                    session_id,
                    ThreadGoalStatus::Paused,
                )
                .await
            }
            "resume" => {
                self.handle_acp_goal_status_update_command(
                    connection_id,
                    request_id,
                    session_id,
                    ThreadGoalStatus::Active,
                )
                .await
            }
            "edit" => AcpSlashCommandPromptResult::Response(acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "/goal edit is only available in the TUI",
            )),
            _ => {
                self.handle_acp_goal_set_command(
                    connection_id,
                    request_id,
                    session_id,
                    trimmed.to_string(),
                )
                .await
            }
        }
    }

    async fn handle_acp_goal_status_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> AcpSlashCommandPromptResult {
        let legacy_response = self
            .handle_goal_status(
                request_id.clone(),
                serde_json::to_value(GoalStatusParams { session_id })
                    .expect("serialize goal status params"),
            )
            .await;
        let Ok(response) =
            serde_json::from_value::<SuccessResponse<GoalStatusResult>>(legacy_response.clone())
        else {
            return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                request_id,
                legacy_response,
            ));
        };
        let message = response
            .result
            .goal
            .as_ref()
            .map(goal_summary_message)
            .unwrap_or_else(|| "No goal is currently set.".to_string());
        self.send_acp_agent_message(connection_id, session_id, message)
            .await;
        AcpSlashCommandPromptResult::Response(acp_prompt_success_response(request_id))
    }

    async fn handle_acp_goal_set_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        objective: String,
    ) -> AcpSlashCommandPromptResult {
        let legacy_response = self
            .handle_goal_set(
                request_id.clone(),
                serde_json::to_value(GoalSetParams {
                    session_id,
                    objective: Some(objective),
                    status: Some(ThreadGoalStatus::Active),
                    token_budget: None,
                })
                .expect("serialize goal set params"),
            )
            .await;
        let Ok(response) =
            serde_json::from_value::<SuccessResponse<GoalSetResult>>(legacy_response.clone())
        else {
            return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                request_id,
                legacy_response,
            ));
        };
        let objective = &response.result.goal.objective;
        self.send_acp_agent_message(connection_id, session_id, format!("Goal set: {objective}"))
            .await;
        AcpSlashCommandPromptResult::Response(acp_prompt_success_response(request_id))
    }

    async fn handle_acp_goal_status_update_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        status: ThreadGoalStatus,
    ) -> AcpSlashCommandPromptResult {
        let legacy_response = self
            .handle_goal_set(
                request_id.clone(),
                serde_json::to_value(GoalSetParams {
                    session_id,
                    objective: None,
                    status: Some(status),
                    token_budget: None,
                })
                .expect("serialize goal status update params"),
            )
            .await;
        let Ok(response) =
            serde_json::from_value::<SuccessResponse<GoalSetResult>>(legacy_response.clone())
        else {
            return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                request_id,
                legacy_response,
            ));
        };
        let status = goal_status_label(response.result.goal.status);
        let objective = &response.result.goal.objective;
        self.send_acp_agent_message(
            connection_id,
            session_id,
            format!("Goal {status}: {objective}"),
        )
        .await;
        AcpSlashCommandPromptResult::Response(acp_prompt_success_response(request_id))
    }

    async fn handle_acp_goal_clear_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> AcpSlashCommandPromptResult {
        let legacy_response = self
            .handle_goal_clear(
                request_id.clone(),
                serde_json::to_value(GoalClearParams { session_id })
                    .expect("serialize goal clear params"),
            )
            .await;
        let Ok(response) =
            serde_json::from_value::<SuccessResponse<GoalClearResult>>(legacy_response.clone())
        else {
            return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                request_id,
                legacy_response,
            ));
        };
        let message = if response.result.cleared {
            "Goal cleared."
        } else {
            "No goal is currently set."
        };
        self.send_acp_agent_message(connection_id, session_id, message)
            .await;
        AcpSlashCommandPromptResult::Response(acp_prompt_success_response(request_id))
    }

    async fn handle_acp_plan_slash_command(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: SessionId,
        argument: &str,
        prompt: &[AcpContentBlock],
    ) -> AcpSlashCommandPromptResult {
        let input = match input_items_from_argument_slash_prompt(
            argument,
            prompt,
            "Usage: /plan <task to plan>",
        ) {
            Ok(input) => input,
            Err(error) => {
                return AcpSlashCommandPromptResult::Response(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    error,
                ));
            }
        };
        let legacy_response = self
            .handle_turn_start_with_queue_policy(
                Some(connection_id),
                request_id.clone(),
                TurnStartParams {
                    session_id,
                    input,
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: CollaborationMode::Plan,
                    execution_mode: TurnExecutionMode::Regular,
                },
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let legacy: SuccessResponse<TurnStartResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => {
                    return AcpSlashCommandPromptResult::Response(legacy_error_to_acp(
                        request_id,
                        legacy_response,
                    ));
                }
            };
        let Some(turn_id) = legacy.result.turn_id() else {
            return AcpSlashCommandPromptResult::Response(acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session/prompt cannot queue behind an active turn",
            ));
        };
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let stop_reason = runtime
                .wait_for_acp_prompt_stop_reason(session_id, turn_id)
                .await;
            runtime
                .send_raw_to_connection(
                    connection_id,
                    acp_success_response(
                        request_id,
                        AcpPromptResult {
                            stop_reason,
                            meta: None,
                        },
                    ),
                )
                .await;
        });
        AcpSlashCommandPromptResult::Pending
    }

    async fn send_acp_agent_message(
        &self,
        connection_id: u64,
        session_id: SessionId,
        message: impl Into<String>,
    ) {
        self.send_acp_session_update(
            connection_id,
            session_id,
            AcpSessionUpdate::AgentMessageChunk {
                content: AcpContentBlock::text(message),
                message_id: None,
                meta: None,
            },
        )
        .await;
    }

    async fn send_acp_session_update(
        &self,
        connection_id: u64,
        session_id: SessionId,
        update: AcpSessionUpdate,
    ) {
        let notification = AcpClientNotification::new(
            ACP_SESSION_UPDATE_METHOD,
            AcpSessionNotification {
                session_id,
                update,
                meta: None,
            },
        );
        self.send_raw_to_connection(
            connection_id,
            serde_json::to_value(notification).expect("serialize ACP session update"),
        )
        .await;
    }
}

fn acp_slash_command_text(prompt: &[AcpContentBlock]) -> Option<(&str, &str)> {
    let Some(AcpContentBlock::Text { text, .. }) = prompt.first() else {
        return None;
    };
    let rest = text.strip_prefix('/')?;
    let name_len = rest
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    if name_len == 0 {
        return None;
    }
    let command = &rest[..name_len];
    let argument = rest[name_len..].trim_start();
    Some((command, argument))
}

fn input_items_from_argument_slash_prompt(
    argument: &str,
    prompt: &[AcpContentBlock],
    usage: &str,
) -> Result<Vec<InputItem>, String> {
    let mut input = Vec::new();
    let trimmed = argument.trim();
    if !trimmed.is_empty() {
        input.push(InputItem::Text {
            text: trimmed.to_string(),
        });
    }
    for block in prompt.iter().skip(1).cloned() {
        input.extend(block.into_input_items()?);
    }
    if input.is_empty() {
        return Err(usage.to_string());
    }
    Ok(input)
}

fn acp_prompt_success_response(request_id: serde_json::Value) -> serde_json::Value {
    acp_success_response(
        request_id,
        AcpPromptResult {
            stop_reason: AcpStopReason::EndTurn,
            meta: None,
        },
    )
}

fn goal_summary_message(goal: &ThreadGoal) -> String {
    let status = goal_status_label(goal.status);
    let objective = &goal.objective;
    format!("Goal {status}: {objective}")
}

fn goal_status_label(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::BudgetLimited => "budget-limited",
        ThreadGoalStatus::Complete => "complete",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_slash_command_text_extracts_command_and_argument() {
        let prompt = vec![AcpContentBlock::text("/goal improve tests")];

        assert_eq!(
            acp_slash_command_text(&prompt),
            Some(("goal", "improve tests"))
        );
    }

    #[test]
    fn acp_slash_command_text_ignores_unknown_prefix_shape() {
        assert_eq!(
            acp_slash_command_text(&[AcpContentBlock::text(" /goal no leading slash")]),
            None
        );
        assert_eq!(acp_slash_command_text(&[AcpContentBlock::text("/")]), None);
        assert_eq!(acp_slash_command_text(&[]), None);
    }

    #[test]
    fn plan_prompt_uses_command_argument_and_preserves_extra_content() {
        let input = input_items_from_argument_slash_prompt(
            "desktop slash triggers",
            &[
                AcpContentBlock::text("/plan desktop slash triggers"),
                AcpContentBlock::text("include footer chip behavior"),
            ],
            "Usage: /plan <task to plan>",
        )
        .expect("plan input");

        assert_eq!(
            input,
            vec![
                InputItem::Text {
                    text: "desktop slash triggers".to_string()
                },
                InputItem::Text {
                    text: "include footer chip behavior".to_string()
                },
            ]
        );
    }
}
