use std::str::FromStr;

use clawcr_provider::ProviderFamily;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumIter};

use crate::truncation::TruncationPolicyConfig;

/// OpenAI models support reasoning effort.
/// See <https://platform.openai.com/docs/guides/reasoning?api-mode=responses#get-started-with-reasoning>
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Display,
    JsonSchema,
    EnumIter,
    Hash,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| format!("invalid reasoning_effort: {s}"))
    }
}

impl ReasoningEffort {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "XHigh",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => "Disable extra reasoning effort",
            Self::Minimal => "Use the lightest supported reasoning effort",
            Self::Low => "Fastest, cheapest, least deliberative",
            Self::Medium => "Balanced speed and deliberation",
            Self::High => "More deliberate for harder tasks",
            Self::XHigh => "Most deliberate, highest effort",
        }
    }
}

/// Maps reasoning efforts onto a stable numeric scale for comparison.
fn effort_rank(effort: ReasoningEffort) -> i32 {
    match effort {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal => 1,
        ReasoningEffort::Low => 2,
        ReasoningEffort::Medium => 3,
        ReasoningEffort::High => 4,
        ReasoningEffort::XHigh => 5,
    }
}

/// Picks the supported effort closest to the requested one.
fn nearest_effort(target: ReasoningEffort, supported: &[ReasoningEffort]) -> ReasoningEffort {
    let target_rank = effort_rank(target);
    supported
        .iter()
        .copied()
        .min_by_key(|candidate| (effort_rank(*candidate) - target_rank).abs())
        .unwrap_or(target)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One selectable reasoning-effort option presented to the UI or protocol client.
pub struct ReasoningEffortPreset {
    pub effort: ReasoningEffort,
    pub description: String,
}

impl ReasoningEffortPreset {
    pub fn new(effort: ReasoningEffort, description: impl Into<String>) -> Self {
        Self {
            effort,
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One selectable thinking option presented to the UI or protocol client.
pub struct ThinkingPreset {
    pub label: String,
    pub description: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingCapability {
    /// Model thinking cannot be controlled.
    Disabled,
    /// Model thinking can be toggled on and off.
    Toggle,
    /// Multiple effort levels can be selected for thinking.
    Levels(Vec<ReasoningEffort>),
}

impl ThinkingCapability {
    pub fn options(&self) -> Vec<ThinkingPreset> {
        match self {
            ThinkingCapability::Disabled => Vec::new(),
            ThinkingCapability::Toggle => vec![
                ThinkingPreset {
                    label: "Off".to_string(),
                    description: "Disable thinking for this turn".to_string(),
                    value: "disabled".to_string(),
                },
                ThinkingPreset {
                    label: "On".to_string(),
                    description: "Enable the model's thinking mode".to_string(),
                    value: "enabled".to_string(),
                },
            ],
            ThinkingCapability::Levels(levels) => levels
                .iter()
                .copied()
                .map(|effort| ThinkingPreset {
                    label: effort.label().to_string(),
                    description: effort.description().to_string(),
                    value: effort.label().to_lowercase(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

impl Default for Verbosity {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Supported input types that a model can accept.
pub enum InputModality {
    /// Plain text input.
    Text,
    /// Image input.
    Image,
}

impl Default for InputModality {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// Static metadata and capability flags for one model in the catalog.
pub struct ModelPreset {
    /// Stable model identifier used in config and requests. such as `claude-sonnet-20250425`
    pub slug: String,
    /// Human-readable display name shown in the UI. such as `claude-sonnet-4.6`
    pub display_name: String,
    /// Provider family that serves this model.
    pub provider_family: ProviderFamily,
    /// Optional short description of the model.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub description: Option<String>,
    /// Thinking control available for this model.
    #[serde(
        default = "default_thinking_capability",
        deserialize_with = "deserialize_thinking_capability"
    )]
    pub thinking_capability: ThinkingCapability,
    /// Default reasoning effort selected for the model when no levels are exposed.
    #[serde(
        default = "default_reasoning_effort",
        alias = "default_reasoning_level",
        deserialize_with = "deserialize_reasoning_effort_option"
    )]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// Base system instructions bundled with the model.
    pub base_instructions: String,
    /// Maximum context window in tokens.
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    /// Percentage of the context window treated as effectively usable.
    pub effective_context_window_percent: Option<u8>,
    /// Policy used when truncating content for requests.
    #[serde(
        default,
        deserialize_with = "crate::truncation::deserialize_truncation_policy_config"
    )]
    pub truncation_policy: TruncationPolicyConfig,
    /// Input types accepted by the model.
    #[serde(default = "default_input_modalities")]
    pub input_modalities: Vec<InputModality>,
    /// Whether the model supports original-resolution image detail.
    pub supports_image_detail_original: bool,
    /// Whether the user configured API access for this model.
    #[serde(rename = "supported_in_api")]
    pub api_configured: bool,
    /// Default temperature to use when the model does not override it.
    pub temperature: Option<f32>,
    /// Default nucleus sampling value to use when the model does not override it.
    pub top_p: Option<f32>,
    /// Default top-k sampling value to use when the model does not override it.
    pub top_k: Option<f32>,
    /// Default maximum token limit for responses from this model.
    pub max_tokens: Option<u32>,
    /// Relative priority used when choosing a default visible model.
    pub priority: i32,
}

impl Default for ModelPreset {
    fn default() -> Self {
        Self {
            slug: String::new(),
            display_name: String::new(),
            provider_family: ProviderFamily::OpenAI,
            description: None,
            thinking_capability: ThinkingCapability::Disabled,
            default_reasoning_effort: Some(ReasoningEffort::default()),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig::default(),
            input_modalities: vec![InputModality::default()],
            supports_image_detail_original: false,
            api_configured: false,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            priority: 0,
        }
    }
}

impl ModelPreset {
    pub fn reasoning_effort_options(&self) -> Vec<ReasoningEffortPreset> {
        match &self.thinking_capability {
            ThinkingCapability::Levels(levels) => levels
                .iter()
                .copied()
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

    pub fn effective_thinking_capability(&self) -> ThinkingCapability {
        self.thinking_capability.clone()
    }

    pub fn effective_context_window_percent(&self) -> u8 {
        self.effective_context_window_percent.unwrap_or(95)
    }

    pub fn default_thinking_selection(&self) -> Option<String> {
        match &self.thinking_capability {
            ThinkingCapability::Disabled => None,
            ThinkingCapability::Toggle => Some(String::from("enabled")),
            ThinkingCapability::Levels(levels) => self
                .default_reasoning_effort
                .or_else(|| levels.first().copied())
                .map(|effort| effort.label().to_lowercase()),
        }
    }

    pub fn nearest_supported_reasoning_effort(&self, target: ReasoningEffort) -> ReasoningEffort {
        match &self.thinking_capability {
            ThinkingCapability::Levels(levels) if !levels.is_empty() => {
                nearest_effort(target, levels)
            }
            _ => self.default_reasoning_effort.unwrap_or(target),
        }
    }
}

fn default_reasoning_effort() -> Option<ReasoningEffort> {
    Some(ReasoningEffort::default())
}

fn default_context_window() -> u32 {
    200_000
}

fn default_input_modalities() -> Vec<InputModality> {
    vec![InputModality::Text, InputModality::Image]
}

fn default_thinking_capability() -> ThinkingCapability {
    ThinkingCapability::Disabled
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(value)
        }
    }))
}

fn deserialize_reasoning_effort_option<'de, D>(
    deserializer: D,
) -> Result<Option<ReasoningEffort>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(default_reasoning_effort()),
        serde_json::Value::String(text) if text.trim().is_empty() => Ok(default_reasoning_effort()),
        other => serde_json::from_value(other)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_thinking_capability<'de, D>(deserializer: D) -> Result<ThinkingCapability, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(default_thinking_capability()),
        serde_json::Value::String(text) if text.trim().is_empty() => {
            Ok(default_thinking_capability())
        }
        other => serde_json::from_value(other).map_err(serde::de::Error::custom),
    }
}

/// Provides read-only access to model definitions and turn-resolution behavior.
pub trait ModelCatalog: Send + Sync {
    fn list_visible(&self) -> Vec<&ModelPreset>;
    fn get(&self, slug: &str) -> Option<&ModelPreset>;
    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelPreset, ModelPresetError>;
}

#[derive(Debug, Clone)]
pub struct InMemoryModelCatalog {
    models: Vec<ModelPreset>,
}

impl InMemoryModelCatalog {
    pub fn new(models: Vec<ModelPreset>) -> Self {
        Self { models }
    }
}

impl ModelCatalog for InMemoryModelCatalog {
    fn list_visible(&self) -> Vec<&ModelPreset> {
        self.models.iter().collect()
    }

    fn get(&self, slug: &str) -> Option<&ModelPreset> {
        self.models.iter().find(|model| model.slug == slug)
    }

    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelPreset, ModelPresetError> {
        if let Some(slug) = requested {
            return self
                .get(slug)
                .ok_or_else(|| ModelPresetError::ModelNotFound {
                    slug: slug.to_string(),
                });
        }

        self.list_visible()
            .into_iter()
            .max_by_key(|model| model.priority)
            .ok_or(ModelPresetError::NoVisibleModels)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelPresetError {
    #[error("model not found: {slug}")]
    ModelNotFound { slug: String },
    #[error("no visible models available")]
    NoVisibleModels,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{
        InMemoryModelCatalog, InputModality, ModelCatalog, ModelPreset, ProviderFamily,
        ReasoningEffort, ThinkingCapability, TruncationPolicyConfig,
    };

    fn model(slug: &str, priority: i32) -> ModelPreset {
        ModelPreset {
            slug: slug.into(),
            display_name: slug.into(),
            provider_family: ProviderFamily::Anthropic,
            description: None,
            thinking_capability: ThinkingCapability::Disabled,
            default_reasoning_effort: Some(ReasoningEffort::Medium),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig {
                mode: crate::TruncationMode::Tokens,
                limit: 10000,
            },
            input_modalities: vec![InputModality::Text],
            supports_image_detail_original: false,
            api_configured: true,
            priority,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
        }
    }

    #[test]
    fn resolve_for_turn_honors_requested_slug() {
        let catalog = InMemoryModelCatalog::new(vec![model("test", 1)]);
        let resolved = catalog
            .resolve_for_turn(Some("test"))
            .expect("resolve explicit");
        assert_eq!(resolved.slug, "test");
    }
}
