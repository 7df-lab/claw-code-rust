use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::create_default_tool_registry;
use devo_protocol::AgentListResult;
use devo_protocol::AgentMessageResult;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SpawnAgentResult;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnStartResult;
use devo_protocol::Usage;
use devo_protocol::WaitAgentResult;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub enum StreamScript {
    Events(Vec<StreamEvent>),
    Delayed(Duration, Vec<StreamEvent>),
    Pending,
}

pub struct ScriptedProvider {
    scripts: Mutex<VecDeque<StreamScript>>,
    requests: Mutex<Vec<ModelRequest>>,
    stream_calls: AtomicUsize,
}

impl ScriptedProvider {
    pub fn new(scripts: impl IntoIterator<Item = StreamScript>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            stream_calls: AtomicUsize::new(0),
        }
    }

    pub fn pending() -> Self {
        Self::new([StreamScript::Pending])
    }

    pub fn completed(text: &str) -> StreamScript {
        StreamScript::Events(text_response_events(text))
    }

    pub fn completed_after(delay: Duration, text: &str) -> StreamScript {
        StreamScript::Delayed(delay, text_response_events(text))
    }

    pub fn completed_with_deltas(deltas: &[&str]) -> StreamScript {
        let full_text = deltas.join("");
        let mut events = deltas
            .iter()
            .map(|text| StreamEvent::TextDelta {
                index: 0,
                text: (*text).to_string(),
            })
            .collect::<Vec<_>>();
        events.push(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "text-response".into(),
                content: vec![ResponseContent::Text(full_text)],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        });
        StreamScript::Events(events)
    }

    pub fn spawn_agent_tool_call(message: &str, fork_turns: &str) -> StreamScript {
        let input = serde_json::json!({
            "message": message,
            "fork_turns": fork_turns,
        });
        let tool_call_id = "spawn-agent-call".to_string();
        StreamScript::Events(vec![
            StreamEvent::ToolCallStart {
                index: 0,
                id: tool_call_id.clone(),
                name: "spawn_agent".to_string(),
                input: input.clone(),
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "spawn-agent-response".to_string(),
                    content: vec![ResponseContent::ToolUse {
                        id: tool_call_id,
                        name: "spawn_agent".to_string(),
                        input,
                    }],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ])
    }

    pub fn dual_spawn_agent_tool_calls(
        first_message: &str,
        second_message: &str,
        fork_turns: &str,
    ) -> StreamScript {
        let first_input = serde_json::json!({
            "message": first_message,
            "fork_turns": fork_turns,
        });
        let second_input = serde_json::json!({
            "message": second_message,
            "fork_turns": fork_turns,
        });
        let first_id = "spawn-agent-call-1".to_string();
        let second_id = "spawn-agent-call-2".to_string();
        StreamScript::Events(vec![
            StreamEvent::ToolCallStart {
                index: 0,
                id: first_id.clone(),
                name: "spawn_agent".to_string(),
                input: first_input.clone(),
            },
            StreamEvent::ToolCallStart {
                index: 1,
                id: second_id.clone(),
                name: "spawn_agent".to_string(),
                input: second_input.clone(),
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "dual-spawn-agent-response".to_string(),
                    content: vec![
                        ResponseContent::ToolUse {
                            id: first_id,
                            name: "spawn_agent".to_string(),
                            input: first_input,
                        },
                        ResponseContent::ToolUse {
                            id: second_id,
                            name: "spawn_agent".to_string(),
                            input: second_input,
                        },
                    ],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ])
    }

    pub fn wait_agent_tool_call(timeout_secs: u64) -> StreamScript {
        let input = serde_json::json!({ "timeout_secs": timeout_secs });
        let tool_call_id = "wait-agent-call".to_string();
        StreamScript::Events(vec![
            StreamEvent::ToolCallStart {
                index: 0,
                id: tool_call_id.clone(),
                name: "wait_agent".to_string(),
                input: input.clone(),
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "wait-agent-response".to_string(),
                    content: vec![ResponseContent::ToolUse {
                        id: tool_call_id,
                        name: "wait_agent".to_string(),
                        input,
                    }],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ])
    }

    pub fn stream_calls(&self) -> usize {
        self.stream_calls.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

#[async_trait]
impl ModelProviderSDK for ScriptedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title".into(),
            content: vec![ResponseContent::Text("title".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let script = self
            .scripts
            .lock()
            .expect("lock stream scripts")
            .pop_front()
            .context("scripted provider stream script exhausted")?;
        match script {
            StreamScript::Events(events) => {
                let events = events.into_iter().map(Ok).collect::<Vec<Result<_>>>();
                Ok(Box::pin(futures::stream::iter(events)))
            }
            StreamScript::Delayed(delay, events) => {
                let state = (Some(delay), VecDeque::from(events));
                let stream = futures::stream::unfold(state, |(delay, mut events)| async move {
                    if let Some(delay) = delay {
                        tokio::time::sleep(delay).await;
                    }
                    let event = events.pop_front()?;
                    Some((Ok(event), (None, events)))
                });
                Ok(Box::pin(stream))
            }
            StreamScript::Pending => {
                Ok(Box::pin(futures::stream::pending::<Result<StreamEvent>>()))
            }
        }
    }

    fn name(&self) -> &str {
        "scripted-provider"
    }
}

pub fn build_runtime(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db_path = data_root.join("subagent_lifecycle.db");
    let db = Arc::new(devo_server::db::Database::open(db_path).expect("open test database"));
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(create_default_tool_registry()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(data_root.to_path_buf(), None).expect("load app config store"),
            )),
        ),
    ))
}

pub async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "test",
                        "title": "test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

pub async fn start_parent_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<devo_protocol::SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/start",
                "params": {
                    "cwd": cwd,
                    "ephemeral": false,
                    "title": "parent",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start")?;
    Ok(
        serde_json::from_value::<devo_server::SuccessResponse<devo_server::SessionStartResult>>(
            response,
        )?
        .result
        .session
        .session_id,
    )
}

pub async fn spawn_child(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    parent_session_id: devo_protocol::SessionId,
) -> Result<SpawnAgentResult> {
    spawn_child_with(
        runtime,
        connection_id,
        parent_session_id,
        "review the current changes",
        Some("none"),
    )
    .await
}

pub async fn spawn_child_with(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    parent_session_id: devo_protocol::SessionId,
    message: &str,
    fork_turns: Option<&str>,
) -> Result<SpawnAgentResult> {
    let mut params = serde_json::json!({
        "session_id": parent_session_id,
        "message": message,
    });
    if let Some(fork_turns) = fork_turns {
        params["fork_turns"] = serde_json::json!(fork_turns);
    }
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "_devo/agent/spawn",
                "params": params
            }),
        )
        .await
        .context("agent/spawn")?;
    Ok(serde_json::from_value::<devo_server::SuccessResponse<SpawnAgentResult>>(response)?.result)
}

pub async fn wait_for_child_turn_started(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    child_session_id: devo_protocol::SessionId,
) -> Result<()> {
    wait_for_session_notification(notifications_rx, "turn/started", child_session_id)
        .await
        .map(|_| ())
}

pub async fn wait_for_session_notification(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
    session_id: devo_protocol::SessionId,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if notification_matches_session(&value, method, session_id) {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before {method} for {session_id}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method} for {session_id}"))?
}

fn notification_matches_session(
    value: &serde_json::Value,
    method: &str,
    session_id: devo_protocol::SessionId,
) -> bool {
    let legacy_match = value.get("method") == Some(&serde_json::json!(method))
        && value["params"]["session_id"] == serde_json::json!(session_id);
    let acp_original_match = value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["sessionId"] == serde_json::json!(session_id)
        && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method);
    legacy_match || acp_original_match
}

pub async fn request_agent_list(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    parent_session_id: devo_protocol::SessionId,
) -> Result<AgentListResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "_devo/agent/list",
                "params": {
                    "session_id": parent_session_id
                }
            }),
        )
        .await
        .context("agent/list")?;
    Ok(serde_json::from_value::<devo_server::SuccessResponse<AgentListResult>>(response)?.result)
}

pub async fn request_agent_wait(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    timeout_secs: u64,
) -> Result<WaitAgentResult> {
    request_agent_wait_with(
        runtime,
        connection_id,
        session_id,
        None::<String>,
        None,
        timeout_secs,
    )
    .await
}

pub async fn request_agent_wait_with<T: serde::Serialize>(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    target: Option<T>,
    after_sequence: Option<u64>,
    timeout_secs: u64,
) -> Result<WaitAgentResult> {
    let target = target
        .map(serde_json::to_value)
        .transpose()
        .context("serialize wait target")?;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "_devo/agent/wait",
                "params": {
                    "session_id": session_id,
                    "target": target,
                    "after_sequence": after_sequence,
                    "timeout_secs": timeout_secs
                }
            }),
        )
        .await
        .context("agent/wait")?;
    Ok(serde_json::from_value::<devo_server::SuccessResponse<WaitAgentResult>>(response)?.result)
}

pub async fn request_agent_send_message<T: serde::Serialize>(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    target: T,
    message: &str,
) -> Result<AgentMessageResult> {
    let target = serde_json::to_value(target).context("serialize agent target")?;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "_devo/agent/send_message",
                "params": {
                    "session_id": session_id,
                    "target": target,
                    "message": message
                }
            }),
        )
        .await
        .context("agent/send_message")?;
    Ok(
        serde_json::from_value::<devo_server::SuccessResponse<AgentMessageResult>>(response)?
            .result,
    )
}

pub async fn request_agent_close<T: serde::Serialize>(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    parent_session_id: devo_protocol::SessionId,
    target: T,
) -> Result<devo_protocol::CloseAgentResult> {
    let target = serde_json::to_value(target).context("serialize agent target")?;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "_devo/agent/close",
                "params": {
                    "session_id": parent_session_id,
                    "target": target
                }
            }),
        )
        .await
        .context("agent/close")?;
    Ok(
        serde_json::from_value::<devo_server::SuccessResponse<devo_protocol::CloseAgentResult>>(
            response,
        )?
        .result,
    )
}

pub async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    text: &str,
) -> Result<TurnStartResult> {
    start_turn_with_approval_policy(runtime, connection_id, session_id, text, None).await
}

pub async fn start_turn_with_approval_policy(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    text: &str,
    approval_policy: Option<&str>,
) -> Result<TurnStartResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 9,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": text }],
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": approval_policy,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start")?;
    Ok(serde_json::from_value::<devo_server::SuccessResponse<TurnStartResult>>(response)?.result)
}

pub async fn wait_for_parent_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    parent_session_id: devo_protocol::SessionId,
) -> Result<()> {
    wait_for_session_notification(notifications_rx, "turn/completed", parent_session_id)
        .await
        .map(|_| ())
}

pub fn message_texts(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| {
            message.content.iter().filter_map(|content| match content {
                RequestContent::Text { text } => Some(text.clone()),
                RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::ToolUse { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolResult { .. } => None,
            })
        })
        .collect()
}

pub async fn wait_for_stream_calls(provider: &ScriptedProvider, expected: usize) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        loop {
            if provider.stream_calls() >= expected {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("timed out waiting for {expected} provider stream calls"))?
}

fn text_response_events(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::TextDelta {
            index: 0,
            text: text.to_string(),
        },
        StreamEvent::MessageDone {
            response: ModelResponse {
                id: format!("resp-{text}"),
                content: vec![ResponseContent::Text(text.to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        },
    ]
}
