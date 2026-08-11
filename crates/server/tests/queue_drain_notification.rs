//! Reproduction audit for the reported UX bug: a message queued while a turn
//! is running must NOT leak into the in-flight turn's model requests, and
//! once the running turn ends and the entry drains into the follow-up turn,
//! a subscribed connection must receive `queue/updated` (`drained`) carrying
//! an empty queue — otherwise clients keep rendering the stale entry.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::ProviderVendorCatalog;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolResult;
use devo_core::tools::ToolResultContent;
use devo_core::tools::json_schema::JsonSchema;
use devo_core::tools::registry::ToolRegistryBuilder;
use devo_core::tools::tool_handler::ToolHandler;
use devo_core::tools::tool_spec::ToolExecutionMode;
use devo_core::tools::tool_spec::ToolOutputMode;
use devo_core::tools::tool_spec::ToolSpec;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::SuccessResponse;

const QUEUED_TEXT: &str = "queued follow-up message";

/// First stream request triggers the blocking tool; every later request ends
/// the turn with plain text. All requests are captured for content asserts.
#[derive(Default)]
struct BlockingThenDoneProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for BlockingThenDoneProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated title".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = {
            let mut requests = self.stream_requests.lock().expect("stream request lock");
            requests.push(request);
            requests.len()
        };
        let events = if request_number == 1 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "blocking_wait".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: "{}".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".into(),
                            name: "blocking_wait".into(),
                            input: json!({}),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Done.".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-n".into(),
                        content: vec![ResponseContent::Text("Done.".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "blocking-then-done-provider"
    }
}

struct BlockingTool {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ToolHandler for BlockingTool {
    fn spec(&self) -> &ToolSpec {
        Box::leak(Box::new(ToolSpec {
            name: "blocking_wait".into(),
            description: "Blocks until the test releases it.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: devo_core::tools::ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        }))
    }

    async fn handle(
        &self,
        _ctx: devo_core::tools::ToolContext,
        _input: serde_json::Value,
        _progress: Option<devo_core::tools::ToolProgressSender>,
    ) -> std::result::Result<ToolResult, ToolCallError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(ToolResult::success(
            ToolResultContent::Text("released".into()),
            "released",
        ))
    }
}

fn build_runtime(
    data_root: &Path,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
) -> Arc<ServerRuntime> {
    let db_path = data_root.join("test_queue_drain.db");
    let db = Arc::new(devo_server::db::Database::open(db_path).expect("open test database"));
    ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            registry,
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                enabled: false,
                user_roots: Vec::new(),
                workspace_roots: Vec::new(),
                watch_for_changes: false,
                bundled: Some(BundledSkillsConfig { enabled: false }),
                include_instructions: Some(false),
                config: Vec::new(),
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(data_root.to_path_buf(), None).expect("load app config store"),
            )),
        ),
    )
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "test", "title": "test", "version": "1.0.0" }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

fn all_user_request_texts(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .flat_map(|message| {
            message.content.iter().filter_map(|content| match content {
                RequestContent::Reasoning { text } => Some(text.clone()),
                RequestContent::Text { text } => Some(text.clone()),
                RequestContent::ProviderReasoning { .. }
                | RequestContent::ToolUse { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolResult { .. } => None,
            })
        })
        .collect()
}

fn queue_updated_change(value: &serde_json::Value) -> Option<&str> {
    if value.get("method").and_then(serde_json::Value::as_str) != Some("queue/updated") {
        return None;
    }
    value
        .get("params")
        .and_then(|params| params.get("change"))
        .and_then(serde_json::Value::as_str)
}

async fn recv_until(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    label: &str,
    predicate: impl Fn(&serde_json::Value) -> bool,
    collected: &mut Vec<serde_json::Value>,
) -> Result<serde_json::Value> {
    loop {
        match timeout(Duration::from_secs(10), notifications_rx.recv()).await {
            Ok(Some(value)) => {
                if predicate(&value) {
                    return Ok(value);
                }
                collected.push(value);
            }
            Ok(None) => anyhow::bail!("notification channel closed waiting for {label}"),
            Err(_) => {
                let seen: Vec<String> = collected
                    .iter()
                    .filter_map(|value| {
                        value
                            .get("method")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .collect();
                anyhow::bail!("timed out waiting for {label}; seen methods: {seen:?}");
            }
        }
    }
}

fn is_turn_completed(value: &serde_json::Value) -> bool {
    value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed")
        || value
            .get("params")
            .and_then(|params| params.get("_meta").or_else(|| params.get("meta")))
            .and_then(|meta| meta.get("devo/originalMethod"))
            .and_then(serde_json::Value::as_str)
            == Some("turn/completed")
}

#[tokio::test]
async fn queued_input_drains_into_followup_turn_and_broadcasts_empty_queue() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "blocking_wait",
        Arc::new(BlockingTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }),
    );
    builder.push_spec(ToolSpec {
        name: "blocking_wait".into(),
        description: "Blocks until the test releases it.".into(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: devo_core::tools::ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let provider = Arc::new(BlockingThenDoneProvider::default());
    let runtime = build_runtime(temp_dir.path(), provider.clone(), Arc::new(builder.build()));
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "additionalDirectories": [],
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session_result: AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(session_response.clone())
            .with_context(|| format!("session/new response: {session_response}"))?;
    let session_id = session_result.result.session_id;

    // Subscribe exactly like the TUI does so `event_selectors` is populated
    // and `queue/updated` broadcasts target this connection.
    let subscription_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3,
                "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription/create response")?;
    assert!(
        subscription_response.get("error").is_none(),
        "subscription/create failed: {subscription_response}"
    );

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "Start with the tool." }],
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        turn_response.get("error").is_none(),
        "turn/start failed: {turn_response}"
    );
    timeout(Duration::from_secs(5), started.notified())
        .await
        .context("timed out waiting for blocking tool to start")?;

    // Turn is running: push the message onto the queue (TUI Enter behavior).
    let push_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "session/queue/push",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": QUEUED_TEXT }],
                    "idempotencyKey": "queue-drain-audit"
                }
            }),
        )
        .await
        .context("session/queue/push response")?;
    let push_result: SuccessResponse<devo_protocol::canonical::rpc_turn::SessionQueuePushResult> =
        serde_json::from_value(push_response.clone())
            .with_context(|| format!("push_response: {push_response}"))?;
    let devo_protocol::canonical::rpc_turn::SessionQueuePushResult::Queued { entry } =
        push_result.result
    else {
        panic!("busy push must queue");
    };
    let queue_item_id = entry.queue_item_id.as_str().to_string();

    // The push broadcast must reach the subscribed connection.
    let mut collected = Vec::new();
    let added = recv_until(
        &mut notifications_rx,
        "queue/updated(added)",
        |value| queue_updated_change(value) == Some("added"),
        &mut collected,
    )
    .await?;
    assert_eq!(
        added["params"]["queue"].as_array().map(Vec::len),
        Some(1),
        "added notification should carry the queued entry: {added}"
    );

    release.notify_one();

    // First turn completes, the entry drains into the follow-up turn.
    recv_until(
        &mut notifications_rx,
        "first turn/completed",
        is_turn_completed,
        &mut collected,
    )
    .await?;
    let drained = recv_until(
        &mut notifications_rx,
        "queue/updated(drained)",
        |value| queue_updated_change(value) == Some("drained"),
        &mut collected,
    )
    .await?;
    assert_eq!(
        drained["params"]["queueItemId"].as_str(),
        Some(queue_item_id.as_str()),
        "drained notification should name the drained entry: {drained}"
    );
    assert_eq!(
        drained["params"]["queue"].as_array().map(Vec::len),
        Some(0),
        "drained notification must carry the empty queue, otherwise clients \
         keep rendering the stale entry: {drained}"
    );
    assert!(
        drained["params"]["startedTurnId"].as_str().is_some(),
        "drained notification should carry the follow-up turn id: {drained}"
    );
    recv_until(
        &mut notifications_rx,
        "second turn/completed",
        is_turn_completed,
        &mut collected,
    )
    .await?;

    // The queue is really empty server-side.
    let list_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 6,
                "method": "session/queue/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/queue/list response")?;
    assert_eq!(
        list_response["result"]["entries"].as_array().map(Vec::len),
        Some(0),
        "queue/list should be empty after the drain: {list_response}"
    );

    // Queued input must NOT steer the in-flight turn: the first turn's
    // post-tool request (request 2) never sees the queued text; only the
    // drained follow-up turn (request 3) does.
    let requests = provider
        .stream_requests
        .lock()
        .expect("captured requests lock");
    assert_eq!(requests.len(), 3, "expected T1 x2 + T2 x1 model requests");
    let in_flight_texts = all_user_request_texts(&requests[1]);
    assert!(
        in_flight_texts
            .iter()
            .all(|text| !text.contains(QUEUED_TEXT)),
        "queued input must not leak into the in-flight turn: {in_flight_texts:?}"
    );
    let followup_texts = all_user_request_texts(&requests[2]);
    assert!(
        followup_texts.iter().any(|text| text.contains(QUEUED_TEXT)),
        "drained input should appear in the follow-up turn: {followup_texts:?}"
    );
    Ok(())
}
