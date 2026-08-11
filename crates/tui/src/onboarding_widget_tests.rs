use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
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

fn command_payload(command: &str, prefix: &str) -> serde_json::Value {
    let payload = command
        .strip_prefix(prefix)
        .expect("command should have expected prefix");
    serde_json::from_str(payload).expect("command payload should be JSON")
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

fn next_shell_command(app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> String {
    loop {
        if let AppEvent::Command(AppCommand::RunUserShellCommand { command }) =
            app_event_rx.try_recv().expect("expected queued app event")
        {
            return command;
        }
    }
}

fn deepseek_model() -> Model {
    devo_core::ModelPreset {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        reasoning_capability: ReasoningCapability::Toggle,
        supported_reasoning_levels: vec![ReasoningEffort::High, ReasoningEffort::Max],
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..devo_core::ModelPreset::default()
    }
    .into()
}

fn toggle_only_model() -> Model {
    devo_core::ModelPreset {
        slug: "laguna-s-2.1".to_string(),
        display_name: "laguna-s-2.1".to_string(),
        reasoning_capability: ReasoningCapability::Toggle,
        supported_reasoning_levels: Vec::new(),
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        ..devo_core::ModelPreset::default()
    }
    .into()
}

fn toggle_only_provider_vendor() -> ProviderVendor {
    ProviderVendor {
        name: "Poolside".to_string(),
        base_url: Some("https://api.poolside.ai".to_string()),
        credential: Some("poolside_api_key".to_string()),
        headers: None,
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
    }
}

fn deepseek_provider_vendor() -> ProviderVendor {
    ProviderVendor {
        name: "Deepseek".to_string(),
        base_url: Some("https://api.deepseek.com".to_string()),
        credential: Some("deepseek_api_key".to_string()),
        headers: None,
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        enabled: true,
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    assert_eq!(command.starts_with("onboard "), true);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(Vec::new());
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
    assert_eq!(view.contains("› Add model anyway"), true);
    assert_eq!(view.contains("  Retry with current settings"), true);
}

#[test]
fn onboarding_existing_provider_validation_payload_preserves_edited_model_name() {
    let (_widget, mut app_event_rx) = edited_existing_provider_widget();

    let command = next_shell_command(&mut app_event_rx);
    let payload = command_payload(&command, "onboard ");

    assert_eq!(payload["model_slug"], "deepseek-v4-flash");
    assert_eq!(payload["request_model"], "DeepSeek-V4-Flash");
    assert_eq!(payload["display_name"], "Deepseek V4 Flash");
}

#[test]
fn onboarding_existing_provider_bypass_payload_preserves_edited_model_name() {
    let (mut widget, mut app_event_rx) = edited_existing_provider_widget();
    let _ = next_shell_command(&mut app_event_rx);
    widget.on_validation_failed("probe failed".to_string(), /*recovery_hint*/ None);

    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    let payload = command_payload(&command, "onboard-skip-validation ");

    assert_eq!(payload["model_slug"], "deepseek-v4-flash");
    assert_eq!(payload["request_model"], "DeepSeek-V4-Flash");
    assert_eq!(payload["display_name"], "Deepseek V4 Flash");
    assert_eq!(widget.take_result(), None);
}

#[test]
fn onboarding_existing_provider_validation_payload_preserves_edited_display_name() {
    let (_widget, mut app_event_rx) = edited_display_name_widget();

    let command = next_shell_command(&mut app_event_rx);
    let payload = command_payload(&command, "onboard ");

    assert_eq!(payload["model_slug"], "deepseek-v4-flash");
    assert_eq!(payload["request_model"], "deepseek-v4-flash");
    assert_eq!(payload["display_name"], "DeepSeek V4 Flash Custom");
}

#[test]
fn onboarding_validation_failure_can_bypass_validation() {
    let (mut widget, mut app_event_rx) = failed_validation_widget();

    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    assert_eq!(command.starts_with("onboard-skip-validation "), true);
    assert_eq!(widget.take_result(), None);

    widget.on_provider_saved(Some(&ProviderModelBinding {
        binding_id: "deepseek-v4-flash-deepseek".to_string(),
        model_slug: "deepseek-v4-flash".to_string(),
        provider: "Deepseek".to_string(),
        request_model: "deepseek-v4-flash".to_string(),
        display_name: Some("deepseek-v4-flash".to_string()),
        invocation_method: ProviderWireApi::OpenAIChatCompletions,
        default_reasoning_effort: Some("high".to_string()),
        enabled: true,
    }));
    assert_eq!(
        widget.take_result(),
        Some(OnboardingResult::ValidationBypassed {
            model_slug: "deepseek-v4-flash".to_string(),
            request_model: "deepseek-v4-flash".to_string(),
            display_name: "deepseek-v4-flash".to_string(),
        })
    );
}

#[test]
fn onboarding_validation_failure_retry_still_validates() {
    let (mut widget, mut app_event_rx) = failed_validation_widget();

    widget.handle_key_event(press(KeyCode::Down));
    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    assert_eq!(command.starts_with("onboard "), true);
    assert_eq!(command.starts_with("onboard-skip-validation "), false);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );
    widget.on_provider_vendors_listed(Vec::new());

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
fn onboarding_existing_provider_renders_values_after_labels_and_masks_saved_key() {
    let models = vec![deepseek_model()];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);

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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(Vec::new());

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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![deepseek_provider_vendor()]);

    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let invocation_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(invocation_view.contains("Configure provider binding"), true);
    assert_eq!(
        invocation_view.contains("Invocation Method: OpenAI Chat Completions"),
        true
    );
    assert_eq!(invocation_view.contains("› OpenAI Chat Completions"), true);

    widget.handle_key_event(press(KeyCode::Enter));

    let reasoning_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(reasoning_view.contains("Reason Effort: High"), true);
    assert_eq!(reasoning_view.contains(" Off"), true);
    assert_eq!(reasoning_view.contains("› High"), true);
    assert_eq!(reasoning_view.contains(" Max"), true);
    assert_eq!(reasoning_view.contains("Medium"), false);
    assert_eq!(reasoning_view.contains("XHigh"), false);

    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    let payload = command
        .strip_prefix("onboard ")
        .expect("onboard command prefix");
    let payload: serde_json::Value = serde_json::from_str(payload).expect("valid onboarding json");

    assert_eq!(
        payload["provider_credential_id"],
        serde_json::Value::String("deepseek_api_key".to_string())
    );
    assert_eq!(
        payload["default_reasoning_effort"],
        serde_json::Value::String("high".to_string())
    );
    assert_eq!(payload["api_key"], serde_json::Value::Null);
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
    assert_eq!(
        next_shell_command(&mut app_event_rx),
        "provider list".to_string()
    );

    widget.on_provider_vendors_listed(vec![toggle_only_provider_vendor()]);
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));
    widget.handle_key_event(press(KeyCode::Enter));

    let reasoning_view = rendered_rows(&widget, 160, 60).join("\n");
    assert_eq!(reasoning_view.contains("Reason Effort: On"), true);
    assert_eq!(reasoning_view.contains(" Off"), true);
    assert_eq!(reasoning_view.contains("› On"), true);
    assert_eq!(reasoning_view.contains("Medium"), false);

    widget.handle_key_event(press(KeyCode::Enter));

    let command = next_shell_command(&mut app_event_rx);
    let payload = command
        .strip_prefix("onboard ")
        .expect("onboard command prefix");
    let payload: serde_json::Value = serde_json::from_str(payload).expect("valid onboarding json");

    assert_eq!(
        payload["default_reasoning_effort"],
        serde_json::Value::String("enabled".to_string())
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
        invocation_view.contains("› OpenAI Chat Completions"),
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
        reasoning_view.contains("› High"),
        "expected selected reasoning effort in short viewport:\n{reasoning_view}"
    );
}
