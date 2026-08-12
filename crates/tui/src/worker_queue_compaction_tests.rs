use std::path::PathBuf;

use devo_protocol::ItemId;
use devo_protocol::Model;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use devo_server::ItemEnvelope;
use devo_server::ItemEventPayload;
use devo_server::ItemKind;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::WorkerEvent;
use crate::history_cell::ScrollbackLine;
use crate::tui::frame_requester::FrameRequester;

fn widget_with_model() -> ChatWidget {
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState::new(
            PathBuf::from("."),
            Some(Model {
                slug: "test-model".to_string(),
                display_name: "Test Model".to_string(),
                ..Model::default()
            }),
        ),
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

fn scrollback_plain_lines(lines: &[ScrollbackLine]) -> Vec<String> {
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

#[test]
fn queue_updated_drain_promotes_pending_into_history() {
    let mut widget = widget_with_model();
    widget.handle_app_event(AppEvent::ClearTranscript);
    widget.handle_worker_event(WorkerEvent::TurnStarted {
        model: "test-model".to_string(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        turn_id: TurnId::new(),
    });

    let queue_item_id = devo_protocol::native::ids::QueueItemId::from_string("qit_test".into());
    widget.handle_worker_event(WorkerEvent::QueueUpdated {
        change: devo_protocol::native::queue::QueueChange::Added,
        queue_item_id: queue_item_id.clone(),
        started_turn_id: None,
        entries: vec![devo_protocol::native::queue::QueueEntry {
            queue_item_id: queue_item_id.clone(),
            position: 1,
            input: vec![devo_protocol::native::item::UserInput::Text {
                text: "remote queued".to_string(),
            }],
            preview: "remote queued".to_string(),
            enqueued_at: chrono::Utc::now(),
        }],
    });
    assert!(widget.bottom_pane_has_pending_for_test());

    widget.handle_worker_event(WorkerEvent::QueueUpdated {
        change: devo_protocol::native::queue::QueueChange::Drained,
        queue_item_id,
        started_turn_id: Some(TurnId::new()),
        entries: Vec::new(),
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        history.contains("remote queued"),
        "expected drained queue entry to be promoted into history:\n{history}"
    );
}

#[test]
fn context_compaction_worker_event_adds_history_item() {
    let mut widget = widget_with_model();
    widget.handle_app_event(AppEvent::ClearTranscript);

    widget.handle_worker_event(WorkerEvent::ContextCompactionCompleted {
        title: "Context Compaction".to_string(),
    });

    let history = scrollback_plain_lines(&widget.drain_scrollback_lines(100)).join("\n");
    assert!(
        history.contains("Context compacted"),
        "expected completed context compaction to be visible in history:\n{history}"
    );
}

#[test]
fn completed_context_compaction_item_emits_worker_event() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    crate::worker::handle_completed_item(
        ItemEventPayload {
            context: devo_server::EventContext {
                session_id: SessionId::new(),
                turn_id: Some(TurnId::new()),
                item_id: None,
                seq: 1,
                item_seq: None,
            },
            item: ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: ItemKind::ContextCompaction,
                payload: serde_json::json!({
                    "title": "Context Compaction"
                }),
            },
        },
        &event_tx,
    );

    assert_eq!(
        event_rx.try_recv().expect("expected worker event"),
        WorkerEvent::ContextCompactionCompleted {
            title: "Context Compaction".to_string()
        }
    );
}
