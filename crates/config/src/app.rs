use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use devo_protocol::CollaborationMode;
use devo_protocol::PermissionPreset;
use serde::Deserialize;
use serde::Serialize;

use devo_util_git::get_git_repo_root;
use devo_util_paths::APP_CONFIG_DIR_NAME;
use devo_util_paths::APP_CONFIG_FILE_NAME;
use devo_util_paths::FileSystemConfigPathResolver;

use crate::AppConfigError;
use crate::ExperimentalConfig;
use crate::HooksConfig;
use crate::LogRotation;
use crate::LoggingConfig;
use crate::LoggingFileConfig;
use crate::McpConfig;
use crate::McpHostConfig;
use crate::McpServerId;
use crate::McpServerRecordToml;
use crate::OAuthCredentialsStoreMode;
use crate::PermissionConfig;
use crate::ProviderConfigError;
use crate::ProviderConfigFile;
use crate::ProviderConfigSection;
use crate::ProviderHttpConfig;
use crate::ServerConfig;
use crate::SkillsConfig;
use crate::ToolsConfig;
use crate::non_empty_string;
use crate::provider::migrate_legacy_provider_config_on_startup;
use crate::read_provider_catalog_config;
use crate::read_provider_config_document;
use crate::remove_user_auth_credential;
use crate::write_atomic;
use crate::write_provider_catalog_config;

mod mcp_store;
#[path = "provider_connection.rs"]
mod provider_connection;

pub use mcp_store::mcp_server_record_for_cli;

/// Stores the fully normalized runtime configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    /// The policy that selects which model should generate context summaries.
    pub summary_model: SummaryModelSelection,
    /// Transport and server runtime defaults.
    pub server: ServerConfig,
    /// Logging and redaction behavior for diagnostics.
    pub logging: LoggingConfig,
    /// Skill discovery roots and behavior.
    pub skills: SkillsConfig,
    /// Experimental feature gates.
    #[serde(default)]
    pub experimental: ExperimentalConfig,
    /// Preferred backend for storing MCP OAuth credentials.
    /// keyring: Use an OS-specific keyring service.
    /// file: Use a file in the Devo home directory.
    /// auto (default): Use the OS-specific keyring service if available, otherwise use a file.
    #[serde(default)]
    pub mcp_oauth_credentials_store: Option<OAuthCredentialsStoreMode>,
    /// MCP host settings stored under `[mcp]`.
    #[serde(default)]
    pub mcp: McpHostConfig,
    /// MCP server records stored under `[mcp_servers.<server_id>]`.
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpServerRecordToml>,
    /// Normalized MCP runtime configuration built from `mcp` + `mcp_servers`.
    #[serde(skip, default)]
    pub mcp_runtime: McpConfig,
    /// Tool-specific runtime configuration.
    #[serde(default, skip_serializing_if = "ToolsConfig::is_empty")]
    pub tools: ToolsConfig,
    /// External lifecycle hooks keyed by event name.
    #[serde(default, skip_serializing_if = "HooksConfig::is_empty")]
    pub hooks: HooksConfig,
    /// Configured rules and default behavior for tool permission requests.
    #[serde(default)]
    pub permission: PermissionConfig,
    /// Provider, model, and active model defaults.
    #[serde(flatten)]
    pub provider: ProviderConfigSection,
    /// Effective standalone provider/model JSON configuration.
    #[serde(skip, default)]
    pub provider_catalog: ProviderConfigFile,
    /// HTTP transport settings shared by model-provider requests.
    #[serde(default, skip_serializing_if = "ProviderHttpConfig::is_empty")]
    pub provider_http: ProviderHttpConfig,
    /// Startup update-check defaults.
    pub updates: UpdatesConfig,
    /// Marker names used to discover the project root for instruction discovery.
    /// These values map to `InstructionDiscoveryConfig::root_markers`, such as ['.git'].
    pub project_root_markers: Vec<String>,
    /// User-level settings remembered per project key.
    pub projects: BTreeMap<String, ProjectConfig>,
    /// Global absolute auto-compaction token limit.
    ///
    /// When set, sessions clamp this value to each model's `context_window`.
    /// When unset, sessions use the model effective context window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_token_limit: Option<u64>,
    /// Default collaboration/input mode (Build or Plan) for new sessions.
    #[serde(default)]
    pub default_collaboration_mode: CollaborationMode,
}

/// Settings remembered for one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Permission preset to use when starting new sessions for this project.
    pub permission_preset: Option<PermissionPreset>,
    /// Sandbox profile to use when starting new sessions for this project.
    /// Built-in values are `workspace`, `devbox`, `read-only`, `strict`, and
    /// `off`; any other value names a custom profile from `sandbox.toml`.
    pub sandbox_profile: Option<String>,
}

/// Controls how the CLI checks for new releases at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatesConfig {
    /// Whether update checking is enabled at all.
    pub enabled: bool,
    /// Whether the CLI should check for updates during startup.
    pub check_on_startup: bool,
    /// Minimum number of hours between network checks.
    pub check_interval_hours: u64,
}

/// Selects the model used for summary generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryModelSelection {
    /// Use the active turn model for compaction summaries.
    UseTurnModel,
    /// Use a separately configured auxiliary model for compaction summaries.
    UseAxiliaryModel,
}

/// Loads the effective application configuration from the supported config sources.
///
/// The effective config must be resolved from exactly three sources, in this
/// priority order:
///
/// 1. command-line startup arguments
/// 2. `<workspace>/.devo/config.toml` and `providers.json` overlays for the
///    currently opened project
/// 3. the user config files under the configured config directory
///
/// When the same field appears in multiple sources, the higher-priority source
/// must win.
pub trait AppConfigLoader {
    /// Loads and validates the effective application config for an optional workspace.
    ///
    /// The user config directory may be supplied explicitly by the process
    /// environment. When it is not explicitly configured, the loader falls back
    /// to the default home-directory-based config location.
    fn load(&self, workspace_root: Option<&Path>) -> Result<AppConfig, AppConfigError>;
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            summary_model: SummaryModelSelection::UseTurnModel,
            server: ServerConfig {
                listen: Vec::new(),
                max_connections: 32,
                event_buffer_size: 1024,
                idle_session_timeout_secs: 1800,
                persist_ephemeral_sessions: false,
                auth: Default::default(),
            },
            logging: LoggingConfig {
                level: "info".into(),
                json: false,
                redact_secrets_in_logs: true,
                file: LoggingFileConfig {
                    directory: None,
                    filename_prefix: "devo".into(),
                    rotation: LogRotation::Daily,
                    max_files: 14,
                },
            },
            skills: SkillsConfig::default(),
            experimental: ExperimentalConfig::default(),
            mcp_oauth_credentials_store: Some(OAuthCredentialsStoreMode::default()),
            mcp: McpHostConfig::default(),
            mcp_servers: BTreeMap::new(),
            mcp_runtime: McpConfig::default(),
            tools: ToolsConfig::default(),
            hooks: HooksConfig::default(),
            permission: PermissionConfig::default(),
            provider: ProviderConfigSection::default(),
            provider_catalog: ProviderConfigFile::default(),
            provider_http: ProviderHttpConfig::default(),
            updates: UpdatesConfig {
                enabled: true,
                check_on_startup: true,
                check_interval_hours: 24,
            },
            project_root_markers: vec![".git".into()],
            projects: BTreeMap::new(),
            compaction_token_limit: None,
            default_collaboration_mode: CollaborationMode::Build,
        }
    }
}

/// Shared runtime view of the effective app configuration.
///
/// Server code should depend on this store instead of carrying separate paths
/// or provider-only stores. Domain-specific mutation helpers update the durable
/// file-backed config and refresh the effective app config afterward.
#[derive(Debug, Clone)]
pub struct AppConfigStore {
    loader: FileSystemAppConfigLoader,
    workspace_root: Option<PathBuf>,
    user_config_file: PathBuf,
    config: AppConfig,
}

impl AppConfigStore {
    /// Loads user/workspace config into a single effective app config store.
    pub fn load(
        user_config_dir: PathBuf,
        workspace_root: Option<&Path>,
    ) -> Result<Self, AppConfigError> {
        let resolver = FileSystemConfigPathResolver::new(user_config_dir.clone());
        let user_config_file = resolver.user_config_file();
        let loader = FileSystemAppConfigLoader::new(user_config_dir);
        let config = loader.load(workspace_root)?;

        Ok(Self {
            loader,
            workspace_root: workspace_root.map(Path::to_path_buf),
            user_config_file,
            config,
        })
    }

    /// Returns the effective app config currently visible to the runtime.
    pub fn effective_config(&self) -> &AppConfig {
        &self.config
    }

    pub fn user_config_dir(&self) -> &Path {
        self.user_config_file
            .parent()
            .expect("user config file should have a parent directory")
    }

    /// Returns the standalone user provider/model configuration path.
    pub fn user_provider_config_file(&self) -> PathBuf {
        self.user_config_dir()
            .join(crate::PROVIDER_CONFIG_FILE_NAME)
    }

    /// Returns provider ids that have a user-created Connection.
    ///
    /// Built-in directory entries are intentionally excluded. A provider is
    /// connected when it exists in the user provider overlay (or in the old
    /// TOML provider section that is still being migrated).
    pub fn provider_connection_ids(&self) -> anyhow::Result<Vec<String>> {
        let target_config_file = self.user_provider_config_file();
        Ok(read_provider_catalog_config(&target_config_file)?
            .providers
            .into_keys()
            .collect())
    }

    /// Disconnects a user-created provider Connection.
    ///
    /// This removes the user's provider overlay, model entries rooted at that
    /// provider, and an unshared credential from auth.json. Built-in catalog
    /// entries remain available for a future connection.
    pub fn disconnect_provider(&mut self, provider_id: &str) -> anyhow::Result<()> {
        let provider_id = non_empty_string(provider_id)
            .ok_or_else(|| anyhow::anyhow!("provider id must not be empty"))?;
        let target_config_file = self.user_provider_config_file();
        let mut config = read_provider_catalog_config(&target_config_file)?;

        let Some(removed_provider) = config.providers.remove(&provider_id) else {
            return Ok(());
        };
        let credential_id = removed_provider.credential;
        let provider_prefix = format!("{provider_id}/");
        if config
            .model
            .as_deref()
            .is_some_and(|model| model == provider_id || model.starts_with(&provider_prefix))
        {
            config.model = None;
        }
        if config
            .small_model
            .as_deref()
            .is_some_and(|model| model == provider_id || model.starts_with(&provider_prefix))
        {
            config.small_model = None;
        }

        let remaining_credentials = config
            .providers
            .values()
            .filter_map(|provider| provider.credential.as_deref())
            .collect::<BTreeSet<_>>();
        write_provider_catalog_config(&target_config_file, &config)?;
        migrate_legacy_provider_config_file(&self.user_config_file)?;
        if let Some(credential_id) = credential_id
            && !remaining_credentials.contains(credential_id.as_str())
        {
            remove_user_auth_credential(self.user_config_dir(), &credential_id)
                .map_err(|error| anyhow::anyhow!(error))?;
        }

        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    pub fn set_model_config_option(&mut self, config_id: &str, value: &str) -> anyhow::Result<()> {
        let value = value.trim();
        if value.is_empty() {
            anyhow::bail!("model config value must not be empty");
        }

        let target_config_file = self.user_provider_config_file();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut config = read_provider_catalog_config(&target_config_file)?;

        match config_id {
            "model" => {
                let (provider_id, model_id) = value
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!("model must use `provider/model` form"))?;
                let provider = config
                    .providers
                    .get(provider_id)
                    .ok_or_else(|| anyhow::anyhow!("provider `{provider_id}` does not exist"))?;
                if provider.enabled == Some(false)
                    || provider
                        .models
                        .get(model_id)
                        .is_some_and(|model| model.enabled == Some(false))
                {
                    anyhow::bail!("model `{value}` is disabled");
                }
                config
                    .providers
                    .get_mut(provider_id)
                    .expect("provider was checked above")
                    .models
                    .entry(model_id.to_string())
                    .or_default();
                config.model = Some(value.to_string());
            }
            "thought_level" => {
                config.reasoning_effort = Some(value.to_string());
            }
            _ => {
                anyhow::bail!("unknown model config option `{config_id}`");
            }
        }

        write_provider_catalog_config(&target_config_file, &config)?;
        migrate_legacy_provider_config_file(&self.user_config_file)?;

        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    /// Persists the global compaction token limit and refreshes effective config.
    pub fn set_compaction_token_limit(&mut self, limit: u64) -> anyhow::Result<()> {
        if limit == 0 {
            anyhow::bail!("compaction_token_limit must be at least 1");
        }

        let target_config_file = self.user_config_file.as_path();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut document = read_provider_config_document(target_config_file)?;
        let document = ensure_toml_table(&mut document);
        let limit_i64 = i64::try_from(limit).map_err(|_| {
            anyhow::anyhow!("compaction_token_limit is too large to store in config.toml")
        })?;
        document.insert(
            "compaction_token_limit".to_string(),
            toml::Value::Integer(limit_i64),
        );

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;

        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    /// Persists the default collaboration mode and refreshes effective config.
    pub fn set_default_collaboration_mode(
        &mut self,
        mode: CollaborationMode,
    ) -> anyhow::Result<()> {
        let target_config_file = self.user_config_file.as_path();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut document = read_provider_config_document(target_config_file)?;
        let document = ensure_toml_table(&mut document);
        document.insert(
            "default_collaboration_mode".to_string(),
            toml::Value::String(match mode {
                CollaborationMode::Build => "build".to_string(),
                CollaborationMode::Plan => "plan".to_string(),
            }),
        );

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;

        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    /// Persists a path-based skill enablement override in the user config.
    pub fn set_skill_enabled(&mut self, path: PathBuf, enabled: bool) -> anyhow::Result<()> {
        if path.as_os_str().is_empty() {
            anyhow::bail!("skill path must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut document = read_provider_config_document(target_config_file)?;
        let document = ensure_toml_table(&mut document);
        let skills = document
            .entry("skills".to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let skills = ensure_toml_table(skills);
        let config = skills
            .entry("config".to_string())
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        if !config.is_array() {
            *config = toml::Value::Array(Vec::new());
        }

        let path_text = path.display().to_string();
        let entries = config
            .as_array_mut()
            .expect("skills.config should be an array after normalization");
        entries.retain(|entry| {
            entry
                .as_table()
                .and_then(|table| table.get("path"))
                .and_then(toml::Value::as_str)
                != Some(path_text.as_str())
        });

        let mut entry = toml::map::Map::new();
        entry.insert("path".to_string(), toml::Value::String(path_text));
        entry.insert("enabled".to_string(), toml::Value::Boolean(enabled));
        entries.push(toml::Value::Table(entry));

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;

        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }
}

/// Removes provider/model binding tables after they have been migrated to JSON.
pub fn migrate_legacy_provider_config_file(config_file: &Path) -> Result<(), ProviderConfigError> {
    if !config_file.exists() {
        return Ok(());
    }

    let mut document = read_provider_config_document(config_file)?;
    let table = ensure_toml_table(&mut document);
    let mut changed = false;
    for key in [
        "model_provider",
        "model",
        "model_reasoning_effort_selection",
        "providers",
        "model_bindings",
        "model_providers",
    ] {
        changed |= table.remove(key).is_some();
    }
    if let Some(defaults) = table
        .get_mut("defaults")
        .and_then(toml::Value::as_table_mut)
    {
        changed |= defaults.remove("model_binding").is_some();
        if defaults.is_empty() {
            changed |= table.remove("defaults").is_some();
        }
    }
    if changed {
        let data =
            toml::to_string_pretty(&document).map_err(|error| ProviderConfigError::Serialize {
                message: error.to_string(),
            })?;
        write_atomic(config_file, data.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "app_store_tests.rs"]
mod app_store_tests;

impl AppConfig {
    /// Returns true when the merged config contains any provider-era setup.
    pub fn has_provider_configuration(&self) -> bool {
        let provider_catalog = self.provider_catalog_config();
        !provider_catalog.providers.is_empty()
    }

    /// Returns the effective JSON provider/model catalog, with a compatibility
    /// projection for callers that construct only the legacy TOML shape.
    pub fn provider_catalog_config(&self) -> ProviderConfigFile {
        if self.provider_catalog == ProviderConfigFile::default() {
            ProviderConfigFile::from_provider_config_section(&self.provider)
        } else {
            self.provider_catalog.clone()
        }
    }
}

/// Returns the stable key used to remember project-level permission settings.
///
/// Git repositories are keyed by their repository root. Non-git directories fall
/// back to the canonical current working directory when possible.
pub fn project_config_key(cwd: &Path) -> String {
    let root = get_git_repo_root(cwd)
        .or_else(|| cwd.canonicalize().ok())
        .unwrap_or_else(|| cwd.to_path_buf());
    strip_unc_prefix(root).display().to_string()
}

fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.display().to_string();
        if let Some(stripped) = value.strip_prefix("\\\\?\\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

fn read_config_value(path: &Path) -> Result<toml::Value, AppConfigError> {
    let contents = fs::read_to_string(path).map_err(|source| AppConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str::<toml::Value>(&contents).map_err(|source: toml::de::Error| {
        AppConfigError::Parse {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })
}

fn provider_section_from_value(
    path: &Path,
    value: &toml::Value,
) -> Result<ProviderConfigSection, AppConfigError> {
    value
        .clone()
        .try_into()
        .map_err(|source: toml::de::Error| AppConfigError::Parse {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
}

pub(crate) fn ensure_toml_table(
    value: &mut toml::Value,
) -> &mut toml::map::Map<String, toml::Value> {
    if !value.is_table() {
        *value = toml::Value::Table(Default::default());
    }
    value
        .as_table_mut()
        .expect("value should be a TOML table after normalization")
}

/// Filesystem-backed loader for project and user config files, plus CLI overrides.
#[derive(Debug, Clone)]
pub struct FileSystemAppConfigLoader {
    /// The user config directory used to locate `config.toml`.
    ///
    /// This path usually comes from the environment-aware config-path resolver.
    /// If the environment does not override it, the resolver falls back to the
    /// default home-directory-based config location.
    config_folder_home: PathBuf,
    /// Command-line overrides applied on top of file-backed config.
    cli_overrides: toml::Value,
}

impl FileSystemAppConfigLoader {
    /// Creates a filesystem-backed loader rooted at the provided user config directory.
    pub fn new(config_folder_home: PathBuf) -> Self {
        Self {
            config_folder_home,
            cli_overrides: toml::Value::Table(Default::default()),
        }
    }

    /// Returns a loader that applies CLI overrides with the highest priority.
    pub fn with_cli_overrides(mut self, cli_overrides: toml::Value) -> Self {
        self.cli_overrides = cli_overrides;
        self
    }

    fn user_config_path(&self) -> PathBuf {
        self.config_folder_home.join(APP_CONFIG_FILE_NAME)
    }

    fn project_config_path(&self, workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(APP_CONFIG_DIR_NAME)
            .join(APP_CONFIG_FILE_NAME)
    }

    fn user_provider_config_path(&self) -> PathBuf {
        self.config_folder_home
            .join(crate::PROVIDER_CONFIG_FILE_NAME)
    }

    fn project_provider_config_path(&self, workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(APP_CONFIG_DIR_NAME)
            .join(crate::PROVIDER_CONFIG_FILE_NAME)
    }
}

impl AppConfigLoader for FileSystemAppConfigLoader {
    fn load(&self, workspace_root: Option<&Path>) -> Result<AppConfig, AppConfigError> {
        // Merge order is user < project < CLI so the highest-priority source
        // wins for any overlapping field.
        let mut merged = toml::Value::try_from(AppConfig::default())
            .expect("default app config must serialize to TOML");
        let mut provider_config = ProviderConfigSection::default();
        let mut provider_catalog = ProviderConfigFile::default();

        let user_path = self.user_config_path();
        let user_provider_path = self.user_provider_config_path();
        migrate_legacy_provider_config_on_startup(
            &user_path,
            &user_provider_path,
            &self.config_folder_home,
        )
        .map_err(|source| AppConfigError::Provider { source })?;
        if user_path.exists() {
            let user_config = read_config_value(&user_path)?;
            provider_config.merge_overlay(
                provider_section_from_value(&user_path, &user_config)?,
                &user_config,
            );
            provider_catalog.merge_overlay(ProviderConfigFile::from_provider_config_section(
                &provider_section_from_value(&user_path, &user_config)?,
            ));
            merge_app_config_values(&mut merged, user_config);
        }

        if user_provider_path.exists() {
            let user_provider_file = read_provider_catalog_config(&user_provider_path)
                .map_err(|source| AppConfigError::Provider { source })?;
            let user_provider_section = user_provider_file.to_provider_config_section();
            let user_provider_source =
                toml::Value::try_from(&user_provider_section).map_err(|error| {
                    AppConfigError::Provider {
                        source: ProviderConfigError::Serialize {
                            message: error.to_string(),
                        },
                    }
                })?;
            provider_config.merge_overlay(user_provider_section, &user_provider_source);
            provider_catalog.merge_overlay(user_provider_file);
        }

        if let Some(workspace_root) = workspace_root {
            let project_path = self.project_config_path(workspace_root);
            let project_provider_path = self.project_provider_config_path(workspace_root);
            migrate_legacy_provider_config_on_startup(
                &project_path,
                &project_provider_path,
                &self.config_folder_home,
            )
            .map_err(|source| AppConfigError::Provider { source })?;
            if project_path.exists() {
                let project_config = read_config_value(&project_path)?;
                provider_config.merge_overlay(
                    provider_section_from_value(&project_path, &project_config)?,
                    &project_config,
                );
                provider_catalog.merge_overlay(ProviderConfigFile::from_provider_config_section(
                    &provider_section_from_value(&project_path, &project_config)?,
                ));
                merge_app_config_values(&mut merged, project_config);
            }

            if project_provider_path.exists() {
                let project_provider_file = read_provider_catalog_config(&project_provider_path)
                    .map_err(|source| AppConfigError::Provider { source })?;
                let project_provider_section = project_provider_file.to_provider_config_section();
                let project_provider_source = toml::Value::try_from(&project_provider_section)
                    .map_err(|error| AppConfigError::Provider {
                        source: ProviderConfigError::Serialize {
                            message: error.to_string(),
                        },
                    })?;
                provider_config.merge_overlay(project_provider_section, &project_provider_source);
                provider_catalog.merge_overlay(project_provider_file);
            }
        }

        let cli_provider_section =
            provider_section_from_value(Path::new("<cli overrides>"), &self.cli_overrides)?;
        provider_catalog.merge_overlay(ProviderConfigFile::from_provider_config_section(
            &cli_provider_section,
        ));
        provider_config.merge_overlay(cli_provider_section, &self.cli_overrides);
        merge_app_config_values_ref(&mut merged, &self.cli_overrides);

        provider_catalog.apply_model_overrides(&provider_config.model_overrides);

        let mut config: AppConfig =
            merged
                .try_into()
                .map_err(|source: toml::de::Error| AppConfigError::Parse {
                    path: PathBuf::from("<merged config>"),
                    message: source.to_string(),
                })?;
        config.provider = provider_config;
        config.provider_catalog = provider_catalog;

        // Build normalized MCP runtime config from the persisted TOML shape.
        let servers = config
            .mcp_servers
            .iter()
            .map(|(id, record)| {
                record
                    .clone()
                    .into_runtime(id.clone())
                    .map_err(|message| AppConfigError::Validation { message })
            })
            .collect::<Result<Vec<_>, AppConfigError>>()?;
        config.mcp_runtime = McpConfig {
            servers,
            auto_start: config.mcp.auto_start,
        };
        config.mcp_runtime.ensure_bundled_servers();
        validate_app_config(&config)?;
        Ok(config)
    }
}

fn merge_app_config_values(base: &mut toml::Value, overlay: toml::Value) {
    replace_permission_section_if_present(base, &overlay);
    merge_toml_values(base, overlay);
}

fn merge_app_config_values_ref(base: &mut toml::Value, overlay: &toml::Value) {
    replace_permission_section_if_present(base, overlay);
    merge_toml_values_ref(base, overlay);
}

/// Replaces permission configuration as a unit when a higher-priority source
/// explicitly supplies its `[permission]` section.
fn replace_permission_section_if_present(base: &mut toml::Value, overlay: &toml::Value) {
    let Some(permission) = overlay.as_table().and_then(|table| table.get("permission")) else {
        return;
    };

    if let Some(base_table) = base.as_table_mut() {
        base_table.insert("permission".to_string(), permission.clone());
    }
}

fn merge_toml_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                if let Some(existing) = base_table.get_mut(&key) {
                    merge_toml_values(existing, value);
                } else {
                    base_table.insert(key, value);
                }
            }
        }
        (base_value, overlay_value) => *base_value = overlay_value,
    }
}

fn merge_toml_values_ref(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                if let Some(existing) = base_table.get_mut(key) {
                    merge_toml_values_ref(existing, value);
                } else {
                    base_table.insert(key.clone(), value.clone());
                }
            }
        }
        (base_value, overlay_value) => *base_value = overlay_value.clone(),
    }
}

fn validate_app_config(config: &AppConfig) -> Result<(), AppConfigError> {
    let mut seen = HashSet::new();
    if config.server.listen.iter().any(|addr| !seen.insert(addr)) {
        return Err(AppConfigError::Validation {
            message: "server.listen must not contain duplicate endpoints".into(),
        });
    }

    if config.server.auth.enabled {
        if config.server.auth.method_id.trim().is_empty() {
            return Err(AppConfigError::Validation {
                message: "server.auth.method_id must not be empty when server auth is enabled"
                    .into(),
            });
        }
        if config.server.auth.name.trim().is_empty() {
            return Err(AppConfigError::Validation {
                message: "server.auth.name must not be empty when server auth is enabled".into(),
            });
        }
    }

    if config.logging.file.max_files < 1 {
        return Err(AppConfigError::Validation {
            message: "logging.file.max_files must be at least 1".into(),
        });
    }

    if config.logging.file.filename_prefix.trim().is_empty() {
        return Err(AppConfigError::Validation {
            message: "logging.file.filename_prefix must not be empty".into(),
        });
    }

    if config.updates.check_interval_hours < 1 {
        return Err(AppConfigError::Validation {
            message: "updates.check_interval_hours must be at least 1".into(),
        });
    }

    let mut seen_skill_roots = HashSet::new();
    if config
        .skills
        .user_roots
        .iter()
        .any(|root| !seen_skill_roots.insert(root))
    {
        return Err(AppConfigError::Validation {
            message: "skills.user_roots must not contain duplicate paths".into(),
        });
    }

    seen_skill_roots.clear();
    if config
        .skills
        .workspace_roots
        .iter()
        .any(|root| !seen_skill_roots.insert(root))
    {
        return Err(AppConfigError::Validation {
            message: "skills.workspace_roots must not contain duplicate paths".into(),
        });
    }

    for entry in &config.skills.config {
        match (entry.path.as_ref(), entry.name.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(AppConfigError::Validation {
                    message: "skills.config entries must select either path or name, not both"
                        .into(),
                });
            }
            (None, None) => {
                return Err(AppConfigError::Validation {
                    message: "skills.config entries must include path or name".into(),
                });
            }
            (None, Some(name)) if name.trim().is_empty() => {
                return Err(AppConfigError::Validation {
                    message: "skills.config name selectors must not be empty".into(),
                });
            }
            (Some(_), None) | (None, Some(_)) => {}
        }
    }

    Ok(())
}
