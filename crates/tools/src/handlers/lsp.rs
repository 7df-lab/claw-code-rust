use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct LspHandler;

#[async_trait]
impl ToolHandler for LspHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Lsp
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let operation = invocation.input["operation"].as_str().unwrap_or("");
        Ok(Box::new(FunctionToolOutput::success(format!(
            "LSP request received for {operation}"
        ))))
    }
}
