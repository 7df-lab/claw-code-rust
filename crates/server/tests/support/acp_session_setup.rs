#![allow(dead_code)]

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader as AsyncBufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub(crate) const STDIO_SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const STDIO_SERVER_LINE_TIMEOUT: Duration = Duration::from_secs(30);
const PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn write_test_config(
    home_dir: &TempDir,
    listen: &[&str],
    openai_base_url: &str,
) -> Result<()> {
    let config_dir = home_dir.path().join(".devo");
    std::fs::create_dir_all(&config_dir)?;
    let listen_entries = listen
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let openai_base_url = toml_string(openai_base_url);
    let config = format!(
        "[server]\nlisten = [{listen_entries}]\nmax_connections = 32\nevent_buffer_size = 128\nidle_session_timeout_secs = 300\npersist_ephemeral_sessions = false\n\n[defaults]\nmodel_binding = \"test-openai\"\n\n[providers.openai]\nenabled = true\nname = \"OpenAI\"\nbase_url = \"{openai_base_url}\"\nwire_apis = [\"openai_chat_completions\"]\n\n[model_bindings.alt-openai]\nenabled = true\nmodel_slug = \"alt-model\"\nprovider = \"openai\"\nmodel_name = \"alt-model\"\ninvocation_method = \"openai_chat_completions\"\n\n[model_bindings.test-openai]\nenabled = true\nmodel_slug = \"test-model\"\nprovider = \"openai\"\nmodel_name = \"test-model\"\ninvocation_method = \"openai_chat_completions\"\n"
    );
    std::fs::write(config_dir.join("config.toml"), config)?;
    Ok(())
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) async fn write_acp_prompt(
    stdin: &mut tokio::process::ChildStdin,
    id: i64,
    session_id: &str,
    text: &str,
) -> Result<()> {
    write_stdio_json(
        stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": text
                    }
                ]
            }
        }),
    )
    .await
}

pub(crate) fn assert_prompt_response(response: &Value, id: i64) {
    assert_eq!(
        response,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "stopReason": "end_turn"
            }
        })
    );
}

pub(crate) fn assert_prompt_updates_before_response(
    messages: &[Value],
    session_id: &str,
) -> Result<()> {
    let (_, before_response) = messages
        .split_last()
        .context("session/prompt produced at least one message")?;
    let saw_agent_message = before_response.iter().any(|message| {
        message["method"] == serde_json::json!("session/update")
            && message["params"]["sessionId"].as_str() == Some(session_id)
            && message["params"]["update"]["sessionUpdate"].as_str() == Some("agent_message_chunk")
    });
    anyhow::ensure!(
        saw_agent_message,
        "session/prompt did not emit an agent_message_chunk before responding: {messages:?}"
    );
    Ok(())
}

pub(crate) fn assert_replayed_history_before_response(
    messages: &[Value],
    session_id: &str,
) -> Result<()> {
    let (_, before_response) = messages
        .split_last()
        .context("session/load produced at least one message")?;
    let mut user_message_id = None;
    let mut agent_message_id = None;
    for message in before_response {
        if message["method"] != serde_json::json!("session/update")
            || message["params"]["sessionId"].as_str() != Some(session_id)
        {
            continue;
        }
        match message["params"]["update"]["sessionUpdate"].as_str() {
            Some("user_message_chunk") => {
                user_message_id = message["params"]["update"]["messageId"]
                    .as_str()
                    .map(ToOwned::to_owned);
            }
            Some("agent_message_chunk") => {
                agent_message_id = message["params"]["update"]["messageId"]
                    .as_str()
                    .map(ToOwned::to_owned);
            }
            _ => {}
        }
    }
    anyhow::ensure!(
        user_message_id.is_some(),
        "session/load did not replay a user message with messageId before responding: {messages:?}"
    );
    anyhow::ensure!(
        agent_message_id.is_some(),
        "session/load did not replay an agent message with messageId before responding: {messages:?}"
    );
    Ok(())
}

pub(crate) fn assert_no_history_replay_before_response(messages: &[Value]) -> Result<()> {
    let (_, before_response) = messages
        .split_last()
        .context("session/resume produced at least one message")?;
    let replayed_update = before_response.iter().find(|message| {
        message["method"] == serde_json::json!("session/update")
            && message["params"]["update"]["sessionUpdate"].as_str()
                != Some("available_commands_update")
    });
    anyhow::ensure!(
        replayed_update.is_none(),
        "session/resume replayed history before responding: {messages:?}"
    );
    Ok(())
}

pub(crate) fn mcp_stdio_server_config(name: &str, command: &Path) -> Result<Value> {
    anyhow::ensure!(
        command.is_absolute(),
        "MCP stdio command must be absolute for ACP compatibility proof: {}",
        command.display()
    );
    Ok(serde_json::json!({
        "name": name,
        "command": command.to_string_lossy(),
        "args": [],
        "env": []
    }))
}

pub(crate) fn assert_openai_request_has_mcp_tool(request: &Value, tool_name: &str) -> Result<()> {
    anyhow::ensure!(
        openai_request_has_tool(request, tool_name),
        "expected OpenAI request to include tool `{tool_name}`: {request}"
    );
    Ok(())
}

pub(crate) fn assert_openai_request_lacks_mcp_tool(request: &Value, tool_name: &str) -> Result<()> {
    anyhow::ensure!(
        !openai_request_has_tool(request, tool_name),
        "expected OpenAI request not to include tool `{tool_name}`: {request}"
    );
    Ok(())
}

fn openai_request_has_tool(request: &Value, tool_name: &str) -> bool {
    request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.pointer("/function/name").and_then(Value::as_str) == Some(tool_name)
            })
        })
}

pub(crate) async fn recv_provider_prompt_request(
    requests: &mut mpsc::Receiver<Value>,
    context: &str,
    prompt: &str,
) -> Result<Value> {
    timeout(PROVIDER_REQUEST_TIMEOUT, async {
        loop {
            let request = requests
                .recv()
                .await
                .with_context(|| format!("{context} channel closed"))?;
            if openai_request_contains_turn_prompt(&request, prompt) {
                return Ok(request);
            }
        }
    })
    .await
    .with_context(|| format!("timed out waiting for {context}"))?
}

fn openai_request_contains_turn_prompt(request: &Value, prompt: &str) -> bool {
    if request
        .to_string()
        .contains("Generate a short session title")
    {
        return false;
    }
    request
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("user")
                    && message
                        .get("content")
                        .and_then(Value::as_str)
                        .is_some_and(|content| content.contains(prompt))
            })
        })
}

pub(crate) struct CapturingOpenAiServer {
    pub(crate) base_url: String,
    pub(crate) requests: mpsc::Receiver<Value>,
}

pub(crate) async fn spawn_openai_chat_completions_server() -> Result<CapturingOpenAiServer> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind local OpenAI-compatible server")?;
    let addr = listener.local_addr().context("local OpenAI server addr")?;
    let (requests_tx, requests_rx) = mpsc::channel(16);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let requests_tx = requests_tx.clone();
            tokio::spawn(async move {
                let _ = handle_openai_connection(&mut socket, requests_tx).await;
            });
        }
    });

    Ok(CapturingOpenAiServer {
        base_url: format!("http://{addr}"),
        requests: requests_rx,
    })
}

async fn handle_openai_connection(
    socket: &mut TcpStream,
    requests_tx: mpsc::Sender<Value>,
) -> Result<()> {
    let request_body = read_http_request_body(socket)
        .await
        .context("read provider HTTP request body")?;
    let request: Value =
        serde_json::from_slice(&request_body).context("parse provider request JSON")?;
    requests_tx
        .send(request.clone())
        .await
        .context("record provider request")?;
    let body = if request.get("stream").and_then(Value::as_bool) == Some(true) {
        openai_streaming_response_body()
    } else {
        openai_completion_response_body()
    };
    let content_type = if request.get("stream").and_then(Value::as_bool) == Some(true) {
        "text/event-stream"
    } else {
        "application/json"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn read_http_request_body(socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = socket.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        let Some(header_end) = header_end(&bytes) else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = content_length(&headers)?;
        if bytes.len() >= header_end + content_length {
            return Ok(bytes[header_end..header_end + content_length].to_vec());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "HTTP request ended before headers and body were complete",
    ))
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn content_length(headers: &str) -> io::Result<usize> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid content-length header: {error}"),
                )
            });
        }
    }
    Ok(0)
}

fn openai_streaming_response_body() -> String {
    [
        sse_data(serde_json::json!({
            "id": "chatcmpl-acp-e2e",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "test-model",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": ""
                    },
                    "finish_reason": null
                }
            ]
        })),
        sse_data(serde_json::json!({
            "id": "chatcmpl-acp-e2e",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "test-model",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "content": "ACP compatibility response."
                    },
                    "finish_reason": null
                }
            ]
        })),
        sse_data(serde_json::json!({
            "id": "chatcmpl-acp-e2e",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "test-model",
            "choices": [
                {
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }
            ]
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat()
}

fn openai_completion_response_body() -> String {
    serde_json::json!({
        "id": "chatcmpl-acp-title-e2e",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "ACP compatibility proof"
                }
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2
        }
    })
    .to_string()
}

fn sse_data(value: Value) -> String {
    format!("data: {value}\n\n")
}

pub(crate) async fn build_test_mcp_server_binary() -> Result<PathBuf> {
    let workspace = workspace_root()?;
    let binary = target_debug_binary(&workspace, "test_stdio_server");
    if binary.is_file() {
        return Ok(binary);
    }

    let manifest = workspace.join("Cargo.toml");
    let cargo_path = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    let status = Command::new(cargo_path)
        .current_dir(&workspace)
        .arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("devo-rmcp-client")
        .arg("--bin")
        .arg("test_stdio_server")
        .status()
        .await
        .context("build MCP test stdio server")?;
    anyhow::ensure!(status.success(), "MCP test stdio server build failed");
    anyhow::ensure!(
        binary.is_file(),
        "MCP test stdio server binary was not built at {}",
        binary.display()
    );
    Ok(binary)
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .context("canonicalize workspace root")
}

fn target_debug_binary(workspace: &Path, name: &str) -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    let target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        workspace.join(target_dir)
    };
    let mut binary = target_dir.join("debug").join(name);
    if cfg!(windows) {
        binary.set_extension("exe");
    }
    binary
}

pub(crate) async fn write_stdio_json(
    stdin: &mut tokio::process::ChildStdin,
    value: serde_json::Value,
) -> Result<()> {
    stdin.write_all(format!("{value}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

pub(crate) async fn read_stdio_json_collect_until<F>(
    child: &mut tokio::process::Child,
    stdout_reader: &mut tokio::io::Lines<AsyncBufReader<tokio::process::ChildStdout>>,
    stderr_reader: &mut AsyncBufReader<tokio::process::ChildStderr>,
    context: &str,
    predicate: F,
) -> Result<Vec<Value>>
where
    F: Fn(&Value) -> bool,
{
    timeout(STDIO_SERVER_LINE_TIMEOUT, async {
        let mut values = Vec::new();
        loop {
            let value = read_stdio_json(
                child,
                stdout_reader,
                stderr_reader,
                context,
                STDIO_SERVER_LINE_TIMEOUT,
            )
            .await?;
            let done = predicate(&value);
            values.push(value);
            if done {
                return Ok(values);
            }
        }
    })
    .await
    .with_context(|| format!("timed out waiting for {context}"))?
}

pub(crate) async fn read_stdio_json_until<F>(
    child: &mut tokio::process::Child,
    stdout_reader: &mut tokio::io::Lines<AsyncBufReader<tokio::process::ChildStdout>>,
    stderr_reader: &mut AsyncBufReader<tokio::process::ChildStderr>,
    context: &str,
    predicate: F,
) -> Result<Value>
where
    F: Fn(&Value) -> bool,
{
    let mut values =
        read_stdio_json_collect_until(child, stdout_reader, stderr_reader, context, predicate)
            .await?;
    values
        .pop()
        .context("read_stdio_json_until collected a value")
}

pub(crate) async fn read_stdio_json(
    child: &mut tokio::process::Child,
    stdout_reader: &mut tokio::io::Lines<AsyncBufReader<tokio::process::ChildStdout>>,
    stderr_reader: &mut AsyncBufReader<tokio::process::ChildStderr>,
    context: &str,
    line_timeout: Duration,
) -> Result<Value> {
    let line = timeout(line_timeout, stdout_reader.next_line())
        .await
        .with_context(|| format!("timed out waiting for {context}"))?
        .with_context(|| format!("failed reading {context} from child stdout"))?
        .with_context(|| format!("{context} reached EOF before a line was produced"))?;
    parse_stdio_json_line(child, stderr_reader, context, &line).await
}

async fn parse_stdio_json_line(
    child: &mut tokio::process::Child,
    stderr_reader: &mut AsyncBufReader<tokio::process::ChildStderr>,
    context: &str,
    line: &str,
) -> Result<Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        let mut stderr_output = String::new();
        stderr_reader.read_to_string(&mut stderr_output).await?;
        let exit_status = child.try_wait()?;
        anyhow::bail!(
            "{context} was empty; child_exit_status={exit_status:?}; child_stderr={stderr_output:?}"
        );
    }

    serde_json::from_str(trimmed).with_context(|| {
        let exit_status = child.try_wait().ok().flatten();
        format!(
            "{context} was not valid JSON; raw_stdout_line={trimmed:?}; child_exit_status={exit_status:?}"
        )
    })
}

pub(crate) fn devo_command() -> Result<Command> {
    if let Some(binary_path) = std::env::var_os("CARGO_BIN_EXE_devo").map(PathBuf::from)
        && binary_path.is_file()
    {
        return Ok(Command::new(binary_path));
    }

    let binary_path = devo_binary_path()?;
    if binary_path.is_file() {
        return Ok(Command::new(binary_path));
    }

    let cargo_path = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    let mut command = Command::new(cargo_path);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("devo-cli")
        .arg("--bin")
        .arg("devo")
        .arg("--");
    Ok(command)
}

fn devo_binary_path() -> Result<PathBuf> {
    let target_root = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root()?.join("target"),
    };
    let mut binary = target_root.join("debug").join("devo");
    if cfg!(windows) {
        binary.set_extension("exe");
    }
    Ok(binary)
}
