use devo_protocol::approx_bytes_for_tokens;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgress;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::tools::ClientTerminalCreate;
use crate::tools::ClientTerminalCreateRequest;
use crate::tools::ClientTerminalEnv;
use crate::tools::ClientTerminalOutput;
use crate::tools::ClientTerminalRequest;

use super::shell_exec::preview;
use super::shell_exec::truncate_output;

pub(crate) struct ClientTerminalShellRequest {
    pub command: String,
    pub workdir: PathBuf,
    pub description: String,
    pub shell_override: Option<String>,
    pub login: bool,
    pub timeout_ms: u64,
    pub max_output_tokens: usize,
}

struct TerminalShellSpec {
    program: &'static str,
    args: &'static [&'static str],
}

pub(crate) async fn execute_with_client_terminal(
    ctx: &ToolContext,
    request: ClientTerminalShellRequest,
    progress: Option<ToolProgressSender>,
) -> Result<Option<ToolResult>, ToolCallError> {
    let Some(client_terminal) = ctx.client_terminal.clone() else {
        return Ok(None);
    };
    let (command, args) = terminal_command_parts(
        &request.command,
        request.shell_override.as_deref(),
        request.login,
    );
    let env = client_terminal_env();
    let create = client_terminal
        .clone()
        .create(
            ClientTerminalCreateRequest {
                session_id: ctx.session_id.clone(),
                command,
                args,
                env,
                cwd: Some(request.workdir.clone()),
                output_byte_limit: Some(approx_bytes_for_tokens(request.max_output_tokens)),
            },
            ctx.cancel_token.clone(),
        )
        .await?;
    let ClientTerminalCreate::Created { terminal_id } = create else {
        return Ok(None);
    };

    if let Some(progress) = progress {
        let _ = progress.send(ToolProgress::Terminal {
            terminal_id: terminal_id.clone(),
        });
    }

    let terminal_request = ClientTerminalRequest {
        session_id: ctx.session_id.clone(),
        terminal_id: terminal_id.clone(),
    };
    let wait = client_terminal
        .clone()
        .wait_for_exit(
            terminal_request.clone(),
            Duration::from_millis(request.timeout_ms),
            ctx.cancel_token.clone(),
        )
        .await;
    let wait_status = match wait {
        Ok(status) => status,
        Err(ToolCallError::TimedOut(seconds)) => {
            let cleanup_token = tokio_util::sync::CancellationToken::new();
            let _ = client_terminal
                .clone()
                .kill(terminal_request.clone(), cleanup_token.clone())
                .await;
            let output = terminal_output_snapshot(
                client_terminal.clone(),
                terminal_request.clone(),
                cleanup_token.clone(),
            )
            .await;
            let _ = client_terminal
                .release(terminal_request, cleanup_token)
                .await;
            return Ok(Some(terminal_error_result(
                &terminal_id,
                &request,
                output,
                "Command timed out",
                ToolCallError::TimedOut(seconds),
            )));
        }
        Err(ToolCallError::Cancelled) => {
            let cleanup_token = tokio_util::sync::CancellationToken::new();
            let _ = client_terminal
                .clone()
                .kill(terminal_request.clone(), cleanup_token.clone())
                .await;
            let _ = client_terminal
                .release(terminal_request, cleanup_token)
                .await;
            return Err(ToolCallError::Cancelled);
        }
        Err(error) => {
            let cleanup_token = tokio_util::sync::CancellationToken::new();
            let _ = client_terminal
                .clone()
                .kill(terminal_request.clone(), cleanup_token.clone())
                .await;
            let output = terminal_output_snapshot(
                client_terminal.clone(),
                terminal_request.clone(),
                cleanup_token.clone(),
            )
            .await;
            let _ = client_terminal
                .release(terminal_request, cleanup_token)
                .await;
            return Ok(Some(terminal_error_result(
                &terminal_id,
                &request,
                output,
                "Command failed",
                error,
            )));
        }
    };

    let output = terminal_output_snapshot(
        client_terminal.clone(),
        terminal_request.clone(),
        ctx.cancel_token.clone(),
    )
    .await?;
    let _ = client_terminal
        .release(terminal_request, ctx.cancel_token.clone())
        .await;

    let success = wait_status.exit_code == Some(0) && wait_status.signal.is_none();
    let metadata = terminal_result_metadata(&terminal_id, &request, Some(output.clone()));
    if success {
        Ok(Some(ToolResult::success(
            ToolResultContent::Json(metadata),
            "Command executed",
        )))
    } else {
        Ok(Some(ToolResult::error(
            ToolResultContent::Json(metadata),
            "Command failed",
            ToolCallError::ExecutionFailed(format_terminal_failure(&wait_status)),
        )))
    }
}

async fn terminal_output_snapshot(
    client_terminal: std::sync::Arc<dyn crate::tools::ClientTerminal>,
    request: ClientTerminalRequest,
    cancel_token: tokio_util::sync::CancellationToken,
) -> Result<ClientTerminalOutput, ToolCallError> {
    client_terminal.output(request, cancel_token).await
}

fn terminal_error_result(
    terminal_id: &str,
    request: &ClientTerminalShellRequest,
    output: Result<ClientTerminalOutput, ToolCallError>,
    summary: &str,
    error: ToolCallError,
) -> ToolResult {
    let metadata = terminal_result_metadata(terminal_id, request, output.ok());
    ToolResult::error(ToolResultContent::Json(metadata), summary, error)
}

fn terminal_result_metadata(
    terminal_id: &str,
    request: &ClientTerminalShellRequest,
    output: Option<ClientTerminalOutput>,
) -> serde_json::Value {
    let output_text = output
        .as_ref()
        .map(|output| truncate_output(&output.output, request.max_output_tokens))
        .unwrap_or_default();
    let truncated = output.as_ref().is_some_and(|output| output.truncated);
    let exit_status = output
        .as_ref()
        .and_then(|output| output.exit_status.as_ref())
        .map(|status| {
            json!({
                "exitCode": status.exit_code,
                "signal": status.signal,
            })
        });
    json!({
        "content": [
            {
                "type": "terminal",
                "terminalId": terminal_id,
            }
        ],
        "terminalId": terminal_id,
        "output": output_text,
        "truncated": truncated,
        "exitStatus": exit_status,
        "command": preview(&request.command),
        "description": request.description,
        "cwd": request.workdir,
    })
}

fn format_terminal_failure(status: &crate::tools::ClientTerminalExitStatus) -> String {
    match (&status.exit_code, &status.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "command exited without status".to_string(),
    }
}

fn terminal_command_parts(
    command: &str,
    shell_override: Option<&str>,
    login: bool,
) -> (String, Vec<String>) {
    let shell = resolve_terminal_shell(shell_override, login);
    let command_to_run = if cfg!(windows) && shell.program.eq_ignore_ascii_case("powershell") {
        let mut command_to_run = concat!(
            "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); ",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
            "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
            "[System.Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); "
        )
        .to_string();
        command_to_run.push_str(command);
        command_to_run
    } else {
        command.to_string()
    };
    let mut args = shell
        .args
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    args.push(command_to_run);
    (shell.program.to_string(), args)
}

fn resolve_terminal_shell(shell: Option<&str>, login: bool) -> TerminalShellSpec {
    let shell = shell.unwrap_or("");
    let normalized = shell.to_ascii_lowercase();

    if normalized.contains("powershell") || normalized == "pwsh" || normalized == "powershell" {
        return TerminalShellSpec {
            program: "powershell",
            args: &["-NoLogo", "-NoProfile", "-Command"],
        };
    }

    if normalized.ends_with("cmd") || normalized.ends_with("cmd.exe") || normalized == "cmd" {
        return TerminalShellSpec {
            program: "cmd",
            args: &["/C"],
        };
    }

    if normalized.contains("zsh") {
        return TerminalShellSpec {
            program: "zsh",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    if normalized.contains("bash") {
        return TerminalShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    platform_terminal_shell(login)
}

fn platform_terminal_shell(login: bool) -> TerminalShellSpec {
    if cfg!(windows) {
        TerminalShellSpec {
            program: "powershell",
            args: &["-NoProfile", "-Command"],
        }
    } else {
        TerminalShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        }
    }
}

fn client_terminal_env() -> Vec<ClientTerminalEnv> {
    if cfg!(windows) {
        vec![ClientTerminalEnv {
            name: "PYTHONUTF8".to_string(),
            value: "1".to_string(),
        }]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use pretty_assertions::assert_eq;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::contracts::ToolAgentScope;
    use crate::contracts::ToolBudgets;
    use crate::contracts::ToolTerminalStatus;
    use crate::invocation::ToolCallId;
    use crate::tools::ClientTerminal;

    #[derive(Clone)]
    struct FakeClientTerminal {
        create: ClientTerminalCreate,
        wait: Result<crate::tools::ClientTerminalExitStatus, ToolCallError>,
        output: Result<ClientTerminalOutput, ToolCallError>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ClientTerminal for FakeClientTerminal {
        async fn create(
            self: Arc<Self>,
            request: ClientTerminalCreateRequest,
            _cancel_token: CancellationToken,
        ) -> Result<ClientTerminalCreate, ToolCallError> {
            self.calls.lock().await.push(format!(
                "create:{}:{}",
                request.command,
                request.args.join(" ")
            ));
            Ok(self.create.clone())
        }

        async fn output(
            self: Arc<Self>,
            _request: ClientTerminalRequest,
            _cancel_token: CancellationToken,
        ) -> Result<ClientTerminalOutput, ToolCallError> {
            self.calls.lock().await.push("output".to_string());
            self.output.clone()
        }

        async fn wait_for_exit(
            self: Arc<Self>,
            _request: ClientTerminalRequest,
            timeout: Duration,
            _cancel_token: CancellationToken,
        ) -> Result<crate::tools::ClientTerminalExitStatus, ToolCallError> {
            self.calls
                .lock()
                .await
                .push(format!("wait:{}", timeout.as_millis()));
            self.wait.clone()
        }

        async fn kill(
            self: Arc<Self>,
            _request: ClientTerminalRequest,
            _cancel_token: CancellationToken,
        ) -> Result<(), ToolCallError> {
            self.calls.lock().await.push("kill".to_string());
            Ok(())
        }

        async fn release(
            self: Arc<Self>,
            _request: ClientTerminalRequest,
            _cancel_token: CancellationToken,
        ) -> Result<(), ToolCallError> {
            self.calls.lock().await.push("release".to_string());
            Ok(())
        }
    }

    fn fake_terminal(
        create: ClientTerminalCreate,
        wait: Result<crate::tools::ClientTerminalExitStatus, ToolCallError>,
        output: Result<ClientTerminalOutput, ToolCallError>,
    ) -> Arc<FakeClientTerminal> {
        Arc::new(FakeClientTerminal {
            create,
            wait,
            output,
            calls: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn context(client_terminal: Arc<dyn ClientTerminal>) -> ToolContext {
        ToolContext {
            tool_call_id: ToolCallId("call-1".to_string()),
            session_id: devo_protocol::SessionId::new().to_string(),
            turn_id: None,
            workspace_root: std::env::current_dir().expect("current dir"),
            budgets: ToolBudgets {
                output_limit_bytes: 1024,
                wall_time_limit_ms: None,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            client_terminal: Some(client_terminal),
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_profile: None,
        }
    }

    fn request() -> ClientTerminalShellRequest {
        ClientTerminalShellRequest {
            command: "echo hi".to_string(),
            workdir: std::env::current_dir().expect("current dir"),
            description: "test command".to_string(),
            shell_override: Some("bash".to_string()),
            login: false,
            timeout_ms: 1234,
            max_output_tokens: 128,
        }
    }

    fn ok_status() -> crate::tools::ClientTerminalExitStatus {
        crate::tools::ClientTerminalExitStatus {
            exit_code: Some(0),
            signal: None,
        }
    }

    fn output(exit_code: Option<i32>) -> ClientTerminalOutput {
        ClientTerminalOutput {
            output: "hello\n".to_string(),
            truncated: false,
            exit_status: Some(crate::tools::ClientTerminalExitStatus {
                exit_code,
                signal: None,
            }),
        }
    }

    fn result_json(result: ToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(json) => json,
            content => panic!("expected json result, got {content:?}"),
        }
    }

    #[test]
    fn terminal_metadata_preserves_empty_output() {
        let request = request();
        let metadata = terminal_result_metadata(
            "term_1",
            &request,
            Some(ClientTerminalOutput {
                output: String::new(),
                truncated: false,
                exit_status: Some(ok_status()),
            }),
        );

        assert_eq!(
            metadata,
            serde_json::json!({
                "content": [{
                    "type": "terminal",
                    "terminalId": "term_1"
                }],
                "terminalId": "term_1",
                "output": "",
                "truncated": false,
                "exitStatus": {
                    "exitCode": 0,
                    "signal": null
                },
                "command": "echo hi",
                "description": "test command",
                "cwd": request.workdir
            })
        );
    }

    #[tokio::test]
    async fn client_terminal_success_returns_terminal_content_and_releases() {
        let terminal = fake_terminal(
            ClientTerminalCreate::Created {
                terminal_id: "term_1".to_string(),
            },
            Ok(ok_status()),
            Ok(output(Some(0))),
        );
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();

        let result =
            execute_with_client_terminal(&context(terminal.clone()), request(), Some(progress_tx))
                .await
                .expect("client terminal succeeds")
                .expect("client terminal used");

        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Completed
        ));
        assert_eq!(
            progress_rx.recv().await,
            Some(ToolProgress::Terminal {
                terminal_id: "term_1".to_string()
            })
        );
        let json = result_json(result);
        assert_eq!(json["content"][0]["type"], "terminal");
        assert_eq!(json["content"][0]["terminalId"], "term_1");
        assert_eq!(json["output"], "hello\n");
        assert_eq!(
            terminal.calls.lock().await.as_slice(),
            &[
                "create:bash:-c echo hi".to_string(),
                "wait:1234".to_string(),
                "output".to_string(),
                "release".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn client_terminal_unsupported_returns_none_for_local_fallback() {
        let terminal = fake_terminal(
            ClientTerminalCreate::Unsupported,
            Ok(ok_status()),
            Ok(output(Some(0))),
        );

        let result = execute_with_client_terminal(&context(terminal), request(), None)
            .await
            .expect("unsupported is not an error");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn client_terminal_nonzero_exit_returns_failed_terminal_result() {
        let terminal = fake_terminal(
            ClientTerminalCreate::Created {
                terminal_id: "term_1".to_string(),
            },
            Ok(crate::tools::ClientTerminalExitStatus {
                exit_code: Some(7),
                signal: None,
            }),
            Ok(output(Some(7))),
        );

        let result = execute_with_client_terminal(&context(terminal), request(), None)
            .await
            .expect("client terminal completes")
            .expect("client terminal used");

        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Failed(ToolCallError::ExecutionFailed(_))
        ));
        let json = result_json(result);
        assert_eq!(json["content"][0]["terminalId"], "term_1");
        assert_eq!(json["exitStatus"]["exitCode"], 7);
    }

    #[tokio::test]
    async fn client_terminal_timeout_kills_and_releases_terminal() {
        let terminal = fake_terminal(
            ClientTerminalCreate::Created {
                terminal_id: "term_1".to_string(),
            },
            Err(ToolCallError::TimedOut(1)),
            Ok(output(None)),
        );

        let result = execute_with_client_terminal(&context(terminal.clone()), request(), None)
            .await
            .expect("timeout becomes failed tool result")
            .expect("client terminal used");

        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Failed(ToolCallError::TimedOut(1))
        ));
        assert_eq!(
            terminal.calls.lock().await.as_slice(),
            &[
                "create:bash:-c echo hi".to_string(),
                "wait:1234".to_string(),
                "kill".to_string(),
                "output".to_string(),
                "release".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn client_terminal_cancel_kills_and_releases_terminal() {
        let terminal = fake_terminal(
            ClientTerminalCreate::Created {
                terminal_id: "term_1".to_string(),
            },
            Err(ToolCallError::Cancelled),
            Ok(output(None)),
        );

        let error = execute_with_client_terminal(&context(terminal.clone()), request(), None)
            .await
            .expect_err("cancel remains a tool cancellation");

        assert!(matches!(error, ToolCallError::Cancelled));
        assert_eq!(
            terminal.calls.lock().await.as_slice(),
            &[
                "create:bash:-c echo hi".to_string(),
                "wait:1234".to_string(),
                "kill".to_string(),
                "release".to_string(),
            ]
        );
    }
}
