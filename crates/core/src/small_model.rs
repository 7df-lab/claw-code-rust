//! Selection of a lightweight model for background tasks.
//!
//! The policy mirrors the useful part of OpenCode's model split: an explicit
//! `small_model` wins, while the automatic path only considers models exposed
//! by the same provider as the primary model. The catalog metadata and model
//! id are both considered so custom provider directories work without a new
//! protocol field.

use crate::{ModelCatalog, ProviderModelInfo};

const SMALL_MODEL_SCORE_THRESHOLD: i32 = 30;

/// Finds a suitable lightweight model for `primary_model` in the same provider.
///
/// This is an automatic fallback used by background tasks such as session-title
/// generation. It returns a canonical `provider/model` reference and returns
/// `None` when the provider has no model that is recognizably lightweight.
pub fn resolve_small_model(catalog: &dyn ModelCatalog, primary_model: &str) -> Option<String> {
    let (provider_id, requested_model_id) = primary_model.split_once('/')?;
    let provider_models = catalog.list_provider_models(provider_id);
    let primary_model_id = base_model_id(&provider_models, requested_model_id);
    let mut best: Option<(String, i32, i32)> = None;

    for (model_id, model) in provider_models {
        if model_id == primary_model_id || model_is_unavailable(&model) {
            continue;
        }
        let score = small_model_score(&model_id, &model);
        if score < SMALL_MODEL_SCORE_THRESHOLD {
            continue;
        }

        let priority = model.priority.unwrap_or_default();
        let should_replace = best.as_ref().is_none_or(|(_, best_score, best_priority)| {
            score > *best_score || (score == *best_score && priority > *best_priority)
        });
        if should_replace {
            best = Some((format!("{provider_id}/{model_id}"), score, priority));
        }
    }

    best.map(|(model, _, _)| model)
}

fn base_model_id<'a>(
    provider_models: &std::collections::BTreeMap<String, ProviderModelInfo>,
    requested_model_id: &'a str,
) -> &'a str {
    if provider_models.contains_key(requested_model_id) {
        return requested_model_id;
    }

    requested_model_id
        .rsplit_once('/')
        .filter(|(model_id, variant_id)| {
            provider_models
                .get(*model_id)
                .is_some_and(|model| model.variants.contains_key(*variant_id))
        })
        .map(|(model_id, _)| model_id)
        .unwrap_or(requested_model_id)
}

fn model_is_unavailable(model: &ProviderModelInfo) -> bool {
    model.enabled == Some(false)
        || model.status.as_deref().is_some_and(|status| {
            matches!(
                status.trim().to_ascii_lowercase().as_str(),
                "disabled" | "deprecated" | "unavailable" | "offline"
            )
        })
}

fn small_model_score(model_id: &str, model: &ProviderModelInfo) -> i32 {
    let model_id = model_id.to_ascii_lowercase();
    let family = model
        .family
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut score = 0;

    if family.contains("flash")
        || family.contains("nano")
        || family.contains("haiku")
        || family.contains("mini")
    {
        score += 100;
    }
    if model_id.contains("flash")
        || model_id.contains("nano")
        || model_id.contains("haiku")
        || model_id.contains("small")
        || model_id.contains("lite")
        || model_id.contains("fast")
        || model_id.contains("instant")
    {
        score += 90;
    }
    if model_id.contains("mini") && !model_id.contains("minimax") {
        score += 70;
    }
    if model_id.contains("a3b") {
        score += 60;
    }
    if ["3b", "4b", "7b", "8b", "1b", "2b"]
        .iter()
        .any(|size| model_id.contains(size))
    {
        score += 35;
    }
    if model_id.contains("pro")
        || model_id.contains("max")
        || model_id.contains("ultra")
        || model_id.contains("opus")
    {
        score -= 40;
    }

    score
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::resolve_small_model;
    use crate::{InMemoryModelCatalog, Model, ProviderModelVariantConfig, ProviderWireApi};

    fn model(slug: &str) -> Model {
        Model {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            ..Model::default()
        }
    }

    #[test]
    fn selects_a_lightweight_model_from_the_primary_provider() {
        let catalog = InMemoryModelCatalog::new(vec![
            model("qwen/qwen3.8-max"),
            model("qwen/qwen3.8-flash"),
            model("qwen/qwen3:4b"),
        ]);

        assert_eq!(
            resolve_small_model(&catalog, "qwen/qwen3.8-max"),
            Some("qwen/qwen3.8-flash".to_string())
        );
    }

    #[test]
    fn does_not_cross_provider_boundaries_or_select_the_primary_model() {
        let catalog = InMemoryModelCatalog::new(vec![
            model("openai/gpt-5.5"),
            model("openai/gpt-5.5-mini"),
            model("ollama/qwen3:4b"),
        ]);

        assert_eq!(resolve_small_model(&catalog, "openai/gpt-5.5-mini"), None);
    }

    #[test]
    fn recognizes_variants_when_excluding_the_primary_model() {
        let catalog =
            crate::PresetModelCatalog::load_from_provider_config(&crate::ProviderConfigFile {
                providers: std::collections::BTreeMap::from([(
                    "deepseek".to_string(),
                    crate::ProviderConfigEntry {
                        models: std::collections::BTreeMap::from([(
                            "deepseek-v4-flash".to_string(),
                            crate::ProviderModelConfig {
                                variants: std::collections::BTreeMap::from([(
                                    "reasoning".to_string(),
                                    ProviderModelVariantConfig::default(),
                                )]),
                                ..crate::ProviderModelConfig::default()
                            },
                        )]),
                        ..crate::ProviderConfigEntry::default()
                    },
                )]),
                ..crate::ProviderConfigFile::default()
            })
            .expect("load provider catalog");

        assert_eq!(
            resolve_small_model(&catalog, "deepseek/deepseek-v4-flash/reasoning"),
            Some("deepseek/deepseek-v4-flash-vision-exp".to_string())
        );
    }
}
