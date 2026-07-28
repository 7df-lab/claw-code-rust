//! Builtin model catalog loading and resolution for core.
//!
//! The embedded `models.json` asset is the catalog base. Configuration can
//! override individual metadata fields or add custom models without creating
//! or reading a filesystem catalog.
use std::collections::BTreeMap;

use crate::{Model, ModelCatalog, ModelError, ModelPreset};
use devo_config::ModelOverrideConfig;

const BUILTIN_MODELS_JSON: &str = include_str!("../models.json");

pub use crate::model_preset::default_base_instructions;

/// A catalog resolved from embedded presets and configuration overrides.
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<Model>,
}

impl PresetModelCatalog {
    /// Loads the built-in embedded catalog only.
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Ok(Self {
            models: load_builtin_models()?,
        })
    }

    /// Loads the embedded catalog with configured metadata overrides.
    pub fn load_from_config(
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::with_model_overrides(model_overrides)
    }

    /// Loads embedded presets and applies overrides by model slug.
    pub fn with_model_overrides(
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
    ) -> Result<Self, PresetModelCatalogError> {
        let mut presets = load_builtin_model_presets()?;
        for (slug, overrides) in model_overrides {
            if let Some(preset) = presets.iter_mut().find(|preset| preset.slug == *slug) {
                preset.apply_overrides(overrides);
            } else {
                presets.push(ModelPreset::from_overrides(slug, overrides));
            }
        }

        // `sort_by` is stable, keeping custom zero-priority entries after the
        // embedded entries that were loaded first.
        presets.sort_by(|left, right| right.priority.cmp(&left.priority));
        Ok(Self {
            models: presets.into_iter().map(Model::from).collect(),
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<Model>) -> Self {
        Self { models }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<Model> {
        self.models
    }
}

impl ModelCatalog for PresetModelCatalog {
    fn list_visible(&self) -> Vec<&Model> {
        self.models.iter().collect()
    }

    fn get(&self, slug: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.slug == slug)
    }

    /// Resolves an explicit requested slug, or falls back to the first visible preset model.
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

/// Loads the built-in raw model preset list bundled with the crate.
pub fn load_builtin_model_presets() -> Result<Vec<ModelPreset>, PresetModelCatalogError> {
    serde_json::from_str(BUILTIN_MODELS_JSON).map_err(Into::into)
}

/// Loads the built-in model list bundled with the crate.
pub fn load_builtin_models() -> Result<Vec<Model>, PresetModelCatalogError> {
    let mut presets = load_builtin_model_presets()?;
    presets.sort_by(|left, right| right.priority.cmp(&left.priority));
    Ok(presets.into_iter().map(Model::from).collect())
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum PresetModelCatalogError {
    /// Parsing the bundled JSON file failed.
    #[error("failed to parse builtin model catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::{
        PresetModelCatalog, default_base_instructions, load_builtin_model_presets,
        load_builtin_models,
    };
    use crate::{
        InputModality, Model, ModelCatalog, ModelOverrideConfig, ProviderWireApi,
        ReasoningCapability, ReasoningEffort, ReasoningImplementation, TruncationPolicyConfig,
    };

    #[test]
    fn builtin_models_load_from_bundled_json() {
        let models = load_builtin_models().expect("load builtin models");
        assert!(!models.is_empty());
        assert_eq!(models[0].slug, "kimi-k3");
    }

    #[test]
    fn builtin_catalog_resolves_visible_defaults() {
        let catalog = PresetModelCatalog::load().expect("load catalog");
        let model = catalog.resolve_for_turn(None).expect("resolve default");
        assert!(!model.slug.is_empty());
    }

    #[test]
    fn default_base_instructions_are_available() {
        assert!(!default_base_instructions().trim().is_empty());
    }

    #[test]
    fn builtin_models_have_channel_fields() {
        let models = load_builtin_models().expect("load builtin models");
        assert!(
            models
                .iter()
                .any(|model| model.channel.as_deref() == Some("DeepSeek"))
        );
    }

    #[test]
    fn builtin_minimax_m3_supports_image_input() {
        let model = load_builtin_models()
            .expect("load builtin models")
            .into_iter()
            .find(|model| model.slug == "MiniMax-M3")
            .expect("MiniMax-M3 model");

        assert!(model.input_modalities.contains(&InputModality::Image));
    }

    #[test]
    fn load_from_config_applies_partial_builtin_override_without_replacing_metadata() {
        let builtin = load_builtin_models()
            .expect("load builtins")
            .into_iter()
            .find(|model| model.slug == "kimi-k3")
            .expect("kimi model");
        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "kimi-k3".to_string(),
            ModelOverrideConfig {
                display_name: Some("Configured Kimi".to_string()),
                ..ModelOverrideConfig::default()
            },
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog.get("kimi-k3").expect("configured kimi"),
            &Model {
                display_name: "Configured Kimi".to_string(),
                ..builtin
            }
        );
    }

    #[test]
    fn load_from_config_keeps_explicit_toggle_from_legacy_toggle_with_levels() {
        let builtin = load_builtin_models()
            .expect("load builtins")
            .into_iter()
            .find(|model| model.slug == "glm-5.2")
            .expect("glm model");
        assert!(matches!(
            builtin.reasoning_capability,
            ReasoningCapability::ToggleWithLevels(_)
        ));

        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "glm-5.2".to_string(),
            ModelOverrideConfig {
                reasoning_capability: Some(ReasoningCapability::Toggle),
                ..ModelOverrideConfig::default()
            },
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog.get("glm-5.2").expect("configured glm"),
            &Model {
                reasoning_capability: ReasoningCapability::Toggle,
                ..builtin
            }
        );
    }

    #[test]
    fn load_from_config_applies_complete_metadata_override() {
        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "qwen3-coder-next".to_string(),
            ModelOverrideConfig {
                display_name: Some("Configured Qwen".to_string()),
                description: Some("Configured description".to_string()),
                context_window: Some(128_000),
                effective_context_window_percent: Some(80),
                max_tokens: Some(8_192),
                temperature: Some(0.4),
                top_p: Some(0.7),
                top_k: Some(24.0),
                provider: Some(ProviderWireApi::AnthropicMessages),
                reasoning_capability: Some(ReasoningCapability::Levels(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                ])),
                reasoning_implementation: Some(ReasoningImplementation::RequestParameter),
                default_reasoning_effort: Some(ReasoningEffort::High),
                base_instructions: Some("Configured instructions".to_string()),
                input_modalities: Some(vec![InputModality::Image]),
                channel: Some("Configured channel".to_string()),
                truncation_policy: Some(TruncationPolicyConfig::tokens(4_096)),
                supports_image_detail_original: Some(true),
            },
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog.get("qwen3-coder-next").expect("configured qwen"),
            &Model {
                slug: "qwen3-coder-next".to_string(),
                display_name: "Configured Qwen".to_string(),
                provider: ProviderWireApi::AnthropicMessages,
                description: Some("Configured description".to_string()),
                reasoning_capability: ReasoningCapability::Levels(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                ]),
                default_reasoning_effort: Some(ReasoningEffort::High),
                reasoning_implementation: Some(ReasoningImplementation::RequestParameter),
                base_instructions: "Configured instructions".to_string(),
                context_window: 128_000,
                effective_context_window_percent: Some(80),
                truncation_policy: TruncationPolicyConfig::tokens(4_096),
                input_modalities: vec![InputModality::Image],
                supports_image_detail_original: true,
                channel: Some("Configured channel".to_string()),
                temperature: Some(0.4),
                top_p: Some(0.7),
                top_k: Some(24.0),
                max_tokens: Some(8_192),
            }
        );
    }

    #[test]
    fn load_from_config_creates_minimal_custom_model_with_fallback_instructions() {
        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "custom".to_string(),
            ModelOverrideConfig::default(),
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog.get("custom").expect("custom model"),
            &Model {
                slug: "custom".to_string(),
                display_name: "custom".to_string(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            }
        );
    }

    #[test]
    fn load_from_config_creates_fully_specified_custom_model() {
        let catalog = PresetModelCatalog::with_model_overrides(&BTreeMap::from([(
            "custom".to_string(),
            ModelOverrideConfig {
                display_name: Some("Custom".to_string()),
                description: Some("Custom description".to_string()),
                context_window: Some(64_000),
                effective_context_window_percent: Some(75),
                max_tokens: Some(4_096),
                temperature: Some(0.2),
                top_p: Some(0.6),
                top_k: Some(12.0),
                provider: Some(ProviderWireApi::OpenAIResponses),
                reasoning_capability: Some(ReasoningCapability::Toggle),
                reasoning_implementation: Some(ReasoningImplementation::RequestParameter),
                default_reasoning_effort: Some(ReasoningEffort::Medium),
                base_instructions: Some("Custom instructions".to_string()),
                input_modalities: Some(vec![InputModality::Text, InputModality::Image]),
                channel: Some("Custom channel".to_string()),
                truncation_policy: Some(TruncationPolicyConfig::tokens(2_048)),
                supports_image_detail_original: Some(true),
            },
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog.get("custom").expect("custom model"),
            &Model {
                slug: "custom".to_string(),
                display_name: "Custom".to_string(),
                provider: ProviderWireApi::OpenAIResponses,
                description: Some("Custom description".to_string()),
                reasoning_capability: ReasoningCapability::Toggle,
                default_reasoning_effort: Some(ReasoningEffort::Medium),
                reasoning_implementation: Some(ReasoningImplementation::RequestParameter),
                base_instructions: "Custom instructions".to_string(),
                context_window: 64_000,
                effective_context_window_percent: Some(75),
                truncation_policy: TruncationPolicyConfig::tokens(2_048),
                input_modalities: vec![InputModality::Text, InputModality::Image],
                supports_image_detail_original: true,
                channel: Some("Custom channel".to_string()),
                temperature: Some(0.2),
                top_p: Some(0.6),
                top_k: Some(12.0),
                max_tokens: Some(4_096),
            }
        );
    }

    #[test]
    fn load_from_config_keeps_explicit_empty_base_instructions() {
        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "custom".to_string(),
            ModelOverrideConfig {
                base_instructions: Some(String::new()),
                ..ModelOverrideConfig::default()
            },
        )]))
        .expect("load catalog");

        assert_eq!(
            catalog
                .get("custom")
                .expect("custom model")
                .base_instructions,
            ""
        );
    }

    #[test]
    fn custom_models_follow_builtins_with_equal_priority() {
        let catalog = PresetModelCatalog::load_from_config(&BTreeMap::from([(
            "custom".to_string(),
            ModelOverrideConfig::default(),
        )]))
        .expect("load catalog");
        let models = catalog.into_inner();

        assert_eq!(models.last().expect("custom model").slug, "custom");
    }

    #[test]
    fn embedded_presets_remain_the_only_catalog_base() {
        assert!(
            !load_builtin_model_presets()
                .expect("load embedded presets")
                .is_empty()
        );
    }
}
