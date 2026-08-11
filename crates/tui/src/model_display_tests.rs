//! Tests for TUI display of provider-specific saved model names.

use std::path::PathBuf;

use devo_core::PresetModelCatalog;
use devo_protocol::Model;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderWireApi;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::app::InitialTuiSession;
use crate::app::InteractiveTuiConfig;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::TuiSessionState;
use crate::events::SavedModelEntry;
use crate::render::renderable::Renderable;
use crate::tui::frame_requester::FrameRequester;

fn rendered_text(widget: &ChatWidget, width: u16, height: u16) -> String {
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    widget.render(area, &mut buf);
    (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|col| buf[(col, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn widget_with_model(
    model: Model,
    request_model: Option<String>,
    available_models: Vec<Model>,
    saved_models: Vec<SavedModelEntry>,
) -> ChatWidget {
    let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
    ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState {
            cwd: PathBuf::from("."),
            model: Some(model),
            request_model,
            model_binding_id: None,
            provider: Some(ProviderWireApi::OpenAIChatCompletions),
            reasoning_effort: None,
            active_agent_label: None,
        },
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_compaction_token_limit: None,
        initial_default_collaboration_mode: devo_protocol::CollaborationMode::Build,
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models,
        saved_models,
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    })
}

#[test]
fn status_summary_prefers_provider_request_model() {
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "deepseek-v4-flash".to_string(),
        ..Model::default()
    };
    let widget = widget_with_model(
        model,
        Some("DeepSeek-V4-Flash".to_string()),
        Vec::new(),
        Vec::new(),
    );

    let rendered = rendered_text(&widget, 100, 16);

    assert_eq!(rendered.contains("DeepSeek-V4-Flash"), true);
    assert_eq!(
        widget.status_summary_text().contains("DeepSeek-V4-Flash"),
        true
    );
}

#[test]
fn saved_model_metadata_overlays_catalog_display_for_picker() {
    let catalog_model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "deepseek-v4-flash".to_string(),
        provider: ProviderWireApi::OpenAIChatCompletions,
        ..Model::default()
    };
    let config = InteractiveTuiConfig {
        initial_session: InitialTuiSession {
            session_id: None,
            model: "deepseek-v4-flash".to_string(),
            request_model: Some("DeepSeek-V4-Flash".to_string()),
            model_binding_id: Some("deepseek-main".to_string()),
            provider: ProviderWireApi::OpenAIChatCompletions,
            reasoning_effort_selection: None,
            permission_preset: PermissionPreset::Default,
            sandbox_profile: Some("workspace".to_string()),
            compaction_token_limit: None,
            default_collaboration_mode: devo_protocol::CollaborationMode::Build,
            cwd: PathBuf::from("."),
        },
        server_log_level: None,
        model_catalog: PresetModelCatalog::new(vec![catalog_model]),
        saved_models: vec![SavedModelEntry {
            binding_id: Some("deepseek-main".to_string()),
            model: "deepseek-v4-flash".to_string(),
            request_model: Some("DeepSeek-V4-Flash".to_string()),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            provider_id: Some("deepseek".to_string()),
            provider_name: Some("DeepSeek".to_string()),
            wire_api: ProviderWireApi::OpenAIChatCompletions,
            base_url: None,
            api_key: None,
        }],
        show_model_onboarding: false,
        exit_after_onboarding: false,
    };

    let available_models = crate::interactive::available_models_with_saved_metadata(&config);

    assert_eq!(
        available_models,
        vec![Model {
            display_name: "DeepSeek-V4-Flash".to_string(),
            ..Model {
                slug: "deepseek-v4-flash".to_string(),
                display_name: "deepseek-v4-flash".to_string(),
                provider: ProviderWireApi::OpenAIChatCompletions,
                ..Model::default()
            }
        }]
    );
}

#[test]
fn model_picker_renders_saved_display_name() {
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek-V4-Flash".to_string(),
        provider: ProviderWireApi::OpenAIChatCompletions,
        ..Model::default()
    };
    let mut widget = widget_with_model(
        model.clone(),
        Some("DeepSeek-V4-Flash".to_string()),
        vec![model],
        vec![SavedModelEntry {
            binding_id: Some("deepseek-main".to_string()),
            model: "deepseek-v4-flash".to_string(),
            request_model: Some("DeepSeek-V4-Flash".to_string()),
            display_name: Some("DeepSeek-V4-Flash".to_string()),
            provider_id: Some("deepseek".to_string()),
            provider_name: Some("DeepSeek".to_string()),
            wire_api: ProviderWireApi::OpenAIChatCompletions,
            base_url: None,
            api_key: None,
        }],
    );

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "model".to_string(),
    });
    let rendered = rendered_text(&widget, 100, 20);

    assert_eq!(rendered.contains("DeepSeek-V4-Flash"), true);
    assert_eq!(rendered.contains("DeepSeek"), true);
}

#[test]
fn model_picker_distinguishes_same_model_slug_by_provider_binding() {
    let model = Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "deepseek-v4-flash".to_string(),
        provider: ProviderWireApi::OpenAIChatCompletions,
        ..Model::default()
    };
    let saved_models = vec![
        SavedModelEntry {
            binding_id: Some("deepseek-v4-flash-deepseek".to_string()),
            model: "deepseek-v4-flash".to_string(),
            request_model: None,
            display_name: Some("deepseek-v4-flash".to_string()),
            provider_id: Some("deepseek".to_string()),
            provider_name: Some("DeepSeek".to_string()),
            wire_api: ProviderWireApi::OpenAIChatCompletions,
            base_url: None,
            api_key: None,
        },
        SavedModelEntry {
            binding_id: Some("deepseek-v4-flash-openrouter".to_string()),
            model: "deepseek-v4-flash".to_string(),
            request_model: None,
            display_name: Some("deepseek-v4-flash".to_string()),
            provider_id: Some("openrouter".to_string()),
            provider_name: Some("OpenRouter".to_string()),
            wire_api: ProviderWireApi::OpenAIChatCompletions,
            base_url: None,
            api_key: None,
        },
    ];
    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let mut widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(app_event_tx),
        initial_session: TuiSessionState {
            cwd: PathBuf::from("."),
            model: Some(model.clone()),
            request_model: None,
            model_binding_id: Some("deepseek-v4-flash-deepseek".to_string()),
            provider: Some(ProviderWireApi::OpenAIChatCompletions),
            reasoning_effort: None,
            active_agent_label: None,
        },
        initial_reasoning_effort_selection: None,
        initial_permission_preset: PermissionPreset::Default,
        initial_sandbox_profile: Some("workspace".to_string()),
        initial_compaction_token_limit: None,
        initial_default_collaboration_mode: devo_protocol::CollaborationMode::Build,
        initial_user_message: None,
        enhanced_keys_supported: true,
        is_first_run: false,
        available_models: vec![model],
        saved_models,
        show_model_onboarding: false,
        exit_after_onboarding: false,
        startup_tooltip_override: None,
        initial_theme_name: None,
        initial_collapse_reasoning: false,
    });

    widget.handle_app_event(AppEvent::RunSlashCommand {
        command: "model".to_string(),
    });
    let rendered = rendered_text(&widget, 100, 20);

    assert!(rendered.matches("deepseek-v4-flash").count() >= 2);
    assert_eq!(rendered.contains("DeepSeek"), true);
    assert_eq!(rendered.contains("OpenRouter"), true);

    widget.handle_app_event(AppEvent::ModelSelected {
        model: "deepseek-v4-flash-openrouter".to_string(),
    });

    assert_eq!(
        app_event_rx
            .try_recv()
            .expect("context override command is emitted"),
        AppEvent::Command(AppCommand::OverrideTurnContext {
            cwd: None,
            model: Some("deepseek-v4-flash-openrouter".to_string()),
            reasoning_effort_selection: Some(None),
            sandbox: None,
            approval_policy: None,
            persist_scope: crate::app_command::PersistScope::Session,
        })
    );
}
