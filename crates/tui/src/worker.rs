use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use clawcr_core::{SessionId, TurnId, TurnStatus};
use clawcr_server::{
    InputItem, ItemEnvelope, ItemEventPayload, ItemKind, ServerEvent, SessionHistoryItem,
    SessionHistoryItemKind, SessionListParams, SessionResumeParams, SessionStartParams,
    SessionTitleUpdateParams, StdioServerClient, StdioServerClientConfig, TurnEventPayload,
    TurnInterruptParams, TurnStartParams,
};

use crate::events::{SessionListEntry, TranscriptItem, TranscriptItemKind, WorkerEvent};

/// Immutable runtime configuration used to construct the background server client worker.
pub(crate) struct QueryWorkerConfig {
    /// Model identifier used for new turns.
    pub(crate) model: String,
    /// Working directory used for the server session.
    pub(crate) cwd: PathBuf,
    /// Environment overrides applied to the spawned server child process.
    pub(crate) server_env: Vec<(String, String)>,
}

/// Commands accepted by the background query worker.
enum WorkerCommand {
    /// Submit a new user prompt to the session.
    SubmitPrompt(String),
    /// Update the model used for future turns.
    SetModel(String),
    /// Request a session list from the server.
    ListSessions,
    /// Clear the active session so the next prompt starts a fresh one lazily.
    StartNewSession,
    /// Switch the active session to a persisted session identifier.
    SwitchSession(SessionId),
    /// Rename the current active session.
    RenameSession(String),
    /// Interrupt the active turn when one is running.
    InterruptTurn,
    /// Stop the worker loop.
    Shutdown,
}

/// Handle used by the UI thread to interact with the background query worker.
pub(crate) struct QueryWorkerHandle {
    /// Sender used to submit commands to the worker.
    command_tx: mpsc::UnboundedSender<WorkerCommand>,
    /// Receiver used by the UI to consume worker events.
    pub(crate) event_rx: mpsc::UnboundedReceiver<WorkerEvent>,
    /// Background task running the worker loop.
    join_handle: JoinHandle<()>,
}

impl QueryWorkerHandle {
    /// Spawns the background worker and returns the UI-facing handle.
    pub(crate) fn spawn(config: QueryWorkerConfig) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let join_handle = tokio::spawn(run_worker(config, command_rx, event_tx));
        Self {
            command_tx,
            event_rx,
            join_handle,
        }
    }

    /// Submits one prompt to the worker.
    pub(crate) fn submit_prompt(&self, prompt: String) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::SubmitPrompt(prompt))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Updates the active session model for future turns.
    pub(crate) fn set_model(&self, model: String) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::SetModel(model))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests the current persisted session list from the background worker.
    pub(crate) fn list_sessions(&self) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::ListSessions)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Clears the active session so the next submitted prompt starts a fresh one lazily.
    pub(crate) fn start_new_session(&self) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::StartNewSession)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Switches the active session to a persisted session identifier.
    pub(crate) fn switch_session(&self, session_id: SessionId) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::SwitchSession(session_id))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Renames the current active session.
    pub(crate) fn rename_session(&self, title: String) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::RenameSession(title))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Interrupts the active turn when one exists.
    pub(crate) fn interrupt_turn(&self) -> Result<()> {
        self.command_tx
            .send(WorkerCommand::InterruptTurn)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Stops the worker task and waits for it to finish.
    pub(crate) async fn shutdown(self) -> Result<()> {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        let _ = self.join_handle.await.map_err(map_join_error);
        Ok(())
    }
}

#[cfg(test)]
impl QueryWorkerHandle {
    /// Creates a lightweight stub worker handle for unit tests that exercise UI logic only.
    pub(crate) fn stub() -> Self {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            command_tx,
            event_rx,
            join_handle: tokio::spawn(async move { while command_rx.recv().await.is_some() {} }),
        }
    }
}

async fn run_worker(
    config: QueryWorkerConfig,
    mut command_rx: mpsc::UnboundedReceiver<WorkerCommand>,
    event_tx: mpsc::UnboundedSender<WorkerEvent>,
) {
    if let Err(error) = run_worker_inner(config, &mut command_rx, &event_tx).await {
        let _ = event_tx.send(WorkerEvent::TurnFailed {
            message: error.to_string(),
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        });
    }
}

async fn run_worker_inner(
    config: QueryWorkerConfig,
    command_rx: &mut mpsc::UnboundedReceiver<WorkerCommand>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    let mut client = StdioServerClient::spawn(StdioServerClientConfig {
        program: std::env::current_exe().context("resolve current executable for server launch")?,
        workspace_root: Some(config.cwd.clone()),
        env: config.server_env,
    })
    .await?;
    let _ = client.initialize().await?;
    let mut session_id: Option<SessionId> = None;
    let mut model = config.model;
    let mut active_turn_id: Option<TurnId> = None;
    let mut turn_count = 0usize;
    let total_input_tokens = 0usize;
    let total_output_tokens = 0usize;

    loop {
        tokio::select! {
            maybe_command = command_rx.recv() => {
                match maybe_command {
                    Some(WorkerCommand::SubmitPrompt(prompt)) => {
                        let active_session_id = ensure_session_started(
                            &mut client,
                            &config.cwd,
                            &model,
                            &mut session_id,
                        ).await?;
                        let start_result = client.turn_start(TurnStartParams {
                            session_id: active_session_id,
                            input: vec![InputItem::Text { text: prompt }],
                            model: Some(model.clone()),
                            sandbox: None,
                            approval_policy: None,
                            cwd: None,
                        }).await;
                        match start_result {
                            Ok(result) => {
                                active_turn_id = Some(result.turn_id);
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                });
                            }
                        }
                    }
                    Some(WorkerCommand::SetModel(next_model)) => {
                        model = next_model;
                    }
                    Some(WorkerCommand::ListSessions) => {
                        match client.session_list(SessionListParams::default()).await {
                            Ok(result) => {
                                let sessions = result
                                    .sessions
                                    .iter()
                                    .map(|session| SessionListEntry {
                                        session_id: session.session_id,
                                        title: session
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| "(untitled)".to_string()),
                                        updated_at: session
                                            .updated_at
                                            .format("%Y-%m-%d %H:%M:%S UTC")
                                            .to_string(),
                                        is_active: Some(session.session_id) == session_id,
                                    })
                                    .collect();
                                let _ = event_tx.send(WorkerEvent::SessionsListed { sessions });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                });
                            }
                        }
                    }
                    Some(WorkerCommand::StartNewSession) => {
                        active_turn_id = None;
                        session_id = None;
                        let _ = event_tx.send(WorkerEvent::NewSessionPrepared);
                    }
                    Some(WorkerCommand::SwitchSession(next_session_id)) => {
                        match client
                            .session_resume(SessionResumeParams {
                                session_id: next_session_id,
                            })
                            .await
                        {
                            Ok(result) => {
                                active_turn_id = None;
                                session_id = Some(next_session_id);
                                if let Some(next_model) = result.session.resolved_model.clone() {
                                    model = next_model;
                                }
                                let _ = event_tx.send(WorkerEvent::SessionSwitched {
                                    session_id: next_session_id.to_string(),
                                    title: result.session.title,
                                    model: result.session.resolved_model,
                                    history_items: project_history_items(&result.history_items),
                                    loaded_item_count: result.loaded_item_count,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                });
                            }
                        }
                    }
                    Some(WorkerCommand::RenameSession(title)) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                            });
                            continue;
                        };
                        match client
                            .session_title_update(SessionTitleUpdateParams {
                                session_id: active_session_id,
                                title: title.clone(),
                            })
                            .await
                        {
                            Ok(result) => {
                                let _ = event_tx.send(WorkerEvent::SessionRenamed {
                                    session_id: active_session_id.to_string(),
                                    title: result
                                        .session
                                        .title
                                        .unwrap_or(title),
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                });
                            }
                        }
                    }
                    Some(WorkerCommand::InterruptTurn) => {
                        if let (Some(turn_id), Some(active_session_id)) = (active_turn_id, session_id) {
                            if let Err(error) = client
                                .turn_interrupt(TurnInterruptParams {
                                    session_id: active_session_id,
                                    turn_id,
                                    reason: Some("user requested interrupt".to_string()),
                                })
                                .await
                            {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                });
                            }
                        }
                    }
                    Some(WorkerCommand::Shutdown) | None => {
                        client.shutdown().await?;
                        break;
                    }
                }
            }
            notification = client.recv_event() => {
                match notification? {
                    Some((method, event)) => {
                        match method.as_str() {
                            "turn/started" => {
                                if let ServerEvent::TurnStarted(payload) = event {
                                    active_turn_id = Some(payload.turn.turn_id);
                                }
                                let _ = event_tx.send(WorkerEvent::TurnStarted);
                            }
                            "item/agentMessage/delta" => {
                                if let ServerEvent::ItemDelta { payload, .. } = event {
                                    let _ = event_tx.send(WorkerEvent::TextDelta(payload.delta));
                                }
                            }
                            "item/completed" => {
                                if let ServerEvent::ItemCompleted(payload) = event {
                                    handle_completed_item(payload, event_tx);
                                }
                            }
                            "turn/completed" => {
                                if let ServerEvent::TurnCompleted(payload) = event {
                                    active_turn_id = None;
                                    let completed = payload.turn.status == TurnStatus::Completed
                                        || payload.turn.status == TurnStatus::Interrupted;
                                    if completed {
                                        turn_count += 1;
                                        let _ = event_tx.send(WorkerEvent::TurnFinished {
                                            stop_reason: format!("{:?}", payload.turn.status),
                                            turn_count,
                                            total_input_tokens,
                                            total_output_tokens,
                                        });
                                    }
                                }
                            }
                            "turn/failed" => {
                                if let ServerEvent::TurnFailed(TurnEventPayload { turn, .. }) = event {
                                    active_turn_id = None;
                                    let _ = event_tx.send(WorkerEvent::TurnFailed {
                                        message: format!("turn failed with status {:?}", turn.status),
                                        turn_count,
                                        total_input_tokens,
                                        total_output_tokens,
                                    });
                                }
                            }
                            "session/title/updated" => {
                                if let ServerEvent::SessionTitleUpdated(payload) = event {
                                    if let Some(title) = payload.session.title {
                                        let _ = event_tx.send(WorkerEvent::SessionTitleUpdated {
                                            session_id: payload.session.session_id.to_string(),
                                            title,
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

async fn ensure_session_started(
    client: &mut StdioServerClient,
    cwd: &PathBuf,
    model: &str,
    session_id: &mut Option<SessionId>,
) -> Result<SessionId> {
    if let Some(session_id) = session_id {
        return Ok(*session_id);
    }

    let session = client
        .session_start(SessionStartParams {
            cwd: cwd.clone(),
            ephemeral: false,
            title: None,
            model: Some(model.to_string()),
        })
        .await?;
    *session_id = Some(session.session_id);
    Ok(session.session_id)
}

fn handle_completed_item(payload: ItemEventPayload, event_tx: &mpsc::UnboundedSender<WorkerEvent>) {
    match payload.item {
        ItemEnvelope {
            item_kind: ItemKind::ToolCall,
            payload,
            ..
        } => {
            let summary = summarize_tool_call(&payload);
            let detail = payload
                .get("input")
                .map(render_json_preview)
                .filter(|detail| !detail.is_empty());
            let _ = event_tx.send(WorkerEvent::ToolCall { summary, detail });
        }
        ItemEnvelope {
            item_kind: ItemKind::ToolResult,
            payload,
            ..
        } => {
            let content = payload
                .get("content")
                .map(render_json_value_text)
                .unwrap_or_default();
            let is_error = payload
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let _ = event_tx.send(WorkerEvent::ToolResult {
                preview: content,
                is_error,
                truncated: false,
            });
        }
        _ => {}
    }
}

fn project_history_items(items: &[SessionHistoryItem]) -> Vec<TranscriptItem> {
    items
        .iter()
        .map(|item| {
            let kind = match item.kind {
                SessionHistoryItemKind::User => TranscriptItemKind::User,
                SessionHistoryItemKind::Assistant => TranscriptItemKind::Assistant,
                SessionHistoryItemKind::ToolCall => TranscriptItemKind::ToolCall,
                SessionHistoryItemKind::ToolResult => TranscriptItemKind::ToolResult,
                SessionHistoryItemKind::Error => TranscriptItemKind::Error,
            };
            TranscriptItem::new(kind, item.title.clone(), item.body.clone())
        })
        .collect()
}

fn summarize_tool_call(payload: &serde_json::Value) -> String {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("tool");
    let input = payload.get("input").unwrap_or(&serde_json::Value::Null);
    match tool_name {
        "bash" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| format!("Ran {command}"))
            .unwrap_or_else(|| "Ran shell command".to_string()),
        other => format!("Ran {other}"),
    }
}

fn render_json_preview(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => truncate_tool_output(text),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            truncate_tool_output(&pretty)
        }
        _ => truncate_tool_output(&value.to_string()),
    }
}

fn render_json_value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn truncate_tool_output(content: &str) -> String {
    const MAX_LINES: usize = 8;
    const MAX_CHARS: usize = 1200;
    let content = normalize_display_output(content);
    let content = content.as_str();

    let mut lines = Vec::new();
    let mut chars = 0usize;
    for line in content.lines() {
        if lines.len() >= MAX_LINES || chars >= MAX_CHARS {
            break;
        }
        let remaining = MAX_CHARS.saturating_sub(chars);
        if line.chars().count() > remaining {
            let preview = line.chars().take(remaining).collect::<String>();
            lines.push(preview);
            break;
        }
        chars += line.chars().count();
        lines.push(line.to_string());
    }

    if lines.is_empty() && !content.is_empty() {
        let preview = content.chars().take(MAX_CHARS).collect::<String>();
        return if preview == content {
            preview
        } else {
            format!("{preview}\n… ")
        };
    }

    let preview = lines.join("\n");
    if preview == content {
        preview
    } else if preview.is_empty() {
        "… ".to_string()
    } else {
        format!("{preview}\n… ")
    }
}

fn normalize_display_output(content: &str) -> String {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_matches('\n')
        .to_string()
}

fn map_join_error(error: JoinError) -> anyhow::Error {
    if error.is_cancelled() {
        anyhow::anyhow!("interactive worker task was cancelled")
    } else if error.is_panic() {
        anyhow::anyhow!("interactive worker task panicked")
    } else {
        anyhow::Error::new(error)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use clawcr_core::{SessionId, SessionTitleState};
    use clawcr_server::{SessionRuntimeStatus, SessionSummary};

    use super::{normalize_display_output, summarize_tool_call, truncate_tool_output};
    use crate::events::SessionListEntry;

    #[test]
    fn bash_tool_summary_uses_command_text() {
        let payload = serde_json::json!({
            "tool_name": "bash",
            "input": {
                "command": "Get-Date -Format \"yyyy-MM-dd\""
            }
        });

        assert_eq!(
            summarize_tool_call(&payload),
            "Ran Get-Date -Format \"yyyy-MM-dd\""
        );
    }

    #[test]
    fn tool_output_preview_truncates_large_content() {
        let content = (1..=12)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            truncate_tool_output(&content),
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n… "
        );
    }

    #[test]
    fn session_list_entries_keep_title_before_identifier() {
        let active_session_id = SessionId::new();
        let summary = SessionSummary {
            session_id: active_session_id,
            cwd: ".".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            title: Some("Saved conversation".to_string()),
            title_state: SessionTitleState::Provisional,
            ephemeral: false,
            resolved_model: Some("test-model".to_string()),
            status: SessionRuntimeStatus::Idle,
        };
        let entry = SessionListEntry {
            session_id: summary.session_id,
            title: summary.title.clone().unwrap_or_default(),
            updated_at: summary
                .updated_at
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
            is_active: true,
        };

        assert_eq!(entry.title, "Saved conversation");
        assert!(entry.updated_at.contains("UTC"));
    }

    #[test]
    fn session_list_entries_mark_inactive_sessions() {
        let summary = SessionSummary {
            session_id: SessionId::new(),
            cwd: ".".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            title: Some("Saved conversation".to_string()),
            title_state: SessionTitleState::Provisional,
            ephemeral: false,
            resolved_model: Some("test-model".to_string()),
            status: SessionRuntimeStatus::Idle,
        };
        let entry = SessionListEntry {
            session_id: summary.session_id,
            title: summary.title.clone().unwrap_or_default(),
            updated_at: summary
                .updated_at
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
            is_active: false,
        };

        assert!(!entry.is_active);
    }

    #[test]
    fn display_output_normalization_trims_crlf_padding() {
        assert_eq!(
            normalize_display_output("\r\n\r\nhello\r\nworld\r\n\r\n"),
            "hello\nworld"
        );
    }
}
