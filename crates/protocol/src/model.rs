//! Runtime model types shared across core, server, and clients.
//!
//! Main focus:
//! - represent the resolved model shape used during execution
//! - expose model capabilities needed by UI, request building, and turn resolution
//! - provide the read-only catalog trait over runtime `Model` values
//!
//! Design:
//! - `Model` is the cross-crate runtime type, not the raw config/catalog input type
//! - this module keeps behavior that belongs to the executable model itself, such as
//!   reasoning effort resolution and effective defaults
//! - callers should be able to use this type without knowing how the model catalog was loaded
//!
//! Boundary:
//! - this module must not own bundled JSON loading or compatibility parsing for catalog files
//! - raw preset/config concerns live in `devo-core`
//! - this module describes runtime state and runtime-facing interfaces only
//!
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use ts_rs::TS;

use crate::HostedToolDefinition;
use crate::ProviderInfo;
use crate::ProviderModelInfo;
use crate::ReasoningCapability;
use crate::ReasoningEffort;
use crate::ReasoningEffortPreset;
use crate::ReasoningImplementation;
use crate::ResolvedReasoningRequest;
use crate::adapter_request_thinking_wire;
use crate::find_effort_variant_key;
use crate::nearest_effort;
use crate::normalize_reasoning_effort_literal;
use crate::truncation::TruncationPolicyConfig;

/// Catalog variant metadata used when a logical effort selection maps onto a
/// named `variants` entry (request-body / request-model encoding).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEffortVariant {
    /// Optional wire model id override for this effort selection.
    pub request_model: Option<String>,
    /// Whether this variant may be selected.
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Verbosity {
    Low,
    #[default]
    Medium,
    High,
}

/// Sampling controls and model-selection hints shared across adapters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SamplingControls {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

/// A message in the request to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMessage {
    pub role: String,
    pub content: Vec<RequestContent>,
}

/// Full request to the model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Identity used for provider capability resolution.
    ///
    /// The catalog slug is metadata and is not sent as the provider's wire
    /// model name. Requests that do not originate from the catalog must opt
    /// into the generic profile explicitly.
    #[serde(skip)]
    pub model_slug: ModelProfileKey,
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<RequestMessage>,
    pub max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosted_tools: Vec<HostedToolDefinition>,
    #[serde(default)]
    pub sampling: SamplingControls,
    #[serde(rename = "thinking", skip_serializing_if = "Option::is_none")]
    pub request_thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<Value>,
}

/// Identifies the model metadata a provider adapter should use when shaping a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ModelProfileKey {
    /// Resolve provider capabilities using this Devo catalog slug.
    CatalogSlug(String),
    /// Use the provider's generic capability profile.
    #[default]
    Generic,
}

/// A tool definition sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

/// A content block within a message sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RequestContent {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "reasoning")]
    Reasoning { text: String },

    #[serde(rename = "provider_reasoning")]
    ProviderReasoning {
        provider: String,
        payload: serde_json::Value,
    },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "hosted_tool_use")]
    HostedToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
/// Supported input types that a model can accept.
#[derive(Default)]
pub enum InputModality {
    /// Plain text input.
    #[default]
    Text,
    /// Image input.
    Image,
}

/// OpenAI-family API surfaces supported by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OpenAiApi {
    #[default]
    ChatCompletions,
    Responses,
}

/// Anthropic-family API surfaces supported by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicApi {
    Messages,
}

/// Provider identity plus its selected wire API.
impl Default for AnthropicApi {
    fn default() -> Self {
        AnthropicApi::Messages
    }
}

/// One supported provider wire protocol exposed by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub enum ProviderWireApi {
    /// OpenAI-compatible `/v1/chat/completions`.
    #[serde(rename = "openai_chat_completions")]
    OpenAIChatCompletions,
    /// OpenAI-compatible `/v1/responses`.
    #[serde(rename = "openai_responses")]
    OpenAIResponses,
    /// Anthropic-compatible `/v1/messages`.
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

impl ProviderWireApi {
    /// Returns the canonical config and environment string for this wire API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAIChatCompletions => "openai_chat_completions",
            Self::OpenAIResponses => "openai_responses",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }
}

impl fmt::Display for ProviderWireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ProviderWireApi> for &'static str {
    fn from(value: ProviderWireApi) -> Self {
        value.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// Resolved runtime model metadata used across core, server, and clients.
pub struct Model {
    /// Stable model identifier used in config and requests. such as `claude-sonnet-20250425`
    pub slug: String,
    /// Human-readable display name shown in the UI. such as `claude-sonnet-4.6`
    pub display_name: String,
    /// Provider selection that serves this model.
    pub provider: ProviderWireApi,
    /// Optional short description of the model.
    pub description: Option<String>,
    /// Reasoning control available for this model.
    #[serde(alias = "thinking_capability")]
    pub reasoning_capability: ReasoningCapability,
    /// Default reasoning effort selected for the model when no levels are exposed.
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// Exact default reasoning selection, including toggle values such as
    /// `on` and `off`.
    pub default_reasoning_selection: Option<String>,
    /// How the selected reasoning effort should be applied to requests.
    #[serde(alias = "thinking_implementation")]
    pub reasoning_implementation: Option<ReasoningImplementation>,
    /// Catalog `variants` keyed by logical effort selection (`off`/`on`/`low`…).
    ///
    /// When the normalized selection matches a non-disabled entry, resolution
    /// uses CatalogVariant mode (no first-class thinking/effort fields).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub catalog_variants: BTreeMap<String, ModelEffortVariant>,
    /// Base system instructions bundled with the model.
    pub base_instructions: String,
    /// Maximum context window in tokens.
    pub context_window: u32,
    /// Percentage of the context window treated as effectively usable.
    ///
    /// May be fractional (for example `25.5`). Defaults to `95` when unset.
    pub effective_context_window_percent: Option<f64>,
    /// Policy used when truncating content for requests.
    pub truncation_policy: TruncationPolicyConfig,
    /// Input types accepted by the model.
    pub input_modalities: Vec<InputModality>,
    /// Whether the model supports original-resolution image detail.
    pub supports_image_detail_original: bool,
    /// Grouping label used to organize models by vendor or family.
    pub channel: Option<String>,
    /// Default temperature to use when the model does not override it.
    pub temperature: Option<f64>,
    /// Default nucleus sampling value to use when the model does not override it.
    pub top_p: Option<f64>,
    /// Default top-k sampling value to use when the model does not override it.
    pub top_k: Option<f64>,
    /// Default maximum token limit for responses from this model.
    pub max_tokens: Option<u32>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            slug: String::new(),
            display_name: String::new(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            description: None,
            reasoning_capability: ReasoningCapability::Unsupported,
            default_reasoning_effort: Some(ReasoningEffort::default()),
            default_reasoning_selection: None,
            reasoning_implementation: None,
            catalog_variants: BTreeMap::new(),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig::default(),
            input_modalities: vec![InputModality::default()],
            supports_image_detail_original: false,
            channel: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
        }
    }
}

impl Model {
    pub fn provider_wire_api(&self) -> ProviderWireApi {
        self.provider
    }

    /// Whether this model should receive the `apply_patch` tool in requests.
    ///
    /// Only OpenAI-channel models get `apply_patch`. Other models tend to misuse
    /// the patch dialect and produce frequent format errors; they should use
    /// `edit` / `write` instead.
    pub fn supports_apply_patch(&self) -> bool {
        self.channel.as_deref() == Some("OpenAI")
    }

    pub fn reasoning_effort_options(&self) -> Vec<ReasoningEffortPreset> {
        match &self.reasoning_capability {
            ReasoningCapability::Levels(_) => self
                .reasoning_capability
                .effort_levels()
                .into_iter()
                .map(|effort| ReasoningEffortPreset::new(effort, effort.description()))
                .collect(),
            _ => self
                .default_reasoning_effort
                .iter()
                .copied()
                .map(|effort| ReasoningEffortPreset::new(effort, effort.description()))
                .collect(),
        }
    }

    pub fn effective_reasoning_capability(&self) -> ReasoningCapability {
        self.reasoning_capability.clone()
    }

    pub fn effective_reasoning_implementation(&self) -> ReasoningImplementation {
        self.reasoning_implementation.clone().unwrap_or({
            if matches!(self.reasoning_capability, ReasoningCapability::Unsupported) {
                ReasoningImplementation::Disabled
            } else {
                ReasoningImplementation::RequestParameter
            }
        })
    }

    pub fn effective_context_window_percent(&self) -> f64 {
        self.effective_context_window_percent.unwrap_or(95.0)
    }

    pub fn effective_context_window(&self) -> u32 {
        let percent = self.effective_context_window_percent().clamp(0.0, 100.0);
        ((f64::from(self.context_window) * percent) / 100.0).floor() as u32
    }

    pub fn default_reasoning_effort_selection(&self) -> Option<String> {
        if let Some(selection) = self
            .default_reasoning_selection
            .as_deref()
            .map(str::trim)
            .filter(|selection| !selection.is_empty())
        {
            return Some(normalize_reasoning_effort_literal(selection));
        }
        match &self.reasoning_capability {
            ReasoningCapability::Unsupported => None,
            ReasoningCapability::Toggle => Some(String::from("on")),
            ReasoningCapability::Levels(choices) => self
                .default_reasoning_effort
                .or_else(|| {
                    choices
                        .iter()
                        .copied()
                        .find_map(crate::ReasoningLevelChoice::effort)
                })
                .map(|effort| effort.label().to_lowercase())
                .or_else(|| {
                    choices
                        .first()
                        .copied()
                        .map(|choice| choice.selection_value().to_string())
                }),
        }
    }

    pub fn normalize_reasoning_effort_selection(&self, selection: Option<&str>) -> Option<String> {
        selection
            .map(str::trim)
            .filter(|selection| !selection.is_empty())
            .filter(|selection| !selection.eq_ignore_ascii_case("default"))
            .map(normalize_reasoning_effort_literal)
            .or_else(|| self.default_reasoning_effort_selection())
    }

    pub fn nearest_supported_reasoning_effort(&self, target: ReasoningEffort) -> ReasoningEffort {
        let levels = self.reasoning_capability.effort_levels();
        if levels.is_empty() {
            self.default_reasoning_effort.unwrap_or(target)
        } else {
            nearest_effort(target, &levels)
        }
    }

    /// Returns the catalog variant key that encodes the given logical effort
    /// selection, when one exists and is selectable.
    pub fn effort_catalog_variant_key(&self, selection: Option<&str>) -> Option<&str> {
        let normalized = self.normalize_reasoning_effort_selection(selection)?;
        let key = find_effort_variant_key(&self.catalog_variants, &normalized)?;
        let variant = self.catalog_variants.get(key)?;
        (!variant.disabled).then_some(key)
    }

    pub fn resolve_reasoning_effort_selection(
        &self,
        selection: Option<&str>,
    ) -> ResolvedReasoningRequest {
        let normalized_selection = self.normalize_reasoning_effort_selection(selection);

        if let Some(variant_key) = self.effort_catalog_variant_key(selection) {
            let variant = &self.catalog_variants[variant_key];
            let effective_reasoning_effort = normalized_selection
                .as_deref()
                .and_then(|value| match value {
                    "off" => None,
                    "on" => self.default_reasoning_effort,
                    other => other.parse::<ReasoningEffort>().ok(),
                })
                .map(|effort| self.nearest_supported_reasoning_effort(effort));
            return ResolvedReasoningRequest {
                request_model: variant
                    .request_model
                    .clone()
                    .unwrap_or_else(|| self.slug.clone()),
                request_thinking: None,
                request_reasoning_effort: None,
                effective_reasoning_effort,
                extra_body: None,
            };
        }

        match self.effective_reasoning_implementation() {
            ReasoningImplementation::Disabled => ResolvedReasoningRequest {
                request_model: self.slug.clone(),
                request_thinking: None,
                request_reasoning_effort: None,
                effective_reasoning_effort: None,
                extra_body: None,
            },
            ReasoningImplementation::RequestParameter => {
                let (request_thinking, request_reasoning_effort, effective_reasoning_effort) =
                    match self.effective_reasoning_capability() {
                        ReasoningCapability::Unsupported => (None, None, None),
                        ReasoningCapability::Toggle => {
                            let logical = normalized_selection
                                .filter(|selection| selection == "on" || selection == "off")
                                .or_else(|| self.default_reasoning_effort_selection());
                            let effective_reasoning_effort = logical
                                .as_deref()
                                .filter(|selection| *selection == "on")
                                .and(self.default_reasoning_effort);
                            let request_thinking =
                                logical.as_deref().map(adapter_request_thinking_wire);
                            (request_thinking, None, effective_reasoning_effort)
                        }
                        ReasoningCapability::Levels(_) => {
                            let allows_off = self.reasoning_capability.allows_off();
                            if allows_off {
                                let request_reasoning_effort = normalized_selection
                                    .as_deref()
                                    .and_then(|selection| match selection {
                                        "on" => self.default_reasoning_effort,
                                        "off" => None,
                                        _ => selection.parse::<ReasoningEffort>().ok(),
                                    })
                                    .map(|effort| self.nearest_supported_reasoning_effort(effort))
                                    .or_else(|| {
                                        normalized_selection
                                            .as_deref()
                                            .filter(|selection| *selection == "on")
                                            .and(self.default_reasoning_effort)
                                    });
                                let request_thinking = normalized_selection.as_deref().map_or_else(
                                    || {
                                        request_reasoning_effort
                                            .map(|_| String::from("enabled"))
                                            .or_else(|| Some(String::from("disabled")))
                                    },
                                    |selection| {
                                        if selection == "off" {
                                            Some(String::from("disabled"))
                                        } else {
                                            Some(String::from("enabled"))
                                        }
                                    },
                                );
                                (
                                    request_thinking,
                                    request_reasoning_effort,
                                    request_reasoning_effort,
                                )
                            } else {
                                let request_reasoning_effort = normalized_selection
                                    .as_deref()
                                    .and_then(|selection| selection.parse::<ReasoningEffort>().ok())
                                    .map(|effort| self.nearest_supported_reasoning_effort(effort))
                                    .or(self.default_reasoning_effort);
                                (
                                    request_reasoning_effort
                                        .map(|effort| effort.label().to_lowercase()),
                                    request_reasoning_effort,
                                    request_reasoning_effort,
                                )
                            }
                        }
                    };
                ResolvedReasoningRequest {
                    request_model: self.slug.clone(),
                    request_thinking,
                    request_reasoning_effort,
                    effective_reasoning_effort,
                    extra_body: None,
                }
            }
            ReasoningImplementation::ModelVariant(config) => {
                let selected_variant = normalized_selection
                    .as_deref()
                    .and_then(|selection| {
                        config.variants.iter().find(|variant| {
                            normalize_reasoning_effort_literal(&variant.selection_value)
                                == selection
                        })
                    })
                    .or_else(|| {
                        self.default_reasoning_effort_selection()
                            .as_deref()
                            .and_then(|selection| {
                                config.variants.iter().find(|variant| {
                                    normalize_reasoning_effort_literal(&variant.selection_value)
                                        == selection
                                })
                            })
                    })
                    .or_else(|| config.variants.first());
                if let Some(variant) = selected_variant {
                    ResolvedReasoningRequest {
                        request_model: variant.model.clone(),
                        request_thinking: None,
                        request_reasoning_effort: variant.reasoning_effort,
                        effective_reasoning_effort: variant.reasoning_effort,
                        extra_body: variant.extra_body.clone(),
                    }
                } else {
                    ResolvedReasoningRequest {
                        request_model: self.slug.clone(),
                        request_thinking: None,
                        request_reasoning_effort: self.default_reasoning_effort,
                        effective_reasoning_effort: self.default_reasoning_effort,
                        extra_body: None,
                    }
                }
            }
        }
    }
}

/// Provides read-only access to resolved runtime model definitions.
pub trait ModelCatalog: Send + Sync {
    /// Lists all models that are available for user-facing selection.
    fn list_visible(&self) -> Vec<&Model>;

    /// Lists provider directory entries associated with this model catalog.
    ///
    /// Implementations that only provide models may return an empty list. A
    /// bundled provider/model directory should return stable provider ids,
    /// display names, endpoints, and credential references without exposing
    /// credential values.
    fn list_providers(&self) -> Vec<ProviderInfo> {
        Vec::new()
    }

    /// Lists provider ids from the read-only built-in directory.
    ///
    /// Resolved catalog overlays may also expose user-defined providers, so
    /// callers should use this list to distinguish templates from Connections.
    fn list_template_provider_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// Lists model metadata below one provider directory entry.
    ///
    /// Catalog implementations with richer source metadata should override
    /// this method. The default projection keeps older in-memory catalogs
    /// compatible while still exposing the canonical nested shape.
    fn list_provider_models(&self, provider_id: &str) -> BTreeMap<String, ProviderModelInfo> {
        self.list_visible()
            .into_iter()
            .filter_map(|model| {
                let (model_provider, model_id) = model.slug.split_once('/')?;
                if model_provider != provider_id {
                    return None;
                }
                Some((
                    model_id.to_string(),
                    ProviderModelInfo {
                        name: Some(model.display_name.clone()),
                        wire_api: Some(model.provider),
                        context_window: Some(model.context_window),
                        effective_context_window_percent: model.effective_context_window_percent,
                        max_tokens: model.max_tokens,
                        temperature: model.temperature,
                        top_p: model.top_p,
                        top_k: model.top_k,
                        reasoning_capability: Some(model.reasoning_capability.clone()),
                        reasoning_implementation: model.reasoning_implementation.clone(),
                        default_reasoning_effort: model.default_reasoning_effort,
                        base_instructions: Some(model.base_instructions.clone()),
                        input_modalities: Some(model.input_modalities.clone()),
                        channel: model.channel.clone(),
                        ..ProviderModelInfo::default()
                    },
                ))
            })
            .collect()
    }

    /// Returns the model whose slug exactly matches `slug`.
    fn get(&self, slug: &str) -> Option<&Model>;

    /// Resolves the model that should be used for a turn.
    ///
    /// `requested` is an optional model slug supplied by the caller. When it is
    /// present, implementations must return that exact model or
    /// [`ModelError::ModelNotFound`]. When it is absent, implementations should
    /// return their highest-priority visible model or [`ModelError::NoVisibleModels`]
    /// if no selectable model exists.
    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError>;
}

/// TODO: Do we really need a In memory model catalog? let's remove it.
#[derive(Debug, Clone)]
pub struct InMemoryModelCatalog {
    models: Vec<Model>,
}

impl InMemoryModelCatalog {
    pub fn new(models: Vec<Model>) -> Self {
        Self { models }
    }
}

impl ModelCatalog for InMemoryModelCatalog {
    fn list_visible(&self) -> Vec<&Model> {
        self.models.iter().collect()
    }

    fn get(&self, slug: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.slug == slug)
    }

    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError> {
        if let Some(slug) = requested {
            return self.get(slug).ok_or_else(|| ModelError::ModelNotFound {
                slug: slug.to_string(),
            });
        }

        self.list_visible()
            .into_iter()
            .next()
            .ok_or(ModelError::NoVisibleModels)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("model not found: {slug}")]
    ModelNotFound { slug: String },
    #[error("no visible models available")]
    NoVisibleModels,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ModelCatalogEntry {
    pub slug: String,
    pub display_name: String,
    pub channel: Option<String>,
    pub description: Option<String>,
    pub provider: ProviderWireApi,
    pub context_window: u32,
    pub reasoning_capability: ReasoningCapability,
    pub input_modalities: Vec<InputModality>,
    pub max_tokens: Option<u32>,
    pub default_reasoning_selection: Option<String>,
}

impl From<&Model> for ModelCatalogEntry {
    fn from(m: &Model) -> Self {
        Self {
            slug: m.slug.clone(),
            display_name: m.display_name.clone(),
            channel: m.channel.clone(),
            description: m.description.clone(),
            provider: m.provider,
            context_window: m.context_window,
            reasoning_capability: m.reasoning_capability.clone(),
            input_modalities: m.input_modalities.clone(),
            max_tokens: m.max_tokens,
            default_reasoning_selection: m.default_reasoning_selection.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ReasoningVariant;
    use crate::ReasoningVariantConfig;
    use crate::RequestRole;
    use pretty_assertions::assert_eq;

    use super::InMemoryModelCatalog;
    use super::InputModality;
    use super::Model;
    use super::ModelCatalog;
    use super::ProviderWireApi;
    use super::ReasoningCapability;
    use super::ReasoningEffort;
    use super::ReasoningImplementation;
    use super::TruncationPolicyConfig;

    fn model(slug: &str) -> Model {
        Model {
            slug: slug.into(),
            display_name: slug.into(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            description: None,
            reasoning_capability: ReasoningCapability::Unsupported,
            default_reasoning_effort: Some(ReasoningEffort::Medium),
            default_reasoning_selection: None,
            reasoning_implementation: None,
            catalog_variants: Default::default(),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig {
                mode: crate::TruncationMode::Tokens,
                limit: 10000,
            },
            input_modalities: vec![InputModality::Text],
            supports_image_detail_original: false,
            channel: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
        }
    }

    #[test]
    fn resolve_for_turn_honors_requested_slug() {
        let catalog = InMemoryModelCatalog::new(vec![model("test")]);
        let resolved = catalog
            .resolve_for_turn(Some("test"))
            .expect("resolve explicit");
        assert_eq!(resolved.slug, "test");
    }

    #[test]
    fn supports_apply_patch_only_for_openai_channel() {
        let mut openai = model("gpt-5.5");
        openai.channel = Some("OpenAI".into());
        assert_eq!(openai.supports_apply_patch(), true);

        let mut poolside = model("laguna-s-2.1");
        poolside.channel = Some("Poolside".into());
        assert_eq!(poolside.supports_apply_patch(), false);

        let unset = model("custom");
        assert_eq!(unset.supports_apply_patch(), false);
    }

    #[test]
    fn provider_wire_api_as_str_returns_canonical_values() {
        for (wire_api, expected) in [
            (
                ProviderWireApi::OpenAIChatCompletions,
                "openai_chat_completions",
            ),
            (ProviderWireApi::OpenAIResponses, "openai_responses"),
            (ProviderWireApi::AnthropicMessages, "anthropic_messages"),
        ] {
            assert_eq!(wire_api.as_str(), expected);
        }
    }

    #[test]
    fn provider_wire_api_as_str_matches_serialized_values() {
        for wire_api in [
            ProviderWireApi::OpenAIChatCompletions,
            ProviderWireApi::OpenAIResponses,
            ProviderWireApi::AnthropicMessages,
        ] {
            assert_eq!(
                serde_json::to_value(wire_api).expect("serialize wire api"),
                serde_json::Value::String(wire_api.as_str().to_string())
            );
        }
    }

    #[test]
    fn provider_wire_api_display_and_from_use_canonical_values() {
        for (wire_api, expected) in [
            (
                ProviderWireApi::OpenAIChatCompletions,
                "openai_chat_completions",
            ),
            (ProviderWireApi::OpenAIResponses, "openai_responses"),
            (ProviderWireApi::AnthropicMessages, "anthropic_messages"),
        ] {
            let converted: &'static str = wire_api.into();

            assert_eq!(wire_api.to_string(), expected);
            assert_eq!(converted, expected);
        }
    }

    #[test]
    fn resolve_reasoning_effort_selection_disables_request_thinking_when_capability_is_disabled() {
        let preset = model("test");

        let resolved = preset.resolve_reasoning_effort_selection(Some("enabled"));

        assert_eq!(resolved.request_model, "test");
        assert_eq!(resolved.request_thinking, None);
        assert_eq!(resolved.effective_reasoning_effort, None);
    }

    #[test]
    fn resolve_reasoning_effort_selection_uses_request_parameter_for_toggle_models() {
        let mut preset = model("glm-5.1");
        preset.reasoning_capability = ReasoningCapability::Toggle;

        let resolved = preset.resolve_reasoning_effort_selection(Some("disabled"));

        assert_eq!(resolved.request_model, "glm-5.1");
        assert_eq!(resolved.request_thinking, Some(String::from("disabled")));
        assert_eq!(resolved.effective_reasoning_effort, None);
    }

    #[test]
    fn resolve_reasoning_effort_selection_snaps_effort_for_level_models() {
        let mut preset = model("o-model");
        preset.reasoning_capability = ReasoningCapability::Levels(vec![
            ReasoningEffort::Low.into(),
            ReasoningEffort::High.into(),
        ]);
        preset.default_reasoning_effort = Some(ReasoningEffort::Low);

        let resolved = preset.resolve_reasoning_effort_selection(Some("medium"));

        assert_eq!(resolved.request_model, "o-model");
        assert_eq!(resolved.request_thinking, Some(String::from("low")));
        assert_eq!(
            resolved.effective_reasoning_effort,
            Some(ReasoningEffort::Low)
        );
    }

    #[test]
    fn resolve_reasoning_effort_selection_supports_levels_with_off() {
        let mut preset = model("deepseek-v4");
        preset.reasoning_capability =
            ReasoningCapability::Levels(crate::levels_with_leading_off([
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]));
        preset.default_reasoning_effort = Some(ReasoningEffort::High);

        let enabled = preset.resolve_reasoning_effort_selection(Some("enabled"));
        assert_eq!(enabled.request_thinking, Some(String::from("enabled")));
        assert_eq!(
            enabled.request_reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert_eq!(
            enabled.effective_reasoning_effort,
            Some(ReasoningEffort::High)
        );

        let max = preset.resolve_reasoning_effort_selection(Some("max"));
        assert_eq!(max.request_thinking, Some(String::from("enabled")));
        assert_eq!(max.request_reasoning_effort, Some(ReasoningEffort::Max));
        assert_eq!(max.effective_reasoning_effort, Some(ReasoningEffort::Max));

        let disabled = preset.resolve_reasoning_effort_selection(Some("disabled"));
        assert_eq!(disabled.request_thinking, Some(String::from("disabled")));
        assert_eq!(disabled.request_reasoning_effort, None);
        assert_eq!(disabled.effective_reasoning_effort, None);
    }

    #[test]
    fn resolve_reasoning_effort_selection_treats_default_as_absent() {
        let mut toggle = model("toggle-model");
        toggle.reasoning_capability = ReasoningCapability::Toggle;

        let mut levels = model("levels-model");
        levels.reasoning_capability = ReasoningCapability::Levels(vec![
            ReasoningEffort::Low.into(),
            ReasoningEffort::High.into(),
        ]);
        levels.default_reasoning_effort = Some(ReasoningEffort::High);

        let mut levels_with_off = model("toggle-levels-model");
        levels_with_off.reasoning_capability =
            ReasoningCapability::Levels(crate::levels_with_leading_off([
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]));
        levels_with_off.default_reasoning_effort = Some(ReasoningEffort::High);

        for preset in [toggle, levels, levels_with_off] {
            let absent = preset.resolve_reasoning_effort_selection(None);

            assert_eq!(
                preset.resolve_reasoning_effort_selection(Some("default")),
                absent
            );
            assert_eq!(
                preset.resolve_reasoning_effort_selection(Some("  ")),
                absent
            );
        }
    }

    #[test]
    fn model_default_reasoning_selection_preserves_toggle_off() {
        let mut model = model("toggle-model");
        model.reasoning_capability = ReasoningCapability::Toggle;
        model.default_reasoning_selection = Some("disabled".to_string());

        assert_eq!(
            model.default_reasoning_effort_selection(),
            Some("off".to_string())
        );
        assert_eq!(
            model.resolve_reasoning_effort_selection(None),
            crate::ResolvedReasoningRequest {
                request_model: "toggle-model".to_string(),
                request_thinking: Some("disabled".to_string()),
                request_reasoning_effort: None,
                effective_reasoning_effort: None,
                extra_body: None,
            }
        );
    }

    #[test]
    fn resolve_reasoning_effort_selection_uses_catalog_variants_when_present() {
        let mut preset = model("custom/gateway-model");
        preset.reasoning_capability = ReasoningCapability::Levels(vec![
            ReasoningEffort::Low.into(),
            ReasoningEffort::High.into(),
        ]);
        preset.catalog_variants = [
            (
                "low".to_string(),
                super::ModelEffortVariant {
                    request_model: Some("gateway-model-fast".to_string()),
                    disabled: false,
                },
            ),
            (
                "high".to_string(),
                super::ModelEffortVariant {
                    request_model: Some("gateway-model-think".to_string()),
                    disabled: false,
                },
            ),
        ]
        .into_iter()
        .collect();

        let resolved = preset.resolve_reasoning_effort_selection(Some("high"));
        assert_eq!(resolved.request_model, "gateway-model-think");
        assert_eq!(resolved.request_thinking, None);
        assert_eq!(resolved.request_reasoning_effort, None);
        assert_eq!(
            resolved.effective_reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert_eq!(preset.effort_catalog_variant_key(Some("low")), Some("low"));
    }

    #[test]
    fn resolve_reasoning_effort_selection_uses_model_variants_when_configured() {
        let mut preset = model("kimi-k2.5");
        preset.reasoning_capability = ReasoningCapability::Toggle;
        preset.reasoning_implementation = Some(ReasoningImplementation::ModelVariant(
            ReasoningVariantConfig {
                variants: vec![
                    ReasoningVariant {
                        selection_value: String::from("disabled"),
                        model: String::from("kimi-k2.5"),
                        reasoning_effort: None,
                        label: String::from("Off"),
                        description: String::from("Use the standard model"),
                        extra_body: None,
                    },
                    ReasoningVariant {
                        selection_value: String::from("enabled"),
                        model: String::from("kimi-k2.5-thinking"),
                        reasoning_effort: Some(ReasoningEffort::Medium),
                        label: String::from("On"),
                        description: String::from("Use the reasoning model"),
                        extra_body: None,
                    },
                ],
            },
        ));

        let resolved = preset.resolve_reasoning_effort_selection(Some("enabled"));

        assert_eq!(resolved.request_model, "kimi-k2.5-thinking");
        assert_eq!(resolved.request_thinking, None);
        assert_eq!(
            resolved.effective_reasoning_effort,
            Some(ReasoningEffort::Medium)
        );
    }

    #[test]
    fn resolve_reasoning_effort_selection_falls_back_to_first_variant_when_selection_is_invalid() {
        let mut preset = model("deepseek-chat");
        preset.reasoning_capability = ReasoningCapability::Toggle;
        preset.reasoning_implementation = Some(ReasoningImplementation::ModelVariant(
            ReasoningVariantConfig {
                variants: vec![ReasoningVariant {
                    selection_value: String::from("disabled"),
                    model: String::from("deepseek-chat"),
                    reasoning_effort: None,
                    label: String::from("Off"),
                    description: String::from("Use the standard model"),
                    extra_body: None,
                }],
            },
        ));

        let resolved = preset.resolve_reasoning_effort_selection(Some("invalid"));

        assert_eq!(resolved.request_model, "deepseek-chat");
        assert_eq!(resolved.request_thinking, None);
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn tool_definition_serde_roundtrip() {
        let def = ToolDefinition {
            name: "bash".into(),
            description: "run commands".into(),
            input_schema: json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
            output_schema: None,
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "bash");
        assert_eq!(deserialized.description, "run commands");
    }

    #[test]
    fn request_content_text_serde() {
        let content = RequestContent::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"text""#));
        let deserialized: RequestContent = serde_json::from_str(&json).unwrap();
        match deserialized {
            RequestContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn request_content_reasoning_serde() {
        let content = RequestContent::Reasoning {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"reasoning""#));
        let deserialized: RequestContent = serde_json::from_str(&json).unwrap();
        match deserialized {
            RequestContent::Reasoning { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Reasoning"),
        }
    }

    #[test]
    fn request_content_tool_result_skips_none_error() {
        let content = RequestContent::ToolResult {
            tool_use_id: "t1".into(),
            content: "ok".into(),
            is_error: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(!json.contains("is_error"));
    }

    #[test]
    fn request_content_tool_result_includes_error() {
        let content = RequestContent::ToolResult {
            tool_use_id: "t1".into(),
            content: "failed".into(),
            is_error: Some(true),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("is_error"));
    }

    #[test]
    fn model_request_serde() {
        let req = ModelRequest {
            model_slug: ModelProfileKey::Generic,
            model: "claude-sonnet-4-20250514".into(),
            system: Some("You are helpful.".into()),
            messages: vec![RequestMessage {
                role: "user".into(),
                content: vec![RequestContent::Text { text: "hi".into() }],
            }],
            max_tokens: 4096,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("model_slug"));
        assert!(!json.contains("tools"));
        assert!(!json.contains("temperature"));
        let deserialized: ModelRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model_slug, ModelProfileKey::Generic);
        assert_eq!(deserialized.model, "claude-sonnet-4-20250514");
        assert_eq!(deserialized.messages.len(), 1);
    }

    #[test]
    fn model_effective_context_window_uses_configured_percent() {
        let model = Model {
            context_window: 1_000,
            effective_context_window_percent: Some(80.0),
            ..Model::default()
        };

        assert_eq!(model.effective_context_window(), 800);
    }

    #[test]
    fn model_effective_context_window_keeps_fractional_percent() {
        let model = Model {
            context_window: 1_000_000,
            effective_context_window_percent: Some(33.3333),
            ..Model::default()
        };

        assert_eq!(model.effective_context_window(), 333_333);
    }

    #[test]
    fn model_effective_context_window_defaults_to_95_percent() {
        let model = Model {
            context_window: 1_000,
            effective_context_window_percent: None,
            ..Model::default()
        };

        assert_eq!(model.effective_context_window(), 950);
    }

    #[test]
    fn request_role_roundtrip() {
        for role in [
            RequestRole::System,
            RequestRole::Developer,
            RequestRole::User,
            RequestRole::Assistant,
            RequestRole::Tool,
            RequestRole::Function,
        ] {
            let rendered = role.as_str();
            let parsed: RequestRole = rendered.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }
}
