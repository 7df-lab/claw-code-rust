use serde::{Deserialize, Serialize};

/// Enumerates the reasoning effort levels a model may support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningLevel {
    /// Select the cheapest and most lightweight reasoning mode.
    Low,
    /// Select the default balanced reasoning mode.
    Medium,
    /// Select a deeper reasoning mode for more complex tasks.
    High,
    /// Select the most thorough reasoning mode available.
    XHigh,
}

/// Enumerates the verbosity levels a model may support for user-facing output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verbosity {
    /// Request a terse response style.
    Low,
    /// Request a balanced amount of detail.
    Medium,
    /// Request a highly detailed response style.
    High,
}

/// Enumerates the input modalities accepted by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputModality {
    /// The model accepts text input.
    Text,
    /// The model accepts image input.
    Image,
}

/// Controls how a model should be exposed in user-facing selection surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelVisibility {
    /// The model is visible in standard pickers.
    Visible,
    /// The model is hidden from standard pickers.
    Hidden,
    /// The model is exposed only as an experimental option.
    Experimental,
}

/// Describes how prompt items should be truncated before provider submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncationPolicyConfig {
    /// The default character limit for ordinary textual items.
    pub default_max_chars: usize,
    /// The character limit applied specifically to tool output items.
    pub tool_output_max_chars: usize,
    /// The character limit applied to large user inputs when truncation is required.
    pub user_input_max_chars: usize,
    /// The placeholder text used when binary data must be represented in text form.
    pub binary_placeholder: String,
    /// Whether truncated JSON payloads should preserve valid structural shape.
    pub preserve_json_shape: bool,
}

/// Stores the normalized configuration and budgeting metadata for a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The stable unique model slug.
    pub slug: String,
    /// The human-readable display name for the model.
    pub display_name: String,
    /// Optional descriptive text for UI or diagnostics.
    pub description: Option<String>,
    /// The reasoning level used when no explicit override is supplied.
    pub default_reasoning_level: ReasoningLevel,
    /// The complete set of reasoning levels supported by the model.
    pub supported_reasoning_levels: Vec<ReasoningLevel>,
    /// The base instructions inserted before turn-specific prompt material.
    pub base_instructions: String,
    /// The total provider-reported context window size.
    pub context_window: u32,
    /// The percentage of the context window reserved for prompt input.
    pub effective_context_window_percent: u8,
    /// The explicit auto-compaction token threshold, if configured.
    pub auto_compact_token_limit: Option<u32>,
    /// The truncation policy applied before prompt serialization.
    pub truncation_policy: TruncationPolicyConfig,
    /// The set of modalities accepted by the model.
    pub input_modalities: Vec<InputModality>,
    /// Whether the model supports original-detail image inputs.
    pub supports_image_detail_original: bool,
    /// Whether the model is visible to users.
    pub visibility: ModelVisibility,
    /// Whether the model may be selected via the runtime API.
    pub supported_in_api: bool,
    /// The priority used when resolving a default visible model.
    pub priority: i32,
}

/// Provides read-only access to model definitions and turn-resolution behavior.
pub trait ModelCatalog {
    /// Returns the models that should be treated as visible.
    fn list_visible(&self) -> Vec<&ModelConfig>;

    /// Returns a single model by slug when it exists.
    fn get(&self, slug: &str) -> Option<&ModelConfig>;

    /// Resolves the active model for a turn using an explicit slug or default selection.
    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelConfig, ModelConfigError>;
}

/// In-memory `ModelCatalog` implementation used by tests and simple bootstraps.
#[derive(Debug, Clone)]
pub struct InMemoryModelCatalog {
    /// The full set of model definitions stored in memory.
    models: Vec<ModelConfig>,
}

impl InMemoryModelCatalog {
    /// Creates a new in-memory model catalog from normalized model configs.
    pub fn new(models: Vec<ModelConfig>) -> Self {
        Self { models }
    }
}

impl ModelCatalog for InMemoryModelCatalog {
    fn list_visible(&self) -> Vec<&ModelConfig> {
        self.models
            .iter()
            .filter(|model| model.visibility == ModelVisibility::Visible)
            .collect()
    }

    fn get(&self, slug: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|model| model.slug == slug)
    }

    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelConfig, ModelConfigError> {
        if let Some(slug) = requested {
            return self
                .get(slug)
                .ok_or_else(|| ModelConfigError::ModelNotFound {
                    slug: slug.to_string(),
                });
        }

        self.list_visible()
            .into_iter()
            .max_by_key(|model| model.priority)
            .ok_or(ModelConfigError::NoVisibleModels)
    }
}

/// Describes failures that can occur while resolving models from the catalog.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelConfigError {
    /// The requested slug does not exist in the catalog.
    #[error("model not found: {slug}")]
    ModelNotFound { slug: String },
    /// No visible models were available for default selection.
    #[error("no visible models available")]
    NoVisibleModels,
}

#[cfg(test)]
mod tests {
    use super::{
        InMemoryModelCatalog, InputModality, ModelCatalog, ModelConfig, ModelVisibility,
        ReasoningLevel, TruncationPolicyConfig,
    };

    fn model(slug: &str, priority: i32, visibility: ModelVisibility) -> ModelConfig {
        ModelConfig {
            slug: slug.into(),
            display_name: slug.into(),
            description: None,
            default_reasoning_level: ReasoningLevel::Medium,
            supported_reasoning_levels: vec![ReasoningLevel::Medium],
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: 90,
            auto_compact_token_limit: None,
            truncation_policy: TruncationPolicyConfig {
                default_max_chars: 8_000,
                tool_output_max_chars: 16_000,
                user_input_max_chars: 32_000,
                binary_placeholder: "[binary]".into(),
                preserve_json_shape: true,
            },
            input_modalities: vec![InputModality::Text],
            supports_image_detail_original: false,
            visibility,
            supported_in_api: true,
            priority,
        }
    }

    #[test]
    fn resolve_for_turn_uses_highest_priority_visible_default() {
        let catalog = InMemoryModelCatalog::new(vec![
            model("hidden", 100, ModelVisibility::Hidden),
            model("visible-low", 1, ModelVisibility::Visible),
            model("visible-high", 10, ModelVisibility::Visible),
        ]);

        let resolved = catalog.resolve_for_turn(None).expect("resolve default");
        assert_eq!(resolved.slug, "visible-high");
    }

    #[test]
    fn resolve_for_turn_honors_requested_slug() {
        let catalog = InMemoryModelCatalog::new(vec![model("test", 1, ModelVisibility::Visible)]);
        let resolved = catalog
            .resolve_for_turn(Some("test"))
            .expect("resolve explicit");
        assert_eq!(resolved.slug, "test");
    }
}
