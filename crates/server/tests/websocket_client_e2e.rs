use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_protocol::TurnId;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::WebSocketServerClient;
use devo_server::WebSocketServerClientConfig;
use futures::stream;
use tempfile::TempDir;
use tokio::time::timeout;

struct PendingProvider;

#[async_trait]
impl ModelProviderSDK for PendingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("test provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::pending()))
    }

    fn name(&self) -> &str {
        "pending-test-provider"
    }
}

#[tokio::test]
async fn websocket_server_client_drives_listener_session_and_notifications() -> Result<()> {
    let workspace = TempDir::new()?;
    let server_home = TempDir::new()?;
    let bind_address = free_loopback_address()?;
    let db = Arc::new(devo_server::db::Database::open(
        server_home.path().join("websocket-client-e2e.db"),
    )?);
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(PendingProvider);
    let runtime = ServerRuntime::new(
        server_home.path().to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig::default())),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                server_home.path().to_path_buf(),
                None,
            )?)),
        ),
    );
    let listen = vec![format!("ws://{bind_address}")];
    let listener_task =
        tokio::spawn(
            async move { devo_server::run_listeners(Arc::clone(&runtime), &listen).await },
        );
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut client = WebSocketServerClient::connect(WebSocketServerClientConfig {
        endpoint: format!("ws://{bind_address}"),
        client_capabilities: Default::default(),
    })
    .await?;
    let initialize = client.initialize().await?;
    assert_eq!(initialize.server_name, "devo-server");

    let session = client
        .session_new_native(
            workspace.path().to_path_buf(),
            "websocket-e2e-session".to_string(),
        )
        .await?
        .session;
    assert_eq!(session.cwd, workspace.path());
    let session_id = devo_protocol::SessionId::try_from(session.id.as_str())?;

    client
        .turn_start_native(
            session_id,
            vec![devo_protocol::native::item::UserInput::Text {
                text: "hello".to_string(),
            }],
            "websocket-e2e-turn".to_string(),
        )
        .await?;
    let _turn_id = wait_for_turn_started(&mut client).await?;
    client
        .session_interrupt_native(
            devo_protocol::native::rpc_session::SessionInterruptScope::Session {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
            },
        )
        .await?;

    client.shutdown().await?;
    listener_task.abort();
    let _ = listener_task.await;
    Ok(())
}

fn free_loopback_address() -> Result<String> {
    let listener = StdTcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(format!("127.0.0.1:{port}"))
}

async fn wait_for_turn_started(client: &mut WebSocketServerClient) -> Result<TurnId> {
    timeout(Duration::from_secs(5), async {
        loop {
            let Some(notification) = client.recv_notification().await else {
                anyhow::bail!("websocket client event stream closed");
            };
            if notification.method != "turn/started" {
                continue;
            }
            let turn: devo_protocol::native::turn::Turn =
                serde_json::from_value(notification.params["turn"].clone())
                    .context("decode Native turn/started event")?;
            return TurnId::try_from(turn.id.as_str()).context("convert Native turn id");
        }
    })
    .await
    .context("timed out waiting for turn/started")?
}
