use crate::{ModelCatalog, ModelPreset, ModelPresetError};

const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../default_base_instructions.txt");

/// Filesystem-independent loader for the built-in model catalog bundled with the binary.
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<ModelPreset>,
}

impl PresetModelCatalog {
    /// Loads the built-in catalog from `crates/core/models.json`.
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Ok(Self {
            models: load_builtin_models()?,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<ModelPreset>) -> Self {
        Self { models }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<ModelPreset> {
        self.models
    }
}

impl ModelCatalog for PresetModelCatalog {
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

/// Loads the built-in model list bundled with the crate.
pub fn load_builtin_models() -> Result<Vec<ModelPreset>, PresetModelCatalogError> {
    serde_json::from_str(include_str!("../models.json")).map_err(Into::into)
}

/// Returns the shared fallback base instructions used when a model has no catalog entry.
pub fn default_base_instructions() -> &'static str {
    DEFAULT_BASE_INSTRUCTIONS
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
    use pretty_assertions::assert_eq;

    use super::{PresetModelCatalog, default_base_instructions, load_builtin_models};
    use crate::ModelCatalog;

    #[test]
    fn builtin_models_load_from_bundled_json() {
        let models = load_builtin_models().expect("load builtin models");
        assert!(!models.is_empty());
        assert_eq!(models[0].slug, "qwen3-coder-next");
        assert!(!models[0].base_instructions.is_empty());
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
}
