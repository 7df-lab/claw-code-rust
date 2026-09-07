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

/// Command tools that advertise sandbox escalation fields on the model-facing schema.
///
/// `write_stdin` is excluded: sandbox policy is decided when the process is spawned.
pub(crate) fn tool_accepts_sandbox_escalation_fields(tool_name: &str) -> bool {
    matches!(tool_name, "bash" | "shell_command" | "exec_command")
}

pub(crate) fn sandbox_overlay_for_spawn(
    overlay: Option<&devo_tools::SandboxPermissionOverlay>,
) -> Option<devo_sandbox::SandboxPermissionOverlay> {
    overlay.map(|overlay| devo_sandbox::SandboxPermissionOverlay {
        network: match overlay.network {
            devo_tools::SandboxNetworkPermission::Unchanged => {
                devo_sandbox::SandboxNetworkPermission::Unchanged
            }
            devo_tools::SandboxNetworkPermission::Enabled => {
                devo_sandbox::SandboxNetworkPermission::Enabled
            }
        },
        read_paths: overlay.read_paths.clone(),
        write_paths: overlay.write_paths.clone(),
    })
}

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

pub use devo_tools::output_store;
