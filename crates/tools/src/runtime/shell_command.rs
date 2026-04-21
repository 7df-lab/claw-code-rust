use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::runtime::{
    RuntimeTool, ToolCapabilityTag, ToolDefinitionSpec, ToolExecuteError, ToolExecutionContext,
    ToolExecutionMode, ToolExecutionOutcome, ToolInputError, ToolName, ToolOutputMode,
    ToolProgressReporter,
};
use crate::shell_exec::{
    ShellExecRequest, default_max_output_tokens, default_timeout_ms, default_yield_time_ms,
    execute_shell_command, shell_command_description,
};

pub struct ShellCommandRuntimeTool;

#[async_trait]
impl RuntimeTool for ShellCommandRuntimeTool {
    fn definition(&self) -> ToolDefinitionSpec {
        ToolDefinitionSpec {
            name: ToolName("shell_command".into()),
            description: shell_command_description(),
            input_schema: shell_command_input_schema(),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
        }
    }

    async fn validate(&self, input: &serde_json::Value) -> Result<(), ToolInputError> {
        let command = input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolInputError::Invalid {
                message: "missing 'command' field".into(),
            })?;
        if command.trim().is_empty() {
            return Err(ToolInputError::Invalid {
                message: "command must not be empty".into(),
            });
        }

        if let Some(workdir) = input.get("workdir")
            && !workdir.is_string()
        {
            return Err(ToolInputError::Invalid {
                message: "workdir must be a string".into(),
            });
        }

        if let Some(timeout_ms) = input.get("timeout_ms")
            && timeout_ms.as_u64().is_none()
        {
            return Err(ToolInputError::Invalid {
                message: "timeout_ms must be an unsigned integer".into(),
            });
        }

        if let Some(login) = input.get("login")
            && !login.is_boolean()
        {
            return Err(ToolInputError::Invalid {
                message: "login must be a boolean".into(),
            });
        }

        Ok(())
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolExecutionContext,
        _reporter: Arc<dyn ToolProgressReporter>,
    ) -> Result<ToolExecutionOutcome, ToolExecuteError> {
        let command = input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolExecuteError::InvalidInput {
                message: "missing 'command' field".into(),
            })?;
        let workdir = input
            .get("workdir")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(default_timeout_ms());
        let login = input
            .get("login")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);

        let output = execute_shell_command(ShellExecRequest {
            command: command.to_string(),
            workdir,
            description: "shell command".into(),
            shell_override: None,
            tty: false,
            login,
            timeout_ms,
            yield_time_ms: default_yield_time_ms(),
            max_output_tokens: default_max_output_tokens(),
        })
        .await
        .map_err(|error| ToolExecuteError::ExecutionFailed {
            message: error.to_string(),
        })?;

        Ok(super::legacy::map_legacy_output(output))
    }
}

fn shell_command_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell script to execute in the user's default shell"
            },
            "workdir": {
                "type": "string",
                "description": "The working directory to execute the command in"
            },
            "timeout_ms": {
                "type": "integer",
                "description": "The timeout for the command in milliseconds"
            },
            "login": {
                "type": "boolean",
                "description": "Whether to run the shell with login shell semantics. Defaults to true."
            }
        },
        "required": ["command"]
    })
}

#[cfg(test)]
mod tests {
    use super::ShellCommandRuntimeTool;
    use crate::runtime::{RuntimeTool, ToolExecuteError, ToolExecutionContext, ToolExecutionMode};
    use crate::runtime::{ToolName, ToolPolicySnapshot, ToolRuntimeConfigSnapshot};
    use devo_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn definition_uses_shell_command_name_and_mutating_mode() {
        let definition = ShellCommandRuntimeTool.definition();
        assert_eq!(definition.name, ToolName("shell_command".into()));
        assert_eq!(definition.execution_mode, ToolExecutionMode::Mutating);
    }

    #[tokio::test]
    async fn validate_rejects_missing_command() {
        let error = ShellCommandRuntimeTool
            .validate(&serde_json::json!({}))
            .await
            .expect_err("missing command should fail");
        assert_eq!(
            error.to_string(),
            "invalid tool input: missing 'command' field"
        );
    }

    #[tokio::test]
    async fn execute_rejects_missing_command() {
        let error = ShellCommandRuntimeTool
            .execute(
                serde_json::json!({}),
                ToolExecutionContext {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    cwd: PathBuf::from("."),
                    permissions: Arc::new(RuleBasedPolicy::new(PermissionMode::AutoApprove)),
                    policy_snapshot: ToolPolicySnapshot::default(),
                    app_config: Arc::new(ToolRuntimeConfigSnapshot::default()),
                },
                Arc::new(crate::runtime::NullToolProgressReporter),
            )
            .await
            .expect_err("missing command should fail");
        assert_eq!(
            error,
            ToolExecuteError::InvalidInput {
                message: "missing 'command' field".into(),
            }
        );
    }
}
