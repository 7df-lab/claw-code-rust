use async_trait::async_trait;

use crate::apply_patch::exec_apply_patch;
use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::invocation::ToolContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;

pub struct ApplyPatchHandler {
    spec: ToolSpec,
}

impl Default for ApplyPatchHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyPatchHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "apply_patch".into(),
                description: "Apply a unified diff patch to the filesystem.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "patchText".to_string(),
                        JsonSchema::string(Some(
                            "The full patch text that describes all changes to be made",
                        )),
                    )]),
                    Some(vec!["patchText".to_string()]),
                    Some(false),
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::WriteFiles],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let output = exec_apply_patch(&ctx.workspace_root, input)
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        if output.is_error {
            let text = output.content.into_string();
            Ok(ToolResult::error(
                ToolResultContent::Text(text.clone()),
                "Patch failed",
                ToolCallError::ExecutionFailed(text),
            ))
        } else {
            let content = match output.content {
                ToolContent::Text(text) => ToolResultContent::Text(text),
                ToolContent::Json(json) => {
                    record_patch_files_in_ledger(&ctx, &json);
                    ToolResultContent::Json(json)
                }
                ToolContent::Mixed { text, json } => {
                    if let Some(json) = json.as_ref() {
                        record_patch_files_in_ledger(&ctx, json);
                    }
                    ToolResultContent::Mixed { text, json }
                }
            };
            let mut result = ToolResult::success(content, "Patch applied");
            result.display_content = output.display_content;
            Ok(result)
        }
    }
}

fn record_patch_files_in_ledger(ctx: &ToolContext, metadata: &serde_json::Value) {
    let Some(ledger) = ctx.file_read_ledger.as_ref() else {
        return;
    };
    let Some(files) = metadata.get("files").and_then(|value| value.as_array()) else {
        return;
    };
    for file in files {
        let path = file
            .get("filePath")
            .or_else(|| file.get("path"))
            .and_then(|value| value.as_str())
            .map(std::path::PathBuf::from);
        let Some(path) = path else {
            continue;
        };
        let absolute = if path.is_absolute() {
            path
        } else {
            ctx.workspace_root.join(path)
        };
        let kind = file
            .get("kind")
            .or_else(|| file.get("type"))
            .and_then(|value| value.as_str())
            .unwrap_or("update");
        if kind == "delete" {
            ledger.invalidate(&absolute);
            continue;
        }
        let Some(content) = file
            .get("postContent")
            .or_else(|| file.get("post_content"))
            .or_else(|| file.get("content"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let mtime = super::file_change_metadata::file_mtime(&absolute);
        ledger.record_write(&absolute, content, mtime);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn apply_patch_schema_matches_executor_input() {
        let handler = ApplyPatchHandler::new();
        let expected = JsonSchema::object(
            BTreeMap::from([(
                "patchText".to_string(),
                JsonSchema::string(Some(
                    "The full patch text that describes all changes to be made",
                )),
            )]),
            Some(vec!["patchText".to_string()]),
            Some(false),
        );

        assert_eq!(handler.spec().input_schema, expected);
    }

    #[tokio::test]
    async fn apply_patch_success_preserves_file_metadata() {
        let root = tempfile::tempdir().expect("tempdir");
        let handler = ApplyPatchHandler::new();

        let result = handler
            .handle(
                ToolContext {
                    tool_call_id: crate::invocation::ToolCallId("call-1".to_string()),
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    workspace_root: root.path().to_path_buf(),
                    budgets: crate::contracts::ToolBudgets {
                        output_limit_bytes: 1024,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    agent_scope: crate::contracts::ToolAgentScope::Parent,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    agent_coordinator: None,
                    client_filesystem: None,
                    file_read_ledger: None,
                    network_proxy: None,
                    network_no_proxy: None,
                    sandbox_profile: None,
                },
                serde_json::json!({
                    "patchText": "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch"
                }),
                None,
            )
            .await
            .expect("apply_patch succeeds");

        let ToolResultContent::Mixed {
            text: Some(text),
            json: Some(json),
        } = result.content
        else {
            panic!("expected mixed output");
        };
        assert!(text.contains("Success. Updated the following files:"));
        assert_eq!(json["files"][0]["kind"], "add");
        assert_eq!(json["files"][0]["content"], "hello\n");
    }
}
