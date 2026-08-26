//! Standalone onboarding widget for first-run model setup.
//!
//! This widget renders inline in the TUI bottom area. It handles all keyboard
//! input directly during onboarding, and is owned by `ChatWidget` — not by
//! `BottomPane` — keeping it decoupled from the composer and popup system.
//!
//! Follows L2-DES-TUI-001 flow:
//! 1. Model slug selection (searchable popup)
//! 2. Provider selection (existing or "Add provider...")
//! 3. Inline setup with vertical rail (* / | markers)
//! 4. Invocation method popup
//! 5. Reasoning effort popup (if model supports reasoning)
//! 6. Validation

use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use devo_protocol::Model;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffortOption;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::exec_cell::spinner;
use crate::onboarding_viewport::ViewportAnchor;
use crate::onboarding_viewport::render_lines_with_anchor;
use crate::onboarding_viewport::render_lines_with_fixed_footer;
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
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
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

/// Onboarding state machine following L2-DES-TUI-001.
#[derive(Debug)]
enum OnboardingState {
    /// Step 1: Select a model from catalog or enter custom.
    ModelSelection {
        items: Vec<ModelSelectionItem>,
        state: ScrollState,
        search_query: String,
        filtered_indices: Vec<usize>,
    },
    /// Step 1b: Enter a custom catalog model slug.
    CustomModelSlug { input: String, cursor_pos: usize },
    /// Step 2: Select an existing provider or add one.
    ProviderSelection {
        model: String,
        display_name: String,
        items: Vec<ProviderSelectionItem>,
        selected_idx: usize,
    },
    /// Steps 3-8: Inline setup for provider vendor and model binding fields.
    InlineSetup {
        model: String,
        provider: ProviderWireApi,
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
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        items: Vec<InvocationMethodItem>,
        selected_idx: usize,
    },
    /// Step 10: Select reasoning effort.
    ReasoningEffort {
        model: String,
        provider: ProviderWireApi,
        provider_name: String,
        provider_credential_id: Option<String>,
        base_url: String,
        api_key: String,
        request_model: String,
        display_name: String,
        invocation_method: ProviderWireApi,
        items: Vec<ReasoningEffortItem>,
        selected_idx: usize,
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
        provider_name: String,
        provider_credential_id: Option<String>,
        default_reasoning_effort: Option<String>,
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
    display_name: String,
    is_custom: bool,
}

#[derive(Debug)]
struct ProviderSelectionItem {
    label: String,
    description: String,
    kind: ProviderSelectionKind,
}

#[derive(Debug, Clone)]
enum ProviderSelectionKind {
    Vendor(ProviderVendor),
    AddProvider,
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
    /// Models from the catalog, stored so `go_back_to_model_selection` can restore them.
    original_models: Vec<Model>,
    provider_vendors: Vec<ProviderVendor>,
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
        let items = Self::build_model_items(models);
        let filtered_indices = (0..items.len()).collect();
        let mut state = ScrollState::new();
        state.selected_idx = Some(0);

        let this = Self {
            state: OnboardingState::ModelSelection {
                items,
                state,
                search_query: String::new(),
                filtered_indices,
            },
            complete: false,
            result: None,
            original_models: models.to_vec(),
            provider_vendors: Vec::new(),
            transcript_events: Vec::new(),
            app_event_tx,
            frame_requester,
            animations_enabled,
        };
        this.app_event_tx
            .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                command: "provider list".to_string(),
            }));
        this
    }

    /// Build `ModelSelectionItem` list from the catalog models.
    fn build_model_items(models: &[Model]) -> Vec<ModelSelectionItem> {
        models
            .iter()
            .map(|m| ModelSelectionItem {
                slug: m.slug.clone(),
                display_name: m.display_name.clone(),
                is_custom: false,
            })
            .collect()
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

    pub(crate) fn on_provider_vendors_listed(&mut self, provider_vendors: Vec<ProviderVendor>) {
        self.provider_vendors = provider_vendors;
        if let OnboardingState::ProviderSelection { items, .. } = &mut self.state {
            *items = Self::provider_selection_items(&self.provider_vendors);
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
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                bypassed: false,
                started_at: Instant::now(),
            };
        }
    }

    pub(crate) fn on_provider_saved(&mut self, model_binding: Option<&ProviderModelBinding>) {
        if let OnboardingState::Saving {
            model_slug,
            request_model,
            display_name,
            bypassed,
            ..
        } = &self.state
        {
            let result_model_slug = model_binding
                .map(|binding| binding.model_slug.clone())
                .unwrap_or_else(|| model_slug.clone());
            let result_request_model = model_binding
                .map(|binding| binding.request_model.clone())
                .unwrap_or_else(|| request_model.clone());
            let result_display_name = model_binding
                .and_then(|binding| binding.display_name.clone())
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

    pub(crate) fn on_provider_save_failed(&mut self, error_message: String) {
        if let OnboardingState::Saving {
            model_slug,
            request_model,
            display_name,
            invocation_method,
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
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
                provider_name: provider_name.clone(),
                provider_credential_id: provider_credential_id.clone(),
                default_reasoning_effort: default_reasoning_effort.clone(),
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
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
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
                provider_name: provider_name.clone(),
                provider_credential_id: provider_credential_id.clone(),
                default_reasoning_effort: default_reasoning_effort.clone(),
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
            } => {
                search_query.push_str(&text);
                Self::model_apply_filter(items, search_query, filtered_indices, state);
            }
            OnboardingState::CustomModelSlug { input, cursor_pos }
            | OnboardingState::InlineSetup {
                input, cursor_pos, ..
            } => {
                Self::insert_at_cursor(input, cursor_pos, &text);
            }
            OnboardingState::ProviderSelection { .. }
            | OnboardingState::InvocationMethod { .. }
            | OnboardingState::ReasoningEffort { .. }
            | OnboardingState::Validating { .. }
            | OnboardingState::Saving { .. }
            | OnboardingState::ValidationFailed { .. } => {}
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

    fn reasoning_effort_items(&self, slug: &str) -> Vec<ReasoningEffortItem> {
        self.model_by_slug(slug)
            .map(|model| model.effective_reasoning_capability().options())
            .unwrap_or_default()
            .into_iter()
            .map(Self::reasoning_effort_item)
            .collect()
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

    fn go_back_to_model_selection(&mut self) {
        let items = Self::build_model_items(&self.original_models);
        let filtered_indices = (0..items.len()).collect();
        let mut state = ScrollState::new();
        state.selected_idx = Some(0);
        self.state = OnboardingState::ModelSelection {
            items,
            state,
            search_query: String::new(),
            filtered_indices,
        };
    }
}

struct ValidationParams {
    model_slug: String,
    request_model: String,
    display_name: String,
    provider_id: String,
    provider_name: String,
    provider_credential_id: Option<String>,
    invocation_method: ProviderWireApi,
    default_reasoning_effort: Option<String>,
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

    fn start_validation(&mut self, params: ValidationParams) {
        let display_name = self.validation_display_name(&params);

        self.state = OnboardingState::Validating {
            model_slug: params.model_slug.clone(),
            request_model: params.request_model.clone(),
            display_name: display_name.clone(),
            provider_id: params.provider_id.clone(),
            provider_name: params.provider_name.clone(),
            provider_credential_id: params.provider_credential_id.clone(),
            invocation_method: params.invocation_method,
            default_reasoning_effort: params.default_reasoning_effort.clone(),
            base_url: params.base_url.clone(),
            api_key: params.api_key.clone(),
            started_at: Instant::now(),
        };
        let payload = serde_json::json!({
            "model_slug": params.model_slug,
            "request_model": params.request_model,
            "display_name": display_name,
            "provider_id": params.provider_id,
            "provider_name": params.provider_name,
            "provider_credential_id": params.provider_credential_id,
            "invocation_method": params.invocation_method,
            "default_reasoning_effort": params.default_reasoning_effort,
            "base_url": params.base_url,
            "api_key": params.api_key,
        });
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                command: format!("onboard {payload}"),
            }));
    }

    // ── Key Handling ──

    fn model_selection_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ModelSelection {
            items,
            state,
            search_query,
            filtered_indices,
        } = &mut self.state
        else {
            return;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Up => {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Char('k') if key.modifiers.is_empty() => {
                Self::model_move_up(state, filtered_indices);
            }
            KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Down => {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Char('j') if key.modifiers.is_empty() => {
                Self::model_move_down(state, filtered_indices);
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                search_query.push(c);
                Self::model_apply_filter(items, search_query, filtered_indices, state);
            }
            KeyCode::Backspace => {
                search_query.pop();
                Self::model_apply_filter(items, search_query, filtered_indices, state);
            }
            KeyCode::Enter => {
                if let Some(visible_idx) = state.selected_idx
                    && let Some(&actual_idx) = filtered_indices.get(visible_idx)
                    && let Some(item) = items.get(actual_idx)
                {
                    if item.is_custom {
                        self.state = OnboardingState::CustomModelSlug {
                            input: String::new(),
                            cursor_pos: 0,
                        };
                    } else {
                        let slug = item.slug.clone();
                        self.transcript_events
                            .push(OnboardingTranscriptEvent::ModelSelected {
                                model_slug: slug.clone(),
                                display_name: item.display_name.clone(),
                            });
                        self.state = OnboardingState::ProviderSelection {
                            model: slug,
                            display_name: item.display_name.clone(),
                            items: Self::provider_selection_items(&self.provider_vendors),
                            selected_idx: 0,
                        };
                    }
                }
            }
            KeyCode::Esc => {
                self.complete = true;
                self.result = Some(OnboardingResult::Cancelled);
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

    fn custom_model_slug_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::CustomModelSlug { input, cursor_pos } = &mut self.state else {
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
                let model = input.trim().to_string();
                if model.is_empty() {
                    return;
                }
                self.transcript_events
                    .push(OnboardingTranscriptEvent::ModelSelected {
                        model_slug: model.clone(),
                        display_name: model.clone(),
                    });
                self.state = OnboardingState::ProviderSelection {
                    model: model.clone(),
                    display_name: model,
                    items: Self::provider_selection_items(&self.provider_vendors),
                    selected_idx: 0,
                };
            }
            KeyCode::Esc => {
                self.go_back_to_model_selection();
            }
            _ => {}
        }
    }

    fn provider_selection_items(provider_vendors: &[ProviderVendor]) -> Vec<ProviderSelectionItem> {
        let mut items = provider_vendors
            .iter()
            .cloned()
            .map(|provider_vendor| {
                let description = provider_vendor
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "Configured provider vendor".to_string());
                ProviderSelectionItem {
                    label: provider_vendor.name.clone(),
                    description,
                    kind: ProviderSelectionKind::Vendor(provider_vendor),
                }
            })
            .collect::<Vec<_>>();
        items.push(ProviderSelectionItem {
            label: "Add provider...".to_string(),
            description: "Enter custom provider settings".to_string(),
            kind: ProviderSelectionKind::AddProvider,
        });
        items
    }

    fn provider_selection_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::ProviderSelection {
            model,
            display_name,
            items,
            selected_idx,
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
                    let model_slug = model.clone();
                    let selected_display_name = display_name.clone();
                    match &item.kind {
                        ProviderSelectionKind::Vendor(provider_vendor) => {
                            let provider = provider_vendor
                                .wire_apis
                                .first()
                                .copied()
                                .unwrap_or_else(|| Self::infer_provider(&model_slug));
                            let base_url = provider_vendor.base_url.clone().unwrap_or_default();
                            self.transcript_events.push(
                                OnboardingTranscriptEvent::ProviderSelected {
                                    provider_name: provider_vendor.name.clone(),
                                    base_url: provider_vendor.base_url.clone(),
                                    credential_summary: Self::credential_summary(
                                        provider_vendor.credential.as_deref(),
                                        None,
                                    ),
                                },
                            );
                            let (active_field, input, cursor_pos) = if base_url.trim().is_empty() {
                                (
                                    InlineField::BaseUrl,
                                    base_url.clone(),
                                    Self::char_count(&base_url),
                                )
                            } else {
                                (
                                    InlineField::RequestModel,
                                    model_slug.clone(),
                                    Self::char_count(&model_slug),
                                )
                            };
                            self.state = OnboardingState::InlineSetup {
                                model: model_slug.clone(),
                                provider,
                                provider_name: provider_vendor.name.clone(),
                                provider_credential_id: provider_vendor.credential.clone(),
                                base_url,
                                api_key: String::new(),
                                request_model: model_slug.clone(),
                                display_name: selected_display_name,
                                active_field,
                                input,
                                cursor_pos,
                            };
                        }
                        ProviderSelectionKind::AddProvider => {
                            self.transcript_events.push(
                                OnboardingTranscriptEvent::ProviderSelected {
                                    provider_name: "Add provider...".to_string(),
                                    base_url: None,
                                    credential_summary: "new provider credentials".to_string(),
                                },
                            );
                            self.state = OnboardingState::InlineSetup {
                                model: model_slug.clone(),
                                provider: ProviderWireApi::OpenAIChatCompletions,
                                provider_name: String::new(),
                                provider_credential_id: None,
                                base_url: String::new(),
                                api_key: String::new(),
                                request_model: model_slug.clone(),
                                display_name: selected_display_name,
                                active_field: InlineField::ProviderName,
                                input: String::new(),
                                cursor_pos: 0,
                            };
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.go_back_to_model_selection();
            }
            _ => {}
        }
    }

    // ── Inline Setup ──

    fn inline_setup_handle_key(&mut self, key: KeyEvent) {
        let OnboardingState::InlineSetup {
            model,
            provider,
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
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            items,
                            selected_idx,
                        };
                    }
                }
            }
            KeyCode::Esc => {
                // Go back to previous field or provider selection.
                match active_field {
                    InlineField::ProviderName => {
                        // Go back to provider selection.
                        let model = model.clone();
                        let display_name = display_name.clone();
                        self.state = OnboardingState::ProviderSelection {
                            model,
                            display_name,
                            items: Self::provider_selection_items(&self.provider_vendors),
                            selected_idx: 0,
                        };
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
            provider_name,
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            items,
            selected_idx,
            ..
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
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    let request_model = request_model.clone();
                    let display_name = display_name.clone();

                    if self.model_supports_reasoning(&model) {
                        let reasoning_items = self.reasoning_effort_items(&model);
                        let selected_reasoning_idx =
                            self.default_reasoning_effort_index(&model, &reasoning_items);
                        self.state = OnboardingState::ReasoningEffort {
                            model,
                            provider,
                            provider_name,
                            provider_credential_id,
                            base_url,
                            api_key,
                            request_model,
                            display_name,
                            invocation_method: invocation,
                            items: reasoning_items,
                            selected_idx: selected_reasoning_idx,
                        };
                    } else {
                        // No reasoning — go straight to validation.
                        let base_url_opt = if base_url.is_empty() {
                            None
                        } else {
                            Some(base_url)
                        };
                        let api_key_opt = if api_key.is_empty() {
                            None
                        } else {
                            Some(api_key)
                        };
                        let params = ValidationParams {
                            model_slug: model,
                            request_model,
                            display_name,
                            provider_id: provider_name.clone(),
                            provider_name,
                            provider_credential_id,
                            invocation_method: invocation,
                            default_reasoning_effort: None,
                            base_url: base_url_opt,
                            api_key: api_key_opt,
                        };
                        self.record_settings_confirmed(&params);
                        self.start_validation(params);
                    }
                }
            }
            KeyCode::Esc => {
                // Go back to inline setup, display name field.
                let model = model.clone();
                let provider = *provider;
                let provider_name = provider_name.clone();
                let provider_credential_id = provider_credential_id.clone();
                let base_url = base_url.clone();
                let api_key = api_key.clone();
                let model_name_val = request_model.clone();
                let display_name_val = display_name.clone();
                self.state = OnboardingState::InlineSetup {
                    model,
                    provider,
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
            provider_credential_id,
            base_url,
            api_key,
            request_model,
            display_name,
            provider_name,
            invocation_method,
            items,
            selected_idx,
            ..
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
                let provider_credential_id = provider_credential_id.clone();
                let default_reasoning_effort =
                    items.get(*selected_idx).map(|item| item.value.clone());
                let base_url = base_url.clone();
                let api_key = api_key.clone();
                let base_url_opt = if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                };
                let api_key_opt = if api_key.is_empty() {
                    None
                } else {
                    Some(api_key)
                };
                let params = ValidationParams {
                    model_slug: model,
                    request_model,
                    display_name,
                    provider_id: provider_name.clone(),
                    provider_name,
                    provider_credential_id,
                    invocation_method,
                    default_reasoning_effort,
                    base_url: base_url_opt,
                    api_key: api_key_opt,
                };
                self.record_settings_confirmed(&params);
                self.start_validation(params);
            }
            KeyCode::Esc => {
                // Go back to invocation method selection.
                // Extract values before reassigning self.state.
                let (m, prov, pn, pc, bu, ak, mn, dn, invocation) = match &self.state {
                    OnboardingState::ReasoningEffort {
                        model,
                        provider,
                        provider_name,
                        provider_credential_id,
                        base_url,
                        api_key,
                        request_model,
                        display_name,
                        invocation_method,
                        ..
                    } => (
                        model.clone(),
                        *provider,
                        provider_name.clone(),
                        provider_credential_id.clone(),
                        base_url.clone(),
                        api_key.clone(),
                        request_model.clone(),
                        display_name.clone(),
                        *invocation_method,
                    ),
                    _ => return,
                };
                let items = Self::invocation_method_items();
                let selected_idx = Self::invocation_method_selection_index(invocation, &items);
                self.state = OnboardingState::InvocationMethod {
                    model: m,
                    provider: prov,
                    provider_name: pn,
                    provider_credential_id: pc,
                    base_url: bu,
                    api_key: ak,
                    request_model: mn,
                    display_name: dn,
                    items,
                    selected_idx,
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
            provider_name,
            provider_credential_id,
            default_reasoning_effort,
            base_url,
            api_key,
            error_message: _,
            recovery_hint: _,
            selected_action,
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
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    let payload = serde_json::json!({
                        "model_slug": result_model_slug.clone(),
                        "request_model": result_request_model.clone(),
                        "display_name": result_display_name.clone(),
                        "provider_id": provider_name.clone(),
                        "provider_name": provider_name.clone(),
                        "provider_credential_id": provider_credential_id.clone(),
                        "invocation_method": provider,
                        "default_reasoning_effort": default_reasoning_effort.clone(),
                        "base_url": base_url.clone(),
                        "api_key": api_key.clone(),
                    });
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                            command: format!("onboard-skip-validation {payload}"),
                        }));
                    self.state = OnboardingState::Saving {
                        model_slug: result_model_slug,
                        request_model: result_request_model,
                        display_name: result_display_name,
                        provider_id: provider_name.clone(),
                        provider_name,
                        provider_credential_id,
                        invocation_method: provider,
                        default_reasoning_effort,
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
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let default_reasoning_effort = default_reasoning_effort.clone();
                    let base_url = base_url.clone();
                    let api_key = api_key.clone();
                    self.start_validation(ValidationParams {
                        model_slug: model,
                        request_model,
                        display_name,
                        provider_id: provider_name.clone(),
                        provider_name,
                        provider_credential_id,
                        invocation_method: provider,
                        default_reasoning_effort,
                        base_url,
                        api_key,
                    });
                }
                2 => {
                    // Edit settings — go back to inline setup API key field.
                    let model_slug = model.clone();
                    let request_model = request_model.clone();
                    let display_name = display_name.clone();
                    let provider = *provider;
                    let provider_name = provider_name.clone();
                    let provider_credential_id = provider_credential_id.clone();
                    let base_url = base_url.clone().unwrap_or_default();
                    let api_key = api_key.clone().unwrap_or_default();
                    self.state = OnboardingState::InlineSetup {
                        model: model_slug.clone(),
                        provider,
                        provider_name,
                        provider_credential_id,
                        base_url,
                        api_key: api_key.clone(),
                        request_model,
                        display_name,
                        active_field: InlineField::ApiKey,
                        input: api_key.clone(),
                        cursor_pos: Self::char_count(&api_key),
                    };
                }
                3 => {
                    self.go_back_to_model_selection();
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
            Span::styled("  ·  ", Style::default().dim()),
            Span::styled(secondary.to_string(), Style::default().dim()),
        ]));
    }

    fn render_option_row(
        lines: &mut Vec<Line<'static>>,
        label: String,
        description: Option<String>,
        is_selected: bool,
    ) {
        let marker = if is_selected { "›" } else { " " };
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
        let label = if more_above { "↑ more" } else { "↓ more" };
        Line::from(vec![
            Span::raw(" ".repeat(LIST_LEFT_PAD + 2)),
            Span::styled(label.to_string(), Style::default().dim()),
        ])
    }

    fn render_inline_setup_header(lines: &mut Vec<Line<'static>>, model: &str) {
        lines.push(Line::from(vec![Span::styled(
            "Configure provider binding",
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
            Span::styled("● ", rail_style),
            Span::raw(" "),
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
            Span::styled("| ", rail_style),
            Span::styled(hint.to_string(), Style::default().dim()),
        ]));
        lines.push(Line::from(vec![Span::styled("|", rail_style)]));
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
            Span::styled("● ", rail_style),
            Span::raw(" "),
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
                Span::styled("| ", rail_style),
                Span::styled(hint.to_string(), Style::default().dim()),
            ]));
        }
        lines.push(Line::from(vec![Span::styled("|", rail_style)]));
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
            Span::styled("| ", Style::default().cyan().bold()),
            Span::styled(if is_selected { "›" } else { " " }, marker_style),
            Span::raw(" "),
            Span::styled(label.to_string(), label_style),
        ]));
        if !description.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("| ", Style::default().cyan().bold()),
                Span::styled("  ", Style::default().dim()),
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
        format!("{}▌{}", &input[..byte_pos], &input[byte_pos..])
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

    fn render_model_selection(
        items: &[ModelSelectionItem],
        state: &ScrollState,
        search_query: &str,
        filtered_indices: &[usize],
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            "Choose model profile",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Type to filter built-in model capabilities.",
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));

        if search_query.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "filter: all",
                Style::default().dim(),
            )]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("filter: ", Style::default().dim()),
                Span::styled(search_query.to_string(), Style::default()),
            ]));
        }
        lines.push(Line::from(""));

        let max_visible = MAX_POPUP_ROWS.min(filtered_indices.len().max(1));
        let scroll_offset = state
            .scroll_top
            .min(filtered_indices.len().saturating_sub(max_visible));
        let has_more_above = scroll_offset > 0;
        let has_more_below = scroll_offset + max_visible < filtered_indices.len();
        let mut anchor = None;

        if has_more_above {
            lines.push(Self::scroll_overflow_line(/*more_above*/ true));
        }
        for (vis_idx, &actual_idx) in filtered_indices
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(max_visible)
        {
            if let Some(item) = items.get(actual_idx) {
                let is_selected = state.selected_idx == Some(vis_idx);
                let start = lines.len();
                // Slug-only rows: display_name is used for filtering, not shown.
                Self::render_option_row(
                    &mut lines,
                    item.slug.clone(),
                    /*description*/ None,
                    is_selected,
                );
                if is_selected {
                    anchor = Some(ViewportAnchor {
                        start,
                        end: lines.len(),
                    });
                }
            }
        }
        if has_more_below {
            lines.push(Self::scroll_overflow_line(/*more_above*/ false));
        }

        let mut footer_lines = Vec::new();
        Self::render_footer(&mut footer_lines, "Enter select", "Esc cancel");
        render_lines_with_fixed_footer(lines, footer_lines, anchor, content_area, buf);
    }

    fn render_custom_model_slug(input: &str, cursor_pos: usize, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }
        let content_area = onboarding_content_area(area);
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            "Custom model profile",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Enter the model slug to use as the local capability profile.",
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));

        let byte_pos = Self::byte_index_for_char(input, cursor_pos);
        lines.push(Line::from(vec![
            Span::styled("› ", Style::default().cyan()),
            Span::styled(
                format!("{}▌{}", &input[..byte_pos], &input[byte_pos..]),
                Style::default(),
            ),
        ]));
        Self::render_footer(&mut lines, "Enter confirm", "Esc back");

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);
    }

    fn render_provider_selection(
        _model: &str,
        items: &[ProviderSelectionItem],
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
            "Choose provider vendor",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Select a configured endpoint, or add a new vendor. Wire API comes next.",
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
            "Choose the effort stored on this model binding.",
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
            "Testing provider binding",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!("model: {model}  ·  wire API: {provider_name}"),
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("│ ", Style::default().cyan()),
            spinner(Some(started_at), animations_enabled),
            Span::raw("  server validation in progress"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("│   ", Style::default().cyan()),
            Span::styled(
                "resolving config, auth, provider SDK, and request model",
                Style::default().dim(),
            ),
        ]));
        lines.push(Line::from(vec![Span::styled(
            format!("│   timeout: {remaining}s remaining"),
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
            "Saving provider binding",
            Style::default().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!(
                "model: {model}  ·  request model: {request_model}  ·  wire API: {provider_name}"
            ),
            Style::default().dim(),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("│ ", Style::default().cyan()),
            spinner(Some(started_at), animations_enabled),
            Span::raw("  waiting for server confirmation"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("│   ", Style::default().cyan()),
            Span::styled(
                "provider/upsert is persisting the provider and model binding",
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
                "The server could not build or probe this provider binding.",
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
            OnboardingState::CustomModelSlug { .. } => self.custom_model_slug_handle_key(key_event),
            OnboardingState::ProviderSelection { .. } => {
                self.provider_selection_handle_key(key_event)
            }
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
            OnboardingState::CustomModelSlug { .. } => 8,
            OnboardingState::ProviderSelection { items, .. } => items.len() as u16 * 2 + 6,
            OnboardingState::InlineSetup { model, .. } => {
                if self.model_supports_reasoning(model) {
                    31
                } else {
                    28
                }
            }
            OnboardingState::InvocationMethod { model, items, .. } => {
                let base_height = if self.model_supports_reasoning(model) {
                    31
                } else {
                    28
                };
                base_height + items.len() as u16 * 2
            }
            OnboardingState::ReasoningEffort { model, items, .. } => {
                let base_height = if self.model_supports_reasoning(model) {
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
            } => {
                Self::render_model_selection(
                    items,
                    state,
                    search_query,
                    filtered_indices,
                    area,
                    buf,
                );
            }
            OnboardingState::CustomModelSlug { input, cursor_pos } => {
                Self::render_custom_model_slug(input, *cursor_pos, area, buf);
            }
            OnboardingState::ProviderSelection {
                model: _,
                display_name: _,
                items,
                selected_idx,
            } => {
                Self::render_provider_selection("", items, *selected_idx, area, buf);
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
                        supports_reasoning: self.model_supports_reasoning(model),
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
                        supports_reasoning: self.model_supports_reasoning(model),
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
