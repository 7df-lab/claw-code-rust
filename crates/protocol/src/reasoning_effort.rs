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
//!    `off`, `on`, or `medium` (legacy `disabled`/`enabled` normalize to
//!    `off`/`on` on read).
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
    /// Logical reasoning-effort selection value, such as `on` or `off`.
    pub selection_value: String,
    /// Concrete provider model id to send for this selection.
    ///
    /// `model_slug` remains a read-only serde alias for older configuration
    /// during startup migration; new JSON uses `model`.
    #[serde(alias = "model_slug")]
    pub model: String,
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
/// this is model-agnostic — it maps legacy toggle keywords (`enabled`/`disabled`)
/// to canonical `on`/`off`, keeps the `"default"` marker untouched, and never
/// falls back to a model default.
/// Read and write paths share it so a stored selection compares equal to the
/// same selection arriving in a patch.
pub fn normalize_reasoning_effort_literal(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "disabled" => String::from("off"),
        "enabled" => String::from("on"),
        other => other.to_string(),
    }
}

/// Maps a canonical logical toggle/effort selection onto the wire value expected
/// by built-in adapters that still speak `disabled`/`enabled` for thinking.
pub fn adapter_request_thinking_wire(selection: &str) -> String {
    match selection {
        "off" => String::from("disabled"),
        "on" => String::from("enabled"),
        other => other.to_string(),
    }
}

/// Candidate catalog-variant keys for a normalized logical selection.
///
/// Canonical keys come first; legacy toggle aliases follow so old overlays keep
/// matching during the migration window.
pub fn effort_variant_lookup_keys(selection: &str) -> Vec<String> {
    let normalized = normalize_reasoning_effort_literal(selection);
    match normalized.as_str() {
        "off" => vec![String::from("off"), String::from("disabled")],
        "on" => vec![String::from("on"), String::from("enabled")],
        _ => vec![normalized],
    }
}

/// Finds the first catalog variant key that matches a logical effort selection.
pub fn find_effort_variant_key<'a, V>(
    variants: &'a std::collections::BTreeMap<String, V>,
    selection: &str,
) -> Option<&'a str> {
    for key in effort_variant_lookup_keys(selection) {
        if let Some(matched) = variants.keys().find(|candidate| candidate.as_str() == key) {
            return Some(matched.as_str());
        }
    }
    None
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

/// One entry in a [`ReasoningCapability::Levels`] list.
///
/// Logical `off` disables reasoning; effort variants are the selectable depths.
/// Legacy `disabled` deserializes as [`Self::Off`].
///
/// On the wire this is a plain string (`"off"` or an effort label), not a tagged
/// object — keep the TypeScript alias aligned with that shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, TS)]
#[ts(type = "\"off\" | ReasoningEffort")]
pub enum ReasoningLevelChoice {
    Off,
    Effort(ReasoningEffort),
}

impl ReasoningLevelChoice {
    pub fn selection_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Effort(effort) => reasoning_effort_wire_value(effort),
        }
    }

    pub fn effort(self) -> Option<ReasoningEffort> {
        match self {
            Self::Off => None,
            Self::Effort(effort) => Some(effort),
        }
    }

    pub fn option(self) -> ReasoningEffortOption {
        match self {
            Self::Off => ReasoningEffortOption {
                label: "Off".to_string(),
                description: "Disable reasoning effort for this turn".to_string(),
                value: "off".to_string(),
            },
            Self::Effort(effort) => reasoning_effort_option_for_effort(effort),
        }
    }
}

impl From<ReasoningEffort> for ReasoningLevelChoice {
    fn from(effort: ReasoningEffort) -> Self {
        Self::Effort(effort)
    }
}

impl Serialize for ReasoningLevelChoice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.selection_value())
    }
}

impl<'de> Deserialize<'de> for ReasoningLevelChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let normalized = normalize_reasoning_effort_literal(&raw);
        match normalized.as_str() {
            "off" => Ok(Self::Off),
            other => other
                .parse::<ReasoningEffort>()
                .map(Self::Effort)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for ReasoningLevelChoice {
    fn schema_name() -> String {
        "ReasoningLevelChoice".to_string()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        String::json_schema(generator)
    }
}

/// Prepends [`ReasoningLevelChoice::Off`] when migrating legacy
/// `toggle_with_levels` arrays that listed only effort depths.
pub fn levels_with_leading_off(
    efforts: impl IntoIterator<Item = ReasoningEffort>,
) -> Vec<ReasoningLevelChoice> {
    let mut choices = vec![ReasoningLevelChoice::Off];
    for effort in efforts {
        let choice = ReasoningLevelChoice::Effort(effort);
        if !choices.contains(&choice) {
            choices.push(choice);
        }
    }
    choices
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningCapability {
    /// Model reasoning effort cannot be controlled.
    Unsupported,
    /// Model reasoning effort can be toggled on and off.
    Toggle,
    /// Selectable reasoning chips in array order. Include [`ReasoningLevelChoice::Off`]
    /// when the model can disable reasoning; omit it when reasoning is always on.
    Levels(Vec<ReasoningLevelChoice>),
}

impl<'de> Deserialize<'de> for ReasoningCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "unsupported" => Ok(Self::Unsupported),
                "toggle" => Ok(Self::Toggle),
                other => Err(serde::de::Error::custom(format!(
                    "invalid reasoning_capability string '{other}'; expected unsupported or toggle"
                ))),
            },
            Value::Object(map) => {
                if let Some(levels) = map.get("levels") {
                    let choices: Vec<ReasoningLevelChoice> =
                        serde_json::from_value(levels.clone()).map_err(serde::de::Error::custom)?;
                    return Ok(Self::Levels(choices));
                }
                let legacy_levels = map
                    .get("toggle_with_levels")
                    .or_else(|| map.get("togglewithlevels"));
                if let Some(levels) = legacy_levels {
                    let efforts: Vec<ReasoningEffort> =
                        serde_json::from_value(levels.clone()).map_err(serde::de::Error::custom)?;
                    return Ok(Self::Levels(levels_with_leading_off(efforts)));
                }
                Err(serde::de::Error::custom(
                    "invalid reasoning_capability object; expected levels or legacy toggle_with_levels",
                ))
            }
            _ => Err(serde::de::Error::custom(
                "invalid reasoning_capability; expected string or object",
            )),
        }
    }
}

impl ReasoningCapability {
    /// Effort depths listed in a [`Self::Levels`] capability (excludes `off`).
    pub fn effort_levels(&self) -> Vec<ReasoningEffort> {
        match self {
            Self::Levels(choices) => choices
                .iter()
                .copied()
                .filter_map(ReasoningLevelChoice::effort)
                .collect(),
            Self::Unsupported | Self::Toggle => Vec::new(),
        }
    }

    /// True when [`Self::Levels`] includes logical `off` (hybrid / disableable).
    pub fn allows_off(&self) -> bool {
        matches!(
            self,
            Self::Levels(choices) if choices.iter().any(|choice| matches!(choice, ReasoningLevelChoice::Off))
        )
    }

    pub fn options(&self) -> Vec<ReasoningEffortOption> {
        match self {
            ReasoningCapability::Unsupported => Vec::new(),
            ReasoningCapability::Toggle => vec![
                ReasoningEffortOption {
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "off".to_string(),
                },
                ReasoningEffortOption {
                    label: "On".to_string(),
                    description: "Enable model reasoning effort".to_string(),
                    value: "on".to_string(),
                },
            ],
            ReasoningCapability::Levels(levels) => levels
                .iter()
                .copied()
                .map(ReasoningLevelChoice::option)
                .collect(),
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ReasoningCapability;
    use super::ReasoningEffort;
    use super::ReasoningEffortOption;
    use super::ReasoningLevelChoice;
    use super::levels_with_leading_off;

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
            ReasoningCapability::Levels(levels_with_leading_off([ReasoningEffort::XHigh]))
                .options(),
            vec![
                ReasoningEffortOption {
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "off".to_string(),
                },
                ReasoningEffortOption {
                    label: "XHigh".to_string(),
                    description: "Most deliberate, highest effort".to_string(),
                    value: "xhigh".to_string(),
                },
            ]
        );
    }

    #[test]
    fn normalize_reasoning_effort_literal_maps_legacy_toggle_aliases() {
        assert_eq!(super::normalize_reasoning_effort_literal("disabled"), "off");
        assert_eq!(super::normalize_reasoning_effort_literal("ENABLED"), "on");
        assert_eq!(
            super::normalize_reasoning_effort_literal(" medium "),
            "medium"
        );
    }

    #[test]
    fn toggle_with_levels_migrates_to_levels_with_leading_off() {
        let canonical =
            serde_json::from_str::<ReasoningCapability>(r#"{"toggle_with_levels":["low","high"]}"#)
                .expect("canonical reasoning capability should deserialize");
        let legacy =
            serde_json::from_str::<ReasoningCapability>(r#"{"togglewithlevels":["low","high"]}"#)
                .expect("legacy reasoning capability should remain readable");

        assert_eq!(canonical, legacy);
        assert_eq!(
            canonical,
            ReasoningCapability::Levels(vec![
                ReasoningLevelChoice::Off,
                ReasoningLevelChoice::Effort(ReasoningEffort::Low),
                ReasoningLevelChoice::Effort(ReasoningEffort::High),
            ])
        );
        assert_eq!(
            serde_json::to_value(canonical).expect("reasoning capability should serialize"),
            serde_json::json!({
                "levels": ["off", "low", "high"]
            })
        );
    }

    #[test]
    fn levels_accepts_off_in_array() {
        let capability =
            serde_json::from_str::<ReasoningCapability>(r#"{"levels":["off","low","high"]}"#)
                .expect("levels with off should deserialize");
        assert_eq!(
            capability,
            ReasoningCapability::Levels(vec![
                ReasoningLevelChoice::Off,
                ReasoningLevelChoice::Effort(ReasoningEffort::Low),
                ReasoningLevelChoice::Effort(ReasoningEffort::High),
            ])
        );
        assert!(capability.allows_off());
        assert_eq!(
            capability.effort_levels(),
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
    }
}
