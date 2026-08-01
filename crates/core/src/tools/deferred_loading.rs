use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use devo_protocol::ToolDefinition;

use crate::tool_spec::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptLoadingPolicy {
    Preloaded,
    Deferred,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredLoadingConfig {
    pub enabled: bool,
    pub default_policy: PromptLoadingPolicy,
    pub preloaded: BTreeSet<String>,
    pub deferred: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
}

impl Default for DeferredLoadingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_policy: PromptLoadingPolicy::Deferred,
            preloaded: default_preloaded_tools(),
            deferred: default_deferred_tools(),
            hidden: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredTool {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchMetrics {
    pub baseline_tool_schema_tokens: usize,
    pub exposed_tool_schema_tokens: usize,
    pub deferred_reminder_tokens: usize,
    pub estimated_net_tool_context_tokens: usize,
    pub estimated_tokens_saved: usize,
    pub exposed_tool_count: usize,
    pub hidden_tool_count: usize,
    pub loaded_deferred_tool_count: usize,
}

#[derive(Debug, Clone)]
pub struct DeferredToolPrompt {
    pub exposed: Vec<ToolDefinition>,
    pub core: Vec<String>,
    pub loaded_deferred: Vec<String>,
    pub deferred: Vec<DeferredTool>,
    pub reminder: Option<String>,
    pub metrics: ToolSearchMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchResult {
    pub loaded: Vec<String>,
    pub already_loaded: Vec<String>,
    pub already_available: Vec<String>,
    pub not_found: Vec<String>,
}

const SUBAGENT_PROHIBITED_AGENT_COORDINATION_TOOLS: &[&str] = &[
    "spawn_agent",
    "spawn-agent",
    "spawnagent",
    "spawn_subagent",
    "spawn-subagent",
    "subagent",
    "sub_agent",
    "delegate",
    "send_message",
    "send-message",
    "sendmessage",
    "await_task",
    "await-task",
    "awaittask",
    "list_tasks",
    "list-tasks",
    "listtasks",
    "cancel_task",
    "cancel-task",
    "canceltask",
    "wait_agent",
    "wait-agent",
    "waitagent",
    "subagent_result",
    "subagent-result",
    "list_agents",
    "list-agents",
    "listagents",
    "subagent_status",
    "subagent-status",
    "close_agent",
    "close-agent",
    "closeagent",
];

impl ToolSearchResult {
    pub fn is_error(&self) -> bool {
        self.loaded.is_empty()
            && self.already_loaded.is_empty()
            && self.already_available.is_empty()
            && !self.not_found.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.is_error() {
            return format!(
                "Not found: {}. Only request exact tool names from the available tools list.",
                self.not_found.join(", ")
            );
        }

        let mut lines = Vec::new();
        if !self.loaded.is_empty() {
            lines.push(format!(
                "Loaded {} tool(s): {}",
                self.loaded.len(),
                self.loaded.join(", ")
            ));
        }
        if !self.already_loaded.is_empty() {
            lines.push(format!(
                "Already loaded {} tool(s): {}. Call these tools directly.",
                self.already_loaded.len(),
                self.already_loaded.join(", ")
            ));
        }
        if !self.already_available.is_empty() {
            lines.push(format!(
                "Already available {} tool(s): {}. Call these tools directly without loading.",
                self.already_available.len(),
                self.already_available.join(", ")
            ));
        }
        if !self.not_found.is_empty() {
            lines.push(format!(
                "Not found: {}. Only request exact tool names from the available tools list.",
                self.not_found.join(", ")
            ));
        }
        lines.join("\n")
    }
}

pub fn hide_subagent_agent_coordination_tools(config: &mut DeferredLoadingConfig) {
    for name in [
        "spawn_agent",
        "send_message",
        "await_task",
        "list_tasks",
        "cancel_task",
        "wait_agent",
        "list_agents",
        "close_agent",
    ] {
        config.hidden.insert(name.to_string());
    }
}

pub fn is_subagent_agent_coordination_tool(name: &str) -> bool {
    SUBAGENT_PROHIBITED_AGENT_COORDINATION_TOOLS
        .iter()
        .any(|tool| name.eq_ignore_ascii_case(tool))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedDeferredTools {
    by_session: HashMap<String, BTreeSet<String>>,
}

// TODO: should noted that the tool search tool is still a simple tool.

impl LoadedDeferredTools {
    pub fn mark_loaded(&mut self, session_id: &str, tool_name: &str) {
        self.by_session
            .entry(session_id.to_string())
            .or_default()
            .insert(tool_name.to_string());
    }

    pub fn is_loaded(&self, session_id: &str, tool_name: &str) -> bool {
        self.by_session
            .get(session_id)
            .is_some_and(|tools| tools.contains(tool_name))
    }

    pub fn list_loaded(&self, session_id: &str) -> BTreeSet<String> {
        self.by_session.get(session_id).cloned().unwrap_or_default()
    }
}

pub fn assemble_deferred_tool_prompt(
    all_tools: &[ToolDefinition],
    loaded: &BTreeSet<String>,
    config: &DeferredLoadingConfig,
) -> DeferredToolPrompt {
    if !config.enabled {
        let exposed = annotate_last_tool(all_tools.to_vec());
        let exposed_tokens = estimate_tool_tokens(&exposed);
        return DeferredToolPrompt {
            exposed,
            core: all_tools.iter().map(|tool| tool.name.clone()).collect(),
            loaded_deferred: Vec::new(),
            deferred: Vec::new(),
            reminder: None,
            metrics: ToolSearchMetrics {
                baseline_tool_schema_tokens: exposed_tokens,
                exposed_tool_schema_tokens: exposed_tokens,
                deferred_reminder_tokens: 0,
                estimated_net_tool_context_tokens: exposed_tokens,
                estimated_tokens_saved: 0,
                exposed_tool_count: all_tools.len(),
                hidden_tool_count: 0,
                loaded_deferred_tool_count: 0,
            },
        };
    }

    let mut exposed = Vec::new();
    let mut core = Vec::new();
    let mut loaded_deferred = Vec::new();
    let mut deferred = Vec::new();

    for tool in all_tools {
        match resolve_tool_policy(&tool.name, config) {
            PromptLoadingPolicy::Preloaded => {
                core.push(tool.name.clone());
                exposed.push(tool.clone());
            }
            PromptLoadingPolicy::Deferred => {
                if loaded.contains(&tool.name) {
                    loaded_deferred.push(tool.name.clone());
                    exposed.push(tool.clone());
                } else {
                    deferred.push(DeferredTool {
                        name: tool.name.clone(),
                        description: first_line(&tool.description),
                    });
                }
            }
            PromptLoadingPolicy::Hidden => {}
        }
    }

    let reminder = build_deferred_reminder(&deferred);
    let exposed = annotate_last_tool(exposed);
    let baseline_tool_schema_tokens = estimate_tool_tokens(all_tools);
    let exposed_tool_schema_tokens = estimate_tool_tokens(&exposed);
    let deferred_reminder_tokens = reminder.as_deref().map(estimate_text_tokens).unwrap_or(0);

    let hidden_tool_count = deferred.len();
    let loaded_deferred_tool_count = loaded_deferred.len();

    DeferredToolPrompt {
        exposed,
        core,
        loaded_deferred,
        deferred,
        reminder,
        metrics: ToolSearchMetrics {
            baseline_tool_schema_tokens,
            exposed_tool_schema_tokens,
            deferred_reminder_tokens,
            estimated_net_tool_context_tokens: exposed_tool_schema_tokens
                + deferred_reminder_tokens,
            estimated_tokens_saved: baseline_tool_schema_tokens
                .saturating_sub(exposed_tool_schema_tokens),
            exposed_tool_count: all_tools
                .iter()
                .filter(|tool| {
                    let policy = resolve_tool_policy(&tool.name, config);
                    policy == PromptLoadingPolicy::Preloaded
                        || (policy == PromptLoadingPolicy::Deferred && loaded.contains(&tool.name))
                })
                .count(),
            hidden_tool_count,
            loaded_deferred_tool_count,
        },
    }
}

pub fn execute_tool_search(
    _session_id: &str,
    query: &str,
    all_tools: &[ToolDefinition],
    _loaded: &mut LoadedDeferredTools,
    config: &DeferredLoadingConfig,
) -> Result<ToolSearchResult, String> {
    let Some(selection) = query
        .trim()
        .strip_prefix("select:")
        .or_else(|| query.trim().strip_prefix("SELECT:"))
    else {
        return Err("Expected query format: select:<name>[,<name>...]".to_string());
    };

    let names = selection
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let registered: HashSet<_> = all_tools.iter().map(|tool| tool.name.as_str()).collect();
    let alias_map = alias_map(&registered);
    let mut result = ToolSearchResult {
        loaded: Vec::new(),
        already_loaded: Vec::new(),
        already_available: Vec::new(),
        not_found: Vec::new(),
    };

    for requested_name in names {
        let canonical_name = resolve_alias(requested_name, &alias_map);
        if !registered.contains(canonical_name.as_str()) {
            result.not_found.push(requested_name.to_string());
            continue;
        }

        match resolve_tool_policy(&canonical_name, config) {
            PromptLoadingPolicy::Preloaded | PromptLoadingPolicy::Deferred => {
                result.already_available.push(canonical_name);
            }
            PromptLoadingPolicy::Hidden => result.not_found.push(requested_name.to_string()),
        }
    }

    if result.is_error() {
        return Err(result.summary());
    }

    Ok(result)
}

pub fn resolve_spec_policy(spec: &ToolSpec, config: &DeferredLoadingConfig) -> PromptLoadingPolicy {
    resolve_tool_policy(&spec.name, config)
}

pub fn resolve_tool_policy(name: &str, config: &DeferredLoadingConfig) -> PromptLoadingPolicy {
    if config.hidden.contains(name) {
        return PromptLoadingPolicy::Hidden;
    }
    if config.preloaded.contains(name) {
        return PromptLoadingPolicy::Preloaded;
    }
    if config.deferred.contains(name) {
        return PromptLoadingPolicy::Deferred;
    }
    if name.starts_with("mcp__") {
        return PromptLoadingPolicy::Deferred;
    }
    config.default_policy
}

fn build_deferred_reminder(deferred: &[DeferredTool]) -> Option<String> {
    if deferred.is_empty() {
        return None;
    }

    let mut reminder = String::from(
        "<system-reminder>\nThe following deferred tools are available via ToolSearch. Use query \"select:<name>[,<name>...]\" to load them.\nDeferred tools:",
    );
    for tool in deferred {
        reminder.push_str(&format!("\n  {}: {}", tool.name, tool.description));
    }
    reminder.push_str("\n</system-reminder>");
    Some(reminder)
}

fn annotate_last_tool(mut tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    if let Some(tool) = tools.last_mut()
        && !tool
            .description
            .contains("To load additional tools, use the ToolSearch tool.")
    {
        tool.description
            .push_str("\n\nTo load additional tools, use the ToolSearch tool.");
    }
    tools
}

fn alias_map(registered: &HashSet<&str>) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for name in registered {
        aliases.insert(name.to_lowercase(), (*name).to_string());
        aliases.insert(name.replace('_', "-").to_lowercase(), (*name).to_string());
        aliases.insert(
            name.replace(['_', '-'], "").to_lowercase(),
            (*name).to_string(),
        );
    }

    insert_alias(&mut aliases, registered, "bash", "shell_command");
    insert_alias(&mut aliases, registered, "shell", "shell_command");
    insert_alias(&mut aliases, registered, "terminal", "shell_command");
    insert_alias(&mut aliases, registered, "runcommand", "shell_command");
    insert_alias(&mut aliases, registered, "exec", "shell_command");
    insert_alias(&mut aliases, registered, "readfile", "read");
    insert_alias(&mut aliases, registered, "read-file", "read");
    insert_alias(&mut aliases, registered, "cat", "read");
    insert_alias(&mut aliases, registered, "view", "read");
    insert_alias(&mut aliases, registered, "glob", "find");
    insert_alias(&mut aliases, registered, "file-search", "find");
    insert_alias(&mut aliases, registered, "file_search", "find");
    insert_alias(&mut aliases, registered, "rg", "grep");
    insert_alias(&mut aliases, registered, "ripgrep", "grep");
    insert_alias(&mut aliases, registered, "fetch-url", "fetch_url");
    insert_alias(&mut aliases, registered, "urlfetch", "fetch_url");
    insert_alias(&mut aliases, registered, "websearch", "web_search");
    insert_alias(&mut aliases, registered, "web-search", "web_search");
    insert_alias(&mut aliases, registered, "fetch_url", "webfetch");
    insert_alias(&mut aliases, registered, "fetch-url", "webfetch");
    insert_alias(&mut aliases, registered, "tool-search", "ToolSearch");
    insert_alias(&mut aliases, registered, "tool_search", "ToolSearch");
    insert_alias(&mut aliases, registered, "loadtool", "ToolSearch");
    insert_alias(&mut aliases, registered, "subagent", "spawn_agent");
    insert_alias(&mut aliases, registered, "delegate", "spawn_agent");
    insert_alias(&mut aliases, registered, "spawn_subagent", "spawn_agent");
    insert_alias(&mut aliases, registered, "sub_agent", "spawn_agent");
    insert_alias(&mut aliases, registered, "subagent_status", "list_agents");
    insert_alias(&mut aliases, registered, "subagent_result", "wait_agent");
    aliases
}

fn insert_alias(
    aliases: &mut HashMap<String, String>,
    registered: &HashSet<&str>,
    alias: &str,
    canonical: &str,
) {
    if registered.contains(canonical) {
        aliases.insert(alias.to_string(), canonical.to_string());
    }
}

fn resolve_alias(requested_name: &str, alias_map: &HashMap<String, String>) -> String {
    alias_map
        .get(&requested_name.to_lowercase())
        .cloned()
        .unwrap_or_else(|| requested_name.to_string())
}

fn default_preloaded_tools() -> BTreeSet<String> {
    [
        "read",
        "find",
        "grep",
        "ls",
        "write",
        "apply_patch",
        "shell_command",
        "exec_command",
        "plan",
        "update_plan",
        "approval",
        "ToolSearch",
        "web_search",
        "webfetch",
        "web_fetch",
        "fetch_url",
        "spawn_agent",
        "send_message",
        "await_task",
        "list_tasks",
        "cancel_task",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_deferred_tools() -> BTreeSet<String> {
    ["multi_tool_use", "goal_update"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn first_line(description: &str) -> String {
    description.lines().next().unwrap_or_default().to_string()
}

fn estimate_tool_tokens(tools: &[ToolDefinition]) -> usize {
    estimate_text_tokens(&serde_json::to_string(tools).unwrap_or_default())
}

fn estimate_text_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        }
    }

    fn tools() -> Vec<ToolDefinition> {
        vec![
            tool("read", "Read a file."),
            tool("find", "Find files."),
            tool("grep", "Search file contents."),
            tool("ToolSearch", "Search available tools."),
            tool(
                "web_search",
                "Search the web.\nLonger details are schema-only.",
            ),
            tool("fetch_url", "Fetch a URL."),
            tool("spawn_agent", "Spawn a subagent."),
            tool("secret_internal", "Internal lifecycle operation."),
        ]
    }

    fn agent_coordination_tools() -> Vec<ToolDefinition> {
        vec![
            tool("ToolSearch", "Search available tools."),
            tool("spawn_agent", "Spawn a subagent."),
            tool("send_message", "Send input to a child agent."),
            tool("await_task", "Wait for task completion."),
            tool("list_tasks", "List child tasks."),
            tool("cancel_task", "Cancel a child task."),
            tool("wait_agent", "Poll child output."),
            tool("list_agents", "List child agents."),
            tool("close_agent", "Close a child agent."),
        ]
    }

    #[test]
    fn classification_exposes_default_preloaded_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());
        let loaded = BTreeSet::from(["web_search".to_string()]);

        let prompt = assemble_deferred_tool_prompt(&tools(), &loaded, &config);

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| &tool.name)
                .collect::<Vec<_>>(),
            vec![
                "read",
                "find",
                "grep",
                "ToolSearch",
                "web_search",
                "fetch_url",
                "spawn_agent"
            ]
        );
        assert_eq!(
            prompt.core,
            vec![
                "read",
                "find",
                "grep",
                "ToolSearch",
                "web_search",
                "fetch_url",
                "spawn_agent"
            ]
        );
        assert_eq!(prompt.loaded_deferred, Vec::<String>::new());
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
        assert_eq!(prompt.reminder, None);
    }

    #[test]
    fn default_reminder_omits_preloaded_web_and_agent_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());

        let prompt = assemble_deferred_tool_prompt(&tools(), &BTreeSet::new(), &config);

        assert_eq!(prompt.reminder, None);
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
    }

    #[test]
    fn tool_search_resolves_aliases_and_reports_available_tools() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();

        let result = execute_tool_search(
            "session-1",
            "select:WebSearch,fetch-url,read,glob",
            &tools(),
            &mut loaded,
            &config,
        )
        .expect("tool search should return available tools");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec![
                    "web_search".to_string(),
                    "fetch_url".to_string(),
                    "read".to_string(),
                    "find".to_string(),
                ],
                not_found: Vec::new(),
            }
        );
        assert!(!loaded.is_loaded("session-1", "web_search"));
        assert!(!loaded.is_loaded("session-1", "fetch_url"));
    }

    #[test]
    fn tool_search_resolves_agent_spawn_aliases() {
        let config = DeferredLoadingConfig::default();

        for requested in ["spawn_agent", "subagent", "delegate"] {
            let mut loaded = LoadedDeferredTools::default();
            let result = execute_tool_search(
                "session-1",
                &format!("select:{requested}"),
                &tools(),
                &mut loaded,
                &config,
            )
            .expect("agent spawn alias should resolve");

            assert_eq!(
                result,
                ToolSearchResult {
                    loaded: Vec::new(),
                    already_loaded: Vec::new(),
                    already_available: vec!["spawn_agent".to_string()],
                    not_found: Vec::new(),
                }
            );
            assert!(!loaded.is_loaded("session-1", "spawn_agent"));
        }
    }

    #[test]
    fn subagent_hidden_tools_hide_parent_agent_coordination_tools() {
        let mut config = DeferredLoadingConfig::default();
        hide_subagent_agent_coordination_tools(&mut config);

        for requested in [
            "spawn_agent",
            "spawn-agent",
            "spawnagent",
            "spawn_subagent",
            "spawn-subagent",
            "subagent",
            "sub_agent",
            "delegate",
            "send_message",
            "send-message",
            "sendmessage",
            "await_task",
            "await-task",
            "awaittask",
            "list_tasks",
            "list-tasks",
            "listtasks",
            "cancel_task",
            "cancel-task",
            "canceltask",
            "wait_agent",
            "wait-agent",
            "waitagent",
            "subagent_result",
            "subagent-result",
            "list_agents",
            "list-agents",
            "listagents",
            "subagent_status",
            "subagent-status",
            "close_agent",
            "close-agent",
            "closeagent",
        ] {
            let mut loaded = LoadedDeferredTools::default();
            let err = execute_tool_search(
                "session-1",
                &format!("select:{requested}"),
                &agent_coordination_tools(),
                &mut loaded,
                &config,
            )
            .expect_err("subagent agent coordination tools should remain hidden");

            assert!(err.contains("Not found"));
            assert!(!loaded.is_loaded("session-1", "spawn_agent"));
            assert!(!loaded.is_loaded("session-1", "send_message"));
            assert!(!loaded.is_loaded("session-1", "await_task"));
            assert!(!loaded.is_loaded("session-1", "list_tasks"));
            assert!(!loaded.is_loaded("session-1", "cancel_task"));
            assert!(!loaded.is_loaded("session-1", "wait_agent"));
            assert!(!loaded.is_loaded("session-1", "list_agents"));
            assert!(!loaded.is_loaded("session-1", "close_agent"));
        }
    }

    #[test]
    fn subagent_hidden_tools_override_loaded_agent_coordination_tools() {
        let mut config = DeferredLoadingConfig::default();
        hide_subagent_agent_coordination_tools(&mut config);
        let loaded = BTreeSet::from([
            "spawn_agent".to_string(),
            "send_message".to_string(),
            "await_task".to_string(),
            "list_tasks".to_string(),
            "cancel_task".to_string(),
            "wait_agent".to_string(),
            "list_agents".to_string(),
            "close_agent".to_string(),
        ]);

        let prompt = assemble_deferred_tool_prompt(&agent_coordination_tools(), &loaded, &config);

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ToolSearch"]
        );
        assert_eq!(prompt.loaded_deferred, Vec::<String>::new());
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
        assert_eq!(prompt.reminder, None);
    }

    #[test]
    fn tool_search_resolves_bash_alias_to_shell_command() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();
        let tools = vec![
            tool("shell_command", "Run a shell command."),
            tool("ToolSearch", "Search available tools."),
        ];

        let result = execute_tool_search("session-1", "select:bash", &tools, &mut loaded, &config)
            .expect("bash alias should resolve");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec!["shell_command".to_string()],
                not_found: Vec::new(),
            }
        );
    }

    #[test]
    fn mcp_tools_are_deferred_by_default() {
        let config = DeferredLoadingConfig {
            default_policy: PromptLoadingPolicy::Preloaded,
            ..DeferredLoadingConfig::default()
        };

        assert_eq!(
            resolve_tool_policy("mcp__docs__search", &config),
            PromptLoadingPolicy::Deferred
        );
    }

    #[test]
    fn tool_search_reports_available_and_not_found() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();
        loaded.mark_loaded("session-1", "web_search");

        let result = execute_tool_search(
            "session-1",
            "select:web_search,missing",
            &tools(),
            &mut loaded,
            &config,
        )
        .expect("partial success should return summary");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec!["web_search".to_string()],
                not_found: vec!["missing".to_string()],
            }
        );
        assert!(
            result
                .summary()
                .contains("Already available 1 tool(s): web_search")
        );
        assert!(result.summary().contains("Not found: missing"));
    }

    #[test]
    fn tool_search_errors_when_all_requested_tools_are_unknown() {
        let mut loaded = LoadedDeferredTools::default();
        let err = execute_tool_search(
            "session-1",
            "select:imaginary",
            &tools(),
            &mut loaded,
            &DeferredLoadingConfig::default(),
        )
        .expect_err("all unknown tools should error");

        assert_eq!(
            err,
            "Not found: imaginary. Only request exact tool names from the available tools list."
        );
    }

    #[test]
    fn tool_search_does_not_resolve_removed_code_search_alias() {
        let mut loaded = LoadedDeferredTools::default();
        let tools = vec![
            tool("read", "Read a file."),
            tool("ToolSearch", "Search available tools."),
        ];

        let err = execute_tool_search(
            "session-1",
            "select:semble",
            &tools,
            &mut loaded,
            &DeferredLoadingConfig::default(),
        )
        .expect_err("removed code_search alias should not resolve");

        assert_eq!(
            err,
            "Not found: semble. Only request exact tool names from the available tools list."
        );
    }

    #[test]
    fn loaded_tools_are_session_scoped_and_listed_deterministically() {
        let mut loaded = LoadedDeferredTools::default();
        loaded.mark_loaded("s1", "skill");
        loaded.mark_loaded("s1", "web_search");
        loaded.mark_loaded("s2", "fetch_url");

        assert_eq!(
            loaded.list_loaded("s1"),
            BTreeSet::from(["skill".to_string(), "web_search".to_string()])
        );
        assert_eq!(
            loaded.list_loaded("s2"),
            BTreeSet::from(["fetch_url".to_string()])
        );
    }

    #[test]
    fn prompt_metrics_report_default_preloaded_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());
        let prompt = assemble_deferred_tool_prompt(&tools(), &BTreeSet::new(), &config);

        assert_eq!(prompt.metrics.exposed_tool_count, 7);
        assert_eq!(prompt.metrics.hidden_tool_count, 0);
        assert_eq!(prompt.metrics.loaded_deferred_tool_count, 0);
        assert_eq!(prompt.metrics.deferred_reminder_tokens, 0);
        assert_eq!(prompt.metrics.estimated_tokens_saved, 0);
        assert_eq!(
            prompt.metrics.estimated_net_tool_context_tokens,
            prompt.metrics.exposed_tool_schema_tokens
        );
    }
}
