use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct TodoWriteHandler;

#[async_trait]
impl ToolHandler for TodoWriteHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::TodoWrite
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let todos = invocation.input["todos"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(Box::new(FunctionToolOutput::success(
            serde_json::to_string_pretty(&todos).map_err(|e| {
                ToolExecutionError::ExecutionFailed {
                    message: e.to_string(),
                }
            })?,
        )))
    }
}
