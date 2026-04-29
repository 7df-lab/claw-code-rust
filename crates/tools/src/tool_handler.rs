use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{ToolInvocation, ToolOutput};

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn tool_kind(&self) -> ToolHandlerKind;

    /// Execute a tool call. If `progress` is provided, the handler may send
    /// incremental output deltas during execution for real-time client rendering.
    async fn handle(
        &self,
        invocation: ToolInvocation,
        progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError>;
}
