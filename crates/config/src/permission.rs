//! Typed permission-policy configuration.

use serde::Deserialize;
use serde::Serialize;

/// Permission-policy configuration loaded from the `[permission]` TOML section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PermissionConfig {
    /// Rules evaluated by the permission-policy runtime in declaration order.
    pub rules: Vec<PermissionRule>,
    /// Behavior when no rule or prior decision resolves a tool call.
    #[serde(rename = "default_mode")]
    pub prompt_policy: PromptPolicy,
    /// Default sandbox profile used when starting new sessions.
    ///
    /// This corresponds to the global `[permission].sandbox_profile` TOML key.
    /// When set, it overrides the sandbox profile implied by the
    /// permission preset (`off` disables the OS sandbox).
    #[serde(default)]
    pub sandbox_profile: Option<String>,
}

/// A single permission-policy rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// The action taken when this rule matches.
    #[serde(default)]
    pub action: RuleAction,
    /// The tool category that this rule applies to.
    #[serde(default)]
    pub tool: ToolFilter,
    /// An optional glob or domain pattern for the selected tool category.
    pub pattern: Option<String>,
    /// How to interpret `pattern`.
    #[serde(default)]
    pub pattern_mode: PatternMode,
}

/// Selects whether a rule pattern matches a glob or a URL host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PatternMode {
    /// Match the target with a glob pattern.
    #[default]
    Glob,
    /// Match the URL host instead of the complete target.
    Domain,
}

/// Action to take when a permission rule matches.
///
/// The default is deny so an omitted `action` cannot silently make a rule
/// permissive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    #[default]
    Deny,
    Ask,
}

/// Tool category used to filter a permission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolFilter {
    /// Match every tool category.
    #[default]
    Any,
    Bash,
    Edit,
    Read,
    Grep,
    Mcp,
    WebFetch,
    WebSearch,
}

/// Default behavior for permission requests that no rule resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptPolicy {
    /// Ask the user to approve the request.
    #[default]
    Ask,
    /// Deny the request without prompting.
    Deny,
    /// Send unresolved requests through the automatic reviewer.
    Auto,
}
