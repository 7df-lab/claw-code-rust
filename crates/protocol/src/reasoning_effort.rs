//! Reasoning effort metadata shared across the catalog, runtime, and UI.
//!
//! This module exists to keep the model schema focused while making the
//! reasoning-effort design explicit in one place.
//!
//! The motivation is that a user's logical reasoning-effort choice is not always
//! transported the same way to every provider or model family:
//!
//! - Some models do not expose configurable reasoning effort at all.
//! - Some models expose thinking as a request parameter such as `thinking`.
//! - Some models expose reasoning by publishing separate model variants, for
//!   example "deepseek-chat" vs "deepseek-reasoner".
//!
//! Because of that, the runtime should not treat the request `thinking` field
//! as the only representation of reasoning effort. Instead, the system uses a
//! two-step design:
//!
//! 1. The user or session stores a logical reasoning-effort selection such as
//!    `disabled`, `enabled`, or `medium`.
//! 2. The runtime resolves that logical selection into concrete provider
//!    request fields:
//!    - the final request model slug
//!    - the final optional `thinking` parameter
//!    - the effective reasoning effort
//!    - optional provider-specific extra request JSON
//!
//! This split is represented by two separate concepts:
//!
//! - `ReasoningCapability` describes what choices the UI should present.
//! - `ReasoningImplementation` describes how that choice should be applied to a
//!   request.
//!
//! Keeping those concerns separate lets the UI remain stable while the runtime
//! adapts request construction for very different provider behaviors. Provider
//! adapters then consume already-resolved request fields instead of embedding
//! model-variant logic themselves.
//!
//! `ResolvedReasoningRequest` is the boundary type produced by resolution. It is
//! the normalized transport-ready result of combining:
//!
//! - a logical model preset
//! - a logical reasoning-effort selection
//! - model-specific reasoning implementation rules
//!
//! That makes model-variant reasoning a catalog/runtime concern rather than a
//! provider-transport concern.

use std::str::FromStr;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use strum_macros::Display;
use strum_macros::EnumIter;
use ts_rs::TS;

/// Describes how a logical reasoning-effort selection should be applied to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningImplementation {
    /// Reasoning effort is not exposed for this model.
    Disabled,
    /// Reasoning effort is sent via the provider request payload for the same model slug.
    RequestParameter,
    /// Reasoning effort selects a different wire-model variant instead of a request parameter.
    ModelVariant(ReasoningVariantConfig),
}

/// Groups the available model variants used to realize reasoning-effort selections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReasoningVariantConfig {
    pub variants: Vec<ReasoningVariant>,
}

/// Maps one logical reasoning-effort selection to a concrete request model and defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReasoningVariant {
    /// Logical reasoning-effort selection value, such as `enabled` or `disabled`.
    pub selection_value: String,
    /// Concrete wire-model slug to send to the provider for this selection.
    pub model_slug: String,
    /// Effective reasoning effort implied by this variant, when one exists.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// User-facing label shown for this selection in pickers.
    pub label: String,
    /// User-facing description shown alongside the label.
    pub description: String,
    /// Optional provider-specific JSON merged into the request body.
    #[serde(default)]
    pub extra_body: Option<Value>,
}

/// Fully resolved request settings derived from a logical model plus reasoning-effort selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema, TS)]
pub struct ResolvedReasoningRequest {
    /// Final model slug that should be sent to the provider.
    pub request_model: String,
    /// Final `thinking` request parameter, when the provider expects one.
    pub request_thinking: Option<String>,
    /// Final reasoning effort request parameter, when the provider expects one.
    pub request_reasoning_effort: Option<ReasoningEffort>,
    /// Effective reasoning effort chosen after normalizing the selection.
    pub effective_reasoning_effort: Option<ReasoningEffort>,
    /// Provider-specific extra request JSON to merge into the outbound payload.
    pub extra_body: Option<Value>,
}

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
    TS,
    EnumIter,
    Hash,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReasoningEffort {
    // GPT reasoning effort: [none, minimal, low, medium, high, xhigh]
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    // DeepSeek V4 reasoning effort: [high, max]
    Max,
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err(format!("invalid reasoning_effort: {s}")),
        }
    }
}

/// Normalizes a persisted reasoning-effort selection literal for storage and
/// comparison: trimmed and ASCII-lowercased. Unlike
/// [`Model::normalize_reasoning_effort_selection`](crate::Model::normalize_reasoning_effort_selection)
/// this is model-agnostic — it keeps toggle keywords (`enabled`/`disabled`) and
/// the `"default"` marker untouched, and never falls back to a model default.
/// Read and write paths share it so a stored selection compares equal to the
/// same selection arriving in a patch.
pub fn normalize_reasoning_effort_literal(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
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
            Self::Max => "Max",
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
            Self::Max => "Most deliberate, highest effort",
        }
    }
}

fn reasoning_effort_wire_value(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "xhigh",
        ReasoningEffort::Max => "max",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ReasoningCapability;
    use super::ReasoningEffort;
    use super::ReasoningEffortOption;

    #[test]
    fn reasoning_effort_from_str_accepts_wire_values() {
        assert_eq!("none".parse::<ReasoningEffort>(), Ok(ReasoningEffort::None));
        assert_eq!(
            "minimal".parse::<ReasoningEffort>(),
            Ok(ReasoningEffort::Minimal)
        );
        assert_eq!("low".parse::<ReasoningEffort>(), Ok(ReasoningEffort::Low));
        assert_eq!(
            "medium".parse::<ReasoningEffort>(),
            Ok(ReasoningEffort::Medium)
        );
        assert_eq!("high".parse::<ReasoningEffort>(), Ok(ReasoningEffort::High));
        assert_eq!(
            "xhigh".parse::<ReasoningEffort>(),
            Ok(ReasoningEffort::XHigh)
        );
        assert_eq!("max".parse::<ReasoningEffort>(), Ok(ReasoningEffort::Max));
    }

    #[test]
    fn reasoning_effort_from_str_preserves_serde_strictness() {
        assert_eq!(
            "High".parse::<ReasoningEffort>(),
            Err("invalid reasoning_effort: High".to_string())
        );
        assert_eq!(
            " high ".parse::<ReasoningEffort>(),
            Err("invalid reasoning_effort:  high ".to_string())
        );
    }

    #[test]
    fn reasoning_options_use_reasoning_effort_wire_values() {
        assert_eq!(
            ReasoningCapability::ToggleWithLevels(vec![ReasoningEffort::XHigh]).options(),
            vec![
                ReasoningEffortOption {
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "disabled".to_string(),
                },
                ReasoningEffortOption {
                    label: "XHigh".to_string(),
                    description: "Most deliberate, highest effort".to_string(),
                    value: "xhigh".to_string(),
                },
            ]
        );
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
        ReasoningEffort::Max => 5,
    }
}

/// Picks the supported effort closest to the requested one.
pub(crate) fn nearest_effort(
    target: ReasoningEffort,
    supported: &[ReasoningEffort],
) -> ReasoningEffort {
    let target_rank = effort_rank(target);
    supported
        .iter()
        .copied()
        .min_by_key(|candidate| (effort_rank(*candidate) - target_rank).abs())
        .unwrap_or(target)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
/// One selectable reasoning-effort option presented to the UI or protocol client.
pub struct ReasoningEffortOption {
    pub label: String,
    pub description: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningCapability {
    /// Model reasoning effort cannot be controlled.
    Unsupported,
    /// Model reasoning effort can be toggled on and off.
    Toggle,
    /// Multiple reasoning effort levels can be selected.
    Levels(Vec<ReasoningEffort>),
    /// Reasoning effort can be turned off, or enabled with one of several effort levels.
    ToggleWithLevels(Vec<ReasoningEffort>),
}

impl ReasoningCapability {
    pub fn options(&self) -> Vec<ReasoningEffortOption> {
        match self {
            ReasoningCapability::Unsupported => Vec::new(),
            ReasoningCapability::Toggle => vec![
                ReasoningEffortOption {
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "disabled".to_string(),
                },
                ReasoningEffortOption {
                    label: "On".to_string(),
                    description: "Enable model reasoning effort".to_string(),
                    value: "enabled".to_string(),
                },
            ],
            ReasoningCapability::Levels(levels) => {
                let mut presets = Vec::with_capacity(levels.len());
                presets.extend(
                    levels
                        .iter()
                        .copied()
                        .map(reasoning_effort_option_for_effort),
                );
                presets
            }
            ReasoningCapability::ToggleWithLevels(levels) => {
                let mut presets = Vec::with_capacity(levels.len() + 1);
                presets.push(ReasoningEffortOption {
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "disabled".to_string(),
                });
                presets.extend(
                    levels
                        .iter()
                        .copied()
                        .map(reasoning_effort_option_for_effort),
                );
                presets
            }
        }
    }
}

fn reasoning_effort_option_for_effort(effort: ReasoningEffort) -> ReasoningEffortOption {
    ReasoningEffortOption {
        label: effort.label().to_string(),
        description: effort.description().to_string(),
        value: reasoning_effort_wire_value(effort).to_string(),
    }
}
