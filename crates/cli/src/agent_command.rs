use anyhow::Context;
use anyhow::Result;
use devo_core::AppConfig;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ResolvedProviderSettings;
use devo_core::SessionId;
use devo_core::project_config_key;
use devo_core::resolve_model_binding;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderWireApi;
use devo_tui::InitialTuiSession;
use devo_tui::InteractiveTuiConfig;
use devo_tui::SavedModelEntry;
use devo_tui::run_interactive_tui;
use devo_util_paths::find_devo_home;

/// Runs the interactive coding-agent entrypoint.
///
/// `force_onboarding` forces the TUI to start in provider onboarding mode even
/// when a provider config already exists. `exit_after_onboarding` exits after a
/// successful onboarding save instead of continuing into the interactive TUI.
/// `log_level` is forwarded to the background server process.
/// `dangerously_skip_permissions` starts the session with full-access permissions.
pub(crate) async fn run_agent(
    force_onboarding: bool,
    exit_after_onboarding: bool,
    log_level: Option<&str>,
    initial_session_id: Option<SessionId>,
    dangerously_skip_permissions: bool,
) -> Result<devo_tui::AppExit> {
    let cwd = std::env::current_dir()?;
    let config_home = find_devo_home().context("could not determine devo home directory")?;
    let app_config = FileSystemAppConfigLoader::new(config_home.clone()).load(Some(&cwd))?;
    let model_catalog = PresetModelCatalog::load_from_config(&app_config.provider.model_overrides)?;
    let project_key = project_config_key(&cwd);
    let permission_preset =
        initial_permission_preset(&app_config, &project_key, dangerously_skip_permissions);
    let sandbox_profile =
        initial_sandbox_profile(&app_config, &project_key, dangerously_skip_permissions);
    let (onboarding_mode, resolved) = resolve_initial_provider_settings(
        force_onboarding,
        &app_config,
        &config_home,
        &model_catalog,
    )?;

    // convert to TUI `SavedModelEntry` type.
    // the `SaveModelEntry` seems utilized to display model at TUI.
    // TODO: Investigate  whether we could simplify it, unify model structure.
    let saved_models = saved_model_entries(&app_config);

    let ResolvedProviderSettings {
        wire_api,
        model,
        base_url: _,
        api_key: _,
        model_reasoning_effort_selection,
        ..
    } = resolved;
    let active_model_binding = if onboarding_mode {
        None
    } else {
        resolve_model_binding(&app_config.provider, /*requested_model*/ None)
    };
    let request_model = active_model_binding.as_ref().and_then(|binding| {
        if binding.request_model == binding.model_slug {
            None
        } else {
            Some(binding.request_model.clone())
        }
    });
    let model_binding_id = active_model_binding
        .as_ref()
        .map(|binding| binding.binding_id.clone());
    let model = active_model_binding
        .as_ref()
        .map(|binding| binding.model_slug.clone())
        .unwrap_or(model);

    tracing::info!("starting interactive tui");
    let exit = run_interactive_tui(InteractiveTuiConfig {
        // initial_session corresponding fields at top of `config.toml`.
        initial_session: InitialTuiSession {
            session_id: initial_session_id,
            model,
            request_model,
            model_binding_id,
            provider: wire_api,
            reasoning_effort_selection: model_reasoning_effort_selection,
            permission_preset,
            sandbox_profile,
            compaction_token_limit: app_config.compaction_token_limit,
            default_collaboration_mode: app_config.default_collaboration_mode,
            // TODO: why do we need cwd here, maybe remove it ?
            cwd,
        },
        server_log_level: log_level.map(ToOwned::to_owned),
        model_catalog,
        saved_models,
        show_model_onboarding: onboarding_mode,
        exit_after_onboarding,
    })
    .await?;
    tracing::info!("interactive tui returned to cli agent command");
    Ok(exit)
}

fn initial_permission_preset(
    app_config: &AppConfig,
    project_key: &str,
    dangerously_skip_permissions: bool,
) -> PermissionPreset {
    let mut permission_preset = app_config
        .projects
        .get(project_key)
        .and_then(|config| config.permission_preset)
        .unwrap_or(PermissionPreset::AutoReview);

    if dangerously_skip_permissions {
        permission_preset = PermissionPreset::FullAccess;
    }

    permission_preset
}

/// Sandbox profile shown as current when the TUI starts. Prefer the project
/// `sandbox_profile` when set; otherwise derive it from the permission preset
/// (Full Access → off, otherwise workspace).
fn initial_sandbox_profile(
    app_config: &AppConfig,
    project_key: &str,
    dangerously_skip_permissions: bool,
) -> Option<String> {
    if dangerously_skip_permissions {
        return Some("off".to_string());
    }
    let project = app_config.projects.get(project_key);
    if let Some(profile) = project.and_then(|config| config.sandbox_profile.clone()) {
        return Some(profile);
    }
    let preset = project
        .and_then(|config| config.permission_preset)
        .unwrap_or(PermissionPreset::AutoReview);
    Some(
        match preset {
            PermissionPreset::FullAccess => "off",
            PermissionPreset::Default | PermissionPreset::AutoReview => "workspace",
        }
        .to_string(),
    )
}

/// Resolves the initial provider settings and whether onboarding should be shown.
///
/// `force_onboarding` requests onboarding regardless of stored configuration.
/// `stored_config` is the persisted provider config used to decide whether this
/// is a first-run session. `model_catalog` supplies the fallback onboarding
/// model when no usable provider settings should be resolved yet.
fn resolve_initial_provider_settings(
    force_onboarding: bool,
    app_config: &AppConfig,
    user_config_dir: &std::path::Path,
    model_catalog: &PresetModelCatalog,
) -> Result<(bool, ResolvedProviderSettings)> {
    let onboarding_mode = force_onboarding || !app_config.has_provider_configuration();
    let resolved = if onboarding_mode {
        // falls back to the first visible preset model.
        let fallback_model = model_catalog
            .resolve_for_turn(None)
            .context("builtin model catalog does not contain a visible onboarding model")?;

        ResolvedProviderSettings {
            provider_id: fallback_model.provider.as_str().to_string(),
            wire_api: fallback_model.provider,
            model: fallback_model.slug.clone(),
            base_url: None,
            api_key: None,
            proxy_url: None,
            no_proxy: None,
            headers: None,
            model_auto_compact_token_limit: None,
            model_context_window: None,
            model_reasoning_effort_selection: None,
            disable_response_storage: false,
            preferred_auth_method: None,
        }
    } else {
        app_config
            .resolve_provider_settings(user_config_dir)
            .with_context(|| "failed to resolve provider settings outside onboarding mode")?
    };
    Ok((onboarding_mode, resolved))
}

/// Converts persisted model bindings into TUI model-picker entries.
fn saved_model_entries(app_config: &AppConfig) -> Vec<SavedModelEntry> {
    let stored_config = &app_config.provider;
    let mut entries = stored_config
        .model_bindings
        .iter()
        .filter(|(_, binding)| binding.enabled)
        .filter_map(|(binding_id, binding)| {
            let provider = stored_config.providers.get(&binding.provider)?;
            let request_model = if binding.request_model == binding.model_slug {
                None
            } else {
                Some(binding.request_model.clone())
            };
            let display_name = binding
                .display_name
                .clone()
                .or_else(|| request_model.clone());
            let provider_name = if provider.name.trim().is_empty() {
                binding.provider.clone()
            } else {
                provider.name.clone()
            };
            Some(SavedModelEntry {
                binding_id: Some(binding_id.clone()),
                model: binding.model_slug.clone(),
                request_model,
                display_name,
                provider_id: Some(binding.provider.clone()),
                provider_name: Some(provider_name),
                wire_api: binding.invocation_method,
                base_url: provider.base_url.clone(),
                api_key: None,
            })
        })
        .collect::<Vec<_>>();

    entries.extend(stored_config.model_providers.iter().flat_map(
        |(provider_id, provider_config)| {
            // Older config entries may not have persisted `wire_api`; keep them
            // on the historical OpenAI-compatible chat-completions default.
            let wire_api = provider_config
                .wire_api
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
            let provider_name = provider_config
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| provider_id.clone());
            provider_config
                .models
                .iter()
                .map(move |model| SavedModelEntry {
                    binding_id: None,
                    model: model.model.clone(),
                    request_model: None,
                    display_name: None,
                    provider_id: Some(provider_id.clone()),
                    provider_name: Some(provider_name.clone()),
                    wire_api,
                    base_url: model
                        .base_url
                        .clone()
                        .or_else(|| provider_config.base_url.clone()),
                    api_key: model
                        .api_key
                        .clone()
                        .or_else(|| provider_config.api_key.clone()),
                })
        },
    ));
    entries
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use pretty_assertions::assert_eq;

    use super::initial_permission_preset;
    use super::initial_sandbox_profile;
    use super::resolve_initial_provider_settings;
    use super::saved_model_entries;
    use devo_core::AppConfig;
    use devo_core::ConfiguredModel;
    use devo_core::LegacyModelProviderConfig;
    use devo_core::Model;
    use devo_core::ModelBindingConfig;
    use devo_core::PresetModelCatalog;
    use devo_core::ProjectConfig;
    use devo_core::ProviderConfigSection;
    use devo_core::ProviderDefaultsConfig;
    use devo_core::ProviderVendorConfig;
    use devo_core::ResolvedProviderSettings;
    use devo_protocol::PermissionPreset;
    use devo_protocol::ProviderWireApi;
    use devo_tui::SavedModelEntry;

    fn test_catalog() -> PresetModelCatalog {
        PresetModelCatalog::new(vec![Model {
            slug: "test-onboard-model".to_string(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            ..Model::default()
        }])
    }

    fn test_user_config_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devo-cli-test-{nanos}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn test_app_config(provider: ProviderConfigSection) -> AppConfig {
        AppConfig {
            provider,
            ..AppConfig::default()
        }
    }

    #[test]
    fn initial_permission_preset_defaults_to_auto_review_when_unset() {
        let app_config = AppConfig::default();
        assert_eq!(
            initial_permission_preset(
                &app_config,
                "project-key",
                /*dangerously_skip_permissions*/ false,
            ),
            PermissionPreset::AutoReview,
        );
    }

    #[test]
    fn initial_permission_preset_uses_project_config_when_flag_is_false() {
        let mut app_config = AppConfig::default();
        app_config.projects.insert(
            "project-key".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::AutoReview),
                sandbox_profile: None,
            },
        );

        assert_eq!(
            initial_permission_preset(
                &app_config,
                "project-key",
                /*dangerously_skip_permissions*/ false,
            ),
            PermissionPreset::AutoReview,
        );
    }

    #[test]
    fn initial_permission_preset_overrides_to_full_access_when_flag_is_true() {
        let mut app_config = AppConfig::default();
        app_config.projects.insert(
            "project-key".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::AutoReview),
                sandbox_profile: None,
            },
        );

        assert_eq!(
            initial_permission_preset(
                &app_config,
                "project-key",
                /*dangerously_skip_permissions*/ true,
            ),
            PermissionPreset::FullAccess,
        );
    }

    #[test]
    fn initial_sandbox_profile_uses_project_config_when_set() {
        let mut app_config = AppConfig::default();
        app_config.projects.insert(
            "project-key".to_string(),
            ProjectConfig {
                permission_preset: None,
                sandbox_profile: Some("strict".to_string()),
            },
        );

        assert_eq!(
            initial_sandbox_profile(
                &app_config,
                "project-key",
                /*dangerously_skip_permissions*/ false,
            ),
            Some("strict".to_string()),
        );
    }

    #[test]
    fn initial_sandbox_profile_falls_back_to_permission_implied() {
        let mut app_config = AppConfig::default();
        app_config.projects.insert(
            "full-access-project".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::FullAccess),
                sandbox_profile: None,
            },
        );

        assert_eq!(
            initial_sandbox_profile(
                &app_config,
                "missing-project-key",
                /*dangerously_skip_permissions*/ false,
            ),
            Some("workspace".to_string()),
        );
        assert_eq!(
            initial_sandbox_profile(
                &app_config,
                "full-access-project",
                /*dangerously_skip_permissions*/ false,
            ),
            Some("off".to_string()),
        );
    }

    #[test]
    fn initial_sandbox_profile_respects_dangerously_skip_permissions() {
        let app_config = AppConfig::default();
        assert_eq!(
            initial_sandbox_profile(
                &app_config,
                "any",
                /*dangerously_skip_permissions*/ true,
            ),
            Some("off".to_string()),
        );
    }

    #[test]
    fn resolve_initial_provider_settings_uses_catalog_fallback_during_onboarding() {
        let actual = resolve_initial_provider_settings(
            false,
            &test_app_config(ProviderConfigSection::default()),
            &test_user_config_dir(),
            &test_catalog(),
        )
        .expect("resolve initial provider settings");

        assert_eq!(
            actual,
            (
                true,
                ResolvedProviderSettings {
                    provider_id: "openai_chat_completions".to_string(),
                    wire_api: ProviderWireApi::OpenAIChatCompletions,
                    model: "test-onboard-model".to_string(),
                    base_url: None,
                    api_key: None,
                    proxy_url: None,
                    no_proxy: None,
                    headers: None,
                    model_auto_compact_token_limit: None,
                    model_context_window: None,
                    model_reasoning_effort_selection: None,
                    disable_response_storage: false,
                    preferred_auth_method: None,
                }
            )
        );
    }

    #[test]
    fn resolve_initial_provider_settings_honors_forced_onboarding_with_existing_config() {
        let mut provider = ProviderConfigSection::default();
        provider.providers.insert(
            "openai_chat_completions".to_string(),
            ProviderVendorConfig::default(),
        );

        let actual = resolve_initial_provider_settings(
            true,
            &test_app_config(provider),
            &test_user_config_dir(),
            &test_catalog(),
        )
        .expect("resolve initial provider settings");

        assert_eq!(
            actual,
            (
                true,
                ResolvedProviderSettings {
                    provider_id: "openai_chat_completions".to_string(),
                    wire_api: ProviderWireApi::OpenAIChatCompletions,
                    model: "test-onboard-model".to_string(),
                    base_url: None,
                    api_key: None,
                    proxy_url: None,
                    no_proxy: None,
                    headers: None,
                    model_auto_compact_token_limit: None,
                    model_context_window: None,
                    model_reasoning_effort_selection: None,
                    disable_response_storage: false,
                    preferred_auth_method: None,
                }
            )
        );
    }

    #[test]
    fn resolve_initial_provider_settings_uses_merged_project_provider_config() {
        let provider = ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("deepseek-binding".to_string()),
            },
            providers: BTreeMap::from([(
                "deepseek".to_string(),
                ProviderVendorConfig {
                    name: "deepseek".to_string(),
                    base_url: Some("https://api.deepseek.com".to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                    enabled: true,
                    ..ProviderVendorConfig::default()
                },
            )]),
            model_bindings: BTreeMap::from([(
                "deepseek-binding".to_string(),
                ModelBindingConfig {
                    model_slug: "deepseek-v4-flash".to_string(),
                    provider: "deepseek".to_string(),
                    request_model: "deepseek-v4-flash".to_string(),
                    invocation_method: ProviderWireApi::OpenAIChatCompletions,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        };

        let actual = resolve_initial_provider_settings(
            false,
            &test_app_config(provider),
            &test_user_config_dir(),
            &test_catalog(),
        )
        .expect("resolve initial provider settings");

        assert_eq!(
            actual,
            (
                false,
                ResolvedProviderSettings {
                    provider_id: "deepseek".to_string(),
                    wire_api: ProviderWireApi::OpenAIChatCompletions,
                    model: "deepseek-v4-flash".to_string(),
                    base_url: Some("https://api.deepseek.com".to_string()),
                    api_key: None,
                    proxy_url: None,
                    no_proxy: None,
                    headers: None,
                    model_auto_compact_token_limit: None,
                    model_context_window: None,
                    model_reasoning_effort_selection: None,
                    disable_response_storage: false,
                    preferred_auth_method: None,
                }
            )
        );
    }

    #[test]
    fn saved_model_entries_inherit_provider_defaults_and_preserve_model_overrides() {
        let app_config = test_app_config(ProviderConfigSection {
            providers: BTreeMap::from([(
                "openai".to_string(),
                ProviderVendorConfig {
                    base_url: Some("https://provider.example".to_string()),
                    credential: Some("provider-key".to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    enabled: true,
                    ..ProviderVendorConfig::default()
                },
            )]),
            model_bindings: BTreeMap::from([(
                "openai".to_string(),
                ModelBindingConfig {
                    model_slug: "provider-defaults".to_string(),
                    provider: "openai".to_string(),
                    request_model: "provider-defaults".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    ..ModelBindingConfig::default()
                },
            )]),
            model_providers: BTreeMap::from([(
                "legacy".to_string(),
                LegacyModelProviderConfig {
                    base_url: Some("https://provider.example".to_string()),
                    api_key: Some("provider-key".to_string()),
                    wire_api: Some(ProviderWireApi::OpenAIResponses),
                    models: vec![ConfiguredModel {
                        model: "model-overrides".to_string(),
                        base_url: Some("https://model.example".to_string()),
                        api_key: Some("model-key".to_string()),
                    }],
                    ..LegacyModelProviderConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        });

        assert_eq!(
            saved_model_entries(&app_config),
            vec![
                SavedModelEntry {
                    binding_id: Some("openai".to_string()),
                    model: "provider-defaults".to_string(),
                    request_model: None,
                    display_name: None,
                    provider_id: Some("openai".to_string()),
                    provider_name: Some("openai".to_string()),
                    wire_api: ProviderWireApi::OpenAIResponses,
                    base_url: Some("https://provider.example".to_string()),
                    api_key: None,
                },
                SavedModelEntry {
                    binding_id: None,
                    model: "model-overrides".to_string(),
                    request_model: None,
                    display_name: None,
                    provider_id: Some("legacy".to_string()),
                    provider_name: Some("legacy".to_string()),
                    wire_api: ProviderWireApi::OpenAIResponses,
                    base_url: Some("https://model.example".to_string()),
                    api_key: Some("model-key".to_string()),
                },
            ]
        );
    }

    #[test]
    fn saved_model_entries_preserve_binding_request_and_display_names() {
        let app_config = test_app_config(ProviderConfigSection {
            providers: BTreeMap::from([(
                "deepseek".to_string(),
                ProviderVendorConfig {
                    base_url: Some("https://api.deepseek.com".to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                    enabled: true,
                    ..ProviderVendorConfig::default()
                },
            )]),
            model_bindings: BTreeMap::from([(
                "deepseek".to_string(),
                ModelBindingConfig {
                    model_slug: "deepseek-v4-flash".to_string(),
                    provider: "deepseek".to_string(),
                    request_model: "DeepSeek-V4-Flash".to_string(),
                    display_name: Some("DeepSeek-V4-Flash".to_string()),
                    invocation_method: ProviderWireApi::OpenAIChatCompletions,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        });

        assert_eq!(
            saved_model_entries(&app_config),
            vec![SavedModelEntry {
                binding_id: Some("deepseek".to_string()),
                model: "deepseek-v4-flash".to_string(),
                request_model: Some("DeepSeek-V4-Flash".to_string()),
                display_name: Some("DeepSeek-V4-Flash".to_string()),
                provider_id: Some("deepseek".to_string()),
                provider_name: Some("deepseek".to_string()),
                wire_api: ProviderWireApi::OpenAIChatCompletions,
                base_url: Some("https://api.deepseek.com".to_string()),
                api_key: None,
            }]
        );
    }

    #[test]
    fn saved_model_entries_defaults_wire_api_to_openai_chat_completions() {
        let app_config = test_app_config(ProviderConfigSection {
            model_providers: BTreeMap::from([(
                "openai".to_string(),
                LegacyModelProviderConfig {
                    models: vec![ConfiguredModel {
                        model: "default-wire-api".to_string(),
                        ..ConfiguredModel::default()
                    }],
                    ..LegacyModelProviderConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        });

        assert_eq!(
            saved_model_entries(&app_config),
            vec![SavedModelEntry {
                binding_id: None,
                model: "default-wire-api".to_string(),
                request_model: None,
                display_name: None,
                provider_id: Some("openai".to_string()),
                provider_name: Some("openai".to_string()),
                wire_api: ProviderWireApi::OpenAIChatCompletions,
                base_url: None,
                api_key: None,
            }]
        );
    }
}
