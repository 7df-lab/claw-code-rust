//! Builtin model catalog loading and resolution for core.
//!
//! The embedded `providers.json` asset is the canonical provider/model
//! directory. Provider and model identity are resolved from the same map.
use std::collections::BTreeMap;

use crate::{
    InputModality, Model, ModelCatalog, ModelEffortVariant, ModelError, ProviderInfo,
    ProviderModelInfo, ProviderModelVariant, ProviderWireApi, ReasoningCapability,
};
use devo_config::{ModelOverrideConfig, ProviderConfigFile, ProviderModelConfig, model_reference};

const BUILTIN_PROVIDERS_JSON: &str = include_str!("../providers.json");
const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../default_base_instructions.txt");

/// Returns the shared fallback base instructions used when a catalog model
/// omits `base_instructions`, or when a custom model has no instructions.
pub fn default_base_instructions() -> &'static str {
    DEFAULT_BASE_INSTRUCTIONS
}

/// A catalog resolved from embedded presets and configuration overrides.
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<Model>,
    providers: Vec<ProviderInfo>,
    provider_models: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
    builtin_provider_ids: Vec<String>,
}

impl PresetModelCatalog {
    /// Loads the embedded provider/model directory without user overlays.
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config(&ProviderConfigFile::default())
    }

    /// Loads the embedded provider/model directory and overlays user-defined
    /// providers and models on top of it.
    pub fn load_from_provider_config(
        provider_config: &ProviderConfigFile,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config_with_overrides(provider_config, &BTreeMap::new())
    }

    /// Loads the embedded catalog, merges the user provider file, then applies
    /// `[model.<slug>]` overlays onto matching builtin or user models.
    pub fn load_from_provider_config_with_overrides(
        provider_config: &ProviderConfigFile,
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
    ) -> Result<Self, PresetModelCatalogError> {
        let mut directory = load_builtin_provider_config()?;
        let builtin_provider_ids = directory.providers.keys().cloned().collect();
        directory.merge_overlay(provider_config.clone());
        directory.apply_model_overrides(model_overrides);
        let providers = directory
            .providers
            .iter()
            .map(|(provider_id, provider)| provider_info_from_config(provider_id, provider))
            .collect();
        let referenced_models = [directory.model.clone(), directory.small_model.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        for model_ref in referenced_models {
            let Some((provider_id, requested_model_id)) = model_ref.split_once('/') else {
                continue;
            };
            let Some(provider) = directory.providers.get(provider_id) else {
                continue;
            };
            let model_id = if provider.models.contains_key(requested_model_id) {
                requested_model_id
            } else if let Some((model_id, variant_id)) = requested_model_id.rsplit_once('/')
                && provider
                    .models
                    .get(model_id)
                    .is_some_and(|model| model.variants.contains_key(variant_id))
            {
                model_id
            } else {
                requested_model_id
            };
            if let Some(provider) = directory.providers.get_mut(provider_id) {
                provider.models.entry(model_id.to_string()).or_default();
            }
        }

        let mut models = directory
            .providers
            .iter()
            .flat_map(|(provider_id, provider)| {
                let provider_wire_api = provider
                    .wire_api
                    .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
                provider.models.iter().filter_map(move |(model_id, model)| {
                    if provider.enabled == Some(false) || model.enabled == Some(false) {
                        return None;
                    }
                    Some((
                        model.priority.unwrap_or(0),
                        model_from_provider_config(provider_id, model_id, model, provider_wire_api),
                    ))
                })
            })
            .collect::<Vec<_>>();
        let provider_models = directory
            .providers
            .iter()
            .map(|(provider_id, provider)| {
                let provider_wire_api = provider
                    .wire_api
                    .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
                // Include disabled models so settings UIs can show them with
                // enabled=false. Visible turn selection still uses `models`
                // above, which filters disabled entries out.
                let models = provider
                    .models
                    .iter()
                    .map(|(model_id, model)| {
                        (
                            model_id.clone(),
                            provider_model_info_from_config(model, provider_wire_api),
                        )
                    })
                    .collect();
                (provider_id.clone(), models)
            })
            .collect();
        models.sort_by(|left, right| right.0.cmp(&left.0));
        let mut models = models
            .into_iter()
            .map(|(_, model)| model)
            .collect::<Vec<_>>();

        if let Some(default_model) = directory.model.as_deref()
            && let Some(index) = models.iter().position(|model| model.slug == default_model)
        {
            let model = models.remove(index);
            models.insert(0, model);
        }

        Ok(Self {
            models,
            providers,
            provider_models,
            builtin_provider_ids,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<Model>) -> Self {
        Self {
            models,
            providers: Vec::new(),
            provider_models: BTreeMap::new(),
            builtin_provider_ids: Vec::new(),
        }
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

    fn list_providers(&self) -> Vec<ProviderInfo> {
        self.providers.clone()
    }

    fn list_template_provider_ids(&self) -> Vec<String> {
        self.builtin_provider_ids.clone()
    }

    fn list_provider_models(&self, provider_id: &str) -> BTreeMap<String, ProviderModelInfo> {
        self.provider_models
            .get(provider_id)
            .cloned()
            .unwrap_or_default()
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

fn load_builtin_provider_config() -> Result<ProviderConfigFile, PresetModelCatalogError> {
    serde_json::from_str(BUILTIN_PROVIDERS_JSON).map_err(Into::into)
}

fn model_from_provider_config(
    provider_id: &str,
    model_id: &str,
    config: &ProviderModelConfig,
    provider_wire_api: ProviderWireApi,
) -> Model {
    let mut config = config.clone();
    config.migrate_reasoning_implementation_into_variants();
    Model {
        slug: model_reference(provider_id, model_id),
        display_name: config.name.clone().unwrap_or_else(|| model_id.to_string()),
        provider: config.wire_api.unwrap_or(provider_wire_api),
        reasoning_capability: config
            .reasoning_capability
            .clone()
            .unwrap_or(ReasoningCapability::Unsupported),
        default_reasoning_effort: config.default_reasoning_effort,
        default_reasoning_selection: config.default_reasoning_selection.clone(),
        reasoning_implementation: config.reasoning_implementation.clone(),
        catalog_variants: config
            .variants
            .iter()
            .map(|(variant_id, variant)| {
                (
                    variant_id.clone(),
                    ModelEffortVariant {
                        request_model: variant.request_model.clone(),
                        disabled: variant.disabled,
                    },
                )
            })
            .collect(),
        base_instructions: config
            .base_instructions
            .clone()
            .unwrap_or_else(|| default_base_instructions().to_string()),
        context_window: config.context_window.unwrap_or(200_000),
        effective_context_window_percent: config.effective_context_window_percent,
        truncation_policy: config.truncation_policy.unwrap_or_default(),
        input_modalities: config
            .input_modalities
            .clone()
            .unwrap_or_else(|| vec![InputModality::Text]),
        supports_image_detail_original: config.supports_image_detail_original.unwrap_or(false),
        channel: config.channel.clone(),
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        max_tokens: config.max_tokens,
        ..Model::default()
    }
}

fn provider_info_from_config(
    provider_id: &str,
    config: &devo_config::ProviderConfigEntry,
) -> ProviderInfo {
    let wire_api = config
        .wire_api
        .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
    ProviderInfo {
        id: provider_id.to_string(),
        name: config
            .name
            .clone()
            .unwrap_or_else(|| provider_id.to_string()),
        description: config.description.clone(),
        base_url: config.base_url.clone(),
        credential: config.credential.clone(),
        headers: config.headers.clone().unwrap_or_default(),
        options: config.options.clone(),
        request: config.request.clone(),
        wire_apis: vec![wire_api],
        models: BTreeMap::new(),
        enabled: config.enabled.unwrap_or(true),
    }
}

fn provider_model_info_from_config(
    config: &ProviderModelConfig,
    provider_wire_api: ProviderWireApi,
) -> ProviderModelInfo {
    ProviderModelInfo {
        name: config.name.clone(),
        family: config.family.clone(),
        release_date: config.release_date.clone(),
        status: config.status.clone(),
        capabilities: config.capabilities.clone(),
        wire_api: Some(config.wire_api.unwrap_or(provider_wire_api)),
        context_window: config.context_window,
        effective_context_window_percent: config.effective_context_window_percent,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        reasoning_capability: config.reasoning_capability.clone(),
        reasoning_implementation: config.reasoning_implementation.clone(),
        default_reasoning_effort: config.default_reasoning_effort,
        default_reasoning_selection: config.default_reasoning_selection.clone(),
        base_instructions: config.base_instructions.clone(),
        input_modalities: config.input_modalities.clone(),
        channel: config.channel.clone(),
        truncation_policy: config
            .truncation_policy
            .and_then(|policy| serde_json::to_value(policy).ok()),
        supports_image_detail_original: config.supports_image_detail_original,
        web_search: config
            .web_search
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok()),
        web_fetch: config
            .web_fetch
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok()),
        cost: config.cost.clone(),
        metadata: config.metadata.clone(),
        request: config.request.clone(),
        options: config.options.clone(),
        headers: config.headers.clone(),
        variants: config
            .variants
            .iter()
            .map(|(variant_id, variant)| {
                (
                    variant_id.clone(),
                    ProviderModelVariant {
                        label: variant.label.clone(),
                        disabled: variant.disabled,
                        request_model: variant.request_model.clone(),
                        request: variant.request.clone(),
                        options: variant.options.clone(),
                        headers: variant.headers.clone(),
                    },
                )
            })
            .collect(),
        default_variant: config.default_variant.clone(),
        enabled: config.enabled,
        priority: config.priority,
    }
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum PresetModelCatalogError {
    /// Parsing the bundled provider directory failed.
    #[error("failed to parse builtin provider catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::{PresetModelCatalog, default_base_instructions};
    use crate::{Model, ModelCatalog, ProviderInfo, ProviderWireApi};

    #[test]
    fn builtin_models_load_from_provider_directory() {
        let catalog = PresetModelCatalog::load().expect("load provider catalog");
        assert!(!catalog.list_visible().is_empty());
        assert_eq!(catalog.list_visible()[0].slug, "kimi/kimi-k3");
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
        let catalog = PresetModelCatalog::load().expect("load provider catalog");
        assert!(
            catalog
                .list_visible()
                .iter()
                .any(|model| model.channel.as_deref() == Some("DeepSeek"))
        );
    }

    #[test]
    fn provider_catalog_uses_provider_model_references_and_accepts_custom_models() {
        let config = crate::ProviderConfigFile {
            providers: BTreeMap::from([(
                "local".to_string(),
                crate::ProviderConfigEntry {
                    wire_api: Some(ProviderWireApi::OpenAIResponses),
                    models: BTreeMap::from([(
                        "qwen3".to_string(),
                        crate::ProviderModelConfig {
                            name: Some("Qwen 3".to_string()),
                            context_window: Some(131_072),
                            ..crate::ProviderModelConfig::default()
                        },
                    )]),
                    ..crate::ProviderConfigEntry::default()
                },
            )]),
            model: Some("local/qwen3".to_string()),
            ..crate::ProviderConfigFile::default()
        };

        let catalog =
            PresetModelCatalog::load_from_provider_config(&config).expect("load provider catalog");
        assert_eq!(
            catalog
                .resolve_for_turn(None)
                .expect("resolve default")
                .slug,
            "local/qwen3"
        );
        assert_eq!(
            catalog
                .get("local/qwen3")
                .expect("custom model")
                .display_name,
            "Qwen 3"
        );
        assert_eq!(
            catalog.get("local/qwen3").expect("custom model").provider,
            ProviderWireApi::OpenAIResponses
        );
    }

    #[test]
    fn builtin_provider_catalog_contains_current_cloud_and_local_models() {
        let catalog =
            PresetModelCatalog::load_from_provider_config(&crate::ProviderConfigFile::default())
                .expect("load builtin provider catalog");

        assert_eq!(
            catalog
                .resolve_for_turn(None)
                .expect("resolve builtin default")
                .slug,
            "kimi/kimi-k3"
        );
        for model in [
            "deepseek/deepseek-v4-flash-vision-exp",
            "zai/glm-5.3-flash",
            "zhipu/glm-5.3-flash",
            "qwen/qwen3.7-plus",
            "minimax/MiniMax-M3",
            "xiaomi/mimo-v2.5-pro",
            "tencent/hunyuan-a13b",
        ] {
            assert!(
                catalog.get(model).is_some(),
                "missing builtin model {model}"
            );
        }
        assert!(
            catalog
                .list_providers()
                .iter()
                .any(|provider| provider.id == "ollama"),
            "missing builtin ollama provider"
        );
        assert!(
            catalog.list_provider_models("ollama").is_empty(),
            "ollama template must not ship placeholder models"
        );

        let zai_models = catalog
            .list_visible()
            .into_iter()
            .filter(|model| model.slug.starts_with("zai/"))
            .map(|model| model.slug.clone())
            .collect::<Vec<_>>();
        assert_eq!(zai_models, ["zai/glm-5.3", "zai/glm-5.3-flash"]);

        let zhipu_models = catalog
            .list_visible()
            .into_iter()
            .filter(|model| model.slug.starts_with("zhipu/"))
            .map(|model| model.slug.clone())
            .collect::<Vec<_>>();
        assert_eq!(zhipu_models, ["zhipu/glm-5.3", "zhipu/glm-5.3-flash"]);

        assert_eq!(
            catalog
                .get("deepseek/deepseek-v4-flash")
                .expect("deepseek model")
                .provider,
            ProviderWireApi::AnthropicMessages
        );

        let providers = catalog.list_providers();
        assert_eq!(
            providers
                .iter()
                .find(|provider| provider.id == "zhipu")
                .cloned(),
            Some(ProviderInfo {
                id: "zhipu".to_string(),
                name: "Zhipu AI".to_string(),
                description: Some("China BigModel GLM API".to_string()),
                base_url: Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
                credential: None,
                headers: BTreeMap::new(),
                options: None,
                request: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::new(),
                enabled: true,
            })
        );
        assert_eq!(
            providers
                .iter()
                .find(|provider| provider.id == "deepseek")
                .map(|provider| provider.wire_apis.clone()),
            Some(vec![ProviderWireApi::AnthropicMessages])
        );
    }

    #[test]
    fn provider_catalog_materializes_a_minimal_referenced_custom_model() {
        let config = crate::ProviderConfigFile {
            model: Some("local/qwen3".to_string()),
            providers: BTreeMap::from([(
                "local".to_string(),
                crate::ProviderConfigEntry::default(),
            )]),
            ..crate::ProviderConfigFile::default()
        };

        let catalog =
            PresetModelCatalog::load_from_provider_config(&config).expect("load provider catalog");

        assert_eq!(
            catalog.get("local/qwen3").expect("referenced custom model"),
            &Model {
                slug: "local/qwen3".to_string(),
                display_name: "qwen3".to_string(),
                default_reasoning_effort: None,
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            }
        );
    }
}
