use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use devo_protocol::Model;
use devo_protocol::ModelCatalog;
use devo_protocol::ProviderWireApi;
use futures::StreamExt;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::app::AppExit;
use crate::app::InteractiveTuiConfig;
use crate::onboarding::save_last_used_model;
use crate::onboarding::save_onboarding_config;
use crate::v2::app_command::AppCommand;
use crate::v2::app_event::AppEvent;
use crate::v2::app_event_sender::AppEventSender;
use crate::v2::chatwidget::ChatWidget;
use crate::v2::chatwidget::ChatWidgetInit;
use crate::v2::chatwidget::TuiSessionState;
use crate::v2::render::renderable::Renderable;
use crate::v2::tui::TuiEvent;
use crate::worker::QueryWorkerConfig;
use crate::worker::QueryWorkerHandle;

#[derive(Debug, Clone)]
struct PendingOnboarding {
    provider: ProviderWireApi,
    model: String,
    base_url: Option<String>,
    api_key: Option<String>,
}

pub async fn run_interactive_tui(config: InteractiveTuiConfig) -> Result<AppExit> {
    let initial_session = config.initial_session.clone();
    let terminal = crate::v2::tui::init()?;
    let mut tui = crate::v2::tui::Tui::new(terminal);
    let mut worker = QueryWorkerHandle::spawn(QueryWorkerConfig {
        model: initial_session.model.clone(),
        cwd: initial_session.cwd.clone(),
        server_log_level: config.server_log_level,
        thinking_selection: config.thinking_selection.clone(),
    });

    let (app_event_tx, mut app_event_rx) = mpsc::unbounded_channel();
    let app_event_sender = AppEventSender::new(app_event_tx);
    let available_models = config
        .model_catalog
        .list_visible()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let model = config
        .model_catalog
        .get(&initial_session.model)
        .cloned()
        .unwrap_or_else(|| Model {
            slug: initial_session.model.clone(),
            display_name: initial_session.model.clone(),
            provider: initial_session.provider,
            ..Model::default()
        });
    let cwd = initial_session.cwd.clone();
    let mut turn_count = 0usize;
    let mut total_input_tokens = 0usize;
    let mut total_output_tokens = 0usize;
    let mut pending_onboarding: Option<PendingOnboarding> = None;
    let mut busy = false;
    let mut last_ctrl_c_at: Option<Instant> = None;
    let mut chat_widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: tui.frame_requester(),
        app_event_tx: app_event_sender,
        initial_session: TuiSessionState::new(cwd.clone(), Some(model)),
        initial_user_message: None,
        enhanced_keys_supported: tui.enhanced_keys_supported(),
        is_first_run: true,
        available_models,
        show_model_onboarding: config.show_model_onboarding,
        startup_tooltip_override: Some(format!("Ready in {}", cwd.display())),
    });
    if let Some(thinking_selection) = config.thinking_selection {
        chat_widget.set_thinking_selection(Some(thinking_selection));
    }

    let events = tui.event_stream();
    tokio::pin!(events);

    tui.frame_requester().schedule_frame();

    loop {
        tokio::select! {
            tui_event = events.next() => {
                let Some(tui_event) = tui_event else {
                    break;
                };
                match tui_event {
                    TuiEvent::Draw => {
                        chat_widget.pre_draw_tick();
                        if chat_widget.is_resume_browser_open() && !tui.is_alt_screen_active() {
                            tui.enter_alt_screen()?;
                        } else if !chat_widget.is_resume_browser_open() && tui.is_alt_screen_active() {
                            tui.leave_alt_screen()?;
                        }
                        let width = tui.terminal.size()?.width.max(1);
                        let scrollback_lines = chat_widget.drain_scrollback_lines(width);
                        if !scrollback_lines.is_empty() {
                            tui.insert_history_lines(scrollback_lines);
                        }
                        let height = chat_widget
                            .desired_height(width)
                            .min(tui.terminal.size()?.height.saturating_sub(1))
                            .max(3);
                        tui.draw(height, |frame| {
                            let area = frame.area();
                            chat_widget.render(area, frame.buffer_mut());
                            if let Some((x, y)) = chat_widget.cursor_pos(area) {
                                frame.set_cursor_position((x, y));
                            }
                        })?;
                    }
                    TuiEvent::Key(key) => {
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if busy {
                                worker.interrupt_turn()?;
                                chat_widget.set_status_message("Interrupted; waiting for model to stop");
                            } else {
                                let now = Instant::now();
                                if last_ctrl_c_at
                                    .is_some_and(|last| now.duration_since(last) <= Duration::from_secs(2))
                                {
                                    break;
                                }
                                last_ctrl_c_at = Some(now);
                                chat_widget.set_status_message("Press Ctrl-C again within 2s to exit");
                            }
                        } else {
                            last_ctrl_c_at = None;
                            chat_widget.handle_key_event(key);
                        }
                    }
                    TuiEvent::Paste(text) => {
                        chat_widget.handle_paste(text);
                    }
                }
            }
            app_event = app_event_rx.recv() => {
                let Some(app_event) = app_event else {
                    break;
                };
                if let AppEvent::Exit(exit_mode) = &app_event {
                    if matches!(exit_mode, crate::v2::app_event::ExitMode::ShutdownFirst) {
                        if tui.is_alt_screen_active() {
                            tui.leave_alt_screen()?;
                        }
                        tui.terminal.clear_scrollback_and_visible_screen_ansi()?;
                    }
                    break;
                }
                if let AppEvent::Command(command) = &app_event {
                    handle_app_command(
                        command,
                        &worker,
                        &mut chat_widget,
                        &config.model_catalog,
                        initial_session.provider,
                        &mut pending_onboarding,
                    )?;
                }
                chat_widget.handle_app_event(app_event);
            }
            worker_event = worker.event_rx.recv() => {
                let Some(worker_event) = worker_event else {
                    chat_widget.set_status_message("Background worker stopped");
                    break;
                };
                match &worker_event {
                    crate::events::WorkerEvent::TurnFinished {
                        turn_count: next_turn_count,
                        total_input_tokens: next_total_input_tokens,
                        total_output_tokens: next_total_output_tokens,
                        ..
                    }
                    | crate::events::WorkerEvent::TurnFailed {
                        turn_count: next_turn_count,
                        total_input_tokens: next_total_input_tokens,
                        total_output_tokens: next_total_output_tokens,
                        ..
                    } => {
                        busy = false;
                        turn_count = *next_turn_count;
                        total_input_tokens = *next_total_input_tokens;
                        total_output_tokens = *next_total_output_tokens;
                    }
                    crate::events::WorkerEvent::TurnStarted { .. } => {
                        busy = true;
                    }
                    crate::events::WorkerEvent::UsageUpdated {
                        total_input_tokens: next_total_input_tokens,
                        total_output_tokens: next_total_output_tokens,
                    } => {
                        total_input_tokens = *next_total_input_tokens;
                        total_output_tokens = *next_total_output_tokens;
                    }
                    crate::events::WorkerEvent::ProviderValidationSucceeded { .. } => {
                        if let Some(pending) = pending_onboarding.take() {
                            save_onboarding_config(
                                pending.provider,
                                &pending.model,
                                pending.base_url.as_deref(),
                                pending.api_key.as_deref(),
                            )?;
                            worker.reconfigure_provider(
                                pending.provider,
                                pending.model,
                                pending.base_url,
                                pending.api_key,
                            )?;
                        }
                    }
                    crate::events::WorkerEvent::ProviderValidationFailed { .. } => {
                        pending_onboarding = None;
                    }
                    _ => {}
                }
                chat_widget.handle_worker_event(worker_event);
            }
        }
    }

    drop(tui);
    worker.shutdown().await?;
    Ok(AppExit {
        turn_count,
        total_input_tokens,
        total_output_tokens,
    })
}

fn handle_app_command(
    command: &AppCommand,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    model_catalog: &impl ModelCatalog,
    default_provider: ProviderWireApi,
    pending_onboarding: &mut Option<PendingOnboarding>,
) -> Result<()> {
    match command {
        AppCommand::UserTurn {
            input,
            model,
            thinking,
            ..
        } => {
            if let Some(model) = model {
                worker.set_model(model.clone())?;
            }
            worker.set_thinking(thinking.clone())?;
            let prompt = input
                .iter()
                .filter_map(|item| match item {
                    devo_protocol::InputItem::Text { text } => Some(text.as_str()),
                    devo_protocol::InputItem::Skill { .. }
                    | devo_protocol::InputItem::LocalImage { .. }
                    | devo_protocol::InputItem::Mention { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            worker.submit_prompt(prompt)?;
        }
        AppCommand::OverrideTurnContext {
            model, thinking, ..
        } => {
            if let Some(model) = model {
                worker.set_model(model.clone())?;
                save_last_used_model(/*wire_api*/ None, default_provider, model)?;
            }
            if let Some(thinking) = thinking {
                worker.set_thinking(thinking.clone())?;
            }
        }
        AppCommand::RunUserShellCommand { command } if command == "session new" => {
            worker.start_new_session()?;
        }
        AppCommand::RunUserShellCommand { command } if command == "session list" => {
            worker.list_sessions()?;
        }
        AppCommand::RunUserShellCommand { command } if command.starts_with("onboard ") => {
            let payload = command.trim_start_matches("onboard ");
            let value: serde_json::Value = serde_json::from_str(payload)?;
            let model = value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let base_url = value
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let api_key = value
                .get("api_key")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let provider = model_catalog
                .get(&model)
                .map(Model::provider_wire_api)
                .unwrap_or(default_provider);
            *pending_onboarding = Some(PendingOnboarding {
                provider,
                model: model.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
            });
            worker.validate_provider(provider, model, base_url, api_key)?;
        }
        AppCommand::BrowseInputHistory { direction } => {
            worker.browse_input_history(*direction)?;
        }
        AppCommand::SwitchSession { session_id } => {
            worker.switch_session(*session_id)?;
        }
        _ => {
            chat_widget.set_status_message(format!("Unsupported command: {}", command.kind()));
        }
    }
    Ok(())
}
