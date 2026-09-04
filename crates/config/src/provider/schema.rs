use std::collections::BTreeMap;

use devo_protocol::InputModality;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningImplementation;
use devo_protocol::TruncationPolicyConfig;
use serde::Deserialize;
use serde::Serialize;

use crate::WebFetchConfig;
use crate::WebSearchConfig;

pub(crate) const AUTH_CONFIG_VERSION: u32 = 1;

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
        let value = value.trim();
        if value.eq_ignore_ascii_case("apikey") || value.eq_ignore_ascii_case("api_key") {
            Ok(Self::Apikey)
        } else {
            let normalized = value.to_ascii_lowercase();
            Err(serde::de::Error::custom(format!(
                "unsupported preferred_auth_method `{normalized}`"
            )))
        }
    }
}

/// Legacy model entry stored under old `[model_providers]` config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredModel {
    /// The model slug or custom model name.
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// Migration-only provider record read from the old `[providers.<id>]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyProviderConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Credential id in user-scoped `auth.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Transient API key compatibility value used only by in-memory callers.
    ///
    /// Canonical provider configuration never loads or persists this value;
    /// API keys are resolved from the credential id and user-scoped auth.json.
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Raw JSON object string containing provider-specific HTTP headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire_apis: Vec<ProviderWireApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for LegacyProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_url: None,
            credential: None,
            api_key: None,
            headers: None,
            wire_apis: Vec::new(),
            web_search: None,
            web_fetch: None,
            enabled: true,
        }
    }
}

impl LegacyProviderConfig {
    /// Returns whether the profile has no configured values.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.base_url.is_none()
            && self.credential.is_none()
            && self.api_key.is_none()
            && self.headers.is_none()
            && self.wire_apis.is_empty()
            && self.web_search.is_none()
            && self.web_fetch.is_none()
            && self.enabled
    }
}

/// Migration-only model record read from the old `[model_bindings.<id>]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyModelBindingConfig {
    pub model_slug: String,
    pub provider: String,
    #[serde(alias = "model_name")]
    pub request_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default = "default_provider_wire_api")]
    pub invocation_method: ProviderWireApi,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for LegacyModelBindingConfig {
    fn default() -> Self {
        Self {
            model_slug: String::new(),
            provider: String::new(),
            request_model: String::new(),
            display_name: None,
            invocation_method: default_provider_wire_api(),
            default_reasoning_effort: None,
            web_search: None,
            web_fetch: None,
            enabled: true,
        }
    }
}

/// Partial metadata overrides for one catalog model stored under `[model.<slug>]`.
///
/// Each field is optional so user, workspace, and command-line config layers can
/// override independent model attributes without replacing lower-priority values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelOverrideConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_context_window_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderWireApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_capability: Option<ReasoningCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_implementation: Option<ReasoningImplementation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<InputModality>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_policy: Option<TruncationPolicyConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_image_detail_original: Option<bool>,
}

/// Durable default selections stored under `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDefaultsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<String>,
}

/// Legacy provider profile stored under old `[model_providers.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyModelProviderConfig {
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

/// User-scoped credential file stored at `<user-config-dir>/auth.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAuthConfigFile {
    #[serde(default = "default_auth_config_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credentials: BTreeMap<String, AuthCredentialConfig>,
}

impl Default for UserAuthConfigFile {
    fn default() -> Self {
        Self {
            version: AUTH_CONFIG_VERSION,
            credentials: BTreeMap::new(),
        }
    }
}

/// One secret value in user-scoped auth storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCredentialConfig {
    pub kind: AuthCredentialKind,
    pub value: String,
}

/// Supported credential kinds in `auth.json`.
/// TODO: support oauth in the near future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthCredentialKind {
    ApiKey,
}

/// Provider-owned portion of app config, including active model selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderConfigSection {
    pub defaults: ProviderDefaultsConfig,
    pub model_provider: Option<String>,
    pub model: Option<String>,
    /// Logical reasoning effort selection for the active model, such as `disabled`,
    /// `enabled`, or one effort-like level supported by the selected model.
    ///
    /// This stores the user-facing selection, not a provider-specific request
    /// field. The runtime later resolves it into the final request model,
    /// provider `thinking` parameter, effective reasoning effort, and any
    /// provider-specific extra payload.
    pub model_reasoning_effort_selection: Option<String>,
    pub model_auto_compact_token_limit: Option<u32>,
    pub model_context_window: Option<u32>,
    pub disable_response_storage: Option<bool>,
    pub preferred_auth_method: Option<PreferredAuthMethod>,
    pub providers: BTreeMap<String, LegacyProviderConfig>,
    pub model_bindings: BTreeMap<String, LegacyModelBindingConfig>,
    pub model_overrides: BTreeMap<String, ModelOverrideConfig>,
    pub model_providers: BTreeMap<String, LegacyModelProviderConfig>,
}

#[derive(Default, Deserialize)]
struct ProviderConfigSectionWire {
    #[serde(default)]
    defaults: ProviderDefaultsConfig,
    model_provider: Option<String>,
    model: Option<ModelConfigField>,
    model_reasoning_effort_selection: Option<String>,
    model_thinking_selection: Option<String>,
    model_thinking: Option<String>,
    model_auto_compact_token_limit: Option<u32>,
    model_context_window: Option<u32>,
    disable_response_storage: Option<bool>,
    preferred_auth_method: Option<PreferredAuthMethod>,
    #[serde(default)]
    providers: BTreeMap<String, LegacyProviderConfig>,
    #[serde(default)]
    model_bindings: BTreeMap<String, LegacyModelBindingConfig>,
    #[serde(default)]
    model_providers: BTreeMap<String, LegacyModelProviderConfig>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelConfigField {
    Legacy(String),
    Overrides(BTreeMap<String, ModelOverrideConfig>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum SerializedModelConfigField<'a> {
    Legacy(&'a str),
    Overrides(&'a BTreeMap<String, ModelOverrideConfig>),
}

#[derive(Serialize)]
struct ProviderConfigSectionSerialize<'a> {
    #[serde(default, skip_serializing_if = "ProviderDefaultsConfig::is_empty")]
    defaults: &'a ProviderDefaultsConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_provider: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<SerializedModelConfigField<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_reasoning_effort_selection: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_auto_compact_token_limit: &'a Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_context_window: &'a Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_response_storage: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preferred_auth_method: &'a Option<PreferredAuthMethod>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    providers: &'a BTreeMap<String, LegacyProviderConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_bindings: &'a BTreeMap<String, LegacyModelBindingConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_providers: &'a BTreeMap<String, LegacyModelProviderConfig>,
}

impl Serialize for ProviderConfigSection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let model = if self.model_overrides.is_empty() {
            self.model
                .as_deref()
                .map(SerializedModelConfigField::Legacy)
        } else {
            Some(SerializedModelConfigField::Overrides(&self.model_overrides))
        };
        ProviderConfigSectionSerialize {
            defaults: &self.defaults,
            model_provider: &self.model_provider,
            model,
            model_reasoning_effort_selection: &self.model_reasoning_effort_selection,
            model_auto_compact_token_limit: &self.model_auto_compact_token_limit,
            model_context_window: &self.model_context_window,
            disable_response_storage: &self.disable_response_storage,
            preferred_auth_method: &self.preferred_auth_method,
            providers: &self.providers,
            model_bindings: &self.model_bindings,
            model_providers: &self.model_providers,
        }
        .serialize(serializer)
    }
}

impl From<ProviderConfigSectionWire> for ProviderConfigSection {
    fn from(wire: ProviderConfigSectionWire) -> Self {
        let (model, model_overrides) = match wire.model {
            Some(ModelConfigField::Legacy(model)) => (Some(model), BTreeMap::new()),
            Some(ModelConfigField::Overrides(model_overrides)) => (None, model_overrides),
            None => (None, BTreeMap::new()),
        };
        Self {
            defaults: wire.defaults,
            model_provider: wire.model_provider,
            model,
            model_reasoning_effort_selection: wire
                .model_reasoning_effort_selection
                .or(wire.model_thinking_selection)
                .or(wire.model_thinking),
            model_auto_compact_token_limit: wire.model_auto_compact_token_limit,
            model_context_window: wire.model_context_window,
            disable_response_storage: wire.disable_response_storage,
            preferred_auth_method: wire.preferred_auth_method,
            providers: wire.providers,
            model_bindings: wire.model_bindings,
            model_overrides,
            model_providers: wire.model_providers,
        }
    }
}

impl<'de> Deserialize<'de> for ProviderConfigSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(ProviderConfigSectionWire::deserialize(deserializer)?.into())
    }
}

impl ProviderConfigSection {
    pub(crate) fn merge_overlay(&mut self, overlay: Self, source: &toml::Value) {
        if overlay.model_provider.is_some() {
            self.model_provider = overlay.model_provider;
        }
        if overlay.model.is_some() {
            self.model = overlay.model;
        }
        if overlay.model_reasoning_effort_selection.is_some() {
            self.model_reasoning_effort_selection = overlay.model_reasoning_effort_selection;
        }
        if overlay.model_auto_compact_token_limit.is_some() {
            self.model_auto_compact_token_limit = overlay.model_auto_compact_token_limit;
        }
        if overlay.model_context_window.is_some() {
            self.model_context_window = overlay.model_context_window;
        }
        if overlay.disable_response_storage.is_some() {
            self.disable_response_storage = overlay.disable_response_storage;
        }
        if overlay.preferred_auth_method.is_some() {
            self.preferred_auth_method = overlay.preferred_auth_method;
        }
        if overlay.defaults.model_binding.is_some() {
            self.defaults.model_binding = overlay.defaults.model_binding;
        }
        for (provider_id, overlay_provider) in overlay.providers {
            let enabled_present =
                nested_table_has_key(source, "providers", &provider_id, "enabled");
            let provider = self.providers.entry(provider_id).or_default();
            if !overlay_provider.name.is_empty() {
                provider.name = overlay_provider.name;
            }
            if overlay_provider.base_url.is_some() {
                provider.base_url = overlay_provider.base_url;
            }
            if overlay_provider.credential.is_some() {
                provider.credential = overlay_provider.credential;
            }
            if overlay_provider.api_key.is_some() {
                provider.api_key = overlay_provider.api_key;
            }
            if overlay_provider.headers.is_some() {
                provider.headers = overlay_provider.headers;
            }
            if !overlay_provider.wire_apis.is_empty() {
                provider.wire_apis = overlay_provider.wire_apis;
            }
            if overlay_provider.web_search.is_some() {
                provider.web_search = overlay_provider.web_search;
            }
            if overlay_provider.web_fetch.is_some() {
                provider.web_fetch = overlay_provider.web_fetch;
            }
            if enabled_present {
                provider.enabled = overlay_provider.enabled;
            }
        }
        for (binding_id, overlay_binding) in overlay.model_bindings {
            let invocation_method_present =
                nested_table_has_key(source, "model_bindings", &binding_id, "invocation_method");
            let enabled_present =
                nested_table_has_key(source, "model_bindings", &binding_id, "enabled");
            let binding = self.model_bindings.entry(binding_id).or_default();
            if !overlay_binding.model_slug.is_empty() {
                binding.model_slug = overlay_binding.model_slug;
            }
            if !overlay_binding.provider.is_empty() {
                binding.provider = overlay_binding.provider;
            }
            if !overlay_binding.request_model.is_empty() {
                binding.request_model = overlay_binding.request_model;
            }
            if overlay_binding.display_name.is_some() {
                binding.display_name = overlay_binding.display_name;
            }
            if invocation_method_present {
                binding.invocation_method = overlay_binding.invocation_method;
            }
            if overlay_binding.default_reasoning_effort.is_some() {
                binding.default_reasoning_effort = overlay_binding.default_reasoning_effort;
            }
            if overlay_binding.web_search.is_some() {
                binding.web_search = overlay_binding.web_search;
            }
            if overlay_binding.web_fetch.is_some() {
                binding.web_fetch = overlay_binding.web_fetch;
            }
            if enabled_present {
                binding.enabled = overlay_binding.enabled;
            }
        }
        for (model_slug, overlay_override) in overlay.model_overrides {
            let model_override = self.model_overrides.entry(model_slug).or_default();
            if overlay_override.display_name.is_some() {
                model_override.display_name = overlay_override.display_name;
            }
            if overlay_override.description.is_some() {
                model_override.description = overlay_override.description;
            }
            if overlay_override.context_window.is_some() {
                model_override.context_window = overlay_override.context_window;
            }
            if overlay_override.effective_context_window_percent.is_some() {
                model_override.effective_context_window_percent =
                    overlay_override.effective_context_window_percent;
            }
            if overlay_override.max_tokens.is_some() {
                model_override.max_tokens = overlay_override.max_tokens;
            }
            if overlay_override.temperature.is_some() {
                model_override.temperature = overlay_override.temperature;
            }
            if overlay_override.top_p.is_some() {
                model_override.top_p = overlay_override.top_p;
            }
            if overlay_override.top_k.is_some() {
                model_override.top_k = overlay_override.top_k;
            }
            if overlay_override.provider.is_some() {
                model_override.provider = overlay_override.provider;
            }
            if overlay_override.reasoning_capability.is_some() {
                model_override.reasoning_capability = overlay_override.reasoning_capability;
            }
            if overlay_override.reasoning_implementation.is_some() {
                model_override.reasoning_implementation = overlay_override.reasoning_implementation;
            }
            if overlay_override.default_reasoning_effort.is_some() {
                model_override.default_reasoning_effort = overlay_override.default_reasoning_effort;
            }
            if overlay_override.base_instructions.is_some() {
                model_override.base_instructions = overlay_override.base_instructions;
            }
            if overlay_override.input_modalities.is_some() {
                model_override.input_modalities = overlay_override.input_modalities;
            }
            if overlay_override.channel.is_some() {
                model_override.channel = overlay_override.channel;
            }
            if overlay_override.truncation_policy.is_some() {
                model_override.truncation_policy = overlay_override.truncation_policy;
            }
            if overlay_override.supports_image_detail_original.is_some() {
                model_override.supports_image_detail_original =
                    overlay_override.supports_image_detail_original;
            }
        }
    }
}

fn nested_table_has_key(source: &toml::Value, section: &str, entry_id: &str, key: &str) -> bool {
    source
        .get(section)
        .and_then(toml::Value::as_table)
        .and_then(|entries| entries.get(entry_id))
        .and_then(toml::Value::as_table)
        .is_some_and(|entry| entry.contains_key(key))
}

impl ProviderDefaultsConfig {
    pub fn is_empty(&self) -> bool {
        self.model_binding.is_none()
    }
}

/// Provider HTTP settings shared by model-provider requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHttpConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

impl ProviderHttpConfig {
    pub fn is_empty(&self) -> bool {
        self.proxy_url.is_none() && self.no_proxy.is_none()
    }
}

fn default_true() -> bool {
    true
}

fn default_auth_config_version() -> u32 {
    AUTH_CONFIG_VERSION
}

fn default_provider_wire_api() -> ProviderWireApi {
    ProviderWireApi::OpenAIChatCompletions
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn preferred_auth_method_accepts_case_insensitive_values() {
        assert_eq!(
            serde_json::from_str::<PreferredAuthMethod>("\"API_KEY\"").expect("parse auth method"),
            PreferredAuthMethod::Apikey
        );
        assert_eq!(
            serde_json::from_str::<PreferredAuthMethod>("\" apiKEY \"").expect("parse auth method"),
            PreferredAuthMethod::Apikey
        );
    }

    #[test]
    fn preferred_auth_method_error_keeps_normalized_value() {
        let err =
            serde_json::from_str::<PreferredAuthMethod>("\"TOKEN\"").expect_err("reject token");

        assert_eq!(err.to_string(), "unsupported preferred_auth_method `token`");
    }

    #[test]
    fn provider_config_reads_legacy_reasoning_effort_selection_keys() {
        let config: ProviderConfigSection = toml::from_str(
            r#"
model_thinking_selection = "low"
model_thinking = "medium"
model_reasoning_effort_selection = "high"
"#,
        )
        .expect("parse provider config");

        assert_eq!(
            config.model_reasoning_effort_selection.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn provider_config_writes_reasoning_effort_selection_key() {
        let config = ProviderConfigSection {
            model_reasoning_effort_selection: Some("medium".to_string()),
            ..ProviderConfigSection::default()
        };

        let serialized = toml::to_string(&config).expect("serialize provider config");

        assert!(serialized.contains("model_reasoning_effort_selection"));
        assert!(!serialized.contains("model_thinking_selection"));
        assert!(!serialized.contains("model_thinking"));
    }

    #[test]
    fn provider_config_reads_legacy_model_selector() {
        let config: ProviderConfigSection =
            toml::from_str("model = \"legacy-model\"").expect("parse legacy model selector");

        assert_eq!(config.model.as_deref(), Some("legacy-model"));
        assert!(config.model_overrides.is_empty());
        assert_eq!(
            toml::to_string(&config).expect("serialize legacy model selector"),
            "model = \"legacy-model\"\n"
        );
    }

    #[test]
    fn provider_config_reads_model_override_table() {
        let config: ProviderConfigSection = toml::from_str(
            r#"
[model.grok-4]
display_name = "Grok 4"
description = "Fast reasoning model"
context_window = 256000
effective_context_window_percent = 90
max_tokens = 8192
temperature = 0.7
top_p = 0.95
top_k = 40.0
provider = "openai_responses"
reasoning_capability = "toggle"
reasoning_implementation = "request_parameter"
default_reasoning_effort = "high"
base_instructions = "Be concise."
input_modalities = ["text", "image"]
channel = "xAI"
truncation_policy = { mode = "tokens", limit = 12000 }
supports_image_detail_original = true
"#,
        )
        .expect("parse model overrides");

        assert_eq!(config.model, None);
        assert_eq!(
            config.model_overrides,
            BTreeMap::from([(
                "grok-4".to_string(),
                ModelOverrideConfig {
                    display_name: Some("Grok 4".to_string()),
                    description: Some("Fast reasoning model".to_string()),
                    context_window: Some(256_000),
                    effective_context_window_percent: Some(90.0),
                    max_tokens: Some(8_192),
                    temperature: Some(0.7),
                    top_p: Some(0.95),
                    top_k: Some(40.0),
                    provider: Some(ProviderWireApi::OpenAIResponses),
                    reasoning_capability: Some(ReasoningCapability::Toggle),
                    reasoning_implementation: Some(ReasoningImplementation::RequestParameter),
                    default_reasoning_effort: Some(ReasoningEffort::High),
                    base_instructions: Some("Be concise.".to_string()),
                    input_modalities: Some(vec![InputModality::Text, InputModality::Image]),
                    channel: Some("xAI".to_string()),
                    truncation_policy: Some(TruncationPolicyConfig::tokens(12_000)),
                    supports_image_detail_original: Some(true),
                },
            )])
        );

        let serialized = toml::to_string(&config).expect("serialize model overrides");
        assert!(serialized.contains("[model.grok-4]"));
        assert!(!serialized.contains("model = \""));
    }
}
