use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use devo_protocol::InputItem;
use devo_protocol::Model;
use devo_protocol::ReasoningEffort;
use devo_protocol::ThinkingCapability;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::v2::app_command::AppCommand;
use crate::v2::app_event::AppEvent;
use crate::v2::app_event_sender::AppEventSender;
use crate::v2::chatwidget::ChatWidget;
use crate::v2::chatwidget::ChatWidgetInit;
use crate::v2::chatwidget::ThinkingListEntry;
use crate::v2::chatwidget::TuiSessionState;
use crate::v2::render::renderable::Renderable;
use crate::v2::tui::frame_requester::FrameRequester;

fn widget_with_model(
    model: Model,
    cwd: PathBuf,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model)),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        show_model_onboarding: false,
        startup_tooltip_override: None,
    });
    (widget, app_event_rx)
}

fn onboarding_widget_with_model(
    model: Model,
    cwd: PathBuf,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model)),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        show_model_onboarding: true,
        startup_tooltip_override: None,
    });
    (widget, app_event_rx)
}

fn rendered_rows(widget: &ChatWidget, width: u16, height: u16) -> Vec<String> {
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

#[test]
fn thinking_entries_are_generated_from_model_capability_options() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        thinking_capability: ThinkingCapability::Levels(vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        ..Model::default()
    };
    let (widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    assert_eq!(
        widget.thinking_entries(),
        vec![
            ThinkingListEntry {
                is_current: false,
                label: "Low".to_string(),
                description: "Fastest, cheapest, least deliberative".to_string(),
                value: "low".to_string(),
            },
            ThinkingListEntry {
                is_current: true,
                label: "Medium".to_string(),
                description: "Balanced speed and deliberation".to_string(),
                value: "medium".to_string(),
            },
        ]
    );
}

#[test]
fn submit_text_emits_user_turn_with_model_and_thinking() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        thinking_capability: ThinkingCapability::Toggle,
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());

    widget.set_thinking_selection(Some("disabled".to_string()));
    widget.submit_text("hello".to_string());

    assert_eq!(
        app_event_rx.try_recv().expect("command event is emitted"),
        AppEvent::Command(AppCommand::UserTurn {
            input: vec![InputItem::Text {
                text: "hello".to_string(),
            }],
            cwd: Some(cwd),
            model: Some("test-model".to_string()),
            thinking: Some("disabled".to_string()),
            sandbox: None,
            approval_policy: None,
        })
    );
}

#[test]
fn typed_character_submits_without_waiting_for_redraw_tick() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_key_event(KeyEvent::from(KeyCode::Char('a')));
    widget.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let emitted_command = std::iter::from_fn(|| app_event_rx.try_recv().ok())
        .find(|event| matches!(event, AppEvent::Command(_)))
        .expect("command event is emitted");
    assert_eq!(
        emitted_command,
        AppEvent::Command(AppCommand::UserTurn {
            input: vec![InputItem::Text {
                text: "a".to_string(),
            }],
            cwd: Some(cwd),
            model: Some("test-model".to_string()),
            thinking: None,
            sandbox: None,
            approval_policy: None,
        })
    );
}

#[test]
fn key_release_does_not_duplicate_text_input() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_key_event(KeyEvent {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    });
    widget.handle_key_event(KeyEvent {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: crossterm::event::KeyEventState::NONE,
    });
    std::thread::sleep(crate::v2::bottom_pane::ChatComposer::recommended_paste_flush_delay());
    widget.pre_draw_tick();
    widget.handle_key_event(KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    });

    let emitted_command = std::iter::from_fn(|| app_event_rx.try_recv().ok())
        .find(|event| matches!(event, AppEvent::Command(_)))
        .expect("command event is emitted");
    assert_eq!(
        emitted_command,
        AppEvent::Command(AppCommand::UserTurn {
            input: vec![InputItem::Text {
                text: "a".to_string(),
            }],
            cwd: Some(cwd),
            model: Some("test-model".to_string()),
            thinking: None,
            sandbox: None,
            approval_policy: None,
        })
    );
}

#[test]
fn onboarding_updates_placeholder_text_for_each_step() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = onboarding_widget_with_model(model, cwd);
    assert_eq!(widget.placeholder_text(), "Onboarding: enter model name");

    widget.submit_text("custom-model".to_string());
    assert_eq!(widget.placeholder_text(), "Onboarding: enter base URL");

    widget.submit_text("https://example.com".to_string());
    assert_eq!(widget.placeholder_text(), "Onboarding: enter API key");

    widget.submit_text("secret".to_string());
    assert_eq!(
        widget.placeholder_text(),
        "Onboarding: validating connection"
    );
}

#[test]
fn desired_height_grows_with_active_transcript_output() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let base_height = widget.desired_height(80);
    for index in 0..12 {
        widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(format!(
            "line {index}\n"
        )));
    }

    assert!(widget.desired_height(80) > base_height);
}

#[test]
fn committed_history_drains_to_scrollback_lines() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let initial_lines = widget.drain_scrollback_lines(80);
    assert!(!initial_lines.is_empty());

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "done".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
    });

    let committed_lines = widget.drain_scrollback_lines(80);
    assert!(committed_lines.is_empty());
}

#[test]
fn turn_finished_does_not_add_completion_status_line_to_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
    });

    let committed_lines = widget.drain_scrollback_lines(80);
    assert!(!committed_lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.contains("Turn completed (Completed)"))
    }));
}

#[test]
fn active_response_renders_generating_status_without_devo_title() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta("hello".to_string()));

    let rendered = rendered_rows(&widget, 80, 12).join("\n");
    assert!(rendered.contains("Generating"));
    assert!(!rendered.contains("Devo -"));
}

// TODO: Still buggy here, need to be fixed.
// #[test]
// fn slash_popup_shows_active_filter_hint() {
//     let cwd = std::env::current_dir().expect("current directory is available");
//     let model = Model {
//         slug: "test-model".to_string(),
//         display_name: "Test Model".to_string(),
//         ..Model::default()
//     };
//     let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

//     widget.handle_paste("/m".to_string());

//     let rendered = rendered_rows(&widget, 80, 6).join("\n");
//     assert!(rendered.contains("filter: /m"));
//     assert!(rendered.contains("/model"));
// }

#[test]
fn slash_model_opens_model_picker_instead_of_printing_current_model() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let alt_model = Model {
        slug: "second-model".to_string(),
        display_name: "Second Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model, alt_model],
        show_model_onboarding: false,
        startup_tooltip_override: None,
    });

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "model".to_string(),
    });

    assert_eq!(widget.placeholder_text(), "Ask Devo");
    assert_eq!(
        widget.current_model().map(|m| m.slug.as_str()),
        Some("test-model")
    );
}

#[test]
fn session_switch_updates_session_identity_projection() {
    let initial_cwd = std::env::current_dir().expect("current directory is available");
    let resumed_cwd = initial_cwd.join("resumed");
    let model = Model {
        slug: "initial-model".to_string(),
        display_name: "Initial Model".to_string(),
        ..Model::default()
    };
    let resumed_model = Model {
        slug: "resumed-model".to_string(),
        display_name: "Resumed Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, initial_cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: resumed_cwd.clone(),
        title: Some("Resumed".to_string()),
        model: Some("resumed-model".to_string()),
        total_input_tokens: 3,
        total_output_tokens: 5,
        history_items: Vec::new(),
        loaded_item_count: 0,
    });

    assert_eq!(widget.current_cwd(), resumed_cwd.as_path());
    assert_eq!(
        widget.current_model(),
        Some(&Model {
            display_name: "resumed-model".to_string(),
            ..resumed_model
        })
    );
}

#[test]
fn new_session_prepared_resets_session_identity_projection() {
    let initial_cwd = std::env::current_dir().expect("current directory is available");
    let resumed_cwd = initial_cwd.join("resumed");
    let model = Model {
        slug: "initial-model".to_string(),
        display_name: "Initial Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, initial_cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: resumed_cwd,
        title: None,
        model: Some("resumed-model".to_string()),
        total_input_tokens: 3,
        total_output_tokens: 5,
        history_items: Vec::new(),
        loaded_item_count: 0,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::NewSessionPrepared {
        cwd: initial_cwd.clone(),
        model: "new-session-model".to_string(),
    });

    assert_eq!(widget.current_cwd(), initial_cwd.as_path());
    assert_eq!(
        widget.current_model().map(|model| model.slug.as_str()),
        Some("new-session-model")
    );
}

#[test]
fn model_selection_updates_session_projection_and_emits_context_override() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let alt_model = Model {
        slug: "second-model".to_string(),
        display_name: "Second Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model, alt_model.clone()],
        show_model_onboarding: false,
        startup_tooltip_override: None,
    });

    widget.handle_app_event(AppEvent::ModelSelected {
        model: "second-model".to_string(),
    });
    widget.submit_text("hello".to_string());

    assert_eq!(widget.current_model(), Some(&alt_model));
    assert_eq!(
        app_event_rx
            .try_recv()
            .expect("context override command is emitted"),
        AppEvent::Command(AppCommand::OverrideTurnContext {
            cwd: None,
            model: Some("second-model".to_string()),
            thinking: Some(None),
            sandbox: None,
            approval_policy: None,
        })
    );
    assert_eq!(
        app_event_rx.try_recv().expect("command event is emitted"),
        AppEvent::Command(AppCommand::UserTurn {
            input: vec![InputItem::Text {
                text: "hello".to_string(),
            }],
            cwd: Some(widget.current_cwd().to_path_buf()),
            model: Some("second-model".to_string()),
            thinking: None,
            sandbox: None,
            approval_policy: None,
        })
    );
}
