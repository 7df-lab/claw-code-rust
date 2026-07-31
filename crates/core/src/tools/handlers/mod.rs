mod agent;
mod apply_patch;
#[cfg(feature = "code-search")]
mod code_search;
mod edit;
mod exec_command;
mod file_change_metadata;
mod file_write;
mod glob;
mod goal_update;
mod grep;
mod invalid;
mod lsp;
mod mcp;
mod plan;
mod question;
mod read;
mod ripgrep;
mod shell_command;
mod skill;
mod tool_search;
mod webfetch;
mod websearch;

pub(crate) use agent::register_agent_tools;
pub use apply_patch::ApplyPatchHandler;
#[cfg(feature = "code-search")]
pub use code_search::CodeSearchHandler;
pub use edit::EditHandler;
pub use exec_command::{ExecCommandHandler, WriteStdinHandler};
pub use file_write::WriteHandler;
pub use glob::GlobHandler;
pub use goal_update::{GoalUpdateHandler, goal_update_spec};
pub use grep::GrepHandler;
pub use invalid::InvalidHandler;
pub use lsp::LspHandler;
pub use mcp::{McpToolHandler, mcp_search_text, mcp_tool_spec};
pub use plan::PlanHandler;
pub use question::QuestionHandler;
pub use read::ReadHandler;
pub use shell_command::ShellCommandHandler;
pub use skill::SkillHandler;
pub use tool_search::{ToolSearchHandler, tool_search_spec};
pub use webfetch::WebFetchHandler;
pub use websearch::WebSearchHandler;

use std::sync::Arc;

use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::handler_kind::ToolHandlerKind;
use crate::mcp::McpManager;
use crate::mcp::build_mcp_tool_exposure;
use crate::registry::ToolExposure;
use crate::registry::ToolRegistryBuilder;
use crate::registry_plan::{ToolPlanConfig, build_tool_registry_plan};
use crate::tool_handler::ToolHandler;
use crate::unified_exec::store::ProcessStore;

pub fn build_registry_from_plan(config: &ToolPlanConfig) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    for spec in specs {
        builder.push_spec(spec);
    }
    build_registry_from_builder(handlers, builder, Vec::new(), config)
}

pub async fn build_registry_from_plan_with_mcp(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    for spec in specs {
        builder.push_spec(spec);
    }

    let mut mcp_handlers = Vec::new();
    let mcp_tools = match mcp_manager.discover_tools().await {
        Ok(tools) => tools,
        Err(err) => {
            tracing::warn!(error = %err, "failed to discover MCP tools");
            Vec::new()
        }
    };
    let exposure = build_mcp_tool_exposure(&mcp_tools);
    for info in exposure.direct_tools {
        let spec = mcp_tool_spec(&info);
        let name = spec.name.clone();
        builder.set_search_text(&name, mcp_search_text(&info));
        builder.push_spec_with_exposure(spec, ToolExposure::Direct);
        mcp_handlers.push((
            name,
            Arc::new(McpToolHandler::new(Arc::clone(&mcp_manager), info)) as Arc<dyn ToolHandler>,
        ));
    }
    for info in exposure.deferred_tools {
        let spec = mcp_tool_spec(&info);
        let name = spec.name.clone();
        builder.set_search_text(&name, mcp_search_text(&info));
        builder.push_spec_with_exposure(spec, ToolExposure::Deferred);
        mcp_handlers.push((
            name,
            Arc::new(McpToolHandler::new(Arc::clone(&mcp_manager), info)) as Arc<dyn ToolHandler>,
        ));
    }

    build_registry_from_builder(handlers, builder, mcp_handlers, config)
}

fn build_registry_from_builder(
    handlers: Vec<(ToolHandlerKind, String)>,
    mut builder: ToolRegistryBuilder,
    mcp_handlers: Vec<(String, Arc<dyn ToolHandler>)>,
    config: &ToolPlanConfig,
) -> crate::registry::ToolRegistry {
    let process_store = Arc::new(ProcessStore::new());
    let background_tasks = Arc::new(crate::tools::background_tasks::BackgroundTaskStore::new(
        Arc::clone(&process_store),
    ));
    register_agent_tools(&mut builder, Arc::clone(&background_tasks));
    builder.push_spec(goal_update_spec());
    builder.push_spec(tool_search_spec());

    let loaded_deferred_tools = Arc::new(std::sync::Mutex::new(LoadedDeferredTools::default()));
    builder.set_unified_exec_store(Arc::clone(&process_store));
    builder.set_loaded_deferred_tools(Arc::clone(&loaded_deferred_tools));
    builder.register_handler("update_goal", Arc::new(GoalUpdateHandler::new()));

    for (kind, name) in handlers {
        let handler: Arc<dyn ToolHandler> = match kind {
            #[cfg(feature = "code-search")]
            ToolHandlerKind::CodeSearch => {
                let service = Arc::new(
                    devo_code_search::CodeSearchService::production_with_network_proxy(
                        devo_network_proxy::NetworkProxyConfig {
                            proxy_url: config.network_proxy.clone(),
                            no_proxy: config.network_no_proxy.clone(),
                        },
                    ),
                );
                builder.set_code_search_service(Arc::clone(&service));
                Arc::new(CodeSearchHandler::with_service(service))
            }
            // When the `code-search` feature is disabled the planner never emits
            // this handler kind (see registry_plan), so the arm is unreachable.
            #[cfg(not(feature = "code-search"))]
            ToolHandlerKind::CodeSearch => {
                unreachable!("code_search handler requested but `code-search` feature is disabled")
            }
            ToolHandlerKind::ShellCommand => Arc::new(ShellCommandHandler::new()),
            ToolHandlerKind::Read => Arc::new(ReadHandler::new()),
            ToolHandlerKind::Write => Arc::new(WriteHandler::new()),
            ToolHandlerKind::Edit => Arc::new(EditHandler::new()),
            ToolHandlerKind::Glob => Arc::new(GlobHandler::new()),
            ToolHandlerKind::Grep => Arc::new(GrepHandler::new()),
            ToolHandlerKind::ApplyPatch => Arc::new(ApplyPatchHandler::new()),
            ToolHandlerKind::Plan => Arc::new(PlanHandler::new()),
            ToolHandlerKind::Question => Arc::new(QuestionHandler::new()),
            ToolHandlerKind::WebFetch => Arc::new(WebFetchHandler::new()),
            ToolHandlerKind::WebSearch => Arc::new(WebSearchHandler::new()),
            ToolHandlerKind::Skill => Arc::new(SkillHandler::new()),
            ToolHandlerKind::Lsp => Arc::new(LspHandler::new()),
            ToolHandlerKind::Invalid => Arc::new(InvalidHandler::new()),
            ToolHandlerKind::ExecCommand => Arc::new(ExecCommandHandler::new(
                Arc::clone(&process_store),
                Arc::clone(&background_tasks),
            )),
            ToolHandlerKind::WriteStdin => {
                Arc::new(WriteStdinHandler::new(Arc::clone(&process_store)))
            }
            ToolHandlerKind::ToolSearch => Arc::new(ToolSearchHandler::new(
                builder.tool_search_entries(),
                Arc::clone(&loaded_deferred_tools),
                builder.effective_deferred_loading_config(&DeferredLoadingConfig::default()),
            )),
        };
        let legacy_alias = match kind {
            ToolHandlerKind::ShellCommand if name == "shell_command" => Some("bash"),
            ToolHandlerKind::Glob if name == "find" => Some("glob"),
            ToolHandlerKind::Question if name == "request_user_input" => Some("question"),
            ToolHandlerKind::WebSearch if name == "web_search" => Some("websearch"),
            _ => None,
        };
        builder.register_handler(&name, Arc::clone(&handler));
        if let Some(alias) = legacy_alias {
            builder.register_handler(alias, Arc::clone(&handler));
        }
        if kind == ToolHandlerKind::WebSearch && name == "web_search" {
            builder.register_handler("web-search", handler);
        }
    }
    for (name, handler) in mcp_handlers {
        builder.register_handler(&name, handler);
    }
    builder.register_handler(
        "ToolSearch",
        Arc::new(ToolSearchHandler::new(
            builder.tool_search_entries(),
            Arc::clone(&loaded_deferred_tools),
            builder.effective_deferred_loading_config(&DeferredLoadingConfig::default()),
        )),
    );

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_spec::ToolExecutionMode;

    #[test]
    fn default_registry_exposes_shell_command_and_accepts_bash_alias() {
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.spec("shell_command").is_some());
        assert!(registry.spec("bash").is_none());
        assert!(registry.get("shell_command").is_some());
        assert!(registry.get("bash").is_some());
    }

    #[cfg(feature = "code-search")]
    #[test]
    fn registry_exposes_the_code_search_handlers_shared_service() {
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.get("code_search").is_some());
        assert!(registry.code_search_service().is_some());
    }

    #[cfg(feature = "code-search")]
    #[test]
    fn registry_has_no_code_search_service_when_the_tool_is_disabled() {
        let registry = build_registry_from_plan(&ToolPlanConfig {
            code_search: false,
            ..ToolPlanConfig::default()
        });

        assert!(registry.get("code_search").is_none());
        assert!(registry.code_search_service().is_none());
    }

    #[test]
    fn default_registry_exposes_update_goal_tool() {
        // Trace: L2-DES-GOAL-001
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.spec("update_goal").is_some());
        assert!(registry.get("update_goal").is_some());
    }

    #[test]
    fn default_registry_exposes_edit_tool() {
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.spec("edit").is_some());
        assert!(registry.get("edit").is_some());
        let spec = registry.spec("edit").expect("edit spec");
        assert_eq!(spec.execution_mode, ToolExecutionMode::Mutating);
        assert!(!spec.supports_parallel);
    }
}
