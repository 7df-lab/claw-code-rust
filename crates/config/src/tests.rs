use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use devo_protocol::CollaborationMode;
use devo_protocol::PermissionPreset;
use pretty_assertions::assert_eq;

use super::AppConfig;
use super::AppConfigLoader;
use super::AppConfigStore;
use super::CommandHookConfig;
use super::ExperimentalConfig;
use super::FileSystemAppConfigLoader;
use super::HookCommandConfig;
use super::HookEvent;
use super::HookMatcherConfig;
use super::HookShell;
use super::HooksConfig;
use super::LogRotation;
use super::LoggingConfig;
use super::McpOutputLimits;
use super::McpRootsPolicy;
use super::McpServerId;
use super::McpServerRecord;
use super::McpStartupPolicy;
use super::McpTransportConfig;
use super::McpTrustPolicy;
use super::ModelBindingConfig;
use super::ModelOverrideConfig;
use super::OAuthCredentialsStoreMode;
use super::PatternMode;
use super::PermissionConfig;
use super::PermissionRule;
use super::ProjectConfig;
use super::PromptPolicy;
use super::ProviderConfigSection;
use super::ProviderDefaultsConfig;
use super::ProviderHttpConfig;
use super::ProviderVendorConfig;
use super::RuleAction;
use super::SummaryModelSelection;
use super::ToolFilter;
use super::ToolsConfig;
use super::UpdatesConfig;
use crate::BundledSkillsConfig;
use crate::SkillsConfig;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::TruncationPolicyConfig;

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("devo-{name}-{nanos}"));
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

#[test]
fn loader_merges_user_project_and_cli_layers() {
    let root = unique_temp_dir("config-merge");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
        "default_model = 'ignored'\n[anthropic]\nmodel = 'also-ignored'\n[context]\npreserve_recent_turns = 5\n[logging]\nlevel = 'debug'\n[logging.file]\nmax_files = 30\n",
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "enable_auxiliary_model = true\nproject_root_markers = ['.git', 'Cargo.toml']\n[context]\nauto_compact_percent = 80\n[logging]\njson = true\n[logging.file]\ndirectory = 'diagnostics'\nfilename_prefix = 'agent'\n[skills]\nenabled = true\nworkspace_roots = ['project-skills']\nwatch_for_changes = false\n",
    )
    .expect("write project config");

    let cli_overrides: toml::Value = r#"
summary_model = "UseAxiliaryModel"
project_root_markers = [".workspace"]

[server]
listen = ["stdio://"]

[logging]
level = "trace"

[logging.file]
directory = "cli-logs"
rotation = "Hourly"
max_files = 2

[skills]
enabled = false
user_roots = ["custom-user-skills"]

[updates]
enabled = false
check_interval_hours = 48
"#
    .parse()
    .expect("parse cli overrides");

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config,
        AppConfig {
            summary_model: SummaryModelSelection::UseAxiliaryModel,
            server: super::ServerConfig {
                listen: vec!["stdio://".into()],
                max_connections: 32,
                event_buffer_size: 1024,
                idle_session_timeout_secs: 1800,
                persist_ephemeral_sessions: false,
                auth: Default::default(),
            },
            logging: LoggingConfig {
                level: "trace".into(),
                json: true,
                redact_secrets_in_logs: true,
                file: super::LoggingFileConfig {
                    directory: Some(PathBuf::from("cli-logs")),
                    filename_prefix: "agent".into(),
                    rotation: LogRotation::Hourly,
                    max_files: 2,
                },
            },
            skills: SkillsConfig {
                enabled: false,
                user_roots: vec![PathBuf::from("custom-user-skills")],
                workspace_roots: vec![PathBuf::from("project-skills")],
                watch_for_changes: false,
                bundled: Some(BundledSkillsConfig { enabled: true }),
                include_instructions: Some(true),
                config: Vec::new(),
            },
            experimental: ExperimentalConfig::default(),
            mcp_oauth_credentials_store: Some(OAuthCredentialsStoreMode::default()),
            mcp: super::McpHostConfig::default(),
            mcp_servers: BTreeMap::new(),
            mcp_runtime: super::McpConfig::default(),
            tools: ToolsConfig::default(),
            hooks: HooksConfig::default(),
            permission: PermissionConfig::default(),
            provider: ProviderConfigSection::default(),
            provider_http: super::ProviderHttpConfig::default(),
            updates: UpdatesConfig {
                enabled: false,
                check_on_startup: true,
                check_interval_hours: 48,
            },
            project_root_markers: vec![".workspace".into()],
            projects: BTreeMap::new(),
            compaction_token_limit: None,
            default_collaboration_mode: CollaborationMode::Build,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_defaults_permission_config_when_section_is_absent() {
    let root = unique_temp_dir("permission-default");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");

    let config = FileSystemAppConfigLoader::new(home)
        .load(None)
        .expect("load config");

    assert_eq!(config.permission, PermissionConfig::default());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_permission_rules_and_auto_default_mode() {
    let root = unique_temp_dir("permission-rules");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[permission]
default_mode = "auto"

[[permission.rules]]
action = "allow"
tool = "bash"
pattern = "git *"

[[permission.rules]]
action = "deny"
tool = "edit"
pattern = "**/.env"

[[permission.rules]]
action = "ask"
tool = "web_fetch"
pattern = "example.com"
pattern_mode = "domain"

[[permission.rules]]
tool = "read"
"#,
    )
    .expect("write user config");

    let config = FileSystemAppConfigLoader::new(home)
        .load(None)
        .expect("load config");

    assert_eq!(
        config.permission,
        PermissionConfig {
            rules: vec![
                PermissionRule {
                    action: RuleAction::Allow,
                    tool: ToolFilter::Bash,
                    pattern: Some("git *".to_string()),
                    pattern_mode: PatternMode::Glob,
                },
                PermissionRule {
                    action: RuleAction::Deny,
                    tool: ToolFilter::Edit,
                    pattern: Some("**/.env".to_string()),
                    pattern_mode: PatternMode::Glob,
                },
                PermissionRule {
                    action: RuleAction::Ask,
                    tool: ToolFilter::WebFetch,
                    pattern: Some("example.com".to_string()),
                    pattern_mode: PatternMode::Domain,
                },
                PermissionRule {
                    action: RuleAction::Deny,
                    tool: ToolFilter::Read,
                    pattern: None,
                    pattern_mode: PatternMode::Glob,
                },
            ],
            prompt_policy: PromptPolicy::Auto,
            sandbox_profile: None,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_permission_sandbox_profile() {
    let root = unique_temp_dir("permission-sandbox-profile");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[permission]
sandbox_profile = "off"
"#,
    )
    .expect("write user config");

    let config = FileSystemAppConfigLoader::new(home)
        .load(None)
        .expect("load config");

    assert_eq!(config.permission.sandbox_profile, Some("off".to_string()));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_app_config_serializes_permission_default_mode() {
    let serialized = toml::Value::try_from(AppConfig::default()).expect("serialize config");

    assert_eq!(
        serialized
            .get("permission")
            .and_then(toml::Value::as_table)
            .and_then(|permission| permission.get("default_mode"))
            .and_then(toml::Value::as_str),
        Some("ask")
    );
}

#[test]
fn loader_rejects_invalid_permission_rule_action() {
    let root = unique_temp_dir("permission-invalid-action");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[[permission.rules]]\naction = 'approve'\n",
    )
    .expect("write user config");

    let result = FileSystemAppConfigLoader::new(home).load(None);

    assert!(matches!(result, Err(super::AppConfigError::Parse { .. })));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_preserves_lower_permission_section_when_higher_layer_omits_it() {
    let root = unique_temp_dir("permission-preserve-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
    std::fs::write(
        home.join("config.toml"),
        "[permission]\ndefault_mode = 'deny'\n[[permission.rules]]\naction = 'allow'\ntool = 'web_search'\n",
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "[logging]\nlevel = 'debug'\n",
    )
    .expect("write project config");

    let config = FileSystemAppConfigLoader::new(home)
        .load(Some(&workspace))
        .expect("load config");

    assert_eq!(
        config.permission,
        PermissionConfig {
            rules: vec![PermissionRule {
                action: RuleAction::Allow,
                tool: ToolFilter::WebSearch,
                pattern: None,
                pattern_mode: PatternMode::Glob,
            }],
            prompt_policy: PromptPolicy::Deny,
            sandbox_profile: None,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_replaces_lower_permission_section_when_higher_layer_supplies_it() {
    let root = unique_temp_dir("permission-replace-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
    std::fs::write(
        home.join("config.toml"),
        "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\n",
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "[permission]\ndefault_mode = 'deny'\n[[permission.rules]]\naction = 'ask'\ntool = 'mcp'\n",
    )
    .expect("write project config");

    let config = FileSystemAppConfigLoader::new(home)
        .load(Some(&workspace))
        .expect("load config");

    assert_eq!(
        config.permission,
        PermissionConfig {
            rules: vec![PermissionRule {
                action: RuleAction::Ask,
                tool: ToolFilter::Mcp,
                pattern: None,
                pattern_mode: PatternMode::Glob,
            }],
            prompt_policy: PromptPolicy::Deny,
            sandbox_profile: None,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_cli_overlay_preserves_workspace_permission_when_omitted() {
    let root = unique_temp_dir("permission-cli-preserve-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\npattern = 'git *'\n",
    )
    .expect("write project config");
    let cli_overrides: toml::Value = "[logging]\nlevel = 'trace'\n"
        .parse()
        .expect("parse cli overrides");

    let config = FileSystemAppConfigLoader::new(home)
        .with_cli_overrides(cli_overrides)
        .load(Some(&workspace))
        .expect("load config");

    assert_eq!(
        config.permission,
        PermissionConfig {
            rules: vec![PermissionRule {
                action: RuleAction::Allow,
                tool: ToolFilter::Bash,
                pattern: Some("git *".to_string()),
                pattern_mode: PatternMode::Glob,
            }],
            prompt_policy: PromptPolicy::Auto,
            sandbox_profile: None,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_cli_overlay_replaces_workspace_permission_section() {
    let root = unique_temp_dir("permission-cli-replace-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\npattern = 'git *'\n",
    )
    .expect("write project config");
    let cli_overrides: toml::Value = r#"
[permission]
default_mode = "deny"

[[permission.rules]]
action = "ask"
tool = "mcp"
pattern = "deploy"
"#
    .parse()
    .expect("parse cli overrides");

    let config = FileSystemAppConfigLoader::new(home)
        .with_cli_overrides(cli_overrides)
        .load(Some(&workspace))
        .expect("load config");

    assert_eq!(
        config.permission,
        PermissionConfig {
            rules: vec![PermissionRule {
                action: RuleAction::Ask,
                tool: ToolFilter::Mcp,
                pattern: Some("deploy".to_string()),
                pattern_mode: PatternMode::Glob,
            }],
            prompt_policy: PromptPolicy::Deny,
            sandbox_profile: None,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_app_config_includes_disabled_code_search_mcp_server() {
    let mcp = AppConfig::default().mcp_runtime;
    let server = mcp
        .servers
        .iter()
        .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
        .expect("bundled code_search server");
    assert!(!server.enabled);
}

#[test]
fn default_app_config_disables_server_auth() {
    assert_eq!(
        AppConfig::default().server.auth,
        super::ServerAuthConfig {
            enabled: false,
            method_id: "agent-login".to_string(),
            name: "Agent login".to_string(),
            description: None,
            logout: true,
        }
    );
}

#[test]
fn loader_reads_server_auth_config() {
    let root = unique_temp_dir("config-server-auth");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[server.auth]
enabled = true
method_id = "company-login"
name = "Company login"
description = "Sign in with company credentials"
logout = false
"#,
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.server.auth,
        super::ServerAuthConfig {
            enabled: true,
            method_id: "company-login".to_string(),
            name: "Company login".to_string(),
            description: Some("Sign in with company credentials".to_string()),
            logout: false,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_empty_server_auth_method_id_when_enabled() {
    let root = unique_temp_dir("config-server-auth-empty-method");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[server.auth]\nenabled = true\nmethod_id = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    match result {
        Err(super::AppConfigError::Validation { message }) => assert_eq!(
            message,
            "server.auth.method_id must not be empty when server auth is enabled"
        ),
        other => panic!("expected server auth validation error, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_empty_server_auth_name_when_enabled() {
    let root = unique_temp_dir("config-server-auth-empty-name");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[server.auth]\nenabled = true\nname = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    match result {
        Err(super::AppConfigError::Validation { message }) => assert_eq!(
            message,
            "server.auth.name must not be empty when server auth is enabled"
        ),
        other => panic!("expected server auth validation error, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_ignores_legacy_experimental_code_search_keys() {
    let root = unique_temp_dir("config-experimental-legacy");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[experimental]\ncode-search = true\ncode_search = false\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(config.experimental, ExperimentalConfig::default());
    let server = config
        .mcp_runtime
        .servers
        .iter()
        .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
        .expect("bundled code_search server");
    assert!(!server.enabled);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_ensures_bundled_code_search_mcp_when_servers_list_is_empty() {
    let root = unique_temp_dir("config-bundled-mcp-ensure");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[mcp]
auto_start = true
"#,
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(config.mcp_runtime.servers.len(), 1);
    assert_eq!(
        config.mcp_runtime.servers[0].id.0,
        super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID
    );
    assert!(!config.mcp_runtime.servers[0].enabled);

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-MCP-002
/// Verifies: enabling bundled code_search materializes it into user config.toml.
#[test]
fn set_mcp_server_enabled_materializes_bundled_code_search() {
    let root = unique_temp_dir("config-bundled-mcp-enable");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[mcp]
auto_start = true
"#,
    )
    .expect("write user config");

    let config_file = home.join("config.toml");
    let mut store = AppConfigStore::load(home, /*workspace_root*/ None).expect("load store");
    assert!(
        !std::fs::read_to_string(&config_file)
            .expect("read user config")
            .contains("code_search")
    );

    store
        .set_mcp_server_enabled(
            super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID,
            /*enabled*/ true,
        )
        .expect("enable bundled code_search");

    let server = store
        .mcp_servers()
        .iter()
        .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
        .expect("bundled code_search server");
    assert!(server.enabled);

    let user_config = std::fs::read_to_string(&config_file).expect("read user config");
    assert!(user_config.contains("code_search"));
    assert!(user_config.contains("devo-code-search-mcp"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_hook_command_config() {
    let root = unique_temp_dir("config-hooks");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[[hooks.PreToolUse]]
matcher = "exec_command"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "hooks/pre_tool.sh"
shell = "powershell"
timeout = 5
statusMessage = "Checking tool use"
"#,
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.hooks,
        HooksConfig(BTreeMap::from([(
            HookEvent::PreToolUse,
            vec![HookMatcherConfig {
                matcher: Some("exec_command".to_string()),
                hooks: vec![HookCommandConfig::Command(CommandHookConfig {
                    command: "hooks/pre_tool.sh".to_string(),
                    shell: Some(HookShell::PowerShell),
                    condition: None,
                    timeout: Some(5),
                    status_message: Some("Checking tool use".to_string()),
                    once: None,
                    async_hook: None,
                    async_rewake: None,
                })],
            }],
        )]))
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: provider HTTP proxy settings and provider header fields follow user/workspace merge precedence.
#[test]
fn loader_merges_provider_sections_with_provider_overlay_rules() {
    let root = unique_temp_dir("config-provider-merge");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
        r#"
[provider_http]
proxy_url = "http://user-proxy.example:8080"

[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
headers = '{"X-User":"yes"}'
wire_apis = ["openai_responses"]

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        r#"
[provider_http]
proxy_url = "http://workspace-proxy.example:8080"

[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
request_model = "project/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write project config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.provider_http,
        ProviderHttpConfig {
            proxy_url: Some("http://workspace-proxy.example:8080".to_string()),
            no_proxy: None,
        }
    );
    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "Project Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: Some(r#"{"X-User":"yes"}"#.to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: true,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "project-model".to_string(),
                    provider: "main".to_string(),
                    request_model: "project/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: omitted defaulted provider fields in a higher-priority partial overlay do not overwrite lower-priority values.
#[test]
fn loader_provider_overlay_preserves_absent_defaulted_provider_fields() {
    let root = unique_temp_dir("config-provider-defaulted-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
        r#"
[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
headers = '{"X-User":"yes"}'
wire_apis = ["openai_responses"]
enabled = false

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
enabled = false
"#,
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        r#"
[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
request_model = "project/model"
"#,
    )
    .expect("write project config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "Project Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: Some(r#"{"X-User":"yes"}"#.to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: false,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "project-model".to_string(),
                    provider: "main".to_string(),
                    request_model: "project/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    enabled: false,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_merges_model_overrides_field_by_field_across_layers() {
    let root = unique_temp_dir("config-model-overrides-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
        r#"
[model.grok-4]
display_name = "User Grok"
description = "User description"
context_window = 128000
temperature = 0.4
provider = "openai_chat_completions"
default_reasoning_effort = "medium"
truncation_policy = { mode = "tokens", limit = 8000 }
"#,
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        r#"
[model.grok-4]
description = "Workspace description"
context_window = 192000
top_p = 0.9
"#,
    )
    .expect("write workspace config");
    let cli_overrides: toml::Value = r#"
[model.grok-4]
display_name = "CLI Grok"
temperature = 0.2

[model.grok-4-mini]
display_name = "Grok 4 Mini"
max_tokens = 4096
"#
    .parse()
    .expect("parse cli overrides");

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.provider.model_overrides,
        BTreeMap::from([
            (
                "grok-4".to_string(),
                ModelOverrideConfig {
                    display_name: Some("CLI Grok".to_string()),
                    description: Some("Workspace description".to_string()),
                    context_window: Some(192_000),
                    temperature: Some(0.2),
                    top_p: Some(0.9),
                    provider: Some(ProviderWireApi::OpenAIChatCompletions),
                    default_reasoning_effort: Some(ReasoningEffort::Medium),
                    truncation_policy: Some(TruncationPolicyConfig::tokens(8_000)),
                    ..ModelOverrideConfig::default()
                },
            ),
            (
                "grok-4-mini".to_string(),
                ModelOverrideConfig {
                    display_name: Some("Grok 4 Mini".to_string()),
                    max_tokens: Some(4_096),
                    ..ModelOverrideConfig::default()
                },
            ),
        ])
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: CLI provider overrides participate in the same provider merge precedence as other CLI config.
#[test]
fn loader_applies_cli_provider_overrides_to_provider_section() {
    let root = unique_temp_dir("config-provider-cli-overlay");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");

    std::fs::write(
        home.join("config.toml"),
        r#"
[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
wire_apis = ["openai_responses"]

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write user config");
    let cli_overrides: toml::Value = r#"
[providers.main]
name = "CLI Provider"
enabled = false

[model_bindings.main]
model_slug = "cli-model"
provider = "main"
request_model = "cli/model"
invocation_method = "openai_responses"
enabled = false
"#
    .parse()
    .expect("parse cli overrides");

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "CLI Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: None,
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: false,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "cli-model".to_string(),
                    provider: "main".to_string(),
                    request_model: "cli/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    enabled: false,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: provider upsert persists custom provider header JSON in user config and projections.
#[test]
fn provider_upsert_writes_user_config_when_workspace_is_active() {
    let root = unique_temp_dir("provider-upsert-user");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    let mut store = AppConfigStore::load(home.clone(), Some(&workspace)).expect("load store");
    let written_provider = store
        .upsert_provider_vendor(
            "openrouter".to_string(),
            ProviderVendor {
                name: "openrouter".to_string(),
                base_url: Some("https://openrouter.ai/api/v1".to_string()),
                credential: None,
                headers: Some(r#"{"X-Devo":"yes"}"#.to_string()),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            },
            Some(ProviderModelBinding {
                binding_id: "qwen-openrouter".to_string(),
                model_slug: "qwen".to_string(),
                provider: "openrouter".to_string(),
                request_model: "qwen/qwen3".to_string(),
                display_name: Some("Qwen".to_string()),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: Some("medium".to_string()),
                enabled: true,
            }),
            Some("qwen-openrouter".to_string()),
            Some("sk-test".to_string()),
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(home.join("config.toml")).expect("user config");
    let workspace_config = workspace.join(".devo").join("config.toml");
    let document: toml::Value = toml::from_str(&user_config).expect("parse user config");

    assert!(user_config.contains("[providers.openrouter]"));
    assert!(user_config.contains("[model_bindings.qwen-openrouter]"));
    assert!(user_config.contains("model_binding = \"qwen-openrouter\""));
    assert!(document.get("model").is_none());
    assert_eq!(
        document["providers"]["openrouter"]["headers"].as_str(),
        Some(r#"{"X-Devo":"yes"}"#)
    );
    assert_eq!(
        written_provider.headers,
        Some(r#"{"X-Devo":"yes"}"#.to_string())
    );
    assert_eq!(
        store.provider_vendors()[0].headers,
        Some(r#"{"X-Devo":"yes"}"#.to_string())
    );
    assert!(!workspace_config.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn provider_upsert_migrates_legacy_model_name_to_request_model() {
    let root = unique_temp_dir("provider-upsert-existing-binding");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[defaults]
model_binding = "deepseek-v4-flash-deepseek"

[providers.Deepseek]
base_url = "https://api.deepseek.com"
credential = "deepseek_api_key"
enabled = true
name = "Deepseek"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-deepseek]
display_name = "deepseek-v4-flash"
enabled = true
invocation_method = "openai_chat_completions"
model_name = "deepseek-v4-flash"
custom_binding_key = "preserved"
model_slug = "deepseek-v4-flash"
provider = "Deepseek"
"#,
    )
    .expect("write user config");

    let mut store =
        AppConfigStore::load(home.clone(), /*workspace_root*/ None).expect("load store");
    store
        .upsert_provider_vendor(
            "Deepseek".to_string(),
            ProviderVendor {
                name: "Deepseek".to_string(),
                base_url: Some("https://api.deepseek.com".to_string()),
                credential: Some("deepseek_api_key".to_string()),
                headers: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            },
            Some(ProviderModelBinding {
                binding_id: "deepseek-v4-flash-deepseek".to_string(),
                model_slug: "deepseek-v4-flash".to_string(),
                provider: "Deepseek".to_string(),
                request_model: "DeepSeek-V4-Flash".to_string(),
                display_name: Some("DeepSeek-V4-Flash".to_string()),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: None,
                enabled: true,
            }),
            Some("deepseek-v4-flash-deepseek".to_string()),
            /*api_key*/ None,
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(home.join("config.toml")).expect("user config");
    let document: toml::Value = toml::from_str(&user_config).expect("parse user config");
    let binding = &document["model_bindings"]["deepseek-v4-flash-deepseek"];

    assert_eq!(binding["model_slug"].as_str(), Some("deepseek-v4-flash"));
    assert_eq!(binding["request_model"].as_str(), Some("DeepSeek-V4-Flash"));
    assert_eq!(binding.get("model_name"), None);
    assert_eq!(binding["custom_binding_key"].as_str(), Some("preserved"));
    assert_eq!(binding["display_name"].as_str(), Some("DeepSeek-V4-Flash"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_invalid_logging_file_prefix() {
    let root = unique_temp_dir("config-validation");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[logging.file]\nfilename_prefix = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_duplicate_skill_roots() {
    let root = unique_temp_dir("config-skill-roots");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[skills]\nuser_roots = ['skills', 'skills']\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_project_configs() {
    let root = unique_temp_dir("config-projects");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[projects.\"C:\\\\repo\"]\npermission_preset = 'auto-review'\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.projects,
        BTreeMap::from([(
            "C:\\repo".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::AutoReview),
                sandbox_profile: None,
            },
        )])
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_project_sandbox_profile() {
    let root = unique_temp_dir("config-projects-sandbox");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[projects.\"C:\\\\repo\"]\nsandbox_profile = 'strict'\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.projects,
        BTreeMap::from([(
            "C:\\repo".to_string(),
            ProjectConfig {
                permission_preset: None,
                sandbox_profile: Some("strict".to_string()),
            },
        )])
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_maps_legacy_read_only_preset_to_default() {
    let root = unique_temp_dir("config-legacy-read-only");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[projects.\"C:\\\\repo\"]\npermission_preset = 'read-only'\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.projects,
        BTreeMap::from([(
            "C:\\repo".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::Default),
                sandbox_profile: None,
            },
        )])
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_app_config_enables_startup_update_checks() {
    assert_eq!(
        AppConfig::default().updates,
        UpdatesConfig {
            enabled: true,
            check_on_startup: true,
            check_interval_hours: 24,
        }
    );
}

#[test]
fn loader_rejects_invalid_update_check_interval() {
    let root = unique_temp_dir("config-update-interval");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[updates]\ncheck_interval_hours = 0\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mcp_upsert_remove_enable_round_trip_preserves_unrelated_sections() {
    let root = unique_temp_dir("mcp-upsert-roundtrip");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[updates]
enabled = true
check_on_startup = false
check_interval_hours = 12

[logging]
level = "warn"
"#,
    )
    .expect("write user config");

    let mut store =
        AppConfigStore::load(home.clone(), /*workspace_root*/ None).expect("load store");
    let stdio_record = McpServerRecord {
        id: McpServerId("time".to_string()),
        display_name: "time".to_string(),
        transport: McpTransportConfig::Stdio {
            command: vec![
                "docker".to_string(),
                "run".to_string(),
                "-i".to_string(),
                "--rm".to_string(),
                "mcp/time".to_string(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        },
        startup_policy: McpStartupPolicy::Lazy,
        enabled: true,
        trust_policy: McpTrustPolicy::User,
        allowed_capabilities: Vec::new(),
        roots_policy: McpRootsPolicy::None,
        output_limits: McpOutputLimits::default(),
        auth_ref: None,
    };
    store
        .upsert_mcp_server(stdio_record.clone())
        .expect("upsert stdio");

    let http_record = McpServerRecord {
        id: McpServerId("hello".to_string()),
        display_name: "hello".to_string(),
        transport: McpTransportConfig::StreamableHttp {
            url: "http://localhost:8080/mcp".to_string(),
            auth: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
        },
        startup_policy: McpStartupPolicy::Lazy,
        enabled: true,
        trust_policy: McpTrustPolicy::User,
        allowed_capabilities: Vec::new(),
        roots_policy: McpRootsPolicy::None,
        output_limits: McpOutputLimits::default(),
        auth_ref: None,
    };
    store
        .upsert_mcp_server(http_record.clone())
        .expect("upsert http");

    let sse_record = McpServerRecord {
        id: McpServerId("legacy".to_string()),
        display_name: "legacy".to_string(),
        transport: McpTransportConfig::Sse {
            url: "https://example.com/mcp/sse".to_string(),
            auth: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
        },
        startup_policy: McpStartupPolicy::Lazy,
        enabled: true,
        trust_policy: McpTrustPolicy::User,
        allowed_capabilities: Vec::new(),
        roots_policy: McpRootsPolicy::None,
        output_limits: McpOutputLimits::default(),
        auth_ref: None,
    };
    store
        .upsert_mcp_server(sse_record.clone())
        .expect("upsert sse");

    let user_config = std::fs::read_to_string(home.join("config.toml")).expect("read user config");
    assert!(user_config.contains("check_on_startup"));
    assert!(user_config.contains("level"));
    let server_ids: Vec<&str> = store
        .mcp_servers()
        .iter()
        .map(|server| server.id.0.as_str())
        .collect();
    assert!(server_ids.contains(&"time"));
    assert!(server_ids.contains(&"hello"));
    assert!(server_ids.contains(&"legacy"));
    assert!(server_ids.contains(&super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID));
    assert_eq!(
        store
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server"),
        &stdio_record
    );

    store
        .set_mcp_server_enabled("time", /*enabled*/ false)
        .expect("disable");
    assert!(
        !store
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server")
            .enabled
    );

    store.remove_mcp_server("hello").expect("remove hello");
    assert!(
        store
            .mcp_servers()
            .iter()
            .all(|server| server.id.0 != "hello")
    );

    let reloaded = AppConfigStore::load(home, /*workspace_root*/ None).expect("reload");
    let reloaded_ids: Vec<&str> = reloaded
        .mcp_servers()
        .iter()
        .map(|server| server.id.0.as_str())
        .collect();
    assert!(reloaded_ids.contains(&"time"));
    assert!(reloaded_ids.contains(&"legacy"));
    assert!(reloaded_ids.contains(&super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID));
    assert!(!reloaded_ids.contains(&"hello"));
    assert!(
        !reloaded
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server")
            .enabled
    );

    let _ = std::fs::remove_dir_all(root);
}
