use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct TaskHandler;

#[async_trait]
impl ToolHandler for TaskHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Task
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let description = invocation.input["description"].as_str().unwrap_or("task");
        let task_id = invocation.input["task_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let prompt = invocation.input["prompt"].as_str().unwrap_or("");
        Ok(Box::new(FunctionToolOutput::success(format!(
            "task_id: {task_id} (for resuming to continue this task if needed)\n\n<task_result>\nTask requested: {description}\n{prompt}\n</task_result>"
        ))))
    }
}
