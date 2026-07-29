use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Context;
use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::AgentsMdConfig;
use devo_core::AppConfig;
use devo_core::AppConfigStore;
use devo_core::FileSystemSkillCatalog;
use devo_core::Model;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderRequestModelMap;
use devo_core::ResolvedSkill;
use devo_core::ResolvedWebFetchConfig;
use devo_core::ResolvedWebSearchConfig;
use devo_core::SessionConfig;
use devo_core::SessionId;
use devo_core::SessionState;
use devo_core::SkillCatalog;
use devo_core::SkillError;
use devo_core::SkillSelector;
use devo_core::TurnConfig;
use devo_core::WebFetchConfig;
use devo_core::WebSearchConfig;
use devo_core::default_base_instructions;
use devo_core::normalize_canonical_path;
use devo_core::provider_request_model_map_for_binding;
use devo_core::read_user_auth_config;
use devo_core::resolve_enabled_model_binding;
use devo_core::resolve_web_fetch_config;
use devo_core::resolve_web_search_config;
use devo_core::tools::ToolPlanConfig;
use devo_core::tools::ToolRegistry;
use devo_core::tools::handlers;
use devo_mcp::manager::RmcpMcpManager;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::SkillDependencies as ProtocolSkillDependencies;
use devo_protocol::SkillInterface as ProtocolSkillInterface;
use devo_protocol::SkillScope as ProtocolSkillScope;
use devo_protocol::SkillToolDependency as ProtocolSkillToolDependency;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use futures::Stream;

use crate::InputItem;
use crate::SkillRecord;
use crate::load_server_provider;

pub(crate) struct SessionRuntimeContext {
    pub(crate) provider: Arc<dyn ModelProviderSDK>,
    pub(crate) provider_router: Arc<dyn ProviderRouter>,
    pub(crate) registry: Arc<ToolRegistry>,
    pub(crate) default_model: String,
    pub(crate) model_catalog: Arc<dyn ModelCatalog>,
    pub(crate) skill_catalog: Arc<StdMutex<Box<dyn SkillCatalog + Send>>>,
    pub(crate) agents_md: AgentsMdConfig,
    pub(crate) config_store: Arc<std::sync::Mutex<AppConfigStore>>,
}

struct RoutedModelProvider {
    router: Arc<dyn ProviderRouter>,
    route: ProviderRoute,
    provider_name: String,
}

impl RoutedModelProvider {
    fn new(router: Arc<dyn ProviderRouter>, route: ProviderRoute, provider_name: String) -> Self {
        Self {
            router,
            route,
            provider_name,
        }
    }
}

#[async_trait::async_trait]
impl ModelProviderSDK for RoutedModelProvider {
    async fn completion(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        self.router
            .complete(self.route.clone(), request)
            .await
            .map_err(anyhow::Error::new)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        self.router
            .stream(self.route.clone(), request)
            .await
            .map_err(anyhow::Error::new)
    }

    fn name(&self) -> &str {
        &self.provider_name
    }
}

impl SessionRuntimeContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        provider: Arc<dyn ModelProviderSDK>,
        provider_router: Arc<dyn ProviderRouter>,
        registry: Arc<ToolRegistry>,
        default_model: String,
        model_catalog: Arc<dyn ModelCatalog>,
        skill_catalog: Arc<StdMutex<Box<dyn SkillCatalog + Send>>>,
        agents_md: AgentsMdConfig,
        config_store: Arc<std::sync::Mutex<AppConfigStore>>,
    ) -> Self {
        Self {
            provider,
            provider_router,
            registry,
            default_model,
            model_catalog,
            skill_catalog,
            agents_md,
            config_store,
        }
    }

    pub(crate) async fn load_for_workspace(
        user_config_dir: PathBuf,
        workspace_root: Option<&Path>,
        inherited_context: &SessionRuntimeContext,
    ) -> anyhow::Result<Arc<Self>> {
        let config_store = Arc::new(std::sync::Mutex::new(AppConfigStore::load(
            user_config_dir.clone(),
            workspace_root,
        )?));
        let config = config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .clone();
        let has_provider_configuration = config.has_provider_configuration();
        let inherited_config = inherited_context
            .config_store
            .lock()
            .expect("inherited app config store mutex should not be poisoned")
            .effective_config()
            .clone();
        let provider_runtime_config_changed = !config
            .provider
            .is_operationally_equivalent_to(&inherited_config.provider)
            || config.provider_http != inherited_config.provider_http;
        let registry = if !has_provider_configuration && config.mcp.servers.is_empty() {
            Arc::clone(&inherited_context.registry)
        } else {
            let mcp_manager = Arc::new(RmcpMcpManager::new(
                config.mcp.clone(),
                config.mcp_oauth_credentials_store.unwrap_or_default(),
            ));
            let tool_plan = ToolPlanConfig::from_app_config(&config);
            Arc::new(handlers::build_registry_from_plan_with_mcp(&tool_plan, mcp_manager).await)
        };
        let model_catalog: Arc<dyn ModelCatalog> = Arc::new(PresetModelCatalog::load_from_config(
            &config.provider.model_overrides,
        )?);
        let default_model = model_catalog.resolve_for_turn(None)?.slug.clone();
        let (provider, provider_router, provider_default_model) =
            if has_provider_configuration && provider_runtime_config_changed {
                let provider =
                    load_server_provider(&config, Some(default_model.as_str()), &user_config_dir)
                        .context("load server provider for session workspace")?;
                (
                    provider.provider,
                    provider.provider_router,
                    provider.default_model,
                )
            } else if has_provider_configuration {
                (
                    Arc::clone(&inherited_context.provider),
                    Arc::clone(&inherited_context.provider_router),
                    inherited_context.default_model.clone(),
                )
            } else {
                (
                    Arc::clone(&inherited_context.provider),
                    Arc::clone(&inherited_context.provider_router),
                    default_model,
                )
            };
        let skill_workspace_root = workspace_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let skill_catalog = Arc::new(StdMutex::new(
            Box::new(FileSystemSkillCatalog::with_devo_home(
                config.skills.clone(),
                user_config_dir,
                skill_workspace_root,
                config.project_root_markers.clone(),
            )) as Box<dyn SkillCatalog + Send>,
        ));

        Ok(Arc::new(Self::from_parts(
            provider,
            provider_router,
            registry,
            provider_default_model,
            model_catalog,
            skill_catalog,
            AgentsMdConfig {
                project_root_markers: config.project_root_markers.clone(),
                ..AgentsMdConfig::default()
            },
            config_store,
        )))
    }

    pub(crate) fn provider_for_route(&self, route: ProviderRoute) -> Arc<dyn ModelProviderSDK> {
        let provider_name = match &route {
            ProviderRoute::Default => self.provider.name().to_owned(),
            ProviderRoute::Binding { provider_id, .. } => provider_id.clone(),
        };
        Arc::new(RoutedModelProvider::new(
            Arc::clone(&self.provider_router),
            route,
            provider_name,
        ))
    }

    pub(crate) fn hook_runner(&self) -> Option<devo_core::HookRunner> {
        let hooks = {
            let config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            config_store.effective_config().hooks.clone()
        };
        (!hooks.is_empty()).then(|| devo_core::HookRunner::new(hooks))
    }

    pub(crate) fn new_session_state(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
    ) -> SessionState {
        let (permission_preset, sandbox_from_project) = {
            let config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            let project = config_store
                .effective_config()
                .projects
                .get(&devo_core::project_config_key(&cwd));
            let preset = match project.and_then(|project| project.permission_preset) {
                Some(devo_protocol::PermissionPreset::AutoReview) => {
                    devo_safety::PermissionPreset::AutoReview
                }
                Some(devo_protocol::PermissionPreset::FullAccess) => {
                    devo_safety::PermissionPreset::FullAccess
                }
                Some(devo_protocol::PermissionPreset::Default) | None => {
                    devo_safety::PermissionPreset::Default
                }
            };
            let sandbox_from_project = project.and_then(|project| project.sandbox_profile.clone());
            (preset, sandbox_from_project)
        };
        let permission_profile =
            devo_safety::RuntimePermissionProfile::from_preset(permission_preset, cwd.clone())
                .with_additional_roots(additional_directories);
        let sandbox_profile = sandbox_from_project
            .unwrap_or_else(|| permission_profile.implied_sandbox_profile().to_string());
        let available_skills_instructions = {
            let mut skill_catalog = self
                .skill_catalog
                .lock()
                .expect("skill catalog mutex should not be poisoned");
            match skill_catalog.available_skills_instructions(Some(&cwd), None) {
                Ok(Some(instructions)) if !instructions.trim().is_empty() => Some(format!(
                    "<available_skills>\n{}\n</available_skills>",
                    instructions.trim()
                )),
                Ok(_) => None,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        cwd = %cwd.display(),
                        "failed to render available skills instructions"
                    );
                    None
                }
            }
        };
        let session_config = SessionConfig {
            permission_mode: permission_profile.permission_mode(),
            permission_profile,
            agents_md: self.agents_md.clone(),
            available_skills_instructions,
            sandbox_profile: Some(sandbox_profile),
            ..SessionConfig::default()
        };
        let mut state = SessionState::new(session_config, cwd);
        state.id = session_id.to_string();
        state
    }

    fn catalog_model_or_fallback(&self, model_slug: &str) -> Model {
        self.model_catalog
            .get(model_slug)
            .cloned()
            .unwrap_or_else(|| Model {
                slug: model_slug.to_string(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            })
    }

    pub(crate) fn resolve_turn_model(&self, requested_model: Option<&str>) -> Model {
        if let Some(model) = requested_model.and_then(|requested| self.model_catalog.get(requested))
        {
            return model.clone();
        }

        self.model_catalog
            .resolve_for_turn(Some(&self.default_model))
            .or_else(|_| self.model_catalog.resolve_for_turn(None))
            .cloned()
            .unwrap_or_else(|_| Model {
                slug: self.default_model.clone(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            })
    }

    pub(crate) fn resolve_turn_config(
        &self,
        requested_model: Option<&str>,
        reasoning_effort_selection: Option<String>,
    ) -> TurnConfig {
        let (config, user_config_dir) = {
            let config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            (
                config_store.effective_config().clone(),
                config_store.user_config_dir().to_path_buf(),
            )
        };
        let provider_config = config.provider.clone();

        if let Some(binding) = resolve_enabled_model_binding(&provider_config, requested_model) {
            let provider = provider_config.providers.get(&binding.provider_id);
            let binding_config = provider_config.model_bindings.get(&binding.binding_id);
            let web_search = self.resolve_turn_web_search(
                &config,
                &user_config_dir,
                provider.and_then(|provider| provider.web_search.as_ref()),
                binding_config.and_then(|binding| binding.web_search.as_ref()),
            );
            let web_fetch = self.resolve_turn_web_fetch(
                &config,
                provider.and_then(|provider| provider.web_fetch.as_ref()),
                binding_config.and_then(|binding| binding.web_fetch.as_ref()),
            );
            let provider_request_models = ProviderRequestModelMap::new(
                provider_request_model_map_for_binding(&provider_config, &binding),
            );
            let binding_id = binding.binding_id.clone();
            let mut turn_config = TurnConfig::with_provider_route_and_web_tools(
                self.catalog_model_or_fallback(&binding.model_slug),
                binding.request_model,
                provider_request_models,
                ProviderRoute::binding(binding.provider_id, binding.invocation_method),
                web_search,
                web_fetch,
                reasoning_effort_selection,
            );
            turn_config.model_binding_id = Some(binding_id);
            return turn_config;
        }

        let model = self.resolve_turn_model(requested_model);
        let web_search = self.resolve_turn_web_search(&config, &user_config_dir, None, None);
        let web_fetch = self.resolve_turn_web_fetch(&config, None, None);
        let mut turn_config = TurnConfig::new(model, reasoning_effort_selection);
        turn_config.web_search = web_search;
        turn_config.web_fetch = web_fetch;
        turn_config
    }

    fn resolve_turn_web_search(
        &self,
        config: &AppConfig,
        user_config_dir: &Path,
        provider_override: Option<&WebSearchConfig>,
        binding_override: Option<&WebSearchConfig>,
    ) -> ResolvedWebSearchConfig {
        let auth = match read_user_auth_config(&user_config_dir.join(AUTH_CONFIG_FILE_NAME)) {
            Ok(auth) => auth,
            Err(error) => {
                tracing::warn!(%error, "failed to load web_search credentials");
                return ResolvedWebSearchConfig::Disabled;
            }
        };
        match resolve_web_search_config(
            &config.tools.web_search,
            provider_override,
            binding_override,
            &auth,
        ) {
            Ok(web_search) => web_search,
            Err(error) => {
                tracing::warn!(%error, "failed to resolve web_search config");
                ResolvedWebSearchConfig::Disabled
            }
        }
    }

    fn resolve_turn_web_fetch(
        &self,
        config: &AppConfig,
        provider_override: Option<&WebFetchConfig>,
        binding_override: Option<&WebFetchConfig>,
    ) -> ResolvedWebFetchConfig {
        resolve_web_fetch_config(&config.tools.web_fetch, provider_override, binding_override)
    }

    pub(crate) fn discover_skills(
        &self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        let mut skill_catalog = self
            .skill_catalog
            .lock()
            .expect("skill catalog mutex should not be poisoned");
        skill_catalog
            .discover(workspace_root, force_reload)
            .map(|skills| {
                skills
                    .into_iter()
                    .map(core_skill_record_to_protocol)
                    .collect()
            })
    }

    pub(crate) fn set_skill_enabled(
        &self,
        path: PathBuf,
        enabled: bool,
        workspace_root: Option<&Path>,
    ) -> anyhow::Result<Vec<SkillRecord>> {
        let (skills_config, project_root_markers) = {
            let mut config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            config_store.set_skill_enabled(path, enabled)?;
            let effective = config_store.effective_config();
            (
                effective.skills.clone(),
                effective.project_root_markers.clone(),
            )
        };

        {
            let mut skill_catalog = self
                .skill_catalog
                .lock()
                .expect("skill catalog mutex should not be poisoned");
            skill_catalog.set_config(skills_config, project_root_markers);
        }

        self.discover_skills(workspace_root, true)
            .map_err(|error| anyhow::anyhow!(error))
    }

    pub(crate) fn resolve_input_items(
        &self,
        input: &[InputItem],
        workspace_root: Option<&Path>,
    ) -> Result<Option<ResolvedInput>, SkillError> {
        let mut skill_catalog = self
            .skill_catalog
            .lock()
            .expect("skill catalog mutex should not be poisoned");
        let discovered_skills = skill_catalog.discover(workspace_root, false)?;

        let mut parts = Vec::new();
        let structured_skill_names = input
            .iter()
            .filter_map(|item| match item {
                InputItem::Skill { name, .. } => Some(name.clone()),
                InputItem::Text { .. }
                | InputItem::LocalImage { .. }
                | InputItem::Mention { .. } => None,
            })
            .collect::<HashSet<_>>();
        let mut injected_plain_skill_paths = HashSet::new();
        for text in input.iter().filter_map(|item| match item {
            InputItem::Text { text } => Some(text),
            InputItem::Skill { .. } | InputItem::LocalImage { .. } | InputItem::Mention { .. } => {
                None
            }
        }) {
            for name in plain_skill_mentions(text) {
                if structured_skill_names.contains(&name) {
                    continue;
                }
                let matches = discovered_skills
                    .iter()
                    .filter(|skill| skill.name == name)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [skill] if skill.enabled => {
                        if injected_plain_skill_paths.insert(skill.path.clone()) {
                            let rendered = skill_catalog
                                .load(
                                    &SkillSelector {
                                        name: skill.name.clone(),
                                        path: Some(skill.path.clone()),
                                    },
                                    workspace_root,
                                )
                                .map(|skill| render_resolved_skill(&skill))?;
                            parts.push(rendered);
                        }
                    }
                    [skill] => {
                        return Err(SkillError::SkillDisabled {
                            name: skill.name.clone(),
                            path: skill.path.clone(),
                        });
                    }
                    [] => {}
                    _ => {
                        return Err(SkillError::AmbiguousSkillName {
                            name,
                            paths: matches.iter().map(|skill| skill.path.clone()).collect(),
                        });
                    }
                }
            }
        }

        let item_parts = input
            .iter()
            .map(|item| match item {
                InputItem::Text { text } => Ok(text.trim().to_string()),
                InputItem::Skill { name, path } => skill_catalog
                    .load(
                        &SkillSelector {
                            name: name.clone(),
                            path: (!path.as_os_str().is_empty()).then(|| path.clone()),
                        },
                        workspace_root,
                    )
                    .map(|skill| render_resolved_skill(&skill)),
                InputItem::LocalImage { path } => Ok(format!("[image:{}]", path.display())),
                InputItem::Mention { path, name } => Ok(format!(
                    "[mention:{}]",
                    name.as_deref().unwrap_or(path.as_str())
                )),
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        parts.extend(item_parts);
        Ok((!parts.is_empty()).then(|| ResolvedInput {
            prompt_text: parts.join("\n"),
            prompt_messages: parts,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedInput {
    pub(crate) prompt_text: String,
    pub(crate) prompt_messages: Vec<String>,
}

fn core_skill_record_to_protocol(record: devo_core::CoreSkillRecord) -> SkillRecord {
    SkillRecord {
        id: record.id.0.to_string(),
        name: record.name,
        description: record.description,
        short_description: record.short_description,
        interface: record.interface.map(|interface| ProtocolSkillInterface {
            display_name: interface.display_name,
            short_description: interface.short_description,
            icon_small: interface.icon_small,
            icon_large: interface.icon_large,
            brand_color: interface.brand_color,
            default_prompt: interface.default_prompt,
        }),
        dependencies: record
            .dependencies
            .map(|dependencies| ProtocolSkillDependencies {
                tools: dependencies
                    .tools
                    .into_iter()
                    .map(|tool| ProtocolSkillToolDependency {
                        r#type: tool.r#type,
                        value: tool.value,
                        description: tool.description,
                        transport: tool.transport,
                        command: tool.command,
                        url: tool.url,
                    })
                    .collect(),
            }),
        path: normalize_canonical_path(record.path),
        enabled: record.enabled,
        source: core_skill_source_to_protocol(record.source),
        scope: core_skill_scope_to_protocol(record.scope),
        plugin_id: record.plugin_id,
    }
}

fn core_skill_source_to_protocol(source: devo_core::CoreSkillSource) -> crate::SkillSource {
    match source {
        devo_core::CoreSkillSource::User => crate::SkillSource::User,
        devo_core::CoreSkillSource::Workspace { cwd } => crate::SkillSource::Workspace { cwd },
        devo_core::CoreSkillSource::Plugin { plugin_id } => {
            crate::SkillSource::Plugin { plugin_id }
        }
        devo_core::CoreSkillSource::System => crate::SkillSource::System,
        devo_core::CoreSkillSource::Admin => crate::SkillSource::Admin,
    }
}

fn core_skill_scope_to_protocol(scope: devo_core::CoreSkillScope) -> ProtocolSkillScope {
    match scope {
        devo_core::CoreSkillScope::Repo => ProtocolSkillScope::Repo,
        devo_core::CoreSkillScope::User => ProtocolSkillScope::User,
        devo_core::CoreSkillScope::System => ProtocolSkillScope::System,
        devo_core::CoreSkillScope::Admin => ProtocolSkillScope::Admin,
        devo_core::CoreSkillScope::Plugin => ProtocolSkillScope::Plugin,
    }
}

fn render_resolved_skill(skill: &ResolvedSkill) -> String {
    let base_dir = normalize_canonical_path(
        skill
            .record
            .path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf(),
    );
    format!(
        "<skill id=\"{}\" name=\"{}\">\n{}\n\nBase directory: {}\n</skill>",
        skill.record.id.0,
        skill.record.name,
        skill.content.trim_end(),
        base_dir.display()
    )
}

fn plain_skill_mentions(text: &str) -> HashSet<String> {
    let bytes = text.as_bytes();
    let mut mentions = HashSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let Some(first) = bytes.get(start) else {
            index += 1;
            continue;
        };
        if !is_skill_mention_name_byte(*first) {
            index += 1;
            continue;
        }
        let mut end = start + 1;
        while let Some(next) = bytes.get(end)
            && is_skill_mention_name_byte(*next)
        {
            end += 1;
        }
        let name = &text[start..end];
        if !is_common_env_var(name) {
            mentions.insert(name.to_string());
        }
        index = end;
    }
    mentions
}

fn is_skill_mention_name_byte(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b':')
}

fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(upper.as_str(), "PATH" | "HOME" | "USER" | "SHELL" | "PWD")
}
