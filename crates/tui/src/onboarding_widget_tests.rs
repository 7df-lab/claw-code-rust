use std::collections::BTreeMap;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ProviderInfo;
use devo_protocol::ProviderModelInfo;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::onboarding_widget::OnboardingResult;
use crate::onboarding_widget::OnboardingTranscriptEvent;
use crate::onboarding_widget::OnboardingWidget;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn shift_char(ch: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn plain_char(ch: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn type_text(widget: &mut OnboardingWidget, text: &str) {
    for ch in text.chars() {
        widget.handle_key_event(plain_char(ch));
    }
}

fn rendered_rows(widget: &OnboardingWidget, width: u16, height: u16) -> Vec<String> {
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    widget.render(area, &mut buf);
    (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|col| buf[(col, row)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn next_command(app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> AppCommand {
    match app_event_rx.try_recv().expect("expected queued app event") {
        AppEvent::Command(command) => command,
        event => panic!("expected queued app command, got {event:?}"),
    }
}

fn next_provider_validate(
    app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>,
) -> devo_protocol::native::rpc_admin::ProviderValidateParams {
    match next_command(app_event_rx) {
        AppCommand::ProviderValidate { params } => params,
        command => panic!("expected provider validation command, got {command:?}"),
    }
}

fn next_provider_upsert(
    app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>,
) -> devo_protocol::native::rpc_admin::ProviderUpsertParams {
    match next_command(app_event_rx) {
        AppCommand::ProviderUpsert { params } => params,
        command => panic!("expected provider upsert command, got {command:?}"),
    }
}

fn deepseek_model() -> Model {
    Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        reasoning_capability: ReasoningCapability::Levels(devo_protocol::levels_with_leading_off(
            [ReasoningEffort::High, ReasoningEffort::Max],
        )),
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..Model::default()
    }
}

fn toggle_only_model() -> Model {
    Model {
        slug: "laguna-s-2.1".to_string(),
        display_name: "laguna-s-2.1".to_string(),
        reasoning_capability: ReasoningCapability::Toggle,
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        ..Model::default()
    }
}

fn toggle_only_provider() -> ProviderInfo {
    ProviderInfo {
        id: "poolside".to_string(),
        name: "Poolside".to_string(),
        description: None,
        base_url: Some("https://api.poolside.ai".to_string()),
        credential: Some("poolside_api_key".to_string()),
        headers: BTreeMap::new(),
        options: None,
        request: None,
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
        models: BTreeMap::new(),
    }
}

fn deepseek_provider() -> ProviderInfo {
    ProviderInfo {
        id: "deepseek".to_string(),
        name: "Deepseek".to_string(),
        description: None,
        base_url: Some("https://api.deepseek.com".to_string()),
        credential: Some("deepseek_api_key".to_string()),
        headers: BTreeMap::new(),
        options: None,
        request: None,
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
        models: BTreeMap::new(),
    }
}

fn widget_at_invocation_method_popup() -> OnboardingWidget {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget
}

fn widget_at_reasoning_effort_popup() -> OnboardingWidget {
    let mut widget = widget_at_invocation_method_popup();
    widget.handle_key_event(press(KeyCode::Enter));
    widget
}

fn failed_validation_widget() -> (OnboardingWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);
    for _ in 0..9 {
        widget.handle_key_event(press(KeyCode::Enter));
    }

    let command = next_command(&mut app_event_rx);
    assert!(matches!(command, AppCommand::ProviderValidate { .. }));
    widget.on_validation_failed("probe failed".to_string(), /*recovery_hint*/ None);
    (widget, app_event_rx)
}

fn edited_existing_provider_widget() -> (OnboardingWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    for _ in 0.."deepseek-v4-flash".chars().count() {
        widget.handle_key_event(press(KeyCode::Backspace));
    }
    type_text(&mut widget, "DeepSeek-V4-Flash");
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    (widget, app_event_rx)
}

fn edited_display_name_widget() -> (OnboardingWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    for _ in 0.."Deepseek V4 Flash".chars().count() {
        widget.handle_key_event(press(KeyCode::Backspace));
    }
    type_text(&mut widget, "DeepSeek V4 Flash Custom");
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    (widget, app_event_rx)
}

#[test]
fn onboarding_inline_input_backspace_handles_non_ascii_characters() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(Vec::new());
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(plain_char('你'));
    widget.handle_key_event(plain_char('好'));
    widget.handle_key_event(press(KeyCode::Backspace));

    let rows = rendered_rows(&widget, 160, 40);
    let provider_row = rows
        .iter()
        .find(|row| row.contains("Provider Name:"))
        .expect("provider name row");
    assert_eq!(provider_row.contains("你"), true);
    assert_eq!(provider_row.contains("好"), false);
}

#[test]
fn onboarding_validation_failure_defaults_to_add_model_anyway() {
    let (widget, _app_event_rx) = failed_validation_widget();

    let view = rendered_rows(&widget, 160, 40).join("\n");
    assert_eq!(view.contains("> Add model anyway"), true);
    assert_eq!(view.contains("  Retry with current settings"), true);
}

#[test]
fn onboarding_existing_provider_validation_payload_preserves_edited_model_name() {
    let (_widget, mut app_event_rx) = edited_existing_provider_widget();

    let params = next_provider_validate(&mut app_event_rx);
    assert_eq!(params.provider.id, "deepseek");
    assert_eq!(params.model, "DeepSeek-V4-Flash");
    assert_eq!(params.api_key, None);
    assert_eq!(
        params.provider.models["DeepSeek-V4-Flash"].name,
        Some("Deepseek V4 Flash".to_string())
    );
}

#[test]
fn onboarding_existing_provider_bypass_payload_preserves_edited_model_name() {
    let (mut widget, mut app_event_rx) = edited_existing_provider_widget();
    let _ = next_provider_validate(&mut app_event_rx);
    widget.on_validation_failed("probe failed".to_string(), /*recovery_hint*/ None);

    widget.handle_key_event(press(KeyCode::Enter));

    let params = next_provider_upsert(&mut app_event_rx);
    assert_eq!(params.provider.id, "deepseek");
    assert_eq!(
        params.default_model,
        Some("deepseek/DeepSeek-V4-Flash".to_string())
    );
    assert_eq!(params.api_key, None);
    assert_eq!(
        params.provider.models["DeepSeek-V4-Flash"].name,
        Some("Deepseek V4 Flash".to_string())
    );
    assert_eq!(widget.take_result(), None);
}

#[test]
fn onboarding_existing_provider_validation_payload_preserves_edited_display_name() {
    let (_widget, mut app_event_rx) = edited_display_name_widget();

    let params = next_provider_validate(&mut app_event_rx);
    assert_eq!(params.model, "deepseek-v4-flash");
    assert_eq!(
        params.provider.models["deepseek-v4-flash"].name,
        Some("DeepSeek V4 Flash Custom".to_string())
    );
}

#[test]
fn onboarding_validation_failure_can_bypass_validation() {
    let (mut widget, mut app_event_rx) = failed_validation_widget();

    widget.handle_key_event(press(KeyCode::Enter));

    let params = next_provider_upsert(&mut app_event_rx);
    assert_eq!(
        params.default_model,
        Some("deepseek/deepseek-v4-flash".to_string())
    );
    assert_eq!(widget.take_result(), None);

    widget.on_provider_upserted(&deepseek_provider(), Some("deepseek/deepseek-v4-flash"));
    assert_eq!(
        widget.take_result(),
        Some(OnboardingResult::ValidationBypassed {
            model_slug: "deepseek-v4-flash".to_string(),
            request_model: "deepseek-v4-flash".to_string(),
            display_name: "Deepseek V4 Flash".to_string(),
        })
    );
}

#[test]
fn onboarding_validation_failure_retry_still_validates() {
    let (mut widget, mut app_event_rx) = failed_validation_widget();

    widget.handle_key_event(press(KeyCode::Down));
    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_command(&mut app_event_rx);
    assert!(matches!(command, AppCommand::ProviderValidate { .. }));
    assert_eq!(widget.take_result(), None);
}

#[test]
fn onboarding_settings_summary_masks_entered_api_key() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);
    widget.on_providers_listed(Vec::new());

    widget.handle_key_event(press(KeyCode::Enter));
    let _ = widget.take_transcript_events();
    widget.handle_key_event(press(KeyCode::Enter));
    let _ = widget.take_transcript_events();

    type_text(&mut widget, "Deepseek");
    widget.handle_key_event(press(KeyCode::Enter));
    type_text(&mut widget, "https://api.deepseek.com");
    widget.handle_key_event(press(KeyCode::Enter));
    type_text(&mut widget, "secret-key");
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    let _ = widget.take_transcript_events();
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let events = widget.take_transcript_events();
    assert_eq!(
        events,
        vec![OnboardingTranscriptEvent::SettingsConfirmed {
            provider_name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            request_model: "deepseek-v4-flash".to_string(),
            display_name: "Deepseek V4 Flash".to_string(),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: Some("high".to_string()),
            credential_summary: "new API key entered".to_string(),
        }]
    );
    assert!(!format!("{events:?}").contains("secret-key"));
}

#[test]
fn onboarding_custom_provider_and_model_can_use_advanced_settings() {
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &[],
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);
    widget.on_providers_listed(Vec::new());

    widget.handle_key_event(press(KeyCode::Enter));
    let setup_view = rendered_rows(&widget, 100, 24).join("\n");
    assert!(setup_view.contains("Enter the connection details for this provider."));
    assert!(setup_view.contains("Stored securely in auth.json."));
    for glyph in ["█", "╚", "╝", "═", "▌", "─", "│", "●"] {
        assert!(
            !setup_view.contains(glyph),
            "unexpected decorative glyph: {glyph}"
        );
    }
    type_text(&mut widget, "Acme Gateway");
    widget.handle_key_event(press(KeyCode::Enter));
    type_text(&mut widget, "https://api.example.com/v1");
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    widget.handle_key_event(press(KeyCode::Enter));
    type_text(&mut widget, "custom-model");
    let custom_model_form = rendered_rows(&widget, 100, 24).join(" ");
    assert!(custom_model_form.contains("Add a custom model"));
    assert!(custom_model_form.contains("Provider model ID"));
    assert!(custom_model_form.contains("Display name"));
    assert!(!custom_model_form.contains("model slug"));
    widget.handle_key_event(press(KeyCode::Enter));
    type_text(&mut widget, "Custom Model");
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Char(' ')));
    type_text(&mut widget, "128000");
    for _ in 0..33 {
        widget.handle_key_event(press(KeyCode::Enter));
    }
    widget.handle_key_event(press(KeyCode::Enter));

    let params = next_provider_validate(&mut app_event_rx);
    assert_eq!(params.provider.id, "acme-gateway");
    assert_eq!(params.model, "custom-model");
    assert_eq!(params.api_key, None);
    assert_eq!(
        params.provider.models["custom-model"].name,
        Some("Custom Model".to_string())
    );
    assert_eq!(
        params.provider.models["custom-model"].context_window,
        Some(128000)
    );
}

#[test]
fn onboarding_existing_provider_renders_values_after_labels_and_masks_saved_key() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let rows = rendered_rows(&widget, 160, 40);
    let provider_row = rows
        .iter()
        .find(|row| row.contains("Provider Name:"))
        .expect("provider row");
    let provider_hint_row = rows
        .iter()
        .find(|row| row.contains("Enter a name to recognize this provider later."))
        .expect("provider hint row");
    let base_url_row = rows
        .iter()
        .find(|row| row.contains("Base URL:"))
        .expect("base url row");
    let api_key_row = rows
        .iter()
        .find(|row| row.contains("API Key:"))
        .expect("api key row");

    assert_eq!(provider_row.contains("Provider Name: Deepseek"), true);
    assert_eq!(
        provider_hint_row
            .trim()
            .contains("Enter a name to recognize this provider later."),
        true
    );
    assert_eq!(
        base_url_row.contains("Base URL: https://api.deepseek.com"),
        true
    );
    assert_eq!(api_key_row.contains("API Key: ****...***"), true);
}

#[test]
fn onboarding_required_provider_name_and_base_url_do_not_advance_when_empty() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(Vec::new());

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(shift_char('D'));

    let provider_rows = rendered_rows(&widget, 160, 40);
    let provider_row = provider_rows
        .iter()
        .find(|row| row.contains("Provider Name:"))
        .expect("provider row after blocked advance");
    assert_eq!(provider_row.contains("Provider Name: D"), true);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(plain_char('h'));

    let base_url_rows = rendered_rows(&widget, 160, 40);
    let base_url_row = base_url_rows
        .iter()
        .find(|row| row.contains("Base URL:"))
        .expect("base url row after blocked advance");
    assert_eq!(base_url_row.contains("Base URL: h"), true);
}

#[test]
fn onboarding_invocation_and_reasoning_popups_render_inline_and_use_model_presets() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![deepseek_provider()]);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let invocation_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(invocation_view.contains("Configure Connection"), true);
    assert_eq!(
        invocation_view.contains("Invocation Method: OpenAI Chat Completions"),
        true
    );
    assert_eq!(invocation_view.contains("> OpenAI Chat Completions"), true);

    widget.handle_key_event(press(KeyCode::Enter));

    let reasoning_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(reasoning_view.contains("Reason Effort: High"), true);
    assert_eq!(reasoning_view.contains(" Off"), true);
    assert_eq!(reasoning_view.contains("> High"), true);
    assert_eq!(reasoning_view.contains(" Max"), true);
    assert_eq!(reasoning_view.contains("Medium"), false);
    assert_eq!(reasoning_view.contains("XHigh"), false);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let params = next_provider_validate(&mut app_event_rx);
    assert_eq!(
        params.provider.credential,
        Some("deepseek_api_key".to_string())
    );
    assert_eq!(params.model, "deepseek-v4-flash");
    assert_eq!(params.api_key, None);
    assert_eq!(
        params.provider.models["deepseek-v4-flash"].default_reasoning_selection,
        Some("high".to_string())
    );
}

#[test]
fn onboarding_toggle_model_reasoning_popup_shows_off_and_on() {
    let models = vec![toggle_only_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(next_command(&mut app_event_rx), AppCommand::ProviderList);

    widget.on_providers_listed(vec![toggle_only_provider()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let reasoning_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(reasoning_view.contains("Reason Effort: On"), true);
    assert_eq!(reasoning_view.contains(" Off"), true);
    assert_eq!(reasoning_view.contains("> On"), true);
    assert_eq!(reasoning_view.contains("Medium"), false);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let params = next_provider_validate(&mut app_event_rx);
    assert_eq!(
        params.provider.models["laguna-s-2.1"].default_reasoning_selection,
        Some("on".to_string())
    );
}

#[test]
fn onboarding_invocation_popup_keeps_active_section_visible_when_short() {
    let widget = widget_at_invocation_method_popup();

    let invocation_view = rendered_rows(&widget, 72, 10).join("\n");

    assert!(
        invocation_view.contains("Invocation Method: OpenAI Chat Completions"),
        "expected invocation step in short viewport:\n{invocation_view}"
    );
    assert!(
        invocation_view.contains("Choose the API protocol."),
        "expected invocation hint in short viewport:\n{invocation_view}"
    );
    assert!(
        invocation_view.contains("> OpenAI Chat Completions"),
        "expected selected invocation option in short viewport:\n{invocation_view}"
    );
}

#[test]
fn onboarding_reasoning_popup_keeps_active_section_visible_when_short_and_narrow() {
    let widget = widget_at_reasoning_effort_popup();

    let reasoning_view = rendered_rows(&widget, 48, 10).join("\n");

    assert!(
        reasoning_view.contains("Reason Effort: High"),
        "expected reasoning step in short viewport:\n{reasoning_view}"
    );
    assert!(
        reasoning_view.contains("Choose the default reasoning effort"),
        "expected wrapped reasoning hint in short viewport:\n{reasoning_view}"
    );
    assert!(
        reasoning_view.contains("> High"),
        "expected selected reasoning effort in short viewport:\n{reasoning_view}"
    );
}

#[test]
fn unconnected_builtin_provider_collects_api_key_without_editing_template() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.on_providers_listed_with_status(
        vec![deepseek_provider()],
        vec!["deepseek".to_string()],
        Vec::new(),
    );

    widget.handle_key_event(press(KeyCode::Enter));
    let setup = rendered_rows(&widget, 120, 30).join("\n");
    assert!(setup.contains("Connect to Deepseek"));
    assert!(setup.contains("Base URL: https://api.deepseek.com"));
    assert!(setup.contains("Fixed by the provider directory template."));
    assert!(setup.contains("Enter once to create this Connection"));
    type_text(&mut widget, "new-secret");
    let entered = rendered_rows(&widget, 120, 30).join("\n");
    assert!(entered.contains("API Key: **********|"));
    assert!(!entered.contains("new-secret"));

    widget.handle_key_event(press(KeyCode::Enter));
    let model_selection = rendered_rows(&widget, 120, 30).join("\n");
    assert!(model_selection.contains("Choose a model"));
}

#[test]
fn connected_builtin_provider_goes_to_model_selection_without_editing_connection() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.on_providers_listed_with_status(
        vec![deepseek_provider()],
        vec!["deepseek".to_string()],
        vec!["deepseek".to_string()],
    );

    widget.handle_key_event(press(KeyCode::Enter));
    let model_selection = rendered_rows(&widget, 120, 30).join("\n");
    assert!(model_selection.contains("Models in this Connection"));
    assert!(!model_selection.contains("Connect to Deepseek"));
}

#[test]
fn connection_model_screen_lists_only_saved_models_and_can_remove_one() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.on_providers_listed_with_status_and_models(
        vec![deepseek_provider()],
        vec!["deepseek".to_string()],
        vec!["deepseek".to_string()],
        BTreeMap::from([(
            "deepseek".to_string(),
            BTreeMap::from([
                (
                    "saved-model".to_string(),
                    ProviderModelInfo {
                        name: Some("Saved model".to_string()),
                        ..ProviderModelInfo::default()
                    },
                ),
                (
                    "second-model".to_string(),
                    ProviderModelInfo {
                        name: Some("Second model".to_string()),
                        ..ProviderModelInfo::default()
                    },
                ),
            ]),
        )]),
    );

    widget.handle_key_event(press(KeyCode::Enter));
    let model_selection = rendered_rows(&widget, 120, 30).join("\n");
    assert!(model_selection.contains("Models in this Connection"));
    assert!(model_selection.contains("saved-model"));
    assert!(model_selection.contains("second-model"));
    assert!(model_selection.contains("Add custom model profile"));
    assert!(!model_selection.contains("deepseek-v4-flash"));

    widget.handle_key_event(press(KeyCode::Char('d')));
    let confirmation = rendered_rows(&widget, 120, 20).join("\n");
    assert!(confirmation.contains("Remove Saved model from Deepseek"));
    widget.handle_key_event(press(KeyCode::Enter));
    assert_eq!(
        app_event_rx.try_recv().expect("remove model command"),
        AppEvent::Command(AppCommand::RemoveProviderModel {
            provider_id: "deepseek".to_string(),
            model_id: "saved-model".to_string(),
        })
    );

    widget.on_provider_model_removed("deepseek", "saved-model");
    assert_eq!(
        app_event_rx.try_recv().expect("provider refresh command"),
        AppEvent::Command(AppCommand::ProviderList)
    );
    let after_remove = rendered_rows(&widget, 120, 30).join("\n");
    assert!(!after_remove.contains("saved-model"));
    assert!(after_remove.contains("second-model"));
    assert!(after_remove.contains("Add custom model profile"));

    widget.handle_key_event(press(KeyCode::Tab));
    widget.handle_key_event(press(KeyCode::Enter));
    let custom_model = rendered_rows(&widget, 120, 20).join(" ");
    assert!(custom_model.contains("Add a custom model"));
    assert!(custom_model.contains("Provider model ID"));
    assert!(custom_model.contains("Display name"));
    widget.handle_key_event(press(KeyCode::Esc));
    let back_to_models = rendered_rows(&widget, 120, 30).join(" ");
    assert!(back_to_models.contains("Models in this Connection"));
}

#[test]
fn connected_provider_can_be_disconnected_without_removing_the_template() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.on_providers_listed_with_status(
        vec![deepseek_provider()],
        vec!["deepseek".to_string()],
        vec!["deepseek".to_string()],
    );

    let provider_selection = rendered_rows(&widget, 120, 30).join("\n");
    assert!(provider_selection.contains("Connections"));
    assert!(provider_selection.contains("Provider templates"));
    assert!(provider_selection.contains("Saved Connection · https://api.deepseek.com"));
    assert!(provider_selection.contains("Read-only template · https://api.deepseek.com"));

    widget.handle_key_event(press(KeyCode::Char('d')));
    let confirmation = rendered_rows(&widget, 120, 20).join("\n");
    assert!(confirmation.contains("Disconnect Deepseek"));
    widget.handle_key_event(press(KeyCode::Enter));
    assert_eq!(
        app_event_rx.try_recv().expect("disconnect command"),
        AppEvent::Command(AppCommand::DisconnectProvider {
            provider_id: "deepseek".to_string(),
        })
    );

    widget.on_provider_disconnected("deepseek");
    assert_eq!(
        app_event_rx.try_recv().expect("provider refresh command"),
        AppEvent::Command(AppCommand::ProviderList)
    );
    let disconnected = rendered_rows(&widget, 120, 20).join("\n");
    assert!(disconnected.contains("Provider templates"));
    assert!(disconnected.contains("Read-only template · https://api.deepseek.com"));
    assert!(disconnected.contains("No saved Connections yet."));
}

#[test]
fn provider_template_cannot_be_disconnected() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.on_providers_listed_with_status(
        vec![deepseek_provider()],
        vec!["deepseek".to_string()],
        vec!["deepseek".to_string()],
    );

    widget.handle_key_event(press(KeyCode::Down));
    widget.handle_key_event(press(KeyCode::Char('d')));

    let view = rendered_rows(&widget, 120, 24).join("\n");
    assert!(view.contains("Connections"));
    assert!(view.contains("Provider templates"));
    assert!(!view.contains("Disconnect Deepseek"));
    assert!(app_event_rx.try_recv().is_err());
}
