use devo_protocol::ToolDefinition as ProtocolToolDefinition;

use crate::runtime::{RuntimeToolRegistry, ToolDefinitionSpec, ToolRuntimeConfigSnapshot};

/// Request-scoped context used to assemble the model-visible tool list.
pub struct ToolAssemblyContext<'a> {
    pub app_config: &'a ToolRuntimeConfigSnapshot,
    pub os: &'a str,
}

impl<'a> ToolAssemblyContext<'a> {
    pub fn current(app_config: &'a ToolRuntimeConfigSnapshot) -> Self {
        Self {
            app_config,
            os: std::env::consts::OS,
        }
    }
}

pub fn assemble_runtime_tool_definitions(
    registry: &RuntimeToolRegistry,
    context: &ToolAssemblyContext<'_>,
) -> Vec<ToolDefinitionSpec> {
    let definitions = registry.list_enabled(context.app_config);
    let has_shell_command = definitions.iter().any(|definition| {
        definition.name.0.as_str() == "shell_command" && is_tool_supported(definition, context)
    });

    definitions
        .into_iter()
        .filter(|definition| is_tool_supported(definition, context))
        .filter(|definition| {
            if has_shell_command {
                definition.name.0.as_str() != "bash"
            } else {
                true
            }
        })
        .collect()
}

pub fn assemble_protocol_tool_definitions(
    registry: &RuntimeToolRegistry,
    context: &ToolAssemblyContext<'_>,
) -> Vec<ProtocolToolDefinition> {
    assemble_runtime_tool_definitions(registry, context)
        .into_iter()
        .map(|definition| ProtocolToolDefinition {
            name: definition.name.0.to_string(),
            description: definition.description,
            input_schema: definition.input_schema,
        })
        .collect()
}

fn is_tool_supported(definition: &ToolDefinitionSpec, context: &ToolAssemblyContext<'_>) -> bool {
    match definition.name.0.as_str() {
        "shell_command" => matches!(
            context.os,
            "windows" | "linux" | "macos" | "freebsd" | "dragonfly" | "openbsd" | "netbsd"
        ),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::runtime::{
        RuntimeToolRegistry, ShellCommandRuntimeTool, ToolAssemblyContext,
        ToolRuntimeConfigSnapshot, assemble_protocol_tool_definitions,
        register_builtin_runtime_tools,
    };

    #[test]
    fn assembly_prefers_shell_command_over_bash_when_both_exist() {
        let registry = RuntimeToolRegistry::new();
        register_builtin_runtime_tools(&registry);
        registry.register(std::sync::Arc::new(ShellCommandRuntimeTool));

        let tools = assemble_protocol_tool_definitions(
            &registry,
            &ToolAssemblyContext {
                app_config: &ToolRuntimeConfigSnapshot::default(),
                os: std::env::consts::OS,
            },
        );

        assert!(tools.iter().any(|tool| tool.name == "shell_command"));
        assert!(!tools.iter().any(|tool| tool.name == "bash"));
    }

    #[test]
    fn assembly_respects_enabled_tool_filter() {
        let registry = RuntimeToolRegistry::new();
        register_builtin_runtime_tools(&registry);
        registry.register(std::sync::Arc::new(ShellCommandRuntimeTool));

        let tools = assemble_protocol_tool_definitions(
            &registry,
            &ToolAssemblyContext {
                app_config: &ToolRuntimeConfigSnapshot {
                    enabled_tools: vec!["shell_command".into(), "read".into()],
                    max_parallel_read_tools: 1,
                },
                os: std::env::consts::OS,
            },
        );

        let mut names = tools.into_iter().map(|tool| tool.name).collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["read".to_string(), "shell_command".to_string()]);
    }
}
