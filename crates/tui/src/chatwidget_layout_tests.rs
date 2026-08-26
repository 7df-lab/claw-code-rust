use std::path::PathBuf;

use devo_protocol::Model;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn widget() -> ChatWidget {
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel::<AppEvent>();
    ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(PathBuf::from("."), Some(Model::default())),
        initial_reasoning_effort_selection: None,
        initial_permission_preset: devo_protocol::PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_compaction_token_limit: None,
        initial_default_collaboration_mode: devo_protocol::CollaborationMode::Build,
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
    })
}

#[test]
fn composer_footer_is_kept_at_content_height_for_single_and_wrapped_drafts() {
    for draft in ["draft", "first line\nsecond line"] {
        let mut widget = widget();
        let bottom_pane = widget.bottom_pane_mut_for_test();
        bottom_pane.set_text_content(draft.to_string(), Vec::new(), Vec::new());
        bottom_pane.set_status_line(Some(Line::from("status sentinel")));
        bottom_pane.set_status_line_enabled(true);

        let width = 80;
        let height = widget.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        widget.render(area, &mut buffer);

        let rendered = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            rendered.iter().any(|row| row.contains("status sentinel")),
            "status line disappeared for draft {draft:?}: {rendered:?}"
        );
    }
}
