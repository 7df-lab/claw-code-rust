use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use devo_client::WebSocketServerClient;
use devo_client::WebSocketServerClientConfig;
use devo_protocol::AcpAgentCapabilities;
use devo_protocol::AcpClientCapabilities;
use devo_protocol::AcpImplementation;
use devo_protocol::AcpInitializeResult;
use devo_protocol::AcpSuccessResponse;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use devo_protocol::native::rpc_admin::McpSetEnabledParams;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn websocket_client_initializes_sends_requests_and_receives_notifications() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("ws://{}", listener.local_addr()?);
    let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
    let server_task = tokio::spawn(run_loopback_server(listener, requests_tx));

    let mut client = WebSocketServerClient::connect(WebSocketServerClientConfig {
        endpoint,
        client_capabilities: AcpClientCapabilities::default(),
    })
    .await?;

    let initialize = client.initialize().await?;
    assert_eq!(initialize.server_name, "devo-server");
    assert_eq!(
        next_request_method(&mut requests_rx).await?,
        "initialize".to_string()
    );

    let cwd = std::env::temp_dir();
    let session = client
        .session_new_native(cwd.clone(), "websocket-session".to_string())
        .await?
        .session;
    assert_eq!(session.cwd, cwd);
    assert_eq!(
        next_request_method(&mut requests_rx).await?,
        "session/new".to_string()
    );

    client
        .turn_start_native(
            SessionId::try_from(session.id.as_str())?,
            vec![devo_protocol::native::item::UserInput::Text {
                text: "hello".to_string(),
            }],
            "websocket-turn".to_string(),
        )
        .await?;
    assert_eq!(
        next_request_method(&mut requests_rx).await?,
        "turn/start".to_string()
    );

    let providers = client.provider_list().await?;
    assert!(providers.providers.is_empty());
    let request = next_request(&mut requests_rx).await?;
    assert_eq!(request["method"], "provider/list");
    assert_eq!(request["params"], serde_json::json!({}));

    client
        .mcp_set_enabled(McpSetEnabledParams {
            name: "time".to_string(),
            enabled: true,
        })
        .await?;
    let request = next_request(&mut requests_rx).await?;
    assert_eq!(request["method"], "mcp/set_enabled");
    assert_eq!(
        request["params"],
        serde_json::json!({"name": "time", "enabled": true})
    );

    let notification = timeout(Duration::from_secs(2), client.recv_notification())
        .await?
        .context("notification")?;
    assert_eq!(notification.method, "test/event");
    assert_eq!(notification.params, serde_json::json!({ "ok": true }));

    client.shutdown().await?;
    server_task.await??;
    Ok(())
}

async fn run_loopback_server(
    listener: TcpListener,
    requests_tx: mpsc::UnboundedSender<serde_json::Value>,
) -> Result<()> {
    let (stream, _) = listener.accept().await?;
    let mut socket = accept_async(stream).await?;
    let session_id = SessionId::new();
    let turn_id = TurnId::new();

    while let Some(frame) = socket.next().await {
        let Message::Text(text) = frame? else {
            continue;
        };
        let request: serde_json::Value = serde_json::from_str(text.as_str())?;
        let _ = requests_tx.send(request.clone());
        let id = request
            .get("id")
            .cloned()
            .context("request id from client")?;
        match request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .context("request method from client")?
        {
            "initialize" => {
                send_success(
                    &mut socket,
                    id,
                    AcpInitializeResult {
                        protocol_version: 1,
                        agent_capabilities: AcpAgentCapabilities::default(),
                        auth_methods: Vec::new(),
                        agent_info: Some(AcpImplementation::new("devo-server", "test")),
                        meta: None,
                    },
                )
                .await?;
            }
            "session/new" => {
                send_success(
                    &mut socket,
                    id,
                    serde_json::json!({
                        "session": native_session(session_id, std::env::temp_dir())
                    }),
                )
                .await?;
            }
            "turn/start" => {
                send_success(
                    &mut socket,
                    id,
                    serde_json::json!({
                        "turn": native_turn(session_id, turn_id)
                    }),
                )
                .await?;
                socket
                    .send(Message::Text(
                        serde_json::json!({
                            "method": "test/event",
                            "params": { "ok": true }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await?;
            }
            "provider/list" => {
                send_success(&mut socket, id, serde_json::json!({"providers": []})).await?;
            }
            "mcp/set_enabled" => {
                send_success(&mut socket, id, serde_json::json!({"servers": []})).await?;
            }
            other => anyhow::bail!("unexpected client request: {other}"),
        }
    }
    Ok(())
}

fn native_session(session_id: SessionId, cwd: std::path::PathBuf) -> serde_json::Value {
    let now = Utc::now();
    serde_json::json!({
        "id": session_id,
        "version": 1,
        "cwd": cwd,
        "ephemeral": false,
        "createdAt": now,
        "status": "idle",
        "flags": [],
        "archived": false,
        "queuedCount": 0,
        "model": { "provider": "test", "model": "test-model" },
        "settings": { "permissionProfile": "default" },
        "preview": "",
        "lastActivityAt": now,
        "usage": {
            "total": empty_usage_totals(),
            "byPurpose": [],
            "updatedAt": now
        }
    })
}

fn native_turn(session_id: SessionId, turn_id: TurnId) -> serde_json::Value {
    serde_json::json!({
        "id": turn_id,
        "sessionId": session_id,
        "sequence": 1,
        "kind": "regular",
        "status": "inProgress",
        "model": { "provider": "test", "model": "test-model" },
        "startedAt": Utc::now()
    })
}

fn empty_usage_totals() -> serde_json::Value {
    serde_json::json!({
        "totalTokens": 0,
        "inputTokens": 0,
        "outputTokens": 0,
        "reasoningTokens": 0,
        "cacheReadInputTokens": 0,
        "cacheCreationInputTokens": 0,
        "callCount": 0,
        "meteredCallCount": 0,
        "failedCallCount": 0,
        "cancelledCallCount": 0
    })
}

async fn send_success<T: serde::Serialize>(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    id: serde_json::Value,
    result: T,
) -> Result<()> {
    socket
        .send(Message::Text(
            serde_json::to_string(&AcpSuccessResponse::new(id, result))?.into(),
        ))
        .await?;
    Ok(())
}

async fn next_request_method(
    requests_rx: &mut mpsc::UnboundedReceiver<serde_json::Value>,
) -> Result<String> {
    let request = next_request(requests_rx).await?;
    request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .context("captured request method")
}

async fn next_request(
    requests_rx: &mut mpsc::UnboundedReceiver<serde_json::Value>,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(2), requests_rx.recv())
        .await?
        .context("captured request")
}
