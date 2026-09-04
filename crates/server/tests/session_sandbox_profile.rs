//! Sandbox profile switching via project config seeding and the
//! canonical `session/metadata/update` JSON-RPC method. The ACP
//! `sandbox_profile` config option is intentionally hidden; sandbox follows
//! `/permissions` (and Session Mode) for interactive clients.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AgentsMdConfig;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::SuccessResponse;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

struct NoopProvider;

#[async_trait]
impl ModelProviderSDK for NoopProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "noop-response".to_string(),
            content: vec![ResponseContent::Text("noop".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn name(&self) -> &str {
        "noop-sandbox-profile-provider"
    }
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("sandbox_profile.db"),
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

#[derive(Clone, Copy)]
enum TestProtocol {
    Native,
    Acp,
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
    protocol: TestProtocol,
) -> Result<u64> {
    let (notifications_tx, _notifications_rx) = devo_server::test_outbound_channel(128);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let mut params = serde_json::json!({
        "protocolVersion": 1,
        "clientCapabilities": {},
        "clientInfo": {
            "name": "sandbox-profile-test",
            "title": "Sandbox Profile Test",
            "version": "1.0.0"
        }
    });
    match protocol {
        TestProtocol::Native => {
            params["_meta"] = serde_json::json!({ "devo": { "protocol": "native" } });
        }
        TestProtocol::Acp => {}
    }
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": params
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok(connection_id)
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "sandbox-profile-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response)?;
    Ok(SessionId::try_from(response.result.session.id.as_str())?)
}

async fn new_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<serde_json::Value> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/new",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned(),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    Ok(response["result"].clone())
}

async fn set_acp_config_option(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    config_id: &str,
    value: &str,
) -> Result<serde_json::Value> {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/set_config_option",
                "params": {
                    "sessionId": session_id,
                    "configId": config_id,
                    "value": value
                }
            }),
        )
        .await
        .context("session/set_config_option response")
}

fn config_option<'a>(
    result: &'a serde_json::Value,
    config_id: &str,
) -> Result<&'a serde_json::Value> {
    result["configOptions"]
        .as_array()
        .and_then(|options| {
            options.iter().find(|option| {
                option.get("id").and_then(serde_json::Value::as_str) == Some(config_id)
            })
        })
        .with_context(|| format!("result included {config_id} config option"))
}

fn write_project_config(data_root: &Path, project_key: &str, sandbox_profile: &str) -> Result<()> {
    let mut project = toml::Table::new();
    project.insert(
        "sandbox_profile".to_string(),
        toml::Value::String(sandbox_profile.to_string()),
    );
    let mut projects = toml::Table::new();
    projects.insert(project_key.to_string(), toml::Value::Table(project));
    let mut root = toml::Table::new();
    root.insert("projects".to_string(), toml::Value::Table(projects));
    std::fs::write(data_root.join("config.toml"), toml::to_string(&root)?)?;
    Ok(())
}

#[tokio::test]
async fn session_new_omits_sandbox_profile_config_option() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let project_key = devo_core::project_config_key(&cwd);
    write_project_config(data_root.path(), &project_key, "strict")?;

    let runtime = build_runtime(data_root.path())?;
    let acp_connection_id = initialize_connection(&runtime, TestProtocol::Acp).await?;
    let native_connection_id = initialize_connection(&runtime, TestProtocol::Native).await?;
    let result = new_acp_session(&runtime, acp_connection_id, &cwd).await?;

    assert!(
        config_option(&result, "sandbox_profile").is_err(),
        "sandbox_profile should not be exposed as an ACP session config option"
    );
    assert!(config_option(&result, "mode").is_ok());

    // Project sandbox_profile still seeds the session; advanced clients use
    // the canonical session settings patch.
    let session_id: SessionId = result["sessionId"]
        .as_str()
        .context("session/new included sessionId")?
        .parse()?;
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(
            update_sandbox_profile(&runtime, native_connection_id, session_id, "strict").await?,
        )?;
    assert_eq!(
        response.result.session.settings.sandbox_profile.as_deref(),
        Some("strict")
    );

    Ok(())
}

#[tokio::test]
async fn acp_set_config_option_still_accepts_sandbox_profile_for_compat() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(cwd.join(".devo"))?;
    std::fs::write(
        cwd.join(".devo").join("sandbox.toml"),
        "[profiles.team-ci]\nextends = \"workspace\"\n",
    )?;

    let runtime = build_runtime(data_root.path())?;
    let acp_connection_id = initialize_connection(&runtime, TestProtocol::Acp).await?;
    let native_connection_id = initialize_connection(&runtime, TestProtocol::Native).await?;
    let result = new_acp_session(&runtime, acp_connection_id, &cwd).await?;
    let session_id: SessionId = result["sessionId"]
        .as_str()
        .context("session/new included sessionId")?
        .parse()?;

    let response = set_acp_config_option(
        &runtime,
        acp_connection_id,
        session_id,
        "sandbox_profile",
        "read-only",
    )
    .await?;
    assert!(response.get("error").is_none(), "{response}");
    assert!(
        config_option(&response["result"], "sandbox_profile").is_err(),
        "sandbox_profile should remain hidden from ACP config options after set"
    );

    let response = set_acp_config_option(
        &runtime,
        acp_connection_id,
        session_id,
        "sandbox_profile",
        "no-such-profile",
    )
    .await?;
    assert_eq!(response["error"]["code"], serde_json::json!(-32602));

    let response = set_acp_config_option(
        &runtime,
        acp_connection_id,
        session_id,
        "sandbox_profile",
        "team-ci",
    )
    .await?;
    assert!(response.get("error").is_none(), "{response}");

    // Full Access implies sandbox off via session mode.
    let response = set_acp_config_option(
        &runtime,
        acp_connection_id,
        session_id,
        "mode",
        "full-access",
    )
    .await?;
    assert!(response.get("error").is_none(), "{response}");
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(
            update_sandbox_profile(&runtime, native_connection_id, session_id, "off").await?,
        )?;
    assert_eq!(
        response.result.session.settings.sandbox_profile.as_deref(),
        Some("off")
    );

    Ok(())
}

async fn update_sandbox_profile(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    profile: &str,
) -> Result<serde_json::Value> {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "sandboxProfile": profile }
                }
            }),
        )
        .await
        .context("session/metadata/update response")
}

#[tokio::test]
async fn session_sandbox_profile_update_applies_normalizes_and_rejects() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;

    let runtime = build_runtime(data_root.path())?;
    let connection_id = initialize_connection(&runtime, TestProtocol::Native).await?;
    let session_id = start_session(&runtime, connection_id, &cwd).await?;

    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(
            update_sandbox_profile(&runtime, connection_id, session_id, "strict").await?,
        )?;
    assert_eq!(
        response.result.session.settings.sandbox_profile.as_deref(),
        Some("strict")
    );

    // Aliases normalize to the canonical profile name.
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(
            update_sandbox_profile(&runtime, connection_id, session_id, "readonly").await?,
        )?;
    assert_eq!(
        response.result.session.settings.sandbox_profile.as_deref(),
        Some("read-only")
    );

    // Unknown profiles are rejected with InvalidParams and do not change the
    // active profile: a follow-up valid update still applies cleanly.
    let response = update_sandbox_profile(
        &runtime,
        connection_id,
        session_id,
        "definitely-not-a-profile",
    )
    .await?;
    assert_eq!(
        response["error"]["code"],
        serde_json::json!("InvalidParams")
    );
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(
            update_sandbox_profile(&runtime, connection_id, session_id, "off").await?,
        )?;
    assert_eq!(
        response.result.session.settings.sandbox_profile.as_deref(),
        Some("off")
    );

    Ok(())
}
