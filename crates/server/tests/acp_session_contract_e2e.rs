#[path = "support/acp_session_setup.rs"]
mod acp_session_setup;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use acp_session_setup::STDIO_SERVER_STARTUP_TIMEOUT;
use acp_session_setup::assert_no_history_replay_before_response;
use acp_session_setup::assert_openai_request_has_mcp_tool;
use acp_session_setup::assert_openai_request_lacks_mcp_tool;
use acp_session_setup::assert_prompt_response;
use acp_session_setup::assert_prompt_updates_before_response;
use acp_session_setup::assert_replayed_history_before_response;
use acp_session_setup::build_test_mcp_server_binary;
use acp_session_setup::devo_command;
use acp_session_setup::mcp_stdio_server_config;
use acp_session_setup::read_stdio_json;
use acp_session_setup::read_stdio_json_collect_until;
use acp_session_setup::read_stdio_json_until;
use acp_session_setup::recv_provider_prompt_request;
use acp_session_setup::spawn_openai_chat_completions_server;
use acp_session_setup::write_acp_prompt;
use acp_session_setup::write_stdio_json;
use acp_session_setup::write_test_config;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as AsyncBufReader;

#[tokio::test]
async fn stdio_acp_load_and_resume_match_session_setup_contract() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;

    let cwd = home_dir.path().join("workspace");
    let additional_directory = home_dir.path().join("shared");
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&additional_directory)?;
    let cwd = cwd.to_string_lossy().into_owned();
    let additional_directory = additional_directory.to_string_lossy().into_owned();
    let mcp_server_binary = build_test_mcp_server_binary().await?;

    let mut command = devo_command()?;
    let mut child = command
        .arg("server")
        .arg("--protocols")
        .arg("acp")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", home_dir.path().join(".devo"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn devo child process in server mode")?;

    let mut stdin = child.stdin.take().context("capture child stdin")?;
    let stdout = child.stdout.take().context("capture child stdout")?;
    let stderr = child.stderr.take().context("capture child stderr")?;
    let mut stdout_reader = AsyncBufReader::new(stdout).lines();
    let mut stderr_reader = AsyncBufReader::new(stderr);

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "acp-session-setup-e2e",
                    "title": "ACP Session Setup E2E",
                    "version": "1.0.0"
                }
            }
        }),
    )
    .await?;

    let initialize_response = read_stdio_json(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(initialize_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(initialize_response["id"], serde_json::json!(0));
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["loadSession"],
        serde_json::json!(true)
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["resume"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["close"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"],
        serde_json::json!({})
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": []
            }
        }),
    )
    .await?;
    let session_new_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/new response",
        |value| value.get("id") == Some(&serde_json::json!(1)),
    )
    .await?;
    let session_id = session_new_response["result"]["sessionId"]
        .as_str()
        .context("session/new response included a sessionId")?
        .to_string();

    let initial_prompt = "create one replayable ACP history item";
    write_acp_prompt(&mut stdin, 2, &session_id, initial_prompt).await?;
    let initial_prompt_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "initial session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(2)),
    )
    .await?;
    let initial_prompt_response = initial_prompt_messages
        .last()
        .context("initial session/prompt produced a response")?;
    assert_prompt_response(initial_prompt_response, 2);
    assert_prompt_updates_before_response(&initial_prompt_messages, &session_id)?;
    let initial_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "initial provider prompt request",
        initial_prompt,
    )
    .await?;
    assert_openai_request_lacks_mcp_tool(&initial_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": [mcp_stdio_server_config("load-tools", &mcp_server_binary)?]
            }
        }),
    )
    .await?;
    let load_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/load response",
        |value| value.get("id") == Some(&serde_json::json!(3)),
    )
    .await?;
    let load_response = load_messages
        .last()
        .context("session/load produced a response")?;
    assert_eq!(load_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(load_response["id"], serde_json::json!(3));
    let _ = acp_config_option(&load_response["result"], "model")?;
    assert_replayed_history_before_response(&load_messages, &session_id)?;

    let load_prompt = "after load, declare load MCP tools";
    write_acp_prompt(&mut stdin, 4, &session_id, load_prompt).await?;
    let load_prompt_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "post-load session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(4)),
    )
    .await?;
    let load_prompt_response = load_prompt_messages
        .last()
        .context("post-load session/prompt produced a response")?;
    assert_prompt_response(load_prompt_response, 4);
    assert_prompt_updates_before_response(&load_prompt_messages, &session_id)?;
    let load_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "post-load provider prompt request",
        load_prompt,
    )
    .await?;
    assert_openai_request_has_mcp_tool(&load_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/resume",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": [mcp_stdio_server_config("resume-tools", &mcp_server_binary)?]
            }
        }),
    )
    .await?;
    let resume_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/resume response",
        |value| value.get("id") == Some(&serde_json::json!(5)),
    )
    .await?;
    let resume_response = resume_messages
        .last()
        .context("session/resume produced a response")?;
    assert_eq!(resume_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(resume_response["id"], serde_json::json!(5));
    assert!(resume_response["result"].is_object());
    assert_no_history_replay_before_response(&resume_messages)?;

    let resume_prompt = "after resume, declare resume MCP tools";
    write_acp_prompt(&mut stdin, 6, &session_id, resume_prompt).await?;
    let resume_prompt_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "post-resume session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(6)),
    )
    .await?;
    let resume_prompt_response = resume_prompt_messages
        .last()
        .context("post-resume session/prompt produced a response")?;
    assert_prompt_response(resume_prompt_response, 6);
    assert_prompt_updates_before_response(&resume_prompt_messages, &session_id)?;
    let resume_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "post-resume provider prompt request",
        resume_prompt,
    )
    .await?;
    assert_openai_request_has_mcp_tool(&resume_provider_request, "mcp__resume_tools__echo")?;
    assert_openai_request_lacks_mcp_tool(&resume_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "session/close",
            "params": {
                "sessionId": session_id
            }
        }),
    )
    .await?;
    let close_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/close response",
        |value| value.get("id") == Some(&serde_json::json!(7)),
    )
    .await?;
    assert_eq!(
        close_response,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {}
        })
    );

    drop(stdin);
    child.kill().await.ok();
    let _ = child.wait().await;
    Ok(())
}

#[tokio::test]
async fn stdio_acp_session_config_options_select_model_binding() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;

    let cwd = home_dir.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(cwd.join(".devo"))?;
    std::fs::write(
        cwd.join(".devo").join("config.toml"),
        r#"
[model.test-model]
display_name = "Test Model"
reasoning_capability = { levels = ["low", "medium", "high"] }
default_reasoning_effort = "medium"
base_instructions = "Test model instructions"

[model.alt-model]
display_name = "Alt Model"
base_instructions = "Alt model instructions"

[model.catalog-only-model]
display_name = "Catalog Only Model"
base_instructions = "Catalog-only model instructions"
"#,
    )?;
    let cwd = cwd.to_string_lossy().into_owned();

    let mut command = devo_command()?;
    let mut child = command
        .arg("server")
        .arg("--protocols")
        .arg("acp")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", home_dir.path().join(".devo"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn devo child process in server mode")?;

    let mut stdin = child.stdin.take().context("capture child stdin")?;
    let stdout = child.stdout.take().context("capture child stdout")?;
    let stderr = child.stderr.take().context("capture child stderr")?;
    let mut stdout_reader = AsyncBufReader::new(stdout).lines();
    let mut stderr_reader = AsyncBufReader::new(stderr);

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "acp-config-options-e2e",
                    "title": "ACP Config Options E2E",
                    "version": "1.0.0"
                }
            }
        }),
    )
    .await?;
    let initialize_response = read_stdio_json(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(initialize_response["jsonrpc"], serde_json::json!("2.0"));

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "mcpServers": []
            }
        }),
    )
    .await?;
    let session_new_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/new response",
        |value| value.get("id") == Some(&serde_json::json!(1)),
    )
    .await?;
    let session_id = session_new_response["result"]["sessionId"]
        .as_str()
        .context("session/new response included a sessionId")?
        .to_string();
    let model_option = acp_model_config_option(&session_new_response["result"])?;
    assert_eq!(model_option["name"], serde_json::json!("Model"));
    assert_eq!(model_option["category"], serde_json::json!("model"));
    assert_eq!(
        model_option["currentValue"],
        serde_json::json!("test-openai")
    );
    assert_model_config_option_values(model_option, &["alt-openai", "test-openai"])?;
    assert_config_option_lacks_value(model_option, "catalog-only-model")?;
    let reasoning_effort_option =
        acp_config_option(&session_new_response["result"], "thought_level")?;
    assert_eq!(
        reasoning_effort_option["name"],
        serde_json::json!("Reasoning Effort")
    );
    assert_eq!(
        reasoning_effort_option["category"],
        serde_json::json!("thought_level")
    );
    assert_eq!(
        reasoning_effort_option["currentValue"],
        serde_json::json!("medium")
    );
    assert_config_option_values(reasoning_effort_option, &["low", "medium", "high"])?;
    let mode_option = acp_config_option(&session_new_response["result"], "mode")?;
    assert_eq!(mode_option["name"], serde_json::json!("Session Mode"));
    assert_eq!(mode_option["category"], serde_json::json!("mode"));
    assert_eq!(
        mode_option["currentValue"],
        serde_json::json!("auto-review")
    );
    assert_config_option_values(mode_option, &["default", "auto-review", "full-access"])?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/set_config_option",
            "params": {
                "sessionId": session_id,
                "configId": "thought_level",
                "value": "high"
            }
        }),
    )
    .await?;
    let set_reasoning_effort_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/set_config_option reasoning effort response",
        |value| value.get("id") == Some(&serde_json::json!(2)),
    )
    .await?;
    let reasoning_effort_option =
        acp_config_option(&set_reasoning_effort_response["result"], "thought_level")?;
    assert_eq!(
        reasoning_effort_option["currentValue"],
        serde_json::json!("high")
    );
    let model_option = acp_model_config_option(&set_reasoning_effort_response["result"])?;
    assert_eq!(
        model_option["currentValue"],
        serde_json::json!("test-openai")
    );

    let reasoning_effort_prompt = "use the selected ACP reasoning effort";
    write_acp_prompt(&mut stdin, 3, &session_id, reasoning_effort_prompt).await?;
    let reasoning_effort_prompt_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/prompt response after reasoning effort update",
        |value| value.get("id") == Some(&serde_json::json!(3)),
    )
    .await?;
    let reasoning_effort_prompt_response = reasoning_effort_prompt_messages
        .last()
        .context("session/prompt after reasoning effort update produced a response")?;
    assert_prompt_response(reasoning_effort_prompt_response, 3);
    let provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "provider prompt request after reasoning effort option update",
        reasoning_effort_prompt,
    )
    .await?;
    assert_eq!(provider_request["model"], serde_json::json!("test-model"));
    assert_eq!(
        provider_request["reasoning_effort"],
        serde_json::json!("high")
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/set_config_option",
            "params": {
                "sessionId": session_id,
                "configId": "model",
                "value": "alt-openai"
            }
        }),
    )
    .await?;
    let set_config_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/set_config_option response",
        |value| value.get("id") == Some(&serde_json::json!(4)),
    )
    .await?;
    let model_option = acp_model_config_option(&set_config_response["result"])?;
    assert_eq!(
        model_option["currentValue"],
        serde_json::json!("alt-openai")
    );
    assert!(acp_config_option_optional(&set_config_response["result"], "thought_level").is_none());
    let mode_option = acp_config_option(&set_config_response["result"], "mode")?;
    assert_eq!(
        mode_option["currentValue"],
        serde_json::json!("auto-review")
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/set_config_option",
            "params": {
                "sessionId": session_id,
                "configId": "mode",
                "value": "full-access"
            }
        }),
    )
    .await?;
    let set_mode_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/set_config_option mode response",
        |value| value.get("id") == Some(&serde_json::json!(5)),
    )
    .await?;
    let mode_option = acp_config_option(&set_mode_response["result"], "mode")?;
    assert_eq!(
        mode_option["currentValue"],
        serde_json::json!("full-access")
    );
    let model_option = acp_model_config_option(&set_mode_response["result"])?;
    assert_eq!(
        model_option["currentValue"],
        serde_json::json!("alt-openai")
    );

    let prompt = "use the selected ACP model binding";
    write_acp_prompt(&mut stdin, 6, &session_id, prompt).await?;
    let prompt_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(6)),
    )
    .await?;
    let prompt_response = prompt_messages
        .last()
        .context("session/prompt produced a response")?;
    assert_prompt_response(prompt_response, 6);
    let provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "provider prompt request after config option update",
        prompt,
    )
    .await?;
    assert_eq!(provider_request["model"], serde_json::json!("alt-model"));
    assert_eq!(
        provider_request["web_search_options"],
        serde_json::json!({})
    );

    drop(stdin);
    child.kill().await.ok();
    let _ = child.wait().await;
    Ok(())
}

#[tokio::test]
async fn stdio_proxy_acp_prompt_streams_each_agent_chunk_once() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;

    let devo_home = home_dir.path().join(".devo");
    let cwd = home_dir.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let cwd = cwd.to_string_lossy().into_owned();

    let mut first_command = devo_command()?;
    let mut first_child = first_command
        .arg("server")
        .arg("--protocols")
        .arg("acp")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", &devo_home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn real devo server process")?;
    let mut first_stdin = first_child.stdin.take().context("capture first stdin")?;
    let first_stdout = first_child.stdout.take().context("capture first stdout")?;
    let first_stderr = first_child.stderr.take().context("capture first stderr")?;
    let mut first_stdout_reader = AsyncBufReader::new(first_stdout).lines();
    let mut first_stderr_reader = AsyncBufReader::new(first_stderr);

    write_stdio_json(
        &mut first_stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "acp-real-server-holder",
                    "title": "ACP Real Server Holder",
                    "version": "1.0.0"
                }
            }
        }),
    )
    .await?;
    let first_initialize_response = read_stdio_json(
        &mut first_child,
        &mut first_stdout_reader,
        &mut first_stderr_reader,
        "real server initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(
        first_initialize_response["jsonrpc"],
        serde_json::json!("2.0")
    );

    write_stdio_json(
        &mut first_stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "mcpServers": []
            }
        }),
    )
    .await?;
    let first_session_new_response = read_stdio_json_until(
        &mut first_child,
        &mut first_stdout_reader,
        &mut first_stderr_reader,
        "real server session/new response",
        |value| value.get("id") == Some(&serde_json::json!(1)),
    )
    .await?;
    let session_id = first_session_new_response["result"]["sessionId"]
        .as_str()
        .context("real server session/new response included a sessionId")?
        .to_string();

    let first_prompt = "create history before proxy load";
    write_acp_prompt(&mut first_stdin, 2, &session_id, first_prompt).await?;
    let first_prompt_messages = read_stdio_json_collect_until(
        &mut first_child,
        &mut first_stdout_reader,
        &mut first_stderr_reader,
        "real server session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(2)),
    )
    .await?;
    let first_prompt_response = first_prompt_messages
        .last()
        .context("real server session/prompt produced a response")?;
    assert_prompt_response(first_prompt_response, 2);
    assert_prompt_updates_before_response(&first_prompt_messages, &session_id)?;
    let _ = recv_provider_prompt_request(
        &mut provider.requests,
        "real server provider prompt request",
        first_prompt,
    )
    .await?;

    let mut proxy_command = devo_command()?;
    let mut proxy_child = proxy_command
        .arg("server")
        .arg("--protocols")
        .arg("acp")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", &devo_home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn proxy devo server process")?;
    let mut proxy_stdin = proxy_child.stdin.take().context("capture proxy stdin")?;
    let proxy_stdout = proxy_child.stdout.take().context("capture proxy stdout")?;
    let proxy_stderr = proxy_child.stderr.take().context("capture proxy stderr")?;
    let mut proxy_stdout_reader = AsyncBufReader::new(proxy_stdout).lines();
    let mut proxy_stderr_reader = AsyncBufReader::new(proxy_stderr);

    write_stdio_json(
        &mut proxy_stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "third-party-acp-proxy-client",
                    "title": "Third Party ACP Proxy Client",
                    "version": "1.0.0"
                }
            }
        }),
    )
    .await?;
    let proxy_initialize_response = read_stdio_json(
        &mut proxy_child,
        &mut proxy_stdout_reader,
        &mut proxy_stderr_reader,
        "proxy initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(
        proxy_initialize_response["jsonrpc"],
        serde_json::json!("2.0")
    );

    write_stdio_json(
        &mut proxy_stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "additionalDirectories": [],
                "mcpServers": []
            }
        }),
    )
    .await?;
    let session_load_messages = read_stdio_json_collect_until(
        &mut proxy_child,
        &mut proxy_stdout_reader,
        &mut proxy_stderr_reader,
        "proxy session/load response",
        |value| value.get("id") == Some(&serde_json::json!(4)),
    )
    .await?;
    let session_load_response = session_load_messages
        .last()
        .context("proxy session/load produced a response")?;
    assert_eq!(session_load_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(session_load_response["id"], serde_json::json!(4));
    let _ = acp_config_option(&session_load_response["result"], "model")?;
    assert_replayed_history_before_response(&session_load_messages, &session_id)?;

    let prompt = "stream one ACP proxy reply";
    write_acp_prompt(&mut proxy_stdin, 5, &session_id, prompt).await?;
    let prompt_messages = read_stdio_json_collect_until(
        &mut proxy_child,
        &mut proxy_stdout_reader,
        &mut proxy_stderr_reader,
        "proxy session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(5)),
    )
    .await?;
    let prompt_response = prompt_messages
        .last()
        .context("proxy session/prompt produced a response")?;
    assert_prompt_response(prompt_response, 5);
    let chunks = prompt_messages
        .iter()
        .filter_map(|message| {
            if message["method"] != serde_json::json!("session/update")
                || message["params"]["sessionId"].as_str() != Some(session_id.as_str())
                || message["params"]["update"]["sessionUpdate"].as_str()
                    != Some("agent_message_chunk")
            {
                return None;
            }
            message["params"]["update"]["content"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    assert_eq!(chunks, vec!["ACP compatibility response.".to_string()]);
    let _ = recv_provider_prompt_request(
        &mut provider.requests,
        "proxy provider prompt request",
        prompt,
    )
    .await?;

    drop(proxy_stdin);
    proxy_child.kill().await.ok();
    let _ = proxy_child.wait().await;
    drop(first_stdin);
    first_child.kill().await.ok();
    let _ = first_child.wait().await;
    Ok(())
}

fn acp_model_config_option(result: &serde_json::Value) -> Result<&serde_json::Value> {
    acp_config_option(result, "model")
}

fn acp_config_option<'a>(
    result: &'a serde_json::Value,
    config_id: &str,
) -> Result<&'a serde_json::Value> {
    acp_config_option_optional(result, config_id)
        .with_context(|| format!("ACP result included {config_id} config option"))
}

fn acp_config_option_optional<'a>(
    result: &'a serde_json::Value,
    config_id: &str,
) -> Option<&'a serde_json::Value> {
    result["configOptions"].as_array().and_then(|options| {
        options
            .iter()
            .find(|option| option.get("id").and_then(serde_json::Value::as_str) == Some(config_id))
    })
}

fn assert_model_config_option_values(
    model_option: &serde_json::Value,
    expected_values: &[&str],
) -> Result<()> {
    assert_config_option_values(model_option, expected_values)
}

fn assert_config_option_values(option: &serde_json::Value, expected_values: &[&str]) -> Result<()> {
    let values = option["options"]
        .as_array()
        .context("config option includes options")?
        .iter()
        .filter_map(|option| option.get("value").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    for expected_value in expected_values {
        anyhow::ensure!(
            values.contains(expected_value),
            "model config option values should contain {expected_value}: {values:?}"
        );
    }
    Ok(())
}

fn assert_config_option_lacks_value(
    option: &serde_json::Value,
    unexpected_value: &str,
) -> Result<()> {
    let values = option["options"]
        .as_array()
        .context("config option includes options")?
        .iter()
        .filter_map(|option| option.get("value").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !values.contains(&unexpected_value),
        "config option values should not contain {unexpected_value}: {values:?}"
    );
    Ok(())
}
