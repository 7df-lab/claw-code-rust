use std::sync::Arc;

use devo_core::tools::{
    ToolAgentScope, ToolCall, ToolCallResult, ToolContent, ToolExecutionOptions, ToolRuntime,
    ToolRuntimeContext,
};
use devo_core::{CommandExecutionItem, SessionId, TurnItem};
use devo_protocol::TurnFailedPayload;
use devo_util_shell_command::parse_command::parse_command;
use tokio_util::sync::CancellationToken;

use super::super::*;
use super::tool_display::{user_shell_command_payload, user_shell_exec_input};
use crate::{
    ItemKind, ServerEvent, SessionRuntimeStatus, SessionStatusChangedPayload, TurnEventPayload,
};

impl ServerRuntime {
    pub(in crate::runtime) async fn execute_shell_command_turn(
        self: Arc<Self>,
        session_id: SessionId,
        turn: crate::TurnMetadata,
        command: String,
        cwd: std::path::PathBuf,
    ) {
        self.capture_turn_workspace_baseline(session_id, turn.turn_id, cwd.clone())
            .await;
        if let Some(session_handle) = self.session(session_id).await {
            session_handle.reset_turn_approval_cache().await;
        }

        let tool_call_id = format!("user-shell-{}", turn.turn_id);
        let input = user_shell_exec_input(&command, cwd.clone());
        let command_actions = parse_command(std::slice::from_ref(&command));
        let (item_id, item_seq) = self
            .start_item(
                session_id,
                turn.turn_id,
                ItemKind::CommandExecution,
                serde_json::to_value(user_shell_command_payload(
                    &tool_call_id,
                    &command,
                    input.clone(),
                    command_actions.clone(),
                    None,
                    false,
                ))
                .expect("serialize command execution payload"),
            )
            .await;

        let Some(session_handle) = self.session(session_id).await else {
            self.clear_active_turn_runtime_handles(session_id).await;
            return;
        };
        let Some(shell_context) = session_handle.shell_exec_context(cwd.clone()).await else {
            self.clear_active_turn_runtime_handles(session_id).await;
            return;
        };
        let permission_mode = shell_context.permission_mode;
        let permission_profile = shell_context.permission_profile;
        let registry = shell_context.tool_registry;
        let file_read_ledger = shell_context.file_read_ledger;
        let provider_http = shell_context
            .runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .clone();
        let turn_cancel_token = self
            .active_turns
            .cancel_token(session_id)
            .await
            .unwrap_or_else(CancellationToken::new);
        let tool_execution_start_runtime = Arc::clone(&self);
        let tool_execution_start_session_id = session_id;
        let tool_execution_start_turn_id = turn.turn_id;
        let runtime = ToolRuntime::new_with_context_and_options(
            registry,
            self.build_permission_checker(
                session_id,
                turn.turn_id,
                permission_mode,
                permission_profile,
            ),
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn.turn_id.to_string()),
                cwd,
                agent_scope: ToolAgentScope::Parent,
                collaboration_mode: devo_protocol::CollaborationMode::Build,
                agent_coordinator: None,
                client_filesystem: None,
                file_read_ledger,
                local_web_search: None,
                hooks: self.hook_context_for_session(session_id).await,
                network_proxy: provider_http.proxy_url,
                network_no_proxy: provider_http.no_proxy,
                sandbox_profile: shell_context.sandbox_profile.clone(),
            },
            ToolExecutionOptions {
                cancel_token: turn_cancel_token,
                on_tool_execution_start: Some(Arc::new(move |call: ToolCall| {
                    let runtime = Arc::clone(&tool_execution_start_runtime);
                    let tool_call_id = call.id;
                    Box::pin(async move {
                        runtime
                            .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                                devo_protocol::ToolCallStatusUpdatedPayload {
                                    session_id: tool_execution_start_session_id,
                                    turn_id: tool_execution_start_turn_id,
                                    tool_call_id,
                                    status: "in_progress".to_string(),
                                },
                            ))
                            .await;
                    })
                })),
                ..ToolExecutionOptions::default()
            },
        );
        let result = runtime
            .execute_batch(&[ToolCall {
                id: tool_call_id.clone(),
                name: "exec_command".to_string(),
                input: input.clone(),
            }])
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| ToolCallResult::error(&tool_call_id, "shell command did not run"));
        let output = match result.content.clone() {
            ToolContent::Text(text) => serde_json::Value::String(text),
            ToolContent::Json(json) => json,
            ToolContent::Mixed { text, json } => {
                json.unwrap_or_else(|| serde_json::Value::String(text.unwrap_or_default()))
            }
        };
        let is_error = result.is_error;
        self.clear_active_turn_runtime_handles(session_id).await;
        self.complete_item(
            session_id,
            turn.turn_id,
            item_id,
            item_seq,
            ItemKind::CommandExecution,
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: tool_call_id.clone(),
                tool_name: "exec_command".to_string(),
                command: command.clone(),
                input: input.clone(),
                output: output.clone(),
                is_error,
            }),
            serde_json::to_value(user_shell_command_payload(
                &tool_call_id,
                &command,
                input.clone(),
                command_actions,
                Some(output),
                is_error,
            ))
            .expect("serialize command execution payload"),
        )
        .await;

        let Some(final_turn) = session_handle
            .complete_shell_turn(turn.clone(), is_error)
            .await
        else {
            return;
        };
        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.record.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(session_id, &final_turn)
                .await
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist shell command turn line");
        }
        self.finalize_turn_workspace_changes(session_id, &final_turn)
            .await;
        if is_error {
            self.broadcast_event(ServerEvent::TurnFailed(TurnFailedPayload {
                session_id,
                turn: final_turn.clone(),
                error: None,
            }))
            .await;
        }
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: final_turn,
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
    }
}
