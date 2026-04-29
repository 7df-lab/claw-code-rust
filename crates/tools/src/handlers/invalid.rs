use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct InvalidHandler;

#[async_trait]
impl ToolHandler for InvalidHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Invalid
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let error = invocation.input["error"]
            .as_str()
            .unwrap_or("invalid tool arguments");
        Ok(Box::new(FunctionToolOutput::error(format!(
            "The arguments provided to the tool are invalid: {error}"
        ))))
    }
}
