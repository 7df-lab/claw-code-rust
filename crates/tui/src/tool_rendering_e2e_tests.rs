use std::path::PathBuf;

use devo_protocol::Model;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::WorkerEvent;
use crate::tui::frame_requester::FrameRequester;

fn widget_with_model(
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
    });
    (widget, app_event_rx)
}

fn active_display(widget: &ChatWidget) -> String {
    widget
        .active_cell_display_lines_for_test(100)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn streaming_read_and_glob_updates_render_in_one_explored_cell() {
    let model = Model {
        slug: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        ..Model::default()
    };
    let (mut widget, _app_event_rx) = widget_with_model(model, PathBuf::from("."));

    widget.handle_worker_event(WorkerEvent::ToolCall {
        tool_use_id: "read-1".to_string(),
        summary: "read {}".to_string(),
        preparing: false,
        parsed_commands: Some(vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: String::new(),
            name: String::new(),
            path: PathBuf::new(),
        }]),
    });
    assert_eq!(
        active_display(&widget).contains("Running read {}"),
        false,
        "read start must render as explored placeholder"
    );

    widget.handle_worker_event(WorkerEvent::ToolCallUpdated {
        tool_use_id: "read-1".to_string(),
        summary: "read README.md".to_string(),
        parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read README.md".to_string(),
            name: "README.md".to_string(),
            path: PathBuf::from("README.md"),
        }],
    });
    widget.handle_worker_event(WorkerEvent::ToolResult {
        tool_use_id: "read-1".to_string(),
        title: "read README.md".to_string(),
        preview: "# Devo".to_string(),
        is_error: false,
        truncated: false,
    });

    widget.handle_worker_event(WorkerEvent::ToolCall {
        tool_use_id: "glob-1".to_string(),
        summary: "glob {}".to_string(),
        preparing: false,
        parsed_commands: Some(vec![
            devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob".to_string(),
                path: Some("glob".to_string()),
            },
        ]),
    });
    widget.handle_worker_event(WorkerEvent::ToolCallUpdated {
        tool_use_id: "glob-1".to_string(),
        summary: "glob **/Cargo.toml in crates".to_string(),
        parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: "glob **/Cargo.toml in crates".to_string(),
            path: Some("**/Cargo.toml in crates".to_string()),
        }],
    });
    widget.handle_worker_event(WorkerEvent::ToolResult {
        tool_use_id: "glob-1".to_string(),
        title: "glob **/Cargo.toml in crates".to_string(),
        preview: "crates/tools/Cargo.toml".to_string(),
        is_error: false,
        truncated: false,
    });

    let display = active_display(&widget);
    assert!(
        display.contains("Explored"),
        "expected explored group:\n{display}"
    );
    assert!(
        display.contains("Read README.md"),
        "expected final read file name:\n{display}"
    );
    assert!(
        display.contains("List **/Cargo.toml in crates"),
        "expected final glob parameters:\n{display}"
    );
    assert!(
        !display.contains("Running read {}"),
        "read must not render as generic running tool:\n{display}"
    );
    assert!(
        !display.contains("Ran read"),
        "read result must not create a generic ran cell:\n{display}"
    );
    assert!(
        !display.contains("List glob"),
        "glob placeholder must be replaced in place:\n{display}"
    );
}
