use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use devo_tools::ClientTextFileRead;

use super::file_change_metadata::file_mtime;
use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::invocation::ToolContent;
use crate::json_schema::JsonSchema;
use crate::read::{is_binary_file, missing_file_message, read_directory, read_file};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

const READ_DESCRIPTION: &str = include_str!("../read.txt");

pub struct ReadHandler {
    spec: ToolSpec,
}

impl Default for ReadHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "read".into(),
                description: READ_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "filePath".to_string(),
                            JsonSchema::string(Some(
                                "The absolute path to the file or directory to read",
                            )),
                        ),
                        (
                            "offset".to_string(),
                            JsonSchema::integer(Some(
                                "Line number to start reading from (1-indexed)",
                            )),
                        ),
                        (
                            "limit".to_string(),
                            JsonSchema::integer(Some("Maximum number of lines to read")),
                        ),
                    ]),
                    Some(vec!["filePath".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![crate::tool_spec::ToolCapabilityTag::ReadFiles],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ReadHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let mut filepath = input["filePath"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'filePath' field".into()))?
            .to_string();
        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);

        if let Some(offset) = offset
            && offset < 1
        {
            return Ok(ToolResult::error(
                ToolResultContent::Text("offset must be greater than or equal to 1".into()),
                "Invalid offset",
                ToolCallError::InvalidInput("offset must be >= 1".into()),
            ));
        }

        if let Some(limit) = limit
            && limit < 1
        {
            return Ok(ToolResult::error(
                ToolResultContent::Text("limit must be greater than or equal to 1".into()),
                "Invalid limit",
                ToolCallError::InvalidInput("limit must be >= 1".into()),
            ));
        }

        if !PathBuf::from(&filepath).is_absolute() {
            filepath = ctx
                .workspace_root
                .join(&filepath)
                .to_string_lossy()
                .to_string();
        }

        let path = PathBuf::from(&filepath);
        if path.is_dir() {
            let output = read_directory(&path, limit.unwrap_or(usize::MAX), offset.unwrap_or(1));
            let output = output.map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
            let display = output.display_content;
            let mut result =
                ToolResult::success(tool_result_content(output.content), "Directory read");
            result.display_content = display;
            return Ok(result);
        }

        if let Some(client_filesystem) = ctx.client_filesystem.clone() {
            match client_filesystem
                .read_text_file(
                    ctx.session_id.clone(),
                    path.clone(),
                    offset.map(|value| value as u64),
                    limit.map(|value| value as u64),
                    ctx.cancel_token.clone(),
                )
                .await?
            {
                ClientTextFileRead::Content(content) => {
                    if is_full_file_read(offset, limit) {
                        record_full_read_in_ledger(&ctx, &path, &content);
                    }
                    return Ok(client_text_file_result(
                        &path,
                        offset.unwrap_or(1),
                        &content,
                    ));
                }
                ClientTextFileRead::Unsupported => {}
            }
        }

        if !path.exists() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(missing_file_message(&filepath)),
                "File not found",
                ToolCallError::ExecutionFailed(format!("file not found: {filepath}")),
            ));
        }

        let is_bin =
            is_binary_file(&path).map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
        if is_bin {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!("Cannot read binary file: {}", path.display())),
                "Binary file",
                ToolCallError::ExecutionFailed("binary file".into()),
            ));
        }

        if is_full_file_read(offset, limit) {
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => record_full_read_in_ledger(&ctx, &path, &content),
                Err(error) => {
                    tracing::debug!(
                        path = %path.display(),
                        %error,
                        "failed to capture full file content for read ledger"
                    );
                }
            }
        }

        let output = read_file(&path, limit.unwrap_or(usize::MAX), offset.unwrap_or(1));
        let output = output.map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
        let display = output.display_content;
        let mut result = ToolResult::success(tool_result_content(output.content), "File read");
        result.display_content = display;
        Ok(result)
    }
}

fn tool_result_content(content: ToolContent) -> ToolResultContent {
    match content {
        ToolContent::Text(text) => ToolResultContent::Text(text),
        ToolContent::Json(json) => ToolResultContent::Json(json),
        ToolContent::Mixed { text, json } => ToolResultContent::Mixed { text, json },
    }
}

fn is_full_file_read(offset: Option<usize>, limit: Option<usize>) -> bool {
    matches!(offset, None | Some(1)) && limit.is_none()
}

fn record_full_read_in_ledger(ctx: &ToolContext, path: &Path, content: &str) {
    if let Some(ledger) = ctx.file_read_ledger.as_ref() {
        ledger.record_full_read(path, content, file_mtime(path));
    }
}

fn client_text_file_result(path: &Path, offset: usize, content: &str) -> ToolResult {
    let display_content = numbered_client_text_content(offset, content);
    let output = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n{display_content}\n</content>",
        path.display()
    );
    let mut result = ToolResult::success(ToolResultContent::Text(output), "File read");
    result.display_content = Some(display_content);
    result
}

fn numbered_client_text_content(offset: usize, content: &str) -> String {
    let mut display_content = String::new();
    for (index, line) in content.lines().enumerate() {
        let _ = writeln!(display_content, "{}: {}", offset + index, line);
    }
    if display_content.is_empty() {
        "(Client file is empty or requested range returned no lines)".to_string()
    } else {
        display_content.push_str("\n(Loaded from client filesystem)");
        display_content
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    use crate::contracts::{ToolAgentScope, ToolBudgets, ToolTerminalStatus};
    use crate::invocation::ToolCallId;

    use super::*;

    #[tokio::test]
    async fn handle_rejects_zero_limit() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("sample.txt"), "one\n").expect("write sample");

        let result = ReadHandler::new()
            .handle(
                ToolContext {
                    tool_call_id: ToolCallId("call-1".to_string()),
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    workspace_root: root.path().to_path_buf(),
                    budgets: ToolBudgets {
                        output_limit_bytes: 32_768,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: CancellationToken::new(),
                    agent_scope: ToolAgentScope::Parent,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    agent_coordinator: None,
                    client_filesystem: None,
                    file_read_ledger: None,
                    network_proxy: None,
                    network_no_proxy: None,
                    sandbox_profile: None,
                },
                serde_json::json!({
                    "filePath": "sample.txt",
                    "limit": 0
                }),
                None,
            )
            .await
            .expect("handler returns tool error result");

        assert_eq!(result.result_summary, "Invalid limit");
        match &result.content {
            ToolResultContent::Text(text) => {
                assert_eq!(text, "limit must be greater than or equal to 1");
            }
            content => panic!("expected text error content, got {content:?}"),
        }
        match &result.structured_status {
            ToolTerminalStatus::Failed(ToolCallError::InvalidInput(message)) => {
                assert_eq!(message, "limit must be >= 1");
            }
            status => panic!("expected invalid input status, got {status:?}"),
        }
    }
}
