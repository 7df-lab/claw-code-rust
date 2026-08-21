#[path = "support/acp_session_setup.rs"]
mod acp_session_setup;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use acp_session_setup::STDIO_SERVER_STARTUP_TIMEOUT;
use acp_session_setup::devo_command;
use acp_session_setup::read_stdio_json;
use acp_session_setup::read_stdio_json_until;
use acp_session_setup::write_stdio_json;
use acp_session_setup::write_test_config;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as AsyncBufReader;

#[tokio::test]
async fn stdio_model_preferences_read_returns_cold_start_options_without_creating_session()
-> Result<()> {
    let home_dir = TempDir::new()?;
    write_test_config(&home_dir, &["stdio://"], "http://127.0.0.1:1")?;

    let cwd = home_dir.path().join("workspace");
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

    let (mut child, mut stdin, mut stdout_reader, mut stderr_reader) =
        spawn_initialized_stdio_server(&home_dir).await?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "model/preferences/read",
            "params": {
                "cwd": cwd
            }
        }),
    )
    .await?;
    let config_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "model/preferences/read response",
        |value| value["id"] == serde_json::json!(1),
    )
    .await?;
    assert_eq!(config_response["error"], serde_json::Value::Null);

    let preferences = &config_response["result"]["preferences"];
    assert!(preferences["model"].is_string());
    assert!(
        preferences["availableModels"]
            .as_array()
            .is_some_and(|models| { models.iter().any(|model| model["value"] == "test-openai") })
    );
    assert!(
        preferences["availableEfforts"]
            .as_array()
            .is_some_and(|efforts| { efforts.iter().any(|effort| effort["value"] == "medium") })
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/list",
            "params": {
                "cwds": [cwd]
            }
        }),
    )
    .await?;
    let session_list_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "session/list response",
        |value| value["id"] == serde_json::json!(2),
    )
    .await?;
    assert_eq!(
        session_list_response["result"]["data"],
        serde_json::json!([])
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "model/preferences/read",
            "params": {
                "cwd": "relative"
            }
        }),
    )
    .await?;
    let response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "model/preferences/read relative cwd response",
        |value| value["id"] == serde_json::json!(3),
    )
    .await?;
    assert_eq!(
        response["error"]["code"],
        serde_json::json!("InvalidParams")
    );
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("model/preferences/read cwd must be an absolute path")
            })
    );

    drop(stdin);
    child.kill().await.ok();
    let _ = child.wait().await;
    Ok(())
}

async fn spawn_initialized_stdio_server(
    home_dir: &TempDir,
) -> Result<(
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::io::Lines<AsyncBufReader<tokio::process::ChildStdout>>,
    AsyncBufReader<tokio::process::ChildStderr>,
)> {
    let mut command = devo_command()?;
    let mut child = command
        .arg("server")
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
                    "_meta": { "devo": { "protocol": "native" } },
                "clientInfo": {
                    "name": "model-config-e2e",
                    "title": "Model Config E2E",
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
        "initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(initialize_response["error"], serde_json::Value::Null);

    Ok((child, stdin, stdout_reader, stderr_reader))
}
