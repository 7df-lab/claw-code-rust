use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

use clawcr_provider::{
    ModelProvider, ProviderFamily, anthropic::AnthropicProvider, openai::OpenAIProvider,
};

/// Resolved provider bootstrap owned by the server runtime.
pub struct ResolvedServerProvider {
    /// Concrete provider used for model requests.
    pub provider: std::sync::Arc<dyn ModelProvider>,
    /// Default model slug used when a session or turn does not request one.
    pub default_model: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderProfile {
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<ModelProfile>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelProfile {
    #[serde(default)]
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AppConfigFile {
    #[serde(default)]
    default_provider: Option<ProviderFamily>,
    #[serde(default)]
    anthropic: ProviderProfile,
    #[serde(default)]
    openai: ProviderProfile,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

/// Loads the server-side provider from config and environment variables.
pub fn load_server_provider(
    config_file: &Path,
    default_model: Option<&str>,
) -> Result<ResolvedServerProvider> {
    let file_config = read_provider_config(config_file).unwrap_or_default();
    let env_provider = env_non_empty("CLAWCR_PROVIDER");
    let env_model = env_non_empty("CLAWCR_MODEL");
    let env_base_url = env_non_empty("CLAWCR_BASE_URL");
    let env_api_key = env_non_empty("CLAWCR_API_KEY");

    let provider_name = env_provider
        .as_deref()
        .and_then(parse_provider_kind)
        .or_else(|| provider_for_model(&file_config, env_model.as_deref()))
        .or(file_config.default_provider)
        .or_else(|| {
            file_config
                .provider
                .as_deref()
                .and_then(parse_provider_kind)
        })
        .or_else(|| infer_default_provider(&file_config))
        .unwrap_or(ProviderFamily::OpenAI);

    let profile = profile_for_provider(&file_config, provider_name);
    let selected_model = select_configured_model(
        profile,
        env_model.as_deref().or(file_config.model.as_deref()),
    );

    let model = env_model
        .or_else(|| selected_model.map(|model| model.model.clone()))
        .or(file_config.model.clone())
        .or_else(|| default_model.map(ToOwned::to_owned))
        .or_else(|| profile.default_model.clone())
        .or_else(|| profile.models.first().map(|model| model.model.clone()))
        .unwrap_or_else(|| default_model_for_provider(provider_name));

    let base_url = env_base_url
        .or_else(|| selected_model.and_then(|model| model.base_url.clone()))
        .or(profile.base_url.clone())
        .or(file_config.base_url.clone())
        .or_else(|| env_non_empty("ANTHROPIC_BASE_URL"))
        .or_else(|| env_non_empty("OPENAI_BASE_URL"));
    let api_key = env_api_key
        .or_else(|| selected_model.and_then(|model| model.api_key.clone()))
        .or(profile.api_key.clone())
        .or(file_config.api_key.clone())
        .or_else(|| env_non_empty("ANTHROPIC_API_KEY"))
        .or_else(|| env_non_empty("ANTHROPIC_AUTH_TOKEN"))
        .or_else(|| env_non_empty("OPENAI_API_KEY"));

    let provider: std::sync::Arc<dyn ModelProvider> = match provider_name {
        ProviderFamily::Anthropic => {
            let api_key = api_key.context("anthropic provider requires an API key")?;
            let base_url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
            std::sync::Arc::new(AnthropicProvider::new(base_url).with_api_key(api_key))
        }
        ProviderFamily::OpenAI => {
            let base_url = normalize_openai_base_url(&base_url.unwrap_or_else(|| {
                // TODO: Figure out, should we put default base url here?
                // Maybe throw an error.
                "https://api.openai.com".to_string()
            }));
            let mut provider = OpenAIProvider::new(base_url);
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            std::sync::Arc::new(provider)
        }
    };

    Ok(ResolvedServerProvider {
        provider,
        default_model: model,
    })
}

fn read_provider_config(config_file: &Path) -> Result<AppConfigFile> {
    if !config_file.exists() {
        return Ok(AppConfigFile::default());
    }
    let contents = fs::read_to_string(config_file)
        .with_context(|| format!("failed to read {}", config_file.display()))?;
    let value: toml::Value = toml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", config_file.display()))?;
    let table = value.as_table().cloned().unwrap_or_default();
    if table.contains_key("default_provider")
        || table.contains_key("anthropic")
        || table.contains_key("openai")
        || table.contains_key("ollama")
        || table.contains_key("models")
        || table.contains_key("default_model")
    {
        let parsed_new: Result<AppConfigFile> =
            value.clone().try_into().map_err(anyhow::Error::from);
        if let Ok(config) = parsed_new {
            return Ok(config);
        }
        let legacy: LegacySectionAppConfig = value
            .try_into()
            .with_context(|| format!("failed to parse {}", config_file.display()))?;
        return Ok(legacy.into_app_config_file());
    }

    let legacy: LegacyFlatAppConfig = value
        .try_into()
        .with_context(|| format!("failed to parse {}", config_file.display()))?;
    Ok(legacy.into_app_config_file())
}

fn profile_for_provider(config: &AppConfigFile, provider: ProviderFamily) -> &ProviderProfile {
    match provider {
        ProviderFamily::Anthropic => &config.anthropic,
        ProviderFamily::OpenAI => &config.openai,
    }
}

fn infer_default_provider(config: &AppConfigFile) -> Option<ProviderFamily> {
    if config.anthropic.default_model.is_some()
        || config.anthropic.base_url.is_some()
        || config.anthropic.api_key.is_some()
        || !config.anthropic.models.is_empty()
    {
        Some(ProviderFamily::Anthropic)
    } else if config.openai.default_model.is_some()
        || config.openai.base_url.is_some()
        || config.openai.api_key.is_some()
        || !config.openai.models.is_empty()
    {
        Some(ProviderFamily::OpenAI)
    } else {
        None
    }
}

fn default_model_for_provider(provider: ProviderFamily) -> String {
    match provider {
        ProviderFamily::Anthropic => "claude-sonnet-4-20250514".to_string(),
        ProviderFamily::OpenAI => "gpt-4o".to_string(),
    }
}

fn select_configured_model<'a>(
    profile: &'a ProviderProfile,
    requested: Option<&str>,
) -> Option<&'a ModelProfile> {
    match requested {
        Some(model) => profile.models.iter().find(|entry| entry.model == model),
        None => profile
            .default_model
            .as_deref()
            .and_then(|default| profile.models.iter().find(|entry| entry.model == default))
            .or_else(|| profile.models.first()),
    }
}

fn provider_for_model(
    config: &AppConfigFile,
    requested_model: Option<&str>,
) -> Option<ProviderFamily> {
    let requested_model = requested_model?;
    for (provider, profile) in [
        (ProviderFamily::Anthropic, &config.anthropic),
        (ProviderFamily::OpenAI, &config.openai),
    ] {
        if profile
            .models
            .iter()
            .any(|entry| entry.model == requested_model)
            || profile.default_model.as_deref() == Some(requested_model)
        {
            return Some(provider);
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyFlatAppConfig {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

impl LegacyFlatAppConfig {
    fn into_app_config_file(self) -> AppConfigFile {
        let provider = self
            .provider
            .as_deref()
            .and_then(parse_provider_kind)
            .unwrap_or(ProviderFamily::Anthropic);
        let model = self
            .model
            .unwrap_or_else(|| default_model_for_provider(provider));
        let profile = ProviderProfile {
            default_model: Some(model.clone()),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            models: vec![ModelProfile {
                model,
                base_url: self.base_url,
                api_key: self.api_key,
            }],
        };
        match provider {
            ProviderFamily::Anthropic => AppConfigFile {
                default_provider: Some(provider),
                anthropic: profile,
                openai: ProviderProfile::default(),
                provider: None,
                model: None,
                base_url: None,
                api_key: None,
            },
            ProviderFamily::OpenAI => AppConfigFile {
                default_provider: Some(provider),
                anthropic: ProviderProfile::default(),
                openai: profile,
                provider: None,
                model: None,
                base_url: None,
                api_key: None,
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LegacySectionAppConfig {
    #[serde(default)]
    default_provider: Option<ProviderFamily>,
    #[serde(default)]
    anthropic: LegacySectionProviderProfile,
    #[serde(default)]
    openai: LegacySectionProviderProfile,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LegacySectionProviderProfile {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

impl LegacySectionAppConfig {
    fn into_app_config_file(self) -> AppConfigFile {
        AppConfigFile {
            default_provider: self.default_provider,
            anthropic: legacy_section_profile_into_provider_profile(self.anthropic),
            openai: legacy_section_profile_into_provider_profile(self.openai),
            provider: None,
            model: None,
            base_url: None,
            api_key: None,
        }
    }
}

fn legacy_section_profile_into_provider_profile(
    legacy: LegacySectionProviderProfile,
) -> ProviderProfile {
    let model = legacy.model.clone();
    ProviderProfile {
        default_model: model.clone(),
        base_url: legacy.base_url.clone(),
        api_key: legacy.api_key.clone(),
        models: model
            .map(|model| ModelProfile {
                model,
                base_url: legacy.base_url,
                api_key: legacy.api_key,
            })
            .into_iter()
            .collect(),
    }
}

fn parse_provider_kind(value: &str) -> Option<ProviderFamily> {
    match value.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderFamily::Anthropic),
        "openai" => Some(ProviderFamily::OpenAI),
        _ => None,
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn normalize_openai_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let Some(scheme_sep) = trimmed.find("://") else {
        return trimmed.to_string();
    };
    let has_explicit_path = trimmed[scheme_sep + 3..].contains('/');
    if has_explicit_path {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::normalize_openai_base_url;

    #[test]
    fn preserves_explicit_openai_compatible_paths() {
        assert_eq!(
            normalize_openai_base_url("https://open.bigmodel.cn/api/paas/v4/"),
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }

    #[test]
    fn appends_v1_for_bare_openai_hosts() {
        assert_eq!(
            normalize_openai_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
    }
}
