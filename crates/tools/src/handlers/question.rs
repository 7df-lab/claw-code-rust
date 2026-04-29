use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct QuestionHandler;

#[async_trait]
impl ToolHandler for QuestionHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Question
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let question = invocation.input["question"].as_str().unwrap_or("");
        Ok(Box::new(FunctionToolOutput::success(format!(
            "Question for user: {question}"
        ))))
    }
}
