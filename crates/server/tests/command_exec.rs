use std::pin::Pin;
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
#[cfg(unix)]
use base64::Engine;
#[cfg(unix)]
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use devo_core::AgentsMdConfig;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
#[cfg(unix)]
use devo_protocol::CommandExecResult;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
#[cfg(unix)]
use devo_protocol::SessionId;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ProtocolErrorCode;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
#[cfg(unix)]
use tokio::time::timeout;

struct UnusedProvider;

#[async_trait]
impl ModelProviderSDK for UnusedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("command exec tests do not use model completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("command exec tests do not use model streaming")
    }

    fn name(&self) -> &str {
        "unused-command-exec-provider"
    }
}

#[cfg(unix)]
/// Trace: L2-DES-APP-003
/// Verifies: sessionless command execution starts with explicit cwd, streams to the owning connection, and does not create a session.
#[tokio::test]
async fn sessionless_command_exec_streams_to_owner_without_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (owner_connection_id, mut owner_notifications_rx) =
        initialize_connection(&runtime, "command-exec-owner").await?;
    let (_other_connection_id, mut other_notifications_rx) =
        initialize_connection(&runtime, "command-exec-other").await?;

    let response = runtime
        .handle_incoming(
            owner_connection_id,
            serde_json::json!({
                "id": 20,
                "method": "command/exec",
                "params": {
                    "process_id": "sessionless-1",
                    "cwd": data_root.path(),
                    "program": {
                        "type": "one_shot",
                        "command": "printf 'sessionless-owned\\n'"
                    },
                    "size": null
                }
            }),
        )
        .await
        .context("command/exec response")?;
    let response: devo_server::SuccessResponse<CommandExecResult> =
        serde_json::from_value(response)?;
    assert_eq!(
        response.result,
        CommandExecResult {
            process_id: "sessionless-1".to_string()
        }
    );

    let output = wait_for_command_exec_exit(
        &mut owner_notifications_rx,
        "sessionless-1",
        /*session_id*/ None,
        Some(0),
    )
    .await?;
    assert!(output.contains("sessionless-owned"));

    assert_no_notification(&mut other_notifications_rx).await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn session_interrupt_stops_sessionless_command_without_a_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) =
        initialize_connection(&runtime, "command-interrupt-owner").await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 22,
                "method": "command/exec",
                "params": {
                    "process_id": "sessionless-interrupt-1",
                    "cwd": data_root.path(),
                    "program": {
                        "type": "one_shot",
                        "command": "sleep 60"
                    },
                    "size": null
                }
            }),
        )
        .await
        .context("command/exec response")?;
    let response: devo_server::SuccessResponse<CommandExecResult> =
        serde_json::from_value(response)?;
    assert_eq!(response.result.process_id, "sessionless-interrupt-1");

    let interrupted = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 23,
                "method": "session/interrupt",
                "params": {
                    "scope": {
                        "scope": "command",
                        "processId": "sessionless-interrupt-1"
                    }
                }
            }),
        )
        .await
        .context("session/interrupt response")?;
    assert!(
        interrupted.get("error").is_none(),
        "session/interrupt failed: {interrupted}"
    );
    let interrupted: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionInterruptResult,
    > = serde_json::from_value(interrupted)?;
    assert!(interrupted.result.interrupted);

    let _ = wait_for_command_exec_exit(
        &mut notifications_rx,
        "sessionless-interrupt-1",
        /*session_id*/ None,
        /*expected_exit_code*/ None,
    )
    .await?;
    Ok(())
}

/// Trace: L2-DES-APP-003
/// Verifies: sessionless command execution rejects requests that omit explicit cwd.
#[tokio::test]
async fn sessionless_command_exec_requires_explicit_cwd() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) =
        initialize_connection(&runtime, "command-exec-missing-cwd").await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "command/exec",
                "params": {
                    "process_id": "sessionless-missing-cwd",
                    "program": {
                        "type": "one_shot",
                        "command": "pwd"
                    },
                    "size": null
                }
            }),
        )
        .await
        .context("command/exec response")?;
    let response: devo_server::ErrorResponse = serde_json::from_value(response)?;

    assert_eq!(response.error.code, ProtocolErrorCode::InvalidParams);
    assert_eq!(
        response.error.message,
        "command/exec cwd is required when session_id is omitted"
    );

    Ok(())
}

#[cfg(unix)]
/// Trace: L2-DES-APP-003
/// Verifies: session-bound command execution can resolve cwd from the existing session.
#[tokio::test]
async fn session_bound_command_exec_resolves_session_cwd() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) =
        initialize_connection(&runtime, "command-exec-session-bound").await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 23,
                "method": "command/exec",
                "params": {
                    "sessionId": session_id,
                    "process_id": "session-bound-1",
                    "program": {
                        "type": "one_shot",
                        "command": "printf 'session-bound\\n'"
                    },
                    "size": null
                }
            }),
        )
        .await
        .context("command/exec response")?;
    let response: devo_server::SuccessResponse<CommandExecResult> =
        serde_json::from_value(response)?;
    assert_eq!(
        response.result,
        CommandExecResult {
            process_id: "session-bound-1".to_string()
        }
    );

    let output = wait_for_command_exec_exit(
        &mut notifications_rx,
        "session-bound-1",
        Some(session_id),
        Some(0),
    )
    .await?;
    assert!(output.contains("session-bound"));

    Ok(())
}

fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(UnusedProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("command_exec.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                /*workspace_root*/ None,
            )?)),
        ),
    ))
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
    client_name: &str,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(128);
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
                    "clientInfo": {
                        "name": client_name,
                        "title": client_name,
                        "version": "1.0.0"
                    },
                    "_meta": { "devo": { "protocol": "native" } }
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

#[cfg(unix)]
async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 22,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "command-exec-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)?;
    Ok(SessionId::try_from(response.result.session.id.as_str())?)
}

#[cfg(unix)]
async fn wait_for_command_exec_exit(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    process_id: &str,
    session_id: Option<SessionId>,
    expected_exit_code: Option<i32>,
) -> Result<String> {
    let mut output = String::new();
    loop {
        let notification = timeout(Duration::from_secs(10), notifications_rx.recv())
            .await
            .context("timed out waiting for command/exec notification")?
            .context("notification channel closed before command/exec exited")?;
        let payload = {
            let method = notification["method"].as_str();
            match method {
                Some(method @ ("command/exec/outputDelta" | "command/exec/exited")) => {
                    Some((method, &notification["params"]))
                }
                Some("session/update") => {
                    let meta = &notification["params"]["_meta"];
                    let original_method = meta["devo/originalMethod"].as_str();
                    let original_event = &meta["devo/originalEvent"];
                    original_method.map(|method| {
                        let event_payload = if original_event.get("process_id").is_some() {
                            original_event
                        } else {
                            match method {
                                "command/exec/outputDelta" => {
                                    &original_event["CommandExecOutputDelta"]
                                }
                                "command/exec/exited" => &original_event["CommandExecExited"],
                                _ => original_event,
                            }
                        };
                        (method, event_payload)
                    })
                }
                Some(method) => Some((method, &notification["params"])),
                None => None,
            }
        };
        match payload {
            Some(("command/exec/outputDelta", params)) => {
                if params["process_id"] != serde_json::json!(process_id) {
                    continue;
                }
                assert_notification_session(params, session_id);
                assert_eq!(params["stream"], "pty");
                let delta_base64 = params["delta_base64"]
                    .as_str()
                    .context("delta_base64 should be a string")?;
                let bytes = BASE64_STANDARD.decode(delta_base64)?;
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
            Some(("command/exec/exited", params)) => {
                if params["process_id"] != serde_json::json!(process_id) {
                    continue;
                }
                assert_notification_session(params, session_id);
                if let Some(expected_exit_code) = expected_exit_code {
                    assert_eq!(params["exit_code"], expected_exit_code);
                }
                return Ok(output);
            }
            Some(("session/created" | "session/started", _)) if session_id.is_none() => {
                anyhow::bail!("sessionless command/exec unexpectedly created a session")
            }
            _ => {}
        }
    }
}

#[cfg(unix)]
fn assert_notification_session(params: &serde_json::Value, session_id: Option<SessionId>) {
    match session_id {
        Some(session_id) => {
            assert_eq!(params["session_id"], serde_json::json!(session_id));
        }
        None => {
            assert!(
                params.get("session_id").is_none(),
                "sessionless command/exec notification should omit session_id: {params}"
            );
        }
    }
}

#[cfg(unix)]
async fn assert_no_notification(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    match timeout(Duration::from_millis(200), notifications_rx.recv()).await {
        Ok(Some(notification)) => anyhow::bail!("unexpected notification: {notification}"),
        Ok(None) | Err(_) => Ok(()),
    }
}
