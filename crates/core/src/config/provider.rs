use std::collections::BTreeMap;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::ProviderWireApi;
use serde::Deserialize;
use serde::Serialize;
use toml::Value;

use devo_utils::current_user_config_file;

pub fn provider_id_from_base_url(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = trimmed
        .split_once("://")
        .map_or(trimmed, |(_, remainder)| remainder);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

pub fn provider_id_for_endpoint(provider: &ProviderWireApi, base_url: Option<&str>) -> String {
    base_url
        .and_then(provider_id_from_base_url)
        .unwrap_or_else(|| provider.as_str().to_string())
}

pub fn provider_name_for_endpoint(provider: &ProviderWireApi, base_url: Option<&str>) -> String {
    provider_id_for_endpoint(provider, base_url)
}

/// The preferred authentication method for the active provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreferredAuthMethod {
    /// Use an API key or bearer token.
    Apikey,
}

impl<'de> Deserialize<'de> for PreferredAuthMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "apikey" | "api_key" => Ok(Self::Apikey),
            other => Err(serde::de::Error::custom(format!(
                "unsupported preferred_auth_method `{other}`"
            ))),
        }
    }
}

/// One model entry stored under a provider section in `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredModel {
    /// The model slug or custom model name.
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// One provider-specific configuration block that can store many model entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ConfiguredModel>,
}

impl ModelProviderConfig {
    /// Returns whether the profile has no configured values.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.base_url.is_none()
            && self.api_key.is_none()
            && self.wire_api.is_none()
            && self.last_model.is_none()
            && self.default_model.is_none()
            && self.models.is_empty()
    }
}

/// Persisted provider and active model configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Logical thinking selection for the active model, such as `disabled`,
    /// `enabled`, or one effort-like level supported by the selected model.
    ///
    /// This stores the user-facing selection, not a provider-specific request
    /// field. The runtime later resolves it into the final request model,
    /// request `thinking` parameter, effective reasoning effort, and any
    /// provider-specific extra payload.
    #[serde(alias = "model_thinking")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_thinking_selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_auto_compact_token_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_response_storage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_auth_method: Option<PreferredAuthMethod>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_providers: BTreeMap<String, ModelProviderConfig>,
}

/// The fully-resolved provider settings that can be forwarded to a server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderSettings {
    /// Selected provider identifier from `[model_providers.<id>]`.
    pub provider_id: String,
    /// Selected provider transport implementation.
    pub wire_api: ProviderWireApi,
    /// Final model identifier.
    pub model: String,
    /// Optional provider base URL override.
    pub base_url: Option<String>,
    /// Optional provider API key override.
    pub api_key: Option<String>,
    /// Optional active model auto-compaction threshold in tokens.
    pub model_auto_compact_token_limit: Option<u32>,
    /// Optional active model context window override in tokens.
    pub model_context_window: Option<u32>,
    /// Optional logical thinking selection for the active model.
    pub model_thinking_selection: Option<String>,
    /// Whether provider-side response storage should be disabled.
    pub disable_response_storage: bool,
    /// Preferred authentication method for the active provider.
    pub preferred_auth_method: Option<PreferredAuthMethod>,
}

/// Loads the user's provider config file from the standard config path.
pub fn load_config() -> Result<ProviderConfigFile> {
    let path = current_user_config_file().context("could not determine user config path")?;
    if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        return parse_config_str(&data)
            .with_context(|| format!("failed to parse {}", path.display()));
    }

    Ok(ProviderConfigFile::default())
}

/// Parses provider config TOML from a string.
pub fn parse_config_str(data: &str) -> Result<ProviderConfigFile> {
    let value: Value = toml::from_str(data)?;
    parse_config_value(value)
}

/// Parses provider config TOML.
pub fn parse_config_value(value: Value) -> Result<ProviderConfigFile> {
    Ok(value.try_into()?)
}

/// Resolves provider settings without constructing a local provider instance.
pub fn resolve_provider_settings() -> Result<ResolvedProviderSettings> {
    resolve_provider_settings_from_config(&load_config().unwrap_or_default())
}

/// Resolves the effective provider settings from persisted provider config.
///
/// `file` contains the full provider config loaded from disk. Selection prefers
/// the explicitly active provider/model first, then falls back to matching model
/// ownership, provider defaults, and finally the first configured provider/model.
pub(crate) fn resolve_provider_settings_from_config(
    file: &ProviderConfigFile,
) -> Result<ResolvedProviderSettings> {
    // Prefer an explicitly selected provider, but ignore stale selections that
    // no longer have a matching provider profile in the config.
    let provider_id = file
        .model_provider
        .as_deref()
        .filter(|provider_id| file.model_providers.contains_key(*provider_id))
        .map(ToOwned::to_owned)
        .or_else(|| provider_id_for_model(file, file.model.as_deref()))
        .or_else(|| first_configured_provider_id(file))
        .context("No provider configured. Run `devo onboard` to complete setup.")?;
    let provider_config = file
        .model_providers
        .get(&provider_id)
        .with_context(|| format!("configured provider `{provider_id}` was not found"))?;
    // Resolve the active model in user-intent order: explicit active model,
    // provider's last/default model, first model in that profile, then any
    // configured model as a final compatibility fallback.
    let model = file
        .model
        .clone()
        .or_else(|| provider_config.last_model.clone())
        .or_else(|| provider_config.default_model.clone())
        .or_else(|| {
            provider_config
                .models
                .first()
                .map(|entry| entry.model.clone())
        })
        .or_else(|| first_configured_model(file))
        .context("No model configured. Run `devo onboard` to complete setup.")?;
    let matched_model = provider_config
        .models
        .iter()
        .find(|entry| entry.model == model);
    let wire_api = provider_config
        .wire_api
        .unwrap_or(ProviderWireApi::OpenAIChatCompletions);

    Ok(ResolvedProviderSettings {
        provider_id,
        wire_api,
        model,
        base_url: matched_model
            .and_then(|entry| entry.base_url.clone())
            .or_else(|| provider_config.base_url.clone()),
        api_key: matched_model
            .and_then(|entry| entry.api_key.clone())
            .or_else(|| provider_config.api_key.clone()),
        model_auto_compact_token_limit: file.model_auto_compact_token_limit,
        model_context_window: file.model_context_window,
        model_thinking_selection: file.model_thinking_selection.clone(),
        disable_response_storage: file.disable_response_storage.unwrap_or(false),
        preferred_auth_method: file.preferred_auth_method,
    })
}

fn first_configured_provider_id(config: &ProviderConfigFile) -> Option<String> {
    config.model_providers.keys().next().cloned()
}

fn first_configured_model(config: &ProviderConfigFile) -> Option<String> {
    config.model_providers.values().find_map(|provider| {
        provider
            .last_model
            .clone()
            .or_else(|| provider.default_model.clone())
            .or_else(|| provider.models.first().map(|entry| entry.model.clone()))
    })
}

fn provider_id_for_model(
    config: &ProviderConfigFile,
    requested_model: Option<&str>,
) -> Option<String> {
    let requested_model = requested_model?;
    config
        .model_providers
        .iter()
        .find(|(_, provider)| {
            provider.last_model.as_deref() == Some(requested_model)
                || provider.default_model.as_deref() == Some(requested_model)
                || provider
                    .models
                    .iter()
                    .any(|entry| entry.model == requested_model)
        })
        .map(|(provider_id, _)| provider_id.clone())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ModelProviderConfig;
    use super::PreferredAuthMethod;
    use super::ProviderConfigFile;
    use super::ProviderWireApi;
    use super::ResolvedProviderSettings;
    use super::parse_config_str;
    use super::resolve_provider_settings_from_config;

    #[test]
    fn resolves_new_style_provider_and_model_settings() {
        let config = parse_config_str(
            r#"
model_provider = "xxxxx"
model = "gpt-5.4"
model_auto_compact_token_limit = 970000
model_context_window = 997500
model_thinking_selection = "medium"
disable_response_storage = true
preferred_auth_method = "apikey"

[model_providers.xxxxx]
name = "xxxxx"
base_url = "https://xxxxx/v1"
wire_api = "openai_responses"
"#,
        )
        .expect("parse config");

        let resolved =
            resolve_provider_settings_from_config(&config).expect("resolve provider settings");

        assert_eq!(
            resolved,
            ResolvedProviderSettings {
                provider_id: "xxxxx".to_string(),
                wire_api: ProviderWireApi::OpenAIResponses,
                model: "gpt-5.4".to_string(),
                base_url: Some("https://xxxxx/v1".to_string()),
                api_key: None,
                model_auto_compact_token_limit: Some(970000),
                model_context_window: Some(997500),
                model_thinking_selection: Some("medium".to_string()),
                disable_response_storage: true,
                preferred_auth_method: Some(PreferredAuthMethod::Apikey),
            }
        );
    }

    #[test]
    fn resolves_provider_from_model_when_provider_id_is_stale() {
        let config = ProviderConfigFile {
            model_provider: Some("missing".to_string()),
            model: Some("qwen3-coder-next".to_string()),
            model_thinking_selection: None,
            model_auto_compact_token_limit: None,
            model_context_window: None,
            disable_response_storage: None,
            preferred_auth_method: None,
            model_providers: [(
                "api.example.com".to_string(),
                ModelProviderConfig {
                    name: Some("api.example.com".to_string()),
                    base_url: Some("https://api.example.com".to_string()),
                    api_key: Some("profile-key".to_string()),
                    wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                    last_model: Some("qwen3-coder-next".to_string()),
                    default_model: None,
                    models: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        };

        let resolved =
            resolve_provider_settings_from_config(&config).expect("resolve provider settings");

        assert_eq!(resolved.provider_id, "api.example.com");
        assert_eq!(resolved.model, "qwen3-coder-next");
        assert_eq!(
            resolved.base_url,
            Some("https://api.example.com".to_string())
        );
        assert_eq!(resolved.api_key, Some("profile-key".to_string()));
    }

    #[test]
    fn provider_id_from_base_url_extracts_hostname() {
        assert_eq!(
            super::provider_id_from_base_url("https://open.bigmodel.cn/api/paas/v4"),
            Some("open.bigmodel.cn".to_string())
        );
        assert_eq!(
            super::provider_id_from_base_url("https://api.deepseek.com/v1"),
            Some("api.deepseek.com".to_string())
        );
    }
}
