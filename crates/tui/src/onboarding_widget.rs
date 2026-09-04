//! Standalone onboarding widget for first-run model setup.
//!
//! This widget owns the onboarding flow and renders it in the TUI alternate
//! screen. It handles all keyboard input directly during onboarding, and is
//! owned by `ChatWidget` — not by `BottomPane` — keeping it decoupled from the
//! composer and popup system.
//!
//! Follows L2-DES-TUI-001 flow:
//! 1. Provider selection (existing or custom)
//! 2. Model selection (provider catalog or custom)
//! 3. Model settings (basic fields plus expandable advanced overrides)
//! 4. Review and confirmation
//! 5. Validation

use std::collections::BTreeMap;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use devo_protocol::Model;
use devo_protocol::ProviderInfo;
use devo_protocol::ProviderModelInfo;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningEffortOption;
use devo_protocol::ReasoningImplementation;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::exec_cell::spinner;
use crate::onboarding_viewport::ViewportAnchor;
use crate::onboarding_viewport::render_lines_with_anchor;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;
use crate::ui_consts::FOOTER_INDENT_COLS;

const SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);
/// Left inset for onboarding list rows, matching `/model` / composer gutters.
const LIST_LEFT_PAD: usize = FOOTER_INDENT_COLS;
const VALIDATION_FAILED_ACTIONS: [&str; 4] = [
    "Add model anyway",
    "Retry with current settings",
    "Edit settings",
    "Choose different model",
];

/// Simple content area with padding, no background styling.
fn onboarding_content_area(area: Rect) -> Rect {
    if area.height < 2 || area.width < 2 {
        return area;
    }
    let padding = u16::from(area.height >= 12);
    Rect {
        x: area.x + padding,
        y: area.y + padding,
        width: area.width.saturating_sub(padding * 2),
        height: area.height.saturating_sub(padding * 2),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OnboardingResult {
    /// Validation succeeded, config should be saved.
    ValidationSucceeded {
        model_slug: String,
        request_model: String,
        display_name: String,
    },
    /// Validation failed, but the user chose to save the binding anyway.
    ValidationBypassed {
        model_slug: String,
        request_model: String,
        display_name: String,
    },
    /// User cancelled onboarding.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OnboardingTranscriptEvent {
    ModelSelected {
        model_slug: String,
        display_name: String,
    },
    ProviderSelected {
        provider_name: String,
        base_url: Option<String>,
        credential_summary: String,
    },
    SettingsConfirmed {
        provider_name: String,
        base_url: Option<String>,
        request_model: String,
        display_name: String,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: Option<String>,
        credential_summary: String,
    },
}

/// Which field is active in the inline setup view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineField {
    ProviderName,
    BaseUrl,
    ApiKey,
    RequestModel,
    DisplayName,
}

/// Fields shown when a user adds a model that is not in the provider catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomModelField {
    ModelId,
    DisplayName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionFocus {
    List,
    Custom,
}

#[derive(Debug, Clone)]
struct ProviderDraft {
    provider: ProviderWireApi,
    provider_id: String,
    provider_name: String,
    provider_credential_id: Option<String>,
    base_url: String,
    api_key: String,
    is_custom: bool,
}

impl Default for ProviderDraft {
    fn default() -> Self {
        Self {
            provider: ProviderWireApi::OpenAIChatCompletions,
            provider_id: String::new(),
            provider_name: String::new(),
            provider_credential_id: None,
            base_url: String::new(),
            api_key: String::new(),
            is_custom: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ModelSettingsDraft {
    display_name: String,
    /// Absolute usable tokens shown in the Context window field.
    context_window: String,
    /// Hard model capacity used to convert usable tokens → percent on save.
    context_window_hard: String,
    max_tokens: String,
    temperature: String,
    input_modalities: String,
    reasoning_capability: String,
    reasoning_levels: String,
    effective_context_window_percent: String,
    top_p: String,
    top_k: String,
    family: String,
    release_date: String,
    status: String,
    capabilities_json: String,
    channel: String,
    base_instructions: String,
    reasoning_implementation: String,
    reasoning_variants_json: String,
    default_variant: String,
    cost_json: String,
    metadata_json: String,
    request_json: String,
    options_json: String,
    headers_json: String,
    variants_json: String,
    web_search_json: String,
    web_fetch_json: String,
    truncation_mode: String,
    truncation_limit: String,
    supports_image_detail_original: Option<bool>,
    enabled: Option<bool>,
    priority: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelSettingsField {
    DisplayName,
    ContextWindow,
    MaxTokens,
    Temperature,
    InputModalities,
    ReasoningCapability,
    ReasoningLevels,
    DefaultReasoning,
    AdvancedToggle,
    EffectiveContext,
    TopP,
    TopK,
    Family,
    ReleaseDate,
    Status,
    CapabilitiesJson,
    Channel,
    BaseInstructions,
    ReasoningImplementation,
    ReasoningVariantsJson,
    DefaultVariant,
    CostJson,
    MetadataJson,
    RequestJson,
    OptionsJson,
    HeadersJson,
    VariantsJson,
    WebSearchJson,
    WebFetchJson,
    TruncationMode,
    TruncationLimit,
    OriginalImageDetail,
    Enabled,
    Priority,
}

impl ModelSettingsDraft {
    fn from_value(value: Option<&serde_json::Value>, display_name: &str) -> Self {
        let Some(object) = value.and_then(serde_json::Value::as_object) else {
            return Self {
                display_name: display_name.to_string(),
                ..Self::default()
            };
        };
        let string_value = |name: &str| {
            object
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let json_value = |name: &str| {
            object
                .get(name)
                .map(|value| serde_json::to_string(value).unwrap_or_default())
                .unwrap_or_default()
        };
        let number_value = |name: &str| {
            object
                .get(name)
                .map(ToString::to_string)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string()
        };
        let truncation = object
            .get("truncation_policy")
            .and_then(serde_json::Value::as_object);
        let (reasoning_capability, reasoning_levels) = object
            .get("reasoning_capability")
            .map(Self::reasoning_capability_fields)
            .unwrap_or_default();
        let (reasoning_implementation, reasoning_variants_json) = object
            .get("reasoning_implementation")
            .map(Self::reasoning_implementation_fields)
            .unwrap_or_default();
        Self {
            display_name: object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(display_name)
                .to_string(),
            context_window: {
                let raw = number_value("context_window");
                match raw.trim().parse::<u32>() {
                    Ok(window) if !raw.trim().is_empty() => {
                        let percent = number_value("effective_context_window_percent")
                            .trim()
                            .parse::<f64>()
                            .unwrap_or(95.0)
                            .clamp(0.0, 100.0);
                        ((f64::from(window) * percent) / 100.0).floor().to_string()
                    }
                    _ => raw,
                }
            },
            context_window_hard: number_value("context_window"),
            max_tokens: number_value("max_tokens"),
            temperature: number_value("temperature"),
            input_modalities: object
                .get("input_modalities")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            reasoning_capability,
            reasoning_levels,
            // Percent is derived on save from usable absolute ÷ hard window.
            effective_context_window_percent: String::new(),
            top_p: number_value("top_p"),
            top_k: number_value("top_k"),
            family: string_value("family"),
            release_date: string_value("release_date"),
            status: string_value("status"),
            capabilities_json: json_value("capabilities"),
            channel: string_value("channel"),
            base_instructions: string_value("base_instructions"),
            reasoning_implementation,
            reasoning_variants_json,
            default_variant: string_value("default_variant"),
            cost_json: json_value("cost"),
            metadata_json: json_value("metadata"),
            request_json: json_value("request"),
            options_json: json_value("options"),
            headers_json: json_value("headers"),
            variants_json: json_value("variants"),
            web_search_json: json_value("web_search"),
            web_fetch_json: json_value("web_fetch"),
            truncation_mode: truncation
                .and_then(|value| value.get("mode"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            truncation_limit: truncation
                .and_then(|value| value.get("limit"))
                .map(ToString::to_string)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
            supports_image_detail_original: object
                .get("supports_image_detail_original")
                .and_then(serde_json::Value::as_bool),
            enabled: object.get("enabled").and_then(serde_json::Value::as_bool),
            priority: number_value("priority"),
        }
    }

    fn reasoning_capability_fields(value: &serde_json::Value) -> (String, String) {
        match serde_json::from_value::<ReasoningCapability>(value.clone()) {
            Ok(ReasoningCapability::Unsupported) => ("unsupported".to_string(), String::new()),
            Ok(ReasoningCapability::Toggle) => ("toggle".to_string(), String::new()),
            Ok(ReasoningCapability::Levels(levels)) => (
                "levels".to_string(),
                levels
                    .iter()
                    .map(|level| level.selection_value())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Err(_) => (
                serde_json::to_string(value).unwrap_or_default(),
                String::new(),
            ),
        }
    }

    fn reasoning_implementation_fields(value: &serde_json::Value) -> (String, String) {
        match serde_json::from_value::<ReasoningImplementation>(value.clone()) {
            Ok(ReasoningImplementation::Disabled) => ("disabled".to_string(), String::new()),
            Ok(ReasoningImplementation::RequestParameter) => {
                ("request_parameter".to_string(), String::new())
            }
            Ok(ReasoningImplementation::ModelVariant(config)) => (
                "model_variant".to_string(),
                serde_json::to_string(&config.variants).unwrap_or_default(),
            ),
            Err(_) => (
                serde_json::to_string(value).unwrap_or_default(),
                String::new(),
            ),
        }
    }

    fn insert_json_field(
        object: &mut serde_json::Map<String, serde_json::Value>,
        name: &str,
        input: &str,
    ) {
        if let Ok(value) = serde_json::from_str(input.trim()) {
            object.insert(name.to_string(), value);
        }
    }

    fn to_value(&self, model_id: &str) -> Option<serde_json::Value> {
        let mut object = serde_json::Map::new();
        let display_name = self.display_name.trim();
        if !display_name.is_empty() && display_name != model_id {
            object.insert(
                "name".to_string(),
                serde_json::Value::String(display_name.to_string()),
            );
        }
        macro_rules! insert_number {
            ($field:ident, $name:literal, $ty:ty) => {
                if let Ok(value) = self.$field.trim().parse::<$ty>() {
                    object.insert($name.to_string(), serde_json::json!(value));
                }
            };
        }
        let usable = self.context_window.trim().parse::<u64>().ok();
        let hard = self
            .context_window_hard
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0);
        match (usable, hard) {
            (Some(user_tokens), Some(hard_window)) => {
                object.insert("context_window".to_string(), serde_json::json!(hard_window));
                let percent =
                    ((user_tokens as f64) * 100.0 / (hard_window as f64)).clamp(1.0, 100.0);
                object.insert(
                    "effective_context_window_percent".to_string(),
                    serde_json::json!(percent),
                );
            }
            (Some(user_tokens), None) => {
                // Custom model with no hard window yet.
                object.insert("context_window".to_string(), serde_json::json!(user_tokens));
                object.insert(
                    "effective_context_window_percent".to_string(),
                    serde_json::json!(100.0),
                );
            }
            (None, Some(hard_window)) => {
                // Cleared usable field: keep hard, omit percent → default 95%.
                object.insert("context_window".to_string(), serde_json::json!(hard_window));
            }
            (None, None) => {}
        }
        insert_number!(max_tokens, "max_tokens", u32);
        insert_number!(temperature, "temperature", f64);
        insert_number!(top_p, "top_p", f64);
        insert_number!(top_k, "top_k", f64);
        insert_number!(priority, "priority", i32);
        for (field, name) in [
            (&self.family, "family"),
            (&self.release_date, "release_date"),
            (&self.status, "status"),
            (&self.channel, "channel"),
            (&self.base_instructions, "base_instructions"),
            (&self.default_variant, "default_variant"),
        ] {
            if !field.trim().is_empty() {
                object.insert(
                    name.to_string(),
                    serde_json::Value::String(field.trim().to_string()),
                );
            }
        }
        let modalities = self
            .input_modalities
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .filter(|value| matches!(*value, "text" | "image"))
            .map(|value| serde_json::Value::String(value.to_string()))
            .collect::<Vec<_>>();
        if !modalities.is_empty() {
            object.insert(
                "input_modalities".to_string(),
                serde_json::Value::Array(modalities),
            );
        }
        if !self.reasoning_capability.trim().is_empty() {
            let capability = self.reasoning_capability.trim().to_ascii_lowercase();
            let levels = self
                .reasoning_levels
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .filter_map(|value| {
                    serde_json::from_value::<devo_protocol::ReasoningLevelChoice>(
                        serde_json::Value::String(value.to_string()),
                    )
                    .ok()
                })
                .collect::<Vec<_>>();
            let capability = match capability.as_str() {
                "unsupported" => Some(ReasoningCapability::Unsupported),
                "toggle" => Some(ReasoningCapability::Toggle),
                "levels" | "toggle_with_levels" if !levels.is_empty() => {
                    let choices = if capability == "toggle_with_levels"
                        && !levels.iter().any(|choice| {
                            matches!(choice, devo_protocol::ReasoningLevelChoice::Off)
                        }) {
                        let mut choices = vec![devo_protocol::ReasoningLevelChoice::Off];
                        choices.extend(levels);
                        choices
                    } else {
                        levels
                    };
                    Some(ReasoningCapability::Levels(choices))
                }
                _ => None,
            };
            if let Some(capability) = capability {
                object.insert(
                    "reasoning_capability".to_string(),
                    serde_json::to_value(capability).expect("reasoning capability serializes"),
                );
            }
        }
        if !self.reasoning_implementation.trim().is_empty() {
            let implementation = match self
                .reasoning_implementation
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "disabled" => Some(ReasoningImplementation::Disabled),
                "request_parameter" => Some(ReasoningImplementation::RequestParameter),
                "model_variant" => serde_json::from_str::<Vec<devo_protocol::ReasoningVariant>>(
                    self.reasoning_variants_json.trim(),
                )
                .ok()
                .map(|variants| {
                    ReasoningImplementation::ModelVariant(devo_protocol::ReasoningVariantConfig {
                        variants,
                    })
                }),
                _ => None,
            };
            if let Some(implementation) = implementation {
                object.insert(
                    "reasoning_implementation".to_string(),
                    serde_json::to_value(implementation)
                        .expect("reasoning implementation serializes"),
                );
            }
        }
        if let Some(value) = self.supports_image_detail_original {
            object.insert(
                "supports_image_detail_original".to_string(),
                serde_json::Value::Bool(value),
            );
        }
        if let Some(value) = self.enabled {
            object.insert("enabled".to_string(), serde_json::Value::Bool(value));
        }
        if let Ok(limit) = self.truncation_limit.trim().parse::<i64>() {
            let mode = match self.truncation_mode.trim() {
                "tokens" => "tokens",
                _ => "bytes",
            };
            object.insert(
                "truncation_policy".to_string(),
                serde_json::json!({"mode": mode, "limit": limit}),
            );
        }
        for (field, name) in [
            (&self.cost_json, "cost"),
            (&self.metadata_json, "metadata"),
            (&self.request_json, "request"),
            (&self.options_json, "options"),
            (&self.headers_json, "headers"),
            (&self.variants_json, "variants"),
            (&self.web_search_json, "web_search"),
            (&self.web_fetch_json, "web_fetch"),
            (&self.capabilities_json, "capabilities"),
        ] {
            Self::insert_json_field(&mut object, name, field);
        }
        (!object.is_empty()).then_some(serde_json::Value::Object(object))
    }

    fn validation_error(&self, model_id: &str) -> Option<String> {
        macro_rules! validate_number {
            ($field:ident, $type:ty, $label:literal) => {
                if !self.$field.trim().is_empty() && self.$field.trim().parse::<$type>().is_err() {
                    return Some(format!("{} must be a valid {}", $label, stringify!($type)));
                }
            };
        }
        validate_number!(context_window, u32, "Context window");
        validate_number!(max_tokens, u32, "Max output tokens");
        validate_number!(temperature, f64, "Temperature");
        validate_number!(top_p, f64, "Top P");
        validate_number!(top_k, f64, "Top K");
        validate_number!(priority, i32, "Priority");
        if self
            .context_window
            .trim()
            .parse::<u32>()
            .is_ok_and(|value| value == 0)
        {
            return Some("Context window must be greater than 0".to_string());
        }
        if self
            .max_tokens
            .trim()
            .parse::<u32>()
            .is_ok_and(|value| value == 0)
        {
            return Some("Max output tokens must be greater than 0".to_string());
        }
        if self
            .temperature
            .trim()
            .parse::<f64>()
            .is_ok_and(|value| !value.is_finite() || value < 0.0)
        {
            return Some(
                "Temperature must be a finite number greater than or equal to 0".to_string(),
            );
        }
        if self
            .top_p
            .trim()
            .parse::<f64>()
            .is_ok_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Some("Top P must be between 0 and 1".to_string());
        }
        if self
            .top_k
            .trim()
            .parse::<f64>()
            .is_ok_and(|value| !value.is_finite() || value < 0.0)
        {
            return Some("Top K must be a finite number greater than or equal to 0".to_string());
        }
        if !self.input_modalities.trim().is_empty()
            && self
                .input_modalities
                .split(',')
                .map(str::trim)
                .any(|value| !matches!(value, "text" | "image"))
        {
            return Some("Input modalities must contain only text and image".to_string());
        }
        if !self.truncation_mode.trim().is_empty()
            && !matches!(self.truncation_mode.trim(), "bytes" | "tokens")
        {
            return Some("Truncation mode must be bytes or tokens".to_string());
        }
        validate_number!(truncation_limit, i64, "Truncation limit");
        if self
            .truncation_limit
            .trim()
            .parse::<i64>()
            .is_ok_and(|value| value <= 0)
        {
            return Some("Truncation limit must be greater than 0".to_string());
        }
        if let Some(error) = Self::validate_json_fields([
            ("Cost", &self.cost_json),
            ("Metadata", &self.metadata_json),
            ("Capabilities", &self.capabilities_json),
            ("Request", &self.request_json),
            ("Options", &self.options_json),
            ("Headers", &self.headers_json),
            ("Variants", &self.variants_json),
            ("Reasoning variants", &self.reasoning_variants_json),
            ("Web search", &self.web_search_json),
            ("Web fetch", &self.web_fetch_json),
        ]) {
            return Some(error);
        }
        if let Some(error) = Self::validate_json_objects([
            ("Metadata", &self.metadata_json),
            ("Capabilities", &self.capabilities_json),
            ("Request", &self.request_json),
            ("Options", &self.options_json),
            ("Headers", &self.headers_json),
            ("Variants", &self.variants_json),
            ("Web search", &self.web_search_json),
            ("Web fetch", &self.web_fetch_json),
        ]) {
            return Some(error);
        }
        if !self.headers_json.trim().is_empty()
            && serde_json::from_str::<BTreeMap<String, String>>(&self.headers_json).is_err()
        {
            return Some("Headers JSON must be an object whose values are strings".to_string());
        }
        if !self.variants_json.trim().is_empty()
            && serde_json::from_str::<BTreeMap<String, devo_core::ProviderModelVariantConfig>>(
                &self.variants_json,
            )
            .is_err()
        {
            return Some(
                "Variants JSON must map effort keys (off/on/levels) to label, disabled, request_model, request, options, or headers"
                    .to_string(),
            );
        }
        if let Some(error) = Self::validate_reasoning(self) {
            return Some(error);
        }
        self.to_value(model_id).and_then(|value| {
            serde_json::from_value::<devo_core::ProviderModelConfig>(value)
                .err()
                .map(|error| format!("Model settings are invalid: {error}"))
        })
    }

    fn validate_json_fields<const N: usize>(fields: [(&str, &String); N]) -> Option<String> {
        for (label, input) in fields {
            if !input.trim().is_empty() && serde_json::from_str::<serde_json::Value>(input).is_err()
            {
                return Some(format!("{label} JSON must be valid JSON"));
            }
        }
        None
    }

    fn validate_json_objects<const N: usize>(fields: [(&str, &String); N]) -> Option<String> {
        for (label, input) in fields {
            if input.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(input) else {
                continue;
            };
            if !value.is_object() {
                return Some(format!("{label} JSON must be an object"));
            }
        }
        None
    }

    fn validate_reasoning(&self) -> Option<String> {
        let capability = self.reasoning_capability.trim().to_ascii_lowercase();
        if !capability.is_empty() {
            let levels = self
                .reasoning_levels
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if !matches!(capability.as_str(), "unsupported" | "toggle" | "levels") {
                return Some(
                    "Reasoning capability must be unsupported, toggle, or levels".to_string(),
                );
            }
            if !matches!(capability.as_str(), "unsupported" | "toggle") && levels.is_empty() {
                return Some("Reasoning levels are required for levels mode".to_string());
            }
            if levels.iter().any(|value| {
                serde_json::from_value::<devo_protocol::ReasoningLevelChoice>(
                    serde_json::Value::String((*value).to_string()),
                )
                .is_err()
            }) {
                return Some(
                    "Reasoning levels must be comma-separated: off, none, minimal, low, medium, high, xhigh, or max"
                        .to_string(),
                );
            }
        }
        let implementation = self.reasoning_implementation.trim().to_ascii_lowercase();
        if !implementation.is_empty()
            && !matches!(
                implementation.as_str(),
                "disabled" | "request_parameter" | "model_variant"
            )
        {
            return Some(
                "Reasoning implementation must be disabled, request_parameter, or model_variant"
                    .to_string(),
            );
        }
        if implementation == "model_variant" && self.reasoning_variants_json.trim().is_empty() {
            return Some("Reasoning variant rules must be a valid JSON array".to_string());
        }
        if !self.reasoning_variants_json.trim().is_empty() {
            if implementation != "model_variant" {
                return Some(
                    "Reasoning variant rules require model_variant implementation".to_string(),
                );
            }
            if serde_json::from_str::<Vec<devo_protocol::ReasoningVariant>>(
                self.reasoning_variants_json.trim(),
            )
            .is_err()
            {
                return Some("Reasoning variant rules must be a valid JSON array".to_string());
            }
        }
        None
    }
}

/// Onboarding state machine following L2-DES-TUI-001.
#[derive(Debug)]
enum OnboardingState {
    /// Step 2: Select a model from the selected provider or enter custom.
    ModelSelection {
        provider: ProviderDraft,
        items: Vec<ModelSelectionItem>,
        state: ScrollState,
        search_query: String,
        filtered_indices: Vec<usize>,
        focus: SelectionFocus,
        manage_connection: bool,
    },
    /// Step 2b: Define a custom model that is not in the provider catalog.
    CustomModelForm {
        provider: ProviderDraft,
        model_id: String,
        display_name: String,
        active_field: CustomModelField,
        input: String,
        cursor_pos: usize,
        manage_connection: bool,
    },
    /// Step 1: Select an existing provider or add one.
    ProviderSelection {
        items: Vec<ProviderSelectionItem>,
        selected_idx: usize,
        focus: SelectionFocus,
    },
    /// Step 1b: Enter a custom provider's connection details.
    ProviderSetup {
        draft: ProviderDraft,
        active_field: InlineField,
        input: String,
        cursor_pos: usize,
    },
    /// Confirm disconnecting an existing provider Connection.
    DisconnectConfirmation { provider: ProviderInfo },
    /// Waiting for the server to remove a provider Connection.
    Disconnecting { provider_name: String },
    /// Confirm removing one model from a provider Connection.
    ModelDeleteConfirmation {
        provider: ProviderDraft,
        model_id: String,
        model_name: String,
    },
    /// Waiting for the server to remove one provider Connection model.
    ModelDeleting {
        provider: ProviderDraft,
        model_name: String,
    },
    /// Step 3: Configure basic and optional advanced model settings.
    ModelSettings {
        model: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: Option<String>,
        settings: Box<ModelSettingsDraft>,
        advanced_open: bool,
        active_field: ModelSettingsField,
        input: String,
        cursor_pos: usize,
        settings_error: Option<String>,
    },
    /// Final confirmation before the server probes and persists the binding.
    Review { params: ValidationParams },
    /// Steps 3-8: Inline setup for provider vendor and model binding fields.
    InlineSetup {
        model: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        active_field: InlineField,
        input: String,
        cursor_pos: usize,
    },
    /// Step 9: Select provider wire API type.
    InvocationMethod {
        model: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        items: Vec<InvocationMethodItem>,
        selected_idx: usize,
        initial_model_settings: Option<serde_json::Value>,
        default_reasoning_effort: Option<String>,
    },
    /// Step 10: Select reasoning effort.
    ReasoningEffort {
        model: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        invocation_method: ProviderWireApi,
        items: Vec<ReasoningEffortItem>,
        selected_idx: usize,
        initial_model_settings: Option<serde_json::Value>,
        default_reasoning_effort: Option<String>,
    },
    /// Validating connection.
    Validating {
        model_slug: String,
        request_model: String,
        display_name: String,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: Option<String>,
        model_settings: Option<serde_json::Value>,
        base_url: Option<String>,
        api_key: Option<String>,
        started_at: Instant,
    },
    /// Saving provider and model binding after validation or explicit bypass.
    Saving {
        model_slug: String,
        request_model: String,
        display_name: String,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: Option<String>,
        model_settings: Option<serde_json::Value>,
        base_url: Option<String>,
        api_key: Option<String>,
        bypassed: bool,
        started_at: Instant,
    },
    /// Validation failed, show error and retry options.
    ValidationFailed {
        model: String,
        request_model: String,
        display_name: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        default_reasoning_effort: Option<String>,
        model_settings: Option<serde_json::Value>,
        base_url: Option<String>,
        api_key: Option<String>,
        error_message: String,
        recovery_hint: Option<String>,
        selected_action: usize,
    },
}

#[derive(Debug)]
struct ModelSelectionItem {
    slug: String,
    model_id: String,
    display_name: String,
    is_custom: bool,
    /// Saved model metadata used to prefill the editor when a Connection
    /// model is opened again. Built-in directory rows do not need this copy.
    initial_settings: Option<serde_json::Value>,
    wire_api: Option<ProviderWireApi>,
    default_reasoning_effort: Option<String>,
}

#[derive(Debug)]
struct ProviderSelectionItem {
    label: String,
    description: String,
    provider: ProviderInfo,
    section: ProviderSelectionSection,
    is_custom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderSelectionSection {
    Connections,
    Templates,
}

#[derive(Debug)]
struct InvocationMethodItem {
    label: String,
    description: String,
    provider: ProviderWireApi,
}

#[derive(Debug)]
struct ReasoningEffortItem {
    label: String,
    value: String,
    description: String,
}

pub(crate) struct OnboardingWidget {
    state: OnboardingState,
    complete: bool,
    result: Option<OnboardingResult>,
    /// Models from the catalog, stored so the provider/model screens can rebuild their lists.
    original_models: Vec<Model>,
    providers: Vec<ProviderInfo>,
    template_provider_ids: Vec<String>,
    connected_provider_ids: Vec<String>,
    connection_models: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
    provider_status_known: bool,
    transcript_events: Vec<OnboardingTranscriptEvent>,
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
}

impl OnboardingWidget {
    pub(crate) fn new(
        models: &[Model],
        app_event_tx: AppEventSender,
        frame_requester: FrameRequester,
        animations_enabled: bool,
    ) -> Self {
        let items = Self::provider_selection_items(&[], &[], &[], false);

        let this = Self {
            state: OnboardingState::ProviderSelection {
                items,
                selected_idx: 0,
                focus: SelectionFocus::Custom,
            },
            complete: false,
            result: None,
            original_models: models.to_vec(),
            providers: Vec::new(),
            template_provider_ids: Vec::new(),
            connected_provider_ids: Vec::new(),
            connection_models: BTreeMap::new(),
            provider_status_known: false,
            transcript_events: Vec::new(),
            app_event_tx,
            frame_requester,
            animations_enabled,
        };
        this.app_event_tx
            .send(AppEvent::Command(AppCommand::ProviderList));
        this
    }

    /// Build the model list for the selected provider and keep the custom entry visible.
    fn build_model_items(models: &[Model], provider_id: &str) -> Vec<ModelSelectionItem> {
        let provider_prefix = format!("{provider_id}/");
        let mut items = models
            .iter()
            .filter(|model| provider_id.is_empty() || model.slug.starts_with(&provider_prefix))
            .map(|m| ModelSelectionItem {
                slug: m.slug.clone(),
                model_id: m
                    .slug
                    .strip_prefix(&provider_prefix)
                    .unwrap_or(&m.slug)
                    .to_string(),
                display_name: m.display_name.clone(),
                is_custom: false,
                initial_settings: None,
                wire_api: None,
                default_reasoning_effort: None,
            })
            .collect::<Vec<_>>();
        if items.is_empty() && !provider_id.is_empty() {
            // Test fixtures and older catalog adapters may still expose
            // provider models without the provider prefix. Keep those usable
            // while prefixed catalog entries remain the canonical form.
            items = models
                .iter()
                .filter(|model| !model.slug.contains('/'))
                .map(|m| ModelSelectionItem {
                    slug: m.slug.clone(),
                    model_id: m.slug.clone(),
                    display_name: m.display_name.clone(),
                    is_custom: false,
                    initial_settings: None,
                    wire_api: None,
                    default_reasoning_effort: None,
                })
                .collect();
        }
        items.push(ModelSelectionItem {
            slug: String::new(),
            model_id: String::new(),
            display_name: "Add custom model profile".to_string(),
            is_custom: true,
            initial_settings: None,
            wire_api: None,
            default_reasoning_effort: None,
        });
        items
    }

    /// Builds the models explicitly saved on a Connection. Template models
    /// are intentionally not mixed into this list.
    fn build_connection_model_items(
        provider_id: &str,
        models: &BTreeMap<String, ProviderModelInfo>,
    ) -> Vec<ModelSelectionItem> {
        let mut items = models
            .iter()
            .map(|(model_id, model)| ModelSelectionItem {
                slug: format!("{provider_id}/{model_id}"),
                model_id: model_id.clone(),
                display_name: model.name.clone().unwrap_or_else(|| model_id.clone()),
                is_custom: false,
                initial_settings: Self::provider_model_settings_value(model),
                wire_api: model.wire_api,
                default_reasoning_effort: Self::provider_model_default_reasoning_effort(model),
            })
            .collect::<Vec<_>>();
        items.push(ModelSelectionItem {
            slug: String::new(),
            model_id: String::new(),
            display_name: "Add custom model profile".to_string(),
            is_custom: true,
            initial_settings: None,
            wire_api: None,
            default_reasoning_effort: None,
        });
        items
    }

    /// Converts the protocol projection of a saved Connection model back to
    /// the canonical snake_case shape consumed by `ModelSettingsDraft`.
    ///
    /// The protocol uses camelCase for RPC clients, while the persisted
    /// provider catalog intentionally uses snake_case. Keeping this boundary
    /// explicit prevents an edit-and-save cycle from silently dropping model
    /// metadata or provider-specific extensions.
    fn provider_model_settings_value(model: &ProviderModelInfo) -> Option<serde_json::Value> {
        let mut object = serde_json::Map::new();
        macro_rules! insert_option {
            ($field:ident, $name:literal) => {
                if let Some(value) = &model.$field {
                    object.insert(
                        $name.to_string(),
                        serde_json::to_value(value).expect("provider model metadata serializes"),
                    );
                }
            };
        }

        insert_option!(name, "name");
        insert_option!(family, "family");
        insert_option!(release_date, "release_date");
        insert_option!(status, "status");
        insert_option!(capabilities, "capabilities");
        insert_option!(context_window, "context_window");
        insert_option!(
            effective_context_window_percent,
            "effective_context_window_percent"
        );
        insert_option!(max_tokens, "max_tokens");
        insert_option!(temperature, "temperature");
        insert_option!(top_p, "top_p");
        insert_option!(top_k, "top_k");
        insert_option!(reasoning_capability, "reasoning_capability");
        insert_option!(reasoning_implementation, "reasoning_implementation");
        insert_option!(default_reasoning_selection, "default_reasoning_selection");
        insert_option!(base_instructions, "base_instructions");
        insert_option!(input_modalities, "input_modalities");
        insert_option!(channel, "channel");
        insert_option!(
            supports_image_detail_original,
            "supports_image_detail_original"
        );
        insert_option!(truncation_policy, "truncation_policy");
        insert_option!(web_search, "web_search");
        insert_option!(web_fetch, "web_fetch");
        insert_option!(cost, "cost");
        insert_option!(metadata, "metadata");
        insert_option!(request, "request");
        insert_option!(options, "options");
        insert_option!(default_variant, "default_variant");
        insert_option!(enabled, "enabled");
        insert_option!(priority, "priority");
        if !model.headers.is_empty() {
            object.insert(
                "headers".to_string(),
                serde_json::to_value(&model.headers).expect("provider model headers serialize"),
            );
        }
        if !model.variants.is_empty() {
            object.insert(
                "variants".to_string(),
                serde_json::to_value(&model.variants).expect("provider model variants serialize"),
            );
        }
        (!object.is_empty()).then_some(serde_json::Value::Object(object))
    }

    fn provider_model_default_reasoning_effort(model: &ProviderModelInfo) -> Option<String> {
        model
            .default_reasoning_selection
            .as_deref()
            .map(devo_protocol::normalize_reasoning_effort_literal)
            .or_else(|| {
                model
                    .default_reasoning_effort
                    .map(|effort| effort.label().to_ascii_lowercase())
            })
            .or_else(|| match model.reasoning_capability.as_ref()? {
                ReasoningCapability::Unsupported => None,
                ReasoningCapability::Toggle => Some("on".to_string()),
                ReasoningCapability::Levels(levels) => levels
                    .iter()
                    .copied()
                    .find_map(devo_protocol::ReasoningLevelChoice::effort)
                    .map(|effort| effort.label().to_ascii_lowercase())
                    .or_else(|| {
                        levels
                            .first()
                            .map(|choice| choice.selection_value().to_string())
                    }),
            })
    }

    pub(crate) fn take_result(&mut self) -> Option<OnboardingResult> {
        self.result.take()
    }

    pub(crate) fn take_transcript_events(&mut self) -> Vec<OnboardingTranscriptEvent> {
        std::mem::take(&mut self.transcript_events)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn is_validating(&self) -> bool {
        matches!(&self.state, OnboardingState::Validating { .. })
    }

    pub(crate) fn cancel(&mut self) {
        self.complete = true;
        self.result = Some(OnboardingResult::Cancelled);
    }

    pub(crate) fn on_providers_listed(&mut self, providers: Vec<ProviderInfo>) {
        self.providers = providers;
        self.template_provider_ids.clear();
        self.connected_provider_ids.clear();
        self.connection_models.clear();
        self.provider_status_known = false;
        if let OnboardingState::ProviderSelection { items, focus, .. } = &mut self.state {
            let was_empty = items.is_empty();
            *items = Self::provider_selection_items(&self.providers, &[], &[], false);
            if was_empty && !items.is_empty() {
                *focus = SelectionFocus::List;
            }
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_providers_listed_with_status(
        &mut self,
        providers: Vec<ProviderInfo>,
        template_provider_ids: Vec<String>,
        connected_provider_ids: Vec<String>,
    ) {
        self.on_providers_listed_with_status_and_models(
            providers,
            template_provider_ids,
            connected_provider_ids,
            BTreeMap::new(),
        );
    }

    pub(crate) fn on_providers_listed_with_status_and_models(
        &mut self,
        providers: Vec<ProviderInfo>,
        template_provider_ids: Vec<String>,
        connected_provider_ids: Vec<String>,
        connection_models: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
    ) {
        self.providers = providers;
        self.template_provider_ids = template_provider_ids;
        self.connected_provider_ids = connected_provider_ids;
        self.connection_models = connection_models;
        self.provider_status_known = true;
        if let OnboardingState::ProviderSelection { items, focus, .. } = &mut self.state {
            let was_empty = items.is_empty();
            *items = Self::provider_selection_items(
                &self.providers,
                &self.template_provider_ids,
                &self.connected_provider_ids,
                true,
            );
            if was_empty && !items.is_empty() {
                *focus = SelectionFocus::List;
            }
        }
        self.frame_requester.schedule_frame();
    }

    /// Called when validation succeeds.
    pub(crate) fn on_validation_succeeded(&mut self, _reply_preview: String) {
        if let OnboardingState::Validating {
            model_slug,
            request_model,
            display_name,
            provider_id,
            provider_name,
            provider_credential_id,
            invocation_method,
            default_reasoning_effort,
            model_settings,
            base_url,
            api_key,
            ..
        } = &self.state
        {
            self.state = OnboardingState::Saving {
                model_slug: model_slug.clone(),
                request_model: request_model.clone(),
                display_name: display_name.clone(),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                provider_credential_id: provider_credential_id.clone(),
                invocation_method: *invocation_method,
                default_reasoning_effort: default_reasoning_effort.clone(),
                model_settings: model_settings.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                bypassed: false,
                started_at: Instant::now(),
            };
        }
    }

    pub(crate) fn on_provider_upserted(
        &mut self,
        provider: &devo_protocol::ProviderInfo,
        default_model: Option<&str>,
    ) {
        if let OnboardingState::Saving {
            model_slug,
            request_model,
            display_name,
            bypassed,
            ..
        } = &self.state
        {
            let model_prefix = format!("{}/", provider.id);
            let result_model_slug = default_model
                .map(|model| {
                    model
                        .strip_prefix(&model_prefix)
                        .unwrap_or(model)
                        .to_string()
                })
                .unwrap_or_else(|| model_slug.clone());
            let result_request_model = default_model
                .map(|model| {
                    model
                        .strip_prefix(&model_prefix)
                        .unwrap_or(model)
                        .to_string()
                })
                .unwrap_or_else(|| request_model.clone());
            let result_display_name = provider
                .models
                .get(&result_model_slug)
                .and_then(|model| model.name.clone())
                .unwrap_or_else(|| display_name.clone());
            self.result = Some(if *bypassed {
                OnboardingResult::ValidationBypassed {
                    model_slug: result_model_slug,
                    request_model: result_request_model,
                    display_name: result_display_name,
                }
            } else {
                OnboardingResult::ValidationSucceeded {
                    model_slug: result_model_slug,
                    request_model: result_request_model,
                    display_name: result_display_name,
                }
            });
            self.complete = true;
        }
    }

    pub(crate) fn on_provider_disconnected(&mut self, provider_id: &str) {
        self.connected_provider_ids
            .retain(|connected_id| connected_id != provider_id);
        self.connection_models.remove(provider_id);
        if matches!(&self.state, OnboardingState::Disconnecting { .. }) {
            self.state = OnboardingState::ProviderSelection {
                items: Self::provider_selection_items(
                    &self.providers,
                    &self.template_provider_ids,
                    &self.connected_provider_ids,
                    self.provider_status_known,
                ),
                selected_idx: 0,
                focus: SelectionFocus::List,
            };
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::ProviderList));
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_provider_model_removed(&mut self, provider_id: &str, model_id: &str) {
        if let Some(models) = self.connection_models.get_mut(provider_id) {
            models.remove(model_id);
        }
        if let OnboardingState::ModelDeleting { provider, .. } = &self.state {
            let provider = provider.clone();
            self.state = self.model_selection_state_for_connection(provider);
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::ProviderList));
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_provider_model_remove_failed(&mut self) {
        if let OnboardingState::ModelDeleting { provider, .. } = &self.state {
            self.state = self.model_selection_state_for_connection(provider.clone());
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_provider_disconnect_failed(&mut self) {
        if matches!(&self.state, OnboardingState::Disconnecting { .. }) {
            self.state = OnboardingState::ProviderSelection {
                items: Self::provider_selection_items(
                    &self.providers,
                    &self.template_provider_ids,
                    &self.connected_provider_ids,
                    self.provider_status_known,
                ),
                selected_idx: 0,
                focus: SelectionFocus::List,
            };
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn on_provider_save_failed(&mut self, error_message: String) {
        if let OnboardingState::Saving {
            model_slug,
            request_model,
            display_name,
            invocation_method,
            provider_id,
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
            model_settings,
            base_url,
            api_key,
            ..
        } = &self.state
        {
            let recovery_hint = devo_provider::recovery_hint_for_message(&error_message);
            self.state = OnboardingState::ValidationFailed {
                model: model_slug.clone(),
                request_model: request_model.clone(),
                display_name: display_name.clone(),
                provider: *invocation_method,
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                provider_credential_id: provider_credential_id.clone(),
                default_reasoning_effort: default_reasoning_effort.clone(),
                model_settings: model_settings.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                error_message,
                recovery_hint,
                selected_action: 0,
            };
        }
    }

    /// Called when validation fails.
    pub(crate) fn on_validation_failed(
        &mut self,
        error_message: String,
        recovery_hint: Option<String>,
    ) {
        if let OnboardingState::Validating {
            model_slug,
            request_model,
            display_name,
            invocation_method,
            provider_id,
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
            model_settings,
            base_url,
            api_key,
            ..
        } = &self.state
        {
            self.state = OnboardingState::ValidationFailed {
                model: model_slug.clone(),
                request_model: request_model.clone(),
                display_name: display_name.clone(),
                provider: *invocation_method,
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                provider_credential_id: provider_credential_id.clone(),
                default_reasoning_effort: default_reasoning_effort.clone(),
                model_settings: model_settings.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                error_message,
                recovery_hint,
                selected_action: 0,
            };
        }
    }

    pub(crate) fn handle_paste(&mut self, text: String) {
        match &mut self.state {
            OnboardingState::ModelSelection {
                items,
                state,
                search_query,
                filtered_indices,
                focus,
                ..
            } => {
                if *focus == SelectionFocus::List {
                    search_query.push_str(&text);
                    Self::model_apply_filter(items, search_query, filtered_indices, state);
                }
            }
            OnboardingState::CustomModelForm {
                input, cursor_pos, ..
            }
            | OnboardingState::ProviderSetup {
                input, cursor_pos, ..
            }
            | OnboardingState::ModelSettings {
                input, cursor_pos, ..
            }
            | OnboardingState::InlineSetup {
                input, cursor_pos, ..
            } => {
                Self::insert_at_cursor(input, cursor_pos, &text);
            }
            OnboardingState::ProviderSelection { .. }
            | OnboardingState::DisconnectConfirmation { .. }
            | OnboardingState::Disconnecting { .. }
            | OnboardingState::ModelDeleteConfirmation { .. }
            | OnboardingState::ModelDeleting { .. }
            | OnboardingState::InvocationMethod { .. }
            | OnboardingState::ReasoningEffort { .. }
            | OnboardingState::Validating { .. }
            | OnboardingState::Saving { .. }
            | OnboardingState::ValidationFailed { .. }
            | OnboardingState::Review { .. } => {}
        }
    }

    fn char_count(input: &str) -> usize {
        input.chars().count()
    }

    fn byte_index_for_char(input: &str, cursor_pos: usize) -> usize {
        input
            .char_indices()
            .nth(cursor_pos.min(Self::char_count(input)))
            .map(|(idx, _)| idx)
            .unwrap_or(input.len())
    }

    fn insert_at_cursor(input: &mut String, cursor_pos: &mut usize, text: &str) {
        let byte_pos = Self::byte_index_for_char(input, *cursor_pos);
        input.insert_str(byte_pos, text);
        *cursor_pos += Self::char_count(text);
    }

    fn remove_char_before_cursor(input: &mut String, cursor_pos: &mut usize) {
        if *cursor_pos == 0 {
            return;
        }
        let start = Self::byte_index_for_char(input, *cursor_pos - 1);
        let end = Self::byte_index_for_char(input, *cursor_pos);
        input.replace_range(start..end, "");
        *cursor_pos -= 1;
    }

    fn remove_char_at_cursor(input: &mut String, cursor_pos: usize) {
        if cursor_pos >= Self::char_count(input) {
            return;
        }
        let start = Self::byte_index_for_char(input, cursor_pos);
        let end = Self::byte_index_for_char(input, cursor_pos + 1);
        input.replace_range(start..end, "");
    }

    // ── Helpers ──

    fn infer_provider(slug: &str) -> ProviderWireApi {
        let slug_lower = slug.to_lowercase();
        if slug_lower.contains("claude") || slug_lower.contains("anthropic") {
            ProviderWireApi::AnthropicMessages
        } else {
            ProviderWireApi::OpenAIChatCompletions
        }
    }

    fn provider_id_from_name(name: &str) -> String {
        let mut id = String::new();
        let mut previous_separator = false;
        for ch in name.trim().chars() {
            if ch.is_ascii_alphanumeric() {
                id.push(ch.to_ascii_lowercase());
                previous_separator = false;
            } else if !previous_separator && !id.is_empty() {
                id.push('-');
                previous_separator = true;
            }
        }
        let id = id.trim_matches('-').to_string();
        if id.is_empty() {
            // Keep non-ASCII names stable too; using one shared fallback would
            // make two custom providers overwrite each other in the catalog.
            name.trim().to_string()
        } else {
            id
        }
    }

    fn provider_display_name(provider: ProviderWireApi) -> &'static str {
        match provider {
            ProviderWireApi::AnthropicMessages => "Anthropic",
            ProviderWireApi::OpenAIChatCompletions => "OpenAI Chat Completions",
            ProviderWireApi::OpenAIResponses => "OpenAI Responses",
        }
    }

    fn catalog_display_name(&self, slug: &str) -> String {
        self.original_models
            .iter()
            .find(|model| model.slug == slug)
            .map(|model| model.display_name.clone())
            .unwrap_or_else(|| slug.to_string())
    }

    fn model_by_slug(&self, slug: &str) -> Option<&Model> {
        self.original_models.iter().find(|model| model.slug == slug)
    }

    fn model_supports_reasoning(&self, slug: &str) -> bool {
        self.model_by_slug(slug).is_some_and(|model| {
            !matches!(
                model.reasoning_capability,
                devo_protocol::ReasoningCapability::Unsupported
            )
        })
    }

    fn reasoning_capability_from_settings(
        settings: Option<&serde_json::Value>,
    ) -> Option<ReasoningCapability> {
        settings
            .and_then(serde_json::Value::as_object)
            .and_then(|object| object.get("reasoning_capability"))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    fn model_supports_reasoning_with_settings(
        &self,
        slug: &str,
        settings: Option<&serde_json::Value>,
    ) -> bool {
        Self::reasoning_capability_from_settings(settings).map_or_else(
            || self.model_supports_reasoning(slug),
            |capability| !matches!(capability, ReasoningCapability::Unsupported),
        )
    }

    fn reasoning_effort_items(&self, slug: &str) -> Vec<ReasoningEffortItem> {
        self.model_by_slug(slug)
            .map(|model| model.effective_reasoning_capability().options())
            .unwrap_or_default()
            .into_iter()
            .map(Self::reasoning_effort_item)
            .collect()
    }

    fn reasoning_effort_items_with_settings(
        &self,
        slug: &str,
        settings: Option<&serde_json::Value>,
    ) -> Vec<ReasoningEffortItem> {
        if let Some(capability) = Self::reasoning_capability_from_settings(settings) {
            capability
                .options()
                .into_iter()
                .map(Self::reasoning_effort_item)
                .collect()
        } else {
            self.reasoning_effort_items(slug)
        }
    }

    fn reasoning_effort_item(option: ReasoningEffortOption) -> ReasoningEffortItem {
        ReasoningEffortItem {
            label: option.label,
            value: option.value,
            description: option.description,
        }
    }

    fn default_reasoning_effort_index(&self, slug: &str, items: &[ReasoningEffortItem]) -> usize {
        self.model_by_slug(slug)
            .and_then(Model::default_reasoning_effort_selection)
            .and_then(|value| items.iter().position(|item| item.value == value))
            .unwrap_or(0)
    }

    fn default_reasoning_effort_index_with_settings(
        &self,
        slug: &str,
        settings: Option<&serde_json::Value>,
        default_reasoning_effort: Option<&str>,
        items: &[ReasoningEffortItem],
    ) -> usize {
        let selection = default_reasoning_effort.map(str::to_string).or_else(|| {
            Self::reasoning_capability_from_settings(settings).and_then(|capability| {
                match capability {
                    ReasoningCapability::Unsupported => None,
                    ReasoningCapability::Toggle => Some("on".to_string()),
                    ReasoningCapability::Levels(levels) => levels
                        .iter()
                        .copied()
                        .find_map(devo_protocol::ReasoningLevelChoice::effort)
                        .map(|effort| effort.label().to_ascii_lowercase())
                        .or_else(|| {
                            levels
                                .first()
                                .map(|choice| choice.selection_value().to_string())
                        }),
                }
            })
        });
        selection
            .as_deref()
            .and_then(|value| items.iter().position(|item| item.value == value))
            .unwrap_or_else(|| self.default_reasoning_effort_index(slug, items))
    }

    fn invocation_method_selection_index(
        provider: ProviderWireApi,
        items: &[InvocationMethodItem],
    ) -> usize {
        items
            .iter()
            .position(|item| item.provider == provider)
            .unwrap_or(0)
    }

    fn invocation_method_label(provider: ProviderWireApi) -> String {
        Self::invocation_method_items()
            .into_iter()
            .find(|item| item.provider == provider)
            .map(|item| item.label)
            .unwrap_or_else(|| provider.as_str().to_string())
    }

    fn go_back_to_provider_selection(&mut self) {
        let items = Self::provider_selection_items(
            &self.providers,
            &self.template_provider_ids,
            &self.connected_provider_ids,
            self.provider_status_known,
        );
        self.state = OnboardingState::ProviderSelection {
            items,
            selected_idx: 0,
            focus: if self.providers.is_empty() {
                SelectionFocus::Custom
            } else {
                SelectionFocus::List
            },
        };
    }

    fn model_selection_state(models: &[Model], provider: ProviderDraft) -> OnboardingState {
        let items = Self::build_model_items(models, &provider.provider_id);
        Self::model_selection_state_with_items(provider, items, false)
    }

    fn model_selection_state_for_connection(&self, provider: ProviderDraft) -> OnboardingState {
        let items = if let Some(models) = self.connection_models.get(&provider.provider_id) {
            Self::build_connection_model_items(&provider.provider_id, models)
        } else if self.provider_status_known {
            Self::build_connection_model_items(&provider.provider_id, &BTreeMap::new())
        } else {
            Self::build_model_items(&self.original_models, &provider.provider_id)
        };
        Self::model_selection_state_with_items(provider, items, true)
    }

    fn model_selection_state_with_items(
        provider: ProviderDraft,
        items: Vec<ModelSelectionItem>,
        manage_connection: bool,
    ) -> OnboardingState {
        let filtered_indices = (0..items.len()).collect();
        let mut state = ScrollState::new();
        state.selected_idx = Some(0);
        OnboardingState::ModelSelection {
            provider,
            items,
            state,
            search_query: String::new(),
            filtered_indices,
            focus: SelectionFocus::List,
            manage_connection,
        }
    }

    fn model_configuration_state_with_request_model(
        provider: ProviderDraft,
        model: String,
        request_model: String,
        display_name: String,
    ) -> OnboardingState {
        Self::model_configuration_state_with_initial_settings(
            provider,
            model,
            request_model,
            display_name,
            None,
            None,
            None,
        )
    }

    fn model_configuration_state_with_initial_settings(
        provider: ProviderDraft,
        model: String,
        request_model: String,
        display_name: String,
        initial_model_settings: Option<serde_json::Value>,
        model_wire_api: Option<ProviderWireApi>,
        default_reasoning_effort: Option<String>,
    ) -> OnboardingState {
        let items = Self::invocation_method_items();
        let selected_idx = Self::invocation_method_selection_index(
            model_wire_api.unwrap_or(provider.provider),
            &items,
        );
        OnboardingState::InvocationMethod {
            model,
            provider: provider.provider,
            provider_id: provider.provider_id,
            provider_name: provider.provider_name,
            provider_credential_id: provider.provider_credential_id,
            base_url: provider.base_url,
            api_key: provider.api_key,
            request_model,
            display_name,
            items,
            selected_idx,
            initial_model_settings,
            default_reasoning_effort,
        }
    }

    fn inline_setup_state(
        provider: ProviderDraft,
        model: String,
        display_name: String,
    ) -> OnboardingState {
        let request_model = model
            .split_once('/')
            .map_or_else(|| model.clone(), |(_, model_id)| model_id.to_string());
        let (active_field, input) = if provider.base_url.trim().is_empty() {
            (InlineField::BaseUrl, provider.base_url.clone())
        } else {
            (InlineField::RequestModel, request_model.clone())
        };
        Self::inline_setup_state_with_values_and_field(
            provider,
            model,
            request_model,
            display_name,
            active_field,
            input,
        )
    }

    fn inline_setup_state_with_values_and_field(
        provider: ProviderDraft,
        model: String,
        request_model: String,
        display_name: String,
        active_field: InlineField,
        input: String,
    ) -> OnboardingState {
        let cursor_pos = Self::char_count(&input);
        OnboardingState::InlineSetup {
            model,
            provider: provider.provider,
            provider_id: provider.provider_id,
            provider_name: provider.provider_name,
            provider_credential_id: provider.provider_credential_id,
            base_url: provider.base_url,
            api_key: provider.api_key,
            request_model,
            display_name,
            active_field,
            input,
            cursor_pos,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn model_settings_state(
        model: String,
        provider: ProviderWireApi,
        provider_id: String,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        invocation_method: ProviderWireApi,
        initial_model_settings: Option<serde_json::Value>,
        default_reasoning_effort: Option<String>,
    ) -> OnboardingState {
        OnboardingState::ModelSettings {
            model,
            provider,
            provider_id,
            provider_name,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            settings: Box::new(ModelSettingsDraft::from_value(
                initial_model_settings.as_ref(),
                &display_name,
            )),
            display_name: display_name.clone(),
            invocation_method,
            default_reasoning_effort,
            advanced_open: false,
            active_field: ModelSettingsField::DisplayName,
            input: display_name.clone(),
            cursor_pos: Self::char_count(&display_name),
            settings_error: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ValidationParams {
    model_slug: String,
    request_model: String,
    display_name: String,
    provider_id: String,
    provider_name: String,
    provider_credential_id: Option<String>,
    invocation_method: ProviderWireApi,
    default_reasoning_effort: Option<String>,
    model_settings: Option<serde_json::Value>,
    base_url: Option<String>,
    api_key: Option<String>,
}

impl OnboardingWidget {
    fn credential_summary(credential_id: Option<&str>, api_key: Option<&str>) -> String {
        if let Some(id) = credential_id.map(str::trim).filter(|id| !id.is_empty()) {
            format!("saved credential: {id}")
        } else if api_key.map(str::trim).is_some_and(|key| !key.is_empty()) {
            "new API key entered".to_string()
        } else {
            "no credential provided".to_string()
        }
    }

    fn validation_display_name(&self, params: &ValidationParams) -> String {
        if params.display_name.trim().is_empty()
            || (params.display_name == params.request_model
                && params.request_model == params.model_slug)
        {
            self.catalog_display_name(&params.model_slug)
        } else {
            params.display_name.clone()
        }
    }

    fn record_settings_confirmed(&mut self, params: &ValidationParams) {
        self.transcript_events
            .push(OnboardingTranscriptEvent::SettingsConfirmed {
                provider_name: params.provider_name.clone(),
                base_url: params.base_url.clone(),
                request_model: params.request_model.clone(),
                display_name: self.validation_display_name(params),
                invocation_method: params.invocation_method,
                default_reasoning_effort: params.default_reasoning_effort.clone(),
                credential_summary: Self::credential_summary(
                    params.provider_credential_id.as_deref(),
                    params.api_key.as_deref(),
                ),
            });
    }

    fn provider_info_from_validation(
        params: &ValidationParams,
        display_name: &str,
    ) -> (ProviderInfo, String) {
        let model_prefix = format!("{}/", params.provider_id);
        let model_id = params
            .request_model
            .strip_prefix(&model_prefix)
            .unwrap_or(params.request_model.as_str())
            .to_string();
        let mut model = params
            .model_settings
            .as_ref()
            .and_then(|settings| serde_json::from_value::<ProviderModelInfo>(settings.clone()).ok())
            .unwrap_or_default();
        model.name = Some(display_name.to_string());
        model.wire_api = Some(params.invocation_method);
        model.default_reasoning_selection = params.default_reasoning_effort.clone();
        model.default_reasoning_effort = params
            .default_reasoning_effort
            .as_deref()
            .and_then(|value| value.parse().ok());

        let provider = ProviderInfo {
            id: params.provider_id.clone(),
            name: params.provider_name.clone(),
            description: None,
            base_url: params.base_url.clone(),
            credential: params.provider_credential_id.clone(),
            headers: BTreeMap::new(),
            options: None,
            request: None,
            wire_apis: vec![params.invocation_method],
            models: [(model_id.clone(), model)].into_iter().collect(),
            enabled: true,
        };
        (provider, model_id)
    }

    fn start_validation(&mut self, params: ValidationParams) {
        let display_name = self.validation_display_name(&params);
        let (provider, model_id) = Self::provider_info_from_validation(&params, &display_name);

        self.state = OnboardingState::Validating {
            model_slug: params.model_slug.clone(),
            request_model: params.request_model.clone(),
            display_name,
            provider_id: params.provider_id.clone(),
            provider_name: params.provider_name.clone(),
            provider_credential_id: params.provider_credential_id.clone(),
            invocation_method: params.invocation_method,
            default_reasoning_effort: params.default_reasoning_effort.clone(),
            model_settings: params.model_settings.clone(),
            base_url: params.base_url.clone(),
            api_key: params.api_key.clone(),
            started_at: Instant::now(),
        };
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::ProviderValidate {
                params: devo_protocol::native::rpc_admin::ProviderValidateParams {
                    provider,
                    model: model_id,
                    api_key: params.api_key,
                },
            }));
    }

    // ── Key Handling ──

    fn model_selection_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ModelSelection {
            provider,
            items,
            state,
            search_query,
            filtered_indices,
            focus,
            manage_connection,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Tab => {
                *focus = match focus {
                    SelectionFocus::List => SelectionFocus::Custom,
                    SelectionFocus::Custom => SelectionFocus::List,
                };
            }
            KeyCode::Left | KeyCode::Right => {
                *focus = match focus {
                    SelectionFocus::List => SelectionFocus::Custom,
                    SelectionFocus::Custom => SelectionFocus::List,
                };
            }
            KeyCode::Up | KeyCode::Char('p')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && *focus == SelectionFocus::List =>
            {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Up if *focus == SelectionFocus::List => {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Char('k') if key.modifiers.is_empty() && *focus == SelectionFocus::List => {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Down | KeyCode::Char('n')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && *focus == SelectionFocus::List =>
            {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Down if *focus == SelectionFocus::List => {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Char('j') if key.modifiers.is_empty() && *focus == SelectionFocus::List => {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Char('d') if key.modifiers.is_empty() && *focus == SelectionFocus::List => {
                if *manage_connection
                    && let Some(visible_idx) = state.selected_idx
                    && let Some(&actual_idx) = filtered_indices.get(visible_idx)
                    && let Some(item) = items.get(actual_idx)
                    && !item.is_custom
                {
                    self.state = OnboardingState::ModelDeleteConfirmation {
                        provider: provider.clone(),
                        model_id: item.model_id.clone(),
                        model_name: item.display_name.clone(),
                    };
                }
            }
            KeyCode::Delete if *focus == SelectionFocus::List => {
                if *manage_connection
                    && let Some(visible_idx) = state.selected_idx
                    && let Some(&actual_idx) = filtered_indices.get(visible_idx)
                    && let Some(item) = items.get(actual_idx)
                    && !item.is_custom
                {
                    self.state = OnboardingState::ModelDeleteConfirmation {
                        provider: provider.clone(),
                        model_id: item.model_id.clone(),
                        model_name: item.display_name.clone(),
                    };
                }
            }
            KeyCode::Char(c)
                if *focus == SelectionFocus::List
                    && (key.modifiers.is_empty()
                        || key.modifiers.contains(KeyModifiers::SHIFT)) =>
            {
                search_query.push(c);
                Self::model_apply_filter(items, search_query, filtered_indices, state);
            }
            KeyCode::Backspace if *focus == SelectionFocus::List => {
                search_query.pop();
                Self::model_apply_filter(items, search_query, filtered_indices, state);
            }
            KeyCode::Enter => {
                if *focus == SelectionFocus::Custom {
                    let provider = provider.clone();
                    self.state = OnboardingState::CustomModelForm {
                        provider,
                        model_id: String::new(),
                        display_name: String::new(),
                        active_field: CustomModelField::ModelId,
                        input: String::new(),
                        cursor_pos: 0,
                        manage_connection: *manage_connection,
                    };
                } else if let Some(visible_idx) = state.selected_idx
                    && let Some(&actual_idx) = filtered_indices.get(visible_idx)
                    && let Some(item) = items.get(actual_idx)
                {
                    if item.is_custom {
                        self.state = OnboardingState::CustomModelForm {
                            provider: provider.clone(),
                            model_id: String::new(),
                            display_name: String::new(),
                            active_field: CustomModelField::ModelId,
                            input: String::new(),
                            cursor_pos: 0,
                            manage_connection: *manage_connection,
                        };
                    } else {
                        let slug = item.slug.clone();
                        let model_id = item.model_id.clone();
                        let display_name = item.display_name.clone();
                        let initial_model_settings = item.initial_settings.clone();
                        let model_wire_api = item.wire_api;
                        let default_reasoning_effort = item.default_reasoning_effort.clone();
                        self.transcript_events
                            .push(OnboardingTranscriptEvent::ModelSelected {
                                model_slug: slug.clone(),
                                display_name: display_name.clone(),
                            });
                        self.state = if self.provider_status_known {
                            Self::model_configuration_state_with_initial_settings(
                                provider.clone(),
                                slug,
                                model_id,
                                display_name,
                                initial_model_settings,
                                model_wire_api,
                                default_reasoning_effort,
                            )
                        } else {
                            Self::inline_setup_state(provider.clone(), slug, display_name)
                        };
                    }
                }
            }
            KeyCode::Esc => {
                self.go_back_to_provider_selection();
            }
            _ => {}
        }
    }

    fn model_move_up(state: &mut ScrollState, filtered_indices: &[usize]) {
        let len = filtered_indices.len();
        if len == 0 {
            return;
        }
        let current = state.selected_idx.unwrap_or(0);
        state.selected_idx = Some(if current == 0 { len - 1 } else { current - 1 });
        state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn model_move_down(state: &mut ScrollState, filtered_indices: &[usize]) {
        let len = filtered_indices.len();
        if len == 0 {
            return;
        }
        let current = state.selected_idx.unwrap_or(0);
        state.selected_idx = Some((current + 1) % len);
        state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn model_apply_filter(
        items: &[ModelSelectionItem],
        query: &str,
        filtered_indices: &mut Vec<usize>,
        state: &mut ScrollState,
    ) {
        let query_lower = query.to_lowercase();
        if query.is_empty() {
            *filtered_indices = (0..items.len()).collect();
        } else {
            *filtered_indices = items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.slug.to_lowercase().contains(&query_lower)
                        || item.display_name.to_lowercase().contains(&query_lower)
                })
                .map(|(idx, _)| idx)
                .collect();
        }
        state.selected_idx = if filtered_indices.is_empty() {
            None
        } else {
            Some(0)
        };
        state.scroll_top = 0;
    }

    fn custom_model_form_handle_key(&mut self, key: KeyEvent) {
        let connection_models = self.connection_models.clone();
        let original_models = self.original_models.clone();
        let provider_status_known = self.provider_status_known;
        let OnboardingState::CustomModelForm {
            provider,
            model_id,
            display_name,
            active_field,
            input,
            cursor_pos,
            manage_connection,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Self::insert_at_cursor(input, cursor_pos, &c.to_string());
            }
            KeyCode::Backspace => {
                Self::remove_char_before_cursor(input, cursor_pos);
            }
            KeyCode::Delete => {
                Self::remove_char_at_cursor(input, *cursor_pos);
            }
            KeyCode::Left => {
                if *cursor_pos > 0 {
                    *cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if *cursor_pos < Self::char_count(input) {
                    *cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                *cursor_pos = 0;
            }
            KeyCode::End => {
                *cursor_pos = Self::char_count(input);
            }
            KeyCode::Enter | KeyCode::Tab => match active_field {
                CustomModelField::ModelId => {
                    if input.trim().is_empty() {
                        return;
                    }
                    *model_id = input.trim().to_string();
                    *active_field = CustomModelField::DisplayName;
                    input.clear();
                    *cursor_pos = 0;
                }
                CustomModelField::DisplayName => {
                    *display_name = input.trim().to_string();
                    let model = model_id.clone();
                    let display_name = if display_name.trim().is_empty() {
                        model.clone()
                    } else {
                        display_name.clone()
                    };
                    self.transcript_events
                        .push(OnboardingTranscriptEvent::ModelSelected {
                            model_slug: model.clone(),
                            display_name: display_name.clone(),
                        });
                    self.state = if provider_status_known {
                        Self::model_configuration_state_with_request_model(
                            provider.clone(),
                            model.clone(),
                            model,
                            display_name,
                        )
                    } else {
                        Self::inline_setup_state_with_values_and_field(
                            provider.clone(),
                            model.clone(),
                            model,
                            display_name.clone(),
                            InlineField::DisplayName,
                            display_name,
                        )
                    };
                }
            },
            KeyCode::Esc => match active_field {
                CustomModelField::DisplayName => {
                    *active_field = CustomModelField::ModelId;
                    *input = model_id.clone();
                    *cursor_pos = Self::char_count(input);
                }
                CustomModelField::ModelId => {
                    self.state = if *manage_connection {
                        let items = connection_models
                            .get(&provider.provider_id)
                            .map(|models| {
                                Self::build_connection_model_items(&provider.provider_id, models)
                            })
                            .or_else(|| {
                                provider_status_known.then(|| {
                                    Self::build_connection_model_items(
                                        &provider.provider_id,
                                        &BTreeMap::new(),
                                    )
                                })
                            })
                            .unwrap_or_else(|| {
                                Self::build_model_items(&original_models, &provider.provider_id)
                            });
                        Self::model_selection_state_with_items(provider.clone(), items, true)
                    } else {
                        Self::model_selection_state(&original_models, provider.clone())
                    };
                }
            },
            _ => {}
        }
    }

    fn provider_selection_items(
        providers: &[ProviderInfo],
        template_provider_ids: &[String],
        connected_provider_ids: &[String],
        status_known: bool,
    ) -> Vec<ProviderSelectionItem> {
        let mut items = Vec::new();
        let mut add_item =
            |provider: &ProviderInfo, section: ProviderSelectionSection, is_custom: bool| {
                let endpoint = provider
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "Endpoint required".to_string());
                let description = match section {
                    ProviderSelectionSection::Connections => {
                        format!("Saved Connection · {endpoint}")
                    }
                    ProviderSelectionSection::Templates => {
                        format!("Read-only template · {endpoint}")
                    }
                };
                items.push(ProviderSelectionItem {
                    label: provider.name.clone(),
                    description,
                    provider: provider.clone(),
                    section,
                    is_custom,
                });
            };

        for provider in providers {
            let is_connected = connected_provider_ids
                .iter()
                .any(|provider_id| provider_id == &provider.id);
            let is_template = template_provider_ids
                .iter()
                .any(|provider_id| provider_id == &provider.id);
            if status_known && is_connected {
                add_item(
                    provider,
                    ProviderSelectionSection::Connections,
                    !is_template,
                );
            }
        }
        for provider in providers {
            let is_connected = connected_provider_ids
                .iter()
                .any(|provider_id| provider_id == &provider.id);
            let is_template = template_provider_ids
                .iter()
                .any(|provider_id| provider_id == &provider.id);
            if !status_known || is_template {
                add_item(provider, ProviderSelectionSection::Templates, !is_template);
            } else if status_known && !is_connected {
                add_item(provider, ProviderSelectionSection::Templates, true);
            }
        }
        items
    }

    fn provider_selection_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ProviderSelection {
            items,
            selected_idx,
            focus,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                *focus = match focus {
                    SelectionFocus::List => SelectionFocus::Custom,
                    SelectionFocus::Custom => SelectionFocus::List,
                };
            }
            KeyCode::Up if *focus == SelectionFocus::List && !items.is_empty() => {
                *selected_idx = if *selected_idx == 0 {
                    items.len() - 1
                } else {
                    *selected_idx - 1
                };
            }
            KeyCode::Down if *focus == SelectionFocus::List && !items.is_empty() => {
                *selected_idx = (*selected_idx + 1) % items.len();
            }
            KeyCode::Enter if *focus == SelectionFocus::Custom => {
                self.transcript_events
                    .push(OnboardingTranscriptEvent::ProviderSelected {
                        provider_name: "Custom provider".to_string(),
                        base_url: None,
                        credential_summary: "new provider credentials".to_string(),
                    });
                self.state = OnboardingState::ProviderSetup {
                    draft: ProviderDraft::default(),
                    active_field: InlineField::ProviderName,
                    input: String::new(),
                    cursor_pos: 0,
                };
            }
            KeyCode::Enter if *focus == SelectionFocus::List => {
                if let Some(item) = items.get(*selected_idx) {
                    let vendor = &item.provider;
                    let provider = vendor
                        .wire_apis
                        .first()
                        .copied()
                        .unwrap_or_else(|| Self::infer_provider(vendor.name.as_str()));
                    let provider_id = if vendor.id.trim().is_empty() {
                        vendor.name.clone()
                    } else {
                        vendor.id.clone()
                    };
                    self.transcript_events
                        .push(OnboardingTranscriptEvent::ProviderSelected {
                            provider_name: vendor.name.clone(),
                            base_url: vendor.base_url.clone(),
                            credential_summary: Self::credential_summary(
                                vendor.credential.as_deref(),
                                None,
                            ),
                        });
                    let is_connection = item.section == ProviderSelectionSection::Connections;
                    let is_custom = item.is_custom;
                    let provider = ProviderDraft {
                        provider,
                        provider_id,
                        provider_name: vendor.name.clone(),
                        provider_credential_id: vendor.credential.clone(),
                        base_url: vendor.base_url.clone().unwrap_or_default(),
                        api_key: String::new(),
                        is_custom,
                    };
                    if is_connection {
                        self.state = self.model_selection_state_for_connection(provider);
                        return;
                    }
                    let input = provider.base_url.clone();
                    if self.provider_status_known {
                        self.state = OnboardingState::ProviderSetup {
                            draft: provider,
                            active_field: if is_custom {
                                InlineField::BaseUrl
                            } else {
                                InlineField::ApiKey
                            },
                            cursor_pos: Self::char_count(&input),
                            input: if is_custom { input } else { String::new() },
                        };
                    } else {
                        let models = self.original_models.clone();
                        self.state = Self::model_selection_state(&models, provider);
                    }
                }
            }
            KeyCode::Char('d') if key.modifiers.is_empty() && *focus == SelectionFocus::List => {
                if let Some(item) = items.get(*selected_idx)
                    && item.section == ProviderSelectionSection::Connections
                    && self.provider_status_known
                {
                    self.state = OnboardingState::DisconnectConfirmation {
                        provider: item.provider.clone(),
                    };
                }
            }
            KeyCode::Delete if *focus == SelectionFocus::List => {
                if let Some(item) = items.get(*selected_idx)
                    && item.section == ProviderSelectionSection::Connections
                    && self.provider_status_known
                {
                    self.state = OnboardingState::DisconnectConfirmation {
                        provider: item.provider.clone(),
                    };
                }
            }
            KeyCode::Esc => {
                self.complete = true;
                self.result = Some(OnboardingResult::Cancelled);
            }
            _ => {}
        }
    }

    fn disconnect_confirmation_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::DisconnectConfirmation { provider } = &self.state else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                let provider_id = provider.id.clone();
                let provider_name = provider.name.clone();
                self.state = OnboardingState::Disconnecting { provider_name };
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::DisconnectProvider {
                        provider_id,
                    }));
            }
            KeyCode::Esc => self.go_back_to_provider_selection(),
            _ => {}
        }
    }

    fn model_delete_confirmation_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ModelDeleteConfirmation {
            provider,
            model_id,
            model_name,
        } = &self.state
        else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                let provider_id = provider.provider_id.clone();
                let model_id = model_id.clone();
                let provider = provider.clone();
                let model_name = model_name.clone();
                self.state = OnboardingState::ModelDeleting {
                    provider,
                    model_name,
                };
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RemoveProviderModel {
                        provider_id,
                        model_id,
                    }));
            }
            KeyCode::Esc => {
                let provider = provider.clone();
                self.state = self.model_selection_state_for_connection(provider);
            }
            _ => {}
        }
    }

    fn provider_setup_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ProviderSetup {
            draft,
            active_field,
            input,
            cursor_pos,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Self::insert_at_cursor(input, cursor_pos, &c.to_string());
            }
            KeyCode::Backspace => Self::remove_char_before_cursor(input, cursor_pos),
            KeyCode::Delete => Self::remove_char_at_cursor(input, *cursor_pos),
            KeyCode::Left => *cursor_pos = cursor_pos.saturating_sub(1),
            KeyCode::Right => *cursor_pos = (*cursor_pos + 1).min(Self::char_count(input)),
            KeyCode::Home => *cursor_pos = 0,
            KeyCode::End => *cursor_pos = Self::char_count(input),
            KeyCode::Enter => match active_field {
                InlineField::ProviderName => {
                    if !draft.is_custom {
                        return;
                    }
                    if input.trim().is_empty() {
                        return;
                    }
                    draft.provider_name = input.trim().to_string();
                    *active_field = InlineField::BaseUrl;
                    input.clear();
                    *cursor_pos = 0;
                }
                InlineField::BaseUrl => {
                    if !draft.is_custom {
                        *active_field = InlineField::ApiKey;
                        input.clear();
                        *cursor_pos = 0;
                        return;
                    }
                    if input.trim().is_empty() {
                        return;
                    }
                    draft.base_url = input.trim().to_string();
                    *active_field = InlineField::ApiKey;
                    input.clear();
                    *cursor_pos = 0;
                }
                InlineField::ApiKey => {
                    draft.api_key = input.trim().to_string();
                    if draft.provider_id.trim().is_empty() {
                        draft.provider_id = Self::provider_id_from_name(&draft.provider_name);
                    }
                    let draft = draft.clone();
                    let models = self.original_models.clone();
                    self.state = Self::model_selection_state(&models, draft);
                }
                InlineField::RequestModel | InlineField::DisplayName => {}
            },
            KeyCode::Esc => match active_field {
                InlineField::ProviderName => self.go_back_to_provider_selection(),
                InlineField::BaseUrl => {
                    if draft.is_custom {
                        *active_field = InlineField::ProviderName;
                        *input = draft.provider_name.clone();
                        *cursor_pos = Self::char_count(input);
                    } else {
                        self.go_back_to_provider_selection();
                    }
                }
                InlineField::ApiKey => {
                    if draft.is_custom {
                        *active_field = InlineField::BaseUrl;
                        *input = draft.base_url.clone();
                        *cursor_pos = Self::char_count(input);
                    } else {
                        self.go_back_to_provider_selection();
                    }
                }
                InlineField::RequestModel | InlineField::DisplayName => {}
            },
            _ => {}
        }
    }

    // ── Inline Setup ──

    fn inline_setup_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::InlineSetup {
            model,
            provider,
            provider_id,
            provider_name,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            active_field,
            input,
            cursor_pos,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Self::insert_at_cursor(input, cursor_pos, &c.to_string());
            }
            KeyCode::Backspace => {
                Self::remove_char_before_cursor(input, cursor_pos);
            }
            KeyCode::Delete => {
                Self::remove_char_at_cursor(input, *cursor_pos);
            }
            KeyCode::Left => {
                if *cursor_pos > 0 {
                    *cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if *cursor_pos < Self::char_count(input) {
                    *cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                *cursor_pos = 0;
            }
            KeyCode::End => {
                *cursor_pos = Self::char_count(input);
            }
            KeyCode::Enter => {
                // Save current field and advance.
                match active_field {
                    InlineField::ProviderName => {
                        if input.trim().is_empty() {
                            return;
                        }
                        *provider_name = input.trim().to_string();
                        *active_field = InlineField::BaseUrl;
                        input.clear();
                        *cursor_pos = 0;
                    }
                    InlineField::BaseUrl => {
                        if input.trim().is_empty() {
                            return;
                        }
                        *base_url = input.trim().to_string();
                        *active_field = InlineField::ApiKey;
                        *input = String::new();
                        *cursor_pos = 0;
                    }
                    InlineField::ApiKey => {
                        *api_key = input.trim().to_string();
                        *active_field = InlineField::RequestModel;
                        *input = request_model.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    InlineField::RequestModel => {
                        *request_model = input.trim().to_string();
                        *active_field = InlineField::DisplayName;
                        let suggestion = if display_name.trim().is_empty() {
                            request_model.clone()
                        } else {
                            display_name.clone()
                        };
                        *input = suggestion.clone();
                        *cursor_pos = Self::char_count(&suggestion);
                    }
                    InlineField::DisplayName => {
                        *display_name = input.trim().to_string();
                        // Move to invocation method selection.
                        let model = model.clone();
                        let provider = *provider;
                        let provider_id = provider_id.clone();
                        let provider_name = provider_name.clone();
                        let provider_credential_id = provider_credential_id.clone();
                        let base_url = base_url.clone();
                        let api_key = api_key.clone();
                        let request_model = request_model.clone();
                        let display_name = display_name.clone();
                        let items = Self::invocation_method_items();
                        let selected_idx =
                            Self::invocation_method_selection_index(provider, &items);
                        self.state = OnboardingState::InvocationMethod {
                            model,
                            provider,
                            provider_id,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            items,
                            selected_idx,
                            initial_model_settings: None,
                            default_reasoning_effort: None,
                        };
                    }
                }
            }
            KeyCode::Esc => {
                // Go back to previous field or provider selection.
                match active_field {
                    InlineField::ProviderName => {
                        // Go back to model selection for the current provider.
                        let provider = ProviderDraft {
                            provider: *provider,
                            provider_id: provider_id.clone(),
                            provider_name: provider_name.clone(),
                            provider_credential_id: provider_credential_id.clone(),
                            base_url: base_url.clone(),
                            api_key: api_key.clone(),
                            is_custom: true,
                        };
                        let models = self.original_models.clone();
                        self.state = Self::model_selection_state(&models, provider);
                    }
                    InlineField::BaseUrl => {
                        *active_field = InlineField::ProviderName;
                        *input = provider_name.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    InlineField::ApiKey => {
                        *active_field = InlineField::BaseUrl;
                        *input = base_url.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    InlineField::RequestModel => {
                        *active_field = InlineField::ApiKey;
                        *input = api_key.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    InlineField::DisplayName => {
                        *active_field = InlineField::RequestModel;
                        *input = request_model.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                }
            }
            _ => {}
        }
    }

    fn invocation_method_items() -> Vec<InvocationMethodItem> {
        vec![
            InvocationMethodItem {
                label: "OpenAI Chat Completions".to_string(),
                description: "Most providers (OpenAI, Together, Groq, ...)".to_string(),
                provider: ProviderWireApi::OpenAIChatCompletions,
            },
            InvocationMethodItem {
                label: "OpenAI Responses".to_string(),
                description: "OpenAI native Responses API".to_string(),
                provider: ProviderWireApi::OpenAIResponses,
            },
            InvocationMethodItem {
                label: "Anthropic Messages".to_string(),
                description: "Claude models via Anthropic API".to_string(),
                provider: ProviderWireApi::AnthropicMessages,
            },
        ]
    }

    fn invocation_method_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::InvocationMethod {
            model,
            provider,
            provider_id,
            provider_name,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            items,
            selected_idx,
            initial_model_settings,
            default_reasoning_effort,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Up => {
                *selected_idx = if *selected_idx == 0 {
                    items.len() - 1
                } else {
                    *selected_idx - 1
                };
            }
            KeyCode::Down => {
                *selected_idx = (*selected_idx + 1) % items.len();
            }
            KeyCode::Enter => {
                if let Some(item) = items.get(*selected_idx) {
                    let invocation = item.provider;
                    let model = model.clone();
                    let provider = *provider;
                    let provider_id = provider_id.clone();
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    let request_model = request_model.clone();
                    let display_name = display_name.clone();
                    let initial_model_settings = initial_model_settings.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();

                    if self.model_supports_reasoning_with_settings(
                        &model,
                        initial_model_settings.as_ref(),
                    ) {
                        let reasoning_items = self.reasoning_effort_items_with_settings(
                            &model,
                            initial_model_settings.as_ref(),
                        );
                        let selected_reasoning_idx = self
                            .default_reasoning_effort_index_with_settings(
                                &model,
                                initial_model_settings.as_ref(),
                                default_reasoning_effort.as_deref(),
                                &reasoning_items,
                            );
                        self.state = OnboardingState::ReasoningEffort {
                            model,
                            provider,
                            provider_id,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            invocation_method: invocation,
                            items: reasoning_items,
                            selected_idx: selected_reasoning_idx,
                            initial_model_settings,
                            default_reasoning_effort,
                        };
                    } else {
                        self.state = Self::model_settings_state(
                            model,
                            provider,
                            provider_id,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            invocation,
                            initial_model_settings,
                            default_reasoning_effort,
                        );
                    }
                }
            }
            KeyCode::Esc => {
                // Go back to inline setup, display name field.
                let model = model.clone();
                let provider = *provider;
                let provider_id = provider_id.clone();
                let provider_name = provider_name.clone();
                let provider_credential_id = provider_credential_id.clone();
                let base_url = base_url.clone();
                let api_key = api_key.clone();
                let model_name_val = request_model.clone();
                let display_name_val = display_name.clone();
                self.state = OnboardingState::InlineSetup {
                    model,
                    provider,
                    provider_id,
                    provider_name,
                    provider_credential_id,
                    base_url,
                    api_key,
                    request_model: model_name_val.clone(),
                    display_name: display_name_val.clone(),
                    active_field: InlineField::DisplayName,
                    input: display_name_val,
                    cursor_pos: Self::char_count(display_name),
                };
            }
            _ => {}
        }
    }

    fn reasoning_effort_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ReasoningEffort {
            model,
            provider,
            provider_id,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            provider_name,
            invocation_method,
            items,
            selected_idx,
            initial_model_settings,
            default_reasoning_effort: _,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Up => {
                *selected_idx = if *selected_idx == 0 {
                    items.len() - 1
                } else {
                    *selected_idx - 1
                };
            }
            KeyCode::Down => {
                *selected_idx = (*selected_idx + 1) % items.len();
            }
            KeyCode::Enter => {
                let model = model.clone();
                let invocation_method = *invocation_method;
                let request_model = request_model.clone();
                let display_name = display_name.clone();
                let provider_name = provider_name.clone();
                let provider_id = provider_id.clone();
                let provider_credential_id = provider_credential_id.clone();
                let default_reasoning_effort =
                    items.get(*selected_idx).map(|item| item.value.clone());
                let initial_model_settings = initial_model_settings.clone();
                let base_url = base_url.clone();
                let api_key = api_key.clone();
                self.state = Self::model_settings_state(
                    model,
                    *provider,
                    provider_id,
                    provider_name,
                    provider_credential_id,
                    base_url,
                    api_key,
                    request_model,
                    display_name,
                    invocation_method,
                    initial_model_settings,
                    default_reasoning_effort,
                );
            }
            KeyCode::Esc => {
                // Go back to invocation method selection.
                // Extract values before reassigning self.state.
                let (m, prov, pid, pn, pc, bu, ak, mn, dn, invocation, settings, default) =
                    match &self.state {
                        OnboardingState::ReasoningEffort {
                            model,
                            provider,
                            provider_id,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            invocation_method,
                            initial_model_settings,
                            default_reasoning_effort,
                            ..
                        } => (
                            model.clone(),
                            *provider,
                            provider_id.clone(),
                            provider_name.clone(),
                            provider_credential_id.clone(),
                            base_url.clone(),
                            api_key.clone(),
                            request_model.clone(),
                            display_name.clone(),
                            *invocation_method,
                            initial_model_settings.clone(),
                            default_reasoning_effort.clone(),
                        ),
                        _ => return,
                    };
                let items = Self::invocation_method_items();
                let selected_idx = Self::invocation_method_selection_index(invocation, &items);
                self.state = OnboardingState::InvocationMethod {
                    model: m,
                    provider: prov,
                    provider_id: pid,
                    provider_name: pn,
                    provider_credential_id: pc,
                    base_url: bu,
                    api_key: ak,
                    request_model: mn,
                    display_name: dn,
                    items,
                    selected_idx,
                    initial_model_settings: settings,
                    default_reasoning_effort: default,
                };
            }
            _ => {}
        }
    }

    fn model_settings_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ModelSettings {
            model,
            provider,
            provider_id,
            provider_name,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            invocation_method,
            default_reasoning_effort,
            settings,
            advanced_open,
            active_field,
            input,
            cursor_pos,
            settings_error,
        } = &mut self.state
        else {
            return;
        };

        *settings_error = None;

        if *active_field == ModelSettingsField::AdvancedToggle
            && matches!(key.code, KeyCode::Char(' '))
        {
            *advanced_open = true;
            *active_field = ModelSettingsField::ContextWindow;
            *input = settings.context_window.clone();
            *cursor_pos = Self::char_count(input);
            return;
        }
        if matches!(key.code, KeyCode::Char(' '))
            && matches!(
                active_field,
                ModelSettingsField::OriginalImageDetail | ModelSettingsField::Enabled
            )
        {
            match active_field {
                ModelSettingsField::OriginalImageDetail => {
                    settings.supports_image_detail_original =
                        Some(!settings.supports_image_detail_original.unwrap_or(false));
                }
                ModelSettingsField::Enabled => {
                    settings.enabled = Some(!settings.enabled.unwrap_or(true));
                }
                _ => {}
            }
            return;
        }

        if key.code == KeyCode::Enter && *active_field == ModelSettingsField::DisplayName {
            settings.display_name = input.trim().to_string();
            *display_name = settings.display_name.clone();
            *active_field = ModelSettingsField::AdvancedToggle;
            input.clear();
            *cursor_pos = 0;
            return;
        }

        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Self::insert_at_cursor(input, cursor_pos, &c.to_string());
            }
            KeyCode::Backspace => Self::remove_char_before_cursor(input, cursor_pos),
            KeyCode::Delete => Self::remove_char_at_cursor(input, *cursor_pos),
            KeyCode::Left => *cursor_pos = (*cursor_pos).saturating_sub(1),
            KeyCode::Right => *cursor_pos = (*cursor_pos + 1).min(Self::char_count(input)),
            KeyCode::Home => *cursor_pos = 0,
            KeyCode::End => *cursor_pos = Self::char_count(input),
            KeyCode::Esc => {
                let model = model.clone();
                let provider = *provider;
                let provider_name = provider_name.clone();
                let provider_credential_id = provider_credential_id.clone();
                let base_url = base_url.clone();
                let api_key = api_key.clone();
                let request_model = request_model.clone();
                let display_name = display_name.clone();
                let initial_model_settings = settings.to_value(&request_model);
                let default_reasoning_effort = default_reasoning_effort.clone();
                let items = Self::invocation_method_items();
                let selected_idx =
                    Self::invocation_method_selection_index(*invocation_method, &items);
                self.state = OnboardingState::InvocationMethod {
                    model,
                    provider,
                    provider_id: provider_id.clone(),
                    provider_name,
                    provider_credential_id,
                    base_url,
                    api_key,
                    request_model,
                    display_name,
                    items,
                    selected_idx,
                    initial_model_settings,
                    default_reasoning_effort,
                };
            }
            KeyCode::Enter | KeyCode::Tab => {
                if *active_field == ModelSettingsField::AdvancedToggle && !*advanced_open {
                    let params = match Self::validation_params_from_settings(
                        model,
                        provider_id,
                        provider_name,
                        provider_credential_id,
                        base_url,
                        api_key,
                        request_model,
                        display_name,
                        *invocation_method,
                        default_reasoning_effort,
                        settings,
                    ) {
                        Ok(params) => params,
                        Err(error) => {
                            *settings_error = Some(error);
                            return;
                        }
                    };
                    self.record_settings_confirmed(&params);
                    self.state = OnboardingState::Review { params };
                    return;
                }

                match active_field {
                    ModelSettingsField::DisplayName => {
                        settings.display_name = input.trim().to_string();
                        *display_name = settings.display_name.clone();
                        *active_field = ModelSettingsField::ContextWindow;
                        *input = settings.context_window.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ContextWindow => {
                        settings.context_window = input.trim().to_string();
                        *active_field = ModelSettingsField::MaxTokens;
                        *input = settings.max_tokens.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::MaxTokens => {
                        settings.max_tokens = input.trim().to_string();
                        *active_field = ModelSettingsField::Temperature;
                        *input = settings.temperature.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::Temperature => {
                        settings.temperature = input.trim().to_string();
                        *active_field = ModelSettingsField::InputModalities;
                        *input = settings.input_modalities.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::InputModalities => {
                        settings.input_modalities = input.trim().to_string();
                        *active_field = ModelSettingsField::ReasoningCapability;
                        *input = settings.reasoning_capability.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ReasoningCapability => {
                        settings.reasoning_capability = input.trim().to_string();
                        *active_field = ModelSettingsField::DefaultReasoning;
                        *input = default_reasoning_effort.clone().unwrap_or_default();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::DefaultReasoning => {
                        let value = input.trim();
                        *default_reasoning_effort = (!value.is_empty()).then(|| value.to_string());
                        *active_field = ModelSettingsField::AdvancedToggle;
                        input.clear();
                        *cursor_pos = 0;
                    }
                    ModelSettingsField::AdvancedToggle => {
                        *active_field = ModelSettingsField::TopP;
                        *input = settings.top_p.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::EffectiveContext => {
                        // Percent editing removed; jump to Top P.
                        *active_field = ModelSettingsField::TopP;
                        *input = settings.top_p.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::TopP => {
                        settings.top_p = input.trim().to_string();
                        *active_field = ModelSettingsField::TopK;
                        *input = settings.top_k.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::TopK => {
                        settings.top_k = input.trim().to_string();
                        *active_field = ModelSettingsField::Family;
                        *input = settings.family.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::Family => {
                        settings.family = input.trim().to_string();
                        *active_field = ModelSettingsField::ReleaseDate;
                        *input = settings.release_date.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ReleaseDate => {
                        settings.release_date = input.trim().to_string();
                        *active_field = ModelSettingsField::Status;
                        *input = settings.status.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::Status => {
                        settings.status = input.trim().to_string();
                        *active_field = ModelSettingsField::CapabilitiesJson;
                        *input = settings.capabilities_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::CapabilitiesJson => {
                        settings.capabilities_json = input.trim().to_string();
                        *active_field = ModelSettingsField::Channel;
                        *input = settings.channel.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::Channel => {
                        settings.channel = input.trim().to_string();
                        *active_field = ModelSettingsField::BaseInstructions;
                        *input = settings.base_instructions.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::BaseInstructions => {
                        settings.base_instructions = input.to_string();
                        *active_field = ModelSettingsField::ReasoningImplementation;
                        *input = settings.reasoning_implementation.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ReasoningImplementation => {
                        settings.reasoning_implementation = input.trim().to_string();
                        *active_field = ModelSettingsField::ReasoningLevels;
                        *input = settings.reasoning_levels.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ReasoningLevels => {
                        settings.reasoning_levels = input.trim().to_string();
                        *active_field = ModelSettingsField::ReasoningVariantsJson;
                        *input = settings.reasoning_variants_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::ReasoningVariantsJson => {
                        settings.reasoning_variants_json = input.trim().to_string();
                        *active_field = ModelSettingsField::DefaultVariant;
                        *input = settings.default_variant.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::DefaultVariant => {
                        settings.default_variant = input.trim().to_string();
                        *active_field = ModelSettingsField::CostJson;
                        *input = settings.cost_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::CostJson => {
                        settings.cost_json = input.trim().to_string();
                        *active_field = ModelSettingsField::MetadataJson;
                        *input = settings.metadata_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::MetadataJson => {
                        settings.metadata_json = input.trim().to_string();
                        *active_field = ModelSettingsField::RequestJson;
                        *input = settings.request_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::RequestJson => {
                        settings.request_json = input.trim().to_string();
                        *active_field = ModelSettingsField::OptionsJson;
                        *input = settings.options_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::OptionsJson => {
                        settings.options_json = input.trim().to_string();
                        *active_field = ModelSettingsField::HeadersJson;
                        *input = settings.headers_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::HeadersJson => {
                        settings.headers_json = input.trim().to_string();
                        *active_field = ModelSettingsField::VariantsJson;
                        *input = settings.variants_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::VariantsJson => {
                        settings.variants_json = input.trim().to_string();
                        *active_field = ModelSettingsField::WebSearchJson;
                        *input = settings.web_search_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::WebSearchJson => {
                        settings.web_search_json = input.trim().to_string();
                        *active_field = ModelSettingsField::WebFetchJson;
                        *input = settings.web_fetch_json.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::WebFetchJson => {
                        settings.web_fetch_json = input.trim().to_string();
                        *active_field = ModelSettingsField::TruncationMode;
                        *input = settings.truncation_mode.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::TruncationMode => {
                        settings.truncation_mode = input.trim().to_string();
                        *active_field = ModelSettingsField::TruncationLimit;
                        *input = settings.truncation_limit.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::TruncationLimit => {
                        settings.truncation_limit = input.trim().to_string();
                        *active_field = ModelSettingsField::OriginalImageDetail;
                        input.clear();
                        *cursor_pos = 0;
                    }
                    ModelSettingsField::OriginalImageDetail => {
                        *active_field = ModelSettingsField::Enabled;
                        input.clear();
                        *cursor_pos = 0;
                    }
                    ModelSettingsField::Enabled => {
                        *active_field = ModelSettingsField::Priority;
                        *input = settings.priority.clone();
                        *cursor_pos = Self::char_count(input);
                    }
                    ModelSettingsField::Priority => {
                        settings.priority = input.trim().to_string();
                        let params = match Self::validation_params_from_settings(
                            model,
                            provider_id,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            *invocation_method,
                            default_reasoning_effort,
                            settings,
                        ) {
                            Ok(params) => params,
                            Err(error) => {
                                *settings_error = Some(error);
                                return;
                            }
                        };
                        self.record_settings_confirmed(&params);
                        self.state = OnboardingState::Review { params };
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validation_params_from_settings(
        model: &str,
        provider_id: &str,
        provider_name: &str,
        provider_credential_id: &Option<String>,
        base_url: &str,
        api_key: &str,
        request_model: &str,
        display_name: &str,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: &Option<String>,
        settings: &ModelSettingsDraft,
    ) -> Result<ValidationParams, String> {
        if let Some(error) = settings.validation_error(request_model) {
            return Err(error);
        }
        if let Some(reasoning) = default_reasoning_effort.as_deref()
            && !matches!(reasoning, "on" | "off" | "enabled" | "disabled")
            && reasoning.parse::<ReasoningEffort>().is_err()
        {
            return Err("Default reasoning must be on, off, or an effort level".to_string());
        }
        if let Some(error) = Self::default_reasoning_error(settings, default_reasoning_effort) {
            return Err(error);
        }
        Ok(ValidationParams {
            model_slug: model.to_string(),
            request_model: request_model.to_string(),
            display_name: display_name.to_string(),
            provider_id: provider_id.to_string(),
            provider_name: provider_name.to_string(),
            provider_credential_id: provider_credential_id.clone(),
            invocation_method,
            default_reasoning_effort: default_reasoning_effort.clone(),
            model_settings: settings.to_value(request_model),
            base_url: (!base_url.is_empty()).then(|| base_url.to_string()),
            api_key: (!api_key.is_empty()).then(|| api_key.to_string()),
        })
    }

    fn default_reasoning_error(
        settings: &ModelSettingsDraft,
        default_reasoning_effort: &Option<String>,
    ) -> Option<String> {
        let selection = default_reasoning_effort
            .as_deref()?
            .trim()
            .to_ascii_lowercase();
        let capability = settings.reasoning_capability.trim().to_ascii_lowercase();
        if capability.is_empty() {
            return None;
        }
        let levels = settings
            .reasoning_levels
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        match capability.as_str() {
            "unsupported" => {
                Some("Unsupported reasoning cannot have a default selection".to_string())
            }
            "toggle" if !matches!(selection.as_str(), "on" | "off" | "enabled" | "disabled") => {
                Some("Toggle reasoning default must be on or off".to_string())
            }
            "levels" if !levels.contains(&selection) => Some(format!(
                "Default reasoning must be one of the configured levels: {}",
                levels.join(", ")
            )),
            _ => None,
        }
    }

    fn review_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::Review { params } = &self.state else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                self.start_validation(params.clone());
            }
            KeyCode::Esc => {
                let params = params.clone();
                let settings = ModelSettingsDraft::from_value(
                    params.model_settings.as_ref(),
                    &params.display_name,
                );
                let advanced_open = params.model_settings.is_some();
                let input = if advanced_open {
                    settings.context_window.clone()
                } else {
                    settings.display_name.clone()
                };
                let active_field = if advanced_open {
                    ModelSettingsField::ContextWindow
                } else {
                    ModelSettingsField::DisplayName
                };
                self.state = OnboardingState::ModelSettings {
                    model: params.model_slug,
                    provider: params.invocation_method,
                    provider_id: params.provider_id,
                    provider_name: params.provider_name,
                    provider_credential_id: params.provider_credential_id,
                    base_url: params.base_url.unwrap_or_default(),
                    api_key: params.api_key.unwrap_or_default(),
                    request_model: params.request_model,
                    display_name: settings.display_name.clone(),
                    invocation_method: params.invocation_method,
                    default_reasoning_effort: params.default_reasoning_effort,
                    settings: Box::new(settings),
                    advanced_open,
                    active_field,
                    cursor_pos: Self::char_count(&input),
                    input,
                    settings_error: None,
                };
            }
            _ => {}
        }
    }

    // ── Validation Failed ──

    fn validation_failed_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ValidationFailed {
            model,
            request_model,
            display_name,
            provider,
            provider_id,
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
            base_url,
            api_key,
            error_message: _,
            recovery_hint: _,
            selected_action,
            model_settings,
        } = &mut self.state
        else {
            return;
        };

        let actions = VALIDATION_FAILED_ACTIONS;

        match key.code {
            KeyCode::Up => {
                *selected_action = if *selected_action == 0 {
                    actions.len() - 1
                } else {
                    *selected_action - 1
                };
            }
            KeyCode::Down => {
                *selected_action = (*selected_action + 1) % actions.len();
            }
            KeyCode::Enter => match *selected_action {
                0 => {
                    let result_model_slug = model.clone();
                    let result_request_model = request_model.clone();
                    let result_display_name = display_name.clone();
                    let provider = *provider;
                    let provider_id = provider_id.clone();
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();
                    let model_settings = model_settings.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    let onboarding_params = ValidationParams {
                        model_slug: result_model_slug.clone(),
                        request_model: result_request_model.clone(),
                        display_name: result_display_name.clone(),
                        provider_id: provider_id.clone(),
                        provider_name: provider_name.clone(),
                        provider_credential_id: provider_credential_id.clone(),
                        invocation_method: provider,
                        default_reasoning_effort: default_reasoning_effort.clone(),
                        model_settings: model_settings.clone(),
                        base_url: base_url.clone(),
                        api_key: api_key.clone(),
                    };
                    let (provider_info, model_id) = Self::provider_info_from_validation(
                        &onboarding_params,
                        &result_display_name,
                    );
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::ProviderUpsert {
                            params: devo_protocol::native::rpc_admin::ProviderUpsertParams {
                                provider: provider_info,
                                default_model: Some(format!("{provider_id}/{model_id}")),
                                small_model: None,
                                api_key: api_key.clone(),
                            },
                        }));
                    self.state = OnboardingState::Saving {
                        model_slug: result_model_slug,
                        request_model: result_request_model,
                        display_name: result_display_name,
                        provider_id,
                        provider_name,
                        provider_credential_id,
                        invocation_method: provider,
                        default_reasoning_effort,
                        model_settings,
                        base_url,
                        api_key,
                        bypassed: true,
                        started_at: Instant::now(),
                    };
                }
                1 => {
                    // Retry.
                    let model = model.clone();
                    let request_model = request_model.clone();
                    let display_name = display_name.clone();
                    let provider = *provider;
                    let provider_id = provider_id.clone();
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();
                    let model_settings = model_settings.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    self.start_validation(ValidationParams {
                        model_slug: model,
                        request_model,
                        display_name,
                        provider_id,
                        provider_name,
                        provider_credential_id,
                        invocation_method: provider,
                        default_reasoning_effort,
                        model_settings,
                        base_url,
                        api_key,
                    });
                }
                2 => {
                    // Edit settings, preserving any advanced values already entered.
                    let model_slug = model.clone();
                    let request_model = request_model.clone();
                    let display_name = display_name.clone();
                    let provider = *provider;
                    let provider_id = provider_id.clone();
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();
                    let base_url = base_url.clone().unwrap_or_default();
                    let api_key = api_key.clone().unwrap_or_default();
                    let settings =
                        ModelSettingsDraft::from_value(model_settings.as_ref(), &display_name);
                    self.state = OnboardingState::ModelSettings {
                        model: model_slug,
                        provider,
                        provider_id,
                        provider_name,
                        provider_credential_id,
                        base_url,
                        api_key,
                        request_model,
                        display_name: settings.display_name.clone(),
                        invocation_method: provider,
                        default_reasoning_effort,
                        advanced_open: model_settings.is_some(),
                        active_field: if model_settings.is_some() {
                            ModelSettingsField::ContextWindow
                        } else {
                            ModelSettingsField::DisplayName
                        },
                        input: if model_settings.is_some() {
                            settings.context_window.clone()
                        } else {
                            settings.display_name.clone()
                        },
                        cursor_pos: if model_settings.is_some() {
                            Self::char_count(&settings.context_window)
                        } else {
                            Self::char_count(&settings.display_name)
                        },
                        settings: Box::new(settings),
                        settings_error: None,
                    };
                }
                3 => {
                    self.go_back_to_provider_selection();
                }
                _ => {}
            },
            KeyCode::Esc => {
                self.complete = true;
                self.result = Some(OnboardingResult::Cancelled);
            }
            _ => {}
        }
    }

    // ── Rendering: Inline Setup with Vertical Rail ──
}

struct InlineSetupRenderParams<'a> {
    model: &'a str,
    supports_reasoning: bool,
    provider_name: &'a str,
    provider_credential_id: Option<&'a str>,
    base_url: &'a str,
    api_key: &'a str,
    request_model: &'a str,
    display_name: &'a str,
    active_field: Option<InlineField>,
    input: &'a str,
    cursor_pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowStepState {
    Pending,
    Active,
    Completed,
}

impl OnboardingWidget {
    const SAVED_SECRET_MASK: &'static str = "****...***";

    fn render_footer(lines: &mut Vec<Line<'static>>, primary: &str, secondary: &str) {
        lines.push(Line::from(""));
        if secondary.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(LIST_LEFT_PAD)),
                Span::styled(primary.to_string(), Style::default().dim()),
            ]));
            return;
        }
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(LIST_LEFT_PAD)),
            Span::styled(primary.to_string(), Style::default().dim()),
            Span::styled("  /  ", Style::default().dim()),
            Span::styled(secondary.to_string(), Style::default().dim()),
        ]));
    }

    fn render_option_row(
        lines: &mut Vec<Line<'static>>,
        label: String,
        description: Option<String>,
        is_selected: bool,
    ) {
        let marker = if is_selected { ">" } else { " " };
        let marker_style = if is_selected {
            Style::default().cyan().bold()
        } else {
            Style::default().dim()
        };
        let label_style = if is_selected {
            Style::default().cyan().bold().underlined()
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::raw(" ".repeat(LIST_LEFT_PAD)),
            Span::styled(marker.to_string(), marker_style),
            Span::raw(" "),
            Span::styled(label, label_style),
        ]));

        if let Some(description) = description {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(LIST_LEFT_PAD + 2)),
                Span::styled(description, Style::default().dim()),
            ]));
        }
    }

    fn scroll_overflow_line(more_above: bool) -> Line<'static> {
        // Align with option labels: LIST_LEFT_PAD + marker + following space.
        let label = if more_above {
            "... more above"
        } else {
            "... more below"
        };
        Line::from(vec![
            Span::raw(" ".repeat(LIST_LEFT_PAD + 2)),
            Span::styled(label.to_string(), Style::default().dim()),
        ])
    }

    fn render_inline_setup_header(lines: &mut Vec<Line<'static>>, model: &str) {
        lines.push(Line::from(vec![Span::styled(
            "Configure Connection",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!("Model profile: {model}"),
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));
    }

    fn render_inline_setup_fields(
        lines: &mut Vec<Line<'static>>,
        params: &InlineSetupRenderParams,
    ) -> Option<ViewportAnchor> {
        let mut anchor = None;
        let field_anchor = Self::render_inline_field(
            lines,
            params,
            InlineField::ProviderName,
            "Provider Name",
            "Enter a name to recognize this provider later.",
            params.provider_name,
            false,
        );
        if field_anchor.is_some() {
            anchor = field_anchor;
        }
        let field_anchor = Self::render_inline_field(
            lines,
            params,
            InlineField::BaseUrl,
            "Base URL",
            "Enter the provider API base URL.",
            params.base_url,
            false,
        );
        if field_anchor.is_some() {
            anchor = field_anchor;
        }
        let field_anchor = Self::render_inline_field(
            lines,
            params,
            InlineField::ApiKey,
            "API Key",
            "Enter the API key for this provider.",
            params.api_key,
            true,
        );
        if field_anchor.is_some() {
            anchor = field_anchor;
        }
        let field_anchor = Self::render_inline_field(
            lines,
            params,
            InlineField::RequestModel,
            "Request Model",
            "Enter the model identifier this provider expects.",
            params.request_model,
            false,
        );
        if field_anchor.is_some() {
            anchor = field_anchor;
        }
        let field_anchor = Self::render_inline_field(
            lines,
            params,
            InlineField::DisplayName,
            "Display Name",
            "Enter the name clients should show for this model.",
            params.display_name,
            false,
        );
        if field_anchor.is_some() {
            anchor = field_anchor;
        }
        anchor
    }

    fn render_inline_setup(params: &InlineSetupRenderParams, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);

        let mut lines: Vec<Line<'static>> = Vec::new();

        Self::render_inline_setup_header(&mut lines, params.model);
        let anchor = Self::render_inline_setup_fields(&mut lines, params);
        Self::render_workflow_step(
            &mut lines,
            "Invocation Method",
            "Choose the API protocol.",
            "[open popup]",
            WorkflowStepState::Pending,
        );
        if params.supports_reasoning {
            Self::render_workflow_step(
                &mut lines,
                "Reason Effort",
                "Choose the default reasoning effort for this model. It can be changed with /model.",
                "[open popup]",
                WorkflowStepState::Pending,
            );
        }
        Self::render_workflow_step(
            &mut lines,
            "Validation Done",
            "",
            "",
            WorkflowStepState::Pending,
        );

        Self::render_footer(&mut lines, "Enter next field", "Esc back");

        render_lines_with_anchor(lines, anchor, content_area, buf);
    }

    fn render_inline_field(
        lines: &mut Vec<Line<'static>>,
        params: &InlineSetupRenderParams,
        field: InlineField,
        label: &str,
        hint: &str,
        value: &str,
        secret: bool,
    ) -> Option<ViewportAnchor> {
        let start = lines.len();
        let active_index = params
            .active_field
            .map(Self::inline_field_index)
            .unwrap_or(usize::MAX);
        let field_index = Self::inline_field_index(field);
        let is_active = params.active_field == Some(field);
        let is_done = params.active_field.is_none() || field_index < active_index;
        let rail_style = if is_active {
            Style::default().cyan().bold()
        } else if is_done {
            Style::default().green()
        } else {
            Style::default().dim()
        };
        let label_style = if is_active {
            Style::default().bold()
        } else {
            Style::default().dim()
        };
        let has_saved_secret = secret && params.provider_credential_id.is_some();
        let shown_value = if is_active {
            Self::input_with_cursor(params.input, params.cursor_pos)
        } else if secret && (!value.is_empty() || has_saved_secret) {
            Self::SAVED_SECRET_MASK.to_string()
        } else if value.is_empty() && is_done {
            "(skip)".to_string()
        } else if value.is_empty() {
            "...".to_string()
        } else {
            value.to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(if is_active { "> " } else { "  " }, rail_style),
            Span::styled(format!("{label}: "), label_style),
            Span::styled(
                shown_value,
                if is_active {
                    Style::default()
                } else {
                    Style::default().dim()
                },
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(hint.to_string(), Style::default().dim()),
        ]));
        lines.push(Line::from(""));
        if is_active {
            Some(ViewportAnchor {
                start,
                end: start.saturating_add(2),
            })
        } else {
            None
        }
    }

    fn render_workflow_step(
        lines: &mut Vec<Line<'static>>,
        label: &str,
        hint: &str,
        value: &str,
        step_state: WorkflowStepState,
    ) {
        let rail_style = match step_state {
            WorkflowStepState::Pending => Style::default().dim(),
            WorkflowStepState::Active => Style::default().cyan().bold(),
            WorkflowStepState::Completed => Style::default().green(),
        };
        let label_style = match step_state {
            WorkflowStepState::Pending => Style::default().dim(),
            WorkflowStepState::Active => Style::default().bold(),
            WorkflowStepState::Completed => Style::default(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                match step_state {
                    WorkflowStepState::Active => "> ",
                    WorkflowStepState::Completed => "  ",
                    WorkflowStepState::Pending => "  ",
                },
                rail_style,
            ),
            Span::styled(format!("{label}: "), label_style),
            Span::styled(
                value.to_string(),
                match step_state {
                    WorkflowStepState::Pending => Style::default().dim(),
                    WorkflowStepState::Active => Style::default(),
                    WorkflowStepState::Completed => Style::default().green(),
                },
            ),
        ]));
        if !hint.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(hint.to_string(), Style::default().dim()),
            ]));
        }
        lines.push(Line::from(""));
    }

    fn render_inline_popup_option(
        lines: &mut Vec<Line<'static>>,
        label: &str,
        description: &str,
        is_selected: bool,
    ) {
        let marker_style = if is_selected {
            Style::default().cyan().bold()
        } else {
            Style::default().dim()
        };
        let label_style = if is_selected {
            Style::default().cyan().bold().underlined()
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(if is_selected { ">" } else { " " }, marker_style),
            Span::raw(" "),
            Span::styled(label.to_string(), label_style),
        ]));
        if !description.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(description.to_string(), Style::default().dim()),
            ]));
        }
    }

    fn render_invocation_method_inline(
        params: &InlineSetupRenderParams,
        items: &[InvocationMethodItem],
        selected_idx: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        Self::render_inline_setup_header(&mut lines, params.model);
        let _ = Self::render_inline_setup_fields(&mut lines, params);
        let active_step_start = lines.len();
        Self::render_workflow_step(
            &mut lines,
            "Invocation Method",
            "Choose the API protocol.",
            items
                .get(selected_idx)
                .map(|item| item.label.as_str())
                .unwrap_or("[open popup]"),
            WorkflowStepState::Active,
        );
        let mut anchor = ViewportAnchor {
            start: active_step_start,
            end: lines.len(),
        };
        for (idx, item) in items.iter().enumerate() {
            Self::render_inline_popup_option(
                &mut lines,
                &item.label,
                &item.description,
                idx == selected_idx,
            );
            if idx == selected_idx {
                anchor.end = lines.len();
            }
        }
        if params.supports_reasoning {
            Self::render_workflow_step(
                &mut lines,
                "Reason Effort",
                "Choose the default reasoning effort for this model. It can be changed with /model.",
                "[open popup]",
                WorkflowStepState::Pending,
            );
        }
        Self::render_workflow_step(
            &mut lines,
            "Validation Done",
            "",
            "",
            WorkflowStepState::Pending,
        );
        Self::render_footer(&mut lines, "Enter select", "Esc back");

        render_lines_with_anchor(lines, Some(anchor), content_area, buf);
    }

    fn render_reasoning_effort_inline(
        params: &InlineSetupRenderParams,
        invocation_method: ProviderWireApi,
        items: &[ReasoningEffortItem],
        selected_idx: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        Self::render_inline_setup_header(&mut lines, params.model);
        let _ = Self::render_inline_setup_fields(&mut lines, params);
        Self::render_workflow_step(
            &mut lines,
            "Invocation Method",
            "Choose the API protocol.",
            &Self::invocation_method_label(invocation_method),
            WorkflowStepState::Completed,
        );
        let active_step_start = lines.len();
        Self::render_workflow_step(
            &mut lines,
            "Reason Effort",
            "Choose the default reasoning effort for this model. It can be changed with /model.",
            items
                .get(selected_idx)
                .map(|item| item.label.as_str())
                .unwrap_or("[open popup]"),
            WorkflowStepState::Active,
        );
        let mut anchor = ViewportAnchor {
            start: active_step_start,
            end: lines.len(),
        };
        for (idx, item) in items.iter().enumerate() {
            Self::render_inline_popup_option(
                &mut lines,
                &item.label,
                &item.description,
                idx == selected_idx,
            );
            if idx == selected_idx {
                anchor.end = lines.len();
            }
        }
        Self::render_workflow_step(
            &mut lines,
            "Validation Done",
            "",
            "",
            WorkflowStepState::Pending,
        );
        Self::render_footer(&mut lines, "Enter select", "Esc back");

        render_lines_with_anchor(lines, Some(anchor), content_area, buf);
    }

    fn input_with_cursor(input: &str, cursor_pos: usize) -> String {
        let byte_pos = Self::byte_index_for_char(input, cursor_pos);
        format!("{}|{}", &input[..byte_pos], &input[byte_pos..])
    }

    fn inline_field_index(field: InlineField) -> usize {
        match field {
            InlineField::ProviderName => 0,
            InlineField::BaseUrl => 1,
            InlineField::ApiKey => 2,
            InlineField::RequestModel => 3,
            InlineField::DisplayName => 4,
        }
    }

    // ── Rendering: Popup Lists ──

    fn render_onboarding_header(
        title: &str,
        subtitle: &str,
        active_step: &str,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let provider_active = active_step == "Provider";
        let model_active = active_step == "Model";
        let settings_active = active_step == "Settings";
        let review_active = active_step == "Review";
        let lines = vec![
            Line::from(vec![Span::styled(
                "devo / Set up a model",
                Style::default().bold(),
            )]),
            Line::from(vec![
                Span::styled(
                    "1 Provider",
                    if provider_active {
                        Style::default().cyan().bold()
                    } else {
                        Style::default().green()
                    },
                ),
                Span::styled("  /  ", Style::default().dim()),
                Span::styled(
                    "2 Model",
                    if model_active {
                        Style::default().cyan().bold()
                    } else if settings_active || review_active {
                        Style::default().green()
                    } else {
                        Style::default().dim()
                    },
                ),
                Span::styled("  /  ", Style::default().dim()),
                Span::styled(
                    "3 Settings",
                    if settings_active {
                        Style::default().cyan().bold()
                    } else if review_active {
                        Style::default().green()
                    } else {
                        Style::default().dim()
                    },
                ),
                Span::styled("  /  ", Style::default().dim()),
                Span::styled(
                    "4 Review",
                    if review_active {
                        Style::default().cyan().bold()
                    } else {
                        Style::default().dim()
                    },
                ),
            ]),
            Line::from(vec![Span::styled(
                title.to_string(),
                Style::default().bold(),
            )]),
            Line::from(vec![Span::styled(
                subtitle.to_string(),
                Style::default().dim(),
            )]),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_custom_card(
        title: &str,
        description: &str,
        focused: bool,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let accent = if focused {
            Style::default().cyan().bold()
        } else {
            Style::default().dim()
        };
        let title_style = if focused {
            Style::default().cyan().bold().underlined()
        } else {
            Style::default().bold()
        };
        let lines = vec![
            Line::from(vec![
                Span::styled(if focused { "> " } else { "  " }, accent),
                Span::styled(title.to_string(), title_style),
            ]),
            Line::from(vec![
                Span::styled("  ", accent),
                Span::styled(description.to_string(), Style::default().dim()),
            ]),
            Line::from(vec![Span::styled("  ", accent)]),
            Line::from(vec![
                Span::styled("  ", accent),
                Span::styled("Press Enter", Style::default().bold()),
                Span::styled(" to configure", Style::default().dim()),
            ]),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn selection_areas(body_area: Rect) -> (Rect, Rect) {
        if body_area.width >= 80 {
            let [list_area, _, custom_area] = Layout::horizontal([
                Constraint::Percentage(58),
                Constraint::Length(3),
                Constraint::Fill(1),
            ])
            .areas(body_area);
            (list_area, custom_area)
        } else {
            let custom_height = body_area.height.min(6);
            let [list_area, custom_area] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(custom_height)])
                    .areas(body_area);
            (list_area, custom_area)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_model_selection(
        items: &[ModelSelectionItem],
        state: &ScrollState,
        search_query: &str,
        filtered_indices: &[usize],
        focus: SelectionFocus,
        manage_connection: bool,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        Self::render_onboarding_header(
            if manage_connection {
                "Models in this Connection"
            } else {
                "Choose a model"
            },
            if manage_connection {
                "Choose an existing model, remove it, or add a custom model."
            } else {
                "Select a model for the chosen provider, or use a custom model id."
            },
            "Model",
            header_area,
            buf,
        );

        let max_visible = MAX_POPUP_ROWS.min(filtered_indices.len().max(1));
        let scroll_offset = state
            .scroll_top
            .min(filtered_indices.len().saturating_sub(max_visible));
        let has_more_above = scroll_offset > 0;
        let has_more_below = scroll_offset + max_visible < filtered_indices.len();
        let mut list_lines: Vec<Line<'static>> = vec![Line::from(vec![
            Span::styled("Search  ", Style::default().dim()),
            Span::styled(
                if search_query.is_empty() {
                    "type to filter"
                } else {
                    search_query
                }
                .to_string(),
                if search_query.is_empty() {
                    Style::default().dim()
                } else {
                    Style::default()
                },
            ),
        ])];
        let mut anchor = None;

        if has_more_above {
            list_lines.push(Self::scroll_overflow_line(/*more_above*/ true));
        }
        for (vis_idx, &actual_idx) in filtered_indices
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(max_visible)
        {
            if let Some(item) = items.get(actual_idx) {
                let is_selected = state.selected_idx == Some(vis_idx);
                let start = list_lines.len();
                Self::render_option_row(
                    &mut list_lines,
                    item.slug.clone(),
                    Some(item.display_name.clone()),
                    is_selected && focus == SelectionFocus::List,
                );
                if is_selected {
                    anchor = Some(ViewportAnchor {
                        start,
                        end: list_lines.len(),
                    });
                }
            }
        }
        if has_more_below {
            list_lines.push(Self::scroll_overflow_line(/*more_above*/ false));
        }
        let (list_area, custom_area) = Self::selection_areas(body_area);
        render_lines_with_anchor(list_lines, anchor, list_area, buf);
        Self::render_custom_card(
            "Add custom model profile",
            "Define the provider model ID, limits, capabilities, and request behavior.",
            focus == SelectionFocus::Custom,
            custom_area,
            buf,
        );
        let mut footer_spans = vec![
            Span::styled("Up/Down", Style::default().bold()),
            Span::styled(" navigate   ", Style::default().dim()),
            Span::styled("Tab", Style::default().bold()),
            Span::styled(" custom   ", Style::default().dim()),
            Span::styled("Enter", Style::default().bold()),
            Span::styled(" select   ", Style::default().dim()),
        ];
        if manage_connection {
            footer_spans.extend([
                Span::styled("d/Delete", Style::default().bold()),
                Span::styled(" remove model   ", Style::default().dim()),
            ]);
        }
        footer_spans.extend([
            Span::styled("Esc", Style::default().bold()),
            Span::styled(" back", Style::default().dim()),
        ]);
        let footer_lines = vec![Line::from(footer_spans)];
        Paragraph::new(footer_lines).render(footer_area, buf);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_custom_model_form(
        provider_name: &str,
        model_id: &str,
        display_name: &str,
        active_field: CustomModelField,
        input: &str,
        cursor_pos: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        Self::render_onboarding_header(
            "Add a custom model",
            &format!(
                "For {provider_name} · next configure API, limits, capabilities, and options."
            ),
            "Custom model",
            header_area,
            buf,
        );

        let mut lines = Vec::new();
        Self::render_custom_model_field(
            &mut lines,
            CustomModelField::ModelId,
            "Provider model ID",
            model_id,
            active_field,
            input,
            cursor_pos,
            "Required · sent verbatim to the provider; this is not a Devo slug",
        );
        Self::render_custom_model_field(
            &mut lines,
            CustomModelField::DisplayName,
            "Display name",
            display_name,
            active_field,
            input,
            cursor_pos,
            "Optional · a friendly name shown in devo",
        );
        lines.push(Line::from(vec![Span::styled(
            "Next: API method, limits, capabilities, reasoning, variants, and request controls",
            Style::default().dim(),
        )]));
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body_area, buf);

        let footer = vec![Line::from(vec![
            Span::styled("Enter/Tab", Style::default().bold()),
            Span::styled(" next   ", Style::default().dim()),
            Span::styled("Esc", Style::default().bold()),
            Span::styled(" back", Style::default().dim()),
        ])];
        Paragraph::new(footer).render(footer_area, buf);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_custom_model_field(
        lines: &mut Vec<Line<'static>>,
        field: CustomModelField,
        label: &str,
        value: &str,
        active_field: CustomModelField,
        input: &str,
        cursor_pos: usize,
        hint: &str,
    ) {
        let active = field == active_field;
        let displayed = if active {
            let byte_pos = Self::byte_index_for_char(input, cursor_pos);
            format!("{}|{}", &input[..byte_pos], &input[byte_pos..])
        } else if value.is_empty() {
            "(optional)".to_string()
        } else {
            value.to_string()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if active { "> " } else { "  " },
                if active {
                    Style::default().cyan()
                } else {
                    Style::default().dim()
                },
            ),
            Span::styled(format!("{label}: "), Style::default().bold()),
            Span::raw(displayed),
        ]));
        lines.push(Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(hint.to_string(), Style::default().dim()),
        ]));
        lines.push(Line::from(""));
    }

    fn render_provider_setup(
        draft: &ProviderDraft,
        active_field: InlineField,
        input: &str,
        cursor_pos: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        let title = if draft.is_custom {
            "Add a custom provider".to_string()
        } else {
            format!("Connect to {}", draft.provider_name)
        };
        let subtitle = if draft.is_custom {
            "Enter the connection details for this provider."
        } else {
            "Review the template, then create your Connection."
        };
        Self::render_onboarding_header(&title, subtitle, "Provider", header_area, buf);

        let values = [
            (
                InlineField::ProviderName,
                "Provider Name",
                if draft.is_custom {
                    "A short name you will recognize later."
                } else {
                    "From the provider directory (read-only)."
                },
                draft.provider_name.as_str(),
            ),
            (
                InlineField::BaseUrl,
                "Base URL",
                if draft.is_custom {
                    "The provider API endpoint."
                } else {
                    "Fixed by the provider directory template."
                },
                draft.base_url.as_str(),
            ),
            (
                InlineField::ApiKey,
                "API Key",
                if !draft.is_custom {
                    "Enter once to create this Connection; change it by disconnecting first."
                } else if draft.provider_credential_id.is_some() {
                    "Leave blank to keep the current key. Stored securely in auth.json."
                } else {
                    "Stored securely in auth.json."
                },
                draft.api_key.as_str(),
            ),
        ];
        let mut lines = Vec::new();
        for (field, label, hint, value) in values {
            let is_active = field == active_field;
            let shown = if is_active {
                if field == InlineField::ApiKey {
                    format!("{}|", "*".repeat(Self::char_count(input)))
                } else {
                    Self::input_with_cursor(input, cursor_pos)
                }
            } else if field == InlineField::ApiKey && !value.is_empty() {
                Self::SAVED_SECRET_MASK.to_string()
            } else if value.is_empty() {
                "...".to_string()
            } else {
                value.to_string()
            };
            let style = if is_active {
                Style::default().cyan().bold()
            } else {
                Style::default().dim()
            };
            lines.push(Line::from(vec![
                Span::styled(if is_active { "> " } else { "  " }, style),
                Span::styled(
                    format!("{label}: "),
                    if is_active {
                        Style::default().bold()
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    shown,
                    if is_active {
                        Style::default()
                    } else {
                        Style::default().dim()
                    },
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(hint.to_string(), Style::default().dim()),
            ]));
            lines.push(Line::from(""));
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body_area, buf);
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Enter", Style::default().bold()),
            Span::styled(
                if draft.is_custom {
                    " next   "
                } else {
                    " create Connection   "
                },
                Style::default().dim(),
            ),
            Span::styled("Esc", Style::default().bold()),
            Span::styled(" back", Style::default().dim()),
        ])])
        .render(footer_area, buf);
    }

    fn render_setting_line(
        lines: &mut Vec<Line<'static>>,
        field: ModelSettingsField,
        label: &str,
        value: String,
        active_field: ModelSettingsField,
        input: &str,
        cursor_pos: usize,
    ) {
        let active = field == active_field;
        let shown = if active
            && !matches!(field, ModelSettingsField::AdvancedToggle)
            && !matches!(
                field,
                ModelSettingsField::OriginalImageDetail | ModelSettingsField::Enabled
            ) {
            Self::input_with_cursor(input, cursor_pos)
        } else if value.is_empty() {
            "default".to_string()
        } else {
            value
        };
        let marker_style = if active {
            Style::default().cyan().bold()
        } else {
            Style::default().dim()
        };
        lines.push(Line::from(vec![
            Span::styled(if active { "> " } else { "  " }, marker_style),
            Span::styled(
                format!("{label}: "),
                if active {
                    Style::default().bold()
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                shown,
                if active {
                    Style::default().cyan()
                } else {
                    Style::default().dim()
                },
            ),
        ]));
    }

    fn model_settings_basic_anchor(active_field: ModelSettingsField) -> Option<ViewportAnchor> {
        let start = match active_field {
            ModelSettingsField::DisplayName => 3,
            ModelSettingsField::ContextWindow => 5,
            ModelSettingsField::MaxTokens => 7,
            ModelSettingsField::Temperature => 8,
            ModelSettingsField::InputModalities => 9,
            ModelSettingsField::ReasoningCapability => 10,
            ModelSettingsField::DefaultReasoning => 11,
            ModelSettingsField::AdvancedToggle => 13,
            _ => return None,
        };
        Some(ViewportAnchor {
            start,
            end: start.saturating_add(1),
        })
    }

    fn model_settings_advanced_anchor(
        active_field: ModelSettingsField,
        basic_line_count: usize,
    ) -> Option<ViewportAnchor> {
        let advanced_fields = [
            ModelSettingsField::TopP,
            ModelSettingsField::TopK,
            ModelSettingsField::Family,
            ModelSettingsField::ReleaseDate,
            ModelSettingsField::Status,
            ModelSettingsField::CapabilitiesJson,
            ModelSettingsField::Channel,
            ModelSettingsField::BaseInstructions,
            ModelSettingsField::ReasoningImplementation,
            ModelSettingsField::ReasoningLevels,
            ModelSettingsField::ReasoningVariantsJson,
            ModelSettingsField::DefaultVariant,
            ModelSettingsField::CostJson,
            ModelSettingsField::MetadataJson,
            ModelSettingsField::RequestJson,
            ModelSettingsField::OptionsJson,
            ModelSettingsField::HeadersJson,
            ModelSettingsField::VariantsJson,
            ModelSettingsField::WebSearchJson,
            ModelSettingsField::WebFetchJson,
            ModelSettingsField::TruncationMode,
            ModelSettingsField::TruncationLimit,
            ModelSettingsField::OriginalImageDetail,
            ModelSettingsField::Enabled,
            ModelSettingsField::Priority,
        ];
        let index = advanced_fields
            .iter()
            .position(|field| *field == active_field)?;
        let start = basic_line_count.saturating_add(3).saturating_add(index);
        Some(ViewportAnchor {
            start,
            end: start.saturating_add(1),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_model_settings(
        model: &str,
        provider_name: &str,
        request_model: &str,
        display_name: &str,
        invocation_method: ProviderWireApi,
        default_reasoning_effort: Option<&str>,
        settings: &ModelSettingsDraft,
        advanced_open: bool,
        active_field: ModelSettingsField,
        input: &str,
        cursor_pos: usize,
        settings_error: Option<&str>,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        let subtitle = format!(
            "Profile: {model}  /  Keep common choices simple; expand advanced settings when needed."
        );
        Self::render_onboarding_header("Configure model", &subtitle, "Settings", header_area, buf);

        let mut basic_lines = Vec::new();
        basic_lines.push(Line::from(vec![
            Span::styled("Model ID: ", Style::default().bold()),
            Span::styled(request_model.to_string(), Style::default().dim()),
        ]));
        basic_lines.push(Line::from(vec![
            Span::styled("Provider: ", Style::default().bold()),
            Span::styled(provider_name.to_string(), Style::default().dim()),
        ]));
        basic_lines.push(Line::from(""));
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::DisplayName,
            "Display label",
            display_name.to_string(),
            active_field,
            input,
            cursor_pos,
        );
        basic_lines.push(Line::from(vec![
            Span::styled("  Protocol: ", Style::default()),
            Span::styled(
                Self::invocation_method_label(invocation_method),
                Style::default().dim(),
            ),
        ]));
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::ContextWindow,
            "Context window (tokens)",
            settings.context_window.clone(),
            active_field,
            input,
            cursor_pos,
        );
        basic_lines.push(Line::from(vec![Span::styled(
            "  Usable tokens; stored as % of model capacity. Occupancy and auto-compact follow it.",
            Style::default().dim(),
        )]));
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::MaxTokens,
            "Max output (tokens)",
            settings.max_tokens.clone(),
            active_field,
            input,
            cursor_pos,
        );
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::Temperature,
            "Temperature",
            settings.temperature.clone(),
            active_field,
            input,
            cursor_pos,
        );
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::InputModalities,
            "Input modalities (text, image)",
            settings.input_modalities.clone(),
            active_field,
            input,
            cursor_pos,
        );
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::ReasoningCapability,
            "Reasoning capability (unsupported, toggle, levels)",
            settings.reasoning_capability.clone(),
            active_field,
            input,
            cursor_pos,
        );
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::DefaultReasoning,
            "Default reasoning (enabled, disabled, or effort)",
            default_reasoning_effort.unwrap_or_default().to_string(),
            active_field,
            input,
            cursor_pos,
        );
        basic_lines.push(Line::from(""));
        Self::render_setting_line(
            &mut basic_lines,
            ModelSettingsField::AdvancedToggle,
            "Advanced settings",
            if advanced_open {
                "[open]".to_string()
            } else {
                "[closed] optional overrides".to_string()
            },
            active_field,
            input,
            cursor_pos,
        );

        let mut advanced_lines = Vec::new();
        if advanced_open {
            advanced_lines.push(Line::from(vec![Span::styled(
                "Optional provider defaults",
                Style::default().bold(),
            )]));
            advanced_lines.push(Line::from(vec![Span::styled(
                "Leave blank for defaults. JSON: request/options/headers/variants are provider escape hatches.",
                Style::default().dim(),
            )]));
            advanced_lines.push(Line::from(""));
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::TopP,
                "Top P",
                settings.top_p.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::TopK,
                "Top K",
                settings.top_k.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::Family,
                "Family",
                settings.family.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::ReleaseDate,
                "Release date",
                settings.release_date.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::Status,
                "Status",
                settings.status.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::CapabilitiesJson,
                "Capabilities JSON",
                settings.capabilities_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::Channel,
                "Channel",
                settings.channel.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::BaseInstructions,
                "Base instructions",
                settings.base_instructions.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::ReasoningImplementation,
                "Reasoning implementation (legacy; prefer Variants JSON)",
                settings.reasoning_implementation.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::ReasoningLevels,
                "Reasoning levels (off, low, medium, high, max)",
                settings.reasoning_levels.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::ReasoningVariantsJson,
                "Reasoning variant rules JSON (legacy model_variant)",
                settings.reasoning_variants_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::DefaultVariant,
                "Default variant (static fallback when effort has no matching key)",
                settings.default_variant.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::CostJson,
                "Cost JSON",
                settings.cost_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::MetadataJson,
                "Metadata JSON",
                settings.metadata_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::RequestJson,
                "Request JSON",
                settings.request_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::OptionsJson,
                "Options JSON",
                settings.options_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::HeadersJson,
                "Headers JSON",
                settings.headers_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::VariantsJson,
                "Variants JSON (keys = off/on/effort; request_model/request/options/headers)",
                settings.variants_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::WebSearchJson,
                "Web search JSON",
                settings.web_search_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::WebFetchJson,
                "Web fetch JSON",
                settings.web_fetch_json.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::TruncationMode,
                "Truncation mode",
                settings.truncation_mode.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::TruncationLimit,
                "Truncation limit",
                settings.truncation_limit.clone(),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::OriginalImageDetail,
                "Original image detail",
                settings
                    .supports_image_detail_original
                    .map_or_else(|| "default".to_string(), |value| value.to_string()),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::Enabled,
                "Enabled",
                settings
                    .enabled
                    .map_or_else(|| "default".to_string(), |value| value.to_string()),
                active_field,
                input,
                cursor_pos,
            );
            Self::render_setting_line(
                &mut advanced_lines,
                ModelSettingsField::Priority,
                "Priority",
                settings.priority.clone(),
                active_field,
                input,
                cursor_pos,
            );
        }

        if let Some(error) = settings_error {
            basic_lines.push(Line::from(vec![
                Span::styled("Error: ", Style::default().red().bold()),
                Span::styled(error.to_string(), Style::default().red()),
            ]));
        }

        let basic_anchor = Self::model_settings_basic_anchor(active_field);
        let advanced_anchor = Self::model_settings_advanced_anchor(active_field, 0);
        if body_area.width >= 88 {
            let [basic_area, advanced_area] =
                Layout::horizontal([Constraint::Percentage(44), Constraint::Fill(1)])
                    .areas(body_area);
            render_lines_with_anchor(basic_lines, basic_anchor, basic_area, buf);
            render_lines_with_anchor(advanced_lines, advanced_anchor, advanced_area, buf);
        } else {
            let advanced_anchor = advanced_anchor.map(|anchor| ViewportAnchor {
                start: basic_lines.len().saturating_add(anchor.start),
                end: basic_lines.len().saturating_add(anchor.end),
            });
            basic_lines.extend(advanced_lines);
            let anchor = if advanced_open {
                advanced_anchor
            } else {
                basic_anchor
            };
            render_lines_with_anchor(basic_lines, anchor, body_area, buf);
        }

        let footer = if advanced_open {
            vec![Line::from(vec![
                Span::styled("Enter", Style::default().bold()),
                Span::styled(" next   ", Style::default().dim()),
                Span::styled("Space", Style::default().bold()),
                Span::styled(" toggle   ", Style::default().dim()),
                Span::styled("Esc", Style::default().bold()),
                Span::styled(" back", Style::default().dim()),
            ])]
        } else {
            vec![Line::from(vec![
                Span::styled("Enter", Style::default().bold()),
                Span::styled(" review   ", Style::default().dim()),
                Span::styled("Tab", Style::default().bold()),
                Span::styled(" edit common fields   ", Style::default().dim()),
                Span::styled("Space", Style::default().bold()),
                Span::styled(" expand advanced   ", Style::default().dim()),
                Span::styled("Esc", Style::default().bold()),
                Span::styled(" back", Style::default().dim()),
            ])]
        };
        Paragraph::new(footer).render(footer_area, buf);
    }

    fn render_review(params: &ValidationParams, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        Self::render_onboarding_header(
            "Review model setup",
            "Everything looks ready. Confirm to test the connection and save it.",
            "Review",
            header_area,
            buf,
        );
        let credential = if params.api_key.is_some() {
            "configured"
        } else {
            "environment or existing auth"
        };
        let advanced = if params.model_settings.is_some() {
            "custom overrides"
        } else {
            "catalog defaults"
        };
        let lines = vec![
            Line::from(vec![
                Span::styled("Provider       ", Style::default().bold()),
                Span::raw(params.provider_name.clone()),
            ]),
            Line::from(vec![
                Span::styled("Model profile  ", Style::default().bold()),
                Span::raw(params.model_slug.clone()),
            ]),
            Line::from(vec![
                Span::styled("Request model  ", Style::default().bold()),
                Span::raw(params.request_model.clone()),
            ]),
            Line::from(vec![
                Span::styled("Protocol       ", Style::default().bold()),
                Span::raw(Self::invocation_method_label(params.invocation_method)),
            ]),
            Line::from(vec![
                Span::styled("Reasoning      ", Style::default().bold()),
                Span::raw(
                    params
                        .default_reasoning_effort
                        .as_deref()
                        .unwrap_or("unsupported"),
                ),
            ]),
            Line::from(vec![
                Span::styled("Credential     ", Style::default().bold()),
                Span::styled(credential, Style::default().green()),
            ]),
            Line::from(vec![
                Span::styled("Model settings ", Style::default().bold()),
                Span::raw(advanced),
            ]),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body_area, buf);
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Enter", Style::default().bold()),
            Span::styled(" test & save   ", Style::default().dim()),
            Span::styled("Esc", Style::default().bold()),
            Span::styled(" edit settings", Style::default().dim()),
        ])])
        .render(footer_area, buf);
    }

    fn render_provider_selection(
        items: &[ProviderSelectionItem],
        selected_idx: usize,
        focus: SelectionFocus,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(content_area);
        Self::render_onboarding_header(
            "Choose a provider",
            "Select a configured endpoint, or add your own provider.",
            "Provider",
            header_area,
            buf,
        );

        let (list_area, custom_area) = Self::selection_areas(body_area);
        let mut lines = Vec::new();
        let connection_count = items
            .iter()
            .filter(|item| item.section == ProviderSelectionSection::Connections)
            .count();
        let template_count = items
            .iter()
            .filter(|item| item.section == ProviderSelectionSection::Templates)
            .count();

        lines.push(Line::from(vec![Span::styled(
            "Connections",
            Style::default().bold(),
        )]));
        if connection_count == 0 {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("No saved Connections yet.", Style::default().dim()),
            ]));
        } else {
            for (idx, item) in items.iter().enumerate() {
                if item.section == ProviderSelectionSection::Connections {
                    Self::render_option_row(
                        &mut lines,
                        item.label.clone(),
                        Some(item.description.clone()),
                        idx == selected_idx && focus == SelectionFocus::List,
                    );
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Provider templates",
            Style::default().bold(),
        )]));
        if template_count == 0 {
            lines.push(Line::from(vec![Span::styled(
                "No provider templates available.",
                Style::default().dim(),
            )]));
        } else {
            for (idx, item) in items.iter().enumerate() {
                if item.section == ProviderSelectionSection::Templates {
                    Self::render_option_row(
                        &mut lines,
                        item.label.clone(),
                        Some(item.description.clone()),
                        idx == selected_idx && focus == SelectionFocus::List,
                    );
                }
            }
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(list_area, buf);
        Self::render_custom_card(
            "Add custom provider",
            "Configure an endpoint, protocol and credential.",
            focus == SelectionFocus::Custom,
            custom_area,
            buf,
        );
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Up/Down", Style::default().bold()),
            Span::styled(" navigate   ", Style::default().dim()),
            Span::styled("Tab", Style::default().bold()),
            Span::styled(" custom   ", Style::default().dim()),
            Span::styled("Enter", Style::default().bold()),
            Span::styled(" select   ", Style::default().dim()),
            Span::styled("Esc", Style::default().bold()),
            Span::styled(" cancel", Style::default().dim()),
            Span::styled("   ", Style::default().dim()),
            Span::styled("d/Delete", Style::default().bold()),
            Span::styled(" disconnect selected Connection", Style::default().dim()),
        ])])
        .render(footer_area, buf);
    }

    fn render_disconnect_confirmation(provider: &ProviderInfo, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines = vec![
            Line::from(vec![Span::styled(
                format!("Disconnect {}", provider.name),
                Style::default().bold(),
            )]),
            Line::from(vec![Span::styled(
                "This removes the saved Connection and its unshared credential.",
                Style::default().dim(),
            )]),
            Line::from(vec![Span::styled(
                "The provider directory template will remain available.",
                Style::default().dim(),
            )]),
            Line::from(""),
        ];
        Self::render_footer(&mut lines, "Enter disconnect", "Esc cancel");
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_disconnecting(provider_name: &str, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let lines = vec![
            Line::from(vec![Span::styled(
                format!("Disconnecting {provider_name}"),
                Style::default().bold(),
            )]),
            Line::from(vec![Span::styled(
                "Removing the user Connection and refreshing the provider directory...",
                Style::default().dim(),
            )]),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_model_delete_confirmation(
        model_name: &str,
        provider_name: &str,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines = vec![
            Line::from(vec![Span::styled(
                format!("Remove {model_name} from {provider_name}"),
                Style::default().bold(),
            )]),
            Line::from(vec![Span::styled(
                "This removes the model from the saved Connection.",
                Style::default().dim(),
            )]),
            Line::from(vec![Span::styled(
                "The provider template and its built-in directory remain unchanged.",
                Style::default().dim(),
            )]),
            Line::from(""),
        ];
        Self::render_footer(&mut lines, "Enter remove", "Esc cancel");
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_model_deleting(model_name: &str, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let lines = vec![
            Line::from(vec![Span::styled(
                format!("Removing {model_name}"),
                Style::default().bold(),
            )]),
            Line::from(vec![Span::styled(
                "Updating the saved Connection…",
                Style::default().dim(),
            )]),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_invocation_method(
        items: &[InvocationMethodItem],
        selected_idx: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            "Choose wire API",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "This is the provider protocol, not the provider vendor.",
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));

        for (idx, item) in items.iter().enumerate() {
            let is_selected = idx == selected_idx;
            Self::render_option_row(
                &mut lines,
                item.label.clone(),
                Some(item.description.clone()),
                is_selected,
            );
        }

        Self::render_footer(&mut lines, "Enter select", "Esc back");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_reasoning_effort(
        items: &[ReasoningEffortItem],
        selected_idx: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            "Default reasoning",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Choose the effort stored on this model profile.",
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));

        for (idx, item) in items.iter().enumerate() {
            let is_selected = idx == selected_idx;
            Self::render_option_row(&mut lines, item.label.clone(), None, is_selected);
        }

        Self::render_footer(&mut lines, "Enter select", "Esc back");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_validating(
        model: &str,
        provider: ProviderWireApi,
        started_at: Instant,
        animations_enabled: bool,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let provider_name = Self::provider_display_name(provider);
        let elapsed = started_at.elapsed().as_secs();
        let remaining = 20u64.saturating_sub(elapsed);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Testing Connection",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!("model: {model}  /  wire API: {provider_name}"),
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().cyan()),
            spinner(Some(started_at), animations_enabled),
            Span::raw("  server validation in progress"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().cyan()),
            Span::styled(
                "resolving config, auth, provider SDK, and request model",
                Style::default().dim(),
            ),
        ]));
        lines.push(Line::from(vec![Span::styled(
            format!("  timeout: {remaining}s remaining"),
            Style::default().dim(),
        )]));
        Self::render_footer(&mut lines, "Esc cancel", "");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_saving(
        model: &str,
        request_model: &str,
        provider: ProviderWireApi,
        started_at: Instant,
        animations_enabled: bool,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let provider_name = Self::provider_display_name(provider);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Saving Connection",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!(
                "model: {model}  /  request model: {request_model}  /  wire API: {provider_name}"
            ),
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().cyan()),
            spinner(Some(started_at), animations_enabled),
            Span::raw("  waiting for server confirmation"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().cyan()),
            Span::styled(
                "provider/upsert is persisting the Connection and model",
                Style::default().dim(),
            ),
        ]));
        Self::render_footer(&mut lines, "Saving", "");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_validation_failed(
        error_message: &str,
        recovery_hint: Option<&str>,
        selected_action: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let actions = VALIDATION_FAILED_ACTIONS;

        let mut lines: Vec<Line<'static>> = vec![
            Line::from(vec![Span::styled(
                "Validation failed",
                Style::default().bold().red(),
            )]),
            Line::from(vec![Span::styled(
                "The server could not build or probe this Connection.",
                Style::default().dim(),
            )]),
            Line::from(vec![Span::styled(
                error_message.to_string(),
                Style::default().red(),
            )]),
        ];
        if let Some(hint) = recovery_hint.filter(|hint| !hint.trim().is_empty()) {
            lines.push(Line::from(vec![Span::styled(
                hint.to_string(),
                Style::default().dim(),
            )]));
        }
        lines.push(Line::from(""));

        for (idx, action) in actions.iter().enumerate() {
            let is_selected = idx == selected_action;
            Self::render_option_row(&mut lines, action.to_string(), None, is_selected);
        }

        Self::render_footer(&mut lines, "Enter select", "Esc exit onboarding");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }
}

// ── Key event entry point ──

impl OnboardingWidget {
    pub(crate) fn handle_key_event(&mut self, key_event: KeyEvent) {
        if matches!(key_event.kind, KeyEventKind::Release) {
            return;
        }
        match &self.state {
            OnboardingState::ModelSelection { .. } => self.model_selection_handle_key(key_event),
            OnboardingState::CustomModelForm { .. } => self.custom_model_form_handle_key(key_event),
            OnboardingState::ProviderSelection { .. } => {
                self.provider_selection_handle_key(key_event)
            }
            OnboardingState::ProviderSetup { .. } => self.provider_setup_handle_key(key_event),
            OnboardingState::DisconnectConfirmation { .. } => {
                self.disconnect_confirmation_handle_key(key_event)
            }
            OnboardingState::Disconnecting { .. } => {}
            OnboardingState::ModelDeleteConfirmation { .. } => {
                self.model_delete_confirmation_handle_key(key_event)
            }
            OnboardingState::ModelDeleting { .. } => {}
            OnboardingState::ModelSettings { .. } => self.model_settings_handle_key(key_event),
            OnboardingState::Review { .. } => self.review_handle_key(key_event),
            OnboardingState::InlineSetup { .. } => self.inline_setup_handle_key(key_event),
            OnboardingState::InvocationMethod { .. } => {
                self.invocation_method_handle_key(key_event)
            }
            OnboardingState::ReasoningEffort { .. } => self.reasoning_effort_handle_key(key_event),
            OnboardingState::Validating { .. } => {
                if key_event.code == KeyCode::Esc {
                    self.complete = true;
                    self.result = Some(OnboardingResult::Cancelled);
                }
            }
            OnboardingState::Saving { .. } => {}
            OnboardingState::ValidationFailed { .. } => {
                self.validation_failed_handle_key(key_event)
            }
        }
    }
}

// ── Renderable ──

impl Renderable for OnboardingWidget {
    fn desired_height(&self, _width: u16) -> u16 {
        match &self.state {
            OnboardingState::ModelSelection {
                state,
                filtered_indices,
                ..
            } => {
                let max_visible = MAX_POPUP_ROWS.min(filtered_indices.len().max(1));
                let scroll_offset = state
                    .scroll_top
                    .min(filtered_indices.len().saturating_sub(max_visible));
                let has_more_above = scroll_offset > 0;
                let has_more_below = scroll_offset + max_visible < filtered_indices.len();
                let option_rows = u16::try_from(max_visible).unwrap_or(u16::MAX).max(1);
                let overflow_rows = u16::from(has_more_above) + u16::from(has_more_below);
                // title + hint + blank + filter + blank + options + overflow + footer spacing
                option_rows + overflow_rows + 9
            }
            OnboardingState::CustomModelForm { .. } => 12,
            OnboardingState::ProviderSelection { items, .. } => {
                let connection_count = items
                    .iter()
                    .filter(|item| item.section == ProviderSelectionSection::Connections)
                    .count();
                let template_count = items
                    .iter()
                    .filter(|item| item.section == ProviderSelectionSection::Templates)
                    .count();
                let empty_rows = u16::from(connection_count == 0) + u16::from(template_count == 0);
                items.len() as u16 * 2 + 9 + empty_rows
            }
            OnboardingState::ProviderSetup { .. } => 14,
            OnboardingState::DisconnectConfirmation { .. } => 9,
            OnboardingState::Disconnecting { .. } => 7,
            OnboardingState::ModelDeleteConfirmation { .. } => 9,
            OnboardingState::ModelDeleting { .. } => 7,
            OnboardingState::ModelSettings { advanced_open, .. } => {
                if *advanced_open {
                    46
                } else {
                    20
                }
            }
            OnboardingState::Review { .. } => 15,
            OnboardingState::InlineSetup { model, .. } => {
                if self.model_supports_reasoning(model) {
                    31
                } else {
                    28
                }
            }
            OnboardingState::InvocationMethod {
                model,
                initial_model_settings,
                items,
                ..
            } => {
                let base_height = if self
                    .model_supports_reasoning_with_settings(model, initial_model_settings.as_ref())
                {
                    31
                } else {
                    28
                };
                base_height + items.len() as u16 * 2
            }
            OnboardingState::ReasoningEffort {
                model,
                initial_model_settings,
                items,
                ..
            } => {
                let base_height = if self
                    .model_supports_reasoning_with_settings(model, initial_model_settings.as_ref())
                {
                    31
                } else {
                    28
                };
                base_height + items.len() as u16 * 2
            }
            OnboardingState::Validating { .. } => 10,
            OnboardingState::Saving { .. } => 10,
            OnboardingState::ValidationFailed { recovery_hint, .. } => {
                if recovery_hint
                    .as_ref()
                    .is_some_and(|hint| !hint.trim().is_empty())
                {
                    14
                } else {
                    13
                }
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        match &self.state {
            OnboardingState::ModelSelection {
                items,
                state,
                search_query,
                filtered_indices,
                focus,
                manage_connection,
                ..
            } => {
                Self::render_model_selection(
                    items,
                    state,
                    search_query,
                    filtered_indices,
                    *focus,
                    *manage_connection,
                    area,
                    buf,
                );
            }
            OnboardingState::CustomModelForm {
                provider,
                model_id,
                display_name,
                active_field,
                input,
                cursor_pos,
                ..
            } => {
                Self::render_custom_model_form(
                    &provider.provider_name,
                    model_id,
                    display_name,
                    *active_field,
                    input,
                    *cursor_pos,
                    area,
                    buf,
                );
            }
            OnboardingState::ProviderSelection {
                items,
                selected_idx,
                focus,
            } => {
                Self::render_provider_selection(items, *selected_idx, *focus, area, buf);
            }
            OnboardingState::ProviderSetup {
                draft,
                active_field,
                input,
                cursor_pos,
            } => {
                Self::render_provider_setup(draft, *active_field, input, *cursor_pos, area, buf);
            }
            OnboardingState::DisconnectConfirmation { provider } => {
                Self::render_disconnect_confirmation(provider, area, buf);
            }
            OnboardingState::Disconnecting { provider_name } => {
                Self::render_disconnecting(provider_name, area, buf);
            }
            OnboardingState::ModelDeleteConfirmation {
                provider,
                model_name,
                ..
            } => {
                Self::render_model_delete_confirmation(
                    model_name,
                    &provider.provider_name,
                    area,
                    buf,
                );
            }
            OnboardingState::ModelDeleting { model_name, .. } => {
                Self::render_model_deleting(model_name, area, buf);
            }
            OnboardingState::ModelSettings {
                model,
                provider_name,
                request_model,
                display_name,
                invocation_method,
                default_reasoning_effort,
                settings,
                advanced_open,
                active_field,
                input,
                cursor_pos,
                settings_error,
                ..
            } => {
                Self::render_model_settings(
                    model,
                    provider_name,
                    request_model,
                    display_name,
                    *invocation_method,
                    default_reasoning_effort.as_deref(),
                    settings,
                    *advanced_open,
                    *active_field,
                    input,
                    *cursor_pos,
                    settings_error.as_deref(),
                    area,
                    buf,
                );
            }
            OnboardingState::Review { params } => {
                Self::render_review(params, area, buf);
            }
            OnboardingState::InlineSetup {
                model,
                provider_name,
                provider_credential_id,
                base_url,
                api_key,
                request_model,
                display_name,
                active_field,
                input,
                cursor_pos,
                ..
            } => {
                Self::render_inline_setup(
                    &InlineSetupRenderParams {
                        model,
                        supports_reasoning: self.model_supports_reasoning(model),
                        provider_name,
                        provider_credential_id: provider_credential_id.as_deref(),
                        base_url,
                        api_key,
                        request_model,
                        display_name,
                        active_field: Some(*active_field),
                        input,
                        cursor_pos: *cursor_pos,
                    },
                    area,
                    buf,
                );
            }
            OnboardingState::InvocationMethod {
                model,
                initial_model_settings,
                provider_name,
                provider_credential_id,
                base_url,
                api_key,
                request_model,
                display_name,
                items,
                selected_idx,
                ..
            } => {
                Self::render_invocation_method_inline(
                    &InlineSetupRenderParams {
                        model,
                        supports_reasoning: self.model_supports_reasoning_with_settings(
                            model,
                            initial_model_settings.as_ref(),
                        ),
                        provider_name,
                        provider_credential_id: provider_credential_id.as_deref(),
                        base_url,
                        api_key,
                        request_model,
                        display_name,
                        active_field: None,
                        input: "",
                        cursor_pos: 0,
                    },
                    items,
                    *selected_idx,
                    area,
                    buf,
                );
            }
            OnboardingState::ReasoningEffort {
                model,
                initial_model_settings,
                provider_name,
                provider_credential_id,
                base_url,
                api_key,
                request_model,
                display_name,
                invocation_method,
                items,
                selected_idx,
                ..
            } => {
                Self::render_reasoning_effort_inline(
                    &InlineSetupRenderParams {
                        model,
                        supports_reasoning: self.model_supports_reasoning_with_settings(
                            model,
                            initial_model_settings.as_ref(),
                        ),
                        provider_name,
                        provider_credential_id: provider_credential_id.as_deref(),
                        base_url,
                        api_key,
                        request_model,
                        display_name,
                        active_field: None,
                        input: "",
                        cursor_pos: 0,
                    },
                    *invocation_method,
                    items,
                    *selected_idx,
                    area,
                    buf,
                );
            }
            OnboardingState::Validating {
                model_slug,
                invocation_method,
                started_at,
                ..
            } => {
                if self.animations_enabled {
                    self.frame_requester.schedule_frame_in(SPINNER_INTERVAL);
                }
                Self::render_validating(
                    model_slug,
                    *invocation_method,
                    *started_at,
                    self.animations_enabled,
                    area,
                    buf,
                );
            }
            OnboardingState::Saving {
                model_slug,
                request_model,
                invocation_method,
                started_at,
                ..
            } => {
                if self.animations_enabled {
                    self.frame_requester.schedule_frame_in(SPINNER_INTERVAL);
                }
                Self::render_saving(
                    model_slug,
                    request_model,
                    *invocation_method,
                    *started_at,
                    self.animations_enabled,
                    area,
                    buf,
                );
            }
            OnboardingState::ValidationFailed {
                error_message,
                recovery_hint,
                selected_action,
                ..
            } => {
                Self::render_validation_failed(
                    error_message,
                    recovery_hint.as_deref(),
                    *selected_action,
                    area,
                    buf,
                );
            }
        }
    }

    fn cursor_pos(&self, _area: Rect) -> Option<(u16, u16)> {
        None
    }
}

#[cfg(test)]
mod model_settings_tests {
    use pretty_assertions::assert_eq;

    use super::{ModelSettingsDraft, OnboardingWidget};

    #[test]
    fn model_settings_form_preserves_full_custom_model_profile() {
        let draft = ModelSettingsDraft {
            display_name: "Acme Reasoner".to_string(),
            context_window: "200000".to_string(),
            context_window_hard: "200000".to_string(),
            max_tokens: "32000".to_string(),
            temperature: "0.2".to_string(),
            input_modalities: "text, image".to_string(),
            reasoning_capability: "levels".to_string(),
            reasoning_levels: "low, medium, high".to_string(),
            effective_context_window_percent: String::new(),
            top_p: "0.9".to_string(),
            top_k: "40".to_string(),
            family: "acme-reasoner".to_string(),
            release_date: "2026-01-15".to_string(),
            status: "active".to_string(),
            capabilities_json:
                r#"{"tools":true,"input":["text","image"],"output":["text"],"interleaved":"reasoning_content"}"#
                    .to_string(),
            channel: "Acme".to_string(),
            base_instructions: "Be precise.".to_string(),
            reasoning_implementation: "model_variant".to_string(),
            reasoning_variants_json: r#"[{"selection_value":"high","model_slug":"acme-reasoner-high","reasoning_effort":"high","label":"High","description":"Deliberate"}]"#.to_string(),
            default_variant: "balanced".to_string(),
            cost_json: r#"{"input":1,"output":2}"#.to_string(),
            metadata_json: r#"{"source":"custom"}"#.to_string(),
            request_json: r#"{"stream":true}"#.to_string(),
            options_json: r#"{"timeout_ms":120000}"#.to_string(),
            headers_json: r#"{"X-Model-Mode":"reasoning"}"#.to_string(),
            variants_json: r#"{"balanced":{"label":"Balanced"},"fast":{"label":"Fast","options":{"reasoning_effort":"low"}}}"#.to_string(),
            web_search_json: r#"{"mode":"provider"}"#.to_string(),
            web_fetch_json: r#"{"mode":"provider"}"#.to_string(),
            truncation_mode: "tokens".to_string(),
            truncation_limit: "4096".to_string(),
            supports_image_detail_original: Some(true),
            enabled: Some(true),
            priority: "10".to_string(),
        };

        let value = draft
            .to_value("acme-reasoner")
            .expect("full profile should produce model settings");
        assert_eq!(draft.validation_error("acme-reasoner"), None);
        assert_eq!(value["context_window"], 200000);
        assert_eq!(
            value["effective_context_window_percent"].as_f64(),
            Some(100.0)
        );
        assert_eq!(
            value["reasoning_capability"],
            serde_json::json!({"levels":["low", "medium", "high"]})
        );
        assert_eq!(
            value["reasoning_implementation"]["model_variant"]["variants"][0]["model"],
            "acme-reasoner-high"
        );
        assert_eq!(value["request"]["stream"], true);
        assert_eq!(value["capabilities"]["tools"], true);
        assert_eq!(value["headers"]["X-Model-Mode"], "reasoning");
        assert_eq!(
            value["variants"]["fast"]["options"]["reasoning_effort"],
            "low"
        );
        assert_eq!(value["web_search"]["mode"], "provider");
        assert_eq!(value["web_fetch"]["mode"], "provider");

        let restored = ModelSettingsDraft::from_value(Some(&value), "acme-reasoner");
        assert_eq!(restored.context_window, draft.context_window);
        assert_eq!(restored.effective_context_window_percent, "");
        assert_eq!(restored.reasoning_capability, draft.reasoning_capability);
        assert_eq!(restored.reasoning_levels, draft.reasoning_levels);
        assert_eq!(restored.request_json, draft.request_json);
        assert_eq!(restored.variants_json, draft.variants_json);
    }

    #[test]
    fn model_settings_form_rejects_invalid_extension_json() {
        let draft = ModelSettingsDraft {
            request_json: "{not json}".to_string(),
            ..ModelSettingsDraft::default()
        };

        assert_eq!(
            draft.validation_error("custom-model"),
            Some("Request JSON must be valid JSON".to_string())
        );
    }

    #[test]
    fn model_settings_form_loads_effective_context_window_tokens() {
        let value = serde_json::json!({
            "name": "Acme",
            "context_window": 200_000,
            "effective_context_window_percent": 95,
        });
        let draft = ModelSettingsDraft::from_value(Some(&value), "acme");
        assert_eq!(draft.context_window, "190000");
        assert_eq!(draft.context_window_hard, "200000");
        assert_eq!(draft.effective_context_window_percent, "");

        let saved = draft.to_value("acme").expect("settings value");
        assert_eq!(saved["context_window"], 200000);
        assert_eq!(
            saved["effective_context_window_percent"].as_f64(),
            Some(95.0)
        );
    }

    #[test]
    fn model_settings_form_stores_absolute_as_percent_of_hard_window() {
        let draft = ModelSettingsDraft {
            context_window: "250000".to_string(),
            context_window_hard: "1000000".to_string(),
            ..ModelSettingsDraft::default()
        };
        let saved = draft.to_value("flash").expect("settings value");
        assert_eq!(saved["context_window"], 1_000_000);
        assert_eq!(
            saved["effective_context_window_percent"].as_f64(),
            Some(25.0)
        );
    }

    #[test]
    fn model_settings_form_stores_fractional_percent_precisely() {
        let draft = ModelSettingsDraft {
            context_window: "333333".to_string(),
            context_window_hard: "1000000".to_string(),
            ..ModelSettingsDraft::default()
        };
        let saved = draft.to_value("flash").expect("settings value");
        assert_eq!(saved["context_window"], 1_000_000);
        let percent = saved["effective_context_window_percent"]
            .as_f64()
            .expect("fractional percent");
        assert!((percent - 33.3333).abs() < 0.0001);
        let restored = ModelSettingsDraft::from_value(Some(&saved), "flash");
        assert_eq!(restored.context_window, "333333");
    }

    #[test]
    fn model_settings_form_rejects_non_positive_limits() {
        let draft = ModelSettingsDraft {
            context_window: "0".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            draft.validation_error("custom-model"),
            Some("Context window must be greater than 0".to_string())
        );

        let draft = ModelSettingsDraft {
            truncation_limit: "-1".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            draft.validation_error("custom-model"),
            Some("Truncation limit must be greater than 0".to_string())
        );
    }

    #[test]
    fn model_settings_form_does_not_silently_ignore_reasoning_variant_rules() {
        let draft = ModelSettingsDraft {
            reasoning_implementation: "request_parameter".to_string(),
            reasoning_variants_json: "[]".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            draft.validation_error("custom-model"),
            Some("Reasoning variant rules require model_variant implementation".to_string())
        );

        let draft = ModelSettingsDraft {
            reasoning_implementation: "model_variant".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            draft.validation_error("custom-model"),
            Some("Reasoning variant rules must be a valid JSON array".to_string())
        );
    }

    #[test]
    fn default_reasoning_must_match_model_capability() {
        let levels = ModelSettingsDraft {
            reasoning_capability: "levels".to_string(),
            reasoning_levels: "low, high".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            OnboardingWidget::default_reasoning_error(&levels, &Some("medium".to_string())),
            Some("Default reasoning must be one of the configured levels: low, high".to_string())
        );

        let unsupported = ModelSettingsDraft {
            reasoning_capability: "unsupported".to_string(),
            ..ModelSettingsDraft::default()
        };
        assert_eq!(
            OnboardingWidget::default_reasoning_error(&unsupported, &Some("high".to_string())),
            Some("Unsupported reasoning cannot have a default selection".to_string())
        );
    }
}
