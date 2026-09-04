//! Canonical provider-connection and model-directory types.
//!
//! These types are intentionally map-shaped: a provider id identifies a
//! connection and a model id identifies a model below that connection. The
//! old provider/model binding vocabulary is kept only in internal migration
//! code and is not part of this public catalog contract.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{
    InputModality, ProviderWireApi, ReasoningCapability, ReasoningEffort, ReasoningImplementation,
};

/// A named model variant, such as a provider's fast or high-reasoning mode.
///
/// `request`, `options`, and `headers` are deliberately open-ended so a
/// provider integration can add capabilities without a protocol migration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelVariant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    /// Optional wire model id override when this variant is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// One model entry in a provider's directory.
///
/// The containing `models` map supplies the model id. No second slug, name,
/// or binding id is needed. `name` is display metadata only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelInfo {
    #[serde(
        default,
        alias = "display_name",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Open-ended model capabilities, compatible with directory sources such
    /// as OpenCode/models.dev (for example tools, input/output, attachment,
    /// and interleaved reasoning support).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    #[serde(
        default,
        alias = "context_window",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<u32>,
    #[serde(
        default,
        alias = "effective_context_window_percent",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_context_window_percent: Option<f64>,
    #[serde(default, alias = "max_tokens", skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(
        default,
        alias = "temperature",
        skip_serializing_if = "Option::is_none"
    )]
    pub temperature: Option<f64>,
    #[serde(default, alias = "top_p", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, alias = "top_k", skip_serializing_if = "Option::is_none")]
    pub top_k: Option<f64>,
    #[serde(
        default,
        alias = "reasoning_capability",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_capability: Option<ReasoningCapability>,
    #[serde(
        default,
        alias = "reasoning_implementation",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_implementation: Option<ReasoningImplementation>,
    #[serde(
        default,
        alias = "default_reasoning_effort",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// Exact persisted/UI selection, including `on` or `off` for
    /// toggle-capable models.
    #[serde(
        default,
        alias = "default_reasoning_selection",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_reasoning_selection: Option<String>,
    #[serde(
        default,
        alias = "base_instructions",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_instructions: Option<String>,
    #[serde(
        default,
        alias = "input_modalities",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_modalities: Option<Vec<InputModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "supports_image_detail_original")]
    pub supports_image_detail_original: Option<bool>,
    #[serde(
        default,
        alias = "truncation_policy",
        skip_serializing_if = "Option::is_none"
    )]
    pub truncation_policy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, ProviderModelVariant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

/// A provider Connection and the model directory available through it.
///
/// `credential` is an id into `auth.json`; it is never the secret itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    pub wire_apis: Vec<ProviderWireApi>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ProviderModelInfo>,
    pub enabled: bool,
}
