// New modules
pub mod errors;
pub mod events;
pub mod handler_kind;
pub mod handlers;
pub mod invocation;
pub mod json_schema;
pub mod registry;
pub mod registry_plan;
pub mod router;
pub mod tool_handler;
pub mod tool_spec;
pub mod tool_summary;
pub mod unified_exec;

// Existing modules (tools)
mod apply_patch;
mod bash;
mod context;
mod file_write;
mod glob;
mod grep;
mod invalid;
mod lsp;
mod orchestrator;
mod plan;
mod question;
mod read;
mod shell_exec;
mod skill;
mod task;
mod todo;
mod tool;
mod webfetch;
mod websearch;

// New re-exports
pub use errors::*;
pub use events::*;
pub use handler_kind::ToolHandlerKind;
pub use invocation::{FunctionToolOutput, ToolCallId, ToolContent, ToolInvocation, ToolName};
pub use json_schema::JsonSchema;
pub use registry::*;
pub use registry_plan::*;
pub use router::*;
pub use tool_handler::ToolHandler;
pub use tool_spec::*;

pub use apply_patch::ApplyPatchTool;
pub use bash::BashTool;
pub use context::ToolContext;
pub use file_write::FileWriteTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use invalid::InvalidTool;
pub use lsp::LspTool;
pub use plan::PlanTool;
pub use question::QuestionTool;
pub use read::ReadTool;
pub use skill::SkillTool;
pub use task::TaskTool;
pub use todo::TodoWriteTool;
pub use tool::{Tool, ToolOutput, ToolProgressEvent};
pub use webfetch::WebFetchTool;
pub use websearch::WebSearchTool;

use std::sync::Arc;

/// Create a fully-configured tool registry with all built-in tools.
/// This is the new recommended way to bootstrap tools.
pub fn create_default_tool_registry() -> registry::ToolRegistry {
    handlers::build_registry_from_plan(&ToolPlanConfig::default())
}

#[allow(deprecated)]
pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    let plan = build_tool_registry_plan(&ToolPlanConfig::default());
    let mut builder = ToolRegistryBuilder::new();
    for spec in plan.specs {
        builder.push_spec(spec);
    }
    for (kind, name) in plan.handlers {
        use crate::tool_handler::ToolHandler;
        let handler: Arc<dyn ToolHandler> = match kind {
            ToolHandlerKind::Bash => Arc::new(handlers::BashHandler),
            ToolHandlerKind::ShellCommand => Arc::new(handlers::ShellCommandHandler),
            ToolHandlerKind::Read => Arc::new(handlers::ReadHandler),
            ToolHandlerKind::Write => Arc::new(handlers::WriteHandler),
            ToolHandlerKind::Glob => Arc::new(handlers::GlobHandler),
            ToolHandlerKind::Grep => Arc::new(handlers::GrepHandler),
            ToolHandlerKind::ApplyPatch => Arc::new(handlers::ApplyPatchHandler),
            ToolHandlerKind::Plan => Arc::new(handlers::PlanHandler),
            ToolHandlerKind::Question => Arc::new(handlers::QuestionHandler),
            ToolHandlerKind::Task => Arc::new(handlers::TaskHandler),
            ToolHandlerKind::TodoWrite => Arc::new(handlers::TodoWriteHandler),
            ToolHandlerKind::WebFetch => Arc::new(handlers::WebFetchHandler),
            ToolHandlerKind::WebSearch => Arc::new(handlers::WebSearchHandler),
            ToolHandlerKind::Skill => Arc::new(handlers::SkillHandler),
            ToolHandlerKind::Lsp => Arc::new(handlers::LspHandler),
            ToolHandlerKind::Invalid => Arc::new(handlers::InvalidHandler),
            ToolHandlerKind::ExecCommand => {
                let store = Arc::new(crate::unified_exec::store::ProcessStore::new());
                Arc::new(handlers::ExecCommandHandler::new(store))
            }
            ToolHandlerKind::WriteStdin => {
                let store = Arc::new(crate::unified_exec::store::ProcessStore::new());
                Arc::new(handlers::WriteStdinHandler::new(store))
            }
        };
        builder.register_handler(&name, handler);
    }
    let new_registry = builder.build();
    registry.handlers = new_registry.handlers;
    registry.specs = new_registry.specs;
    registry.spec_index = new_registry.spec_index;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_tool_names_default() -> [&'static str; 17] {
        [
            "bash",
            "read",
            "write",
            "glob",
            "grep",
            "invalid",
            "question",
            "task",
            "todowrite",
            "webfetch",
            "websearch",
            "skill",
            "apply_patch",
            "lsp",
            "update_plan",
            "exec_command",
            "write_stdin",
        ]
    }

    #[test]
    fn registry_from_plan_contains_all_tools_default() {
        let registry = handlers::build_registry_from_plan(&ToolPlanConfig::default());

        for name in &expected_tool_names_default() {
            assert!(
                registry.get(name).is_some(),
                "expected tool '{name}' to be registered"
            );
        }
        // shell_command not registered by default (use_shell_command = false)
        assert!(registry.get("shell_command").is_none());
    }

    #[test]
    fn registry_from_plan_uses_shell_command_when_configured() {
        let mut config = ToolPlanConfig::default();
        config.use_shell_command = true;
        let registry = handlers::build_registry_from_plan(&config);

        // When use_shell_command = true, bash is replaced by shell_command
        assert!(registry.get("bash").is_none());
        assert!(
            registry.get("shell_command").is_some(),
            "expected shell_command tool to be registered"
        );
    }

    #[test]
    fn registry_from_plan_without_unified_exec() {
        let config = ToolPlanConfig {
            use_unified_exec: false,
            ..ToolPlanConfig::default()
        };
        let registry = handlers::build_registry_from_plan(&config);
        assert!(
            registry.get("exec_command").is_none(),
            "exec_command should not be registered when use_unified_exec is false"
        );
        assert!(
            registry.get("write_stdin").is_none(),
            "write_stdin should not be registered when use_unified_exec is false"
        );
    }

    #[test]
    fn builtin_tools_have_nonempty_definitions() {
        let registry = handlers::build_registry_from_plan(&ToolPlanConfig::default());
        let defs = registry.tool_definitions();
        for def in &defs {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            assert!(def.input_schema.is_object());
        }
    }

    #[test]
    fn register_builtin_tools_populates_registry() {
        #[allow(deprecated)]
        {
            let mut registry = ToolRegistry::new();
            register_builtin_tools(&mut registry);
            for name in &expected_tool_names_default()[..15] {
                assert!(
                    registry.get(name).is_some(),
                    "expected builtin tool '{name}' to be registered"
                );
            }
        }
    }
}
