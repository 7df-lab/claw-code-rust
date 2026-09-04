use anyhow::Context;
use anyhow::Result;
use devo_core::AppConfig;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SessionId;
use devo_core::project_config_key;
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
    let model_catalog = PresetModelCatalog::load_from_provider_config_with_overrides(
        &app_config.provider_catalog_config(),
        &app_config.provider.model_overrides,
    )?;
    let project_key = project_config_key(&cwd);
    let permission_preset =
        initial_permission_preset(&app_config, &project_key, dangerously_skip_permissions);
    let sandbox_profile =
        initial_sandbox_profile(&app_config, &project_key, dangerously_skip_permissions);
    let onboarding_mode = force_onboarding || !app_config.has_provider_configuration();
    let provider_config = app_config.provider_catalog_config();
    let configured_selection = provider_config.resolve_model(None).ok();
    let fallback_model = model_catalog
        .resolve_for_turn(None)
        .context("builtin model catalog does not contain a visible onboarding model")?;
    let model = if onboarding_mode {
        fallback_model.slug.clone()
    } else {
        provider_config
            .model
            .clone()
            .or_else(|| {
                configured_selection
                    .as_ref()
                    .map(|selection| format!("{}/{}", selection.provider_id, selection.model_id))
            })
            .unwrap_or_else(|| fallback_model.slug.clone())
    };
    let model_metadata = model_catalog.get(&model).unwrap_or(fallback_model);
    let provider = configured_selection
        .as_ref()
        .map(|selection| selection.wire_api)
        .unwrap_or_else(|| model_metadata.provider_wire_api());
    let model_binding_id = (!onboarding_mode).then(|| model.clone());

    // convert to TUI `SavedModelEntry` type.
    // the `SaveModelEntry` seems utilized to display model at TUI.
    // TODO: Investigate  whether we could simplify it, unify model structure.
    let saved_models = saved_model_entries(&app_config);

    tracing::info!("starting interactive tui");
    let exit = run_interactive_tui(InteractiveTuiConfig {
        // initial_session corresponding fields at top of `config.toml`.
        initial_session: InitialTuiSession {
            session_id: initial_session_id,
            model,
            request_model: None,
            model_binding_id,
            provider,
            reasoning_effort_selection: provider_config.reasoning_effort.clone(),
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

/// Converts persisted Connection models into TUI model-picker entries.
fn saved_model_entries(app_config: &AppConfig) -> Vec<SavedModelEntry> {
    app_config
        .provider_catalog_config()
        .providers
        .into_iter()
        .filter(|(_, provider)| provider.enabled != Some(false))
        .flat_map(|(provider_id, provider)| {
            let provider_name = provider
                .name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| provider_id.clone());
            let provider_wire_api = provider
                .wire_api
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
            provider
                .models
                .into_iter()
                .filter(|(_, model)| model.enabled != Some(false))
                .map(move |(model_id, model)| {
                    let model_ref = format!("{provider_id}/{model_id}");
                    SavedModelEntry {
                        binding_id: Some(model_ref.clone()),
                        model: model_ref,
                        request_model: None,
                        display_name: model.name,
                        provider_id: Some(provider_id.clone()),
                        provider_name: Some(provider_name.clone()),
                        wire_api: model.wire_api.unwrap_or(provider_wire_api),
                        base_url: provider.base_url.clone(),
                        api_key: None,
                    }
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::initial_permission_preset;
    use super::initial_sandbox_profile;
    use super::saved_model_entries;
    use devo_core::AppConfig;
    use devo_core::ProjectConfig;
    use devo_core::ProviderConfigEntry;
    use devo_core::ProviderConfigFile;
    use devo_core::ProviderModelConfig;
    use devo_protocol::PermissionPreset;
    use devo_protocol::ProviderWireApi;
    use devo_tui::SavedModelEntry;

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
    fn saved_model_entries_use_canonical_connection_model_references() {
        let app_config = AppConfig {
            provider_catalog: ProviderConfigFile {
                providers: BTreeMap::from([(
                    "openai".to_string(),
                    ProviderConfigEntry {
                        name: Some("OpenAI".to_string()),
                        base_url: Some("https://provider.example".to_string()),
                        wire_api: Some(ProviderWireApi::OpenAIResponses),
                        models: BTreeMap::from([(
                            "gpt-test".to_string(),
                            ProviderModelConfig {
                                name: Some("GPT Test".to_string()),
                                ..ProviderModelConfig::default()
                            },
                        )]),
                        ..ProviderConfigEntry::default()
                    },
                )]),
                ..ProviderConfigFile::default()
            },
            ..AppConfig::default()
        };

        assert_eq!(
            saved_model_entries(&app_config),
            vec![SavedModelEntry {
                binding_id: Some("openai/gpt-test".to_string()),
                model: "openai/gpt-test".to_string(),
                request_model: None,
                display_name: Some("GPT Test".to_string()),
                provider_id: Some("openai".to_string()),
                provider_name: Some("OpenAI".to_string()),
                wire_api: ProviderWireApi::OpenAIResponses,
                base_url: Some("https://provider.example".to_string()),
                api_key: None,
            }]
        );
    }
}
