use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task;

#[path = "support/rollout.rs"]
mod support;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::ProviderVendorCatalog;
use futures::stream::Stream;
use futures::stream::{self};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_core::FileSystemSkillCatalog;
use devo_core::ItemLine;
use devo_core::ItemRecord;
use devo_core::PresetModelCatalog;
use devo_core::RolloutLine;
use devo_core::SessionMetaLine;
use devo_core::SessionRecord;
use devo_core::SkillsConfig;
use devo_core::TextItem;
use devo_core::TurnError;
use devo_core::TurnItem;
use devo_core::TurnLine;
use devo_core::TurnRecord;
use devo_core::tools::ToolRegistry;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::ServerEvent;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnStatus;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;

struct SingleReplyProvider;

struct UsageReplyProvider {
    input_tokens: usize,
    output_tokens: usize,
    cache_read_input_tokens: Option<usize>,
}

impl UsageReplyProvider {
    fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_read_input_tokens: Some(input_tokens / 2),
        }
    }

    fn usage(&self) -> Usage {
        Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: self.cache_read_input_tokens,
            reasoning_output_tokens: None,
            total_tokens: Some(self.input_tokens + self.output_tokens),
        }
    }
}

#[derive(Default)]
struct CapturingProvider {
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated rollout title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello from persistence test.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-1".into(),
                    content: vec![ResponseContent::Text("Hello from persistence test.".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "single-reply-test-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for UsageReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-usage".into(),
            content: vec![ResponseContent::Text("Generated usage title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: self.usage(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let usage = self.usage();
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::UsageDelta(usage.clone())),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Usage reply".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-usage".into(),
                    content: vec![ResponseContent::Text("Usage reply".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage,
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "usage-reply-test-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for CapturingProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated rollout title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Captured request reply.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-capture".into(),
                    content: vec![ResponseContent::Text("Captured request reply.".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-provider"
    }
}

/// A stream that yields one TextDelta, then blocks on a oneshot until unblocked or
/// cancelled, then yields MessageDone.  Used by tests that need to interrupt a turn
/// mid-stream to exercise the deferred-item completion race.
struct GatedStream {
    block_rx: oneshot::Receiver<()>,
    state: u8,
}

impl GatedStream {
    fn new(block_rx: oneshot::Receiver<()>) -> Self {
        Self { block_rx, state: 0 }
    }
}

impl Stream for GatedStream {
    type Item = Result<StreamEvent>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Self::Item>> {
        match self.state {
            0 => {
                self.state = 1;
                task::Poll::Ready(Some(Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "mid-interrupt content".into(),
                })))
            }
            1 => match Pin::new(&mut self.block_rx).poll(cx) {
                task::Poll::Ready(Ok(())) => {
                    self.state = 2;
                    task::Poll::Ready(Some(Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-gated".into(),
                            content: vec![ResponseContent::Text("mid-interrupt content".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    })))
                }
                task::Poll::Ready(Err(_)) => task::Poll::Ready(None),
                task::Poll::Pending => task::Poll::Pending,
            },
            2 => {
                self.state = 3;
                task::Poll::Ready(None)
            }
            _ => task::Poll::Ready(None),
        }
    }
}

/// Provider whose stream blocks mid-way, letting the test send an interrupt while
/// the assistant item is still in-progress.
struct GatedProvider {
    /// Kept alive so the oneshot receiver in GatedStream blocks forever
    /// (or until the task is aborted, dropping the receiver).
    _block_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Receiver taken by the first completion_stream call.
    block_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl GatedProvider {
    fn new() -> Self {
        let (tx, rx) = oneshot::channel();
        Self {
            _block_tx: Mutex::new(Some(tx)),
            block_rx: Mutex::new(Some(rx)),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-gated".into(),
            content: vec![ResponseContent::Text("Gated title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let rx = self
            .block_rx
            .lock()
            .expect("lock block_rx")
            .take()
            .expect("completion_stream called more than once");
        Ok(Box::pin(GatedStream::new(rx)))
    }

    fn name(&self) -> &str {
        "gated-provider"
    }
}

#[tokio::test]
async fn runtime_rebuilds_sessions_from_rollout_and_resume_works() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Persistent session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist this session" }],
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id.as_str(), session_id.to_string());
    assert_eq!(sessions[0].title.as_deref(), Some("Persistent session"));

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert_eq!(resume_result.session.session_id, session_id);
    assert_eq!(
        resume_result.session.title.as_deref(),
        Some("Persistent session")
    );
    assert!(resume_result.loaded_item_count >= 2);
    assert!(resume_result.latest_turn.is_some());
    Ok(())
}

#[tokio::test]
async fn resume_restores_plan_collaboration_mode_from_latest_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Plan mode session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "draft a plan" }],
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "plan"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert_eq!(
        resume_result.session.collaboration_mode,
        devo_protocol::CollaborationMode::Plan
    );
    let turn_summary = resume_result
        .history_items
        .iter()
        .rev()
        .find(|item| item.kind == SessionHistoryItemKind::TurnSummary)
        .expect("resumed plan turn should include a turn summary");
    assert_eq!(
        turn_summary.metadata,
        Some(devo_protocol::SessionHistoryMetadata::TurnSummary {
            collaboration_mode: devo_protocol::CollaborationMode::Plan,
        })
    );
    Ok(())
}

#[tokio::test]
async fn resume_restores_session_permission_preset_and_plan_mode_without_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Session overrides",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let start_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result;
    let session_id = start_result.session.session_id;
    let started_model = start_result.session.model.clone();

    let permissions_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "permissionProfile": "fullAccess" }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let permissions_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(permissions_response)?
    .result;
    assert_eq!(
        permissions_result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );

    let metadata_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "mode": "plan" }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let metadata_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(metadata_response)?
    .result;
    assert_eq!(
        metadata_result.session.settings.mode.as_deref(),
        Some("plan")
    );
    assert_eq!(
        metadata_result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );
    assert_eq!(Some(metadata_result.session.model.model), started_model);

    drop(runtime);
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (connection_id, _notifications_rx) = initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response.clone())
    .with_context(|| format!("decode session/resume response: {resume_response}"))?
    .result;

    assert_eq!(
        resume_result.session.collaboration_mode,
        devo_protocol::CollaborationMode::Plan
    );
    assert_eq!(
        resume_result.session.permission_preset,
        Some(devo_protocol::PermissionPreset::FullAccess)
    );
    Ok(())
}

#[tokio::test]
async fn runtime_generates_final_title_and_persists_explicit_rename() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 11,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 12,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "implement rollout persistence for the rust server" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    // Title generation starts at turn start, so the final title may arrive
    // before turn/completed. Wait for the title first so the notification is
    // not drained by wait_for_turn_completed.
    wait_for_title_update(&mut notifications_rx, "Generated rollout title").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let resume_after_completion = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 13,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after completion")?;
    let completed_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_after_completion)?
    .result;
    assert_eq!(
        completed_result.session.title.as_deref(),
        Some("Generated rollout title")
    );
    assert_eq!(
        completed_result.session.title_state,
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::ModelGenerated)
    );

    let rename_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 14,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "title": "Rollout persistence follow-up"
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let rename_result = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(rename_response)?
    .result;
    assert_eq!(
        rename_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_after_rebuild = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 15,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after rebuild")?;
    let rebuilt_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_after_rebuild)?
    .result;
    assert_eq!(
        rebuilt_result.session.title.as_deref(),
        Some("Rollout persistence follow-up")
    );
    // v2 title-update lines deliberately drop the title lifecycle (a derived
    // cache in the canonical model); any Final variant is preserved, which
    // keeps suppressing regeneration of the recorded title.
    assert_eq!(
        rebuilt_result.session.title_state,
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::ExplicitCreate)
    );
    Ok(())
}

#[tokio::test]
async fn runtime_assigns_provisional_title_after_first_prompt() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 22,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "investigate why the current session title stays null" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    // Title work starts at turn start: provisional is assigned first, then the
    // final model title may overwrite it immediately on a fast mock provider.
    let first_title = wait_for_any_title_update(&mut notifications_rx).await?;
    assert!(
        first_title == "Investigate why the current session title stays null"
            || first_title == "Generated rollout title",
        "unexpected first title update: {first_title}"
    );

    let list_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 23,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;
    assert!(sessions[0].title.is_some());
    Ok(())
}

#[tokio::test]
async fn runtime_skips_invalid_rollout_files_when_loading_sessions() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 31,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Valid session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 32,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist the valid session" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let bad_rollout_dir = data_root.path().join("sessions/2026/04/28");
    std::fs::create_dir_all(&bad_rollout_dir)?;
    let bad_rollout_path =
        bad_rollout_dir.join("rollout-2026-04-28T15-12-34Z-legacy-invalid.jsonl");
    std::fs::write(
        &bad_rollout_path,
        "{ definitely not valid json\n{\"still\":\"broken\"}\n",
    )?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 33,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id.as_str(), session_id.to_string());
    assert_eq!(sessions[0].title.as_deref(), Some("Valid session"));
    Ok(())
}

#[tokio::test]
async fn resume_normalizes_historical_default_reasoning_effort() -> Result<()> {
    fn write_historical_rollout(
        data_root: &std::path::Path,
        session_id: &SessionId,
        reasoning_effort_selection: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let rollout_dir = data_root.join("sessions/2026/06/07");
        std::fs::create_dir_all(&rollout_dir)?;
        let rollout_path =
            rollout_dir.join(format!("rollout-2026-06-07T00-00-00-{session_id}.jsonl"));
        let session = SessionRecord {
            id: *session_id,
            rollout_path: rollout_path.clone(),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "openai_chat_completions".into(),
            model: Some("deepseek-v4-flash".into()),
            model_binding_id: None,
            reasoning_effort_selection: reasoning_effort_selection.clone(),
            cwd: data_root.to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "0.1.0".into(),
            title: Some("Historical session".into()),
            title_state: devo_core::SessionTitleState::Final(
                devo_core::SessionTitleFinalSource::ExplicitCreate,
            ),
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        };
        let turn = TurnRecord {
            id: devo_protocol::TurnId::new(),
            session_id: *session_id,
            sequence: 1,
            started_at: now,
            completed_at: Some(now),
            status: TurnStatus::Completed,
            kind: devo_core::TurnKind::Regular,
            model: "deepseek-v4-flash".into(),
            model_binding_id: None,
            reasoning_effort_selection,
            request_model: "deepseek-v4-flash".into(),
            request_thinking: Some("default".into()),
            input_token_estimate: None,
            usage: None,
            latest_query_usage: None,
            context_occupancy: None,
            stop_reason: None,
            failure_reason: None,
            error: None,
            session_context: None,
            turn_context: None,
            schema_version: 2,
        };

        let mut file = std::fs::File::create(&rollout_path)?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: now,
                session,
            })))?
        )?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn,
            })))?
        )?;
        Ok(())
    }

    let data_root = TempDir::new()?;
    let missing_thinking_session = SessionId::new();
    let default_thinking_session = SessionId::new();
    write_historical_rollout(data_root.path(), &missing_thinking_session, None)?;
    write_historical_rollout(
        data_root.path(),
        &default_thinking_session,
        Some("default".into()),
    )?;

    let runtime = build_runtime(data_root.path())?;
    runtime.load_persisted_sessions().await?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    for session_id in [&missing_thinking_session, &default_thinking_session] {
        let resume_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 34,
                    "method": "session/resume",
                    "params": {
                        "session_id": session_id
                    }
                }),
            )
            .await
            .context("session/resume response")?;
        let resume_result = serde_json::from_value::<
            devo_server::SuccessResponse<devo_server::SessionResumeResult>,
        >(resume_response)?
        .result;

        assert_eq!(resume_result.session.session_id, (*session_id).clone());
        assert_eq!(
            resume_result.session.model.as_deref(),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            resume_result.session.reasoning_effort_selection.as_deref(),
            Some("high")
        );
        assert_eq!(
            resume_result.session.reasoning_effort,
            Some(ReasoningEffort::High)
        );
    }

    Ok(())
}

#[tokio::test]
async fn failed_turn_resume_restores_terminal_history_without_prompt_contamination() -> Result<()> {
    let data_root = TempDir::new()?;
    let session_id = SessionId::new();
    let failed_turn_id = devo_protocol::TurnId::new();
    let completed_turn_id = devo_protocol::TurnId::new();
    let now = chrono::Utc::now();
    let rollout_dir = data_root.path().join("sessions/2026/07/14");
    std::fs::create_dir_all(&rollout_dir)?;
    let rollout_path = rollout_dir.join(format!("rollout-2026-07-14T00-00-00-{session_id}.jsonl"));
    let terminal_error = TurnError {
        code: "PROVIDER_SERVER_ERROR".to_string(),
        message: "exact persisted provider failure".to_string(),
        recovery_hint: None,
    };
    let session = SessionRecord {
        id: session_id,
        rollout_path: rollout_path.clone(),
        created_at: now,
        updated_at: now + chrono::Duration::seconds(5),
        last_activity_at: Some(now + chrono::Duration::seconds(5)),
        source: "cli".into(),
        agent_nickname: None,
        agent_role: None,
        agent_path: None,
        model_provider: "test-provider".into(),
        model: Some("test-model".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        cwd: data_root.path().to_path_buf(),
        additional_directories: Vec::new(),
        cli_version: "0.1.0".into(),
        title: Some("Failed turn resume".into()),
        title_state: devo_core::SessionTitleState::Final(
            devo_core::SessionTitleFinalSource::ExplicitCreate,
        ),
        sandbox_policy: "workspace-write".into(),
        approval_mode: "on-request".into(),
        effective_context_window: None,
        tokens_used: 0,
        first_user_message: Some("failing prompt".into()),
        archived_at: None,
        git_sha: None,
        git_branch: None,
        git_origin_url: None,
        parent_session_id: None,
        session_context: None,
        latest_turn_context: None,
        collaboration_mode: None,
        permission_preset: None,
        schema_version: 2,
    };
    let failed_running = TurnRecord {
        id: failed_turn_id,
        session_id,
        sequence: 1,
        started_at: now,
        completed_at: None,
        status: TurnStatus::Running,
        kind: devo_core::TurnKind::Regular,
        model: "test-model".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "test-model".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    };
    let failed_terminal = TurnRecord {
        completed_at: Some(now + chrono::Duration::seconds(2)),
        status: TurnStatus::Failed,
        error: Some(terminal_error.clone()),
        ..failed_running.clone()
    };
    let completed_running = TurnRecord {
        id: completed_turn_id,
        sequence: 2,
        started_at: now + chrono::Duration::seconds(3),
        ..failed_running.clone()
    };
    let completed_terminal = TurnRecord {
        completed_at: Some(now + chrono::Duration::seconds(5)),
        status: TurnStatus::Completed,
        ..completed_running.clone()
    };
    let item_record = |turn_id, seq, timestamp, turn_item| ItemRecord {
        id: devo_core::ItemId::new(),
        session_id,
        turn_id,
        seq,
        timestamp,
        attempt_placement: None,
        turn_status: None,
        sibling_turn_ids: Vec::new(),
        input_items: Vec::new(),
        output_items: vec![turn_item],
        worklog: None,
        error: None,
        schema_version: 1,
    };
    let rollout_lines = vec![
        RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: now,
            session,
        })),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: now,
            turn: failed_running,
        })),
        RolloutLine::Item(ItemLine {
            timestamp: now,
            item: item_record(
                failed_turn_id,
                1,
                now,
                TurnItem::UserMessage(TextItem {
                    text: "failing prompt".into(),
                }),
            ),
        }),
        RolloutLine::Item(ItemLine {
            timestamp: now + chrono::Duration::seconds(1),
            item: item_record(
                failed_turn_id,
                2,
                now + chrono::Duration::seconds(1),
                TurnItem::AgentMessage(TextItem {
                    text: "partial response".into(),
                }),
            ),
        }),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: now + chrono::Duration::seconds(2),
            turn: failed_terminal,
        })),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: now + chrono::Duration::seconds(3),
            turn: completed_running,
        })),
        RolloutLine::Item(ItemLine {
            timestamp: now + chrono::Duration::seconds(3),
            item: item_record(
                completed_turn_id,
                3,
                now + chrono::Duration::seconds(3),
                TurnItem::UserMessage(TextItem {
                    text: "next prompt".into(),
                }),
            ),
        }),
        RolloutLine::Item(ItemLine {
            timestamp: now + chrono::Duration::seconds(4),
            item: item_record(
                completed_turn_id,
                4,
                now + chrono::Duration::seconds(4),
                TurnItem::AgentMessage(TextItem {
                    text: "next response".into(),
                }),
            ),
        }),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: now + chrono::Duration::seconds(5),
            turn: completed_terminal,
        })),
    ];
    let mut file = std::fs::File::create(&rollout_path)?;
    for line in rollout_lines {
        writeln!(file, "{}", serde_json::to_string(&line)?)?;
    }

    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    runtime.load_persisted_sessions().await?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 35,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume failed turn history")?;
    let resume = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;
    let expected_error = devo_protocol::SessionHistoryItem::new(
        None,
        SessionHistoryItemKind::Error,
        terminal_error.code.clone(),
        terminal_error.message.clone(),
    );
    let expected_summary = devo_protocol::SessionHistoryItem {
        tool_call_id: None,
        kind: SessionHistoryItemKind::TurnSummary,
        title: "test-model".to_string(),
        body: "failed".to_string(),
        tool_io: None,
        metadata: Some(devo_protocol::SessionHistoryMetadata::TurnSummary {
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        }),
        duration_ms: Some(2),
    };
    let terminal_index = resume
        .history_items
        .windows(2)
        .position(|items| items == [expected_error.clone(), expected_summary.clone()])
        .context("top-level terminal error followed by failed summary")?;
    assert_eq!(resume.history_items[terminal_index + 2].body, "next prompt");
    assert!(
        resume.history_items[..terminal_index]
            .iter()
            .any(|item| item.body == "partial response")
    );

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 36,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "continue after failure" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start after failed resume")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;
    let requests = provider.requests.lock().expect("lock requests");
    let request_json = serde_json::to_string(requests.last().context("captured model request")?)?;
    assert!(!request_json.contains(&terminal_error.code));
    assert!(!request_json.contains(&terminal_error.message));

    Ok(())
}

#[tokio::test]
async fn runtime_recovers_session_when_middle_rollout_line_is_corrupted() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 41,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Recoverable session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 42,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist this session before corruption" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let sessions_root = data_root.path().join("sessions");
    let rollout_path = std::fs::read_dir(&sessions_root)?
        .next()
        .context("expected year partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected month partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected day partition")??
        .path();
    let rollout_path = std::fs::read_dir(rollout_path)?
        .next()
        .context("expected rollout file")??
        .path();

    let mut lines = std::fs::read_to_string(&rollout_path)?
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert!(lines.len() >= 4);
    lines[2] = "{\"Turn\":{\"timestamp\":\"broken\"".to_string();
    std::fs::write(&rollout_path, format!("{}\n", lines.join("\n")))?;

    // Fail closed (05 §2.2): a damaged mid-file line marks the session
    // damaged — it is skipped at load and refuses to resume, rather than
    // silently dropping the history after the damage.
    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 43,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    assert!(
        resume_response.get("error").is_some(),
        "damaged session must refuse resume: {resume_response}"
    );
    Ok(())
}

#[tokio::test]
async fn session_compact_runs_asynchronously_and_emits_lifecycle_events() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 51,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Compaction session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 52,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "create some history first" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;

    wait_for_turn_completed(&mut notifications_rx).await?;

    let compact_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 53,
                "method": "session/compact/start",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    let compact_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(compact_response)?;
    assert_eq!(
        compact_result.result.turn.session_id.as_str(),
        session_id.to_string()
    );

    wait_for_notification_method(&mut notifications_rx, "turn/started").await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionStarted").await?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;
    Ok(())
}

#[tokio::test]
async fn compacted_session_resume_keeps_full_transcript_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 61,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Persist compacted session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    for request_id in 0..3 {
        let large_prompt = "x".repeat(30_000);
        let _ = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 62 + request_id,
                    "method": "turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": large_prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 70,
                "method": "session/compact/start",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 71,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert!(
        resume_result.history_items.len() >= 6,
        "expected full transcript to survive compaction, got {:?}",
        resume_result.history_items
    );
    assert!(
        resume_result.history_items.iter().any(|item| {
            item.kind == SessionHistoryItemKind::ContextCompaction
                && item.title == "Context compacted"
        }),
        "expected ContextCompaction history row after compact resume, got {:?}",
        resume_result.history_items
    );
    assert!(
        resume_result
            .history_items
            .iter()
            .all(|item| !item.body.contains("<compaction_summary>")),
        "compaction summary must not appear in user-visible transcript"
    );
    assert!(
        resume_result
            .history_items
            .iter()
            .any(|item| item.body.contains("Hello from persistence test.")),
        "expected assistant transcript entries to remain visible"
    );
    Ok(())
}

#[tokio::test]
async fn compacted_session_next_query_uses_compaction_summary_after_restart() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 81,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Prompt snapshot session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    for request_id in 0..3 {
        let large_prompt = "x".repeat(30_000);
        let _ = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 82 + request_id,
                    "method": "turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": large_prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 90,
                "method": "session/compact/start",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/compact response")?;
    wait_for_notification_method(&mut notifications_rx, "context/compactionCompleted").await?;

    let capturing_provider = Arc::new(CapturingProvider::default());
    let rebuilt_runtime =
        build_runtime_with_provider(data_root.path(), capturing_provider.clone())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 90,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response after restart")?;
    let _: devo_server::SuccessResponse<devo_server::SessionResumeResult> =
        serde_json::from_value(resume_response)?;

    let _ = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 91,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "go on" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response after restart")?;
    wait_for_turn_completed(&mut rebuilt_notifications_rx).await?;

    let requests = capturing_provider.requests.lock().expect("lock requests");
    let request = requests
        .last()
        .context("expected captured model request after restart")?;

    assert!(
        request.messages.iter().any(|message| {
            message.content.iter().any(|content| match content {
                devo_protocol::RequestContent::Text { text }
                | devo_protocol::RequestContent::Reasoning { text } => {
                    text.contains("<compaction_summary>")
                }
                devo_protocol::RequestContent::ProviderReasoning { .. }
                | devo_protocol::RequestContent::ToolUse { .. }
                | devo_protocol::RequestContent::HostedToolUse { .. }
                | devo_protocol::RequestContent::ToolResult { .. } => false,
            })
        }),
        "expected prompt request to include compaction summary after restart"
    );
    Ok(())
}

/// High-usage streaming provider that also captures requests and returns a
/// compaction summary from the non-streaming completion path.
struct AutoCompactTestProvider {
    requests: Mutex<Vec<ModelRequest>>,
    input_tokens: usize,
    output_tokens: usize,
}

impl AutoCompactTestProvider {
    fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            input_tokens,
            output_tokens,
        }
    }

    fn usage(&self) -> Usage {
        Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(self.input_tokens / 2),
            reasoning_output_tokens: None,
            total_tokens: Some(self.input_tokens + self.output_tokens),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for AutoCompactTestProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(ModelResponse {
            id: "compact-summary".into(),
            content: vec![ResponseContent::Text(
                "auto compact summary for resume".to_string(),
            )],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        let usage = self.usage();
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::UsageDelta(usage.clone())),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello from auto compact test.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-auto-compact".into(),
                    content: vec![ResponseContent::Text(
                        "Hello from auto compact test.".into(),
                    )],
                    stop_reason: Some(StopReason::EndTurn),
                    usage,
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "auto-compact-test-provider"
    }
}

#[tokio::test]
async fn auto_compaction_persists_snapshot_and_survives_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(AutoCompactTestProvider::new(
        /*input_tokens*/ 80_000, /*output_tokens*/ 1_000,
    ));
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 201,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Auto compact persist session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 202,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "effectiveContextWindow": 5_000 }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;

    // Build enough oversized history that Auto preserve budget cannot keep it all.
    for request_id in 0..3 {
        let large_prompt = "x".repeat(100_000);
        let _ = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 210 + request_id,
                    "method": "turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": large_prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let capturing_provider = Arc::new(CapturingProvider::default());
    let rebuilt_runtime =
        build_runtime_with_provider(data_root.path(), capturing_provider.clone())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 220,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume after auto compact")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    assert!(
        resume_result.history_items.iter().any(|item| {
            item.kind == SessionHistoryItemKind::ContextCompaction
                && item.title == "Context compacted"
        }),
        "expected ContextCompaction history after auto-compact resume, got {:?}",
        resume_result.history_items
    );

    let _ = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 221,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "continue" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("post-resume turn")?;
    wait_for_turn_completed(&mut rebuilt_notifications_rx).await?;

    let requests = capturing_provider.requests.lock().expect("lock requests");
    let request = requests
        .last()
        .context("expected captured model request after auto-compact resume")?;
    assert!(
        request.messages.iter().any(|message| {
            message.content.iter().any(|content| match content {
                devo_protocol::RequestContent::Text { text }
                | devo_protocol::RequestContent::Reasoning { text } => {
                    text.contains("<compaction_summary>") || text.contains("auto compact summary")
                }
                devo_protocol::RequestContent::ProviderReasoning { .. }
                | devo_protocol::RequestContent::ToolUse { .. }
                | devo_protocol::RequestContent::HostedToolUse { .. }
                | devo_protocol::RequestContent::ToolResult { .. } => false,
            })
        }),
        "expected post-resume prompt to use compacted history, got {:?}",
        request.messages
    );
    Ok(())
}

#[tokio::test]
async fn configured_request_model_is_used_for_turn_metadata_and_provider_request() -> Result<()> {
    let data_root = TempDir::new()?;
    std::fs::create_dir_all(data_root.path().join(".devo"))?;
    std::fs::write(
        data_root.path().join(".devo").join("config.toml"),
        r#"
[model.test-model]
display_name = "test-model"
provider = "openai_chat_completions"
reasoning_capability = "toggle"
reasoning_implementation = { model_variant = { variants = [
  { selection_value = "disabled", model_slug = "test-model", label = "Off", description = "Disable reasoning effort" },
  { selection_value = "enabled", model_slug = "vendor/test-model", reasoning_effort = "medium", label = "On", description = "Enable reasoning effort" },
] } }
base_instructions = "Test model"
"#,
    )?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 101,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Request model session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 102,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "use configured request model" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let turn_started = wait_for_notification_value(&mut notifications_rx, "turn/started").await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(
        turn_started["params"]["turn"]["model"],
        serde_json::json!({
            "provider": "unknown",
            "model": "vendor/test-model",
            "reasoningEffort": "medium"
        })
    );
    let requests = provider.requests.lock().expect("lock requests");
    assert_eq!(
        requests.last().expect("captured request").model,
        "vendor/test-model"
    );

    Ok(())
}

fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_provider(data_root, Arc::new(SingleReplyProvider))
}

fn build_runtime_with_provider(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db_path = data_root.join("test_persistence.db");
    let db = Arc::new(devo_server::db::Database::open(db_path).expect("open test database"));
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![
                Model {
                    slug: "test-model".to_string(),
                    display_name: "test-model".to_string(),
                    ..Model::default()
                },
                Model {
                    slug: "deepseek-v4-flash".to_string(),
                    display_name: "deepseek-v4-flash".to_string(),
                    reasoning_capability: ReasoningCapability::ToggleWithLevels(vec![
                        ReasoningEffort::High,
                        ReasoningEffort::Max,
                    ]),
                    default_reasoning_effort: Some(ReasoningEffort::High),
                    ..Model::default()
                },
            ])),
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
                "id": 10,
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

fn decode_native_session_list_response(
    response: serde_json::Value,
) -> Result<Vec<devo_protocol::native::session::Session>> {
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionListResult,
    > = serde_json::from_value(response)?;
    Ok(response.result.data)
}

fn legacy_event_from_acp_notification(value: serde_json::Value) -> serde_json::Value {
    if value.get("method") != Some(&serde_json::json!("session/update")) {
        return value;
    }
    let Ok(notification) =
        serde_json::from_value::<devo_protocol::AcpSessionNotification>(value["params"].clone())
    else {
        return value;
    };
    let Some((method, event)) = devo_protocol::original_event_from_acp_notification(&notification)
    else {
        return value;
    };
    let params = match event {
        ServerEvent::TurnCompleted(payload)
        | ServerEvent::TurnInterrupted(payload)
        | ServerEvent::TurnStarted(payload) => serde_json::to_value(payload),
        ServerEvent::TurnFailed(payload) => serde_json::to_value(payload),
        ServerEvent::SessionCompactionStarted(payload) => serde_json::to_value(payload),
        ServerEvent::SessionCompactionCompleted(payload) => serde_json::to_value(payload),
        ServerEvent::SessionTitleUpdated(payload) | ServerEvent::SessionStarted(payload) => {
            serde_json::to_value(payload)
        }
        ServerEvent::ItemCompleted(payload) | ServerEvent::ItemStarted(payload) => {
            serde_json::to_value(payload)
        }
        ServerEvent::ItemDelta {
            delta_kind,
            payload,
        } => serde_json::to_value(serde_json::json!({
            "delta_kind": delta_kind,
            "payload": payload,
        })),
        other => serde_json::to_value(other),
    }
    .expect("serialize legacy event params");
    serde_json::json!({
        "method": method,
        "params": params,
    })
}

fn title_from_notification(value: &serde_json::Value) -> Option<&str> {
    if value.get("method") == Some(&serde_json::json!("session/metadataUpdated")) {
        return value["params"]["session"]["title"].as_str();
    }
    if value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["update"]["sessionUpdate"] == serde_json::json!("session_info_update")
    {
        return value["params"]["update"]["title"].as_str();
    }
    None
}

fn notification_matches_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method") == Some(&serde_json::json!(method))
        || (method == "item/agentMessage/delta"
            && value.get("method") == Some(&serde_json::json!("session/update"))
            && value["params"]["update"]["sessionUpdate"]
                == serde_json::json!("agent_message_chunk"))
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            let value = legacy_event_from_acp_notification(value);
            if value.get("method") == Some(&serde_json::json!("turn/completed")) {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")??;
    Ok(())
}

async fn wait_for_turn_usage_updated(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<devo_protocol::native::usage::UsageTotals> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("turn/usage/updated")) {
                return serde_json::from_value(value["params"]["sessionTotals"].clone())
                    .context("decode Native turn/usage/updated session totals");
            }
        }
        anyhow::bail!("notification channel closed before turn/usage/updated")
    })
    .await
    .context("timed out waiting for turn/usage/updated")?
}

async fn wait_for_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_title: &str,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if title_from_notification(&value) == Some(expected_title) {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before expected title update")
    })
    .await
    .context("timed out waiting for title update")??;
    Ok(())
}

async fn wait_for_any_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<String> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if let Some(title) = title_from_notification(&value) {
                return Ok(title.to_string());
            }
        }
        anyhow::bail!("notification channel closed before any title update")
    })
    .await
    .context("timed out waiting for title update")?
}

async fn wait_for_notification_method(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<()> {
    wait_for_notification_value(notifications_rx, method)
        .await
        .map(|_| ())
}

async fn wait_for_notification_value(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value> {
    let value = timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            let normalized = legacy_event_from_acp_notification(value.clone());
            if notification_matches_method(&normalized, method)
                || notification_matches_method(&value, method)
            {
                return Ok(normalized);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method}"))??;
    Ok(value)
}

#[tokio::test]
async fn interrupt_mid_stream_does_not_duplicate_last_item_on_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let gated = Arc::new(GatedProvider::new());
    let runtime = build_runtime_with_provider(data_root.path(), Arc::clone(&gated) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "interrupt me" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start")?;
    let _turn_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::TurnStartResult>,
    >(turn_start_response)?
    .result
    .turn_id()
    .expect("turn/start should start a streaming turn");

    // Wait until the assistant item has started streaming.  The provider yields
    // one TextDelta, then blocks, so once we see the delta notification we know
    // deferred_assistant has been stored in the session.
    wait_for_notification_method(&mut notifications_rx, "item/assistantMessage/delta").await?;

    // Now interrupt the turn while it is still in-progress.
    let interrupt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/interrupt",
                "params": {
                    "scope": {
                        "scope": "session",
                        "sessionId": session_id
                    }
                }
            }),
        )
        .await
        .context("session/interrupt")?;
    let interrupt_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionInterruptResult,
    > = serde_json::from_value(interrupt_response)?;
    assert!(interrupt_result.result.interrupted);

    // Native projects every terminal turn state through turn/completed.
    wait_for_notification_method(&mut notifications_rx, "turn/completed").await?;

    // Rebuild runtime (simulates restart) and resume the session.
    let gated2 = Arc::new(GatedProvider::new());
    let rebuilt = build_runtime_with_provider(data_root.path(), Arc::clone(&gated2) as _)?;
    rebuilt.load_persisted_sessions().await?;
    let (rebuilt_cid, _) = initialize_connection(&rebuilt).await?;

    let resume_response = rebuilt
        .handle_incoming(
            rebuilt_cid,
            serde_json::json!({
                "id": 4,
                "method": "session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;

    // The crucial assertion: no two consecutive items should have the same
    // kind if they are Assistant or Reasoning — those are the types that
    // were being duplicated by the event_task post-loop cleanup race.
    let kinds: Vec<_> = resume_result
        .history_items
        .iter()
        .map(|i| &i.kind)
        .collect();
    for window in kinds.windows(2) {
        if window[0] == window[1] {
            match window[0] {
                SessionHistoryItemKind::Assistant | SessionHistoryItemKind::Reasoning => {
                    anyhow::bail!(
                        "duplicate consecutive {:?} items detected: indices {:?}",
                        window[0],
                        kinds
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, k)| {
                                if *k == window[0] { Some(idx) } else { None }
                            })
                            .collect::<Vec<_>>()
                    );
                }
                _ => {}
            }
        }
    }

    // Sanity: there should be exactly one User and one Assistant item.
    let user_count = kinds
        .iter()
        .filter(|k| matches!(k, SessionHistoryItemKind::User))
        .count();
    let assistant_count = kinds
        .iter()
        .filter(|k| matches!(k, SessionHistoryItemKind::Assistant))
        .count();
    assert_eq!(user_count, 1, "expected exactly one User item");
    assert_eq!(
        assistant_count, 1,
        "expected exactly one Assistant item, got history: {kinds:?}"
    );

    Ok(())
}

#[tokio::test]
async fn first_usage_update_after_resume_preserves_historical_session_totals() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(UsageReplyProvider::new(100, 25));
    let runtime = build_runtime_with_provider(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Usage resume base",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let first_turn_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist usage totals" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("first turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(first_turn_response)?;
    let first_usage = wait_for_turn_usage_updated(&mut notifications_rx).await?;
    assert_eq!(first_usage.input_tokens, 100);
    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime_with_provider(data_root.path(), provider)?;
    rebuilt_runtime.refresh_session_index()?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let resumed = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result
    .session;
    assert_eq!(resumed.total_input_tokens, 100);
    // Context length comes from the latest query snapshot, not cumulative totals.
    let last_query_usage = resumed
        .last_query_usage
        .expect("resumed session should include last_query_usage");
    assert_eq!(last_query_usage.input_tokens, 100);
    assert_eq!(last_query_usage.output_tokens, 25);
    assert_eq!(last_query_usage.total_tokens, Some(125));
    assert_eq!(resumed.last_query_total_tokens, 125);
    // Cumulative session totals remain distinct from latest-query context length.
    assert_ne!(resumed.total_input_tokens, resumed.last_query_total_tokens);

    let second_turn_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 4,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "post resume usage" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("second turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(second_turn_response)?;
    let post_resume_usage = wait_for_turn_usage_updated(&mut rebuilt_notifications_rx).await?;
    assert_eq!(post_resume_usage.input_tokens, 200);
    assert_eq!(post_resume_usage.output_tokens, 50);
    assert_eq!(post_resume_usage.cache_read_input_tokens, 100);
    Ok(())
}

#[tokio::test]
async fn rollout_writes_base_instructions_once_across_multiple_turns() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Rollout dedupe",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    for (request_id, prompt) in [(2, "first turn"), (3, "second turn")] {
        let turn_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": request_id,
                    "method": "turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": prompt }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
            serde_json::from_value(turn_response)?;
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    runtime.refresh_session_index()?;
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db.get_session_index(&session_id)?.expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");
    let rollout_lines = support::read_rollout_lines_dual(&rollout_path)?;
    let mut session_context_lines = 0usize;
    let mut turn_lines_with_session_context = 0usize;
    for rollout_line in rollout_lines {
        match rollout_line {
            RolloutLine::SessionContextUpdated(_) => session_context_lines += 1,
            RolloutLine::Turn(turn_line) if turn_line.turn.session_context.is_some() => {
                turn_lines_with_session_context += 1;
            }
            _ => {}
        }
    }
    assert_eq!(session_context_lines, 1);
    assert_eq!(turn_lines_with_session_context, 0);

    let rebuilt_runtime = build_runtime(data_root.path())?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let _: devo_server::SuccessResponse<devo_server::SessionResumeResult> =
        serde_json::from_value(resume_response)?;

    Ok(())
}

#[tokio::test]
async fn turn_start_persists_session_context_before_turn_completes() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Turn start crash window",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "crash before complete" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_response)?;

    // Inspect the rollout before waiting for turn completion to simulate a crash
    // after durable turn-start persistence.
    runtime.refresh_session_index()?;
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db.get_session_index(&session_id)?.expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let (session_context_lines, turn_lines) = loop {
        let rollout_lines = support::read_rollout_lines_dual(&rollout_path)?;
        let mut session_context_lines = 0usize;
        let mut turn_lines = 0usize;
        for rollout_line in rollout_lines {
            match rollout_line {
                RolloutLine::SessionContextUpdated(_) => session_context_lines += 1,
                RolloutLine::Turn(_) => turn_lines += 1,
                _ => {}
            }
        }
        if session_context_lines >= 1 && turn_lines >= 1 {
            break (session_context_lines, turn_lines);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for turn-start SessionContextUpdated; context_lines={session_context_lines} turn_lines={turn_lines}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(session_context_lines, 1);
    assert!(turn_lines >= 1);

    wait_for_turn_completed(&mut notifications_rx).await?;
    Ok(())
}

#[tokio::test]
async fn metadata_update_renames_cold_session_by_id_without_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Cold title",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;
    let turn_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist the cold session" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;
    drop(runtime);

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let rename_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "title": "Renamed while cold"
                }
            }),
        )
        .await
        .context("cold session metadata update")?;
    let renamed = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(rename_response)?
    .result;
    assert_eq!(renamed.session.title.as_deref(), Some("Renamed while cold"));

    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list after cold rename")?;
    let sessions = decode_native_session_list_response(list_response)?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("renamed cold session remains listed");
    assert_eq!(listed.title.as_deref(), Some("Renamed while cold"));
    Ok(())
}

#[tokio::test]
async fn lazy_resume_loads_parent_session_from_rollout_on_map_miss() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Lazy resume session",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist for lazy resume" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let rollout_path = db
        .get_session_index(&session_id)?
        .expect("indexed session")
        .rollout_path
        .expect("rollout path");
    let expected_transcript_size = std::fs::metadata(&rollout_path)?.len();
    let list_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("listed persisted session");
    assert_eq!(listed.transcript_size_bytes, Some(expected_transcript_size));

    let turns_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "session/turns/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/turns/list response")?;
    assert_eq!(
        turns_response["result"]["data"][0]["collaborationMode"],
        "build"
    );

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;

    let (rebuilt_connection_id, _rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;
    assert_eq!(resume_result.session.session_id, session_id);
    Ok(())
}

#[tokio::test]
async fn lazy_resume_after_compat_backfill_without_refresh_session_index() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Lazy resume backfill",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist for compat backfill" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let db_path = data_root.path().join("test_persistence.db");
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "UPDATE sessions SET rollout_path = NULL WHERE id = ?1",
            rusqlite::params![session_id.to_string()],
        )?;
    }

    let rebuilt_runtime = build_runtime(data_root.path())?;
    assert!(rebuilt_runtime.backfill_session_index_if_required()?);

    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_result = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;
    assert_eq!(resume_result.session.session_id, session_id);
    Ok(())
}

#[tokio::test]
async fn concurrent_lazy_resume_single_actor() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Concurrent lazy resume",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "persist for concurrent resume" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.refresh_session_index()?;
    let (connection_a, _) = initialize_connection(&rebuilt_runtime).await?;
    let (connection_b, _) = initialize_connection(&rebuilt_runtime).await?;

    let request_a = rebuilt_runtime.handle_incoming(
        connection_a,
        serde_json::json!({
            "id": 3,
            "method": "session/resume",
            "params": { "session_id": session_id }
        }),
    );
    let request_b = rebuilt_runtime.handle_incoming(
        connection_b,
        serde_json::json!({
            "id": 4,
            "method": "session/resume",
            "params": { "session_id": session_id }
        }),
    );
    let (response_a, response_b) = tokio::join!(request_a, request_b);
    let response_a = response_a.context("first session/resume response")?;
    let response_b = response_b.context("second session/resume response")?;
    let result_a = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(response_a)?
    .result;
    let result_b = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(response_b)?
    .result;
    assert_eq!(result_a.session.session_id, session_id);
    assert_eq!(result_b.session.session_id, session_id);

    Ok(())
}

#[tokio::test]
async fn lazy_resume_rejects_subagent_session_ids() -> Result<()> {
    let data_root = TempDir::new()?;
    let parent_id = SessionId::new();
    let child_id = SessionId::new();
    let now = chrono::Utc::now();
    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let mut parent = sample_indexed_session(parent_id, data_root.path(), now, None);
    parent.title = Some("Parent".into());
    db.upsert_session(&parent, Some("/tmp/parent.jsonl".as_ref()))?;
    let mut child = sample_indexed_session(child_id, data_root.path(), now, Some(parent_id));
    child.title = Some("Child".into());
    child.agent_path = Some("root/subagent".into());
    db.upsert_session(&child, Some("/tmp/child.jsonl".as_ref()))?;
    let runtime = build_runtime(data_root.path())?;

    let (connection_id, _) = initialize_connection(&runtime).await?;
    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/resume",
                "params": { "session_id": child_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let error = serde_json::from_value::<devo_server::ErrorResponse>(resume_response)?;
    assert_eq!(
        error.error.code,
        devo_server::ProtocolErrorCode::InvalidParams
    );
    assert!(
        error
            .error
            .message
            .contains("subagent sessions cannot be resumed directly")
    );
    Ok(())
}

#[tokio::test]
async fn lazy_resume_fails_when_rollout_file_is_missing() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/start",
                "params": {
                    "cwd": data_root.path(),
                    "ephemeral": false,
                    "title": "Missing rollout",
                    "model": "test-model"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionStartResult>,
    >(start_response)?
    .result
    .session
    .session_id;

    let turn_start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "create rollout then delete it" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(turn_start_response)?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    let db = devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?;
    let index = db.get_session_index(&session_id)?.expect("indexed session");
    let rollout_path = index.rollout_path.expect("rollout path");
    std::fs::remove_file(&rollout_path)?;

    let rebuilt_runtime = build_runtime(data_root.path())?;
    let (rebuilt_connection_id, _) = initialize_connection(&rebuilt_runtime).await?;
    let list_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 30,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;
    let listed = sessions
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("missing-rollout session remains listed");
    assert_eq!(listed.transcript_size_bytes, None);

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/resume",
                "params": { "session_id": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let error = serde_json::from_value::<devo_server::ErrorResponse>(resume_response)?;
    assert_eq!(
        error.error.code,
        devo_server::ProtocolErrorCode::InternalError
    );
    assert!(error.error.message.contains("rollout file is missing"));
    Ok(())
}

fn sample_indexed_session(
    session_id: SessionId,
    cwd: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    parent_session_id: Option<SessionId>,
) -> devo_server::SessionMetadata {
    devo_server::SessionMetadata {
        session_id,
        cwd: cwd.to_path_buf(),
        additional_directories: Vec::new(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: None,
        title_state: devo_core::SessionTitleState::Unset,
        parent_session_id,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: false,
        model: Some("test-model".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        last_context_occupancy: None,
        status: devo_protocol::SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
        effective_context_window: None,
        permission_preset: None,
    }
}
