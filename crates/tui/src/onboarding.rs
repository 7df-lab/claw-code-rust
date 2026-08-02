use anyhow::Context;
use anyhow::Result;
use devo_core::ModelBindingConfig;
use devo_core::ProviderConfigSection;
use devo_core::ProviderDefaultsConfig;
use devo_core::ProviderVendorConfig;
use devo_core::provider_id_for_endpoint;
use devo_core::upsert_user_auth_api_key;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_util_paths::find_devo_home;
use std::collections::BTreeMap;
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnboardingModelBinding {
    pub model_slug: String,
    pub request_model: String,
    pub display_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub invocation_method: ProviderWireApi,
    pub default_reasoning_effort: Option<String>,
}

/// Persists the onboarding choice into the user's `config.toml`.
pub(crate) fn save_onboarding_config(
    binding: &OnboardingModelBinding,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    let config_home = find_devo_home().context("could not determine user config path")?;
    save_onboarding_config_to_dir(&config_home, binding, base_url, api_key)
}

pub(crate) fn onboarding_provider_vendor(
    binding: &OnboardingModelBinding,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> ProviderVendor {
    let provider_id = provider_id_for_binding(binding, base_url);
    ProviderVendor {
        name: provider_id.clone(),
        base_url: normalized_optional(base_url).map(ToOwned::to_owned),
        credential: normalized_optional(api_key).map(|_| credential_id_for_provider(&provider_id)),
        headers: None,
        wire_apis: vec![binding.invocation_method],
        enabled: true,
    }
}

pub(crate) fn onboarding_provider_model_binding(
    binding: &OnboardingModelBinding,
    base_url: Option<&str>,
) -> ProviderModelBinding {
    let provider_id = provider_id_for_binding(binding, base_url);
    ProviderModelBinding {
        binding_id: model_binding_id(&binding.model_slug, &provider_id),
        model_slug: binding.model_slug.clone(),
        provider: provider_id,
        request_model: binding.request_model.clone(),
        display_name: Some(binding.display_name.clone()),
        invocation_method: binding.invocation_method,
        default_reasoning_effort: binding.default_reasoning_effort.clone(),
        enabled: true,
    }
}

fn save_onboarding_config_to_dir(
    config_home: &std::path::Path,
    binding: &OnboardingModelBinding,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    let path = config_home.join("config.toml");
    let provider_id = provider_id_for_binding(binding, base_url);
    let credential_id = normalized_optional(api_key)
        .map(|api_key| {
            let credential_id = credential_id_for_provider(&provider_id);
            upsert_user_auth_api_key(config_home, &credential_id, api_key).map(|()| credential_id)
        })
        .transpose()?;

    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };

    root = merge_onboarding_config(root, binding, base_url, credential_id.as_deref())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn save_last_used_model(
    wire_api: Option<ProviderWireApi>,
    provider: ProviderWireApi,
    model: &str,
) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_last_used_model(root, wire_api, provider, model)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

#[allow(dead_code)]
pub(crate) fn save_reasoning_effort_selection(selection: Option<&str>) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_reasoning_effort_selection(root, selection)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn save_compaction_token_limit(limit: u64) -> Result<()> {
    let config_home = find_devo_home().context("could not determine user config path")?;
    let mut store = devo_core::AppConfigStore::load(config_home, /*workspace_root*/ None)
        .map_err(|error| anyhow::anyhow!(error))?;
    store
        .set_compaction_token_limit(limit)
        .context("failed to persist compaction_token_limit")?;
    Ok(())
}

pub(crate) fn save_default_collaboration_mode(
    mode: devo_protocol::CollaborationMode,
) -> Result<()> {
    let config_home = find_devo_home().context("could not determine user config path")?;
    let mut store = devo_core::AppConfigStore::load(config_home, /*workspace_root*/ None)
        .map_err(|error| anyhow::anyhow!(error))?;
    store
        .set_default_collaboration_mode(mode)
        .context("failed to persist default_collaboration_mode")?;
    Ok(())
}

pub(crate) fn save_theme_selection(name: &str) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_theme_selection(root, name)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn save_project_permission_preset(
    project_key: &str,
    preset: PermissionPreset,
) -> Result<()> {
    save_project_config_value(
        project_key,
        "permission_preset",
        permission_preset_to_config_value(preset),
    )
}

pub(crate) fn save_project_sandbox_profile(project_key: &str, profile: &str) -> Result<()> {
    save_project_config_value(project_key, "sandbox_profile", profile)
}

fn save_project_config_value(project_key: &str, key: &str, value: &str) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_project_config_value(root, project_key, key, value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_theme_selection() -> Option<String> {
    let path = find_devo_home().ok()?.join("config.toml");
    let data = std::fs::read_to_string(&path).ok()?;
    let root: Value = data.parse().ok()?;
    root.get("theme")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub(crate) fn save_collapse_reasoning(collapsed: bool) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_collapse_reasoning(root, collapsed)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_collapse_reasoning() -> bool {
    let path = find_devo_home().ok().map(|home| home.join("config.toml"));
    let Some(path) = path else {
        return true;
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(root) = data.parse::<Value>() else {
        return true;
    };
    root.get("collapse_reasoning")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn merge_project_config_value(
    mut root: Value,
    project_key: &str,
    key: &str,
    value: &str,
) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    let projects = table
        .entry("projects".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let projects_table = projects
        .as_table_mut()
        .context("projects must be a TOML table")?;
    let project = projects_table
        .entry(project_key.to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let project_table = project
        .as_table_mut()
        .context("project permission entry must be a TOML table")?;
    project_table.insert(key.to_string(), Value::String(value.to_string()));
    Ok(root)
}

fn merge_theme_selection(mut root: Value, name: &str) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    table.insert("theme".to_string(), Value::String(name.to_string()));
    Ok(root)
}

fn merge_collapse_reasoning(mut root: Value, collapsed: bool) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    table.insert("collapse_reasoning".to_string(), Value::Boolean(collapsed));
    Ok(root)
}

fn permission_preset_to_config_value(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::Default => "default",
        PermissionPreset::AutoReview => "auto-review",
        PermissionPreset::FullAccess => "full-access",
    }
}

#[allow(dead_code)]
fn merge_reasoning_effort_selection(mut root: Value, selection: Option<&str>) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    match normalized_optional(selection) {
        Some(value) => {
            table.insert(
                "model_reasoning_effort_selection".to_string(),
                Value::String(value.to_string()),
            );
        }
        None => {
            table.remove("model_reasoning_effort_selection");
        }
    }
    Ok(root)
}

fn merge_onboarding_config(
    mut root: Value,
    binding_config: &OnboardingModelBinding,
    base_url: Option<&str>,
    credential_id: Option<&str>,
) -> Result<Value> {
    // Preserve unrelated config keys while updating only the onboarding-selected
    // provider profile.
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    let provider_id = provider_id_for_binding(binding_config, base_url);
    let binding_id = model_binding_id(&binding_config.model_slug, &provider_id);
    let provider_name = normalized_optional(Some(&binding_config.provider_name))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| provider_id.clone());
    let provider_section = ProviderConfigSection {
        defaults: ProviderDefaultsConfig {
            model_binding: Some(binding_id.clone()),
        },
        model_provider: Some(provider_id.clone()),
        model: Some(binding_config.model_slug.clone()),
        providers: BTreeMap::from([(
            provider_id.clone(),
            ProviderVendorConfig {
                name: provider_name,
                base_url: normalized_optional(base_url).map(ToOwned::to_owned),
                credential: normalized_optional(credential_id).map(ToOwned::to_owned),
                headers: None,
                wire_apis: vec![binding_config.invocation_method],
                web_search: None,
                web_fetch: None,
                enabled: true,
            },
        )]),
        model_bindings: BTreeMap::from([(
            binding_id,
            ModelBindingConfig {
                model_slug: binding_config.model_slug.clone(),
                provider: provider_id,
                request_model: binding_config.request_model.clone(),
                display_name: Some(binding_config.display_name.clone()),
                invocation_method: binding_config.invocation_method,
                default_reasoning_effort: binding_config.default_reasoning_effort.clone(),
                web_search: None,
                web_fetch: None,
                enabled: true,
            },
        )]),
        ..ProviderConfigSection::default()
    };
    overlay_provider_section(table, &provider_section)?;
    Ok(root)
}

fn overlay_provider_section(
    table: &mut toml::map::Map<String, Value>,
    section: &ProviderConfigSection,
) -> Result<()> {
    let replacement =
        toml::Value::try_from(section).context("failed to serialize onboarding provider config")?;
    let replacement = replacement
        .as_table()
        .context("provider config section must serialize to a TOML table")?;

    overlay_optional_key(table, replacement, "model_provider");
    overlay_optional_key(table, replacement, "model");
    overlay_optional_key(table, replacement, "defaults");

    overlay_nested_known_fields(
        table,
        replacement,
        "providers",
        &["name", "base_url", "credential", "wire_apis", "enabled"],
    )?;
    overlay_nested_known_fields(
        table,
        replacement,
        "model_bindings",
        &[
            "model_slug",
            "provider",
            "request_model",
            "display_name",
            "invocation_method",
            "default_reasoning_effort",
            "enabled",
        ],
    )?;
    if let Some(model_bindings) = table
        .get_mut("model_bindings")
        .and_then(Value::as_table_mut)
    {
        for binding_id in section.model_bindings.keys() {
            if let Some(binding) = model_bindings
                .get_mut(binding_id)
                .and_then(Value::as_table_mut)
            {
                binding.remove("model_name");
            }
        }
    }
    Ok(())
}

fn overlay_nested_known_fields(
    table: &mut toml::map::Map<String, Value>,
    replacement: &toml::map::Map<String, Value>,
    section_key: &str,
    field_keys: &[&str],
) -> Result<()> {
    let replacement_entries = replacement
        .get(section_key)
        .and_then(Value::as_table)
        .cloned()
        .unwrap_or_default();
    let section = table
        .entry(section_key.to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .with_context(|| format!("{section_key} must be a TOML table"))?;
    for (entry_id, replacement_entry) in replacement_entries {
        let replacement_entry = replacement_entry
            .as_table()
            .with_context(|| format!("{section_key}.{entry_id} must be a TOML table"))?;
        let entry = section
            .entry(entry_id)
            .or_insert_with(|| Value::Table(Default::default()))
            .as_table_mut()
            .with_context(|| format!("{section_key} entry must be a TOML table"))?;
        for key in field_keys {
            overlay_optional_key(entry, replacement_entry, key);
        }
    }
    Ok(())
}

fn overlay_optional_key(
    table: &mut toml::map::Map<String, Value>,
    replacement: &toml::map::Map<String, Value>,
    key: &str,
) {
    if let Some(value) = replacement.get(key) {
        table.insert(key.to_string(), value.clone());
    } else {
        table.remove(key);
    }
}

fn merge_last_used_model(
    mut root: Value,
    wire_api: Option<ProviderWireApi>,
    provider: ProviderWireApi,
    model: &str,
) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    if let Some((provider_id, model_slug)) = existing_model_binding_selection(table, model) {
        let defaults = table
            .entry("defaults".to_string())
            .or_insert_with(|| Value::Table(Default::default()))
            .as_table_mut()
            .context("defaults must be a TOML table")?;
        defaults.insert(
            "model_binding".to_string(),
            Value::String(model.to_string()),
        );
        table.insert("model_provider".to_string(), Value::String(provider_id));
        table.insert("model".to_string(), Value::String(model_slug));
        return Ok(root);
    }
    let provider_id = current_provider_id(table, &provider, model);
    let binding_id = current_model_binding_id(table, &provider_id, model)
        .unwrap_or_else(|| model_binding_id(model, &provider_id));
    let defaults = table
        .entry("defaults".to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .context("defaults must be a TOML table")?;
    defaults.insert(
        "model_binding".to_string(),
        Value::String(binding_id.clone()),
    );
    table.insert(
        "model_provider".to_string(),
        Value::String(provider_id.clone()),
    );
    table.insert("model".to_string(), Value::String(model.to_string()));

    let providers = table
        .entry("providers".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let providers_table = providers
        .as_table_mut()
        .context("providers must be a TOML table")?;
    let profile = providers_table
        .entry(provider_id.clone())
        .or_insert_with(|| Value::Table(Default::default()));
    let profile_table = profile
        .as_table_mut()
        .context("provider config must be a TOML table")?;
    if let Some(wire_api) = wire_api.or_else(|| {
        profile_table
            .get("wire_apis")
            .and_then(Value::as_array)
            .and_then(|apis| apis.first())
            .and_then(Value::as_str)
            .and_then(provider_wire_api_from_str)
    }) {
        profile_table.insert(
            "wire_apis".to_string(),
            Value::Array(vec![Value::String(
                wire_api_to_string(wire_api).to_string(),
            )]),
        );
        let model_bindings = table
            .entry("model_bindings".to_string())
            .or_insert_with(|| Value::Table(Default::default()))
            .as_table_mut()
            .context("model_bindings must be a TOML table")?;
        let binding = model_bindings
            .entry(binding_id)
            .or_insert_with(|| Value::Table(Default::default()))
            .as_table_mut()
            .context("model binding must be a TOML table")?;
        binding.insert("enabled".to_string(), Value::Boolean(true));
        binding.insert("model_slug".to_string(), Value::String(model.to_string()));
        binding.insert("provider".to_string(), Value::String(provider_id));
        binding.insert(
            "request_model".to_string(),
            Value::String(model.to_string()),
        );
        binding.remove("model_name");
        binding.insert(
            "invocation_method".to_string(),
            Value::String(wire_api_to_string(wire_api).to_string()),
        );
    }
    Ok(root)
}

fn current_provider_id(
    table: &toml::map::Map<String, Value>,
    provider: &ProviderWireApi,
    model: &str,
) -> String {
    table
        .get("model_bindings")
        .and_then(Value::as_table)
        .and_then(|bindings| {
            bindings.values().find_map(|value| {
                let binding = value.as_table()?;
                let matches_model = binding.get("model_slug").and_then(Value::as_str)
                    == Some(model)
                    || binding
                        .get("request_model")
                        .or_else(|| binding.get("model_name"))
                        .and_then(Value::as_str)
                        == Some(model);
                let matches_provider = binding
                    .get("invocation_method")
                    .and_then(Value::as_str)
                    .and_then(provider_wire_api_from_str)
                    == Some(*provider);
                (matches_model && matches_provider)
                    .then(|| {
                        binding
                            .get("provider")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .flatten()
            })
        })
        .or_else(|| {
            table
                .get("providers")
                .and_then(Value::as_table)
                .and_then(|providers| {
                    providers.iter().find_map(|(provider_id, value)| {
                        let profile = value.as_table()?;
                        let matches_provider = profile
                            .get("wire_apis")
                            .and_then(Value::as_array)
                            .is_some_and(|wire_apis| {
                                wire_apis.iter().any(|wire_api| {
                                    wire_api.as_str().and_then(provider_wire_api_from_str)
                                        == Some(*provider)
                                })
                            });
                        matches_provider.then(|| provider_id.clone())
                    })
                })
        })
        .or_else(|| {
            table
                .get("model_providers")
                .and_then(Value::as_table)
                .and_then(|providers| {
                    providers.iter().find_map(|(provider_id, value)| {
                        let profile = value.as_table()?;
                        let contains_model = profile
                            .get("models")
                            .and_then(Value::as_array)
                            .is_some_and(|models| {
                                models.iter().any(|entry| {
                                    entry
                                        .as_table()
                                        .and_then(|model_entry| model_entry.get("model"))
                                        .and_then(Value::as_str)
                                        == Some(model)
                                })
                            });
                        let matches_last_model =
                            profile.get("last_model").and_then(Value::as_str) == Some(model);
                        let matches_default_model =
                            profile.get("default_model").and_then(Value::as_str) == Some(model);
                        (contains_model || matches_last_model || matches_default_model)
                            .then(|| provider_id.clone())
                    })
                })
                .or_else(|| {
                    table
                        .get("model_provider")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .or_else(|| {
                    table
                        .get("model_providers")
                        .and_then(Value::as_table)
                        .and_then(|providers| {
                            providers.iter().find_map(|(provider_id, value)| {
                                let profile = value.as_table()?;
                                let wire_api = profile.get("wire_api")?.as_str()?;
                                let matches_provider = match provider {
                                    ProviderWireApi::AnthropicMessages => {
                                        wire_api == ProviderWireApi::AnthropicMessages.as_str()
                                    }
                                    ProviderWireApi::OpenAIResponses => {
                                        wire_api == ProviderWireApi::OpenAIResponses.as_str()
                                    }
                                    ProviderWireApi::OpenAIChatCompletions => {
                                        wire_api == ProviderWireApi::OpenAIChatCompletions.as_str()
                                    }
                                };
                                matches_provider.then(|| provider_id.clone())
                            })
                        })
                })
        })
        .unwrap_or_else(|| provider_id_for_endpoint(provider, None))
}

fn current_model_binding_id(
    table: &toml::map::Map<String, Value>,
    provider_id: &str,
    model: &str,
) -> Option<String> {
    table
        .get("model_bindings")
        .and_then(Value::as_table)
        .and_then(|bindings| {
            bindings.iter().find_map(|(binding_id, value)| {
                let binding = value.as_table()?;
                let matches_provider =
                    binding.get("provider").and_then(Value::as_str) == Some(provider_id);
                let matches_model = binding.get("model_slug").and_then(Value::as_str)
                    == Some(model)
                    || binding
                        .get("request_model")
                        .or_else(|| binding.get("model_name"))
                        .and_then(Value::as_str)
                        == Some(model);
                (matches_provider && matches_model).then(|| binding_id.clone())
            })
        })
}

fn existing_model_binding_selection(
    table: &toml::map::Map<String, Value>,
    binding_id: &str,
) -> Option<(String, String)> {
    let binding = table
        .get("model_bindings")
        .and_then(Value::as_table)?
        .get(binding_id)?
        .as_table()?;
    let provider_id = binding.get("provider").and_then(Value::as_str)?;
    let model_slug = binding.get("model_slug").and_then(Value::as_str)?;
    Some((provider_id.to_string(), model_slug.to_string()))
}

fn model_binding_id(model: &str, provider_id: &str) -> String {
    format!("{}-{}", slug_component(model), slug_component(provider_id))
        .trim_matches('-')
        .to_string()
}

fn provider_id_for_binding(binding: &OnboardingModelBinding, base_url: Option<&str>) -> String {
    normalized_optional(Some(&binding.provider_id))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            provider_id_for_endpoint(&binding.invocation_method, normalized_optional(base_url))
        })
}

fn credential_id_for_provider(provider_id: &str) -> String {
    format!("{}_api_key", slug_component(provider_id).replace('-', "_"))
}

fn slug_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn normalized_optional(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn provider_wire_api_from_str(value: &str) -> Option<ProviderWireApi> {
    match value.trim().to_ascii_lowercase().as_str() {
        "chat_completion"
        | "chat_completions"
        | "openai"
        | "openai_chat_completion"
        | "openai_chat_completions" => Some(ProviderWireApi::OpenAIChatCompletions),
        "responses" | "openai_responses" => Some(ProviderWireApi::OpenAIResponses),
        "anthropic" | "messages" | "anthropic_messages" => Some(ProviderWireApi::AnthropicMessages),
        _ => None,
    }
}

fn wire_api_to_string(wire_api: ProviderWireApi) -> &'static str {
    wire_api.as_str()
}

fn upsert_model_entry(
    models: &mut Vec<Value>,
    model: &str,
    base_url: Option<&str>,
    api_key: Option<&str>,
) {
    // Keep exactly one entry per model slug so repeated onboarding runs replace
    // the existing profile instead of appending duplicates.
    let mut entry = toml::map::Map::new();
    entry.insert("model".to_string(), Value::String(model.to_string()));
    if let Some(base_url) = base_url {
        entry.insert("base_url".to_string(), Value::String(base_url.to_string()));
    }
    if let Some(api_key) = api_key {
        entry.insert("api_key".to_string(), Value::String(api_key.to_string()));
    }

    if let Some(existing) = models.iter_mut().find(|value| {
        value
            .as_table()
            .and_then(|table| table.get("model"))
            .and_then(Value::as_str)
            == Some(model)
    }) {
        *existing = Value::Table(entry);
    } else {
        models.push(Value::Table(entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_core::AuthCredentialConfig;
    use devo_core::AuthCredentialKind;
    use devo_core::UserAuthConfigFile;
    use devo_core::read_user_auth_config;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalized_optional_trims_and_drops_empty_values() {
        assert_eq!(
            normalized_optional(Some("  https://example.com  ")),
            Some("https://example.com")
        );
        assert_eq!(normalized_optional(Some("   ")), None);
        assert_eq!(normalized_optional(None), None);
    }

    #[test]
    fn onboarding_provider_vendor_uses_provider_id_and_auth_reference() {
        let binding_config = OnboardingModelBinding {
            model_slug: "qwen3-coder-next".to_string(),
            request_model: "qwen3-coder-next".to_string(),
            display_name: "Qwen3 Coder Next".to_string(),
            provider_id: "openai_chat_completions".to_string(),
            provider_name: "OpenAI".to_string(),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
        };

        assert_eq!(
            onboarding_provider_vendor(
                &binding_config,
                Some(" https://example.com/v1 "),
                Some("sk-test-secret")
            ),
            ProviderVendor {
                name: "openai_chat_completions".to_string(),
                base_url: Some("https://example.com/v1".to_string()),
                credential: Some("openai_chat_completions_api_key".to_string()),
                headers: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            }
        );
    }

    #[test]
    fn merge_onboarding_config_creates_provider_and_model_binding() {
        let root = Value::Table(Default::default());
        let binding_config = OnboardingModelBinding {
            model_slug: "qwen3-coder-next".to_string(),
            request_model: "qwen3-coder-next".to_string(),
            display_name: "Qwen3 Coder Next".to_string(),
            provider_id: "openai_chat_completions".to_string(),
            provider_name: "OpenAI".to_string(),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: Some("medium".to_string()),
        };
        let merged = merge_onboarding_config(
            root,
            &binding_config,
            Some("https://example.com/v1"),
            Some("openai_chat_completions_api_key"),
        )
        .expect("merge");

        let table = merged.as_table().expect("table");
        assert_eq!(
            table
                .get("defaults")
                .and_then(Value::as_table)
                .and_then(|defaults| defaults.get("model_binding"))
                .and_then(Value::as_str),
            Some("qwen3-coder-next-openai-chat-completions")
        );

        let profile = table
            .get("providers")
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("openai_chat_completions"))
            .and_then(Value::as_table)
            .expect("provider profile");
        assert_eq!(profile.get("name").and_then(Value::as_str), Some("OpenAI"));
        assert_eq!(
            profile
                .get("wire_apis")
                .and_then(Value::as_array)
                .and_then(|wire_apis| wire_apis.first())
                .and_then(Value::as_str),
            Some("openai_chat_completions")
        );
        assert_eq!(
            profile.get("base_url").and_then(Value::as_str),
            Some("https://example.com/v1")
        );
        assert_eq!(
            profile.get("credential").and_then(Value::as_str),
            Some("openai_chat_completions_api_key")
        );

        let binding = table
            .get("model_bindings")
            .and_then(Value::as_table)
            .and_then(|bindings| bindings.get("qwen3-coder-next-openai-chat-completions"))
            .and_then(Value::as_table)
            .expect("model binding");
        assert_eq!(
            binding.get("model_slug").and_then(Value::as_str),
            Some("qwen3-coder-next")
        );
        assert_eq!(
            binding.get("provider").and_then(Value::as_str),
            Some("openai_chat_completions")
        );
        assert_eq!(
            binding.get("request_model").and_then(Value::as_str),
            Some("qwen3-coder-next")
        );
        assert_eq!(
            binding.get("display_name").and_then(Value::as_str),
            Some("Qwen3 Coder Next")
        );
        assert_eq!(
            binding
                .get("default_reasoning_effort")
                .and_then(Value::as_str),
            Some("medium")
        );
    }

    #[test]
    fn merge_onboarding_config_migrates_legacy_model_name_without_losing_custom_fields() {
        let mut root = Value::Table(Default::default());
        {
            let table = root.as_table_mut().expect("table");
            let mut providers = toml::map::Map::new();
            providers.insert(
                "openai_chat_completions".to_string(),
                Value::Table(Default::default()),
            );
            table.insert("providers".to_string(), Value::Table(providers));
            let mut binding = toml::map::Map::new();
            binding.insert(
                "model_slug".to_string(),
                Value::String("qwen3-coder-next".to_string()),
            );
            binding.insert(
                "provider".to_string(),
                Value::String("openai_chat_completions".to_string()),
            );
            binding.insert(
                "model_name".to_string(),
                Value::String("old-provider-name".to_string()),
            );
            binding.insert(
                "custom_binding_key".to_string(),
                Value::String("preserved".to_string()),
            );
            binding.insert(
                "invocation_method".to_string(),
                Value::String("openai_chat_completions".to_string()),
            );
            let mut bindings = toml::map::Map::new();
            bindings.insert(
                "qwen3-coder-next-openai-chat-completions".to_string(),
                Value::Table(binding),
            );
            table.insert("model_bindings".to_string(), Value::Table(bindings));
        }

        let binding_config = OnboardingModelBinding {
            model_slug: "qwen3-coder-next".to_string(),
            request_model: "qwen3-coder-next".to_string(),
            display_name: "Qwen3 Coder Next".to_string(),
            provider_id: "openai_chat_completions".to_string(),
            provider_name: "OpenAI".to_string(),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
        };
        let merged = merge_onboarding_config(
            root,
            &binding_config,
            Some("https://new.example/v1"),
            Some("openai_chat_completions_api_key"),
        )
        .expect("merge");

        let table = merged.as_table().expect("table");
        let profile = table
            .get("providers")
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("openai_chat_completions"))
            .and_then(Value::as_table)
            .expect("provider");
        assert_eq!(
            profile.get("base_url").and_then(Value::as_str),
            Some("https://new.example/v1")
        );
        assert_eq!(
            profile.get("credential").and_then(Value::as_str),
            Some("openai_chat_completions_api_key")
        );
        let binding = table
            .get("model_bindings")
            .and_then(Value::as_table)
            .and_then(|bindings| bindings.get("qwen3-coder-next-openai-chat-completions"))
            .and_then(Value::as_table)
            .expect("binding");
        assert_eq!(
            binding.get("request_model").and_then(Value::as_str),
            Some("qwen3-coder-next")
        );
        assert_eq!(binding.get("model_name"), None);
        assert_eq!(
            binding.get("custom_binding_key").and_then(Value::as_str),
            Some("preserved")
        );
        assert_eq!(
            table
                .get("defaults")
                .and_then(Value::as_table)
                .and_then(|defaults| defaults.get("model_binding"))
                .and_then(Value::as_str),
            Some("qwen3-coder-next-openai-chat-completions")
        );
    }

    #[test]
    fn save_onboarding_config_writes_config_reference_and_user_auth_secret() {
        let dir = tempfile::tempdir().expect("temp dir");
        let binding_config = OnboardingModelBinding {
            model_slug: "qwen3-coder-next".to_string(),
            request_model: "qwen3-coder-next".to_string(),
            display_name: "Qwen3 Coder Next".to_string(),
            provider_id: "openai_chat_completions".to_string(),
            provider_name: "OpenAI".to_string(),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
        };

        save_onboarding_config_to_dir(
            dir.path(),
            &binding_config,
            Some("https://example.com/v1"),
            Some("sk-test-secret"),
        )
        .expect("save onboarding config");

        let config = std::fs::read_to_string(dir.path().join("config.toml")).expect("read config");
        let config: Value = config.parse().expect("parse config");
        assert_eq!(
            config["providers"]["openai_chat_completions"]["credential"].as_str(),
            Some("openai_chat_completions_api_key")
        );
        assert!(
            config["providers"]["openai_chat_completions"]
                .get("api_key")
                .is_none()
        );
        assert_eq!(
            read_user_auth_config(&dir.path().join("auth.json")).expect("load auth"),
            UserAuthConfigFile {
                credentials: [(
                    "openai_chat_completions_api_key".to_string(),
                    AuthCredentialConfig {
                        kind: AuthCredentialKind::ApiKey,
                        value: "sk-test-secret".to_string(),
                    },
                )]
                .into_iter()
                .collect(),
                ..UserAuthConfigFile::default()
            }
        );
    }

    #[test]
    fn legacy_model_name_remains_discoverable_for_provider_and_binding_lookup() {
        let root: Value = r#"
[providers.openrouter]
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
model_slug = "catalog-model"
provider = "openrouter"
model_name = "vendor/model"
invocation_method = "openai_chat_completions"
"#
        .parse()
        .expect("parse");
        let table = root.as_table().expect("table");

        assert_eq!(
            current_provider_id(
                table,
                &ProviderWireApi::OpenAIChatCompletions,
                "vendor/model",
            ),
            "openrouter"
        );
        assert_eq!(
            current_model_binding_id(table, "openrouter", "vendor/model"),
            Some("main".to_string())
        );
    }

    #[test]
    fn merge_last_used_model_prefers_profile_that_contains_model() {
        let root: Value = r#"
model_provider = "anthropic"

[model_providers.anthropic]
wire_api = "anthropic_messages"

[[model_providers.anthropic.models]]
model = "claude-sonnet-4"

[model_providers.openai]
wire_api = "openai_chat_completions"

[[model_providers.openai.models]]
model = "gpt-5.4"
"#
        .parse()
        .expect("parse");

        let merged =
            merge_last_used_model(root, None, ProviderWireApi::AnthropicMessages, "gpt-5.4")
                .expect("merge");

        let table = merged.as_table().expect("table");
        assert_eq!(
            table.get("model_provider").and_then(Value::as_str),
            Some("openai")
        );
        assert_eq!(
            table
                .get("model_providers")
                .and_then(Value::as_table)
                .and_then(|providers| providers.get("openai"))
                .and_then(Value::as_table)
                .and_then(|profile| profile.get("wire_api"))
                .and_then(Value::as_str),
            Some("openai_chat_completions")
        );
    }

    #[test]
    fn merge_last_used_model_accepts_existing_model_binding_id() {
        let root: Value = r#"
model_provider = "deepseek"
model = "deepseek-v4-flash"

[defaults]
model_binding = "deepseek-v4-flash-deepseek"

[providers.deepseek]
wire_apis = ["openai_chat_completions"]

[providers.openrouter]
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-deepseek]
model_slug = "deepseek-v4-flash"
provider = "deepseek"
request_model = "deepseek-v4-flash"
invocation_method = "openai_chat_completions"

[model_bindings.deepseek-v4-flash-openrouter]
model_slug = "deepseek-v4-flash"
provider = "openrouter"
request_model = "deepseek-v4-flash"
invocation_method = "openai_chat_completions"
"#
        .parse()
        .expect("parse");

        let merged = merge_last_used_model(
            root,
            None,
            ProviderWireApi::OpenAIChatCompletions,
            "deepseek-v4-flash-openrouter",
        )
        .expect("merge");

        let table = merged.as_table().expect("table");
        assert_eq!(
            table
                .get("defaults")
                .and_then(Value::as_table)
                .and_then(|defaults| defaults.get("model_binding"))
                .and_then(Value::as_str),
            Some("deepseek-v4-flash-openrouter")
        );
        assert_eq!(
            table.get("model_provider").and_then(Value::as_str),
            Some("openrouter")
        );
        assert_eq!(
            table.get("model").and_then(Value::as_str),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            table
                .get("model_bindings")
                .and_then(Value::as_table)
                .map(toml::map::Map::len),
            Some(2)
        );
    }

    #[test]
    fn merge_last_used_model_preserves_existing_wire_api_when_not_provided() {
        let root: Value = r#"
[model_providers.openai]
wire_api = "openai_responses"

[[model_providers.openai.models]]
model = "gpt-5.4"
"#
        .parse()
        .expect("parse");

        let merged = merge_last_used_model(root, None, ProviderWireApi::OpenAIResponses, "gpt-5.4")
            .expect("merge");

        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("model_providers"))
                .and_then(Value::as_table)
                .and_then(|providers| providers.get("openai"))
                .and_then(Value::as_table)
                .and_then(|profile| profile.get("wire_api"))
                .and_then(Value::as_str),
            Some("openai_responses")
        );
    }

    #[test]
    fn merge_reasoning_effort_selection_updates_and_removes_value() {
        let merged =
            merge_reasoning_effort_selection(Value::Table(Default::default()), Some("medium"))
                .expect("merge");
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("model_reasoning_effort_selection"))
                .and_then(Value::as_str),
            Some("medium")
        );

        let removed = merge_reasoning_effort_selection(merged, None).expect("remove");
        assert_eq!(
            removed
                .as_table()
                .and_then(|table| table.get("model_reasoning_effort_selection")),
            None
        );
    }

    #[test]
    fn merge_project_permission_preset_preserves_unrelated_config() {
        let root: Value = r#"
model = "gpt-5.4"

[projects.old]
permission_preset = "default"
custom = "keep"
"#
        .parse()
        .expect("parse");

        let merged =
            merge_project_config_value(root, "C:\\repo", "permission_preset", "full-access")
                .expect("merge");

        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("model"))
                .and_then(Value::as_str),
            Some("gpt-5.4")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("permission_preset"))
                .and_then(Value::as_str),
            Some("default")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("custom"))
                .and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("C:\\repo"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("permission_preset"))
                .and_then(Value::as_str),
            Some("full-access")
        );
    }

    #[test]
    fn merge_project_sandbox_profile_preserves_other_project_keys() {
        let root: Value = r#"
[projects.old]
permission_preset = "default"
sandbox_profile = "strict"
"#
        .parse()
        .expect("parse");

        let merged = merge_project_config_value(root, "C:\\repo", "sandbox_profile", "read-only")
            .expect("merge");

        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("sandbox_profile"))
                .and_then(Value::as_str),
            Some("strict")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("C:\\repo"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("sandbox_profile"))
                .and_then(Value::as_str),
            Some("read-only")
        );
    }
}
