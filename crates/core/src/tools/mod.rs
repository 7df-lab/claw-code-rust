pub mod contracts {
    pub use devo_tools::contracts::*;
}

pub mod deferred_loading;
pub mod errors {
    pub use devo_tools::errors::*;
}
pub mod events {
    pub use devo_tools::events::*;
}
pub mod handler_kind {
    pub use devo_tools::handler_kind::*;
}
pub mod handlers;
mod hook_events;
pub mod invocation {
    pub use devo_tools::invocation::*;
}
pub mod json_schema {
    pub use devo_tools::json_schema::*;
}
pub mod registry;
pub mod registry_plan;
pub mod router;
pub mod tool_handler {
    pub use devo_tools::tool_handler::*;
}
pub mod tool_spec {
    pub use devo_tools::tool_spec::*;
}
pub mod tool_summary {
    pub use devo_tools::tool_summary::*;
}
pub mod unified_exec;

pub(crate) mod apply_patch;
pub mod exec_policy_amend;
pub(crate) mod read;
pub use exec_policy_amend::is_banned_prefix_suggestion;
pub(crate) mod exec_policy_loader;
pub use exec_policy_loader::{
    ExecPolicyLoadError, load_exec_policy_from_devo_home, load_exec_policy_from_dir,
};
pub(crate) mod shell_exec;
pub(crate) mod websearch_prompt;

pub use contracts::{
    RedactionState, SessionMode, ToolAgentScope, ToolCallError, ToolContext, ToolPermissionProfile,
    ToolProgress, ToolProgressSender, ToolResult, ToolResultContent, ToolTerminalStatus,
};
pub use deferred_loading::*;
pub use devo_tools::{
    AgentToolCoordinator, ClientFilesystem, ClientTextFileRead, ClientTextFileWrite,
    FileReadFreshnessError, FileReadLedger,
};
pub use errors::*;
pub use events::ToolEvent;
pub use handler_kind::ToolHandlerKind;
pub use invocation::{
    FunctionToolOutput, ToolCallId, ToolContent, ToolInvocation, ToolName, ToolOutput,
};
pub use json_schema::JsonSchema;
pub use registry::*;
pub use registry_plan::*;
pub use router::*;
pub use tool_handler::ToolHandler;
pub use tool_spec::*;

pub fn create_default_tool_registry() -> registry::ToolRegistry {
    handlers::build_registry_from_plan(&ToolPlanConfig::default())
}
pub(crate) mod background_tasks;
