use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::invocation::ToolContent;
use crate::registry_plan::shell_command_tool_spec;
use crate::shell_exec::{
    DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_TIMEOUT_MS, DEFAULT_YIELD_TIME_MS, ShellExecRequest,
    execute_shell_command,
};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

/// Tool adapter for `shell_command` (and the legacy `bash` alias).
///
/// Parses model input and runs the command locally via the shell executor.
/// The ToolSpec is built from the shared registry-plan schema so the plan and
/// handler stay aligned.
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

fn tool_result_content(content: ToolContent) -> ToolResultContent {
    match content {
        ToolContent::Text(text) => ToolResultContent::Text(text),
        ToolContent::Json(json) => ToolResultContent::Json(json),
        ToolContent::Mixed { text, json } => ToolResultContent::Mixed { text, json },
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
        _progress: Option<ToolProgressSender>,
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

        let output = execute_shell_command(
            ShellExecRequest {
                output_capture: ctx.output_store.as_ref().map(|store| {
                    store
                        .capture(&ctx.tool_call_id.0)
                        .map(|capture| std::sync::Arc::new(std::sync::Mutex::new(capture)))
                        .map_err(|error| error.to_string())
                }),
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
                sandbox_permission_overlay: crate::tools::sandbox_overlay_for_spawn(
                    ctx.sandbox_permission_overlay.as_ref(),
                ),
            },
            None,
            ctx.cancel_token.clone(),
        )
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        let display = output.display_content;
        let content = tool_result_content(output.content);
        let mut result = if output.is_error {
            let message = match &content {
                ToolResultContent::Text(text) => text.clone(),
                ToolResultContent::Mixed { text, json } => text
                    .clone()
                    .unwrap_or_else(|| json.as_ref().map(ToString::to_string).unwrap_or_default()),
                ToolResultContent::Json(json) => json.to_string(),
            };
            ToolResult::error(
                content,
                "Command failed",
                ToolCallError::ExecutionFailed(message),
            )
        } else {
            ToolResult::success(content, "Command executed")
        };
        result.display_content = display;
        Ok(result)
    }
}
