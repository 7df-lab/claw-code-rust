use std::path::PathBuf;

use async_trait::async_trait;
use tracing::info;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct WriteHandler;

#[async_trait]
impl ToolHandler for WriteHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Write
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let path_str = invocation.input["filePath"].as_str().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: "missing 'filePath' field".into(),
            }
        })?;
        let content = invocation.input["content"].as_str().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: "missing 'content' field".into(),
            }
        })?;

        let path = resolve_path(&invocation.cwd, path_str);
        info!(path = %path.display(), bytes = content.len(), "writing file");

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("failed to create directories: {e}"),
                }
            })?;
        }

        tokio::fs::write(&path, content).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("failed to write file: {e}"),
            }
        })?;

        Ok(Box::new(FunctionToolOutput::success(format!(
            "wrote {} bytes to {}",
            content.len(),
            path.display()
        ))))
    }
}

fn resolve_path(cwd: &std::path::Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}
