use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::registry_plan::shell_command_tool_spec;
use crate::shell_exec::{
    DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_TIMEOUT_MS, DEFAULT_YIELD_TIME_MS, ShellExecRequest,
    execute_shell_command,
};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;
use crate::tools::client_terminal_shell::{
    ClientTerminalShellRequest, execute_with_client_terminal,
};

/// Tool adapter for `shell_command` (and the legacy `bash` alias).
///
/// Parses model input and delegates process execution to [`execute_shell_command`]
/// or the client terminal when available. The ToolSpec comes from
/// [`shell_command_tool_spec`] so the registry plan and handler share one schema.
pub struct ShellCommandHandler {
    spec: ToolSpec,
}

impl Default for ShellCommandHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellCommandHandler {
    pub fn new() -> Self {
        Self {
            spec: shell_command_tool_spec("shell_command"),
        }
    }
}

#[async_trait]
impl ToolHandler for ShellCommandHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let command = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'command' field".into()))?;

        let timeout_ms = input["timeout"]
            .as_u64()
            .or_else(|| input["timeout_ms"].as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let workdir = input["workdir"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ctx.workspace_root.clone());
        let description = input["description"]
            .as_str()
            .unwrap_or("shell command")
            .to_string();
        let shell_override = input["shell"].as_str().map(ToOwned::to_owned);
        let tty = input["tty"].as_bool().unwrap_or(false);
        let login = input["login"].as_bool().unwrap_or(true);
        let yield_time_ms = input["yield_time_ms"]
            .as_u64()
            .unwrap_or(DEFAULT_YIELD_TIME_MS);
        let max_output_tokens = input["max_output_tokens"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
        let terminal_workdir = if workdir.is_absolute() {
            workdir.clone()
        } else {
            ctx.workspace_root.join(&workdir)
        };

        if let Some(result) = execute_with_client_terminal(
            &ctx,
            ClientTerminalShellRequest {
                command: command.to_string(),
                workdir: terminal_workdir,
                description: description.clone(),
                shell_override: shell_override.clone(),
                login,
                timeout_ms,
                max_output_tokens,
            },
            progress,
        )
        .await?
        {
            return Ok(result);
        }

        let output = execute_shell_command(
            ShellExecRequest {
                command: command.to_string(),
                workdir,
                description,
                shell_override,
                tty,
                login,
                timeout_ms,
                yield_time_ms,
                max_output_tokens,
                sandbox_profile: ctx.sandbox_profile.clone(),
            },
            None,
            ctx.cancel_token.clone(),
        )
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        let display = output.display_content;
        let text = output.content.into_string();
        let mut result = if output.is_error {
            ToolResult::error(
                ToolResultContent::Text(text.clone()),
                "Command failed",
                ToolCallError::ExecutionFailed(text),
            )
        } else {
            ToolResult::success(ToolResultContent::Text(text), "Command executed")
        };
        result.display_content = display;
        Ok(result)
    }
}
