use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::CollaborationMode;
use devo_protocol::InputItem;
use devo_protocol::ItemId;
use devo_protocol::Model;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::RequestUserInputOption;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;
use tokio::sync::mpsc;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event::ExitMode;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::ReasoningEffortListEntry;
use crate::chatwidget::TuiSessionState;
use crate::events::PlanStep;
use crate::events::PlanStepStatus;
use crate::events::SavedModelEntry;
use crate::events::TextItemKind;
use crate::history_cell::HistoryCell;
use crate::render::renderable::Renderable;
use crate::slash_command::built_in_slash_commands;
use crate::tui::frame_requester::FrameRequester;
use crate::ui_consts::LIVE_PREFIX_COLS;

fn widget_with_model(
    model: Model,
    cwd: PathBuf,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    widget_with_model_and_reasoning_effort(model, cwd, None)
}

fn widget_with_model_and_reasoning_effort(
    model: Model,
    cwd: PathBuf,
    initial_reasoning_effort_selection: Option<String>,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model)),
        initial_reasoning_effort_selection,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });
    (widget, app_event_rx)
}

fn saved_model_entry(model: &str) -> SavedModelEntry {
    SavedModelEntry {
        binding_id: None,
        model: model.to_string(),
        request_model: None,
        display_name: None,
        provider_id: Some("provider".to_string()),
        provider_name: Some("Provider".to_string()),
        wire_api: ProviderWireApi::OpenAIChatCompletions,
        base_url: None,
        api_key: None,
    }
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
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: true,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });
    (widget, app_event_rx)
}

fn onboarding_widget_with_available_model(
    model: Model,
    cwd: PathBuf,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    onboarding_widget_with_available_model_and_exit_after_onboarding(
        model, cwd, /*exit_after_onboarding*/ false,
    )
}

fn onboarding_widget_with_available_model_and_exit_after_onboarding(
    model: Model,
    cwd: PathBuf,
    exit_after_onboarding: bool,
) -> (ChatWidget, mpsc::UnboundedReceiver<AppEvent>) {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model],
        saved_models: Vec::new(),
        show_model_onboarding: true,
        exit_after_onboarding,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });
    (widget, app_event_rx)
}

fn rendered_buffer(widget: &ChatWidget, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    widget.render(area, &mut buf);
    buf
}

fn rendered_rows(widget: &ChatWidget, width: u16, height: u16) -> Vec<String> {
    let buf = rendered_buffer(widget, width, height);
    let area = buf.area;
    (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|col| buf[(col, row)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn lines_contain_fg(lines: &[Line<'static>], color: Color) -> bool {
    lines
        .iter()
        .any(|line| line.spans.iter().any(|span| span.style.fg == Some(color)))
}

fn press_key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    }
}

fn scrollback_contains_text(lines: &[crate::history_cell::ScrollbackLine], text: &str) -> bool {
    lines.iter().any(|line| {
        line.line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .contains(text)
    })
}

fn find_row_index(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|row| row.contains(needle))
}

fn scrollback_plain_lines(lines: &[crate::history_cell::ScrollbackLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn trim_trailing_blank_scrollback_lines(
    mut lines: Vec<crate::history_cell::ScrollbackLine>,
) -> Vec<crate::history_cell::ScrollbackLine> {
    while lines.last().is_some_and(|line| {
        line.line
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty())
    }) {
        lines.pop();
    }
    lines
}

fn line_texts(lines: Vec<ratatui::text::Line<'static>>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect()
}

fn indices_containing(lines: &[String], needles: &[&str]) -> Vec<usize> {
    needles
        .iter()
        .map(|needle| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle} in:\n{}", lines.join("\n")))
        })
        .collect()
}

#[test]
fn user_prompt_multiline_has_single_marker_and_aligned_continuation_rows() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("line one\nline two\nline three".to_string());

    let transcript = line_texts(widget.transcript_overlay_lines(80));
    let first_line_index = transcript
        .iter()
        .position(|line| line.contains("line one"))
        .unwrap_or_else(|| panic!("missing user prompt in:\n{}", transcript.join("\n")));
    let user_lines = &transcript[first_line_index - 1..first_line_index + 3];

    assert_eq!(
        user_lines,
        ["  ", "▌ line one", "  line two", "  line three"]
    );
}

#[test]
fn restore_user_message_to_composer_restores_text() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget
        .restore_user_message_to_composer(crate::chatwidget::UserMessage::from("previous message"));

    let rendered = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rendered.contains("previous message"),
        "composer should show restored text:\n{rendered}"
    );
}

#[test]
fn transcript_overlay_cell_carries_user_message_payload() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("previous message".to_string());
    let _ = widget.drain_scrollback_lines(80);

    let cells = widget.transcript_overlay_cells(80);
    let user_cell = cells
        .into_iter()
        .find(|cell| cell.user_message.is_some())
        .expect("user transcript cell");
    assert_eq!(
        user_cell.user_message.expect("user payload").text,
        "previous message"
    );
}

#[test]
fn backtrack_preview_restore_latest_user_message() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("first message".to_string());
    let _ = widget.drain_scrollback_lines(80);
    widget.submit_text("second message".to_string());
    let _ = widget.drain_scrollback_lines(80);

    let mut overlay =
        crate::pager_overlay::Overlay::new_transcript(widget.transcript_overlay_cells(80), 80);
    let crate::pager_overlay::Overlay::Transcript(transcript) = &mut overlay else {
        panic!("expected transcript overlay");
    };
    transcript.begin_backtrack_preview();
    let selected = transcript
        .selected_user_message()
        .expect("selected latest user");
    widget.restore_user_message_to_composer(selected);

    let rendered = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rendered.contains("second message"),
        "expected latest message to be restored into composer:\n{rendered}"
    );
}

#[test]
fn backtrack_preview_can_restore_previous_and_next_user_messages() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("first message".to_string());
    let _ = widget.drain_scrollback_lines(80);
    widget.submit_text("second message".to_string());
    let _ = widget.drain_scrollback_lines(80);

    let mut overlay =
        crate::pager_overlay::Overlay::new_transcript(widget.transcript_overlay_cells(80), 80);
    let crate::pager_overlay::Overlay::Transcript(transcript) = &mut overlay else {
        panic!("expected transcript overlay");
    };
    transcript.begin_backtrack_preview();
    transcript.select_prev_user();
    let previous = transcript
        .selected_user_message()
        .expect("selected previous user");
    widget.restore_user_message_to_composer(previous);
    let rendered_prev = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rendered_prev.contains("first message"),
        "expected previous message after select_prev:\n{rendered_prev}"
    );

    transcript.select_next_user();
    let next = transcript
        .selected_user_message()
        .expect("selected next user");
    widget.restore_user_message_to_composer(next);
    let rendered_next = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rendered_next.contains("second message"),
        "expected next message after select_next:\n{rendered_next}"
    );
}

#[test]
fn restoring_previous_message_truncates_later_transcript_history() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("first message".to_string());
    widget.add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
        Line::from("assistant 1"),
    ]));
    widget.submit_text("second message".to_string());
    widget.add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
        Line::from("assistant 2"),
    ]));
    let _ = widget.drain_scrollback_lines(80);

    widget.truncate_history_to_user_turn_count(1);
    widget.restore_user_message_to_composer(crate::chatwidget::UserMessage::from("first message"));

    let rendered = rendered_rows(&widget, 80, 16).join("\n");
    assert!(rendered.contains("first message"));
    let transcript_lines = widget
        .transcript_overlay_cells(80)
        .into_iter()
        .flat_map(|cell| cell.lines)
        .flat_map(|line| line.spans.into_iter())
        .map(|span| span.content)
        .collect::<String>();
    assert!(transcript_lines.contains("first message"));
    assert!(!transcript_lines.contains("second message"));
    assert!(!transcript_lines.contains("assistant 2"));
}

#[test]
fn esc_backtrack_hint_is_shown_before_restore() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.show_esc_backtrack_hint();
    let rendered = rendered_rows(&widget, 100, 14).join("\n");
    assert!(
        rendered.contains("esc again to edit previous message")
            || rendered.contains("esc esc to edit previous message"),
        "expected esc backtrack hint before opening overlay:\n{rendered}"
    );
}

#[test]
fn resume_command_opens_loading_browser_immediately() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_app_event(AppEvent::Command(AppCommand::RunUserShellCommand {
        command: "session list".to_string(),
    }));

    assert!(widget.is_resume_browser_open());

    let rows = rendered_rows(&widget, 80, 12);
    assert!(
        rows.iter()
            .any(|row| row.contains("Loading saved sessions"))
    );
}

#[test]
fn resume_loading_browser_closes_with_esc_or_q() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::Command(AppCommand::RunUserShellCommand {
        command: "session list".to_string(),
    }));
    assert!(widget.is_resume_browser_open());

    widget.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!widget.is_resume_browser_open());

    widget.handle_app_event(AppEvent::Command(AppCommand::RunUserShellCommand {
        command: "session list".to_string(),
    }));
    assert!(widget.is_resume_browser_open());

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!widget.is_resume_browser_open());
}

#[test]
fn resume_browser_clips_sessions_to_viewport_height() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions = (0..12)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    widget.open_resume_browser_for_test(sessions);

    let rows = rendered_rows(&widget, 80, 10);
    let blob = rows.join("\n");
    assert!(blob.contains("Session 0"));
    assert!(blob.contains("Session 1"));
    assert!(
        !blob.contains("Session 2"),
        "rows should be clipped to viewport:\n{blob}"
    );
    assert!(
        !blob.contains("Session 3"),
        "rows should be clipped to viewport:\n{blob}"
    );
    assert!(
        blob.contains("↓ more"),
        "expected lower overflow indicator:\n{blob}"
    );
}

#[test]
fn resume_browser_list_closes_with_esc_or_q() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions = vec![crate::events::SessionListEntry {
        session_id: SessionId::new(),
        title: "Session".to_string(),
        updated_at: "2026-05-18 10:00".to_string(),
        is_active: true,
    }];
    widget.open_resume_browser_for_test(sessions.clone());
    assert!(widget.is_resume_browser_open());

    widget.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!widget.is_resume_browser_open());

    widget.open_resume_browser_for_test(sessions);
    assert!(widget.is_resume_browser_open());
    widget.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!widget.is_resume_browser_open());
}

#[test]
fn resume_browser_keeps_selection_visible_when_navigating_down() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions = (0..12)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    widget.open_resume_browser_for_test(sessions);

    for _ in 0..11 {
        widget.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    assert_eq!(widget.resume_browser_selection_for_test(), Some(11));

    let rows = rendered_rows(&widget, 80, 10);
    let blob = rows.join("\n");
    assert!(
        blob.contains("Session 11"),
        "selected tail item should be visible:\n{blob}"
    );
    assert!(
        !blob.contains("Session 0"),
        "viewport should have scrolled away from the head:\n{blob}"
    );
    assert!(
        blob.contains("↑ more"),
        "expected upper overflow indicator after scrolling:\n{blob}"
    );
}

#[test]
fn resume_browser_enter_resumes_selected_scrolled_session() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions: Vec<_> = (0..12)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    let expected = sessions[11].session_id;
    widget.open_resume_browser_for_test(sessions);

    for _ in 0..11 {
        widget.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = app_event_rx
        .try_recv()
        .expect("resume selection should emit switch command");
    assert_eq!(
        event,
        AppEvent::Command(AppCommand::switch_session(expected))
    );
}

#[test]
fn resume_browser_enter_blocks_prompt_submission_until_switch_completes() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let target_session_id = SessionId::new();
    widget.open_resume_browser_for_test(vec![crate::events::SessionListEntry {
        session_id: target_session_id,
        title: "Session".to_string(),
        updated_at: "2026-05-18 10:00".to_string(),
        is_active: false,
    }]);

    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().expect("switch command"),
        AppEvent::Command(AppCommand::switch_session(target_session_id))
    );
    assert!(widget.is_resuming_session_for_test());
    assert_eq!(
        widget.status_indicator_header_for_test(),
        Some("Resuming session...")
    );
    assert!(
        rendered_rows(&widget, 80, 12)
            .iter()
            .any(|row| row.contains("Resuming session"))
    );

    widget.submit_text("should not send".to_string());

    assert!(
        app_event_rx.try_recv().is_err(),
        "prompt submission should not emit another app command while resuming"
    );
    assert_eq!(
        widget.status_message_for_test(),
        "Cannot send while resuming session"
    );
}

#[test]
fn session_switched_clears_resume_blocking_state() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let cwd = PathBuf::from(".");
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());
    widget.open_resume_browser_for_test(vec![crate::events::SessionListEntry {
        session_id: SessionId::new(),
        title: "Session".to_string(),
        updated_at: "2026-05-18 10:00".to_string(),
        is_active: false,
    }]);
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = app_event_rx.try_recv().expect("switch command");

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: Some("Resumed".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: Vec::new(),
        collaboration_mode: CollaborationMode::Build,
    });

    assert!(!widget.is_resuming_session_for_test());
    widget.submit_text("after resume".to_string());

    let event = app_event_rx
        .try_recv()
        .expect("user turn should be emitted after session switch");
    assert!(
        matches!(event, AppEvent::Command(AppCommand::UserTurn { .. })),
        "expected user turn after resume, got {event:?}"
    );
}

#[test]
fn resume_browser_supports_page_and_home_end_navigation() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions: Vec<_> = (0..12)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    widget.open_resume_browser_for_test(sessions);
    let _ = rendered_rows(&widget, 80, 10);

    widget.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(3));

    widget.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(11));

    widget.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(0));

    let blob = rendered_rows(&widget, 80, 10).join("\n");
    assert!(
        blob.contains("pgup/pgdn page"),
        "expected paging hint text in resume browser:\n{blob}"
    );
    assert!(
        blob.contains("home/end jump"),
        "expected home/end hint text in resume browser:\n{blob}"
    );
}

#[test]
fn resume_browser_up_down_do_not_wrap_around() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions: Vec<_> = (0..4)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    widget.open_resume_browser_for_test(sessions);

    widget.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(0));

    widget.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(3));

    widget.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(widget.resume_browser_selection_for_test(), Some(3));
}

#[test]
fn resume_browser_shows_position_and_scroll_progress() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let sessions: Vec<_> = (0..12)
        .map(|index| crate::events::SessionListEntry {
            session_id: SessionId::new(),
            title: format!("Session {index}"),
            updated_at: format!("2026-05-{index:02} 10:00"),
            is_active: index == 0,
        })
        .collect();
    widget.open_resume_browser_for_test(sessions);
    let _ = rendered_rows(&widget, 80, 10);
    widget.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));

    let blob = rendered_rows(&widget, 80, 10).join("\n");
    assert!(
        blob.contains("12 / 12"),
        "expected position label in resume header:\n{blob}"
    );
    assert!(
        blob.contains("100%"),
        "expected scroll percent in resume header:\n{blob}"
    );
}

#[test]
fn resume_browser_title_uses_ascii_ellipsis_when_too_long() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.open_resume_browser_for_test(vec![crate::events::SessionListEntry {
        session_id: SessionId::new(),
        title: "This is a very long session title that should be truncated in resume browser"
            .to_string(),
        updated_at: "2026-05-17 10:00".to_string(),
        is_active: true,
    }]);

    let blob = rendered_rows(&widget, 54, 10).join("\n");
    assert!(
        blob.contains("..."),
        "expected ASCII ellipsis truncation in title column:\n{blob}"
    );
}

#[test]
fn resume_browser_dash_only_title_is_truncated_with_ascii_ellipsis() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.open_resume_browser_for_test(vec![crate::events::SessionListEntry {
        session_id: SessionId::new(),
        title: "------------------------------------------------------------".to_string(),
        updated_at: "2026-05-18 10:00".to_string(),
        is_active: true,
    }]);

    let blob = rendered_rows(&widget, 54, 10).join("\n");
    assert!(
        blob.contains("..."),
        "expected dash-only title to be truncated with ASCII ellipsis:\n{blob}"
    );
}

#[test]
fn resume_browser_cjk_title_truncates_by_display_width() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.open_resume_browser_for_test(vec![crate::events::SessionListEntry {
        session_id: SessionId::new(),
        title: "这是一个非常非常长的中文会话标题用于测试截断显示是否正确".to_string(),
        updated_at: "2026-05-18 10:00".to_string(),
        is_active: true,
    }]);

    let blob = rendered_rows(&widget, 54, 10).join("\n");
    assert!(
        blob.contains("..."),
        "expected CJK title truncation to include ASCII ellipsis:\n{blob}"
    );
    assert!(
        !blob.contains("是否正确"),
        "expected tail of long CJK title to be truncated:\n{blob}"
    );
}

#[test]
fn resume_browser_cjk_and_ascii_titles_keep_session_id_column_aligned() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let cjk_session_id = SessionId::new();
    let ascii_session_id = SessionId::new();
    widget.open_resume_browser_for_test(vec![
        crate::events::SessionListEntry {
            session_id: cjk_session_id,
            title: "中文标题用于对齐测试".to_string(),
            updated_at: "2026-05-18 10:00".to_string(),
            is_active: true,
        },
        crate::events::SessionListEntry {
            session_id: ascii_session_id,
            title: "ASCII title".to_string(),
            updated_at: "2026-05-18 10:00".to_string(),
            is_active: false,
        },
    ]);

    let area = ratatui::layout::Rect::new(0, 0, 90, 10);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    widget.render(area, &mut buf);

    let cjk_id_text = cjk_session_id.to_string();
    let ascii_id_text = ascii_session_id.to_string();
    let mut cjk_pos = None;
    let mut ascii_pos = None;
    for row in 0..area.height {
        let row_text = (0..area.width)
            .map(|col| buf[(col, row)].symbol())
            .collect::<String>();
        if row_text.contains(&cjk_id_text) {
            cjk_pos = (0..area.width).find(|col| {
                let tail = (*col..area.width)
                    .map(|scan_col| buf[(scan_col, row)].symbol())
                    .collect::<String>();
                tail.starts_with(&cjk_id_text)
            });
        }
        if row_text.contains(&ascii_id_text) {
            ascii_pos = (0..area.width).find(|col| {
                let tail = (*col..area.width)
                    .map(|scan_col| buf[(scan_col, row)].symbol())
                    .collect::<String>();
                tail.starts_with(&ascii_id_text)
            });
        }
    }
    let cjk_col = cjk_pos.expect("cjk session id column");
    let ascii_col = ascii_pos.expect("ascii session id column");
    assert_eq!(
        cjk_col, ascii_col,
        "expected Session ID column alignment across CJK and ASCII rows"
    );
}

#[test]
fn approval_request_renders_bottom_pane_menu_and_accepts_once() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let session_id = SessionId::new();
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::ApprovalRequest {
        session_id,
        turn_id,
        approval_id: "approval-call-1".to_string(),
        action_summary: "write src/main.rs".to_string(),
        justification: "Tool execution requires approval.".to_string(),
        resource: Some("FileWrite".to_string()),
        available_scopes: vec!["once".to_string(), "session".to_string()],
        path: Some("src/main.rs".to_string()),
        host: None,
        target: None,
        command_pattern: None,
        command_prefix: None,
    });

    let scrollback = widget.drain_scrollback_lines(80);
    assert!(!scrollback_contains_text(
        &scrollback,
        "Permission required"
    ));

    let rendered = rendered_rows(&widget, 80, 24).join("\n");
    assert!(rendered.contains("Permission approval required"));
    assert!(rendered.contains("Yes, proceed"));
    assert!(rendered.contains("don't ask again"));
    assert!(rendered.contains("No, continue without running it"));

    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = app_event_rx.try_recv().expect("approval response event");
    assert_eq!(
        event,
        AppEvent::Command(AppCommand::ApprovalRespond {
            session_id,
            turn_id,
            approval_id: "approval-call-1".to_string(),
            decision: ApprovalDecisionValue::Approve,
            scope: ApprovalScopeValue::Once,
        })
    );
}

#[test]
fn approval_request_does_not_duplicate_already_committed_assistant_text() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let item_id = ItemId::new();
    let text = "明白，我来随便加点内容，测试一下 apply_patch。".to_string();

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id,
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id,
        kind: crate::events::TextItemKind::Assistant,
        delta: text.clone(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id,
        kind: crate::events::TextItemKind::Assistant,
        final_text: text.clone(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::AssistantMessageCompleted(
        text.clone(),
    ));

    widget.handle_worker_event(crate::events::WorkerEvent::ApprovalRequest {
        session_id,
        turn_id,
        approval_id: "approval-call-1".to_string(),
        action_summary: "apply_patch".to_string(),
        justification: "Tool execution requires approval.".to_string(),
        resource: Some("FileWrite".to_string()),
        available_scopes: vec!["once".to_string(), "session".to_string()],
        path: Some("src/main.rs".to_string()),
        host: None,
        target: None,
        command_pattern: None,
        command_prefix: None,
    });

    let transcript = widget.transcript_overlay_lines(100);
    let rows = transcript
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        rows.matches(&text).count(),
        1,
        "assistant text should not be committed twice around approval request:\n{rows}"
    );
}

#[test]
fn approval_request_apply_patch_uses_friendly_label() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let session_id = SessionId::new();
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::ApprovalRequest {
        session_id,
        turn_id,
        approval_id: "approval-call-friendly".to_string(),
        action_summary: "apply_patch".to_string(),
        justification: "Tool execution requires approval.".to_string(),
        resource: Some("FileWrite".to_string()),
        available_scopes: vec!["once".to_string()],
        path: Some("src/main.rs".to_string()),
        host: None,
        target: None,
        command_pattern: None,
        command_prefix: None,
    });

    let rendered = rendered_rows(&widget, 80, 24).join("\n");
    assert!(rendered.contains("Permission approval required"));
    assert!(rendered.contains("Patch"));
}

#[test]
fn approval_request_bottom_pane_menu_denies_with_n_shortcut() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let session_id = SessionId::new();
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::ApprovalRequest {
        session_id,
        turn_id,
        approval_id: "approval-call-2".to_string(),
        action_summary: "run shell command".to_string(),
        justification: "Tool execution requires approval.".to_string(),
        resource: Some("ShellExec".to_string()),
        available_scopes: vec!["once".to_string()],
        path: None,
        host: None,
        target: Some("cargo test".to_string()),
        command_pattern: None,
        command_prefix: None,
    });

    let rendered = rendered_rows(&widget, 80, 24).join("\n");
    assert!(rendered.contains("Permission approval required"));
    assert!(rendered.contains("run shell command"));
    assert!(rendered.contains("No, continue without running it"));

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

    let event = app_event_rx.try_recv().expect("approval response event");
    assert_eq!(
        event,
        AppEvent::Command(AppCommand::ApprovalRespond {
            session_id,
            turn_id,
            approval_id: "approval-call-2".to_string(),
            decision: ApprovalDecisionValue::Deny,
            scope: ApprovalScopeValue::Once,
        })
    );
}

#[test]
fn submitted_prompt_requests_on_request_approval_policy() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.submit_text("please edit a file".to_string());

    let event = app_event_rx.try_recv().expect("user turn event");
    let AppEvent::Command(AppCommand::UserTurn {
        approval_policy, ..
    }) = event
    else {
        panic!("expected user turn command");
    };
    assert_eq!(approval_policy, Some("on-request".to_string()));
}

/// Trace: L2-DES-TUI-003
/// Verifies: Shift+Tab cycles from Build to Plan and marks submitted turns as Plan mode.
#[test]
fn shift_tab_plan_submission_marks_user_turn_plan_mode() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let cwd = PathBuf::from(".");
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd);

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    paste_and_submit(&mut widget, "plan this");

    let AppEvent::Command(AppCommand::UserTurn {
        collaboration_mode, ..
    }) = app_event_rx.try_recv().expect("user turn event")
    else {
        panic!("expected user turn command");
    };
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Plan);
}

fn status_row_starting_with(rows: &[String], mode: &str) -> String {
    rows.iter()
        .find(|row| row.trim_start().starts_with(mode))
        .unwrap_or_else(|| panic!("missing {mode} status row in:\n{}", rows.join("\n")))
        .clone()
}

fn composer_marker_color(widget: &ChatWidget) -> Color {
    let buf = rendered_buffer(widget, 100, 12);
    let area = buf.area;

    for row in 0..area.height {
        let row_text = (0..area.width)
            .map(|col| buf[(col, row)].symbol())
            .collect::<String>();
        if !row_text.contains("Ask Devo") {
            continue;
        }

        for col in 0..area.width {
            let cell = &buf[(col, row)];
            if cell.symbol() == "┃" {
                return cell.fg;
            }
        }
    }

    panic!("missing composer marker in rendered buffer")
}

fn scrollback_marker_color_for_text(widget: &mut ChatWidget, needle: &str) -> Color {
    let lines = widget.drain_scrollback_lines(100);
    for line in &lines {
        let row_text = line
            .line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        if !row_text.contains(needle) {
            continue;
        }

        for span in &line.line.spans {
            if span.content.contains('▌')
                && let Some(color) = span.style.fg
            {
                return color;
            }
        }
    }

    let rendered = scrollback_plain_lines(&lines).join("\n");
    panic!("missing history marker for {needle} in scrollback:\n{rendered}")
}

fn paste_and_submit(widget: &mut ChatWidget, text: &str) {
    widget.handle_paste(text.to_string());
    std::thread::sleep(crate::bottom_pane::ChatComposer::recommended_paste_flush_delay());
    widget.pre_draw_tick();
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

/// Trace: L2-DES-TUI-003
/// Verifies: Mode labels and switch hints render as the first bottom status-line field.
#[test]
fn mode_label_and_switch_hint_render_at_left_of_status_line() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let rows = rendered_rows(&widget, 100, 12);
    let build_row = status_row_starting_with(&rows, "BUILD");
    assert!(
        build_row
            .trim_start()
            .starts_with("BUILD SHIFT+TAB switch ·")
    );

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let rows = rendered_rows(&widget, 100, 12);
    let plan_row = status_row_starting_with(&rows, "PLAN");
    assert!(plan_row.trim_start().starts_with("PLAN SHIFT+TAB switch ·"));

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let rows = rendered_rows(&widget, 100, 12);
    let build_row = status_row_starting_with(&rows, "BUILD");
    assert!(
        build_row
            .trim_start()
            .starts_with("BUILD SHIFT+TAB switch ·")
    );
    assert!(
        rows.iter()
            .all(|row| !row.trim_start().starts_with("SHELL")),
        "Shift+Tab should not enter Shell mode:\n{}",
        rows.join("\n")
    );

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    let rows = rendered_rows(&widget, 100, 12);
    let shell_row = status_row_starting_with(&rows, "SHELL");
    assert!(shell_row.trim_start().starts_with("SHELL ·"));
}

/// Trace: L2-DES-TUI-003
/// Verifies: Composer prompt marker color follows the active input mode.
#[test]
fn composer_marker_uses_active_mode_color() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    assert_eq!(composer_marker_color(&widget), Color::Cyan);

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(composer_marker_color(&widget), Color::Magenta);

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(composer_marker_color(&widget), Color::Cyan);

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    assert_eq!(composer_marker_color(&widget), Color::Rgb(245, 142, 53));
}

/// Trace: L2-DES-TUI-003
/// Verifies: Historical user prompt marker color follows the submitted mode.
#[test]
fn submitted_user_prompt_marker_uses_submitted_mode_color() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::ClearTranscript);

    paste_and_submit(&mut widget, "build message");
    assert_eq!(
        scrollback_marker_color_for_text(&mut widget, "build message"),
        Color::Cyan
    );

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    paste_and_submit(&mut widget, "plan message");
    assert_eq!(
        scrollback_marker_color_for_text(&mut widget, "plan message"),
        Color::Magenta
    );
}

#[test]
fn turn_summary_uses_submitted_mode_after_composer_mode_changes() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::ClearTranscript);

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    widget.handle_paste("plan this".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "done".to_string(),
        turn_count: 1,
        total_input_tokens: 10,
        total_output_tokens: 20,
        total_tokens: 30,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 30,
        last_query_input_tokens: 10,
        prompt_token_estimate: 10,
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    assert!(
        history.contains("▣ PLAN · Test Model"),
        "expected Plan mode in turn summary:
{history}"
    );
}

#[test]
fn queued_prompt_keeps_submitted_mode_when_promoted_to_history() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::ClearTranscript);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });

    widget.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    paste_and_submit(&mut widget, "queued plan");
    widget.handle_worker_event(crate::events::WorkerEvent::InputQueueUpdated {
        pending_count: 0,
        pending_texts: Vec::new(),
    });

    assert_eq!(
        scrollback_marker_color_for_text(&mut widget, "queued plan"),
        Color::Magenta
    );

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "done".to_string(),
        turn_count: 2,
        total_input_tokens: 10,
        total_output_tokens: 20,
        total_tokens: 30,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 30,
        last_query_input_tokens: 10,
        prompt_token_estimate: 10,
    });
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    assert!(
        history.contains("▣ PLAN · Test Model"),
        "expected queued Plan mode in turn summary:
{history}"
    );
}

#[test]
fn queued_prompt_promotes_after_active_assistant_stream() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::ClearTranscript);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    let item_id = ItemId::new();
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id,
        kind: TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id,
        kind: TextItemKind::Assistant,
        delta: "assistant before promotion".to_string(),
    });

    paste_and_submit(&mut widget, "queued prompt");
    widget.handle_worker_event(crate::events::WorkerEvent::InputQueueUpdated {
        pending_count: 0,
        pending_texts: Vec::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id,
        kind: TextItemKind::Assistant,
        final_text: "assistant before promotion".to_string(),
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100));
    let assistant_indexes = history
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.contains("assistant before promotion").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_indexes.len(),
        1,
        "late item completion should not duplicate assistant cell:\n{}",
        history.join("\n")
    );
    let queued_index = history
        .iter()
        .position(|line| line.contains("queued prompt"))
        .expect("queued prompt should be promoted");
    assert!(
        assistant_indexes[0] < queued_index,
        "assistant stream should stay before queued prompt:\n{}",
        history.join("\n")
    );
}

/// Trace: L2-DES-TUI-003
/// Verifies: Bare bang enters Shell mode and submits composer text as a shell command.
#[test]
fn bare_bang_enters_shell_mode_and_submits_shell_command() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    widget.handle_paste("pwd".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().expect("shell command event"),
        AppEvent::Command(AppCommand::SubmitShellInput {
            command: "pwd".to_string(),
        })
    );
}

/// Trace: L2-DES-TUI-003
/// Verifies: `!cmd` submits a one-shot shell command and returns to Build mode.
#[test]
fn bang_command_from_build_submits_one_shot_shell_command() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("!pwd".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().expect("shell command event"),
        AppEvent::Command(AppCommand::ExecuteShellCommand {
            command: "pwd".to_string(),
        })
    );

    widget.handle_paste("next task".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let AppEvent::Command(AppCommand::UserTurn {
        input,
        collaboration_mode,
        ..
    }) = app_event_rx.try_recv().expect("user turn event")
    else {
        panic!("expected user turn command");
    };
    assert_eq!(
        input,
        vec![InputItem::Text {
            text: "next task".to_string()
        }]
    );
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Build);
}

/// Trace: L2-DES-TUI-003
/// Verifies: `\!` escapes a leading bang and submits normal chat.
#[test]
fn escaped_bang_prefix_submits_normal_chat() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("\\!important".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let AppEvent::Command(AppCommand::UserTurn {
        input,
        collaboration_mode,
        ..
    }) = app_event_rx.try_recv().expect("user turn event")
    else {
        panic!("expected user turn command");
    };
    assert_eq!(
        input,
        vec![InputItem::Text {
            text: "!important".to_string()
        }]
    );
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Build);
}

/// Trace: L2-DES-TUI-003
/// Verifies: Leading whitespace before `!` does not trigger Shell mode.
#[test]
fn leading_space_before_bang_submits_normal_chat() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste(" !pwd".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let AppEvent::Command(AppCommand::UserTurn {
        input,
        collaboration_mode,
        ..
    }) = app_event_rx.try_recv().expect("user turn event")
    else {
        panic!("expected user turn command");
    };
    assert_eq!(
        input,
        vec![InputItem::Text {
            text: "!pwd".to_string()
        }]
    );
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Build);
}

#[test]
fn permissions_command_opens_bottom_pane_picker_and_updates_default() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "permissions".to_string(),
    });

    let rendered = rendered_rows(&widget, 100, 18).join("\n");
    assert!(rendered.contains("Update Permissions"));
    assert!(rendered.contains("● 1. Default"));
    assert!(rendered.contains("Auto-review"));
    assert!(rendered.contains("Full Access"));

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));

    let event = app_event_rx.try_recv().expect("permissions update event");
    assert_eq!(
        event,
        AppEvent::Command(AppCommand::UpdatePermissions {
            preset: devo_protocol::PermissionPreset::Default,
        })
    );
}

#[test]
fn busy_widget_blocks_permissions_change_with_transcript_message() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "permissions".to_string(),
    });

    assert!(app_event_rx.try_recv().is_err());

    let scrollback = widget
        .drain_scrollback_lines(80)
        .into_iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(scrollback.contains("Cannot change permissions while generating"));
}

#[test]
fn permissions_command_marks_initial_project_preset_current() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::FullAccess,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "permissions".to_string(),
    });

    let rendered = rendered_rows(&widget, 100, 18).join("\n");
    assert!(rendered.contains("● 3. Full Access"));
}

#[test]
fn sandbox_slash_command_is_removed() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "sandbox".to_string(),
    });

    assert!(
        app_event_rx.try_recv().is_err(),
        "/sandbox should no longer open a picker or emit update commands"
    );
}

#[test]
fn reasoning_effort_entries_are_generated_from_model_capability_options() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        reasoning_capability: ReasoningCapability::Levels(vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        ..Model::default()
    };
    let (widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    assert_eq!(
        widget.reasoning_effort_entries(),
        vec![
            ReasoningEffortListEntry {
                is_current: false,
                label: "Low".to_string(),
                description: "Fastest, cheapest, least deliberative".to_string(),
                value: "low".to_string(),
            },
            ReasoningEffortListEntry {
                is_current: true,
                label: "Medium".to_string(),
                description: "Balanced speed and deliberation".to_string(),
                value: "medium".to_string(),
            },
        ]
    );
}

#[test]
fn initial_reasoning_effort_selection_overrides_model_default() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        reasoning_capability: ReasoningCapability::Levels(vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        ..Model::default()
    };
    let (widget, _app_event_rx) =
        widget_with_model_and_reasoning_effort(model, PathBuf::from("."), Some("low".to_string()));

    assert_eq!(widget.current_reasoning_effort_selection(), Some("low"));
}

#[test]
fn slash_command_list_does_not_include_thinking() {
    let commands = built_in_slash_commands();
    assert!(!commands.iter().any(|(name, _)| *name == "thinking"));
}

#[test]
fn trailing_space_exit_slash_command_exits() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/exit ".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().ok(),
        Some(AppEvent::Exit(crate::app_event::ExitMode::ShutdownFirst))
    );
}

#[test]
fn typed_clear_slash_command_clears_history_and_active_streams() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.handle_app_event(AppEvent::ClearTranscript);

    paste_and_submit(&mut widget, "old prompt");
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "active stream body".to_string(),
    ));
    let before_scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    assert!(before_scrollback.contains("old prompt"));
    let before = rendered_rows(&widget, 100, 16).join(
        "
",
    );
    assert!(before.contains("active stream body"));

    paste_and_submit(&mut widget, "/clear");

    let after_scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    let after = rendered_rows(&widget, 100, 16).join(
        "
",
    );
    assert!(
        !after_scrollback.contains("old prompt") && !after.contains("active stream body"),
        "typed /clear should remove visible history and active streams:
scrollback:
{after_scrollback}
rendered:
{after}"
    );
}

#[test]
fn clear_transcript_event_uses_same_visual_clear_path() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    widget.submit_text("event old prompt".to_string());
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "event active stream".to_string(),
    ));

    widget.handle_app_event(AppEvent::ClearTranscript);

    let after = rendered_rows(&widget, 100, 16).join(
        "
",
    );
    assert!(
        !after.contains("event old prompt") && !after.contains("event active stream"),
        "ClearTranscript should use the same visual clear path:
{after}"
    );
}

#[test]
fn slash_command_parameter_hints_render_for_inline_commands() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };

    let (mut goal_widget, _app_event_rx) = widget_with_model(model.clone(), PathBuf::from("."));
    goal_widget.handle_paste("/goal".to_string());
    let goal_rendered = rendered_rows(&goal_widget, 100, 12).join("\n");
    assert!(
        goal_rendered.contains("/goal <objective for autonomous work>"),
        "expected /goal parameter hint:\n{goal_rendered}"
    );

    let (mut spaced_goal_widget, _app_event_rx) =
        widget_with_model(model.clone(), PathBuf::from("."));
    spaced_goal_widget.handle_paste("/goal ".to_string());
    let spaced_goal_rendered = rendered_rows(&spaced_goal_widget, 100, 12).join("\n");
    assert!(
        spaced_goal_rendered.contains("/goal <objective for autonomous work>"),
        "expected /goal parameter hint after trailing space:\n{spaced_goal_rendered}"
    );
    assert!(
        !spaced_goal_rendered.contains("/goal  <objective for autonomous work>"),
        "parameter hint should not duplicate spaces:\n{spaced_goal_rendered}"
    );

    let (mut btw_widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    btw_widget.handle_paste("/btw".to_string());
    let btw_rendered = rendered_rows(&btw_widget, 100, 12).join("\n");
    assert!(
        btw_rendered.contains("/btw <side conversation message>"),
        "expected /btw parameter hint:\n{btw_rendered}"
    );
}

#[test]
fn goal_slash_command_emits_set_goal_objective() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/goal improve benchmark coverage".to_string());
    let rendered = rendered_rows(&widget, 100, 12).join("\n");
    assert!(
        rendered.contains("/goal improve benchmark coverage"),
        "expected typed /goal objective in composer:\n{rendered}"
    );
    assert!(
        !rendered.contains("<objective for autonomous work>"),
        "parameter hint should disappear after objective text:\n{rendered}"
    );

    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(widget.composer_is_empty());
    let rendered_after_submit = widget
        .transcript_overlay_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_after_submit.contains("/goal improve benchmark coverage"),
        "expected submitted /goal command in history:\n{rendered_after_submit}"
    );

    assert_eq!(
        app_event_rx.try_recv().expect("goal command event"),
        AppEvent::Command(AppCommand::SetGoalObjective {
            objective: "improve benchmark coverage".to_string(),
            mode: crate::app_command::GoalObjectiveMode::ConfirmIfExists,
        })
    );
}

#[test]
fn rename_slash_command_emits_rename_session() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/rename My New Title".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().expect("rename command event"),
        AppEvent::Command(AppCommand::RenameSession {
            title: "My New Title".to_string(),
        })
    );
}

#[test]
fn rename_slash_command_without_title_shows_usage() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/rename".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app_event_rx.try_recv().is_err());
    let rendered = widget
        .transcript_overlay_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /rename <new title>"),
        "expected rename usage hint:\n{rendered}"
    );
}

#[test]
fn delete_slash_command_emits_delete_session() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/delete".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx.try_recv().expect("delete command event"),
        AppEvent::Command(AppCommand::DeleteSession)
    );
}

#[test]
fn goal_control_slash_commands_emit_goal_app_commands() {
    fn event_for_slash(input: &str) -> AppEvent {
        let model = Model {
            slug: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            ..Model::default()
        };
        let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
        widget.handle_paste(input.to_string());
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app_event_rx.try_recv().expect("goal command event")
    }

    assert_eq!(
        event_for_slash("/goal"),
        AppEvent::Command(AppCommand::ShowGoal)
    );
    assert_eq!(
        event_for_slash("/goal pause"),
        AppEvent::Command(AppCommand::SetGoalStatus {
            status: devo_protocol::ThreadGoalStatus::Paused,
        })
    );
    assert_eq!(
        event_for_slash("/goal clear"),
        AppEvent::Command(AppCommand::ClearGoal)
    );
}

#[test]
fn btw_slash_command_clears_composer_and_records_history() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id,
    });
    widget.handle_paste("/btw check the failing edge case".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(widget.composer_is_empty());
    let rendered_after_submit = widget
        .transcript_overlay_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_after_submit.contains("/btw check the failing edge case"),
        "expected submitted /btw command in history:\n{rendered_after_submit}"
    );
    assert_eq!(
        app_event_rx.try_recv().expect("btw command event"),
        AppEvent::Command(AppCommand::RunBtwQuestion {
            question: "check the failing edge case".to_string(),
        })
    );
}

#[test]
fn empty_btw_slash_command_shows_usage() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_paste("/btw ".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(widget.composer_is_empty());
    assert!(app_event_rx.try_recv().is_err());
    let transcript = widget
        .transcript_overlay_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("Usage: /btw <your question>"),
        "expected /btw usage in transcript:\n{transcript}"
    );
}

#[test]
fn btw_completed_renders_temporary_answer() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::BtwCompleted {
        question: "what changed?".to_string(),
        answer: "Only the side answer is shown here.".to_string(),
    });

    let transcript = widget
        .transcript_overlay_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("BTW") && transcript.contains("Only the side answer is shown here."),
        "expected temporary /btw answer in transcript:\n{transcript}"
    );
}

#[test]
fn busy_widget_blocks_model_change_with_transcript_message() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_paste("/model".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app_event_rx.try_recv().is_err());

    let scrollback = widget
        .drain_scrollback_lines(80)
        .into_iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(scrollback.contains("Cannot change model while generating"));
}

#[test]
fn theme_selection_applies_header_accent_immediately() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let aurora_accent = Color::Rgb(0x78, 0xD0, 0xA8);

    // Flush the startup header into scrollback, matching the live TUI path.
    let _ = widget.drain_scrollback_lines(100);

    widget.handle_app_event(AppEvent::ThemeSelected {
        name: "aurora".to_string(),
    });

    let reload = app_event_rx
        .try_recv()
        .expect("theme apply should request an inline transcript reload");
    assert_eq!(reload, AppEvent::ReloadInlineTranscript);

    let reloaded = widget.drain_scrollback_lines(100);
    let reloaded_lines = reloaded
        .iter()
        .map(|line| line.line.clone())
        .collect::<Vec<_>>();
    assert!(
        lines_contain_fg(&reloaded_lines, aurora_accent),
        "header should re-emit with aurora accent after theme selection"
    );
}

#[test]
fn toggle_with_levels_treats_enabled_as_default_effort_in_picker() {
    let model = Model {
        slug: "deepseek-v4".to_string(),
        display_name: "Deepseek V4".to_string(),
        reasoning_capability: ReasoningCapability::ToggleWithLevels(vec![
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..Model::default()
    };
    let (widget, _app_event_rx) = widget_with_model_and_reasoning_effort(
        model,
        PathBuf::from("."),
        Some("enabled".to_string()),
    );

    assert_eq!(
        widget.reasoning_effort_entries(),
        vec![
            ReasoningEffortListEntry {
                is_current: false,
                label: "Off".to_string(),
                description: "Disable reasoning effort for this turn".to_string(),
                value: "disabled".to_string(),
            },
            ReasoningEffortListEntry {
                is_current: true,
                label: "High".to_string(),
                description: "More deliberate for harder tasks".to_string(),
                value: "high".to_string(),
            },
            ReasoningEffortListEntry {
                is_current: false,
                label: "Max".to_string(),
                description: "Most deliberate, highest effort".to_string(),
                value: "max".to_string(),
            },
        ]
    );
}

#[test]
fn reasoning_effort_entries_show_off_and_levels_for_toggle_models_with_supported_levels() {
    let model = devo_core::ModelPreset {
        slug: "deepseek-v4".to_string(),
        display_name: "Deepseek V4".to_string(),
        reasoning_capability: ReasoningCapability::Toggle,
        supported_reasoning_levels: vec![ReasoningEffort::High, ReasoningEffort::Max],
        default_reasoning_effort: None,
        ..devo_core::ModelPreset::default()
    }
    .into();
    let (widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    assert_eq!(
        widget.reasoning_effort_entries(),
        vec![
            ReasoningEffortListEntry {
                is_current: false,
                label: "Off".to_string(),
                description: "Disable reasoning effort for this turn".to_string(),
                value: "disabled".to_string(),
            },
            ReasoningEffortListEntry {
                is_current: true,
                label: "High".to_string(),
                description: "More deliberate for harder tasks".to_string(),
                value: "high".to_string(),
            },
            ReasoningEffortListEntry {
                is_current: false,
                label: "Max".to_string(),
                description: "Most deliberate, highest effort".to_string(),
                value: "max".to_string(),
            },
        ]
    );
}

#[test]
fn submit_text_emits_user_turn_with_model_and_reasoning_effort() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        reasoning_capability: ReasoningCapability::Toggle,
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());

    widget.set_reasoning_effort_selection(Some("disabled".to_string()));
    widget.submit_text("hello".to_string());

    assert_eq!(
        app_event_rx.try_recv().expect("command event is emitted"),
        AppEvent::Command(AppCommand::UserTurn {
            input: vec![InputItem::Text {
                text: "hello".to_string(),
            }],
            cwd: Some(cwd),
            model: Some("test-model".to_string()),

            model_binding_id: None,
            reasoning_effort_selection: Some("disabled".to_string()),
            sandbox: None,
            approval_policy: Some("on-request".to_string()),
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        })
    );
}

#[test]
fn typed_character_submits_after_paste_burst_flush() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    std::thread::sleep(crate::bottom_pane::ChatComposer::recommended_paste_flush_delay());
    widget.pre_draw_tick();
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

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

            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: Some("on-request".to_string()),
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        })
    );
}

fn assert_no_command_emitted(app_event_rx: &mut mpsc::UnboundedReceiver<AppEvent>) {
    let command = std::iter::from_fn(|| app_event_rx.try_recv().ok())
        .find(|event| matches!(event, AppEvent::Command(_)));
    assert_eq!(command, None);
}

fn submitted_text_after_modified_enter(
    modifier: KeyModifiers,
    test_model: Model,
    cwd: PathBuf,
) -> String {
    let (mut widget, mut app_event_rx) = widget_with_model(test_model, cwd);

    widget.handle_paste("hello".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, modifier));
    assert_no_command_emitted(&mut app_event_rx);
    widget.handle_paste("world".to_string());
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let emitted_command = std::iter::from_fn(|| app_event_rx.try_recv().ok())
        .find(|event| matches!(event, AppEvent::Command(_)))
        .expect("command event is emitted");
    let AppEvent::Command(AppCommand::UserTurn { input, .. }) = emitted_command else {
        unreachable!("filtered for user command");
    };
    let [InputItem::Text { text }] = input.as_slice() else {
        panic!("expected one text input item, got {input:?}");
    };
    text.clone()
}

#[test]
fn shift_enter_inserts_newline_in_composer_without_submitting() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };

    let text = submitted_text_after_modified_enter(KeyModifiers::SHIFT, model, cwd);

    assert_eq!(text, "hello\nworld");
}

#[test]
fn ctrl_enter_inserts_newline_in_composer_without_submitting() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };

    let text = submitted_text_after_modified_enter(KeyModifiers::CONTROL, model, cwd);

    assert_eq!(text, "hello\nworld");
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
    std::thread::sleep(crate::bottom_pane::ChatComposer::recommended_paste_flush_delay());
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

            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: Some("on-request".to_string()),
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        })
    );
}

#[test]
fn plan_update_updates_progress_and_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::PlanUpdated {
        explanation: Some("Working through checklist".to_string()),
        steps: vec![
            PlanStep {
                text: "Inspect implementation".to_string(),
                status: PlanStepStatus::Completed,
            },
            PlanStep {
                text: "Patch runtime".to_string(),
                status: PlanStepStatus::InProgress,
            },
        ],
    });

    assert_eq!(widget.last_plan_progress_for_test(), Some((1, 2)));

    let lines = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(lines.iter().any(|line| line.contains("Updated Plan")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Working through checklist"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Inspect implementation"))
    );
    assert!(lines.iter().any(|line| line.contains("Patch runtime")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("  ✔ Inspect implementation"))
    );
    assert!(lines.iter().any(|line| line.contains("  → Patch runtime")));
}

#[test]
fn proposed_plan_keeps_assistant_preamble_before_plan() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let assistant_id = ItemId::new();
    let plan_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
        delta: "现在我已经了解了代码库。以下是计划：\n".to_string(),
    });
    widget
        .handle_worker_event(crate::events::WorkerEvent::ProposedPlanStarted { item_id: plan_id });
    widget.handle_worker_event(crate::events::WorkerEvent::ProposedPlanDelta {
        item_id: plan_id,
        delta: "## Summary\n\nBuild the feature.".to_string(),
    });

    let mut lines = scrollback_plain_lines(&widget.drain_scrollback_lines(100));
    lines.extend(line_texts(widget.active_viewport_lines_for_test(100)));
    let preamble_index = lines
        .iter()
        .position(|line| line.contains("现在我已经了解了代码库"))
        .expect("assistant preamble is rendered");
    let plan_index = lines
        .iter()
        .position(|line| line.contains("Build the feature"))
        .expect("proposed plan is rendered");
    assert!(
        preamble_index < plan_index,
        "assistant preamble should render before proposed plan:\n{}",
        lines.join("\n")
    );
}

#[test]
fn proposed_plan_completion_does_not_duplicate_boundary_preamble() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let assistant_id = ItemId::new();
    let plan_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
        delta: "Intro before plan.\n".to_string(),
    });
    widget
        .handle_worker_event(crate::events::WorkerEvent::ProposedPlanStarted { item_id: plan_id });
    widget.handle_worker_event(crate::events::WorkerEvent::ProposedPlanCompleted {
        item_id: plan_id,
        final_text: "## Summary\n\nBuild the feature.".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id: assistant_id,
        kind: TextItemKind::Assistant,
        final_text: "Intro before plan.\n".to_string(),
    });

    let rendered = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert_eq!(rendered.matches("Intro before plan.").count(), 1);
}

#[test]
fn proposed_plan_cell_renders_markdown_body_only() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let cell = crate::history_cell::new_proposed_plan(
        "## Summary\n\nBuild the feature.".to_string(),
        &cwd,
    );
    let lines = line_texts(cell.display_lines(100));
    let rendered = lines.join("\n");

    assert!(rendered.contains("Summary"));
    assert!(rendered.contains("Build the feature."));
    assert!(!rendered.contains("Proposed Plan"));
    assert!(!rendered.contains("Implement Plan"));
    assert!(!rendered.contains("Revise Plan"));
}

#[test]
fn session_switch_restores_plan_mode_and_proposed_plan_actions() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-plan".to_string(),
        cwd,
        title: Some("Plan session".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: None,
            kind: devo_protocol::SessionHistoryItemKind::Assistant,
            title: String::new(),
            body: "## Approach\n\n1. Inspect\n2. Patch\n".to_string(),
            tool_io: None,
            metadata: Some(devo_protocol::SessionHistoryMetadata::ProposedPlan),
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: Vec::new(),
        collaboration_mode: CollaborationMode::Plan,
    });

    assert_eq!(
        widget.input_mode_for_test(),
        crate::bottom_pane::InputMode::Plan
    );
    assert!(widget.has_bottom_pane_view_for_test());
    assert_eq!(widget.status_message_for_test(), "Choose plan action");

    let rendered = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        rendered.contains("Inspect") && rendered.contains("Patch"),
        "expected Proposed Plan body after resume:\n{rendered}"
    );
}

#[test]
fn session_switch_restores_plan_turn_summary_label() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-plan-summary".to_string(),
        cwd,
        title: Some("Plan session".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::Assistant,
                title: String::new(),
                body: "## Approach\n\n1. Inspect\n2. Patch\n".to_string(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::ProposedPlan),
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::TurnSummary,
                title: "Test Model".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::TurnSummary {
                    collaboration_mode: CollaborationMode::Plan,
                }),
                duration_ms: Some(5),
            },
        ],
        loaded_item_count: 2,
        pending_texts: Vec::new(),
        collaboration_mode: CollaborationMode::Plan,
    });

    assert_eq!(
        widget.input_mode_for_test(),
        crate::bottom_pane::InputMode::Plan
    );
    let rendered = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        rendered.contains("▣ PLAN · Test Model"),
        "expected Plan mode in restored turn summary:\n{rendered}"
    );
    assert!(
        !rendered.contains("▣ BUILD · Test Model"),
        "did not expect Build mode in restored plan turn summary:\n{rendered}"
    );
}

#[test]
fn session_switch_after_implement_stays_in_build_without_plan_actions() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-build".to_string(),
        cwd,
        title: Some("Build session".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::Assistant,
                title: String::new(),
                body: "## Approach\n\n1. Inspect\n2. Patch\n".to_string(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::ProposedPlan),
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::User,
                title: String::new(),
                body: "Implement Plan".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
        ],
        loaded_item_count: 2,
        pending_texts: Vec::new(),
        collaboration_mode: CollaborationMode::Build,
    });

    assert_eq!(
        widget.input_mode_for_test(),
        crate::bottom_pane::InputMode::Build
    );
    assert!(!widget.has_bottom_pane_view_for_test());
    assert_eq!(widget.status_message_for_test(), "Session switched");
}

#[test]
fn proposed_plan_implement_action_sends_build_turn() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());
    let plan_id = ItemId::new();

    widget
        .handle_worker_event(crate::events::WorkerEvent::ProposedPlanStarted { item_id: plan_id });
    widget.handle_worker_event(crate::events::WorkerEvent::ProposedPlanCompleted {
        item_id: plan_id,
        final_text: "## Summary\n\nBuild the feature.".to_string(),
    });
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = app_event_rx.try_recv().expect("implement event is emitted");
    widget.handle_app_event(event.clone());
    assert_eq!(
        widget.input_mode_for_test(),
        crate::bottom_pane::InputMode::Build
    );
    let AppEvent::Command(AppCommand::UserTurn {
        input,
        cwd: event_cwd,
        collaboration_mode,
        ..
    }) = event
    else {
        panic!("expected build user turn");
    };
    assert_eq!(event_cwd, Some(cwd));
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Build);
    assert_eq!(
        input,
        vec![InputItem::Text {
            text: "Implement Plan".to_string(),
        }]
    );
}

#[test]
fn proposed_plan_revise_action_submits_plan_turn_with_feedback() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, cwd.clone());
    let plan_id = ItemId::new();

    widget
        .handle_worker_event(crate::events::WorkerEvent::ProposedPlanStarted { item_id: plan_id });
    widget.handle_worker_event(crate::events::WorkerEvent::ProposedPlanCompleted {
        item_id: plan_id,
        final_text: "## Summary\n\nBuild the feature.".to_string(),
    });
    widget.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for ch in "Skip migrations".chars() {
        widget.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = app_event_rx.try_recv().expect("revise event is emitted");
    widget.handle_app_event(event.clone());
    assert_eq!(
        widget.input_mode_for_test(),
        crate::bottom_pane::InputMode::Plan
    );
    let AppEvent::Command(AppCommand::UserTurn {
        input,
        cwd: event_cwd,
        collaboration_mode,
        ..
    }) = event
    else {
        panic!("expected plan user turn");
    };
    assert_eq!(event_cwd, Some(cwd));
    assert_eq!(collaboration_mode, devo_protocol::CollaborationMode::Plan);
    assert_eq!(
        input,
        vec![InputItem::Text {
            text: "Skip migrations".to_string(),
        }]
    );
    assert!(app_event_rx.try_recv().is_err());
}

#[test]
fn session_switch_restores_plan_metadata_into_progress() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: None,
            kind: devo_protocol::SessionHistoryItemKind::Assistant,
            title: String::new(),
            body: r#"{"explanation":"Do work","plan":[{"step":"Inspect","status":"completed"},{"step":"Patch","status":"in_progress"}]}"#.to_string(),
            tool_io: None,
            metadata: Some(devo_protocol::SessionHistoryMetadata::PlanUpdate {
                explanation: Some("Do work".to_string()),
                steps: vec![
                    devo_protocol::SessionPlanStep {
                        text: "Inspect".to_string(),
                        status: devo_protocol::SessionPlanStepStatus::Completed,
                    },
                    devo_protocol::SessionPlanStep {
                        text: "Patch".to_string(),
                        status: devo_protocol::SessionPlanStepStatus::InProgress,
                    },
                ],
            }),
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    assert_eq!(widget.last_plan_progress_for_test(), Some((1, 2)));
}

#[test]
fn session_switch_restores_explored_metadata_into_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::CommandExecution,
            title: "cat foo.txt".to_string(),
            body: "hello".to_string(),
            tool_io: None,
            metadata: Some(devo_protocol::SessionHistoryMetadata::Explored {
                actions: vec![devo_protocol::parse_command::ParsedCommand::Read {
                    cmd: "cat foo.txt".to_string(),
                    name: "foo.txt".to_string(),
                    path: PathBuf::from("foo.txt"),
                }],
            }),
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Explored") || blob.contains("Exploring"),
        "expected explored block after resume, got:\n{blob}"
    );
    assert!(
        blob.contains("Read foo.txt"),
        "expected read summary, got:\n{blob}"
    );
}

#[test]
fn session_switch_restores_edited_metadata_into_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::ToolResult,
            title: "apply_patch".to_string(),
            body: String::new(),
            tool_io: None,
            metadata: Some(devo_protocol::SessionHistoryMetadata::Edited { changes }),
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Edited foo.txt") || blob.contains("Edited 1 file"),
        "expected edited block after resume, got:\n{blob}"
    );
}

#[test]
fn session_switch_merges_consecutive_explored_items() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![],
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolCall,
                title: "read crates/tui/src/worker.rs".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::Explored {
                    actions: vec![devo_protocol::parse_command::ParsedCommand::Read {
                        cmd: "read crates/tui/src/worker.rs".to_string(),
                        name: "worker.rs".to_string(),
                        path: PathBuf::from("crates/tui/src/worker.rs"),
                    }],
                }),
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-2".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolCall,
                title: "grep command_actions in crates/tui/src/worker.rs".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::Explored {
                    actions: vec![devo_protocol::parse_command::ParsedCommand::Search {
                        cmd: "grep command_actions in crates/tui/src/worker.rs".to_string(),
                        query: Some("command_actions".to_string()),
                        path: Some("crates/tui/src/worker.rs".to_string()),
                    }],
                }),
                duration_ms: None,
            },
        ],
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert_eq!(
        blob.matches("Explored").count() + blob.matches("Exploring").count(),
        1,
        "expected one merged explored block, got:\n{blob}"
    );
    assert!(
        blob.contains("Read crates/tui/src/worker.rs"),
        "expected read entry, got:\n{blob}"
    );
    assert!(
        blob.contains("Search command_actions in crates/tui/src/worker.rs"),
        "expected search entry, got:\n{blob}"
    );
}

#[test]
fn session_switch_restores_error_via_tool_result_cell_style() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![],
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::Error,
            title: "bash error".to_string(),
            body: "permission denied".to_string(),
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Ran bash error"),
        "expected tool-result style title, got:\n{blob}"
    );
    assert!(
        blob.contains("permission denied"),
        "expected tool-result body, got:\n{blob}"
    );
}

#[test]
fn rich_session_restore_orders_terminal_error_before_single_failed_footer() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());
    let terminal_error = "exact persisted provider failure";

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: None,
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![],
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolCall,
                title: "bash error".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::Error,
                title: "bash error".to_string(),
                body: "permission denied".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::Error,
                title: "PROVIDER_SERVER_ERROR".to_string(),
                body: terminal_error.to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::TurnSummary,
                title: "test-model".to_string(),
                body: "failed".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: Some(7),
            },
        ],
        loaded_item_count: 4,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    let tool_error_index = history
        .find("permission denied")
        .expect("history should contain tool error");
    let terminal_error_index = history
        .find(terminal_error)
        .expect("history should contain exact terminal error");
    let failed_index = history
        .find(" · failed")
        .expect("history should contain failed footer");
    assert!(
        tool_error_index < terminal_error_index && terminal_error_index < failed_index,
        "expected tool error < terminal error < failed footer:\n{history}"
    );
    assert!(history.contains("Ran bash error"), "history:\n{history}");
    assert_eq!(history.matches(" · failed").count(), 1);
    assert_eq!(history.matches(terminal_error).count(), 1);
    assert!(
        !history.contains("PROVIDER_SERVER_ERROR"),
        "history:\n{history}"
    );
}

#[test]
fn live_and_resume_error_share_same_rendering_chain() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut live_widget, _live_rx) = widget_with_model(model.clone(), PathBuf::from("."));
    let (mut resume_widget, _resume_rx) = widget_with_model(model, PathBuf::from("."));

    live_widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "bash error".to_string(),
        preview: "permission denied".to_string(),
        is_error: true,
        truncated: false,
    });
    let live_blob = scrollback_plain_lines(&live_widget.drain_scrollback_lines(80))
        .into_iter()
        .filter(|line| line.contains("Ran bash error") || line.contains("permission denied"))
        .collect::<Vec<_>>()
        .join("\n");

    resume_widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![],
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::Error,
            title: "bash error".to_string(),
            body: "permission denied".to_string(),
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });
    let resume_blob = scrollback_plain_lines(&resume_widget.drain_scrollback_lines(80))
        .into_iter()
        .filter(|line| line.contains("Ran bash error") || line.contains("permission denied"))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        live_blob, resume_blob,
        "live and resume error cells diverged"
    );
}

#[test]
fn startup_header_mascot_animation_advances_on_pre_draw_tick() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    assert_eq!(widget.startup_header_mascot_frame_index(), 0);

    widget.force_startup_header_animation_due();
    widget.pre_draw_tick();

    assert_eq!(widget.startup_header_mascot_frame_index(), 1);
}

#[test]
fn onboarding_view_is_active_on_first_run() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (_widget, _app_event_rx) = onboarding_widget_with_model(model, cwd);
    // Onboarding widget is owned by ChatWidget when show_model_onboarding is true.
}

#[test]
fn onboarding_validation_succeeded_waits_for_provider_upsert() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = onboarding_widget_with_available_model(model, cwd);

    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorsListed {
        provider_vendors: vec![ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        }],
    });
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    let _ = app_event_rx.try_recv().expect("onboard command");

    widget.handle_worker_event(crate::events::WorkerEvent::ProviderValidationSucceeded {
        reply_preview: "OK".to_string(),
    });

    assert_eq!(widget.is_onboarding_active(), true);

    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorUpserted {
        provider_vendor: ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        },
        model_binding: Some(ProviderModelBinding {
            binding_id: "deepseek-v4-flash-deepseek".to_string(),
            model_slug: "deepseek-v4-flash".to_string(),
            provider: "Deepseek".to_string(),
            request_model: "DeepSeek-V4-Flash".to_string(),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
            enabled: true,
        }),
    });

    assert_eq!(
        app_event_rx.try_recv().expect("onboarding completed"),
        AppEvent::OnboardingCompleted
    );
    assert_eq!(app_event_rx.try_recv().is_err(), true);
    assert_eq!(widget.is_onboarding_active(), false);
    assert_eq!(widget.placeholder_text(), "Ask Devo");
    assert_eq!(
        widget.current_model().map(|model| model.slug.as_str()),
        Some("deepseek-v4-flash")
    );
    assert_eq!(
        widget.status_summary_text().contains("DeepSeek-V4-Flash"),
        true
    );
}

#[test]
fn onboarding_validation_succeeded_exits_when_configured() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) =
        onboarding_widget_with_available_model_and_exit_after_onboarding(
            model, cwd, /*exit_after_onboarding*/ true,
        );

    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorsListed {
        provider_vendors: vec![ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        }],
    });
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    let _ = app_event_rx.try_recv().expect("onboard command");

    widget.handle_worker_event(crate::events::WorkerEvent::ProviderValidationSucceeded {
        reply_preview: "OK".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorUpserted {
        provider_vendor: ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        },
        model_binding: Some(ProviderModelBinding {
            binding_id: "deepseek-v4-flash-deepseek".to_string(),
            model_slug: "deepseek-v4-flash".to_string(),
            provider: "Deepseek".to_string(),
            request_model: "DeepSeek-V4-Flash".to_string(),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
            enabled: true,
        }),
    });

    assert_eq!(widget.is_onboarding_active(), false);
    assert_eq!(
        app_event_rx.try_recv().expect("onboarding completed"),
        AppEvent::OnboardingCompleted
    );
    assert_eq!(
        app_event_rx.try_recv().expect("exit event"),
        AppEvent::Exit(ExitMode::ShutdownFirst)
    );
}

#[test]
fn onboarding_validation_bypassed_exits_when_configured() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "Deepseek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) =
        onboarding_widget_with_available_model_and_exit_after_onboarding(
            model, cwd, /*exit_after_onboarding*/ true,
        );

    let _ = app_event_rx.try_recv().expect("provider list command");
    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorsListed {
        provider_vendors: vec![ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        }],
    });
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    widget.handle_key_event(press_key(KeyCode::Enter));
    let _ = app_event_rx.try_recv().expect("onboard command");

    widget.handle_worker_event(crate::events::WorkerEvent::ProviderValidationFailed {
        message: "validation failed".to_string(),
        hint: None,
    });
    widget.handle_key_event(press_key(KeyCode::Enter));
    match app_event_rx.try_recv().expect("skip validation command") {
        AppEvent::Command(AppCommand::RunUserShellCommand { command }) => {
            assert_eq!(command.starts_with("onboard-skip-validation "), true);
        }
        other => panic!("expected skip validation command, got {other:?}"),
    }

    widget.handle_worker_event(crate::events::WorkerEvent::ProviderVendorUpserted {
        provider_vendor: ProviderVendor {
            name: "Deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            credential: Some("deepseek_api_key".to_string()),
            headers: None,
            wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
            enabled: true,
        },
        model_binding: Some(ProviderModelBinding {
            binding_id: "deepseek-v4-flash-deepseek".to_string(),
            model_slug: "deepseek-v4-flash".to_string(),
            provider: "Deepseek".to_string(),
            request_model: "DeepSeek-V4-Flash".to_string(),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            invocation_method: ProviderWireApi::OpenAIChatCompletions,
            default_reasoning_effort: None,
            enabled: true,
        }),
    });

    assert_eq!(widget.is_onboarding_active(), false);
    assert_eq!(
        app_event_rx.try_recv().expect("onboarding completed"),
        AppEvent::OnboardingCompleted
    );
    assert_eq!(
        app_event_rx.try_recv().expect("exit event"),
        AppEvent::Exit(ExitMode::ShutdownFirst)
    );
}

#[test]
fn onboarding_paste_does_not_write_to_composer() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = onboarding_widget_with_model(model, cwd);

    widget.handle_paste("https://api.example.com/v1".to_string());

    assert!(widget.is_onboarding_active());
    assert!(widget.composer_is_empty());
}

/// Trace: L2-DES-TUI-001
/// Verifies: Esc during model selection cancels onboarding and exits program
#[test]
fn onboarding_esc_cancels_onboarding_and_exits() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = onboarding_widget_with_model(model, cwd);

    // Onboarding should be active.
    assert!(widget.is_onboarding_active());

    // Press Esc — should cancel onboarding and request exit.
    let esc = KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(esc);

    // Onboarding should be cleared.
    assert!(!widget.is_onboarding_active());

    // An exit event should have been sent.
    let event = app_event_rx.try_recv();
    assert!(event.is_ok(), "expected an AppEvent after Esc cancel");
}

/// Trace: L2-DES-TUI-001
/// Verifies: Down/Up navigation works during model selection
#[test]
fn onboarding_up_down_navigates_model_list() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    let _cwd = std::env::current_dir().expect("current directory is available");
    let models = vec![
        Model {
            slug: "model-a".to_string(),
            display_name: "Model A".to_string(),
            ..Model::default()
        },
        Model {
            slug: "model-b".to_string(),
            display_name: "Model B".to_string(),
            ..Model::default()
        },
        Model {
            slug: "model-c".to_string(),
            display_name: "Model C".to_string(),
            ..Model::default()
        },
    ];
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = crate::onboarding_widget::OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );

    // Initial state: first item selected (index 0).
    // Press Down — should move to index 1.
    let down = KeyEvent {
        code: KeyCode::Down,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(down);

    // Press Down again — should move to index 2.
    widget.handle_key_event(down);

    // Press Up — should move back to index 1.
    let up = KeyEvent {
        code: KeyCode::Up,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(up);

    // Widget should still be active (not completed by navigation).
    assert!(!widget.is_complete());
}

/// Trace: L2-DES-TUI-001
/// Verifies: Go-back from provider selection restores model selection
#[test]
fn onboarding_go_back_restores_model_selection() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    let _cwd = std::env::current_dir().expect("current directory is available");
    let models = vec![
        Model {
            slug: "model-a".to_string(),
            display_name: "Model A".to_string(),
            ..Model::default()
        },
        Model {
            slug: "model-b".to_string(),
            display_name: "Model B".to_string(),
            ..Model::default()
        },
    ];
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = crate::onboarding_widget::OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );

    // Select first model (Enter on index 0 → goes to BaseUrl step).
    let enter = KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(enter);

    // Now press Esc — should go back to model selection (not cancel).
    let esc = KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(esc);

    // Widget should still be active — go-back, not cancel.
    assert!(!widget.is_complete());
    // Onboarding should still be active in the chat widget sense.
    assert!(widget.take_result().is_none());
}

/// Trace: L2-DES-TUI-001
/// Verifies: onboarding model selection only offers catalog-backed models
#[test]
fn onboarding_model_selection_uses_catalog_models_only() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    let _cwd = std::env::current_dir().expect("current directory is available");
    let models = vec![Model {
        slug: "model-a".to_string(),
        display_name: "Model A".to_string(),
        ..Model::default()
    }];
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = crate::onboarding_widget::OnboardingWidget::new(
        &models,
        AppEventSender::new(app_event_tx),
        FrameRequester::test_dummy(),
        true,
    );

    // With one catalog model, Down wraps back to that same catalog-backed entry.
    let down = KeyEvent {
        code: KeyCode::Down,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(down);

    // Select the catalog model.
    let enter = KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    widget.handle_key_event(enter);

    // Widget should still be active (moved to provider selection, not completed).
    assert!(!widget.is_complete());
}

/// Trace: L2-DES-TUI-001, L2-DES-APP-007
/// Verifies: is_onboarding_active reflects widget state correctly
#[test]
fn chatwidget_is_onboarding_active_tracks_state() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (widget, _app_event_rx) = onboarding_widget_with_model(model, cwd);

    // Should be active initially.
    assert!(widget.is_onboarding_active());
    // is_normal_backtrack_mode should be false during onboarding.
    assert!(!widget.is_normal_backtrack_mode());
}

#[test]
fn streamed_lines_stay_in_live_viewport_until_turn_finishes() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model.clone(), cwd);

    let base_height = widget.desired_height(80);
    for index in 0..12 {
        widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(format!(
            "line {index}\n"
        )));
    }

    assert!(widget.desired_height(80) > base_height);

    let committed_before_finish = widget.drain_scrollback_lines(80);
    let committed_before_finish_text = committed_before_finish
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(!committed_before_finish_text.contains("line 0"));
    assert!(!committed_before_finish_text.contains("line 11"));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "stop".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed_after_finish = widget.drain_scrollback_lines(80);
    let committed_after_finish_text = committed_after_finish
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(committed_after_finish_text.contains("line 0"));
    assert!(committed_after_finish_text.contains("line 11"));
}

#[test]
fn committed_history_drains_to_scrollback_lines() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model.clone(), cwd.clone());

    let initial_lines = widget.drain_scrollback_lines(80);
    assert!(!initial_lines.is_empty());

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "done".to_string(),
        turn_count: 1,
        total_input_tokens: 10,
        total_output_tokens: 20,
        total_tokens: 30,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 30,
        last_query_input_tokens: 10,
        prompt_token_estimate: 10,
    });

    let committed_lines = trim_trailing_blank_scrollback_lines(widget.drain_scrollback_lines(80));
    // TurnSummaryCell is now added on TurnFinished, so scrollback is non-empty.
    assert!(
        !committed_lines.is_empty(),
        "TurnSummaryCell should be committed"
    );
    assert!(
        committed_lines.iter().any(|line| {
            line.line
                .spans
                .iter()
                .any(|span| span.content.contains("▣"))
        }),
        "expected ▣ symbol in turn summary"
    );
}

#[test]
fn streamed_history_stays_empty_until_turn_finishes() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model.clone(), cwd.clone());

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "first\nsecond\n".to_string(),
    ));

    let committed_lines = trim_trailing_blank_scrollback_lines(widget.drain_scrollback_lines(80));
    assert!(committed_lines.is_empty());
}

#[test]
fn batched_history_inserts_separator_and_trailing_blank_lines() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model.clone(), cwd.clone());

    let _ = widget.drain_scrollback_lines(80);
    widget.add_to_history(crate::history_cell::new_info_event(
        "first".to_string(),
        None,
    ));
    widget.add_to_history(crate::history_cell::new_info_event(
        "second".to_string(),
        None,
    ));

    let committed_lines = widget.drain_scrollback_lines(80);
    let blank_lines = committed_lines
        .iter()
        .filter(|line| {
            line.line
                .spans
                .iter()
                .all(|span| span.content.trim().is_empty())
        })
        .count();

    assert_eq!(
        2, blank_lines,
        "unexpected blank lines: {committed_lines:?}"
    );
}

#[test]
fn session_switch_restores_header_and_spacing_before_user_input() {
    let initial_cwd = std::env::current_dir().expect("current directory is available");
    let resumed_cwd = initial_cwd.join("resumed");
    let model = Model {
        slug: "initial-model".to_string(),
        display_name: "Initial Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, initial_cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.add_to_history(crate::history_cell::new_info_event(
        "session 1 lingering line".to_string(),
        None,
    ));
    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: resumed_cwd.clone(),
        title: Some("Resumed".to_string()),
        model: Some("resumed-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 3,
        total_output_tokens: 5,
        total_tokens: 8,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 8,
        last_query_input_tokens: 3,
        prompt_token_estimate: 3,
        history_items: vec![
            crate::events::TranscriptItem::new(
                crate::events::TranscriptItemKind::User,
                String::new(),
                "hello".to_string(),
            ),
            crate::events::TranscriptItem::new(
                crate::events::TranscriptItemKind::Assistant,
                String::new(),
                "world".to_string(),
            ),
        ],
        rich_history_items: Vec::new(),
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let committed_lines = widget.drain_scrollback_lines(80);
    let committed_text = committed_lines
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let committed_rows = committed_lines
        .iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    // The header box is rendered only once on initial launch, not on session switch.
    assert_eq!(0, committed_text.matches("directory:").count());
    assert!(committed_text.contains("hello"));
    assert!(committed_text.contains("world"));
    assert!(!committed_text.contains("session 1 lingering line"));
    assert!(
        committed_rows
            .windows(5)
            .any(|window| window[0].contains("▌ hello")
                && window[1].trim().is_empty()
                && window[2].trim().is_empty()
                && window[3].contains("world")),
        "expected restored spaced user prompt before assistant response: {committed_lines:?}"
    );
}

#[test]
fn restored_user_spacing_matches_live_turn_batch_spacing() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut live_widget, mut live_rx) = widget_with_model(model.clone(), cwd.clone());
    let (mut restored_widget, _restored_rx) = widget_with_model(model, cwd.clone());

    let _ = live_widget.drain_scrollback_lines(80);
    live_widget.submit_text("hello".to_string());
    let _ = live_rx.try_recv().expect("submitted user turn");
    let mut live_rows = scrollback_plain_lines(&live_widget.drain_scrollback_lines(80));
    live_widget.add_markdown_history("Assistant", "world");
    live_rows.extend(scrollback_plain_lines(
        &live_widget.drain_scrollback_lines(80),
    ));

    let _ = restored_widget.drain_scrollback_lines(80);
    restored_widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: Some("Resumed".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 3,
        total_output_tokens: 5,
        total_tokens: 8,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 8,
        last_query_input_tokens: 3,
        prompt_token_estimate: 3,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem::new(
                None,
                devo_protocol::SessionHistoryItemKind::User,
                String::new(),
                "hello".to_string(),
            ),
            devo_protocol::SessionHistoryItem::new(
                None,
                devo_protocol::SessionHistoryItemKind::Assistant,
                String::new(),
                "world".to_string(),
            ),
        ],
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });
    let restored_rows = scrollback_plain_lines(&restored_widget.drain_scrollback_lines(80));

    let live_user = live_rows
        .iter()
        .position(|row| row.contains("▌ hello"))
        .expect("live user row");
    let live_assistant = live_rows
        .iter()
        .position(|row| row.contains("world"))
        .expect("live assistant row");
    let restored_user = restored_rows
        .iter()
        .position(|row| row.contains("▌ hello"))
        .expect("restored user row");
    let restored_assistant = restored_rows
        .iter()
        .position(|row| row.contains("world"))
        .expect("restored assistant row");

    let live_gap = &live_rows[live_user + 1..live_assistant];
    let restored_gap = &restored_rows[restored_user + 1..restored_assistant];
    assert_eq!(
        live_gap, restored_gap,
        "restored user-to-assistant spacing should match live turn batching\nlive: {live_rows:?}\nrestored: {restored_rows:?}"
    );
}

#[test]
fn rich_session_switch_restores_user_spacing_before_assistant_response() {
    let initial_cwd = std::env::current_dir().expect("current directory is available");
    let resumed_cwd = initial_cwd.join("resumed");
    let model = Model {
        slug: "initial-model".to_string(),
        display_name: "Initial Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, initial_cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: resumed_cwd,
        title: Some("Resumed".to_string()),
        model: Some("resumed-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 3,
        total_output_tokens: 5,
        total_tokens: 8,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 8,
        last_query_input_tokens: 3,
        prompt_token_estimate: 3,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem::new(
                None,
                devo_protocol::SessionHistoryItemKind::User,
                String::new(),
                "hello".to_string(),
            ),
            devo_protocol::SessionHistoryItem::new(
                None,
                devo_protocol::SessionHistoryItemKind::Assistant,
                String::new(),
                "world".to_string(),
            ),
        ],
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let committed_rows = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(
        committed_rows
            .windows(5)
            .any(|window| window[0].contains("▌ hello")
                && window[1].trim().is_empty()
                && window[2].trim().is_empty()
                && window[3].contains("world")),
        "expected restored rich user prompt to keep live spacing before assistant response: {committed_rows:?}"
    );
}

#[test]
fn turn_finished_does_not_add_completion_status_line_to_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model.clone(), cwd.clone());

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed_lines = widget.drain_scrollback_lines(80);
    assert!(!committed_lines.iter().any(|line| {
        line.line
            .spans
            .iter()
            .any(|span| span.content.contains("Turn completed (Completed)"))
    }));
}

#[test]
fn completed_turn_summary_keeps_duration_for_text_turns() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.force_task_elapsed_seconds(257);
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta("hello".to_string()));
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed = widget
        .drain_scrollback_lines(80)
        .into_iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(committed.contains("▣"));
    assert!(committed.contains("Test Model"));
    assert!(committed.contains("4m17s"));
    assert!(!committed.contains("257s"));
}

#[test]
fn user_shell_command_renders_direct_output_and_shell_summary() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(100);

    widget.handle_worker_event(crate::events::WorkerEvent::CommandExecutionStarted {
        tool_use_id: "user-shell-1".to_string(),
        command: "ls".to_string(),
        input: None,
        source: devo_protocol::protocol::ExecCommandSource::UserShell,
        command_actions: vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: "ls".to_string(),
            path: None,
        }],
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolOutputDelta {
        tool_use_id: "user-shell-1".to_string(),
        delta: "Cargo.toml
crates
"
        .to_string(),
    });

    let live = rendered_rows(&widget, 100, 16).join(
        "
",
    );
    assert!(
        live.contains("Cargo.toml"),
        "expected live user shell output:
{live}"
    );
    assert!(
        !live.contains("Explored") && !live.contains("List ls"),
        "user shell list command must not render as an explored agent group:
{live}"
    );
    assert!(
        !live.contains("Ran ls") && !live.contains("You ran ls"),
        "user shell command should not use agent command wording:
{live}"
    );
    assert!(
        live.contains("▌ $ ls"),
        "user shell command should render the prompt-style header:
{live}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "user-shell-1".to_string(),
        title: "ls".to_string(),
        preview: "Cargo.toml
crates
"
        .to_string(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ShellCommandFinished {
        exit_code: Some(0),
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    assert!(
        history.contains("Cargo.toml"),
        "expected committed shell output:
{history}"
    );
    assert!(
        history.contains("▣ SHELL · Shell"),
        "shell command turn summary should use Shell mode label:
{history}"
    );
    assert!(
        !history.contains("▣ DeepSeek V4 Flash"),
        "shell command turn summary should not use model display name:
{history}"
    );
    assert!(
        !history.contains("Explored") && !history.contains("List ls"),
        "committed user shell command must not render as explored agent group:
{history}"
    );
    assert!(
        history.contains("▌ $ ls"),
        "committed user shell command should render the prompt-style header:
{history}"
    );
}

#[test]
fn two_shell_commands_render_as_separate_prompt_cells() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(100);

    widget.handle_worker_event(crate::events::WorkerEvent::CommandExecutionStarted {
        tool_use_id: "user-shell-1".to_string(),
        command: "pwd".to_string(),
        input: None,
        source: devo_protocol::protocol::ExecCommandSource::UserShell,
        command_actions: Vec::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolOutputDelta {
        tool_use_id: "user-shell-1".to_string(),
        delta: "/tmp/project\n".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "user-shell-1".to_string(),
        title: "Shell".to_string(),
        preview: "/tmp/project\n".to_string(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ShellCommandFinished {
        exit_code: Some(0),
    });

    widget.handle_worker_event(crate::events::WorkerEvent::CommandExecutionStarted {
        tool_use_id: "user-shell-2".to_string(),
        command: "whoami".to_string(),
        input: None,
        source: devo_protocol::protocol::ExecCommandSource::UserShell,
        command_actions: Vec::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolOutputDelta {
        tool_use_id: "user-shell-2".to_string(),
        delta: "tsiao\n".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "user-shell-2".to_string(),
        title: "Shell".to_string(),
        preview: "tsiao\n".to_string(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ShellCommandFinished {
        exit_code: Some(0),
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join(
        "
",
    );
    assert!(
        history.contains("▌ $ pwd") && history.contains("▌ $ whoami"),
        "expected two prompt-style shell command headers:
{history}"
    );
    assert!(
        history.contains("/tmp/project") && history.contains("tsiao"),
        "expected output from both shell commands:
{history}"
    );
    assert_eq!(
        history.matches("▌ $ ").count(),
        2,
        "shell commands should render as separate command cells:
{history}"
    );
    assert!(
        !history.contains("Explored") && !history.contains("List pwd"),
        "shell commands must not render as explored agent groups:
{history}"
    );
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

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta("hello".to_string()));

    let rendered = rendered_rows(&widget, 80, 12).join("\n");
    assert!(!rendered.contains("Devo -"));
}

#[test]
fn streaming_pending_ai_reply_respects_wrap_limit_before_finalize() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    widget.handle_app_event(AppEvent::ClearTranscript);
    let _ = widget.drain_scrollback_lines(80);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "see https://example.test/path/abcdef12345 tail words".to_string(),
    ));

    let rendered = rendered_rows(&widget, 24, 12).join("\n");
    assert!(
        rendered.contains("tail words"),
        "expected pending streaming reply to wrap suffix words together, got:\n{rendered}"
    );
}

#[test]
fn active_assistant_markdown_does_not_double_wrap() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let body = format!("{} betabet gamma", ["alpha"; 12].join(" "));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(body));

    let rendered = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        rendered.contains("betabet gamma"),
        "expected active assistant markdown to keep trailing words together, got:\n{rendered}"
    );
}

#[test]
fn active_assistant_multiline_text_has_no_extra_blank_rows() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "Line1\nLine2\nLine3\n".to_string(),
    ));

    let rows = rendered_rows(&widget, 80, 12);
    let line1 = find_row_index(&rows, "Line1").expect("missing Line1");
    let line2 = find_row_index(&rows, "Line2").expect("missing Line2");
    let line3 = find_row_index(&rows, "Line3").expect("missing Line3");
    assert_eq!(line2, line1 + 1, "unexpected rows:\n{}", rows.join("\n"));
    assert_eq!(line3, line2 + 1, "unexpected rows:\n{}", rows.join("\n"));
}

#[test]
fn active_assistant_renders_resume_like_markdown_without_fragment_gaps() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "## devo-cli -- Binary entry point that assembles all crates\n\n".to_string(),
    ));
    widget.pre_draw_tick();
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "4 source files, produces the devo binary.\n\n".to_string(),
    ));
    widget.pre_draw_tick();
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "Command dispatch (/crates/cli/src/main.rs)\n\n".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "devo                 -> run_agent()            interactive TUI (default)\n".to_string(),
    ));

    let rows = rendered_rows(&widget, 180, 24);
    let indices = indices_containing(
        &rows,
        &[
            "devo-cli",
            "4 source files",
            "Command dispatch",
            "run_agent",
        ],
    );

    assert_eq!(
        indices
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>(),
        vec![2, 2, 2],
        "expected active assistant markdown blocks to have one separator row, not doubled gaps:\n{}",
        rows.join("\n")
    );
}

#[test]
fn committed_assistant_markdown_does_not_double_wrap() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let body = format!("{} betabet gamma", ["alpha"; 12].join(" "));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(body));
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed = widget
        .drain_scrollback_lines(80)
        .into_iter()
        .map(|line| {
            line.line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        committed.contains("betabet gamma"),
        "expected committed assistant markdown to keep trailing words together, got:\n{committed}"
    );
}

#[test]
fn committed_assistant_multiline_text_has_no_extra_blank_rows() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "Line1\nLine2\nLine3\n".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let lines = scrollback_plain_lines(&trim_trailing_blank_scrollback_lines(
        widget.drain_scrollback_lines(80),
    ));
    let line1 = lines
        .iter()
        .position(|line| line.contains("Line1"))
        .unwrap();
    let line2 = lines
        .iter()
        .position(|line| line.contains("Line2"))
        .unwrap();
    let line3 = lines
        .iter()
        .position(|line| line.contains("Line3"))
        .unwrap();
    assert_eq!(line2, line1 + 1, "unexpected lines:\n{}", lines.join("\n"));
    assert_eq!(line3, line2 + 1, "unexpected lines:\n{}", lines.join("\n"));
}

#[test]
fn tool_call_start_and_finish_are_both_visible_in_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(80);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "powershell -NoProfile -Command Get-Date".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    let running = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        running.contains("Running powershell -NoProfile -Command Get-Date"),
        "expected running tool cell, got:\n{running}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "powershell -NoProfile -Command Get-Date".to_string(),
        preview: "2026-05-09".to_string(),
        is_error: false,
        truncated: false,
    });

    let ran = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        !ran.contains("Running powershell -NoProfile -Command Get-Date"),
        "running tool cell should not remain in history, got:\n{ran}"
    );
    assert!(
        ran.contains("Ran powershell -NoProfile -Command Get-Date"),
        "expected ran tool cell, got:\n{ran}"
    );
    assert!(
        ran.contains("2026-05-09"),
        "expected tool output, got:\n{ran}"
    );
}

#[test]
fn web_search_tool_call_renders_title_and_status_without_running_prefix() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(80);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "Web Search(\"latest OpenAI API docs\")".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    let running = rendered_rows(&widget, 80, 12).join(
        "
",
    );
    assert!(
        running.contains("Web Search(\"latest OpenAI API docs\")"),
        "expected web search title, got:
{running}"
    );
    assert!(
        !running.contains("Running Web Search"),
        "web search should not render a Running prefix, got:
{running}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "Web Search(\"latest OpenAI API docs\")".to_string(),
        preview: "status: completed".to_string(),
        is_error: false,
        truncated: false,
    });

    let rendered = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join(
        "
",
    );
    assert!(
        rendered.contains("Web Search(\"latest OpenAI API docs\")"),
        "expected completed web search title, got:
{rendered}"
    );
    assert!(
        rendered.contains("└ status: completed"),
        "expected completed status line, got:
{rendered}"
    );
    assert!(
        !rendered.contains("Ran Web Search") && !rendered.contains("Running Web Search"),
        "web search should not render Ran/Running prefix, got:
{rendered}"
    );
}

#[test]
fn web_fetch_tool_call_renders_title_and_status_without_running_prefix() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(80);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "Web Fetch(\"https://example.test/docs\")".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    let running = rendered_rows(&widget, 80, 12).join(
        "
",
    );
    assert!(
        running.contains("Web Fetch(\"https://example.test/docs\")"),
        "expected web fetch title, got:
{running}"
    );
    assert!(
        !running.contains("Running Web Fetch"),
        "web fetch should not render a Running prefix, got:
{running}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "Web Fetch(\"https://example.test/docs\")".to_string(),
        preview: "status: completed".to_string(),
        is_error: false,
        truncated: false,
    });

    let rendered = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join(
        "
",
    );
    assert!(
        rendered.contains("Web Fetch(\"https://example.test/docs\")"),
        "expected completed web fetch title, got:
{rendered}"
    );
    assert!(
        rendered.contains("└ status: completed"),
        "expected completed status line, got:
{rendered}"
    );
    assert!(
        !rendered.contains("Ran Web Fetch") && !rendered.contains("Running Web Fetch"),
        "web fetch should not render Ran/Running prefix, got:
{rendered}"
    );
}

#[test]
fn preparing_write_tool_call_is_visible_before_result() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "write src/lib.rs".to_string(),
        preparing: true,
        parsed_commands: None,
    });

    let display = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        display.contains("Preparing write..."),
        "expected preparing write row:\n{display}"
    );
}

#[test]
fn non_preparing_tool_call_keeps_existing_summary() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    let display = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        display.contains("Exploring") || display.contains("Search plan"),
        "expected normal tool summary:\n{display}"
    );
    assert!(
        !display.contains("Preparing grep"),
        "grep should not use preparing state:\n{display}"
    );
}

#[test]
fn generic_running_tool_call_disappears_after_result() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "code_search".to_string(),
        preparing: false,
        parsed_commands: Some(Vec::new()),
    });

    let running = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        running.contains("Running code_search"),
        "expected running generic tool row:\n{running}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "code_search".to_string(),
        preview: "Missing necessary parameter display".to_string(),
        is_error: true,
        truncated: false,
    });

    let rendered = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !rendered.contains("Running code_search"),
        "running row should disappear after result:\n{rendered}"
    );

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        !history.contains("Running code_search"),
        "running row should not be committed to history:\n{history}"
    );
    assert!(
        history.contains("Ran code_search"),
        "expected completed generic tool row in history:\n{history}"
    );
    assert!(
        history.contains("Missing necessary parameter display"),
        "expected final tool output in history:\n{history}"
    );
}

#[test]
fn edit_running_row_is_path_free_and_disappears_after_patch_result() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "edit-1".to_string(),
        summary: "Edit".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "edit-1".to_string(),
        tool_name: "edit".to_string(),
        input: serde_json::json!({"filePath": "test_edit_test.md"}),
    });

    let running = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        running.contains("Running Edit"),
        "expected live Edit row:\n{running}"
    );
    assert!(
        !running.contains("test_edit_test.md"),
        "live Edit row should not repeat the path:\n{running}"
    );

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("test_edit_test.md"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: Some("old\n".to_string()),
            new_text: Some("new\n".to_string()),
            move_path: None,
        },
    );
    widget.handle_worker_event(crate::events::WorkerEvent::PatchAppliedIo {
        tool_use_id: "edit-1".to_string(),
        tool_name: "edit".to_string(),
        input: serde_json::json!({"filePath": "test_edit_test.md"}),
        changes,
    });

    let after = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !after.contains("Running Edit"),
        "completed Edit should leave no live row:\n{after}"
    );
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        history.contains("Edited test_edit_test.md") || history.contains("Edited 1 file"),
        "completed Edit diff should remain visible:\n{history}"
    );
}

#[test]
fn patch_result_removes_only_matching_running_tool_row() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "edit-1".to_string(),
        summary: "Edit".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "search-1".to_string(),
        summary: "code_search".to_string(),
        preparing: false,
        parsed_commands: Some(Vec::new()),
    });

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "edit-1".to_string(),
        changes: std::collections::HashMap::new(),
    });

    let after = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !after.contains("Running Edit"),
        "Edit row should be removed:\n{after}"
    );
    assert!(
        after.contains("Running code_search"),
        "unrelated active tool row should remain:\n{after}"
    );
}

#[test]
fn interrupted_turn_flushes_explored_cell_before_summary() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(100);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "deepseek-v4-flash".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "code_search update_plan tool handler".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "code_search update_plan tool handler".to_string(),
            query: Some("update_plan tool handler".to_string()),
            path: Some("crates/core/src/tools/handlers".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "code_search update_plan tool handler".to_string(),
        preview: "crates/core/src/tools/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });

    let live_display = rendered_rows(&widget, 100, 12).join("\n");
    assert!(
        live_display.contains("▌ Explored"),
        "expected completed exploration to be live before turn finish:\n{live_display}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Interrupted".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let active_display = widget
        .active_cell_display_lines_for_test(100)
        .into_iter()
        .flat_map(|line| line.spans)
        .map(|span| span.content.to_string())
        .collect::<String>();
    assert!(
        !active_display.contains("Explored"),
        "explored cell should not remain live after turn finish:\n{active_display}"
    );

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    let explored_index = history
        .find("▌ Explored")
        .expect("history should contain explored cell");
    let interrupted_index = history
        .find("interrupted")
        .expect("history should contain interrupted summary");
    assert!(
        explored_index < interrupted_index,
        "explored cell should appear before interrupted summary:\n{history}"
    );
}

fn widget_with_live_explored_cell() -> ChatWidget {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek V4 Flash".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let _ = widget.drain_scrollback_lines(100);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "deepseek-v4-flash".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "code_search update_plan tool handler".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "code_search update_plan tool handler".to_string(),
            query: Some("update_plan tool handler".to_string()),
            path: Some("crates/core/src/tools/handlers".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "code_search update_plan tool handler".to_string(),
        preview: "crates/core/src/tools/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });

    let live_display = rendered_rows(&widget, 100, 12).join("\n");
    assert!(
        live_display.contains("▌ Explored"),
        "expected completed exploration to be live before turn finish:\n{live_display}"
    );
    widget
}

#[test]
fn paired_failed_turn_events_finalize_ui_once_in_order() {
    let mut widget = widget_with_live_explored_cell();
    let provider_error = "provider rejected the request: quota exceeded";

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFailed {
        message: provider_error.to_string(),
        hint: None,
        turn_count: 0,
        total_input_tokens: 10,
        total_output_tokens: 2,
        total_tokens: 12,
        total_cache_read_tokens: 1,
        prompt_token_estimate: 10,
        last_query_input_tokens: 10,
    });
    assert_eq!(
        widget.status_message_for_test(),
        "Query failed; see error above"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Failed".to_string(),
        turn_count: 1,
        total_input_tokens: 20,
        total_output_tokens: 4,
        total_tokens: 24,
        total_cache_read_tokens: 2,
        last_query_total_tokens: 14,
        last_query_input_tokens: 11,
        prompt_token_estimate: 11,
    });

    assert_eq!(
        widget.status_message_for_test(),
        "Query failed; see error above"
    );
    let authoritative_summary = widget.status_summary_text();
    assert!(
        authoritative_summary.contains("↑20"),
        "summary should use TurnFinished input total: {authoritative_summary}"
    );
    assert!(
        authoritative_summary.contains("cached 2 10%"),
        "summary should use TurnFinished cache total: {authoritative_summary}"
    );
    assert!(
        authoritative_summary.contains("↓4"),
        "summary should use TurnFinished output total: {authoritative_summary}"
    );
    assert!(
        authoritative_summary.contains("14/200k"),
        "summary should use TurnFinished latest-query total: {authoritative_summary}"
    );
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    let explored_index = history
        .find("▌ Explored")
        .expect("history should contain explored cell");
    let error_index = history
        .find(provider_error)
        .expect("history should contain the exact provider error");
    let failed_index = history
        .find(" · failed")
        .expect("history should contain failed summary");
    assert!(
        explored_index < error_index && error_index < failed_index,
        "expected Explored < provider error < failed:\n{history}"
    );
    assert_eq!(history.matches(" · failed").count(), 1);
    assert!(!history.contains("interrupted"), "history:\n{history}");
}

#[test]
fn turn_failed_renders_recovery_hint() {
    let mut widget = widget_with_live_explored_cell();
    let provider_error = "model provider error: provider timeout: stream idle timeout";
    let recovery_hint = devo_provider::NETWORK_PROXY_HINT;

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFailed {
        message: provider_error.to_string(),
        hint: Some(recovery_hint.to_string()),
        turn_count: 0,
        total_input_tokens: 10,
        total_output_tokens: 2,
        total_tokens: 12,
        total_cache_read_tokens: 1,
        prompt_token_estimate: 10,
        last_query_input_tokens: 10,
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        history.contains(provider_error),
        "history should contain provider error:\n{history}"
    );
    assert!(
        history.contains(recovery_hint),
        "history should contain recovery hint:\n{history}"
    );
}

#[test]
fn legacy_failed_turn_finished_flushes_explored_before_footer() {
    let mut widget = widget_with_live_explored_cell();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Failed".to_string(),
        turn_count: 1,
        total_input_tokens: 10,
        total_output_tokens: 2,
        total_tokens: 12,
        total_cache_read_tokens: 1,
        last_query_total_tokens: 12,
        last_query_input_tokens: 10,
        prompt_token_estimate: 10,
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    let explored_index = history
        .find("▌ Explored")
        .expect("history should contain explored cell");
    let failed_index = history
        .find(" · failed")
        .expect("history should contain failed summary");
    assert!(
        explored_index < failed_index,
        "explored cell should appear before failed summary:\n{history}"
    );
    assert_eq!(history.matches(" · failed").count(), 1);
    assert!(
        !history
            .lines()
            .any(|line| line.trim_start().starts_with("■ ")),
        "standalone failed completion should not invent an error message:\n{history}"
    );
    assert!(!history.contains("interrupted"), "history:\n{history}");
}

#[test]
fn preparing_write_disappears_after_patch_applied() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "write src/lib.rs".to_string(),
        preparing: true,
        parsed_commands: None,
    });
    let before = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        before.contains("Preparing write..."),
        "expected preparing state before result:\n{before}"
    );

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("src/lib.rs"),
        devo_protocol::protocol::FileChange::Add {
            content: "pub fn demo() {}\n".to_string(),
        },
    );
    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let after = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !after.contains("Preparing write..."),
        "preparing state should disappear after patch applied:\n{after}"
    );
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        history.contains("Added src/lib.rs")
            || history.contains("Edited src/lib.rs")
            || history.contains("Added 1 file")
    );
}

#[test]
fn preparing_apply_patch_tool_call_is_visible_before_result() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "apply_patch".to_string(),
        preparing: true,
        parsed_commands: None,
    });

    let display = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        display.contains("Preparing apply_patch..."),
        "expected preparing apply_patch row:\n{display}"
    );
}

#[test]
fn preparing_apply_patch_disappears_after_patch_applied() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "apply_patch".to_string(),
        preparing: true,
        parsed_commands: None,
    });
    let before = rendered_rows(&widget, 80, 12).join("\n");
    assert!(
        before.contains("Preparing apply_patch..."),
        "expected preparing state before result:\n{before}"
    );

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("src/lib.rs"),
        devo_protocol::protocol::FileChange::Add {
            content: "pub fn demo() {}\n".to_string(),
        },
    );
    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let after = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !after.contains("Preparing apply_patch..."),
        "preparing state should disappear after patch applied:\n{after}"
    );
}

#[test]
fn preparing_tool_row_animates_with_pre_draw_tick() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "write src/lib.rs".to_string(),
        preparing: true,
        parsed_commands: None,
    });
    let before = rendered_rows(&widget, 80, 12).join("\n");
    std::thread::sleep(std::time::Duration::from_millis(80));
    widget.pre_draw_tick();
    let after = rendered_rows(&widget, 80, 12).join("\n");
    assert_ne!(
        before, after,
        "expected preparing row to animate across ticks"
    );
}

#[test]
fn reasoning_text_commits_to_history_when_turn_finishes() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "thinking text\n".to_string(),
    ));

    let empty_scrollback = widget.drain_scrollback_lines(80);
    assert!(!scrollback_contains_text(
        &empty_scrollback,
        "thinking text"
    ));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "stop".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let scrollback = widget.drain_scrollback_lines(80);
    let scrollback_text = scrollback_plain_lines(&scrollback).join("\n");
    assert!(scrollback_text.contains("Thought: thinking text"));
    assert!(!scrollback_text.contains("reasoning_effort_selection: thinking text"));
}

#[test]
fn restored_reasoning_text_is_visible_in_transcript() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![crate::events::TranscriptItem::new(
            crate::events::TranscriptItemKind::Reasoning,
            "",
            "thinking text",
        )],
        rich_history_items: Vec::new(),
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let scrollback = widget.drain_scrollback_lines(80);
    let scrollback_text = scrollback_plain_lines(&scrollback).join("\n");
    assert!(scrollback_text.contains("Thought: thinking text"));
    assert!(!scrollback_text.contains("reasoning_effort_selection: thinking text"));
}

#[test]
fn reasoning_and_assistant_stream_in_separate_cells() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "thinking".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "final answer line 1\nfinal answer line 2\n".to_string(),
    ));

    let before_rows = rendered_rows(&widget, 80, 16);
    let before = before_rows.join("\n");
    assert!(
        before.contains("thinking") && before.contains("final answer line 1"),
        "reasoning/text should both be visible while streaming:\n{before}"
    );
    assert!(
        before.contains("Thinking: thinking"),
        "live reasoning should keep Thinking label while streaming:\n{before}"
    );
    let reasoning_row = find_row_index(&before_rows, "thinking").expect("missing reasoning row");
    let assistant_row =
        find_row_index(&before_rows, "final answer line 1").expect("missing assistant row");
    assert_eq!(
        assistant_row,
        reasoning_row + 2,
        "expected one blank row between live cells"
    );
    assert!(
        before_rows[reasoning_row + 1].trim().is_empty(),
        "expected blank separator row, got: {:?}",
        before_rows[reasoning_row + 1]
    );

    widget.pre_draw_tick();
    let committed_before_reasoning_complete =
        trim_trailing_blank_scrollback_lines(widget.drain_scrollback_lines(80));
    assert!(
        !scrollback_contains_text(&committed_before_reasoning_complete, "final answer line 1"),
        "assistant output should stay live, not drain to scrollback while reasoning is pending"
    );
    let active_before_reasoning_complete = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        active_before_reasoning_complete.contains("final answer line 1"),
        "assistant output should remain visible in the active viewport:\n{active_before_reasoning_complete}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningCompleted(
        "thinking".to_string(),
    ));

    // Reasoning is now committed to scrollback on ReasoningCompleted,
    // no longer visible in the live viewport.
    let after = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !after.contains("thinking"),
        "reasoning text should commit to scrollback, not remain in viewport:\n{after}"
    );

    let committed_after_reasoning_complete =
        trim_trailing_blank_scrollback_lines(widget.drain_scrollback_lines(80));
    let committed_after_text = committed_after_reasoning_complete
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(
        committed_after_text.contains("Thought: thinking"),
        "completed reasoning should use Thought label in scrollback: {committed_after_reasoning_complete:?}"
    );
    assert!(
        !committed_after_text.contains("reasoning_effort_selection: thinking"),
        "completed reasoning should not keep Thinking label in scrollback: {committed_after_reasoning_complete:?}"
    );
    let after_reasoning_rows = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        after_reasoning_rows.contains("final answer line 2"),
        "undrained assistant output should remain active after reasoning completes:\n{after_reasoning_rows}"
    );
}

#[test]
fn lifecycle_text_items_render_as_ordered_sibling_cells() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let reasoning_id = ItemId::new();
    let assistant_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        delta: "thinking".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        delta: "Line1\nLine2\n".to_string(),
    });

    let rows = rendered_rows(&widget, 80, 16);
    let reasoning_row = find_row_index(&rows, "thinking").expect("missing reasoning row");
    let line1 = find_row_index(&rows, "Line1").expect("missing assistant row");
    let line2 = find_row_index(&rows, "Line2").expect("missing second assistant row");
    assert_eq!(
        line1,
        reasoning_row + 2,
        "unexpected rows:\n{}",
        rows.join("\n")
    );
    assert_eq!(line2, line1 + 1, "unexpected rows:\n{}", rows.join("\n"));

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        final_text: "thinking".to_string(),
    });
    let rows_after_reasoning = rendered_rows(&widget, 80, 16);
    assert!(
        !rows_after_reasoning
            .iter()
            .any(|row| row.contains("thinking")),
        "completed reasoning should leave active viewport:\n{}",
        rows_after_reasoning.join("\n")
    );
    assert!(
        rows_after_reasoning.iter().any(|row| row.contains("Line1")),
        "assistant should remain active:\n{}",
        rows_after_reasoning.join("\n")
    );
    let committed_after_reasoning = widget.drain_scrollback_lines(80);
    let committed_after_reasoning_text =
        scrollback_plain_lines(&committed_after_reasoning).join("\n");
    assert!(
        committed_after_reasoning_text.contains("Thought: thinking"),
        "completed reasoning should use Thought label in scrollback: {committed_after_reasoning:?}"
    );
    assert!(
        !committed_after_reasoning_text.contains("reasoning_effort_selection: thinking"),
        "completed reasoning should not keep Thinking label in scrollback: {committed_after_reasoning:?}"
    );
}

#[test]
fn lifecycle_text_items_keep_reasoning_before_assistant_when_events_arrive_out_of_order() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let reasoning_id = ItemId::new();
    let assistant_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        delta: "answer line\n".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        delta: "thinking text".to_string(),
    });

    let rows = rendered_rows(&widget, 80, 16);
    let reasoning_row = find_row_index(&rows, "thinking text").expect("missing reasoning row");
    let assistant_row = find_row_index(&rows, "answer line").expect("missing assistant row");
    assert!(
        reasoning_row < assistant_row,
        "reasoning should render above assistant:\n{}",
        rows.join("\n")
    );

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        final_text: "answer line".to_string(),
    });
    let committed_before_reasoning = widget.drain_scrollback_lines(80);
    assert!(
        !scrollback_contains_text(&committed_before_reasoning, "answer line"),
        "assistant should wait for prior reasoning before committing: {committed_before_reasoning:?}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        final_text: "thinking text".to_string(),
    });
    let committed = scrollback_plain_lines(&trim_trailing_blank_scrollback_lines(
        widget.drain_scrollback_lines(80),
    ))
    .join("\n");
    let reasoning_index = committed
        .find("thinking text")
        .expect("missing committed reasoning");
    let assistant_index = committed
        .find("answer line")
        .expect("missing committed assistant");
    assert!(
        reasoning_index < assistant_index,
        "reasoning should commit before assistant:\n{committed}"
    );
}

#[test]
fn completed_assistant_flushes_before_next_reasoning_starts() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let stale_reasoning_id = ItemId::new();
    let assistant_id = ItemId::new();
    let next_reasoning_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: stale_reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: stale_reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        delta: "first thought".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        delta: "first answer".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemCompleted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        final_text: "first answer".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: next_reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: next_reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        delta: "second thought".to_string(),
    });

    let committed = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    let first_thought_index = committed
        .find("first thought")
        .expect("stale reasoning should be reconciled");
    let first_answer_index = committed
        .find("first answer")
        .expect("completed assistant should flush");
    assert!(
        first_thought_index < first_answer_index,
        "reconciled reasoning should remain before its assistant:\n{committed}"
    );
    assert_eq!(committed.matches("first thought").count(), 1, "{committed}");
    assert_eq!(committed.matches("first answer").count(), 1, "{committed}");

    let active = line_texts(widget.active_viewport_lines_for_test(100)).join("\n");
    assert!(active.contains("Thinking: second thought"), "{active}");
    assert!(!active.contains("first thought"), "{active}");
    assert!(!active.contains("first answer"), "{active}");
}

#[test]
fn assistant_stream_commit_tick_runs_while_reasoning_is_pending() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let reasoning_id = ItemId::new();
    let assistant_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: reasoning_id,
        kind: crate::events::TextItemKind::Reasoning,
        delta: "thinking text".to_string(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
        delta: "first line\nsecond line\n".to_string(),
    });

    widget.pre_draw_tick();
    let committed = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    let active = rendered_rows(&widget, 80, 16).join("\n");
    assert!(
        !committed.contains("first line"),
        "assistant stream should stay out of scrollback until completion:\n{committed}"
    );
    assert!(
        active.contains("first line"),
        "assistant stream should remain visible even with pending reasoning:\n{active}"
    );
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
        reasoning_capability: ReasoningCapability::Levels(vec![
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model, alt_model],
        saved_models: vec![
            saved_model_entry("test-model"),
            saved_model_entry("second-model"),
        ],
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
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

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 3,
        total_output_tokens: 5,
        total_tokens: 8,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 8,
        last_query_input_tokens: 3,
        prompt_token_estimate: 3,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
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
fn status_summary_uses_last_turn_total_when_idle_and_live_estimate_while_busy() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: Some("Resumed".to_string()),
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 12,
        total_output_tokens: 18,
        total_tokens: 30,
        total_cache_read_tokens: 4,
        last_query_total_tokens: 42,
        last_query_input_tokens: 42,
        prompt_token_estimate: 12,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let idle_summary = widget.status_summary_text();
    assert!(idle_summary.contains("↑12"));
    assert!(idle_summary.contains("cached 4 33%"));
    assert!(idle_summary.contains("↓18"));
    assert!(idle_summary.contains("42/200k"));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::UsageUpdated {
        total_input_tokens: 7,
        total_output_tokens: 2,
        total_tokens: 9,
        total_cache_read_tokens: 6,
        last_query_total_tokens: 9,
        last_query_input_tokens: 7,
    });

    let busy_summary = widget.status_summary_text();
    assert!(busy_summary.contains("↑7"));
    assert!(busy_summary.contains("cached 6 86%"));
    assert!(busy_summary.contains("9/200k"));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "stop".to_string(),
        turn_count: 2,
        total_input_tokens: 19,
        total_output_tokens: 20,
        total_tokens: 39,
        total_cache_read_tokens: 6,
        last_query_total_tokens: 9,
        last_query_input_tokens: 7,
        prompt_token_estimate: 7,
    });

    let finished_summary = widget.status_summary_text();
    assert!(finished_summary.contains("↑19"));
    assert!(finished_summary.contains("cached 6 32%"));
    assert!(finished_summary.contains("9/200k"));
}

#[test]
fn session_compacted_updates_context_bar_to_compacted_prompt_estimate() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: Some("Resumed".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 10_000,
        total_output_tokens: 1_000,
        total_tokens: 11_000,
        total_cache_read_tokens: 500,
        last_query_total_tokens: 9_000,
        last_query_input_tokens: 8_500,
        prompt_token_estimate: 8_500,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    widget.handle_worker_event(crate::events::WorkerEvent::SessionCompacted {
        total_input_tokens: 10_000,
        total_output_tokens: 1_000,
        total_tokens: 11_000,
        last_query_total_tokens: 1_200,
        last_query_input_tokens: 1_200,
        prompt_token_estimate: 1_200,
    });

    let summary = widget.status_summary_text();
    assert!(summary.contains("↑10k"));
    assert!(summary.contains("1k/200k"));
    assert!(!summary.contains("9k/200k"));
}

#[test]
fn usage_updated_keeps_context_bar_on_last_query_not_cumulative_totals() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: Some("Resumed".to_string()),
        model: Some("test-model".to_string()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        // Cumulative totals are intentionally larger than latest-query usage.
        total_input_tokens: 500,
        total_output_tokens: 100,
        total_tokens: 600,
        total_cache_read_tokens: 50,
        last_query_total_tokens: 42,
        last_query_input_tokens: 30,
        prompt_token_estimate: 30,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let idle_summary = widget.status_summary_text();
    assert!(idle_summary.contains("↑500"));
    assert!(idle_summary.contains("42/200k"));
    assert!(!idle_summary.contains("500/200k"));

    widget.handle_worker_event(crate::events::WorkerEvent::UsageUpdated {
        total_input_tokens: 550,
        total_output_tokens: 110,
        total_tokens: 660,
        total_cache_read_tokens: 60,
        last_query_total_tokens: 48,
        last_query_input_tokens: 35,
    });

    let busy_summary = widget.status_summary_text();
    assert!(busy_summary.contains("↑550"));
    assert!(busy_summary.contains("48/200k"));
    assert!(!busy_summary.contains("550/200k"));
}

#[test]
fn streaming_controller_is_initialized_and_commit_ticks_drain_lines() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    assert!(!widget.has_stream_controller());

    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "first line\nsecond line\n".to_string(),
    ));
    assert!(widget.has_stream_controller());

    widget.pre_draw_tick();
    let first_pass = rendered_rows(&widget, 80, 12).join("\n");
    assert!(first_pass.contains("first line"));
    assert!(first_pass.contains("second line"));
    let first_scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(!first_scrollback.contains("first line"));

    widget.pre_draw_tick();
    let second_pass = rendered_rows(&widget, 80, 12).join("\n");
    assert!(second_pass.contains("second line"));
}

#[test]
fn fragmented_random_assistant_stream_keeps_rendering_without_queue_stall() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);
    let assistant_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: assistant_id,
        kind: crate::events::TextItemKind::Assistant,
    });

    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut expected_lines = Vec::new();
    for index in 0..64 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let line = format!("line-{index:02}-{:016x}", seed);
        let streamed_line = format!("{line}\n");
        let split_at = 1 + (seed as usize % (streamed_line.len() - 1));
        expected_lines.push(line);

        for delta in [&streamed_line[..split_at], &streamed_line[split_at..]] {
            widget.handle_worker_event(crate::events::WorkerEvent::TextItemDelta {
                item_id: assistant_id,
                kind: crate::events::TextItemKind::Assistant,
                delta: delta.to_string(),
            });
            widget.pre_draw_tick();
        }

        for _ in 0..8 {
            if widget.assistant_stream_queued_lines_for_test() == 0 {
                break;
            }
            widget.pre_draw_tick();
        }
        assert_eq!(
            widget.assistant_stream_queued_lines_for_test(),
            0,
            "assistant stream queue should drain after complete random line {index}"
        );

        let rows = rendered_rows(&widget, 120, 90).join("\n");
        let latest_line = expected_lines.last().expect("line was generated");
        assert!(
            rows.contains(latest_line),
            "latest streamed line should be visible before turn completion:\n{rows}"
        );
    }

    let live_rows = rendered_rows(&widget, 120, 90).join("\n");
    for expected_line in expected_lines.iter().rev().take(12) {
        assert!(
            live_rows.contains(expected_line),
            "recent streamed line should remain visible before turn completion: {expected_line}"
        );
    }

    let committed_before_finish =
        scrollback_plain_lines(&widget.drain_scrollback_lines(120)).join("\n");
    let final_line = expected_lines.last().expect("line was generated");
    assert!(
        !committed_before_finish.contains(final_line),
        "assistant stream should still be live, not prematurely committed"
    );
}

fn monitor_agent(
    session_id: SessionId,
    parent_session_id: SessionId,
    nickname: &str,
) -> crate::events::SubagentMonitorAgent {
    crate::events::SubagentMonitorAgent {
        session_id,
        parent_session_id,
        agent_path: format!("root/{nickname}"),
        nickname: nickname.to_string(),
        role: "default".to_string(),
        status: "running".to_string(),
        last_task_message: Some(format!("run {nickname}")),
    }
}

fn request_user_input_question() -> RequestUserInputQuestion {
    RequestUserInputQuestion {
        id: "scope".to_string(),
        header: "Scope".to_string(),
        question: "Which scope should research use?".to_string(),
        is_other: false,
        is_secret: false,
        options: Some(vec![
            RequestUserInputOption {
                label: "Narrow".to_string(),
                description: "Inspect only the current behavior.".to_string(),
            },
            RequestUserInputOption {
                label: "Broad".to_string(),
                description: "Inspect related behavior too.".to_string(),
            },
        ]),
    }
}

#[test]
fn subagent_discovery_shows_inline_live_list_without_focusing_it() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "reviewer"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: "checking files".to_string(),
        },
    });

    assert!(!widget.is_subagent_monitor_open_for_test());
    assert_eq!(widget.selected_subagent_for_test(), Some(child));
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("ctrl + x agents"), "rows:\n{rows}");
    assert!(rows.contains("reviewer: working"), "rows:\n{rows}");
    assert!(rows.contains("checking files"), "rows:\n{rows}");
    let rendered = rendered_rows(&widget, 160, 18);
    let live_prefix = " ".repeat(usize::from(LIVE_PREFIX_COLS));
    assert!(
        rendered
            .iter()
            .any(|row| row.starts_with(&format!("{live_prefix}● reviewer: working"))),
        "live-list title should use the shared live prefix only:\n{}",
        rendered.join("\n")
    );
    assert!(
        rendered
            .iter()
            .any(|row| row.starts_with(&format!("{live_prefix}> checking files"))),
        "live-list preview should use the shared live prefix only:\n{}",
        rendered.join("\n")
    );
    let parent_transcript = line_texts(widget.transcript_overlay_lines(80)).join("\n");
    assert!(!parent_transcript.contains("checking files"));
}

#[test]
fn request_user_input_keeps_working_status_indicator_visible() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::RequestUserInput {
        session_id: SessionId::new(),
        turn_id,
        request_id: "request-1".to_string(),
        questions: vec![request_user_input_question()],
    });

    let rows = rendered_rows(&widget, 120, 24);
    let live_prefix = " ".repeat(usize::from(LIVE_PREFIX_COLS));
    assert!(
        rows.iter()
            .any(|row| row.starts_with(&format!("{live_prefix}Input requested"))),
        "request_user_input header should align with live content prefix:\n{}",
        rows.join("\n")
    );
    assert!(
        rows.iter()
            .any(|row| row.starts_with(&format!("{live_prefix}Which scope should research use?"))),
        "request_user_input question should align with live content prefix:\n{}",
        rows.join("\n")
    );
    let rows = rows.join("\n");
    assert!(rows.contains("Working"), "rows:\n{rows}");
    assert!(
        rows.contains("Which scope should research use?"),
        "rows:\n{rows}"
    );
}

#[test]
fn interrupt_request_switches_working_status_to_stopping_immediately() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });

    widget.handle_key_event(press_key(KeyCode::Esc));
    assert!(app_event_rx.try_recv().is_err());
    widget.handle_key_event(press_key(KeyCode::Esc));
    assert_eq!(app_event_rx.try_recv(), Ok(AppEvent::Interrupt));
    assert!(widget.request_interrupt());
    let rows = rendered_rows(&widget, 120, 20).join("\n");
    assert!(rows.contains("Stopping…"), "rows:\n{rows}");
    assert!(!rows.contains("to interrupt"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Interrupted".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });
    let rows = rendered_rows(&widget, 120, 20).join("\n");
    assert!(!rows.contains("Stopping…"), "rows:\n{rows}");
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(120)).join("\n");
    assert!(history.contains("interrupted"), "history:\n{history}");
}

#[test]
fn interrupt_failure_restores_working_status() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });
    assert!(widget.request_interrupt());

    widget.handle_worker_event(crate::events::WorkerEvent::InterruptFailed {
        message: "connection reset".to_string(),
    });

    let rows = rendered_rows(&widget, 120, 20).join("\n");
    assert!(rows.contains("Working"), "rows:\n{rows}");
    assert!(!rows.contains("Stopping…"), "rows:\n{rows}");
}

#[test]
fn session_compaction_live_rows_use_live_prefix_cols() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let live_prefix = " ".repeat(usize::from(LIVE_PREFIX_COLS));

    widget.handle_worker_event(crate::events::WorkerEvent::SessionCompactionStarted);

    let started_history = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(
        started_history
            .iter()
            .any(|line| line.starts_with(&format!("{live_prefix}Compaction started"))),
        "context compaction start should be visible in history:\n{}",
        started_history.join("\n")
    );

    let rows = rendered_rows(&widget, 120, 24);
    assert!(
        rows.iter()
            .any(|row| { row.starts_with(&live_prefix) && row.contains("Compacting session") }),
        "compaction in-progress row should align with live prefix:\n{}",
        rows.join("\n")
    );

    widget.handle_worker_event(crate::events::WorkerEvent::SessionCompacted {
        total_input_tokens: 10,
        total_output_tokens: 5,
        total_tokens: 15,
        last_query_total_tokens: 8,
        last_query_input_tokens: 8,
        prompt_token_estimate: 8,
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(
        history
            .iter()
            .any(|line| { line.starts_with(&format!("{live_prefix}Context compacted")) }),
        "compaction completion history should align with live prefix:\n{}",
        history.join("\n")
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ContextCompactionCompleted {
        title: "Context compacted for turn".to_string(),
    });
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(
        history
            .iter()
            .all(|line| !line.contains("Context compacted")),
        "paired session/item completion should not duplicate history:\n{}",
        history.join("\n")
    );

    widget.handle_worker_event(crate::events::WorkerEvent::SessionCompactionFailed {
        message: "compaction timed out".to_string(),
    });
    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(80));
    assert!(
        history
            .iter()
            .any(|line| { line.starts_with(&format!("{live_prefix}■ compaction timed out")) }),
        "compaction failure history should align with live prefix:\n{}",
        history.join("\n")
    );
}

#[test]
fn context_compaction_item_lifecycle_emits_worker_events() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let item_id = ItemId::new();
    let context = devo_server::EventContext {
        session_id,
        turn_id: Some(turn_id),
        item_id: Some(item_id),
        seq: 1,
        item_seq: None,
    };
    let item = devo_server::ItemEnvelope {
        item_id,
        item_kind: devo_server::ItemKind::ContextCompaction,
        payload: serde_json::json!({"title": "Context Compaction"}),
    };
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    crate::worker::handle_started_item(
        devo_server::ItemEventPayload {
            context: context.clone(),
            item: item.clone(),
        },
        &event_tx,
    );
    crate::worker::handle_completed_item(
        devo_server::ItemEventPayload { context, item },
        &event_tx,
    );

    assert_eq!(
        event_rx.try_recv().expect("compaction start event"),
        crate::events::WorkerEvent::SessionCompactionStarted
    );
    assert_eq!(
        event_rx.try_recv().expect("compaction completion event"),
        crate::events::WorkerEvent::ContextCompactionCompleted {
            title: "Context Compaction".to_string()
        }
    );
}

#[test]
fn failed_context_compaction_item_emits_failure_event() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    crate::worker::handle_completed_item(
        devo_server::ItemEventPayload {
            context: devo_server::EventContext {
                session_id: SessionId::new(),
                turn_id: Some(TurnId::new()),
                item_id: None,
                seq: 1,
                item_seq: None,
            },
            item: devo_server::ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: devo_server::ItemKind::ContextCompaction,
                payload: serde_json::json!({
                    "title": "Compaction failed",
                    "message": "summary generation failed"
                }),
            },
        },
        &event_tx,
    );

    assert_eq!(
        event_rx.try_recv().expect("compaction failure event"),
        crate::events::WorkerEvent::SessionCompactionFailed {
            message: "summary generation failed".to_string()
        }
    );
}

#[test]
fn request_user_input_and_subagent_live_list_are_visible_together() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let turn_id = TurnId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::RequestUserInput {
        session_id: parent,
        turn_id,
        request_id: "request-1".to_string(),
        questions: vec![request_user_input_question()],
    });

    let rows = rendered_rows(&widget, 120, 28).join("\n");
    assert!(rows.contains("researcher: working"), "rows:\n{rows}");
    assert!(rows.contains("Working"), "rows:\n{rows}");
    assert!(
        rows.contains("Which scope should research use?"),
        "rows:\n{rows}"
    );
}

#[test]
fn ctrl_x_focuses_inline_live_list_and_ctrl_x_esc_or_q_exits() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let first = SessionId::new();
    let second = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(first, parent, "first"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(second, parent, "second"),
    });

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(widget.is_subagent_monitor_open_for_test());
    assert_eq!(widget.selected_subagent_for_test(), Some(second));
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(!rows.contains("Sub-agents"), "rows:\n{rows}");
    assert!(rows.contains("first: working"), "rows:\n{rows}");
    assert!(rows.contains("second: working"), "rows:\n{rows}");
    assert!(rows.contains("run first"), "rows:\n{rows}");
    assert!(!rows.contains("root/second"), "rows:\n{rows}");

    widget.handle_key_event(press_key(KeyCode::Up));
    assert_eq!(widget.selected_subagent_for_test(), Some(first));
    widget.handle_key_event(press_key(KeyCode::Down));
    assert_eq!(widget.selected_subagent_for_test(), Some(second));

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(!widget.is_subagent_monitor_open_for_test());

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(widget.is_subagent_monitor_open_for_test());
    widget.handle_key_event(press_key(KeyCode::Esc));
    assert!(!widget.is_subagent_monitor_open_for_test());

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(widget.is_subagent_monitor_open_for_test());
    widget.handle_key_event(press_key(KeyCode::Char('q')));
    assert!(!widget.is_subagent_monitor_open_for_test());
}

#[test]
fn subagent_live_list_scrolls_with_three_visible_rows() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let first = SessionId::new();
    let second = SessionId::new();
    let third = SessionId::new();
    let fourth = SessionId::new();

    for (session_id, nickname) in [
        (first, "first"),
        (second, "second"),
        (third, "third"),
        (fourth, "fourth"),
    ] {
        widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
            agent: monitor_agent(session_id, parent, nickname),
        });
    }

    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert_eq!(widget.selected_subagent_for_test(), Some(fourth));
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(!rows.contains("run first"), "rows:\n{rows}");
    assert!(rows.contains("run second"), "rows:\n{rows}");
    assert!(rows.contains("run third"), "rows:\n{rows}");
    assert!(rows.contains("run fourth"), "rows:\n{rows}");

    widget.handle_key_event(press_key(KeyCode::Up));
    widget.handle_key_event(press_key(KeyCode::Up));
    widget.handle_key_event(press_key(KeyCode::Up));
    assert_eq!(widget.selected_subagent_for_test(), Some(first));
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("run first"), "rows:\n{rows}");
    assert!(rows.contains("run second"), "rows:\n{rows}");
    assert!(rows.contains("run third"), "rows:\n{rows}");
    assert!(!rows.contains("run fourth"), "rows:\n{rows}");
}

#[test]
fn terminal_subagent_status_hides_ctrl_x_hint_when_no_live_children_remain() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });
    assert!(widget.has_live_subagents_for_test());
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("ctrl + x agents"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TurnFinished {
            session_id: child,
            status: "completed".to_string(),
        },
    });

    assert!(!widget.has_live_subagents_for_test());
    assert!(!widget.is_subagent_monitor_open_for_test());
    let rows = rendered_rows(&widget, 100, 18).join("\n");
    assert!(rows.contains("builder: done"), "rows:\n{rows}");
    widget.expire_subagent_inactivity_for_test();
    let rows = rendered_rows(&widget, 100, 18).join("\n");
    assert!(!rows.contains("builder: done"), "rows:\n{rows}");
    assert!(!rows.contains("ctrl + x agents"), "rows:\n{rows}");
}

#[test]
fn terminal_cancelled_subagent_disappears_from_live_list() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TurnFinished {
            session_id: child,
            status: "cancelled".to_string(),
        },
    });

    assert!(!widget.has_live_subagents_for_test());
    let rows = rendered_rows(&widget, 100, 18).join("\n");
    assert!(rows.contains("builder: cancelled"), "rows:\n{rows}");
    widget.expire_subagent_inactivity_for_test();
    let rows = rendered_rows(&widget, 100, 18).join("\n");
    assert!(!rows.contains("builder: cancelled"), "rows:\n{rows}");
    assert!(!rows.contains("ctrl + x agents"), "rows:\n{rows}");
}

#[test]
fn subagent_live_list_latest_preview_updates_without_parent_transcript_pollution() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: "reading design notes".to_string(),
        },
    });
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("reading design notes"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::ToolCall {
            session_id: child,
            tool_use_id: "tool-1".to_string(),
            summary: "rg query".to_string(),
        },
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::ToolOutputDelta {
            session_id: child,
            tool_use_id: "tool-1".to_string(),
            delta: "found matches".to_string(),
        },
    });
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("rg query: found matches"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::ToolResult {
            session_id: child,
            tool_use_id: "tool-1".to_string(),
            title: "rg query".to_string(),
            preview: "matches summarized".to_string(),
            is_error: false,
        },
    });
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("matches summarized"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::PlanUpdated {
            session_id: child,
            explanation: Some("Checking candidate files".to_string()),
            steps: vec![PlanStep {
                text: "Inspect TUI state".to_string(),
                status: PlanStepStatus::InProgress,
            }],
        },
    });
    let rows = rendered_rows(&widget, 160, 18).join("\n");
    assert!(rows.contains("[~] Inspect TUI state"), "rows:\n{rows}");

    let parent_transcript = line_texts(widget.transcript_overlay_lines(80)).join("\n");
    assert!(!parent_transcript.contains("reading design notes"));
    assert!(!parent_transcript.contains("matches summarized"));
    assert!(!parent_transcript.contains("Checking candidate files"));
}

#[test]
fn subagent_live_list_preview_uses_latest_streaming_tail() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: "first line should disappear\nsecond line should remain".to_string(),
        },
    });
    let rows = rendered_rows(&widget, 180, 18).join("\n");
    assert!(
        !rows.contains("first line should disappear"),
        "rows:\n{rows}"
    );
    assert!(rows.contains("second line should remain"), "rows:\n{rows}");

    let long_tail = format!("{}LATEST", "x".repeat(150));
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: format!("\n{long_tail}"),
        },
    });
    let rows = rendered_rows(&widget, 220, 18).join("\n");
    assert!(rows.contains("LATEST"), "rows:\n{rows}");
    assert!(
        !rows.contains("second line should remain"),
        "rows should show the latest tail, not the prior line:\n{rows}"
    );
}

#[test]
fn subagent_live_list_preview_uses_rolling_tail_across_chunks() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    for delta in [
        "the opening sentence should not stay visible after enough chunks ",
        "middle chunk adds more text and should also age out ",
        "latest chunk tail stays visible",
    ] {
        widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
            event: crate::events::SubagentMonitorEvent::TextItemDelta {
                session_id: child,
                item_id: Some(item_id),
                kind: crate::events::TextItemKind::Assistant,
                delta: delta.to_string(),
            },
        });
    }

    let rows = rendered_rows(&widget, 220, 18).join("\n");
    assert!(
        rows.contains("latest chunk tail stays visible"),
        "rows:\n{rows}"
    );
    assert!(
        !rows.contains("the opening sentence should not stay visible"),
        "rows should keep the rolling tail, not the first chunk:\n{rows}"
    );
}

#[test]
fn subagent_live_list_preview_preserves_right_tail_when_narrow() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: "very long preview text whose beginning must disappear and RIGHT_TAIL"
                .to_string(),
        },
    });

    let rows = rendered_rows(&widget, 42, 18).join("\n");
    assert!(rows.contains("RIGHT_TAIL"), "rows:\n{rows}");
    assert!(
        !rows.contains("very long preview text"),
        "narrow preview should preserve the right tail, not the left prefix:\n{rows}"
    );
}

#[test]
fn subagent_live_list_preview_updates_from_completed_reasoning_tool_and_plan_tails() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "researcher"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemCompleted {
            session_id: child,
            item_id: Some(ItemId::new()),
            kind: crate::events::TextItemKind::Reasoning,
            final_text: "old thought\nnew thought tail".to_string(),
        },
    });
    let rows = rendered_rows(&widget, 180, 18).join("\n");
    assert!(rows.contains("new thought tail"), "rows:\n{rows}");
    assert!(!rows.contains("old thought"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::ToolOutputDelta {
            session_id: child,
            tool_use_id: "tool-1".to_string(),
            delta: "first output\nlatest output".to_string(),
        },
    });
    let rows = rendered_rows(&widget, 180, 18).join("\n");
    assert!(rows.contains("latest output"), "rows:\n{rows}");
    assert!(!rows.contains("first output"), "rows:\n{rows}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::PlanUpdated {
            session_id: child,
            explanation: Some("old plan note".to_string()),
            steps: vec![PlanStep {
                text: "latest plan step".to_string(),
                status: PlanStepStatus::InProgress,
            }],
        },
    });
    let rows = rendered_rows(&widget, 180, 18).join("\n");
    assert!(rows.contains("[~] latest plan step"), "rows:\n{rows}");
    assert!(!rows.contains("old plan note"), "rows:\n{rows}");
}

#[test]
fn subagent_transcript_overlay_includes_spawn_task_message() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });

    let cells = widget
        .subagent_transcript_overlay_cells(child, 80)
        .expect("overlay cells");
    assert!(
        cells
            .first()
            .and_then(|cell| cell.user_message.as_ref())
            .is_some_and(|message| { message.text.contains("run builder") }),
        "expected spawn task message at top of subagent overlay"
    );
}

#[test]
fn subagent_live_list_enter_emits_overlay_request_for_selected_child() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, mut app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });
    widget.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    widget.handle_key_event(press_key(KeyCode::Enter));

    assert!(!widget.is_subagent_monitor_open_for_test());
    assert_eq!(
        app_event_rx.try_recv().expect("overlay request event"),
        AppEvent::OpenSubagentOverlay { session_id: child }
    );
}

#[test]
fn subagent_transcript_overlay_live_tail_reflects_additional_child_text_delta() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: "partial".to_string(),
        },
    });

    let initial_tail = line_texts(
        widget
            .subagent_transcript_overlay_live_tail_lines(child, 80)
            .expect("child live tail"),
    )
    .join("\n");
    assert!(initial_tail.contains("partial"), "{initial_tail}");

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemDelta {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            delta: " update".to_string(),
        },
    });

    let updated_tail = line_texts(
        widget
            .subagent_transcript_overlay_live_tail_lines(child, 80)
            .expect("updated child live tail"),
    )
    .join("\n");
    assert!(updated_tail.contains("partial update"), "{updated_tail}");
}

#[test]
fn subagent_transcript_overlay_cells_render_assistant_and_tool_result() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let parent = SessionId::new();
    let child = SessionId::new();
    let item_id = ItemId::new();

    widget.handle_worker_event(crate::events::WorkerEvent::SubagentDiscovered {
        agent: monitor_agent(child, parent, "builder"),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::TextItemCompleted {
            session_id: child,
            item_id: Some(item_id),
            kind: crate::events::TextItemKind::Assistant,
            final_text: "assistant report".to_string(),
        },
    });
    widget.handle_worker_event(crate::events::WorkerEvent::SubagentMonitor {
        event: crate::events::SubagentMonitorEvent::ToolResult {
            session_id: child,
            tool_use_id: "tool-1".to_string(),
            title: "exec".to_string(),
            preview: "tool output".to_string(),
            is_error: false,
        },
    });

    let overlay_text = widget
        .subagent_transcript_overlay_cells(child, 80)
        .expect("child transcript cells")
        .into_iter()
        .flat_map(|cell| line_texts(cell.lines))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(overlay_text.contains("assistant report"), "{overlay_text}");
    assert!(overlay_text.contains("Ran exec"), "{overlay_text}");
    assert!(overlay_text.contains("tool output"), "{overlay_text}");
}

#[test]
fn session_switch_sets_active_agent_footer_label() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: Some("Agent Session".to_string()),
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: Some("Agent: cr".to_string()),
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let rows = rendered_rows(&widget, 160, 16);
    assert!(
        rows.iter().any(|row| row.contains("Agent: cr")),
        "expected active agent footer label in rows:\n{}",
        rows.join("\n")
    );
}

#[test]
fn new_session_prepared_appends_header_after_existing_history_and_resets_status() {
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

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 30,
        total_output_tokens: 5,
        total_tokens: 35,
        total_cache_read_tokens: 12,
        last_query_total_tokens: 25,
        last_query_input_tokens: 20,
        prompt_token_estimate: 20,
        history_items: Vec::new(),
        rich_history_items: Vec::new(),
        loaded_item_count: 0,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });
    widget.add_to_history(crate::history_cell::new_info_event(
        "old session line".to_string(),
        None,
    ));

    widget.handle_worker_event(crate::events::WorkerEvent::NewSessionPrepared {
        cwd: initial_cwd.clone(),
        model: "new-session-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        last_query_total_tokens: 25,
        last_query_input_tokens: 20,
        total_cache_read_tokens: 12,
    });

    assert_eq!(widget.current_cwd(), initial_cwd.as_path());
    assert_eq!(
        widget.current_model().map(|model| model.slug.as_str()),
        Some("new-session-model")
    );

    let summary = widget.status_summary_text();
    assert!(summary.contains("↑0"));
    assert!(summary.contains("cached 0 0%"));
    assert!(summary.contains("↓0"));
    assert!(summary.contains("0/200k"));

    let transcript_lines = scrollback_plain_lines(
        &widget
            .transcript_overlay_lines(80)
            .into_iter()
            .map(crate::history_cell::ScrollbackLine::new)
            .collect::<Vec<_>>(),
    );
    let transcript_text = transcript_lines.join("\n");
    assert!(transcript_text.contains("old session line"));
    let old_line_index = find_row_index(&transcript_lines, "old session line")
        .expect("old session line remains in transcript");
    let header_index =
        find_row_index(&transcript_lines, "Workspace").expect("new session header is appended");
    assert!(header_index > old_line_index);
}

#[test]
fn new_session_prepared_does_not_duplicate_startup_header_without_history() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::NewSessionPrepared {
        cwd,
        model: "new-session-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        last_query_total_tokens: 10,
        last_query_input_tokens: 10,
        total_cache_read_tokens: 4,
    });

    let rows = rendered_rows(&widget, 80, 16);
    assert_eq!(rows.iter().filter(|row| row.contains("Devo")).count(), 1);
    assert!(widget.status_summary_text().contains("cached 0 0%"));
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
        reasoning_capability: ReasoningCapability::Levels(vec![
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..Model::default()
    };
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model.clone())),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model, alt_model.clone()],
        saved_models: vec![
            saved_model_entry("test-model"),
            saved_model_entry("second-model"),
        ],
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });

    widget.handle_app_event(AppEvent::ModelSelected {
        model: "second-model".to_string(),
    });
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    widget.submit_text("hello".to_string());

    assert_eq!(widget.current_model(), Some(&alt_model));
    assert_eq!(
        app_event_rx
            .try_recv()
            .expect("context override command is emitted"),
        AppEvent::Command(AppCommand::OverrideTurnContext {
            cwd: None,
            model: Some("second-model".to_string()),
            reasoning_effort_selection: Some(Some("high".to_string())),
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

            model_binding_id: None,
            reasoning_effort_selection: Some("high".to_string()),
            sandbox: None,
            approval_policy: Some("on-request".to_string()),
            collaboration_mode: devo_protocol::CollaborationMode::Build,
        })
    );
}

#[test]
fn model_selection_with_reasoning_effort_support_waits_for_second_step() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let alt_model = Model {
        slug: "second-model".to_string(),
        display_name: "Second Model".to_string(),
        reasoning_capability: ReasoningCapability::Levels(vec![
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ]),
        default_reasoning_effort: Some(ReasoningEffort::High),
        ..Model::default()
    };
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![alt_model.clone()],
        saved_models: vec![saved_model_entry("second-model")],
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });

    widget.handle_app_event(AppEvent::ModelSelected {
        model: "second-model".to_string(),
    });

    assert_eq!(widget.current_model(), Some(&alt_model));
    assert!(app_event_rx.try_recv().is_err());

    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app_event_rx
            .try_recv()
            .expect("context override command is emitted"),
        AppEvent::Command(AppCommand::OverrideTurnContext {
            cwd: None,
            model: Some("second-model".to_string()),
            reasoning_effort_selection: Some(Some("high".to_string())),
            sandbox: None,
            approval_policy: None,
        })
    );
}

#[test]
fn model_selection_without_reasoning_effort_support_finishes_immediately() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let base_model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let alt_model = Model {
        slug: "plain-model".to_string(),
        display_name: "Plain Model".to_string(),
        reasoning_capability: ReasoningCapability::Unsupported,
        ..Model::default()
    };
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(cwd, Some(base_model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![alt_model.clone()],
        saved_models: vec![saved_model_entry("plain-model")],
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });

    widget.handle_app_event(AppEvent::ModelSelected {
        model: "plain-model".to_string(),
    });

    assert_eq!(widget.current_model(), Some(&alt_model));
    assert_eq!(
        app_event_rx
            .try_recv()
            .expect("context override command is emitted"),
        AppEvent::Command(AppCommand::OverrideTurnContext {
            cwd: None,
            model: Some("plain-model".to_string()),
            reasoning_effort_selection: Some(None),
            sandbox: None,
            approval_policy: None,
        })
    );
}

#[test]
fn flushed_assistant_lines_after_reasoning_are_in_one_cell() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    // Activate reasoning pause
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "thinking".to_string(),
    ));
    // Queue assistant lines while reasoning is active
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "line one\nline two\nline three\n".to_string(),
    ));
    // Complete reasoning; assistant stays active until its own item or turn completes.
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningCompleted(
        "thinking".to_string(),
    ));

    let committed = trim_trailing_blank_scrollback_lines(widget.drain_scrollback_lines(80));
    let committed_text = committed
        .iter()
        .flat_map(|l| l.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(committed_text.contains("thinking"));
    assert!(!committed_text.contains("line one"));

    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed = widget.drain_scrollback_lines(80);
    let non_blank: Vec<&crate::history_cell::ScrollbackLine> = committed
        .iter()
        .filter(|l| {
            !l.line
                .spans
                .iter()
                .all(|span| span.content.trim().is_empty())
        })
        .collect();
    let text = non_blank
        .iter()
        .flat_map(|l| l.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("line one"));
    assert!(text.contains("line two"));
    assert!(text.contains("line three"));
}

#[test]
fn completed_streaming_assistant_consolidates_to_source_backed_cell() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "## Architecture\n\nA. Input pipeline\n\n".to_string(),
    ));
    widget.pre_draw_tick();
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "TuiEvent".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "Completed".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let committed = widget.drain_scrollback_lines(80);
    let text = committed
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(
        text.matches("Architecture").count(),
        1,
        "completed assistant history should be consolidated without replay: {text}"
    );
    assert!(text.contains("TuiEvent"));
}

#[test]
fn reasoning_appears_exactly_once_after_full_turn() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let _ = widget.drain_scrollback_lines(80);
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "I am a unique thought".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TextDelta(
        "final answer\n".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningCompleted(
        "I am a unique thought".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::TurnFinished {
        stop_reason: "stop".to_string(),
        turn_count: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
    });

    let scrollback = widget.drain_scrollback_lines(80);
    let full_text = scrollback
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(
        full_text.matches("I am a unique thought").count(),
        1,
        "reasoning should appear exactly once in scrollback, got:\n{full_text}"
    );
}

#[test]
fn live_reasoning_cell_renders_without_duplication() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "step by step analysis".to_string(),
    ));

    let rows = rendered_rows(&widget, 80, 12);
    let before = rows.join("\n");
    // Reasoning text should be visible and appear exactly once.
    assert!(
        before.contains("step by step analysis"),
        "reasoning text should be visible:\n{before}"
    );
    let occurrences = before.matches("step by step analysis").count();
    assert_eq!(
        occurrences, 1,
        "reasoning should appear exactly once, got {occurrences}:\n{before}"
    );
}

#[test]
fn collapsed_reasoning_live_view_keeps_only_latest_lines() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: true,
    });

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "line one\n\nline two\n\nline three\n\nline four\n\nline five".to_string(),
    ));

    let live = widget
        .active_viewport_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        live.contains("line five"),
        "collapsed live view should keep the latest reasoning line:\n{live}"
    );
    assert!(
        !live.contains("line one"),
        "collapsed live view should drop older reasoning lines:\n{live}"
    );
    assert!(
        live.contains("ctrl + t to view transcript"),
        "collapsed live reasoning should hint Ctrl+T:\n{live}"
    );
}

#[test]
fn collapsed_short_reasoning_stays_full_after_completion() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: true,
    });

    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        "short thought".to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningCompleted(
        "short thought".to_string(),
    ));

    let scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        scrollback.contains("short thought"),
        "short collapsed reasoning should stay fully visible after completion:\n{scrollback}"
    );
    assert!(
        scrollback.contains("ctrl + t to view transcript"),
        "collapsed short reasoning should hint Ctrl+T:\n{scrollback}"
    );
}

#[test]
fn collapsed_long_reasoning_compacts_to_one_line_after_completion() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: Vec::new(),
        saved_models: Vec::new(),
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: true,
    });

    let long_reasoning = "line one\n\nline two\n\nline three\n\nline four\n\nline five";
    widget.handle_worker_event(crate::events::WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: Default::default(),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningDelta(
        long_reasoning.to_string(),
    ));
    widget.handle_worker_event(crate::events::WorkerEvent::ReasoningCompleted(
        long_reasoning.to_string(),
    ));

    let scrollback = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        scrollback.contains("Thought · line one"),
        "long collapsed reasoning should compact to a one-line Thought summary:\n{scrollback}"
    );
    assert!(
        scrollback.contains("ctrl + t to view transcript"),
        "collapsed reasoning should hint Ctrl+T near the Thought cell:\n{scrollback}"
    );
    assert!(
        !scrollback.contains("line five"),
        "long collapsed reasoning should not keep the full body in main scrollback:\n{scrollback}"
    );

    let transcript = widget
        .transcript_overlay_cells(80)
        .into_iter()
        .flat_map(|cell| cell.lines)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("line five"),
        "long collapsed reasoning should remain available in Ctrl+T transcript:\n{transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_full_completed_tool_output() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let output = (1..=8)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "bash".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "bash".to_string(),
        preview: output,
        is_error: false,
        truncated: false,
    });

    let inline = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    let transcript = widget
        .transcript_overlay_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        inline.contains("line 1") && inline.contains("line 2"),
        "inline output should include the head of the preview: {inline}"
    );
    assert!(
        inline.contains("ctrl + t to view transcript"),
        "inline output should include the transcript hint when truncated: {inline}"
    );
    assert!(
        inline.contains("line 7") && inline.contains("line 8"),
        "inline output should include the tail of the preview: {inline}"
    );
    assert!(
        !inline.contains("line 3") && !inline.contains("line 6"),
        "inline output should stay compact: {inline}"
    );
    assert!(
        transcript.contains("line 5") && transcript.contains("line 8"),
        "transcript output should include the full tool output: {transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_running_tool_output_delta() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "bash".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolOutputDelta {
        tool_use_id: "tool-1".to_string(),
        delta: "streamed output line".to_string(),
    });

    let transcript = widget
        .transcript_overlay_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        transcript.contains("streamed output line"),
        "transcript output should include running tool deltas: {transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_running_tool_input_and_output_delta() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "custom job".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "tool-1".to_string(),
        tool_name: "custom_tool".to_string(),
        input: serde_json::json!({"alpha": 1, "target": "crate"}),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolOutputDelta {
        tool_use_id: "tool-1".to_string(),
        delta: "streamed output line".to_string(),
    });

    let transcript = line_texts(widget.transcript_overlay_lines(80)).join("\n");

    assert!(
        transcript.contains("Input") && transcript.contains("\"alpha\": 1"),
        "transcript should include running tool input: {transcript}"
    );
    assert!(
        transcript.contains("Output") && transcript.contains("streamed output line"),
        "transcript should include running tool output deltas: {transcript}"
    );
}

#[test]
fn generic_tool_call_has_one_running_render_owner() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "custom job".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "tool-1".to_string(),
        tool_name: "custom_tool".to_string(),
        input: serde_json::json!({"target": "crate"}),
    });

    let active = line_texts(widget.active_viewport_lines_for_test(100)).join("\n");
    assert_eq!(
        active.matches("Running custom job").count(),
        1,
        "one tool_use_id should have one live render owner:\n{active}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "custom job".to_string(),
        preview: "done".to_string(),
        is_error: false,
        truncated: false,
    });
    let active = line_texts(widget.active_viewport_lines_for_test(100)).join("\n");
    assert!(!active.contains("Running custom job"), "{active}");
}

#[test]
fn duplicate_command_execution_start_is_idempotent() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let started = crate::events::WorkerEvent::CommandExecutionStarted {
        tool_use_id: "command-1".to_string(),
        command: "pwd".to_string(),
        input: None,
        source: devo_protocol::protocol::ExecCommandSource::Agent,
        command_actions: Vec::new(),
    };

    widget.handle_worker_event(started.clone());
    widget.handle_worker_event(started);

    let transcript = line_texts(widget.transcript_overlay_lines(100)).join("\n");
    assert_eq!(
        transcript.matches("pwd").count(),
        1,
        "duplicate starts should retain one command cell:\n{transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_completed_tool_input_and_full_output() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let output = (1..=8)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "custom job".to_string(),
        preparing: false,
        parsed_commands: None,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "tool-1".to_string(),
        tool_name: "custom_tool".to_string(),
        input: serde_json::json!({"query": "needle", "path": "crates/tui"}),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResultIo {
        tool_use_id: "tool-1".to_string(),
        tool_name: "custom_tool".to_string(),
        title: "custom job".to_string(),
        input: serde_json::json!({"query": "needle", "path": "crates/tui"}),
        output: serde_json::Value::String(output),
        display_content: None,
        is_error: false,
        truncated: false,
    });

    let inline = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    let transcript = line_texts(widget.transcript_overlay_lines(80)).join("\n");

    assert!(
        inline.contains("line 1") && inline.contains("ctrl + t to view transcript"),
        "inline output should stay compact and show the transcript hint: {inline}"
    );
    assert!(
        transcript.contains("Input") && transcript.contains("\"query\": \"needle\""),
        "transcript should include completed tool input: {transcript}"
    );
    assert!(
        transcript.contains("line 5") && transcript.contains("line 8"),
        "transcript should include the full completed tool output: {transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_completed_read_input_and_full_output() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "read src/lib.rs".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read src/lib.rs".to_string(),
            name: "lib.rs".to_string(),
            path: PathBuf::from("src/lib.rs"),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "tool-1".to_string(),
        tool_name: "read".to_string(),
        input: serde_json::json!({"path": "src/lib.rs", "offset": 4, "limit": 2}),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResultIo {
        tool_use_id: "tool-1".to_string(),
        tool_name: "read".to_string(),
        title: "read src/lib.rs".to_string(),
        input: serde_json::json!({"path": "src/lib.rs", "offset": 4, "limit": 2}),
        output: serde_json::Value::String("read output line 1\nread output line 2".to_string()),
        display_content: None,
        is_error: false,
        truncated: false,
    });

    let inline = line_texts(widget.active_viewport_lines_for_test(80)).join("\n");
    let transcript = line_texts(widget.transcript_overlay_lines(80)).join("\n");

    assert!(
        inline.contains("Explored") && inline.contains("Read src/lib.rs"),
        "inline read rendering should stay as the compact explored block: {inline}"
    );
    assert!(
        !inline.contains("Input") && !inline.contains("offset: 4"),
        "inline read rendering should not expose raw transcript input: {inline}"
    );
    assert!(
        transcript.contains("file: src/lib.rs")
            && transcript.contains("offset: 4")
            && transcript.contains("limit: 2"),
        "transcript should include read input details: {transcript}"
    );
    assert!(
        transcript.contains("read output line 1") && transcript.contains("read output line 2"),
        "transcript should include full read output: {transcript}"
    );
}

#[test]
fn transcript_overlay_lines_include_patch_input_and_diff_output() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));
    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchAppliedIo {
        tool_use_id: "tool-1".to_string(),
        tool_name: "apply_patch".to_string(),
        input: serde_json::json!({
            "patch": "*** Begin Patch\n*** Update File: foo.txt\n-old\n+new\n*** End Patch"
        }),
        changes,
    });

    let transcript = line_texts(widget.transcript_overlay_lines(100)).join("\n");

    assert!(
        transcript.contains("Input") && transcript.contains("patch:"),
        "transcript should include patch input: {transcript}"
    );
    assert!(
        transcript.contains("Output")
            && (transcript.contains("Edited foo.txt") || transcript.contains("Edited 1 file")),
        "transcript should include rendered diff output: {transcript}"
    );
}

#[test]
fn restored_session_transcript_overlay_preserves_paired_tool_io() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolCall,
                title: "read src/lib.rs".to_string(),
                body: String::new(),
                tool_io: Some(devo_protocol::SessionHistoryToolIo {
                    tool_name: "read".to_string(),
                    input: serde_json::json!({"path": "src/lib.rs", "offset": 10, "limit": 3}),
                    output: None,
                    display_content: None,
                }),
                metadata: None,
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolResult,
                title: "read output".to_string(),
                body: "legacy preview".to_string(),
                tool_io: Some(devo_protocol::SessionHistoryToolIo {
                    tool_name: "read".to_string(),
                    input: serde_json::Value::Null,
                    output: Some(serde_json::Value::String(
                        "restored line 1\nrestored line 2".to_string(),
                    )),
                    display_content: None,
                }),
                metadata: None,
                duration_ms: None,
            },
        ],
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let transcript = line_texts(widget.transcript_overlay_lines(100)).join("\n");

    assert!(
        transcript.contains("file: src/lib.rs")
            && transcript.contains("offset: 10")
            && transcript.contains("limit: 3"),
        "restored transcript should include paired input: {transcript}"
    );
    assert!(
        transcript.contains("restored line 1") && transcript.contains("restored line 2"),
        "restored transcript should include paired output: {transcript}"
    );
}

#[test]
fn legacy_restored_session_without_tool_io_keeps_existing_tool_result_rendering() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd.clone());

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd,
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolCall,
                title: "legacy tool".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            devo_protocol::SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: devo_protocol::SessionHistoryItemKind::ToolResult,
                title: "legacy tool output".to_string(),
                body: "legacy result".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
        ],
        loaded_item_count: 2,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let transcript = line_texts(widget.transcript_overlay_lines(100)).join("\n");

    assert!(
        transcript.contains("legacy result"),
        "legacy restored transcript should still show the old result body: {transcript}"
    );
    assert!(
        !transcript.contains("Input") && !transcript.contains("Output"),
        "legacy restored transcript should not synthesize tool I/O sections: {transcript}"
    );
}

#[test]
fn read_tool_call_renders_as_explored_group_in_viewport() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "cat foo.txt".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    let live_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        live_display.contains("Exploring"),
        "expected read start to render immediately: {live_display}"
    );
    assert!(
        live_display.contains("Read foo.txt"),
        "expected live read summary: {live_display}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "cat foo.txt".to_string(),
        preview: "hello".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("Explored") || display.contains("Exploring"),
        "expected explored viewport grouping: {display}"
    );
    assert!(
        display.contains("Read foo.txt"),
        "expected read summary in explored viewport: {display}"
    );
    assert!(display.contains("▌ Explored") || display.contains("▌ Exploring"));
}

#[test]
fn read_tool_call_renders_relative_path_with_line_range() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "read crates/core/src/query.rs".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read crates/core/src/query.rs".to_string(),
            name: "crates/core/src/query.rs L:10-19".to_string(),
            path: PathBuf::from("crates/core/src/query.rs"),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "read crates/core/src/query.rs".to_string(),
        preview: "impl Query {}".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(100)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("Read crates/core/src/query.rs L:10-19"),
        "expected read summary with line range: {display}"
    );
}

#[test]
fn read_tool_call_falls_back_to_path_when_read_name_is_empty() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "read crates/tui/src/mod.rs".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read crates/tui/src/mod.rs".to_string(),
            name: String::new(),
            path: PathBuf::from("crates/tui/src/mod.rs"),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "read crates/tui/src/mod.rs".to_string(),
        preview: "mod tui;".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("Read crates/tui/src/mod.rs"),
        "expected read summary fallback in explored viewport: {display}"
    );
    assert!(
        !display.contains("  └ Read\n"),
        "read summary should not be bare Read: {display}"
    );
}

#[test]
fn read_tool_call_updates_placeholder_from_completed_tool_call_metadata() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "read {}".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: String::new(),
            name: String::new(),
            path: PathBuf::new(),
        }]),
    });

    let initial_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        initial_display.contains("Explored") || initial_display.contains("Exploring"),
        "expected read start to render as explored cell: {initial_display}"
    );
    assert!(
        initial_display.contains("Read"),
        "expected placeholder read line: {initial_display}"
    );
    assert!(
        !initial_display.contains("Running read {}"),
        "read placeholder should not render as a generic running tool: {initial_display}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallUpdated {
        tool_use_id: "tool-1".to_string(),
        summary: "read crates/tui/src/mod.rs".to_string(),
        parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read crates/tui/src/mod.rs".to_string(),
            name: "mod.rs".to_string(),
            path: PathBuf::from("crates/tui/src/mod.rs"),
        }],
    });

    let updated_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        updated_display.contains("Read crates/tui/src/mod.rs"),
        "expected read placeholder to update in place: {updated_display}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "read crates/tui/src/mod.rs".to_string(),
        preview: "mod tui;".to_string(),
        is_error: false,
        truncated: false,
    });

    let completed_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        completed_display.contains("Read crates/tui/src/mod.rs"),
        "expected completed read to remain explored: {completed_display}"
    );
    assert!(
        !completed_display.contains("Ran read"),
        "matching result should not create generic ran cell: {completed_display}"
    );
}

#[test]
fn consecutive_read_tool_calls_render_each_on_its_own_line() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let paths = [
        "crates/tui/src/mod.rs",
        "crates/tui/src/lib.rs",
        "crates/tui/src/file1.rs",
        "crates/tui/src/file2.rs",
    ];
    for path in paths {
        let name = path.rsplit('/').next().expect("basename");
        let tool_use_id = format!("tool-{name}");
        widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
            tool_use_id: tool_use_id.clone(),
            summary: "read {}".to_string(),
            preparing: false,
            parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: String::new(),
                name: String::new(),
                path: PathBuf::new(),
            }]),
        });
        widget.handle_worker_event(crate::events::WorkerEvent::ToolCallUpdated {
            tool_use_id: tool_use_id.clone(),
            summary: format!("read {path}"),
            parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: format!("read {path}"),
                name: name.to_string(),
                path: PathBuf::from(path),
            }],
        });
        widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
            tool_use_id,
            title: format!("read {path}"),
            preview: String::new(),
            is_error: false,
            truncated: false,
        });
    }

    let display = widget
        .active_cell_display_lines_for_test(120)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    for path in paths {
        assert!(
            display.contains(&format!("Read {path}")),
            "expected dedicated Read line for {path}: {display}"
        );
    }
    assert!(
        !display.contains("Read crates/tui/src/mod.rs crates/tui/src/lib.rs"),
        "reads must not be space-joined on one Read line: {display}"
    );
}

#[test]
fn glob_tool_call_renders_as_explored_group_in_viewport() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "glob **/Cargo.toml in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/Cargo.toml in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "glob **/Cargo.toml in crates".to_string(),
        preview: "crates/tools/Cargo.toml".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(display.contains("Explored") || display.contains("Exploring"));
    assert!(
        display.contains("List crates"),
        "expected list summary, got:\n{display}"
    );
}

#[test]
fn grep_tool_call_renders_as_explored_group_in_viewport() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'rebuild_restored_session' in crates/tui/src".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'rebuild_restored_session' in crates/tui/src".to_string(),
            query: Some("rebuild_restored_session".to_string()),
            path: Some("crates/tui/src".to_string()),
        }]),
    });

    let live_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        live_display.contains("Exploring"),
        "expected grep start to render immediately: {live_display}"
    );
    assert!(
        live_display.contains("Search rebuild_restored_session in crates/tui/src"),
        "expected live search summary, got:\n{live_display}"
    );

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "grep 'rebuild_restored_session' in crates/tui/src".to_string(),
        preview: "chatwidget.rs".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(display.contains("Explored") || display.contains("Exploring"));
    assert!(
        display.contains("Search rebuild_restored_session in crates/tui/src"),
        "expected search summary, got:\n{display}"
    );
}

#[test]
fn code_search_tool_call_renders_as_explored_group_in_viewport() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "code_search live tool feedback in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "code_search live tool feedback in crates".to_string(),
            query: Some("live tool feedback".to_string()),
            path: Some("crates".to_string()),
        }]),
    });

    let live_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        live_display.contains("Exploring"),
        "expected code_search start to render immediately: {live_display}"
    );
    assert!(
        live_display.contains("Search live tool feedback in crates"),
        "expected live code_search summary, got:\n{live_display}"
    );
    assert!(
        !live_display.contains("Running code_search {}"),
        "code_search should not render raw empty JSON: {live_display}"
    );
}

#[test]
fn exploring_code_search_with_details_shows_input_in_active_cell() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "code_search live tool feedback in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "code_search live tool feedback in crates".to_string(),
            query: Some("live tool feedback".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCallDetails {
        tool_use_id: "tool-1".to_string(),
        tool_name: "code_search".to_string(),
        input: serde_json::json!({
            "operation": "search",
            "query": "live tool feedback",
            "path": "crates"
        }),
    });

    let live_display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        live_display.contains("Exploring"),
        "expected Exploring header: {live_display}"
    );
    assert!(
        live_display.contains("operation") && live_display.contains("search"),
        "active ExecCell should show 'operation: search' while exploring:\n{live_display}"
    );
    assert!(
        live_display.contains("query") && live_display.contains("live tool feedback"),
        "active ExecCell should show 'query: live tool feedback' while exploring:\n{live_display}"
    );
}

#[test]
fn merged_explored_group_becomes_explored_after_all_results_arrive() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "grep 'plan' in crates".to_string(),
        preview: "crates/tools/src/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-2".to_string(),
        title: "glob **/plan.rs in crates".to_string(),
        preview: "crates/tools/src/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("▌ Explored"),
        "expected merged explored group to become completed, got:\n{display}"
    );
    assert!(
        !display.contains("▌ Exploring"),
        "merged explored group should not stay active after all completions:\n{display}"
    );
}

#[test]
fn live_viewport_shows_explored_group_while_active() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });

    let display = widget
        .active_viewport_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("▌ Exploring") || display.contains("▌ Explored"),
        "live viewport should show explored exec cell:\n{display}"
    );
    assert!(
        display.contains("Search plan in crates"),
        "live viewport should include search summary:\n{display}"
    );
    assert!(
        display.contains("List crates"),
        "live viewport should include list summary:\n{display}"
    );
}

#[test]
fn reasoning_start_closes_current_explored_group() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: devo_core::ItemId::new(),
        kind: crate::events::TextItemKind::Reasoning,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });

    let transcript = widget
        .transcript_overlay_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        transcript.matches("Explored").count() + transcript.matches("Exploring").count(),
        2,
        "reasoning boundary should split explored groups:\n{transcript}"
    );
}

#[test]
fn assistant_text_start_closes_current_explored_group() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::TextItemStarted {
        item_id: devo_core::ItemId::new(),
        kind: crate::events::TextItemKind::Assistant,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });

    let transcript = widget
        .transcript_overlay_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        transcript.matches("Explored").count() + transcript.matches("Exploring").count(),
        2,
        "assistant text boundary should split explored groups:\n{transcript}"
    );
}

#[test]
fn merged_explored_group_stays_completed_when_tool_results_arrive_after_tool_call_completion() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "grep 'plan' in crates".to_string(),
        preview: String::new(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-2".to_string(),
        title: "glob **/plan.rs in crates".to_string(),
        preview: String::new(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "grep output".to_string(),
        preview: "crates/tools/src/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-2".to_string(),
        title: "glob output".to_string(),
        preview: "crates/tools/src/handlers/plan.rs".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = widget
        .active_cell_display_lines_for_test(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        display.contains("▌ Explored"),
        "tool result follow-up events should not reactivate explored group:\n{display}"
    );
    assert!(
        !display.contains("▌ Exploring"),
        "tool result follow-up events should not leave explored group active:\n{display}"
    );
}

#[test]
fn explored_group_in_history_can_finish_late_completions() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-1".to_string(),
        summary: "grep 'plan' in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep 'plan' in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-2".to_string(),
        summary: "glob **/plan.rs in crates".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/plan.rs in crates".to_string(),
                path: Some("crates".to_string()),
            },
        ]),
    });
    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-1".to_string(),
        title: "grep 'plan' in crates".to_string(),
        preview: String::new(),
        is_error: false,
        truncated: false,
    });

    widget.handle_worker_event(crate::events::WorkerEvent::ToolCall {
        tool_use_id: "tool-3".to_string(),
        summary: "write src/main.rs".to_string(),
        preparing: false,
        parsed_commands: None,
    });

    widget.handle_worker_event(crate::events::WorkerEvent::ToolResult {
        tool_use_id: "tool-2".to_string(),
        title: "glob **/plan.rs in crates".to_string(),
        preview: String::new(),
        is_error: false,
        truncated: false,
    });

    let history_blob = widget
        .transcript_overlay_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        history_blob.contains("▌ Explored"),
        "late completion should finish explored cell already flushed to history:\n{history_blob}"
    );
    assert!(
        !history_blob.contains("▌ Exploring"),
        "flushed explored cell should not stay active after late completion:\n{history_blob}"
    );
}

#[test]
fn auto_git_diff_trigger_matches_editing_tools_only() {
    assert!(ChatWidget::should_auto_show_git_diff(
        "write src/main.rs",
        false
    ));
    assert!(ChatWidget::should_auto_show_git_diff("apply_patch", false));
    assert!(!ChatWidget::should_auto_show_git_diff("bash", false));
    assert!(!ChatWidget::should_auto_show_git_diff(
        "bash echo hi > file.txt",
        false
    ));
    assert!(!ChatWidget::should_auto_show_git_diff(
        "read src/main.rs",
        false
    ));
    assert!(!ChatWidget::should_auto_show_git_diff(
        "write src/main.rs",
        true
    ));
}

#[test]
fn patch_applied_event_renders_edited_block() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Edited foo.txt") || blob.contains("Edited 1 file"),
        "expected edited patch block, got:\n{blob}"
    );
    assert!(blob.contains("▌ Edited") || blob.contains("▌ Added"));
}

#[test]
fn added_file_patch_applied_event_renders_added_content_lines() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("quicksort.rs"),
        devo_protocol::protocol::FileChange::Add {
            content: "pub fn quicksort() {\n    println!(\"hi\");\n}\n".to_string(),
        },
    );
    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        blob.contains("Added quicksort.rs")
            || blob.contains("Edited quicksort.rs")
            || blob.contains("Added 1 file")
    );
    assert!(
        blob.contains("pub fn quicksort()"),
        "expected added file content to render:\n{blob}"
    );
    assert!(
        blob.contains("println!(\"hi\");"),
        "expected added file body to render:\n{blob}"
    );
}

#[test]
fn apply_patch_style_full_git_diff_reports_non_zero_counts() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("update.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "diff --git a/update.txt b/update.txt\n--- a/update.txt\n+++ b/update.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("(+1 -1)"),
        "full git-style apply_patch diff should report non-zero counts:\n{blob}"
    );
    assert!(
        !blob.contains("Edited 0 files (+0 -0)"),
        "full git-style apply_patch diff should not collapse to zero summary:\n{blob}"
    );
}

#[test]
fn diff_count_parser_handles_write_generated_update_diff_shape() {
    let diff = "diff --git a/foo.txt b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n";
    assert_eq!(
        crate::diff_render::calculate_add_remove_from_diff(diff),
        (1, 1)
    );
}

#[test]
fn diff_count_parser_handles_apply_patch_generated_update_diff_shape() {
    let diff = "diff --git a/update.txt b/update.txt\n@@ -1 +1 @@\n-old\n+new\n";
    assert_eq!(
        crate::diff_render::calculate_add_remove_from_diff(diff),
        (1, 1)
    );
}

#[test]
fn write_patch_applied_event_renders_edited_block() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Edited foo.txt") || blob.contains("Edited 1 file"),
        "expected edited patch block for write result, got:\n{blob}"
    );
}

#[test]
fn write_patch_applied_event_reports_non_zero_counts() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        !blob.contains("Edited 0 files (+0 -0)"),
        "write-derived edited block should not collapse to zero summary:\n{blob}"
    );
    assert!(
        blob.contains("(+1 -1)"),
        "write-derived edited block should report non-zero counts:\n{blob}"
    );
}

#[test]
fn patch_applied_event_with_diff_only_reports_non_zero_counts() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        !blob.contains("Edited 0 files (+0 -0)"),
        "patch-derived edited block should not collapse to zero summary:\n{blob}"
    );
}

#[test]
fn patch_applied_event_with_empty_update_is_not_rendered() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        devo_protocol::protocol::FileChange::Update {
            unified_diff: String::new(),
            old_text: None,
            new_text: None,
            move_path: None,
        },
    );

    widget.handle_worker_event(crate::events::WorkerEvent::PatchApplied {
        tool_use_id: "tool-1".to_string(),
        changes,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        !blob.contains("Edited"),
        "empty patch summary should not be rendered:\n{blob}"
    );
}

#[test]
fn session_switch_without_rich_edited_metadata_degrades_to_tool_result_path() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![crate::events::TranscriptItem::restored_tool_result(
            "Ran apply_patch output",
            "{\"diff\":\"diff --git a/foo.txt b/foo.txt\\n--- a/foo.txt\\n+++ b/foo.txt\\n@@ -1 +1 @@\\n-old\\n+new\\n\",\"files\":[{\"path\":\"foo.txt\",\"kind\":\"update\",\"additions\":1,\"deletions\":1}]}",
        )],
        rich_history_items: Vec::new(),
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Ran apply_patch output"),
        "missing rich metadata currently falls back to tool-result rendering:\n{blob}"
    );
}

#[test]
fn session_switch_restores_added_file_content_in_edited_block() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        PathBuf::from("quicksort.rs"),
        devo_protocol::protocol::FileChange::Add {
            content: "pub fn quicksort() {\n    println!(\"hi\");\n}\n".to_string(),
        },
    );
    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: Vec::new(),
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::ToolResult,
            title: "write".to_string(),
            body: String::new(),
            tool_io: None,
            metadata: Some(devo_protocol::SessionHistoryMetadata::Edited { changes }),
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: Vec::new(),
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        blob.contains("pub fn quicksort()"),
        "expected restored added file content:\n{blob}"
    );
    assert!(
        blob.contains("println!(\"hi\");"),
        "expected restored added file body:\n{blob}"
    );
}

#[test]
fn session_switch_without_rich_edited_metadata_still_restores_edited_block() {
    let cwd = std::env::current_dir().expect("current directory is available");
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, cwd);

    widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
        session_id: "session-1".to_string(),
        cwd: std::env::current_dir().expect("current directory is available"),
        title: None,
        model: Some("test-model".to_string()),

        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        active_agent_label: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_read_tokens: 0,
        last_query_total_tokens: 0,
        last_query_input_tokens: 0,
        prompt_token_estimate: 0,
        history_items: vec![crate::events::TranscriptItem::restored_tool_result(
            "Ran apply_patch output",
            "{\"diff\":\"diff --git a/foo.txt b/foo.txt\\n--- a/foo.txt\\n+++ b/foo.txt\\n@@ -1 +1 @@\\n-old\\n+new\\n\",\"files\":[{\"path\":\"foo.txt\",\"kind\":\"update\",\"additions\":1,\"deletions\":1}]}",
        )],
        rich_history_items: vec![devo_protocol::SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: devo_protocol::SessionHistoryItemKind::ToolResult,
            title: "apply_patch output".to_string(),
            body: "{\"diff\":\"diff --git a/foo.txt b/foo.txt\\n--- a/foo.txt\\n+++ b/foo.txt\\n@@ -1 +1 @@\\n-old\\n+new\\n\",\"files\":[{\"path\":\"foo.txt\",\"kind\":\"update\",\"additions\":1,\"deletions\":1}]}".to_string(),
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }],
        loaded_item_count: 1,
        pending_texts: vec![],
        collaboration_mode: CollaborationMode::Build,
    });

    let blob = scrollback_plain_lines(&widget.drain_scrollback_lines(80)).join("\n");
    assert!(
        blob.contains("Edited foo.txt") || blob.contains("Edited 1 file"),
        "fallback parse should restore edited block without rich metadata:\n{blob}"
    );
    assert!(
        !blob.contains("Ran apply_patch output"),
        "fallback parse should avoid tool-result degradation:\n{blob}"
    );
}
