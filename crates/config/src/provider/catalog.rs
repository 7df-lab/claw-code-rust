//! The canonical JSON provider/model configuration and catalog shape.
//!
//! Provider identity and model identity are deliberately expressed by map keys:
//! a model is addressed as `provider/model`. This removes the old binding layer
//! from the user-facing format while retaining a TOML projection for backward
//! compatibility inside the runtime.

use std::collections::BTreeMap;

use devo_protocol::{
    InputModality, ProviderWireApi, ReasoningCapability, ReasoningEffort, ReasoningImplementation,
    TruncationPolicyConfig, find_effort_variant_key, normalize_reasoning_effort_literal,
};
use serde::{Deserialize, Serialize};

use super::schema::{
    LegacyModelBindingConfig, LegacyProviderConfig, ModelOverrideConfig, ProviderConfigSection,
    ProviderDefaultsConfig,
};
use crate::{WebFetchConfig, WebSearchConfig};

/// The canonical provider/model selection resolved from `providers.json`.
///
/// A selection contains the Connection id, the model map key, an optional
/// named variant, and the wire API used to invoke it. It deliberately has no
/// binding id: the map path is the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub variant_id: Option<String>,
    pub wire_api: ProviderWireApi,
}

/// Current version of the standalone provider/model JSON format.
pub const PROVIDER_CONFIG_FILE_VERSION: u32 = 1;

/// Standalone provider/model configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfigFile {
    /// Default primary model for normal turns in `provider/model` form.
    ///
    /// The built-in directory leaves this unset; user or workspace overlays
    /// may set it when they want an explicit default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional lower-cost model for lightweight background tasks, such as
    /// session-title generation. It falls back to the primary model when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
    /// User's global logical reasoning selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Providers keyed by their stable provider id.
    #[serde(
        default,
        rename = "provider",
        alias = "providers",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub providers: BTreeMap<String, ProviderConfigEntry>,
}

/// One provider connection plus its model directory entries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfigEntry {
    /// Optional display name. The provider id is the fallback name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Short subtitle shown in provider lists (for example region or product line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Provider endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Credential id resolved from the separate user-scoped `auth.json` file.
    ///
    /// The credential value itself is never stored in this catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Additional HTTP headers sent to this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Provider-level SDK/request options. The object is passed through
    /// unchanged so custom integrations can use provider-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    /// Provider-level request-body defaults merged into model requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
    /// Wire protocol used by models unless a model overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    /// Whether this provider is available for selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Environment variable names that may provide credentials in integrations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    /// Optional provider-hosted web search behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    /// Optional provider-hosted web fetch behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    /// Models keyed by the model id sent to the provider.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ProviderModelConfig>,
}

/// Optional catalog metadata and per-model request defaults.
///
/// `name` is intentionally the only human-facing identifier. The model id is
/// already supplied by the map key, so no `slug`, `model_name`, `model_id`, or
/// description field is needed in the persisted format.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    /// Optional display name shown in model pickers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional wire protocol override for this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_capability: Option<ReasoningCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_implementation: Option<ReasoningImplementation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// Exact UI selection used as the model default, including `on` or
    /// `off` for toggle-capable models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_selection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<InputModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_policy: Option<TruncationPolicyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_image_detail_original: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Optional model-specific web search behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    /// Optional model-specific web fetch behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    /// Higher values are preferred when selecting an implicit default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    /// Provider family, release metadata, and availability status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Open-ended model capability metadata, such as tools and modalities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<serde_json::Value>,
    /// Pricing and provider-defined metadata from a directory source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Arbitrary model options and request-body defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, ProviderModelVariantConfig>,
    /// Variant selected when a turn does not provide an explicit variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
}

/// A model variant in the standalone provider catalog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderModelVariantConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    /// Optional wire model id override when this variant is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl ProviderConfigFile {
    /// Resolves a `provider/model[/variant]` reference from this catalog.
    ///
    /// When `requested` is absent, the persisted `model` default is used; if
    /// that is also absent, the first enabled model in map order is selected.
    pub fn resolve_model(
        &self,
        requested: Option<&str>,
    ) -> Result<ProviderModelSelection, crate::ProviderConfigError> {
        let requested = requested.or(self.model.as_deref());
        if let Some(requested) = requested {
            return self.resolve_model_reference(requested);
        }

        self.providers
            .iter()
            .filter(|(_, provider)| provider.enabled != Some(false))
            .flat_map(|(provider_id, provider)| {
                provider
                    .models
                    .iter()
                    .filter(|(_, model)| model.enabled != Some(false))
                    .map(move |(model_id, model)| {
                        self.selection_for_model(provider_id, model_id, model, None)
                    })
            })
            .next()
            .ok_or_else(|| crate::ProviderConfigError::Validation {
                message: "no enabled provider model is configured".to_string(),
            })
    }

    fn resolve_model_reference(
        &self,
        requested: &str,
    ) -> Result<ProviderModelSelection, crate::ProviderConfigError> {
        let Some((provider_id, requested_model_id)) = requested.split_once('/') else {
            return Err(crate::ProviderConfigError::Validation {
                message: format!(
                    "model `{requested}` must use `provider/model` or `provider/model/variant` form"
                ),
            });
        };
        let Some(provider) = self.providers.get(provider_id) else {
            return Err(crate::ProviderConfigError::Validation {
                message: format!("provider Connection `{provider_id}` is not configured"),
            });
        };
        if provider.enabled == Some(false) {
            return Err(crate::ProviderConfigError::Validation {
                message: format!("provider Connection `{provider_id}` is disabled"),
            });
        }

        let (model_id, variant_id) = if provider.models.contains_key(requested_model_id) {
            (requested_model_id, None)
        } else if let Some((model_id, variant_id)) = requested_model_id.rsplit_once('/') {
            if provider
                .models
                .get(model_id)
                .is_some_and(|model| model.variants.contains_key(variant_id))
            {
                (model_id, Some(variant_id))
            } else {
                return Err(crate::ProviderConfigError::Validation {
                    message: format!("model `{requested}` is not configured"),
                });
            }
        } else {
            return Err(crate::ProviderConfigError::Validation {
                message: format!("model `{requested}` is not configured"),
            });
        };
        let model = provider
            .models
            .get(model_id)
            .expect("model was checked above");
        if model.enabled == Some(false) {
            return Err(crate::ProviderConfigError::Validation {
                message: format!("model `{requested}` is disabled"),
            });
        }
        if let Some(variant_id) = variant_id
            && provider
                .models
                .get(model_id)
                .and_then(|model| model.variants.get(variant_id))
                .is_some_and(|variant| variant.disabled)
        {
            return Err(crate::ProviderConfigError::Validation {
                message: format!("model variant `{requested}` is disabled"),
            });
        }

        Ok(self.selection_for_model(provider_id, model_id, model, variant_id))
    }

    fn selection_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
        model: &ProviderModelConfig,
        variant_id: Option<&str>,
    ) -> ProviderModelSelection {
        let provider = self
            .providers
            .get(provider_id)
            .expect("provider selection must belong to this catalog");
        ProviderModelSelection {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            variant_id: variant_id
                .map(ToOwned::to_owned)
                .or_else(|| model.default_variant.clone()),
            wire_api: model
                .wire_api
                .or(provider.wire_api)
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
        }
    }

    /// Merges a higher-priority provider file over this one.
    pub fn merge_overlay(&mut self, overlay: Self) {
        if overlay.model.is_some() {
            self.model = overlay.model;
        }
        if overlay.small_model.is_some() {
            self.small_model = overlay.small_model;
        }
        if overlay.reasoning_effort.is_some() {
            self.reasoning_effort = overlay.reasoning_effort;
        }
        for (provider_id, overlay_provider) in overlay.providers {
            let provider = self.providers.entry(provider_id).or_default();
            merge_provider_entry(provider, overlay_provider);
        }
    }

    /// Applies `[model.<slug>]` / `[model."<provider>/<model>"]` overlays onto
    /// matching provider catalog models.
    ///
    /// Overlay keys may be bare model ids or full `provider/model` references.
    /// Bare ids update every matching model id across providers.
    pub fn apply_model_overrides(&mut self, overrides: &BTreeMap<String, ModelOverrideConfig>) {
        for (model_reference, override_config) in overrides {
            let matches = self
                .providers
                .iter()
                .flat_map(|(provider_id, provider)| {
                    provider.models.keys().filter_map(move |model_id| {
                        let exact_reference = format!("{provider_id}/{model_id}");
                        (model_reference == model_id || model_reference == &exact_reference)
                            .then(|| (provider_id.clone(), model_id.clone()))
                    })
                })
                .collect::<Vec<_>>();
            for (provider_id, model_id) in matches {
                if let Some(model) = self
                    .providers
                    .get_mut(&provider_id)
                    .and_then(|provider| provider.models.get_mut(&model_id))
                {
                    model.apply_model_override(override_config);
                }
            }
        }
    }

    /// Projects the JSON shape into the legacy normalized runtime config.
    pub fn to_provider_config_section(&self) -> ProviderConfigSection {
        let default_model_binding = self
            .model
            .as_deref()
            .map(|model| self.base_model_reference(model));
        let mut section = ProviderConfigSection {
            model: self.model.clone(),
            model_reasoning_effort_selection: self.reasoning_effort.clone(),
            defaults: ProviderDefaultsConfig {
                model_binding: default_model_binding,
            },
            ..ProviderConfigSection::default()
        };

        for (provider_id, provider) in &self.providers {
            let wire_api = provider
                .wire_api
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
            section.providers.insert(
                provider_id.clone(),
                LegacyProviderConfig {
                    name: provider.name.clone().unwrap_or_else(|| provider_id.clone()),
                    base_url: provider.base_url.clone(),
                    credential: provider.credential.clone(),
                    api_key: None,
                    headers: provider
                        .headers
                        .as_ref()
                        .and_then(|headers| serde_json::to_string(headers).ok()),
                    wire_apis: vec![wire_api],
                    web_search: provider.web_search.clone(),
                    web_fetch: provider.web_fetch.clone(),
                    enabled: provider.enabled.unwrap_or(true),
                },
            );

            for (model_id, model) in &provider.models {
                let model_ref = model_reference(provider_id, model_id);
                let model_wire_api = model.wire_api.unwrap_or(wire_api);
                section.model_bindings.insert(
                    model_ref.clone(),
                    LegacyModelBindingConfig {
                        model_slug: model_ref.clone(),
                        provider: provider_id.clone(),
                        request_model: model_id.clone(),
                        display_name: model.name.clone(),
                        invocation_method: model_wire_api,
                        default_reasoning_effort: model
                            .default_reasoning_selection
                            .clone()
                            .or_else(|| {
                                model
                                    .default_reasoning_effort
                                    .map(|effort| effort.to_string())
                            }),
                        web_search: model.web_search.clone(),
                        web_fetch: model.web_fetch.clone(),
                        enabled: model.enabled.unwrap_or(provider.enabled.unwrap_or(true)),
                    },
                );
                section
                    .model_overrides
                    .insert(model_ref, model.to_model_override(model_wire_api));
            }
        }

        if let Some(model_ref) = self.model.as_deref()
            && let Some((provider_id, requested_model_id)) = model_ref.split_once('/')
            && let Some(provider) = self.providers.get(provider_id)
        {
            let model_id = provider
                .models
                .contains_key(requested_model_id)
                .then_some(requested_model_id)
                .or_else(|| {
                    requested_model_id
                        .rsplit_once('/')
                        .filter(|(model_id, variant_id)| {
                            provider
                                .models
                                .get(*model_id)
                                .is_some_and(|model| model.variants.contains_key(*variant_id))
                        })
                        .map(|(model_id, _)| model_id)
                })
                .unwrap_or(requested_model_id);
            let model_ref = model_reference(provider_id, model_id);
            let binding = section
                .model_bindings
                .entry(model_ref.to_string())
                .or_insert_with(|| LegacyModelBindingConfig {
                    model_slug: model_ref.to_string(),
                    provider: provider_id.to_string(),
                    request_model: model_id.to_string(),
                    invocation_method: provider
                        .wire_api
                        .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
                    ..LegacyModelBindingConfig::default()
                });
            section
                .model_overrides
                .entry(model_ref.to_string())
                .or_default();
            if section.defaults.model_binding.is_none() {
                section.defaults.model_binding = Some(binding.model_slug.clone());
            }
        }

        section
    }

    fn base_model_reference(&self, model_ref: &str) -> String {
        let Some((provider_id, requested_model_id)) = model_ref.split_once('/') else {
            return model_ref.to_string();
        };
        let Some(provider) = self.providers.get(provider_id) else {
            return model_ref.to_string();
        };
        if provider.models.contains_key(requested_model_id) {
            return model_ref.to_string();
        }
        let Some((model_id, variant_id)) = requested_model_id.rsplit_once('/') else {
            return model_ref.to_string();
        };
        if provider
            .models
            .get(model_id)
            .is_some_and(|model| model.variants.contains_key(variant_id))
        {
            return model_reference(provider_id, model_id);
        }
        model_ref.to_string()
    }

    /// Converts the old normalized shape into the new canonical JSON shape.
    /// This is used only as an in-memory compatibility fallback for callers
    /// that construct `AppConfig` directly in tests or integrations.
    pub fn from_provider_config_section(section: &ProviderConfigSection) -> Self {
        let mut file = Self {
            model: selected_model_reference(section),
            reasoning_effort: section.model_reasoning_effort_selection.clone(),
            ..Self::default()
        };
        for (provider_id, provider) in &section.providers {
            let entry = file
                .providers
                .entry(provider_id.clone())
                .or_insert_with(|| ProviderConfigEntry {
                    name: Some(provider.name.clone()),
                    base_url: provider.base_url.clone(),
                    credential: provider.credential.clone(),
                    wire_api: provider.wire_apis.first().copied(),
                    enabled: Some(provider.enabled),
                    web_search: provider.web_search.clone(),
                    web_fetch: provider.web_fetch.clone(),
                    ..ProviderConfigEntry::default()
                });
            entry.name = Some(provider.name.clone());
            entry.base_url = provider.base_url.clone();
            entry.credential = provider.credential.clone();
            entry.wire_api = provider.wire_apis.first().copied();
            entry.enabled = Some(provider.enabled);
            entry.web_search = provider.web_search.clone();
            entry.web_fetch = provider.web_fetch.clone();
        }
        for binding in section.model_bindings.values() {
            let provider_id = if binding.provider.is_empty() {
                "default"
            } else {
                binding.provider.as_str()
            };
            let model_id = model_id_from_reference(provider_id, &binding.request_model);
            let entry = file.providers.entry(provider_id.to_string()).or_default();
            let model = entry.models.entry(model_id).or_default();
            model.name = binding.display_name.clone();
            model.wire_api = Some(binding.invocation_method);
            model.web_search = binding.web_search.clone();
            model.web_fetch = binding.web_fetch.clone();
            model.enabled = Some(binding.enabled);
            model.default_reasoning_effort =
                binding
                    .default_reasoning_effort
                    .as_deref()
                    .and_then(|value| {
                        serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
                    });
            model.default_reasoning_selection = binding.default_reasoning_effort.clone();
            if let Some(override_config) = section.model_overrides.get(&binding.model_slug) {
                model.apply_model_override(override_config);
            }
        }
        file
    }
}

fn selected_model_reference(section: &ProviderConfigSection) -> Option<String> {
    let selected = section
        .model
        .as_deref()
        .filter(|selected| section.model_bindings.contains_key(*selected))
        .or(section.defaults.model_binding.as_deref())?;
    let binding = section.model_bindings.get(selected)?;
    let model_id = if binding.request_model.trim().is_empty() {
        binding.model_slug.trim()
    } else {
        binding.request_model.trim()
    };
    Some(model_reference(&binding.provider, model_id))
}

impl ProviderModelConfig {
    /// Projects legacy `reasoning_implementation: model_variant` entries into
    /// catalog `variants` when the variants map is empty.
    ///
    /// Selection values are normalized to canonical `off`/`on` (from
    /// `disabled`/`enabled`). The legacy implementation field is cleared after
    /// projection so Adapter vs CatalogVariant mode is derived from variants.
    pub fn migrate_reasoning_implementation_into_variants(&mut self) {
        if !self.variants.is_empty() {
            return;
        }
        let Some(ReasoningImplementation::ModelVariant(config)) =
            self.reasoning_implementation.clone()
        else {
            return;
        };
        for variant in config.variants {
            let key = normalize_reasoning_effort_literal(&variant.selection_value);
            self.variants.insert(
                key,
                ProviderModelVariantConfig {
                    label: Some(variant.label).filter(|label| !label.is_empty()),
                    disabled: false,
                    request_model: Some(variant.model).filter(|model| !model.is_empty()),
                    request: variant.extra_body,
                    options: None,
                    headers: BTreeMap::new(),
                },
            );
        }
        self.reasoning_implementation = None;
    }

    /// Resolves the catalog variant id used for a turn given an optional
    /// explicit model-reference variant and a logical reasoning selection.
    ///
    /// Explicit variants that are not effort option values stay as static
    /// overlays. Otherwise the matching effort-keyed variant wins, then
    /// `default_variant`.
    pub fn resolve_turn_variant_id(
        &self,
        explicit_variant: Option<&str>,
        reasoning_selection: Option<&str>,
    ) -> Option<String> {
        let capability_option_values: Vec<String> = self
            .reasoning_capability
            .as_ref()
            .map(|capability| {
                capability
                    .options()
                    .into_iter()
                    .map(|option| option.value)
                    .collect()
            })
            .unwrap_or_default();

        if let Some(explicit) = explicit_variant
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let normalized_explicit = normalize_reasoning_effort_literal(explicit);
            let is_effort_option = capability_option_values
                .iter()
                .any(|value| value == &normalized_explicit);
            if !is_effort_option {
                return Some(explicit.to_string());
            }
        }

        if let Some(selection) = reasoning_selection
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(normalize_reasoning_effort_literal)
            && let Some(key) = find_effort_variant_key(&self.variants, &selection)
        {
            let variant = self.variants.get(key)?;
            if !variant.disabled {
                return Some(key.to_string());
            }
        }

        self.default_variant
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .filter(|id| {
                self.variants
                    .get(id)
                    .is_some_and(|variant| !variant.disabled)
            })
    }

    /// Applies the fields present in a higher-priority model configuration.
    ///
    /// This is also used when the onboarding protocol carries a sparse JSON
    /// model-settings object. Absent fields preserve the existing catalog
    /// value, matching the normal provider catalog merge semantics.
    pub fn apply_overlay(&mut self, overlay: Self) {
        macro_rules! replace_some {
            ($field:ident) => {
                if overlay.$field.is_some() {
                    self.$field = overlay.$field;
                }
            };
        }
        replace_some!(name);
        replace_some!(wire_api);
        replace_some!(context_window);
        replace_some!(effective_context_window_percent);
        replace_some!(max_tokens);
        replace_some!(temperature);
        replace_some!(top_p);
        replace_some!(top_k);
        replace_some!(reasoning_capability);
        replace_some!(reasoning_implementation);
        replace_some!(default_reasoning_effort);
        replace_some!(default_reasoning_selection);
        replace_some!(base_instructions);
        replace_some!(input_modalities);
        replace_some!(channel);
        replace_some!(truncation_policy);
        replace_some!(supports_image_detail_original);
        replace_some!(enabled);
        replace_some!(web_search);
        replace_some!(web_fetch);
        replace_some!(priority);
        replace_some!(family);
        replace_some!(release_date);
        replace_some!(status);
        replace_some!(capabilities);
        merge_optional_json(&mut self.cost, overlay.cost);
        merge_optional_json(&mut self.metadata, overlay.metadata);
        merge_optional_json(&mut self.request, overlay.request);
        merge_optional_json(&mut self.options, overlay.options);
        self.headers.extend(overlay.headers);
        for (variant_id, overlay_variant) in overlay.variants {
            let variant = self.variants.entry(variant_id).or_default();
            if overlay_variant.label.is_some() {
                variant.label = overlay_variant.label;
            }
            if overlay_variant.disabled {
                variant.disabled = true;
            }
            if overlay_variant.request_model.is_some() {
                variant.request_model = overlay_variant.request_model;
            }
            merge_optional_json(&mut variant.request, overlay_variant.request);
            merge_optional_json(&mut variant.options, overlay_variant.options);
            variant.headers.extend(overlay_variant.headers);
        }
        replace_some!(default_variant);
    }

    fn to_model_override(&self, provider_wire_api: ProviderWireApi) -> ModelOverrideConfig {
        ModelOverrideConfig {
            display_name: self.name.clone(),
            context_window: self.context_window,
            effective_context_window_percent: self.effective_context_window_percent,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            provider: Some(self.wire_api.unwrap_or(provider_wire_api)),
            reasoning_capability: self.reasoning_capability.clone(),
            reasoning_implementation: self.reasoning_implementation.clone(),
            default_reasoning_effort: self.default_reasoning_effort,
            base_instructions: self.base_instructions.clone(),
            input_modalities: self.input_modalities.clone(),
            channel: self.channel.clone(),
            truncation_policy: self.truncation_policy,
            supports_image_detail_original: self.supports_image_detail_original,
            ..ModelOverrideConfig::default()
        }
    }

    fn apply_model_override(&mut self, override_config: &ModelOverrideConfig) {
        if override_config.display_name.is_some() {
            self.name = override_config.display_name.clone();
        }
        if override_config.context_window.is_some() {
            self.context_window = override_config.context_window;
        }
        if override_config.effective_context_window_percent.is_some() {
            self.effective_context_window_percent =
                override_config.effective_context_window_percent;
        }
        if override_config.max_tokens.is_some() {
            self.max_tokens = override_config.max_tokens;
        }
        if override_config.temperature.is_some() {
            self.temperature = override_config.temperature;
        }
        if override_config.top_p.is_some() {
            self.top_p = override_config.top_p;
        }
        if override_config.top_k.is_some() {
            self.top_k = override_config.top_k;
        }
        if override_config.provider.is_some() {
            self.wire_api = override_config.provider;
        }
        if override_config.reasoning_capability.is_some() {
            self.reasoning_capability = override_config.reasoning_capability.clone();
        }
        if override_config.reasoning_implementation.is_some() {
            self.reasoning_implementation = override_config.reasoning_implementation.clone();
        }
        if override_config.default_reasoning_effort.is_some() {
            self.default_reasoning_effort = override_config.default_reasoning_effort;
        }
        if override_config.base_instructions.is_some() {
            self.base_instructions = override_config.base_instructions.clone();
        }
        if override_config.input_modalities.is_some() {
            self.input_modalities = override_config.input_modalities.clone();
        }
        if override_config.channel.is_some() {
            self.channel = override_config.channel.clone();
        }
        if override_config.truncation_policy.is_some() {
            self.truncation_policy = override_config.truncation_policy;
        }
        if override_config.supports_image_detail_original.is_some() {
            self.supports_image_detail_original = override_config.supports_image_detail_original;
        }
    }
}

fn merge_provider_entry(base: &mut ProviderConfigEntry, overlay: ProviderConfigEntry) {
    if overlay.name.is_some() {
        base.name = overlay.name;
    }
    if overlay.description.is_some() {
        base.description = overlay.description;
    }
    if overlay.base_url.is_some() {
        base.base_url = overlay.base_url;
    }
    if overlay.credential.is_some() {
        base.credential = overlay.credential;
    }
    if let Some(headers) = overlay.headers {
        base.headers
            .get_or_insert_with(BTreeMap::new)
            .extend(headers);
    }
    merge_optional_json(&mut base.options, overlay.options);
    merge_optional_json(&mut base.request, overlay.request);
    if overlay.wire_api.is_some() {
        base.wire_api = overlay.wire_api;
    }
    if overlay.enabled.is_some() {
        base.enabled = overlay.enabled;
    }
    if !overlay.env.is_empty() {
        base.env = overlay.env;
    }
    if overlay.web_search.is_some() {
        base.web_search = overlay.web_search;
    }
    if overlay.web_fetch.is_some() {
        base.web_fetch = overlay.web_fetch;
    }
    for (model_id, overlay_model) in overlay.models {
        merge_model_entry(base.models.entry(model_id).or_default(), overlay_model);
    }
}

fn merge_model_entry(base: &mut ProviderModelConfig, overlay: ProviderModelConfig) {
    base.apply_overlay(overlay);
}

fn merge_optional_json(base: &mut Option<serde_json::Value>, overlay: Option<serde_json::Value>) {
    let Some(overlay) = overlay else {
        return;
    };
    match overlay {
        serde_json::Value::Object(overlay) => {
            if let Some(serde_json::Value::Object(base)) = base.as_mut() {
                for (key, value) in overlay {
                    let entry = base.entry(key).or_insert(serde_json::Value::Null);
                    merge_json_value(entry, value);
                }
            } else {
                *base = Some(serde_json::Value::Object(overlay));
            }
        }
        overlay => *base = Some(overlay),
    }
}

fn merge_json_value(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match overlay {
        serde_json::Value::Object(overlay) => {
            if let serde_json::Value::Object(base) = base {
                for (key, value) in overlay {
                    let entry = base.entry(key).or_insert(serde_json::Value::Null);
                    merge_json_value(entry, value);
                }
            } else {
                *base = serde_json::Value::Object(overlay);
            }
        }
        overlay => {
            *base = overlay;
        }
    }
}

/// Forms the only stable model reference accepted by the new configuration.
pub fn model_reference(provider_id: &str, model_id: &str) -> String {
    format!("{provider_id}/{model_id}")
}

fn model_id_from_reference(provider_id: &str, model: &str) -> String {
    model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model)
        .to_string()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{
        ProviderConfigFile, ProviderModelConfig, ProviderModelVariantConfig, ReasoningCapability,
        ReasoningEffort, ReasoningImplementation,
    };

    #[test]
    fn canonical_json_uses_nested_model_keys_without_legacy_identifiers() {
        let file: ProviderConfigFile = serde_json::from_str(
            r#"
{
  "model": "local/qwen3",
  "provider": {
    "local": {
      "models": {
        "qwen3": {"name": "Qwen 3"}
      }
    }
  }
}
"#,
        )
        .expect("parse provider config");

        assert_eq!(
            file.providers["local"].models["qwen3"],
            ProviderModelConfig {
                name: Some("Qwen 3".to_string()),
                ..ProviderModelConfig::default()
            }
        );
        let rendered = serde_json::to_string(&file).expect("serialize provider config");
        assert!(!rendered.contains("model_slug"));
        assert!(!rendered.contains("model_name"));
        assert!(!rendered.contains("description"));
    }

    #[test]
    fn json_provider_overlay_projects_a_top_level_model_without_nested_metadata() {
        let file: ProviderConfigFile = serde_json::from_str(
            r#"
{
  "model": "openai/gpt-5.5",
  "provider": {
    "openai": {
      "base_url": "https://example.com/v1",
      "wire_api": "openai_responses"
    }
  }
}
"#,
        )
        .expect("parse provider config");
        let section = file.to_provider_config_section();
        let binding = &section.model_bindings["openai/gpt-5.5"];

        assert_eq!(binding.provider, "openai");
        assert_eq!(binding.request_model, "gpt-5.5");
        assert_eq!(binding.invocation_method.to_string(), "openai_responses");
    }

    #[test]
    fn json_disabled_entries_remain_disabled_in_the_compatibility_projection() {
        let file: ProviderConfigFile = serde_json::from_str(
            r#"
{
  "model": "local/qwen3",
  "provider": {
    "local": {
      "enabled": false,
      "models": {
        "qwen3": {"enabled": false}
      }
    }
  }
}
"#,
        )
        .expect("parse provider config");
        let section = file.to_provider_config_section();

        assert!(!section.providers["local"].enabled);
        assert!(!section.model_bindings["local/qwen3"].enabled);
    }

    #[test]
    fn model_overlay_replaces_present_fields_and_preserves_omitted_fields() {
        let mut model = ProviderModelConfig {
            name: Some("Catalog model".to_string()),
            context_window: Some(128_000),
            temperature: Some(0.2),
            ..ProviderModelConfig::default()
        };
        model.apply_overlay(ProviderModelConfig {
            context_window: Some(256_000),
            priority: Some(10),
            ..ProviderModelConfig::default()
        });

        assert_eq!(
            model,
            ProviderModelConfig {
                name: Some("Catalog model".to_string()),
                context_window: Some(256_000),
                temperature: Some(0.2),
                priority: Some(10),
                ..ProviderModelConfig::default()
            }
        );
    }

    #[test]
    fn canonical_catalog_preserves_open_ended_model_settings_and_variants() {
        let file: ProviderConfigFile = serde_json::from_str(
            r#"
{
  "provider": {
    "custom": {
      "options": {"timeout": 30, "enterprise": true},
      "request": {"extra_body": {"provider_flag": true}},
      "headers": {"X-Provider": "one"},
      "models": {
        "reasoning-model": {
          "family": "custom-family",
          "release_date": "2026-01-01",
          "cost": {"input": 1.2, "output": 4.8},
          "options": {"thinking": {"budget": 4096}},
          "request": {"reasoning_effort": "high"},
          "headers": {"X-Model": "two"},
          "variants": {
            "fast": {
              "label": "Fast",
              "options": {"thinking": {"budget": 1024}},
              "request": {"speed": "fast"},
              "headers": {"X-Variant": "three"}
            }
          }
        }
      }
    }
  }
}
"#,
        )
        .expect("parse rich provider catalog");
        let model = &file.providers["custom"].models["reasoning-model"];
        assert_eq!(model.family.as_deref(), Some("custom-family"));
        assert_eq!(model.release_date.as_deref(), Some("2026-01-01"));
        assert_eq!(model.variants["fast"].label.as_deref(), Some("Fast"));
        assert_eq!(model.headers["X-Model"], "two");
        assert_eq!(
            model.options,
            Some(serde_json::json!({"thinking": {"budget": 4096}}))
        );
        assert!(
            !serde_json::to_string(&file)
                .expect("serialize rich provider catalog")
                .contains("model_slug")
        );
    }

    #[test]
    fn migrate_reasoning_implementation_projects_model_variants() {
        use devo_protocol::{ReasoningVariant, ReasoningVariantConfig};

        let mut model = ProviderModelConfig {
            reasoning_capability: Some(ReasoningCapability::Toggle),
            reasoning_implementation: Some(ReasoningImplementation::ModelVariant(
                ReasoningVariantConfig {
                    variants: vec![
                        ReasoningVariant {
                            selection_value: "disabled".to_string(),
                            model: "chat".to_string(),
                            reasoning_effort: None,
                            label: "Off".to_string(),
                            description: "Off".to_string(),
                            extra_body: Some(serde_json::json!({"mode": "chat"})),
                        },
                        ReasoningVariant {
                            selection_value: "enabled".to_string(),
                            model: "reasoner".to_string(),
                            reasoning_effort: None,
                            label: "On".to_string(),
                            description: "On".to_string(),
                            extra_body: Some(serde_json::json!({"mode": "think"})),
                        },
                    ],
                },
            )),
            ..ProviderModelConfig::default()
        };
        model.migrate_reasoning_implementation_into_variants();

        assert!(model.reasoning_implementation.is_none());
        assert_eq!(model.variants["off"].request_model.as_deref(), Some("chat"));
        assert_eq!(
            model.variants["on"].request_model.as_deref(),
            Some("reasoner")
        );
        assert_eq!(
            model
                .resolve_turn_variant_id(None, Some("enabled"))
                .as_deref(),
            Some("on")
        );
    }

    #[test]
    fn resolve_turn_variant_id_keeps_non_effort_explicit_variant() {
        let model = ProviderModelConfig {
            reasoning_capability: Some(ReasoningCapability::Levels(vec![
                ReasoningEffort::Low.into(),
                ReasoningEffort::High.into(),
            ])),
            variants: [
                (
                    "low".to_string(),
                    ProviderModelVariantConfig {
                        request: Some(serde_json::json!({"effort": "L"})),
                        ..ProviderModelVariantConfig::default()
                    },
                ),
                (
                    "fast".to_string(),
                    ProviderModelVariantConfig {
                        request: Some(serde_json::json!({"speed": "fast"})),
                        ..ProviderModelVariantConfig::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..ProviderModelConfig::default()
        };

        assert_eq!(
            model
                .resolve_turn_variant_id(Some("fast"), Some("low"))
                .as_deref(),
            Some("fast")
        );
        assert_eq!(
            model.resolve_turn_variant_id(None, Some("low")).as_deref(),
            Some("low")
        );
    }
}
