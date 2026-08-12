use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use async_trait::async_trait;
use devo_protocol::ErrorResponse;
use devo_protocol::NotificationEnvelope;
use devo_protocol::SuccessResponse;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::AcpErrorCode;
use crate::ClientTransportKind;
use crate::ProtocolSet;
use crate::ServerProtocol;
use crate::ServerRuntime;
use crate::acp_error_response;
use crate::runtime::INBOUND_CONCURRENCY_LIMIT;
use crate::runtime::IncomingResponse;
use crate::runtime::OUTBOUND_CHANNEL_CAPACITY;
use crate::runtime::OutboundFrame;
use crate::runtime::enqueue_outbound;
use crate::runtime::log_outbound_frame;
use crate::runtime::outbound_frame_to_value;
use crate::singleton::SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD;
use crate::singleton::SERVER_CONTROL_SHUTDOWN_METHOD;
use crate::singleton::SERVER_CONTROL_STATUS_METHOD;

/// Per-connection transport state shared between the read loop and handler tasks.
struct ConnectionTransport {
    outbound_tx: mpsc::Sender<OutboundFrame>,
    inbound_semaphore: Arc<Semaphore>,
}

impl ConnectionTransport {
    fn new() -> (Self, mpsc::Receiver<OutboundFrame>) {
        let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        (
            Self {
                outbound_tx,
                inbound_semaphore: Arc::new(Semaphore::new(INBOUND_CONCURRENCY_LIMIT)),
            },
            outbound_rx,
        )
    }
}

enum OutboundSink {
    Stdio,
    WebSocket(
        futures::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            Message,
        >,
    ),
}

async fn write_outbound_frame(sink: &mut OutboundSink, frame: OutboundFrame) -> Result<bool> {
    let value = outbound_frame_to_value(&frame);
    log_outbound_frame(&frame, &value);
    let delivered = frame.delivered;
    let sent = match sink {
        OutboundSink::Stdio => {
            let mut stdout = tokio::io::stdout();
            let bytes = serde_json::to_vec(&value).expect("serialize stdio outbound frame");
            stdout.write_all(&bytes).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
            true
        }
        OutboundSink::WebSocket(writer) => writer
            .send(Message::Text(
                serde_json::to_string(&value)
                    .expect("serialize websocket outbound frame")
                    .into(),
            ))
            .await
            .is_ok(),
    };
    if let Some(delivered) = delivered {
        let _ = delivered.send(sent);
    }
    Ok(sent)
}

fn spawn_outbound_writer(
    connection_id: u64,
    mut rx: mpsc::Receiver<OutboundFrame>,
    mut sink: OutboundSink,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            match write_outbound_frame(&mut sink, frame).await {
                Ok(true) => {}
                Ok(false) if matches!(sink, OutboundSink::WebSocket(_)) => {
                    tracing::debug!(connection_id, "outbound websocket writer closed");
                    break;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(connection_id, error = %error, "outbound writer failed");
                    break;
                }
            }
        }
    })
}

/// Transport trait per L3-BEH-SERVER-001.
///
/// Abstracts the connection to a client, allowing different transport
/// implementations (stdio, WebSocket, etc.) to be used interchangeably.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a success response to the client.
    async fn send_response(&self, response: SuccessResponse<serde_json::Value>);

    /// Send an error response to the client.
    async fn send_error(&self, error: ErrorResponse);

    /// Send a notification to the client.
    async fn send_notification(&self, notification: NotificationEnvelope<serde_json::Value>);

    /// Close the transport connection.
    async fn close(&self);
}

/// EventBroadcaster per L3-BEH-SERVER-001.
///
/// Manages event delivery to connected clients with monotonic per-session
/// sequence numbering and subscription filtering.
pub struct EventBroadcaster {
    /// Per-connection senders, keyed by connection_id.
    connections:
        Arc<tokio::sync::RwLock<std::collections::HashMap<u64, mpsc::Sender<serde_json::Value>>>>,
    /// Per-session monotonic sequence counters.
    sequence_counters: Arc<tokio::sync::RwLock<std::collections::HashMap<String, u64>>>,
}

impl EventBroadcaster {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            sequence_counters: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Register a connection's event sender.
    pub async fn register(&self, connection_id: u64, sender: mpsc::Sender<serde_json::Value>) {
        self.connections.write().await.insert(connection_id, sender);
    }

    /// Unregister a connection.
    pub async fn unregister(&self, connection_id: u64) {
        self.connections.write().await.remove(&connection_id);
    }

    /// Broadcast an event to all connected clients.
    /// Returns the next sequence number for the session.
    pub async fn broadcast(&self, session_id: &str, event: serde_json::Value) -> u64 {
        let mut counters = self.sequence_counters.write().await;
        let seq = counters.entry(session_id.to_string()).or_insert(0);
        *seq += 1;
        let current_seq = *seq;
        drop(counters);

        let senders = self
            .connections
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for sender in senders {
            let _ = sender.send(event.clone()).await;
        }
        current_seq
    }

    /// Get the current sequence number for a session.
    pub async fn current_sequence(&self, session_id: &str) -> u64 {
        self.sequence_counters
            .read()
            .await
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// Get the number of active connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Default bind address used when the WebSocket transport is selected without
/// an explicit host-and-port suffix.
pub const DEFAULT_WEBSOCKET_BIND_ADDRESS: &str = "127.0.0.1:3210";
const INTERNAL_PROXY_BIND_ADDRESS: &str = "127.0.0.1:0";

/// Enumerates the supported listener targets parsed from server config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenTarget {
    /// Start the process-scoped stdio transport.
    Stdio,
    /// Start a WebSocket listener at one host and port pair.
    WebSocket {
        /// The socket address host and port, without the `ws://` prefix.
        bind_address: String,
    },
}

/// Process-private loopback listener used by stdio proxy child processes.
pub struct InternalProxyEndpoint {
    listener: TcpListener,
    endpoint: String,
}

/// Control hooks accepted by the authenticated internal proxy listener.
#[derive(Clone)]
pub struct InternalProxyControl {
    shutdown_token: CancellationToken,
}

impl InternalProxyControl {
    pub fn new(shutdown_token: CancellationToken) -> Self {
        Self { shutdown_token }
    }

    fn request_shutdown(&self) {
        self.shutdown_token.cancel();
    }
}

impl InternalProxyEndpoint {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(INTERNAL_PROXY_BIND_ADDRESS).await?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            endpoint: format!("ws://{local_addr}"),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Parses one configured listen-address string into a typed transport target.
pub fn parse_listen_target(value: &str) -> Result<ListenTarget> {
    if value.eq_ignore_ascii_case("stdio://") || value.eq_ignore_ascii_case("stdio") {
        return Ok(ListenTarget::Stdio);
    }
    if let Some(bind_address) = value.strip_prefix("ws://") {
        return Ok(ListenTarget::WebSocket {
            bind_address: if bind_address.is_empty() {
                DEFAULT_WEBSOCKET_BIND_ADDRESS.to_string()
            } else {
                bind_address.to_string()
            },
        });
    }
    Err(anyhow!("unsupported listen target: {value}"))
}

/// Resolves the configured listen-address strings into the concrete listener
/// targets the process will start.
pub fn resolve_listen_targets(listen: &[String]) -> Result<Vec<ListenTarget>> {
    if listen.is_empty() {
        Ok(vec![
            ListenTarget::Stdio,
            ListenTarget::WebSocket {
                bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.to_string(),
            },
        ])
    } else {
        listen
            .iter()
            .map(|value| parse_listen_target(value))
            .collect::<Result<Vec<_>>>()
    }
}

/// Runs every configured listener target until shutdown.
pub async fn run_listeners(runtime: Arc<ServerRuntime>, listen: &[String]) -> Result<()> {
    let targets = resolve_listen_targets(listen)?;
    run_listener_tasks(runtime, targets, None).await
}

/// Runs configured listener targets plus the internal stdio-proxy endpoint.
///
/// Spawns one async task per listen target (stdio and/or public WebSocket) and,
/// when `internal_proxy` is `Some`, an additional loopback WebSocket accept loop.
/// All tasks run concurrently inside a `JoinSet`; this function returns when the
/// first task completes (normally only on fatal listener error).
///
/// The internal proxy task authenticates clients with `token`, handles one-shot
/// control RPCs (`status` / `shutdown`), then treats the connection as a
/// protocol-uninitialized `ClientTransportKind::StdioProxy` connection (see
/// `handle_internal_proxy_connection`).
pub async fn run_listeners_with_internal_proxy(
    runtime: Arc<ServerRuntime>,
    listen: &[String],
    internal_proxy: InternalProxyEndpoint,
    token: String,
    control: InternalProxyControl,
) -> Result<()> {
    let targets = resolve_listen_targets(listen)?;
    run_listener_tasks(runtime, targets, Some((internal_proxy, token, control))).await
}

async fn run_listener_tasks(
    runtime: Arc<ServerRuntime>,
    targets: Vec<ListenTarget>,
    internal_proxy: Option<(InternalProxyEndpoint, String, InternalProxyControl)>,
) -> Result<()> {
    let mut tasks = JoinSet::new();
    for target in targets {
        let runtime = Arc::clone(&runtime);
        tasks.spawn(async move {
            match target {
                ListenTarget::Stdio => {
                    tracing::info!("stdio listener active on stdin/stdout");
                    run_stdio(runtime).await
                }
                ListenTarget::WebSocket { bind_address } => {
                    tracing::info!(bind_address = %bind_address, "websocket listener starting");
                    run_websocket(runtime, &bind_address).await
                }
            }
        });
    }

    if let Some((internal_proxy, token, control)) = internal_proxy {
        let runtime = Arc::clone(&runtime);
        tasks.spawn(
            async move { run_internal_proxy(runtime, internal_proxy, token, control).await },
        );
    }

    if let Some(result) = tasks.join_next().await {
        tasks.abort_all();
        result??;
    }
    Ok(())
}

/// Stdio uses **NDJSON** (newline-delimited JSON): one JSON-RPC message per line.
///
/// Outbound messages are serialized with `serde_json::to_vec`, which escapes
/// embedded newlines in string values as `\n`. The trailing `\n` written by the
/// stdout task is therefore a frame delimiter only, not part of the payload.
/// Clients must send the same framing: each request, notification, or response
/// must occupy exactly one line of valid JSON. Pretty-printed or otherwise
/// multi-line payloads are rejected at the transport layer.
async fn run_stdio(runtime: Arc<ServerRuntime>) -> Result<()> {
    let (transport, outbound_rx) = ConnectionTransport::new();
    let outbound_tx = transport.outbound_tx.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, transport.outbound_tx)
        .await;
    tracing::info!(connection_id, "stdio connection established");

    let outbound_writer = spawn_outbound_writer(connection_id, outbound_rx, OutboundSink::Stdio);

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        accept_incoming_client_message(
            Arc::clone(&runtime),
            connection_id,
            outbound_tx.clone(),
            Arc::clone(&transport.inbound_semaphore),
            &line,
            "stdio_notifications",
        )
        .await;
    }

    runtime.unregister_connection(connection_id).await;
    tracing::info!(connection_id, "stdio connection closed");
    outbound_writer.abort();
    Ok(())
}

async fn run_internal_proxy(
    runtime: Arc<ServerRuntime>,
    internal_proxy: InternalProxyEndpoint,
    token: String,
    control: InternalProxyControl,
) -> Result<()> {
    let InternalProxyEndpoint { listener, endpoint } = internal_proxy;
    tracing::info!(endpoint = %endpoint, "internal stdio proxy listener bound");
    loop {
        let (stream, remote_addr) = listener.accept().await?;
        let runtime = Arc::clone(&runtime);
        let token = token.clone();
        let control = control.clone();
        tokio::spawn(async move {
            tracing::info!(remote_addr = %remote_addr, "internal stdio proxy connected");
            if let Err(error) =
                handle_internal_proxy_connection(runtime, stream, &token, control).await
            {
                tracing::warn!(remote_addr = %remote_addr, error = %error, "internal stdio proxy closed with error");
            }
            tracing::info!(remote_addr = %remote_addr, "internal stdio proxy disconnected");
        });
    }
}

async fn handle_internal_proxy_connection(
    runtime: Arc<ServerRuntime>,
    stream: tokio::net::TcpStream,
    expected_token: &str,
    control: InternalProxyControl,
) -> Result<()> {
    let websocket = accept_async(stream).await?;
    let (mut writer, mut reader) = websocket.split();
    match reader.next().await {
        Some(Ok(Message::Text(token))) if token.as_str() == expected_token => {}
        Some(Ok(Message::Close(_))) | None => return Ok(()),
        Some(Ok(_)) => bail!("internal stdio proxy did not send auth token"),
        Some(Err(error)) => return Err(error.into()),
    }

    let first_value = loop {
        match reader.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = serde_json::from_str(&text)?;
                if let Some(request) = parse_internal_proxy_control_request(&value) {
                    let enabled_protocols = match &request.action {
                        InternalProxyControlAction::EnableProtocols(requested) => {
                            runtime.enable_protocols(requested).await
                        }
                        InternalProxyControlAction::Status
                        | InternalProxyControlAction::Shutdown => runtime.enabled_protocols().await,
                    };
                    let response = internal_proxy_control_response(&request, &enabled_protocols);
                    writer
                        .send(Message::Text(response.to_string().into()))
                        .await
                        .context("send internal proxy control response")?;
                    if matches!(request.action, InternalProxyControlAction::Shutdown) {
                        control.request_shutdown();
                    }
                    return Ok(());
                }
                break value;
            }
            Some(Ok(Message::Close(_))) | None => return Ok(()),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(error.into()),
        }
    };

    let (transport, outbound_rx) = ConnectionTransport::new();
    let outbound_tx = transport.outbound_tx.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::StdioProxy, transport.outbound_tx)
        .await;
    tracing::info!(connection_id, "internal stdio proxy connection established");

    let outbound_writer =
        spawn_outbound_writer(connection_id, outbound_rx, OutboundSink::WebSocket(writer));

    if let Some(response) = runtime
        .handle_incoming_with_actions(connection_id, first_value)
        .await
        && !send_incoming_response(
            &runtime,
            &outbound_tx,
            response,
            connection_id,
            "internal_proxy_notifications",
        )
        .await
    {
        runtime.unregister_connection(connection_id).await;
        tracing::info!(connection_id, "internal stdio proxy connection closed");
        outbound_writer.abort();
        return Ok(());
    }

    while let Some(frame) = reader.next().await {
        let frame = frame?;
        match frame {
            Message::Text(text) => {
                accept_incoming_client_message(
                    Arc::clone(&runtime),
                    connection_id,
                    outbound_tx.clone(),
                    Arc::clone(&transport.inbound_semaphore),
                    text.as_str(),
                    "internal_proxy_notifications",
                )
                .await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    runtime.unregister_connection(connection_id).await;
    tracing::info!(connection_id, "internal stdio proxy connection closed");
    outbound_writer.abort();
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InternalProxyControlAction {
    Status,
    Shutdown,
    EnableProtocols(ProtocolSet),
}

impl InternalProxyControlAction {
    fn response_status(&self) -> &'static str {
        match self {
            InternalProxyControlAction::Status => "running",
            InternalProxyControlAction::Shutdown => "shutting down",
            InternalProxyControlAction::EnableProtocols(_) => "running",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InternalProxyControlRequest {
    id: serde_json::Value,
    action: InternalProxyControlAction,
}

fn parse_internal_proxy_control_request(
    value: &serde_json::Value,
) -> Option<InternalProxyControlRequest> {
    let method = value.get("method")?.as_str()?;
    let action = match method {
        SERVER_CONTROL_STATUS_METHOD => InternalProxyControlAction::Status,
        SERVER_CONTROL_SHUTDOWN_METHOD => InternalProxyControlAction::Shutdown,
        SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD => {
            let protocols = serde_json::from_value::<Vec<ServerProtocol>>(
                value.pointer("/params/protocols")?.clone(),
            )
            .ok()?;
            InternalProxyControlAction::EnableProtocols(ProtocolSet::new(protocols).ok()?)
        }
        _ => return None,
    };
    Some(InternalProxyControlRequest {
        id: value.get("id").cloned().unwrap_or(serde_json::Value::Null),
        action,
    })
}

fn internal_proxy_control_response(
    request: &InternalProxyControlRequest,
    enabled_protocols: &ProtocolSet,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id.clone(),
        "result": {
            "status": request.action.response_status(),
            "enabledProtocols": enabled_protocols.names(),
        },
    })
}

async fn run_websocket(runtime: Arc<ServerRuntime>, bind_address: &str) -> Result<()> {
    let listener = TcpListener::bind(bind_address).await?;
    tracing::info!(bind_address = %bind_address, "websocket listener bound");
    loop {
        let (stream, remote_addr) = listener.accept().await?;
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            tracing::info!(remote_addr = %remote_addr, "websocket client connected");
            if let Err(error) = handle_websocket_connection(runtime, stream).await {
                tracing::warn!(remote_addr = %remote_addr, error = %error, "websocket connection closed with error");
            }
            tracing::info!(remote_addr = %remote_addr, "websocket client disconnected");
        });
    }
}

async fn handle_websocket_connection(
    runtime: Arc<ServerRuntime>,
    stream: tokio::net::TcpStream,
) -> Result<()> {
    let websocket = accept_async(stream).await?;
    let (writer, mut reader) = websocket.split();
    let (transport, outbound_rx) = ConnectionTransport::new();
    let outbound_tx = transport.outbound_tx.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::WebSocket, transport.outbound_tx)
        .await;
    tracing::info!(connection_id, "websocket connection established");

    let outbound_writer =
        spawn_outbound_writer(connection_id, outbound_rx, OutboundSink::WebSocket(writer));

    while let Some(frame) = reader.next().await {
        let frame = frame?;
        match frame {
            Message::Text(text) => {
                accept_incoming_client_message(
                    Arc::clone(&runtime),
                    connection_id,
                    outbound_tx.clone(),
                    Arc::clone(&transport.inbound_semaphore),
                    text.as_str(),
                    "websocket_notifications",
                )
                .await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    runtime.unregister_connection(connection_id).await;
    tracing::info!(connection_id, "websocket connection closed");
    outbound_writer.abort();
    Ok(())
}

/// Parses one inbound client payload (NDJSON line or WebSocket text frame).
///
/// Returns `None` when the payload is empty after trimming. Returns `Err` with
/// a JSON-RPC `ParseError` response when the payload is not valid JSON.
fn parse_incoming_client_payload(
    raw_payload: &str,
) -> Option<Result<serde_json::Value, serde_json::Value>> {
    let trimmed = raw_payload.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(match serde_json::from_str(trimmed) {
        Ok(value) => Ok(value),
        Err(error) => Err(acp_error_response(
            serde_json::Value::Null,
            AcpErrorCode::ParseError,
            format!("malformed client payload: {error}"),
        )),
    })
}

/// Validates a decoded client JSON-RPC 2.0 message before handler dispatch.
///
/// Returns `Ok(())` for well-formed requests, notifications, or responses to
/// server-initiated calls. Returns `Err(error_response)` when the payload is
/// structurally invalid; the caller should send that response and skip the
/// handler.
fn validate_incoming_client_message(value: &serde_json::Value) -> Result<(), serde_json::Value> {
    let Some(object) = value.as_object() else {
        return Err(acp_error_response(
            serde_json::Value::Null,
            AcpErrorCode::InvalidRequest,
            "client message must be a JSON object",
        ));
    };

    let request_id = object.get("id").cloned().unwrap_or(serde_json::Value::Null);

    match object.get("jsonrpc").and_then(serde_json::Value::as_str) {
        Some("2.0") => {}
        Some(_) => {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "jsonrpc must be \"2.0\"",
            ));
        }
        None => {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "jsonrpc field is required",
            ));
        }
    }

    let has_method = object.contains_key("method");
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");

    if has_result && has_error {
        return Err(acp_error_response(
            request_id,
            AcpErrorCode::InvalidRequest,
            "client message must not contain both result and error",
        ));
    }

    if !has_method && object.contains_key("id") && (has_result || has_error) {
        return Ok(());
    }

    if has_method {
        if has_result || has_error {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "client request must not contain result or error",
            ));
        }
        let Some(method) = object.get("method").and_then(serde_json::Value::as_str) else {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "method must be a non-empty string",
            ));
        };
        if method.is_empty() {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "method must be a non-empty string",
            ));
        }
        if let Some(params) = object.get("params")
            && !params.is_object()
            && !params.is_array()
            && !params.is_null()
        {
            return Err(acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "params must be an object, array, or null",
            ));
        }
        return Ok(());
    }

    Err(acp_error_response(
        request_id,
        AcpErrorCode::InvalidRequest,
        "client message must be a request, notification, or response",
    ))
}

/// Returns `true` when the payload is a client response to a server-initiated
/// JSON-RPC request (see the call site in [`accept_incoming_client_message`]).
fn is_client_response_message(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    !object.contains_key("method")
        && object.contains_key("id")
        && (object.contains_key("result") || object.contains_key("error"))
}

/// Parses, validates, and dispatches one inbound client payload without blocking
/// the transport read loop on handler work.
async fn accept_incoming_client_message(
    runtime: Arc<ServerRuntime>,
    connection_id: u64,
    outbound_tx: mpsc::Sender<OutboundFrame>,
    inbound_semaphore: Arc<Semaphore>,
    raw_payload: &str,
    queue: &'static str,
) {
    let Some(parsed) = parse_incoming_client_payload(raw_payload) else {
        return;
    };
    let value = match parsed {
        Ok(value) => value,
        Err(error_response) => {
            tracing::warn!(connection_id, "rejected malformed client payload");
            let outbound_tx = outbound_tx.clone();
            tokio::spawn(async move {
                enqueue_outbound(
                    &outbound_tx,
                    OutboundFrame::json_rpc_response(connection_id, error_response),
                    queue,
                )
                .await;
            });
            return;
        }
    };
    if let Err(error_response) = validate_incoming_client_message(&value) {
        tracing::warn!(connection_id, "rejected invalid client message");
        let outbound_tx = outbound_tx.clone();
        tokio::spawn(async move {
            enqueue_outbound(
                &outbound_tx,
                OutboundFrame::json_rpc_response(connection_id, error_response),
                queue,
            )
            .await;
        });
        return;
    }
    // The server may initiate JSON-RPC requests to the client (ACP client-side
    // tools such as fs/read, fs/write, permission prompts). Those replies arrive
    // as client responses (id + result/error, no method) and must be matched to
    // the pending server request instead of entering the normal inbound handler.
    if is_client_response_message(&value) {
        tokio::spawn(async move {
            runtime.resolve_client_response(connection_id, value).await;
        });
        return;
    }
    // Bound how many client requests/notifications may run concurrently on this
    // connection (see INBOUND_CONCURRENCY_LIMIT). The transport read loop awaits a
    // permit before spawning the next handler, so inbound work applies backpressure
    // instead of unbounded task growth. Client responses above skip this permit
    // because a handler may hold one while blocked on a server-initiated client call
    // (e.g. ACP fs/read); requiring a permit for the reply would deadlock.
    //
    // Concurrency risks to keep in mind:
    // - Handlers for the same connection run in parallel; ordering is not preserved
    //   beyond what the runtime/session actors enforce.
    // - If every permit is held by handlers waiting on the client, the read loop
    //   blocks here until one finishes. Responses already bypass the permit, but a
    //   pipelined client request queued ahead of those responses in the socket
    //   buffer can delay reading them (head-of-line blocking).
    // - Semaphore close drops the message silently (no JSON-RPC error).
    let Ok(permit) = inbound_semaphore.acquire_owned().await else {
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        if let Some(response) = runtime
            .handle_incoming_with_actions(connection_id, value)
            .await
        {
            send_incoming_response(&runtime, &outbound_tx, response, connection_id, queue).await;
        }
    });
}

async fn send_incoming_response(
    runtime: &Arc<ServerRuntime>,
    outbound_tx: &mpsc::Sender<OutboundFrame>,
    response: IncomingResponse,
    connection_id: u64,
    queue: &'static str,
) -> bool {
    let (response, post_response_actions) = response.into_parts();
    if !enqueue_outbound(
        outbound_tx,
        OutboundFrame::json_rpc_response(connection_id, response),
        queue,
    )
    .await
    {
        return false;
    }
    runtime
        .run_post_response_actions(post_response_actions)
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_WEBSOCKET_BIND_ADDRESS;
    use super::EventBroadcaster;
    use super::InternalProxyControlAction;
    use super::InternalProxyControlRequest;
    use super::ListenTarget;
    use super::internal_proxy_control_response;
    use super::is_client_response_message;
    use super::parse_incoming_client_payload;
    use super::parse_internal_proxy_control_request;
    use super::parse_listen_target;
    use super::resolve_listen_targets;
    use super::validate_incoming_client_message;
    use crate::AcpErrorCode;
    use crate::ProtocolSet;
    use crate::ServerProtocol;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[test]
    fn parse_stdio_target() {
        assert_eq!(
            parse_listen_target("stdio://").expect("stdio"),
            ListenTarget::Stdio
        );
    }

    #[test]
    fn parse_ws_target() {
        assert_eq!(
            parse_listen_target("ws://127.0.0.1:9000").expect("ws"),
            ListenTarget::WebSocket {
                bind_address: "127.0.0.1:9000".into(),
            }
        );
    }

    #[test]
    fn parse_ws_target_without_bind_address_uses_default() {
        assert_eq!(
            parse_listen_target("ws://").expect("ws"),
            ListenTarget::WebSocket {
                bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.into(),
            }
        );
    }

    #[test]
    fn resolve_empty_listener_list_defaults_to_stdio() {
        assert_eq!(
            resolve_listen_targets(&[]).expect("targets"),
            vec![
                ListenTarget::Stdio,
                ListenTarget::WebSocket {
                    bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.into(),
                },
            ]
        );
    }

    #[test]
    fn internal_proxy_control_request_parses_status_and_shutdown() {
        assert_eq!(
            [
                parse_internal_proxy_control_request(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": crate::singleton::SERVER_CONTROL_STATUS_METHOD,
                }))
                .expect("status request"),
                parse_internal_proxy_control_request(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": crate::singleton::SERVER_CONTROL_SHUTDOWN_METHOD,
                }))
                .expect("shutdown request"),
            ],
            [
                InternalProxyControlRequest {
                    id: serde_json::json!(1),
                    action: InternalProxyControlAction::Status,
                },
                InternalProxyControlRequest {
                    id: serde_json::json!(2),
                    action: InternalProxyControlAction::Shutdown,
                },
            ]
        );
    }

    #[test]
    fn internal_proxy_control_request_parses_protocol_enablement() {
        assert_eq!(
            parse_internal_proxy_control_request(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": crate::singleton::SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD,
                "params": { "protocols": ["acp"] },
            }))
            .expect("enable protocols request"),
            InternalProxyControlRequest {
                id: serde_json::json!(3),
                action: InternalProxyControlAction::EnableProtocols(ProtocolSet::only(
                    ServerProtocol::Acp,
                )),
            }
        );
    }

    #[test]
    fn internal_proxy_control_response_reports_action_status() {
        assert_eq!(
            internal_proxy_control_response(
                &InternalProxyControlRequest {
                    id: serde_json::json!(2),
                    action: InternalProxyControlAction::Shutdown,
                },
                &ProtocolSet::only(ServerProtocol::Native),
            ),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "status": "shutting down",
                    "enabledProtocols": ["native"],
                },
            })
        );
    }

    #[test]
    fn is_client_response_message_detects_server_reply_payloads() {
        assert!(is_client_response_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "ok": true },
        })));
        assert!(!is_client_response_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {},
        })));
    }

    #[tokio::test]
    async fn enqueue_outbound_backpressures_full_receiver() {
        use crate::runtime::OutboundFrame;
        use crate::runtime::enqueue_outbound;
        use std::time::Duration;

        let (tx, mut rx) = mpsc::channel(1);
        assert!(
            enqueue_outbound(
                &tx,
                OutboundFrame::json_rpc_response(1, serde_json::json!({ "first": true })),
                "test_outbound",
            )
            .await
        );
        let tx_for_task = tx.clone();
        let mut blocked_enqueue = tokio::spawn(async move {
            enqueue_outbound(
                &tx_for_task,
                OutboundFrame::json_rpc_response(1, serde_json::json!({ "second": true })),
                "test_outbound",
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut blocked_enqueue)
                .await
                .is_err()
        );
        rx.recv().await.expect("first frame");
        assert!(blocked_enqueue.await.expect("blocked enqueue"));
        rx.recv().await.expect("second frame");
    }

    #[test]
    fn parse_incoming_client_payload_skips_empty_and_parses_json() {
        assert_eq!(parse_incoming_client_payload(""), None);
        assert_eq!(parse_incoming_client_payload("   \n"), None);

        let parsed = parse_incoming_client_payload(r#"  {"jsonrpc":"2.0","id":1}  "#)
            .expect("non-empty payload")
            .expect("valid json");
        assert_eq!(parsed, serde_json::json!({ "jsonrpc": "2.0", "id": 1 }));
    }

    #[test]
    fn parse_incoming_client_payload_returns_parse_error_response() {
        let error_response = parse_incoming_client_payload("{not json}")
            .expect("non-empty payload")
            .expect_err("invalid json");
        assert_eq!(
            error_response["error"]["code"],
            AcpErrorCode::ParseError as i64
        );
        assert_eq!(error_response["id"], serde_json::Value::Null);
    }

    #[test]
    fn validate_accepts_client_request_notification_and_response() {
        assert!(
            validate_incoming_client_message(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {},
            }))
            .is_ok()
        );
        assert!(
            validate_incoming_client_message(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {},
            }))
            .is_ok()
        );
        assert!(
            validate_incoming_client_message(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 9,
                "result": { "ok": true },
            }))
            .is_ok()
        );
    }

    #[test]
    fn validate_rejects_malformed_client_messages() {
        let invalid_request =
            validate_incoming_client_message(&serde_json::json!([])).expect_err("array payload");
        assert_eq!(
            invalid_request["error"]["code"],
            AcpErrorCode::InvalidRequest as i64
        );

        let missing_jsonrpc = validate_incoming_client_message(&serde_json::json!({
            "id": 1,
            "method": "initialize",
        }))
        .expect_err("missing jsonrpc");
        assert_eq!(
            missing_jsonrpc["error"]["code"],
            AcpErrorCode::InvalidRequest as i64
        );

        let conflicting_fields = validate_incoming_client_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "result": {},
        }))
        .expect_err("request with result");
        assert_eq!(
            conflicting_fields["error"]["code"],
            AcpErrorCode::InvalidRequest as i64
        );
    }

    #[tokio::test]
    async fn event_broadcaster_backpressures_full_sender() {
        let broadcaster = Arc::new(EventBroadcaster::new());
        let (tx, mut rx) = mpsc::channel(/*buffer*/ 1);
        broadcaster.register(/*connection_id*/ 1, tx).await;

        assert_eq!(
            broadcaster
                .broadcast("session", serde_json::json!({ "event": "first" }))
                .await,
            1
        );
        let broadcaster_for_task = Arc::clone(&broadcaster);
        let mut blocked_broadcast = tokio::spawn(async move {
            broadcaster_for_task
                .broadcast("session", serde_json::json!({ "event": "second" }))
                .await
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut blocked_broadcast)
                .await
                .is_err()
        );
        assert_eq!(
            rx.recv().await.expect("first event"),
            serde_json::json!({ "event": "first" })
        );
        assert_eq!(blocked_broadcast.await.expect("blocked broadcast"), 2);
        assert_eq!(
            rx.recv().await.expect("second event"),
            serde_json::json!({ "event": "second" })
        );
    }
}
