//! Exact string-replacement edit tool (`edit`).

use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use devo_tools::ClientTextFileRead;
use devo_tools::ClientTextFileWrite;
use devo_tools::FileReadFreshnessError;
use tracing::info;

use super::file_change_metadata::{file_mtime, write_tool_result};
use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::read::is_binary_file;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

const EDIT_DESCRIPTION: &str = include_str!("../edit.txt");

pub struct EditHandler {
    spec: ToolSpec,
}

impl Default for EditHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EditHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "edit".into(),
                description: EDIT_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "filePath".to_string(),
                            JsonSchema::string(Some(
                                "The absolute path to the file to modify. Preferred field name; `path` and `file_path` are also accepted.",
                            )),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some("Alias for `filePath`.")),
                        ),
                        (
                            "file_path".to_string(),
                            JsonSchema::string(Some("Alias for `filePath`.")),
                        ),
                        (
                            "oldString".to_string(),
                            JsonSchema::string(Some(
                                "The exact text to replace. Must be non-empty and unique unless replaceAll is true. Preferred field name; `old_string` is also accepted.",
                            )),
                        ),
                        (
                            "old_string".to_string(),
                            JsonSchema::string(Some("Alias for `oldString`.")),
                        ),
                        (
                            "newString".to_string(),
                            JsonSchema::string(Some(
                                "The text to replace oldString with. May be empty to delete text. Preferred field name; `new_string` is also accepted.",
                            )),
                        ),
                        (
                            "new_string".to_string(),
                            JsonSchema::string(Some("Alias for `newString`.")),
                        ),
                        (
                            "replaceAll".to_string(),
                            JsonSchema::boolean(Some(
                                "Replace every occurrence of oldString. Defaults to false. Preferred field name; `replace_all` is also accepted.",
                            )),
                        ),
                        (
                            "replace_all".to_string(),
                            JsonSchema::boolean(Some("Alias for `replaceAll`.")),
                        ),
                    ]),
                    Some(vec![
                        "filePath".to_string(),
                        "oldString".to_string(),
                        "newString".to_string(),
                    ]),
                    Some(/*additional_properties*/ false),
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::WriteFiles],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::LiveOnly,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for EditHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let path_str = string_field(&input, &["filePath", "path", "file_path"])
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'filePath' field".into()))?;
        let old_string = string_field(&input, &["oldString", "old_string"])
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'oldString' field".into()))?;
        let new_string = string_field(&input, &["newString", "new_string"])
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'newString' field".into()))?;
        let replace_all = bool_field(&input, &["replaceAll", "replace_all"]).unwrap_or(false);

        if old_string.is_empty() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(
                    "oldString must be non-empty. Use the Write tool to create new files.".into(),
                ),
                "Invalid oldString",
                ToolCallError::InvalidInput("empty oldString".into()),
            ));
        }
        if old_string == new_string {
            return Ok(ToolResult::error(
                ToolResultContent::Text("oldString and newString must be different".into()),
                "No-op edit",
                ToolCallError::InvalidInput("oldString equals newString".into()),
            ));
        }

        let path = resolve_path(&ctx.workspace_root, path_str);
        info!(path = %path.display(), replace_all, "editing file");

        let previous = match read_text_file(&ctx, &path).await? {
            Some(content) => content,
            None => {
                return Ok(ToolResult::error(
                    ToolResultContent::Text(format!(
                        "File not found: {}. Use the Write tool to create new files.",
                        path.display()
                    )),
                    "File not found",
                    ToolCallError::ExecutionFailed(format!("file not found: {}", path.display())),
                ));
            }
        };

        if is_binary_file(&path).unwrap_or(false) {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!("Cannot edit binary file: {}", path.display())),
                "Binary file",
                ToolCallError::ExecutionFailed("binary file".into()),
            ));
        }

        if let Some(ledger) = ctx.file_read_ledger.as_ref() {
            match ledger.require_fresh(&path, &previous, file_mtime(&path)) {
                Ok(()) => {}
                Err(FileReadFreshnessError::NotRead) => {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text(format!(
                            "You must Read the full file before using edit on {}. Read the file without offset/limit, then retry with the exact oldString from that output.",
                            path.display()
                        )),
                        "Read required",
                        ToolCallError::ExecutionFailed(format!(
                            "must read file before editing: {}",
                            path.display()
                        )),
                    ));
                }
                Err(FileReadFreshnessError::Stale) => {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text(format!(
                            "The file {} changed since it was last read. Read the full file again, then retry with an updated oldString.",
                            path.display()
                        )),
                        "Stale read",
                        ToolCallError::ExecutionFailed(format!(
                            "file changed since it was last read: {}",
                            path.display()
                        )),
                    ));
                }
            }
        }

        let match_count = previous.matches(old_string).count();
        if match_count == 0 {
            return Ok(ToolResult::error(
                ToolResultContent::Text(old_string_not_found_message(old_string)),
                "No match",
                ToolCallError::ExecutionFailed(old_string_not_found_error(old_string)),
            ));
        }
        if match_count > 1 && !replace_all {
            return Ok(ToolResult::error(
                ToolResultContent::Text(
                    "Found multiple matches for oldString. Provide more surrounding lines in oldString to identify the correct match, or set replaceAll to true if every match should change.".into(),
                ),
                "Ambiguous match",
                ToolCallError::ExecutionFailed(
                    "found multiple matches for oldString".into(),
                ),
            ));
        }
        let content = if replace_all {
            previous.replace(old_string, new_string)
        } else {
            previous.replacen(old_string, new_string, 1)
        };

        write_text_file(&ctx, &path, &content).await?;

        let summary = if replace_all {
            format!(
                "edited {} (replaced {match_count} occurrence{})",
                path.display(),
                if match_count == 1 { "" } else { "s" }
            )
        } else {
            format!("edited {}", path.display())
        };
        Ok(write_tool_result(
            &path,
            Some(previous.as_str()),
            &content,
            summary,
        ))
    }
}

fn resolve_path(cwd: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}

fn string_field<'a>(input: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(serde_json::Value::as_str))
}

fn bool_field(input: &serde_json::Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(serde_json::Value::as_bool))
}

fn old_string_not_found_error(old_string: &str) -> String {
    if looks_like_numbered_read_line(old_string) {
        "oldString not found; it appears to include a Read tool line number prefix".into()
    } else {
        "oldString not found".into()
    }
}

fn old_string_not_found_message(old_string: &str) -> String {
    if looks_like_numbered_read_line(old_string) {
        "oldString not found in content. It looks like oldString includes a Read tool line number prefix such as `12: `. Remove the line number prefix and retry with only the actual file text.".into()
    } else {
        "oldString not found in content. Read the full file again and copy the exact text, including whitespace, tabs, and newlines. Do not include Read tool line number prefixes like `12: `.".into()
    }
}

fn looks_like_numbered_read_line(old_string: &str) -> bool {
    let digits = old_string
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digits > 0 && old_string[digits..].starts_with(": ")
}

async fn read_text_file(ctx: &ToolContext, path: &Path) -> Result<Option<String>, ToolCallError> {
    if let Some(client_filesystem) = ctx.client_filesystem.clone() {
        match client_filesystem
            .read_text_file(
                ctx.session_id.clone(),
                path.to_path_buf(),
                None,
                None,
                ctx.cancel_token.clone(),
            )
            .await
        {
            Ok(ClientTextFileRead::Content(content)) => return Ok(Some(content)),
            Ok(ClientTextFileRead::Unsupported) => {}
            Err(error) => {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "client filesystem read failed; falling back to local fs"
                );
            }
        }
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ToolCallError::ExecutionFailed(format!(
            "failed to read file: {error}"
        ))),
    }
}

async fn write_text_file(
    ctx: &ToolContext,
    path: &Path,
    content: &str,
) -> Result<(), ToolCallError> {
    if let Some(client_filesystem) = ctx.client_filesystem.clone() {
        match client_filesystem
            .write_text_file(
                ctx.session_id.clone(),
                path.to_path_buf(),
                content.to_string(),
                ctx.cancel_token.clone(),
            )
            .await?
        {
            ClientTextFileWrite::Written => {
                record_write_in_ledger(ctx, path, content);
                return Ok(());
            }
            ClientTextFileWrite::Unsupported => {}
        }
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ToolCallError::ExecutionFailed(format!("failed to create directories: {e}"))
        })?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(format!("failed to write file: {e}")))?;
    record_write_in_ledger(ctx, path, content);
    Ok(())
}

fn record_write_in_ledger(ctx: &ToolContext, path: &Path, content: &str) {
    if let Some(ledger) = ctx.file_read_ledger.as_ref() {
        ledger.record_write(path, content, file_mtime(path));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    use super::super::file_change_metadata::file_mtime;
    use super::*;
    use crate::contracts::{ToolAgentScope, ToolBudgets, ToolTerminalStatus};
    use crate::invocation::ToolCallId;
    use devo_tools::FileReadLedger;

    fn ctx(root: &Path, ledger: Arc<FileReadLedger>) -> ToolContext {
        ToolContext {
            output_store: None,
            tool_call_id: ToolCallId("call-1".to_string()),
            session_id: "session-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            workspace_root: root.to_path_buf(),
            budgets: ToolBudgets {
                output_limit_bytes: 32_768,
                wall_time_limit_ms: None,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: Some(ledger),
            network_proxy: None,
            network_no_proxy: None,
            sandbox_permission_overlay: None,
            sandbox_profile: None,
        }
    }

    #[tokio::test]
    async fn edit_rejects_empty_old_string() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "", file_mtime(&path));

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "",
                    "newString": "x",
                }),
                None,
            )
            .await
            .expect("handle");
        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Failed(_)
        ));
    }

    #[tokio::test]
    async fn consecutive_edits_without_reread_succeed() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "one two three").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "one two three", file_mtime(&path));

        EditHandler::new()
            .handle(
                ctx(root.path(), Arc::clone(&ledger)),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "one",
                    "newString": "1",
                }),
                None,
            )
            .await
            .expect("first edit");

        let second = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "two",
                    "newString": "2",
                }),
                None,
            )
            .await
            .expect("second edit");
        assert!(matches!(
            second.structured_status,
            ToolTerminalStatus::Completed
        ));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "1 2 three");
    }

    #[tokio::test]
    async fn edit_accepts_path_aliases() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "hello").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "hello", file_mtime(&path));

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "path": path,
                    "old_string": "hello",
                    "new_string": "world",
                }),
                None,
            )
            .await
            .expect("handle");

        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Completed
        ));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "world");
    }

    #[tokio::test]
    async fn edit_rejects_when_file_was_not_read_first() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "hello").expect("write");

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), Arc::new(FileReadLedger::new())),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "hello",
                    "newString": "world",
                }),
                None,
            )
            .await
            .expect("handle");

        match &result.structured_status {
            ToolTerminalStatus::Failed(ToolCallError::ExecutionFailed(message)) => {
                assert!(
                    message.contains("must read"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected failed execution status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_rejects_stale_read_content() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "hello").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "hello", file_mtime(&path));
        std::fs::write(&path, "hello there").expect("rewrite");

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "hello",
                    "newString": "world",
                }),
                None,
            )
            .await
            .expect("handle");

        match &result.structured_status {
            ToolTerminalStatus::Failed(ToolCallError::ExecutionFailed(message)) => {
                assert!(
                    message.contains("changed since it was last read"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected failed execution status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_rejects_ambiguous_old_string_without_replace_all() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "dup dup").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "dup dup", file_mtime(&path));

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "dup",
                    "newString": "value",
                }),
                None,
            )
            .await
            .expect("handle");

        match &result.structured_status {
            ToolTerminalStatus::Failed(ToolCallError::ExecutionFailed(message)) => {
                assert!(
                    message.contains("multiple matches"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected failed execution status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_old_string_not_found_message_mentions_line_numbers() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "alpha\nbeta\n").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "alpha\nbeta\n", file_mtime(&path));

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "1: alpha",
                    "newString": "gamma",
                }),
                None,
            )
            .await
            .expect("handle");

        match &result.structured_status {
            ToolTerminalStatus::Failed(ToolCallError::ExecutionFailed(message)) => {
                assert!(
                    message.contains("line number"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected failed execution status, got {other:?}"),
        }
    }
}
