use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
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
use devo_server::ClientTransportKind;
use devo_server::ErrorResponse;
use devo_server::ProtocolErrorCode;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::SkillRecord;
use devo_server::SkillScope;
use devo_server::SkillSource;
use devo_server::SuccessResponse;

#[derive(Default)]
struct CapturingProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for CapturingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated skill title".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.stream_requests
            .lock()
            .expect("stream request lock")
            .push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Skill acknowledged.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-1".into(),
                    content: vec![ResponseContent::Text("Skill acknowledged.".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-test-provider"
    }
}

fn create_skill(root: &Path, name: &str, content: &str) -> PathBuf {
    let skill_dir = root.join(name);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_path, content).expect("write skill");
    skill_path
}

fn native_skill_path(path: &Path) -> PathBuf {
    devo_core::normalize_native_path(std::fs::canonicalize(path).expect("canonicalize skill path"))
}

fn native_skill_base_dir(path: &Path) -> PathBuf {
    native_skill_path(path)
        .parent()
        .expect("canonical skill base directory")
        .to_path_buf()
}

fn skill_description(path: &Path) -> String {
    format!("Skill discovered at {}", path.display())
}

fn build_runtime(
    data_root: &Path,
    user_skill_root: PathBuf,
    workspace_root: Option<PathBuf>,
    provider: Arc<dyn ModelProviderSDK>,
) -> Arc<ServerRuntime> {
    build_runtime_with_registry(
        data_root,
        user_skill_root,
        workspace_root,
        provider,
        Arc::new(ToolRegistry::new()),
    )
}

fn build_runtime_with_registry(
    data_root: &Path,
    user_skill_root: PathBuf,
    workspace_root: Option<PathBuf>,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
) -> Arc<ServerRuntime> {
    let workspace_skill_roots = workspace_root
        .iter()
        .map(|root| root.join(".devo").join("skills"))
        .collect::<Vec<_>>();
    write_test_config(data_root, &user_skill_root, &workspace_skill_roots);
    let db_path = data_root.join("test_skills.db");
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
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                enabled: true,
                user_roots: vec![user_skill_root],
                workspace_roots: workspace_skill_roots,
                watch_for_changes: false,
                bundled: Some(BundledSkillsConfig { enabled: false }),
                include_instructions: Some(true),
                config: Vec::new(),
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(data_root.to_path_buf(), workspace_root.as_deref())
                    .expect("load app config store"),
            )),
        ),
    )
}

fn write_test_config(data_root: &Path, user_skill_root: &Path, workspace_skill_roots: &[PathBuf]) {
    let workspace_roots = workspace_skill_roots
        .iter()
        .map(|root| format!("\"{}\"", toml_path(root)))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::create_dir_all(data_root).expect("create test config dir");
    std::fs::write(
        data_root.join("config.toml"),
        format!(
            r#"[skills]
enabled = true
user_roots = ["{}"]
workspace_roots = [{}]
watch_for_changes = false
include_instructions = true

[skills.bundled]
enabled = false
"#,
            toml_path(user_skill_root),
            workspace_roots
        ),
    )
    .expect("write test app config");
}

fn toml_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
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
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
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

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<devo_core::SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "skills-integration-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let result: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("session/new response: {response}"))?;
    let session_id = devo_core::SessionId::try_from(result.result.session.id.as_str())?;
    let title_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "title": "Skills integration"
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let _: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(title_response.clone())
            .with_context(|| format!("session/metadata/update response: {title_response}"))?;
    Ok(session_id)
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    let mut seen = Vec::new();
    loop {
        match timeout(Duration::from_secs(5), notifications_rx.recv()).await {
            Ok(Some(value)) => {
                if is_original_method(&value, "turn/completed") {
                    return Ok(());
                }
                seen.push(value);
            }
            Ok(None) => {
                anyhow::bail!(
                    "notification channel closed before turn/completed; seen: {}",
                    serde_json::to_string(&seen)?
                );
            }
            Err(error) => {
                anyhow::bail!(
                    "timed out waiting for turn/completed: {error}; seen: {}",
                    serde_json::to_string(&seen)?
                );
            }
        }
    }
}

fn is_original_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method").and_then(serde_json::Value::as_str) == Some(method)
        || value
            .get("params")
            .and_then(|params| params.get("_meta").or_else(|| params.get("meta")))
            .and_then(|meta| meta.get("devo/originalMethod"))
            .and_then(serde_json::Value::as_str)
            == Some(method)
}

async fn wait_for_approval_request(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    let mut seen = Vec::new();
    loop {
        match timeout(Duration::from_secs(5), notifications_rx.recv()).await {
            Ok(Some(value)) => {
                if is_approval_request_notification(&value) {
                    return Ok(());
                }
                seen.push(value);
            }
            Ok(None) => {
                anyhow::bail!(
                    "notification channel closed before approval request; seen: {}",
                    serde_json::to_string(&seen)?
                );
            }
            Err(error) => {
                anyhow::bail!(
                    "timed out waiting for approval request: {error}; seen: {}",
                    serde_json::to_string(&seen)?
                );
            }
        }
    }
}

fn is_approval_request_notification(value: &serde_json::Value) -> bool {
    if matches!(
        value.get("method").and_then(serde_json::Value::as_str),
        Some(
            "session/request_permission"
                | "approval/command/request"
                | "approval/fileChange/request"
                | "approval/permission/request"
        )
    ) {
        return true;
    }
    let direct = value.get("method") == Some(&serde_json::json!("item/started"))
        && value
            .get("params")
            .and_then(|params| params.get("item"))
            .and_then(|item| item.get("item_kind"))
            == Some(&serde_json::json!("approval_request"));
    let original = original_event(value)
        .filter(|event| event.get("kind") == Some(&serde_json::json!("item_started")))
        .and_then(|event| event.get("item"))
        .and_then(|item| item.get("item_kind"))
        == Some(&serde_json::json!("approval_request"));
    direct || original
}

fn original_event(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .get("params")
        .and_then(|params| params.get("_meta").or_else(|| params.get("meta")))
        .and_then(|meta| meta.get("devo/originalEvent"))
}

fn user_request_text(request: &ModelRequest) -> Result<String> {
    let text = all_user_request_texts(request).join("\n");
    (!text.is_empty())
        .then_some(text)
        .context("expected a user text request payload")
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

fn auto_review_registry(calls: Arc<std::sync::atomic::AtomicUsize>) -> Arc<ToolRegistry> {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(RecordingMutatingTool { calls }));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: "Mutates test state.".into(),
        input_schema: JsonSchema::object(std::collections::BTreeMap::new(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![devo_core::tools::ToolCapabilityTag::WriteFiles],
        supports_parallel: false,
        preparation_feedback: devo_core::tools::ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    Arc::new(builder.build())
}

async fn update_permissions_to_auto_review(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "permissionProfile": "autoReview" }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let result: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(response)?;
    assert_eq!(
        result.result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::AutoReview
    );
    Ok(())
}

async fn start_auto_review_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [
                        { "type": "text", "text": "Use the mutating tool." }
                    ],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start auto-review response")?;
    let result: SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("turn/start response: {response}"))?;
    assert_eq!(
        result.result.turn.status,
        devo_protocol::native::turn::TurnStatus::InProgress
    );
    Ok(())
}

struct BlockingReadOnlyTool {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ToolHandler for BlockingReadOnlyTool {
    fn spec(&self) -> &ToolSpec {
        Box::leak(Box::new(ToolSpec {
            name: "blocking_read".into(),
            description: "blocking read test tool".into(),
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

struct RecordingMutatingTool {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ToolHandler for RecordingMutatingTool {
    fn spec(&self) -> &ToolSpec {
        Box::leak(Box::new(ToolSpec {
            name: "recording_write".into(),
            description: "recording write test tool".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
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
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolResult::success(
            ToolResultContent::Text("mutated".into()),
            "mutated",
        ))
    }
}

struct AutoReviewProvider {
    risk: &'static str,
    tool_input: serde_json::Value,
    reviewer_calls: Arc<AtomicUsize>,
    completion_failures_remaining: Arc<AtomicUsize>,
    stream_calls: Arc<AtomicUsize>,
    stream_requests: Mutex<Vec<ModelRequest>>,
    completion_requests: Mutex<Vec<ModelRequest>>,
}

impl AutoReviewProvider {
    fn new(
        risk: &'static str,
        tool_input: serde_json::Value,
        reviewer_calls: Arc<AtomicUsize>,
        completion_failures_remaining: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            risk,
            tool_input,
            reviewer_calls,
            completion_failures_remaining: Arc::new(AtomicUsize::new(
                completion_failures_remaining,
            )),
            stream_calls: Arc::new(AtomicUsize::new(0)),
            stream_requests: Mutex::new(Vec::new()),
            completion_requests: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl ModelProviderSDK for AutoReviewProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.completion_requests
            .lock()
            .expect("completion request lock")
            .push(request);
        self.reviewer_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut remaining = self
            .completion_failures_remaining
            .load(std::sync::atomic::Ordering::SeqCst);
        while remaining > 0 {
            match self.completion_failures_remaining.compare_exchange(
                remaining,
                remaining - 1,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            ) {
                Ok(_) => return Err(anyhow::anyhow!("test reviewer provider failure")),
                Err(current) => remaining = current,
            }
        }
        Ok(ModelResponse {
            id: "review-1".into(),
            content: vec![ResponseContent::Text(format!(
                r#"{{"risk":"{}","rationale":"test decision"}}"#,
                self.risk
            ))],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.stream_requests
            .lock()
            .expect("stream request lock")
            .push(request);
        let stream_call = self
            .stream_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let events = if stream_call == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: self.tool_input.clone(),
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
                            name: "mutating_tool".into(),
                            input: self.tool_input.clone(),
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
                        id: "resp-2".into(),
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
        "auto-review-test-provider"
    }
}

#[derive(Default)]
struct SteerCapturingProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for SteerCapturingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated skill title".into())],
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
                    text: "Steer applied.".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("Steer applied.".into())],
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
        "steer-capturing-provider"
    }
}

#[tokio::test]
async fn skills_list_returns_user_and_workspace_skills() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let workspace_skill_root = workspace_root.join(".devo").join("skills");

    let rust_skill_path =
        create_skill(&user_skill_root, "rust-docs", "# Rust Docs\n\nUse rustdoc.");
    let team_skill_path = create_skill(
        &workspace_skill_root,
        "team-style",
        "# Team Style\n\nFollow the formatter.",
    );

    let runtime = build_runtime(
        temp_dir.path(),
        user_skill_root.clone(),
        Some(workspace_root.clone()),
        Arc::new(CapturingProvider::default()),
    );
    let (connection_id, _) = initialize_connection(&runtime).await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "skill/list",
                "params": {
                    "cwd": workspace_root,
                    "forceReload": false,
                }
            }),
        )
        .await
        .context("skill/list response")?;
    let result: devo_protocol::native::rpc_admin::SkillListResult =
        serde_json::from_value(response["result"].clone())
            .with_context(|| format!("skill/list response: {response}"))?;
    let result_skills = result
        .skills
        .into_iter()
        .map(SkillRecord::from)
        .collect::<Vec<_>>();
    let native_rust_skill_path = native_skill_path(&rust_skill_path);
    let native_team_skill_path = native_skill_path(&team_skill_path);

    assert_eq!(
        result_skills,
        vec![
            SkillRecord {
                id: "team-style".into(),
                name: "team-style".into(),
                description: skill_description(&native_team_skill_path),
                short_description: None,
                interface: None,
                dependencies: None,
                path: native_team_skill_path,
                enabled: true,
                source: SkillSource::Workspace {
                    cwd: workspace_root,
                },
                scope: SkillScope::Repo,
                plugin_id: None,
            },
            SkillRecord {
                id: "rust-docs".into(),
                name: "rust-docs".into(),
                description: skill_description(&native_rust_skill_path),
                short_description: None,
                interface: None,
                dependencies: None,
                path: native_rust_skill_path,
                enabled: true,
                source: SkillSource::User,
                scope: SkillScope::User,
                plugin_id: None,
            },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn skill_list_force_reload_rediscovers_new_workspace_skill() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let workspace_skill_root = workspace_root.join(".devo").join("skills");

    let alpha_skill_path = create_skill(&workspace_skill_root, "alpha", "# Alpha\n\nFirst skill.");

    let runtime = build_runtime(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        Arc::new(CapturingProvider::default()),
    );
    let (connection_id, _) = initialize_connection(&runtime).await?;

    let first_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "skill/list",
                "params": {
                    "cwd": workspace_root.clone(),
                    "forceReload": true,
                }
            }),
        )
        .await
        .context("first skill/list response")?;
    let first_result: devo_protocol::native::rpc_admin::SkillListResult =
        serde_json::from_value(first_response["result"].clone())?;
    let first_skills = first_result
        .skills
        .into_iter()
        .map(SkillRecord::from)
        .collect::<Vec<_>>();
    let native_alpha_skill_path = native_skill_path(&alpha_skill_path);
    assert_eq!(
        first_skills,
        vec![SkillRecord {
            id: "alpha".into(),
            name: "alpha".into(),
            description: skill_description(&native_alpha_skill_path),
            short_description: None,
            interface: None,
            dependencies: None,
            path: native_alpha_skill_path.clone(),
            enabled: true,
            source: SkillSource::Workspace {
                cwd: workspace_root.clone(),
            },
            scope: SkillScope::Repo,
            plugin_id: None,
        }]
    );

    let bravo_skill_path = create_skill(&workspace_skill_root, "bravo", "# Bravo\n\nSecond skill.");
    let second_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "skill/list",
                "params": {
                    "cwd": workspace_root,
                    "forceReload": true,
                }
            }),
        )
        .await
        .context("second skill/list response")?;
    let second_result: devo_protocol::native::rpc_admin::SkillListResult =
        serde_json::from_value(second_response["result"].clone())?;
    let second_skills = second_result
        .skills
        .into_iter()
        .map(SkillRecord::from)
        .collect::<Vec<_>>();
    let native_bravo_skill_path = native_skill_path(&bravo_skill_path);
    assert_eq!(
        second_skills,
        vec![
            SkillRecord {
                id: "alpha".into(),
                name: "alpha".into(),
                description: skill_description(&native_alpha_skill_path),
                short_description: None,
                interface: None,
                dependencies: None,
                path: native_alpha_skill_path,
                enabled: true,
                source: SkillSource::Workspace {
                    cwd: workspace_root.clone(),
                },
                scope: SkillScope::Repo,
                plugin_id: None,
            },
            SkillRecord {
                id: "bravo".into(),
                name: "bravo".into(),
                description: skill_description(&native_bravo_skill_path),
                short_description: None,
                interface: None,
                dependencies: None,
                path: native_bravo_skill_path,
                enabled: true,
                source: SkillSource::Workspace {
                    cwd: workspace_root,
                },
                scope: SkillScope::Repo,
                plugin_id: None,
            },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn turn_start_resolves_skill_content_into_model_request() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let skill_path = create_skill(
        &user_skill_root,
        "rust-docs",
        "# Rust Docs\n\nPrefer `cargo test` before `cargo fmt`.",
    );
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        provider.clone(),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [
                        { "type": "text", "text": "Follow this skill." },
                        { "type": "skill", "name": "rust-docs" }
                    ],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
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
    let start_result: SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("turn/start response: {response}"))?;
    assert_eq!(
        start_result.result.turn.status,
        devo_protocol::native::turn::TurnStatus::InProgress
    );

    wait_for_turn_completed(&mut notifications_rx).await?;

    let captured_request = provider
        .stream_requests
        .lock()
        .expect("captured requests lock")
        .first()
        .cloned()
        .context("expected one streamed model request")?;
    let request_text = user_request_text(&captured_request)?;
    let skill_base_dir = native_skill_base_dir(&skill_path);

    assert!(request_text.contains("Follow this skill."));
    assert!(request_text.contains("<skill id=\"rust-docs\" name=\"rust-docs\">"));
    assert!(request_text.contains("Prefer `cargo test` before `cargo fmt`."));
    assert!(request_text.contains(&format!("Base directory: {}", skill_base_dir.display())));
    Ok(())
}

#[tokio::test]
async fn turn_start_rejects_missing_skill_references() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let runtime = build_runtime(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        Arc::new(CapturingProvider::default()),
    );
    let (connection_id, _) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [
                        { "type": "skill", "name": "missing-skill" }
                    ],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start missing skill response")?;
    let error: ErrorResponse = serde_json::from_value(response)?;

    assert_eq!(error.error.code, ProtocolErrorCode::InvalidParams);
    assert!(error.error.message.contains("skill not found"));
    Ok(())
}

#[tokio::test]
async fn auto_review_approval_executes_mutating_tool_without_user_prompt() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(AtomicUsize::new(0));
    let provider = AutoReviewProvider::new("low", json!({}), Arc::clone(&reviewer_calls), 0);
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        Arc::clone(&provider) as Arc<dyn ModelProviderSDK>,
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let stream_requests = provider
        .stream_requests
        .lock()
        .expect("stream request lock");
    let completion_requests = provider
        .completion_requests
        .lock()
        .expect("completion request lock");
    assert_eq!(completion_requests.len(), 1);
    assert!(!stream_requests.is_empty());
    let turn = &stream_requests[0];
    let review = &completion_requests[0];
    assert_eq!(turn.system, review.system);
    assert_eq!(turn.model, review.model);
    assert_eq!(
        turn.tools.as_ref().map(|tools| tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()),
        review.tools.as_ref().map(|tools| tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>())
    );
    assert_eq!(review.messages.len(), turn.messages.len() + 1);
    for (expected, actual) in turn
        .messages
        .iter()
        .zip(&review.messages[..turn.messages.len()])
    {
        assert_eq!(expected.role, actual.role);
        assert_eq!(expected.content.len(), actual.content.len());
    }
    Ok(())
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: explicit full sandbox escalation is routed through AutoReview.
#[tokio::test]
async fn auto_review_approval_executes_full_sandbox_escalation() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new(
            "medium",
            json!({
                "sandbox_permissions": "require_escalated"
            }),
            Arc::clone(&reviewer_calls),
            0,
        ),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: Tier1 additional permissions are routed through AutoReview.
#[tokio::test]
async fn auto_review_approval_executes_additional_permissions_request() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new(
            "low",
            json!({
                "sandbox_permissions": "with_additional_permissions",
                "additional_permissions": {
                    "file_system": {
                        "read": [workspace_root.join("external-input").display().to_string()]
                    }
                }
            }),
            Arc::clone(&reviewer_calls),
            0,
        ),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn auto_review_high_risk_falls_back_to_user_approval() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new("high", json!({}), Arc::clone(&reviewer_calls), 0),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_approval_request(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn auto_review_medium_risk_executes_mutating_tool_without_user_prompt() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new("medium", json!({}), Arc::clone(&reviewer_calls), 0),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: provider failures retry once, then fall back to user approval.
#[tokio::test]
async fn auto_review_provider_failure_retries_once_then_falls_back() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new("low", json!({}), Arc::clone(&reviewer_calls), 2),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_approval_request(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: invalid reviewer output falls back to user approval.
#[tokio::test]
async fn auto_review_invalid_output_falls_back_to_user_approval() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let tool_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reviewer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        AutoReviewProvider::new("invalid", json!({}), Arc::clone(&reviewer_calls), 0),
        auto_review_registry(Arc::clone(&tool_calls)),
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;
    update_permissions_to_auto_review(&runtime, connection_id, session_id).await?;

    start_auto_review_turn(&runtime, connection_id, session_id).await?;
    wait_for_approval_request(&mut notifications_rx).await?;

    assert_eq!(reviewer_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn turn_steer_injects_resolved_skill_into_next_model_request() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let user_skill_root = temp_dir.path().join("user-skills");
    let workspace_root = temp_dir.path().join("workspace");
    let skill_path = create_skill(
        &user_skill_root,
        "steer-rust",
        "---\nname: steer-rust\ndescription: Rust steering\n---\nPrefer exhaustive matches and cargo tests.",
    );
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "blocking_wait",
        Arc::new(BlockingReadOnlyTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }),
    );
    builder.push_spec(ToolSpec {
        name: "blocking_wait".into(),
        description: "Blocks until the integration test releases it.".into(),
        input_schema: JsonSchema::object(std::collections::BTreeMap::new(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: devo_core::tools::ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let provider = Arc::new(SteerCapturingProvider::default());
    let runtime = build_runtime_with_registry(
        temp_dir.path(),
        user_skill_root,
        Some(workspace_root.clone()),
        provider.clone(),
        registry,
    );
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, &workspace_root).await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 8,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [
                        { "type": "text", "text": "Start with the tool." }
                    ],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response for steering test")?;
    let start_result: SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("turn/start response: {response}"))?;
    let start_turn_id = start_result.result.turn.id;

    timeout(Duration::from_secs(5), started.notified())
        .await
        .context("timed out waiting for blocking tool to start")?;

    // Native flow: push the input onto the busy session queue, then
    // promote the queued entry into the running turn as a steer.
    let push_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 9,
                "method": "session/queue/push",
                "params": {
                    "sessionId": session_id,
                    "input": [
                        { "type": "text", "text": "Apply this steer now." },
                        { "type": "skill", "name": "steer-rust" }
                    ],
                    "idempotencyKey": "steer-skill-push"
                }
            }),
        )
        .await
        .context("session/queue/push response")?;
    let push_result: SuccessResponse<devo_protocol::native::rpc_turn::SessionQueuePushResult> =
        serde_json::from_value(push_response.clone())
            .with_context(|| format!("push_response: {push_response}"))?;
    let devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { entry } =
        push_result.result
    else {
        panic!("busy push must queue");
    };

    let steer_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 10,
                "method": "session/queue/steer",
                "params": {
                    "sessionId": session_id,
                    "queueItemId": entry.queue_item_id.as_str(),
                    "expectedTurnId": start_turn_id.to_string()
                }
            }),
        )
        .await
        .context("session/queue/steer response")?;
    let steer_result: SuccessResponse<devo_protocol::native::rpc_turn::SessionQueueSteerResult> =
        serde_json::from_value(steer_response)?;
    assert!(!steer_result.result.item_id.as_str().is_empty());

    release.notify_one();
    wait_for_turn_completed(&mut notifications_rx).await?;

    let captured_requests = provider
        .stream_requests
        .lock()
        .expect("captured requests lock");
    assert_eq!(captured_requests.len(), 2);

    let first_user_texts = all_user_request_texts(&captured_requests[0]);
    let second_user_texts = all_user_request_texts(&captured_requests[1]);
    let skill_base_dir = native_skill_base_dir(&skill_path);

    assert!(
        first_user_texts
            .iter()
            .all(|text| !text.contains("Apply this steer now.")),
        "steer text should not appear before the follow-up request"
    );
    assert!(
        second_user_texts
            .iter()
            .any(|text| text.contains("Apply this steer now.")),
        "expected steer text in the follow-up request"
    );
    assert!(
        second_user_texts
            .iter()
            .any(|text| text.contains("<skill id=\"steer-rust\" name=\"steer-rust\">")),
        "expected resolved skill wrapper in the follow-up request"
    );
    assert!(
        second_user_texts
            .iter()
            .any(|text| text.contains("Prefer exhaustive matches and cargo tests.")),
        "expected skill body in the follow-up request"
    );
    assert!(
        second_user_texts
            .iter()
            .any(|text| text.contains(&format!("Base directory: {}", skill_base_dir.display()))),
        "expected skill base directory in the follow-up request"
    );
    Ok(())
}
