//! Hosts the interactive TUI event loop and connects app events, worker events, and
//! terminal rendering into one session.

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_protocol::Model;
use devo_protocol::ModelCatalog;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderWireApi;
use devo_util_paths::find_devo_home;
use futures::StreamExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::app::AppExit;
use crate::app::InitialTuiSession;
use crate::app::InteractiveTuiConfig;
use crate::app_command::AppCommand;
use crate::app_command::PersistScope;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::MCP_SERVERS_TRANSCRIPT_TITLE;
use crate::chatwidget::TuiSessionState;
use crate::chatwidget::UserMessage;
use crate::events::WorkerEvent;
use crate::host_overlay::OverlayState;
use crate::onboarding::OnboardingModelBinding;
use crate::onboarding::onboarding_provider_model_binding;
use crate::onboarding::onboarding_provider_vendor;
use crate::onboarding::save_default_collaboration_mode;
use crate::onboarding::save_last_used_model;
use crate::onboarding::save_project_permission_preset;
use crate::onboarding::save_project_sandbox_profile;
use crate::onboarding::save_reasoning_effort_selection;
use crate::pager_overlay::TranscriptOverlay;
use crate::render::renderable::Renderable;
use crate::tui::Tui;
use crate::tui::TuiEvent;
use crate::worker::QueryWorkerConfig;
use crate::worker::QueryWorkerHandle;

const APP_EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
struct PendingOnboarding {
    binding: OnboardingModelBinding,
    base_url: Option<String>,
    api_key: Option<String>,
    provider_credential_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OnboardingCommandPayload {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnboardingCommandAction {
    Validate,
    SkipValidation,
}

fn parse_onboarding_command(
    command: &str,
) -> Option<(OnboardingCommandAction, OnboardingCommandPayload)> {
    let (action, payload) = if let Some(payload) = command.strip_prefix("onboard-skip-validation ")
    {
        (OnboardingCommandAction::SkipValidation, payload)
    } else {
        (
            OnboardingCommandAction::Validate,
            command.strip_prefix("onboard ")?,
        )
    };
    serde_json::from_str(payload)
        .ok()
        .map(|payload| (action, payload))
}

fn normalized_display_name(
    model_catalog: &impl ModelCatalog,
    model_slug: &str,
    selected_display_name: &str,
) -> String {
    let selected = selected_display_name.trim();
    if !selected.is_empty() && selected != model_slug {
        return selected.to_string();
    }
    model_catalog
        .get(model_slug)
        .map(|model| model.display_name.clone())
        .unwrap_or_else(|| model_slug.to_string())
}

#[derive(Debug, Default)]
struct InteractiveLoopState {
    session_id: Option<devo_core::SessionId>,
    onboarding_completed: bool,
    turn_count: usize,
    total_input_tokens: usize,
    total_output_tokens: usize,
    total_tokens: usize,
    total_cache_read_tokens: usize,
    pending_onboarding: Option<PendingOnboarding>,
    // True while the resume browser is waiting for the worker's session list.
    resume_browser_pending: bool,
    // indicate whther LLM worker is working, is started by TurnStarted,
    // it ended by TurnFailed/TurnFinished
    busy: bool,
    // True after clearing the inline UI for a session switch and before the
    // replacement session has been restored into widget state.
    session_switch_pending: bool,
    pending_backtrack_restore: Option<UserMessage>,
    last_ctrl_c_at: Option<Instant>,
    esc_backtrack_primed: bool,
    overlay: OverlayState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopAction {
    Continue,
    ClearAndExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtrlCKeyAction {
    PromptInterruptWithEsc,
    PromptExitConfirmation,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscBacktrackAction {
    Noop,
    PrimeHint,
    OpenOverlay,
    ClearHint,
}

#[derive(Debug, Clone, PartialEq)]
enum TranscriptBacktrackSelection {
    Selected {
        user_message: UserMessage,
        user_turn_index: u32,
    },
    NoSelection,
}

struct AppCommandContext<'a, M: ModelCatalog> {
    model_catalog: &'a M,
    default_provider: ProviderWireApi,
    cwd: &'a Path,
    project_config_key: &'a str,
    app_event_tx: &'a AppEventSender,
}

/// RAII guard that restores terminal modes exactly once after the TUI loop ends.
///
/// The restore is owned by the outer host instead of `Tui::drop()`:
///
/// ```text
/// app loop exits
///    |
///    v
/// clear live TUI area
///    |
///    v
/// drop Tui wrapper
///    |
///    v
/// restore terminal modes once
///    |
///    v
/// shell prints the next prompt
/// ```
///
/// This avoids the older pattern where the `Tui` drop path emitted extra terminal
/// control sequences after the clear, which could cause prompt drift in Terminal.app.
struct TerminalRestoreGuard {
    active: bool,
}

impl TerminalRestoreGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn restore(&mut self) -> Result<()> {
        if self.active {
            crate::tui::restore()?;
            self.active = false;
        }
        Ok(())
    }

    fn restore_silently(&mut self) {
        if self.active {
            if let Err(err) = crate::tui::restore() {
                eprintln!(
                    "failed to restore terminal. Run `reset` or restart your terminal to recover: {err}"
                );
            }
            self.active = false;
        }
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        self.restore_silently();
    }
}

/// Runs the interactive terminal UI until the user exits or the worker stops.
pub async fn run_interactive_tui(config: InteractiveTuiConfig) -> Result<AppExit> {
    // Build the initial terminal, session, and background worker state.
    let initial_session = config.initial_session.clone();
    let terminal = crate::tui::init()?;
    let mut tui = crate::tui::Tui::new(terminal);
    let mut terminal_restore_guard = TerminalRestoreGuard::new();

    // spawn a worker with stdio transport with server, it'll emit events
    // such as `[WorkerEvent::TurnStarted]`, `[WorkerEvent::UsageUpdated]` etc.
    let mut worker = QueryWorkerHandle::spawn(QueryWorkerConfig {
        initial_session_id: initial_session.session_id,
        model: initial_session.model.clone(),
        model_binding_id: initial_session.model_binding_id.clone(),
        cwd: initial_session.cwd.clone(),
        server_log_level: config.server_log_level.clone(),
        reasoning_effort_selection: initial_session.reasoning_effort_selection.clone(),
        permission_preset: initial_session.permission_preset,
        default_collaboration_mode: initial_session.default_collaboration_mode,
        initial_sandbox_profile: initial_session.sandbox_profile.clone(),
        client_capabilities: devo_protocol::AcpClientCapabilities {
            fs: devo_protocol::AcpFileSystemCapabilities {
                read_text_file: false,
                write_text_file: false,
                meta: None,
            },
            terminal: false,
            meta: None,
            ..devo_protocol::AcpClientCapabilities::default()
        },
    });

    // App events come from widgets and request host-level actions such as commands or exit.
    let (app_event_tx, mut app_event_rx) = mpsc::channel(APP_EVENT_CHANNEL_CAPACITY);
    let app_event_sender = AppEventSender::new_bounded(app_event_tx);
    let host_app_event_sender = app_event_sender.clone();

    // Resolve model metadata for the chat widget, falling back to the session slug.
    let available_models = available_models_with_saved_metadata(&config);

    let cwd = initial_session.cwd.clone();
    let project_config_key = devo_core::project_config_key(&cwd);

    let model = resolve_initial_model(&initial_session, &available_models);
    let request_model = initial_session
        .request_model
        .clone()
        .and_then(|request_model| {
            if request_model == model.slug {
                None
            } else {
                Some(request_model)
            }
        });
    let initial_provider = model.provider_wire_api();
    let initial_reasoning_effort = model
        .resolve_reasoning_effort_selection(initial_session.reasoning_effort_selection.as_deref())
        .effective_reasoning_effort;

    let mut loop_state = InteractiveLoopState::default();

    let initial_theme_name = crate::onboarding::load_theme_selection();

    // Create the root chat widget that owns visible TUI state and input handling.
    let mut chat_widget = ChatWidget::new_with_app_event(ChatWidgetInit {
        frame_requester: tui.frame_requester(),
        app_event_tx: app_event_sender,
        initial_session: TuiSessionState {
            cwd: cwd.clone(),
            model: Some(model),
            request_model,
            model_binding_id: initial_session.model_binding_id.clone(),
            provider: Some(initial_provider),
            reasoning_effort: initial_reasoning_effort,
            active_agent_label: None,
        },
        initial_reasoning_effort_selection: initial_session.reasoning_effort_selection.clone(),
        initial_permission_preset: initial_session.permission_preset,
        initial_sandbox_profile: initial_session.sandbox_profile.clone(),
        initial_compaction_token_limit: initial_session.compaction_token_limit,
        initial_default_collaboration_mode: initial_session.default_collaboration_mode,
        initial_user_message: None,
        enhanced_keys_supported: tui.enhanced_keys_supported(),
        is_first_run: config.saved_models.is_empty(),
        available_models,
        saved_models: config.saved_models.clone(),
        show_model_onboarding: config.show_model_onboarding,
        exit_after_onboarding: config.exit_after_onboarding,
        startup_tooltip_override: Some(format!("Ready in {}", cwd.display())),
        initial_theme_name,
        initial_collapse_reasoning: crate::onboarding::load_collapse_reasoning(),
    });

    if initial_session.session_id.is_some() && !config.show_model_onboarding {
        chat_widget.begin_session_resume();
    }

    // tui events, such as `[TuiEvent::Draw]`, `[TuiEvent::Key]`, `TuiEvent::Paste`
    let events = tui.event_stream();
    tokio::pin!(events);

    tui.frame_requester().schedule_frame();

    // Drive the TUI by racing terminal input, app commands, and worker events.
    loop {
        tokio::select! {
            tui_event = events.next() => {
                match handle_tui_event(
                    tui_event,
                    &mut tui,
                    &worker,
                    &mut chat_widget,
                    &mut loop_state,
                )? {
                    LoopAction::Continue => {}
                    LoopAction::ClearAndExit => {
                        tracing::info!("interactive loop exiting from tui event");
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
            app_event = app_event_rx.recv() => {
                match handle_app_event(
                    app_event,
                    &worker,
                    &mut chat_widget,
                    &mut tui,
                    &mut loop_state,
                    &AppCommandContext {
                        model_catalog: &config.model_catalog,
                        default_provider: initial_session.provider,
                        cwd: &cwd,
                        project_config_key: &project_config_key,
                        app_event_tx: &host_app_event_sender,
                    },
                )? {
                    LoopAction::Continue => {}
                    LoopAction::ClearAndExit => {
                        tracing::info!("interactive loop exiting from app event");
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
            worker_event = worker.event_rx.recv() => {
                match handle_worker_event(
                    worker_event,
                    &worker,
                    &mut chat_widget,
                    &mut loop_state,
                )? {
                    LoopAction::Continue => {}
                    LoopAction::ClearAndExit => {
                        tracing::info!("interactive loop exiting from worker event");
                        clear_before_exit(&mut tui)?;
                        break;
                    }
                }
            }
        }
    }

    // Tear down the terminal wrapper before awaiting worker shutdown.
    tracing::info!("dropping tui before terminal restore");
    drop(tui);
    tracing::info!("restoring terminal before worker shutdown");
    terminal_restore_guard.restore()?;
    tracing::info!("terminal restored; starting worker shutdown");
    worker.shutdown().await?;
    tracing::info!("worker shutdown completed; returning app exit");
    Ok(AppExit {
        session_id: loop_state.session_id,
        onboarding_completed: loop_state.onboarding_completed,
        turn_count: loop_state.turn_count,
        total_input_tokens: loop_state.total_input_tokens,
        total_output_tokens: loop_state.total_output_tokens,
        total_tokens: loop_state.total_tokens,
        total_cache_read_tokens: loop_state.total_cache_read_tokens,
    })
}

pub(crate) fn available_models_with_saved_metadata(config: &InteractiveTuiConfig) -> Vec<Model> {
    let mut available_models = config
        .model_catalog
        .list_visible()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    for saved_model in &config.saved_models {
        let display_name = saved_model
            .display_name
            .as_deref()
            .or(saved_model.request_model.as_deref())
            .map(str::trim)
            .filter(|display_name| !display_name.is_empty())
            .unwrap_or(saved_model.model.as_str())
            .to_string();

        if let Some(model) = available_models
            .iter_mut()
            .find(|model| model.slug == saved_model.model)
        {
            model.display_name = display_name;
            continue;
        }

        available_models.push(Model {
            slug: saved_model.model.clone(),
            display_name,
            provider: saved_model.wire_api,
            ..Model::default()
        });
    }

    available_models
}

fn resolve_initial_model(initial_session: &InitialTuiSession, available_models: &[Model]) -> Model {
    available_models
        .iter()
        .find(|model| model.slug == initial_session.model)
        .cloned()
        .unwrap_or_else(|| Model {
            slug: initial_session.model.clone(),
            display_name: initial_session.model.clone(),
            provider: initial_session.provider,
            ..Model::default()
        })
}

fn clear_before_exit(tui: &mut Tui) -> Result<()> {
    tracing::info!("clearing tui before exit");
    let result = tui.shutdown_terminal_safe();
    tracing::info!(
        success = result.is_ok(),
        "finished clearing tui before exit"
    );
    Ok(result?)
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn handle_tui_event(
    tui_event: Option<TuiEvent>,
    tui: &mut Tui,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    loop_state: &mut InteractiveLoopState,
) -> Result<LoopAction> {
    let Some(tui_event) = tui_event else {
        return Ok(LoopAction::ClearAndExit);
    };

    if loop_state.overlay.is_active() {
        if let TuiEvent::Key(key_event) = tui_event
            && matches!(
                key_event.kind,
                crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat
            )
            && key_event.code == KeyCode::Enter
        {
            let selection = loop_state
                .overlay
                .parent_transcript()
                .map(selected_transcript_backtrack_selection)
                .unwrap_or(TranscriptBacktrackSelection::NoSelection);
            match selection {
                TranscriptBacktrackSelection::Selected {
                    user_message,
                    user_turn_index,
                } => {
                    if loop_state.busy {
                        loop_state.overlay.close(tui)?;
                        chat_widget.set_status_message("Cannot edit message while generating");
                        return Ok(LoopAction::Continue);
                    }
                    loop_state.pending_backtrack_restore = Some(user_message);
                    loop_state.overlay.close(tui)?;
                    loop_state.session_switch_pending = true;
                    tui.replace_inline_session_ui()?;
                    worker.rollback_before_user_turn(user_turn_index)?;
                    return Ok(LoopAction::Continue);
                }
                TranscriptBacktrackSelection::NoSelection => {}
            }
        }
        if matches!(tui_event, TuiEvent::Draw) {
            chat_widget.pre_draw_tick();
        }
        loop_state
            .overlay
            .handle_tui_event(tui_event, tui, chat_widget)?;
        return Ok(LoopAction::Continue);
    }

    match tui_event {
        TuiEvent::Draw => {
            if loop_state.session_switch_pending {
                return Ok(LoopAction::Continue);
            }

            // Update time-sensitive widget state before measuring or rendering.
            chat_widget.pre_draw_tick();

            if !chat_widget.is_resume_browser_open()
                && !loop_state.resume_browser_pending
                && !loop_state.overlay.is_active()
                && tui.is_alt_screen_active()
            {
                tui.leave_alt_screen()?;
            }

            // Wrap pending scrollback using the current terminal width.
            let width = tui.terminal.size()?.width.max(1);
            // Completed transcript lines are written directly above the live inline viewport.
            let scrollback_lines = chat_widget.drain_scrollback_lines(width);

            if !scrollback_lines.is_empty() {
                tui.insert_history_lines(scrollback_lines);
            }

            // Size the chat area within the visible terminal and render the frame.
            let height = chat_widget
                .desired_height(width)
                .min(tui.terminal.size()?.height.saturating_sub(1))
                .max(3);

            tui.draw(height, |frame| {
                let area = frame.area();
                chat_widget.render(area, frame.buffer_mut());
                // Restore the terminal cursor to the widget-provided input position.
                if let Some((x, y)) = chat_widget.cursor_pos(area) {
                    frame.set_cursor_position((x, y));
                }
            })?;
            let assistant_render_snapshot =
                chat_widget.active_assistant_render_snapshot(tui.terminal.viewport_area);
            chat_widget.note_active_assistant_terminal_flush(
                assistant_render_snapshot.as_ref(),
                tui.terminal.last_flush_stats(),
            );
        }
        TuiEvent::Key(key) => {
            if chat_widget.handle_onboarding_key_event(key) {
                return Ok(LoopAction::Continue);
            }
            if chat_widget.is_onboarding_active() && ChatWidget::is_copy_shortcut(key) {
                return Ok(LoopAction::Continue);
            }

            if matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('\n' | '\r') | KeyCode::Modifier(_)
            ) || (matches!(key.code, KeyCode::Char('j' | 'm'))
                && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                tracing::debug!(
                    code = ?key.code,
                    modifiers = ?key.modifiers,
                    kind = ?key.kind,
                    state = ?key.state,
                    "received enter-like key event"
                );
            }
            // Keep Ctrl-C available for terminal copy workflows while work is
            // active. Cancellation is owned by the bottom pane's Esc flow.
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                match handle_ctrl_c_key(loop_state, Instant::now()) {
                    CtrlCKeyAction::PromptInterruptWithEsc => {
                        chat_widget.set_status_message("Press Esc twice to interrupt");
                    }
                    CtrlCKeyAction::PromptExitConfirmation => {
                        chat_widget.set_status_message("Press Ctrl-C again to exit");
                    }
                    CtrlCKeyAction::Exit => {
                        return Ok(LoopAction::ClearAndExit);
                    }
                }
                return Ok(LoopAction::Continue);
            }

            if key.code == KeyCode::Char('t')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && !chat_widget.is_resume_browser_open()
            {
                loop_state.overlay.open_transcript(tui, chat_widget)?;
                return Ok(LoopAction::Continue);
            }

            loop_state.last_ctrl_c_at = None;
            match determine_esc_backtrack_action(
                key,
                loop_state.esc_backtrack_primed,
                chat_widget.is_normal_backtrack_mode(),
                chat_widget.composer_is_empty(),
            ) {
                EscBacktrackAction::PrimeHint => {
                    loop_state.esc_backtrack_primed = true;
                    chat_widget.show_esc_backtrack_hint();
                    return Ok(LoopAction::Continue);
                }
                EscBacktrackAction::OpenOverlay => {
                    loop_state.esc_backtrack_primed = false;
                    chat_widget.clear_esc_backtrack_hint();
                    loop_state.overlay.open_transcript(tui, chat_widget)?;
                    if let Some(transcript) = loop_state.overlay.transcript_mut() {
                        transcript.begin_backtrack_preview();
                    }
                    return Ok(LoopAction::Continue);
                }
                EscBacktrackAction::ClearHint => {
                    loop_state.esc_backtrack_primed = false;
                    chat_widget.clear_esc_backtrack_hint();
                }
                EscBacktrackAction::Noop => {}
            }
            chat_widget.handle_key_event(key);
        }
        TuiEvent::Paste(pasted) => {
            // Many terminals convert newlines to \r when pasting (e.g., iTerm2),
            // but tui-textarea expects \n. Normalize CR to LF.
            // [tui-textarea]: <https://github.com/rhysd/tui-textarea/blob/4d18622eeac13b309e0ff6a55a46ac6706da68cf/src/textarea.rs#L782-L783>
            // [iTerm2]: <https://github.com/gnachman/iTerm2/blob/5d0c0d9f68523cbd0494dad5422998964a2ecd8d/sources/iTermPasteHelper.m#L206-L216>
            let pasted = pasted.replace("\r", "\n");
            chat_widget.handle_paste(pasted);
        }
    }

    Ok(LoopAction::Continue)
}

fn handle_ctrl_c_key(loop_state: &mut InteractiveLoopState, now: Instant) -> CtrlCKeyAction {
    if loop_state.busy {
        loop_state.last_ctrl_c_at = None;
        return CtrlCKeyAction::PromptInterruptWithEsc;
    }

    if loop_state
        .last_ctrl_c_at
        .is_some_and(|last| now.duration_since(last) <= Duration::from_secs(2))
    {
        return CtrlCKeyAction::Exit;
    }

    loop_state.last_ctrl_c_at = Some(now);
    CtrlCKeyAction::PromptExitConfirmation
}

fn determine_esc_backtrack_action(
    key: crossterm::event::KeyEvent,
    esc_backtrack_primed: bool,
    is_normal_backtrack_mode: bool,
    composer_is_empty: bool,
) -> EscBacktrackAction {
    if !matches!(
        key.kind,
        crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat
    ) {
        return EscBacktrackAction::Noop;
    }
    if key.code == KeyCode::Esc && is_normal_backtrack_mode && composer_is_empty {
        return if esc_backtrack_primed {
            EscBacktrackAction::OpenOverlay
        } else {
            EscBacktrackAction::PrimeHint
        };
    }
    if key.code != KeyCode::Esc && esc_backtrack_primed {
        return EscBacktrackAction::ClearHint;
    }
    EscBacktrackAction::Noop
}

fn selected_transcript_backtrack_selection(
    transcript: &TranscriptOverlay,
) -> TranscriptBacktrackSelection {
    let Some(user_message) = transcript.selected_user_message() else {
        return TranscriptBacktrackSelection::NoSelection;
    };
    let Some(position) = transcript.selected_user_history_position() else {
        return TranscriptBacktrackSelection::NoSelection;
    };
    let Ok(user_turn_index) = u32::try_from(position) else {
        return TranscriptBacktrackSelection::NoSelection;
    };
    TranscriptBacktrackSelection::Selected {
        user_message,
        user_turn_index,
    }
}

fn handle_app_event(
    app_event: Option<AppEvent>,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    tui: &mut Tui,
    loop_state: &mut InteractiveLoopState,
    context: &AppCommandContext<'_, impl ModelCatalog>,
) -> Result<LoopAction> {
    let Some(app_event) = app_event else {
        return Ok(LoopAction::ClearAndExit);
    };

    if let AppEvent::Exit(mode) = &app_event {
        tracing::info!(?mode, "host received app exit event");
        return Ok(LoopAction::ClearAndExit);
    }

    if matches!(&app_event, AppEvent::OnboardingCompleted) {
        loop_state.onboarding_completed = true;
        return Ok(LoopAction::Continue);
    }

    if matches!(&app_event, AppEvent::Interrupt) {
        if loop_state.busy && chat_widget.request_interrupt() {
            worker.interrupt_active_work()?;
        }
        return Ok(LoopAction::Continue);
    }

    match &app_event {
        AppEvent::ReferenceSearchRequested { query } => {
            if let Err(error) = worker.reference_search_requested(query.clone()) {
                tracing::warn!(?error, "failed to request composer reference search");
            }
            return Ok(LoopAction::Continue);
        }
        AppEvent::ReferenceSearchCancelled => {
            if let Err(error) = worker.reference_search_cancelled() {
                tracing::warn!(?error, "failed to cancel composer reference search");
            }
            return Ok(LoopAction::Continue);
        }
        AppEvent::ReferenceSearchResults { .. } => {
            chat_widget.handle_app_event(app_event);
            return Ok(LoopAction::Continue);
        }
        AppEvent::OpenSubagentOverlay { session_id } => {
            loop_state
                .overlay
                .open_subagent_transcript(tui, chat_widget, *session_id)?;
            return Ok(LoopAction::Continue);
        }
        AppEvent::ReloadInlineTranscript => {
            tui.replace_inline_session_ui()?;
            chat_widget.handle_app_event(app_event);
            return Ok(LoopAction::Continue);
        }
        _ => {}
    }
    if let AppEvent::Command(command) = &app_event {
        chat_widget.handle_app_event(app_event.clone());
        // Commands that affect sessions, providers, or turns are forwarded to the worker.
        handle_app_command(command, worker, chat_widget, tui, loop_state, context)?;
        return Ok(LoopAction::Continue);
    }

    if let AppEvent::DiffResult(text) = app_event {
        loop_state.overlay.open_diff(tui, chat_widget, text)?;
        return Ok(LoopAction::Continue);
    }

    chat_widget.handle_app_event(app_event);

    Ok(LoopAction::Continue)
}
fn handle_worker_event(
    worker_event: Option<WorkerEvent>,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    loop_state: &mut InteractiveLoopState,
) -> Result<LoopAction> {
    let Some(worker_event) = worker_event else {
        chat_widget.set_status_message("Background worker stopped");
        return Ok(LoopAction::ClearAndExit);
    };

    match &worker_event {
        WorkerEvent::TurnFinished {
            turn_count: next_turn_count,
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            total_tokens: next_total_tokens,
            total_cache_read_tokens: next_total_cache_read_tokens,
            ..
        }
        | WorkerEvent::TurnFailed {
            turn_count: next_turn_count,
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            total_tokens: next_total_tokens,
            total_cache_read_tokens: next_total_cache_read_tokens,
            ..
        } => {
            loop_state.busy = false;
            loop_state.turn_count = *next_turn_count;
            loop_state.total_input_tokens = *next_total_input_tokens;
            loop_state.total_output_tokens = *next_total_output_tokens;
            loop_state.total_tokens = *next_total_tokens;
            loop_state.total_cache_read_tokens = *next_total_cache_read_tokens;
            loop_state.session_switch_pending = false;
        }
        WorkerEvent::InterruptFailed { .. } => {}
        WorkerEvent::TurnStarted { .. } => {
            loop_state.busy = true;
        }
        WorkerEvent::SessionActivated { session_id } => {
            loop_state.session_id = Some(*session_id);
        }
        // Streaming deltas are handled entirely within the ChatWidget
        WorkerEvent::ToolOutputDelta { .. } => {}
        WorkerEvent::CommandExecutionStarted { source, .. }
            if matches!(
                source,
                &devo_protocol::protocol::ExecCommandSource::UserShell
            ) =>
        {
            loop_state.busy = true;
        }
        WorkerEvent::CommandExecutionStarted { .. } => {}
        WorkerEvent::ShellCommandFinished { .. } => {
            loop_state.busy = false;
        }
        WorkerEvent::UsageUpdated {
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            total_tokens: next_total_tokens,
            total_cache_read_tokens: next_total_cache_read_tokens,
            ..
        } => {
            loop_state.total_input_tokens = *next_total_input_tokens;
            loop_state.total_output_tokens = *next_total_output_tokens;
            loop_state.total_tokens = *next_total_tokens;
            loop_state.total_cache_read_tokens = *next_total_cache_read_tokens;
        }
        WorkerEvent::ProviderValidationSucceeded { .. } => {
            if let Some(pending) = loop_state.pending_onboarding.as_ref() {
                let mut provider_vendor = onboarding_provider_vendor(
                    &pending.binding,
                    pending.base_url.as_deref(),
                    pending.api_key.as_deref(),
                );
                if pending.api_key.as_deref().is_none() {
                    provider_vendor.credential = pending.provider_credential_id.clone();
                }
                let model_binding = onboarding_provider_model_binding(
                    &pending.binding,
                    pending.base_url.as_deref(),
                );
                worker.upsert_provider_vendor(
                    provider_vendor,
                    Some(model_binding.clone()),
                    Some(model_binding.binding_id),
                    pending.api_key.clone(),
                )?;
            }
        }
        WorkerEvent::ProviderVendorUpserted { model_binding, .. } => {
            if let Some(pending) = loop_state.pending_onboarding.take() {
                let request_model = model_binding
                    .as_ref()
                    .map(|binding| binding.request_model.clone())
                    .unwrap_or_else(|| pending.binding.request_model.clone());
                worker.reconfigure_provider(
                    pending.binding.invocation_method,
                    request_model,
                    pending.base_url,
                    pending.api_key,
                )?;
            }
        }
        WorkerEvent::ProviderValidationFailed { .. }
        | WorkerEvent::ProviderVendorUpsertFailed { .. } => {
            loop_state.pending_onboarding = None;
        }
        WorkerEvent::SessionCompactionStarted => {
            loop_state.busy = true;
        }
        WorkerEvent::SessionCompacted {
            total_input_tokens: next_total_input_tokens,
            total_output_tokens: next_total_output_tokens,
            total_tokens: next_total_tokens,
            last_query_total_tokens: _,
            last_query_input_tokens: _,
            prompt_token_estimate: _,
        } => {
            loop_state.busy = false;
            loop_state.total_input_tokens = *next_total_input_tokens;
            loop_state.total_output_tokens = *next_total_output_tokens;
            loop_state.total_tokens = *next_total_tokens;
        }
        WorkerEvent::SessionCompactionFailed { .. } => {
            loop_state.busy = false;
        }
        WorkerEvent::SessionSwitched {
            session_id,
            total_input_tokens,
            total_output_tokens,
            total_tokens,
            total_cache_read_tokens,
            ..
        } => {
            loop_state.session_switch_pending = false;
            loop_state.session_id = devo_core::SessionId::try_from(session_id.as_str()).ok();
            loop_state.total_input_tokens = *total_input_tokens;
            loop_state.total_output_tokens = *total_output_tokens;
            loop_state.total_tokens = *total_tokens;
            loop_state.total_cache_read_tokens = *total_cache_read_tokens;
        }
        WorkerEvent::TextDelta(_)
        | WorkerEvent::TextItemStarted { .. }
        | WorkerEvent::TextItemDelta { .. }
        | WorkerEvent::TextItemCompleted { .. }
        | WorkerEvent::ProposedPlanStarted { .. }
        | WorkerEvent::ProposedPlanDelta { .. }
        | WorkerEvent::ProposedPlanCompleted { .. }
        | WorkerEvent::ReasoningDelta(_)
        | WorkerEvent::AssistantMessageCompleted(_)
        | WorkerEvent::ReasoningCompleted(_)
        | WorkerEvent::ToolCall { .. }
        | WorkerEvent::ToolCallDetails { .. }
        | WorkerEvent::ToolCallUpdated { .. }
        | WorkerEvent::ToolResult { .. }
        | WorkerEvent::ToolResultIo { .. }
        | WorkerEvent::PatchApplied { .. }
        | WorkerEvent::PatchAppliedIo { .. }
        | WorkerEvent::PlanUpdated { .. }
        | WorkerEvent::ProviderVendorsListed { .. }
        | WorkerEvent::SessionsListed { .. }
        | WorkerEvent::SubagentDiscovered { .. }
        | WorkerEvent::SubagentMonitor { .. }
        | WorkerEvent::SkillsListed { .. }
        | WorkerEvent::McpServersListed { .. }
        | WorkerEvent::McpToolsListed { .. }
        | WorkerEvent::McpServerEnabled { .. }
        | WorkerEvent::McpServerEnableFailed { .. }
        | WorkerEvent::AcpAvailableCommandsUpdated { .. }
        | WorkerEvent::AcpCurrentModeUpdated { .. }
        | WorkerEvent::AcpConfigOptionsUpdated { .. }
        | WorkerEvent::AcpUsageUpdated { .. }
        | WorkerEvent::ContextUsageUpdated { .. }
        | WorkerEvent::ReferenceSearchUpdated { .. }
        | WorkerEvent::NewSessionPrepared { .. }
        | WorkerEvent::SessionRenamed { .. }
        | WorkerEvent::SessionDeleted { .. }
        | WorkerEvent::SessionTitleUpdated { .. }
        | WorkerEvent::ContextCompactionCompleted { .. }
        | WorkerEvent::InputHistoryLoaded { .. }
        | WorkerEvent::QueueUpdated { .. }
        | WorkerEvent::ApprovalRequest { .. }
        | WorkerEvent::RequestUserInput { .. }
        | WorkerEvent::ApprovalDecision { .. }
        | WorkerEvent::SteerAccepted { .. }
        | WorkerEvent::ProviderRetryStatus { .. }
        | WorkerEvent::GoalStatusLoaded { .. }
        | WorkerEvent::GoalUpdated { .. }
        | WorkerEvent::GoalReplaceConfirmationRequested { .. }
        | WorkerEvent::GoalEditLoaded { .. }
        | WorkerEvent::GoalCleared { .. }
        | WorkerEvent::GoalOperationFailed { .. }
        | WorkerEvent::BtwStarted { .. }
        | WorkerEvent::BtwCompleted { .. }
        | WorkerEvent::BtwFailed { .. }
        | WorkerEvent::EffectiveContextWindowUpdated { .. } => {}
    }
    if matches!(&worker_event, WorkerEvent::SessionsListed { .. }) {
        loop_state.resume_browser_pending = false;
    }
    if loop_state.resume_browser_pending && matches!(&worker_event, WorkerEvent::TurnFailed { .. })
    {
        loop_state.resume_browser_pending = false;
    }
    let session_switched = matches!(&worker_event, WorkerEvent::SessionSwitched { .. });
    let turn_failed = matches!(&worker_event, WorkerEvent::TurnFailed { .. });
    chat_widget.handle_worker_event(worker_event);
    if session_switched {
        if let Some(user_message) = loop_state.pending_backtrack_restore.take() {
            chat_widget.restore_user_message_to_composer(user_message);
        }
    } else if turn_failed {
        loop_state.pending_backtrack_restore = None;
    }

    Ok(LoopAction::Continue)
}

fn handle_app_command(
    command: &AppCommand,
    worker: &QueryWorkerHandle,
    chat_widget: &mut ChatWidget,
    tui: &mut Tui,
    loop_state: &mut InteractiveLoopState,
    context: &AppCommandContext<'_, impl ModelCatalog>,
) -> Result<()> {
    match command {
        AppCommand::UserTurn {
            input,
            model,
            model_binding_id,
            reasoning_effort_selection,
            approval_policy,
            collaboration_mode,
            ..
        } => {
            if let Some(model) = model {
                worker.set_model_selection(model.clone(), model_binding_id.clone())?;
            }
            worker.set_reasoning_effort(reasoning_effort_selection.clone())?;
            worker.submit_input_with_collaboration_mode(
                input.clone(),
                approval_policy.clone(),
                *collaboration_mode,
            )?;
        }
        AppCommand::ExecuteShellCommand { command } => {
            worker.execute_shell_command(command.clone())?;
        }
        AppCommand::SubmitShellInput { command } => {
            worker.submit_shell_input(command.clone())?;
        }
        AppCommand::QueuePush { input } => {
            worker.queue_push(input.clone())?;
        }
        AppCommand::QueueSteer {
            queue_item_id,
            expected_turn_id,
        } => {
            worker.queue_steer(queue_item_id.clone(), *expected_turn_id)?;
        }
        AppCommand::QueueRemove { queue_item_id } => {
            worker.queue_remove(queue_item_id.clone())?;
        }
        AppCommand::QueueUpdate {
            queue_item_id,
            input,
        } => {
            worker.queue_update(queue_item_id.clone(), input.clone())?;
        }
        AppCommand::RunBtwQuestion { question } => {
            worker.run_btw_question(question.clone())?;
        }
        AppCommand::ApprovalRespond {
            session_id,
            turn_id,
            approval_id,
            decision,
            scope,
        } => {
            worker.approval_respond(
                *session_id,
                *turn_id,
                approval_id.clone(),
                decision.clone(),
                scope.clone(),
            )?;
        }
        AppCommand::RequestUserInputRespond {
            session_id,
            turn_id,
            request_id,
            response,
        } => {
            worker.request_user_input_respond(
                *session_id,
                *turn_id,
                request_id.clone(),
                response.clone(),
            )?;
        }
        AppCommand::UpdatePermissions {
            preset,
            persist_scope,
        } => {
            worker.update_permissions(*preset, *persist_scope)?;
            let implied_sandbox = match preset {
                PermissionPreset::FullAccess => "off",
                PermissionPreset::Default | PermissionPreset::AutoReview => "workspace",
            };
            worker.update_sandbox_profile(implied_sandbox.to_string())?;
            // Sync implied sandbox locally without a user-visible notice (permissions
            // message below is enough).
            chat_widget.note_sandbox_profile_updated(implied_sandbox.to_string());
            if *persist_scope == PersistScope::Default {
                save_project_permission_preset(context.project_config_key, *preset)?;
                save_project_sandbox_profile(context.project_config_key, implied_sandbox)?;
            }
            chat_widget.note_permissions_updated(*preset);
        }
        AppCommand::UpdateEffectiveContextWindow {
            effective_context_window,
        } => {
            crate::onboarding::save_compaction_token_limit(*effective_context_window)?;
            chat_widget.note_effective_context_window_updated(*effective_context_window);
            worker.update_effective_context_window(*effective_context_window)?;
        }
        AppCommand::UpdateSandboxProfile { profile } => {
            worker.update_sandbox_profile(profile.clone())?;
            save_project_sandbox_profile(context.project_config_key, profile)?;
            // No transcript notice: sandbox is an implementation detail of permissions /
            // explicit picks; settings hub reflects the change silently.
            chat_widget.note_sandbox_profile_updated(profile.clone());
        }
        AppCommand::OverrideTurnContext {
            model,
            reasoning_effort_selection,
            persist_scope,
            ..
        } => {
            if let Some(model) = model {
                worker.set_model_selection_with_scope(model.clone(), None, *persist_scope)?;
                if *persist_scope == PersistScope::Default {
                    let provider = context
                        .model_catalog
                        .get(model)
                        .map(Model::provider_wire_api)
                        .unwrap_or(context.default_provider);
                    save_last_used_model(/*wire_api*/ None, provider, model)?;
                }
            }
            if let Some(reasoning_effort_selection) = reasoning_effort_selection {
                worker.set_reasoning_effort(reasoning_effort_selection.clone())?;
                if *persist_scope == PersistScope::Default {
                    save_reasoning_effort_selection(reasoning_effort_selection.as_deref())?;
                }
            }
        }
        AppCommand::SetCollaborationMode {
            collaboration_mode,
            persist_scope,
        } => {
            if *persist_scope == PersistScope::Default {
                save_default_collaboration_mode(*collaboration_mode)?;
            }
            worker.set_collaboration_mode(*collaboration_mode, *persist_scope)?;
            chat_widget.apply_collaboration_mode(*collaboration_mode, *persist_scope);
        }
        AppCommand::RunUserShellCommand { command } => {
            if command == "session list" {
                tui.enter_alt_screen()?;
                if let Err(error) = worker.list_sessions() {
                    let _ = tui.leave_alt_screen();
                    return Err(error);
                }
                loop_state.resume_browser_pending = true;
                chat_widget.set_status_message("Loading sessions");
            } else if command == "provider list" {
                worker.list_provider_vendors()?;
            } else if command == "skills list" {
                worker.list_skills()?;
                chat_widget.set_status_message("Loading skills");
            } else if command == "mcp list" {
                match find_devo_home()
                    .map_err(anyhow::Error::from)
                    .and_then(|config_home| {
                        FileSystemAppConfigLoader::new(config_home)
                            .load(Some(context.cwd))
                            .map_err(anyhow::Error::from)
                    }) {
                    Ok(app_config) => {
                        let body = crate::mcp_servers::render_mcp_servers_markdown(
                            &app_config.mcp_runtime,
                        );
                        chat_widget
                            .add_padded_markdown_history(MCP_SERVERS_TRANSCRIPT_TITLE, &body);
                        chat_widget.set_status_message("MCP servers loaded");
                    }
                    Err(error) => {
                        chat_widget.add_to_history(crate::history_cell::new_error_event_with_hint(
                            format!("Failed to load MCP server list: {error}"),
                            Some("mcp list failed".to_string()),
                        ));
                        chat_widget.set_status_message("Failed to load MCP servers");
                    }
                }
            } else if command == "session new" {
                worker.start_new_session()?;
            } else if let Some((onboarding_action, payload)) = parse_onboarding_command(command) {
                if context.model_catalog.get(&payload.model_slug).is_none() {
                    chat_widget.set_status_message(format!(
                        "Unsupported model slug: {}",
                        payload.model_slug
                    ));
                    return Ok(());
                }
                let display_name = normalized_display_name(
                    context.model_catalog,
                    &payload.model_slug,
                    &payload.display_name,
                );
                let base_url = payload.base_url;
                let api_key = payload.api_key;
                let provider_credential_id = payload.provider_credential_id;
                let binding = OnboardingModelBinding {
                    model_slug: payload.model_slug,
                    request_model: payload.request_model,
                    display_name,
                    provider_id: payload.provider_id,
                    provider_name: payload.provider_name,
                    invocation_method: payload.invocation_method,
                    default_reasoning_effort: payload.default_reasoning_effort,
                };
                worker.list_provider_vendors()?;
                let mut provider_vendor =
                    onboarding_provider_vendor(&binding, base_url.as_deref(), api_key.as_deref());
                if api_key.as_deref().is_none() {
                    provider_vendor.credential = provider_credential_id.clone();
                }
                let model_binding =
                    onboarding_provider_model_binding(&binding, base_url.as_deref());
                let pending = PendingOnboarding {
                    binding,
                    base_url,
                    api_key,
                    provider_credential_id,
                };
                match onboarding_action {
                    OnboardingCommandAction::Validate => {
                        worker.validate_provider(
                            provider_vendor,
                            model_binding,
                            pending.api_key.clone(),
                        )?;
                        loop_state.pending_onboarding = Some(pending);
                        chat_widget.set_status_message("Validating provider");
                    }
                    OnboardingCommandAction::SkipValidation => {
                        let default_model_binding = Some(model_binding.binding_id.clone());
                        worker.upsert_provider_vendor(
                            provider_vendor,
                            Some(model_binding),
                            default_model_binding,
                            pending.api_key.clone(),
                        )?;
                        loop_state.pending_onboarding = Some(pending);
                        chat_widget.set_status_message("Adding provider without validation");
                    }
                }
            } else {
                chat_widget.set_status_message(format!("Unsupported command: {}", command));
            }
        }
        AppCommand::Compact => {
            worker.compact_session()?;
        }
        AppCommand::RenameSession { title } => {
            worker.rename_session(title.clone())?;
        }
        AppCommand::DeleteSession { session_id } => {
            worker.delete_session(*session_id)?;
        }
        AppCommand::ShowGoal => {
            worker.show_goal()?;
        }
        AppCommand::EditGoal => {
            worker.edit_goal()?;
        }
        AppCommand::SetGoalObjective { objective, mode } => {
            worker.set_goal_objective(objective.clone(), *mode)?;
        }
        AppCommand::SetGoalStatus { status } => {
            worker.set_goal_status(*status)?;
        }
        AppCommand::ClearGoal => {
            worker.clear_goal()?;
        }
        AppCommand::BrowseInputHistory { direction } => {
            worker.browse_input_history(*direction)?;
        }
        AppCommand::SwitchSession { session_id } => {
            if tui.is_alt_screen_active() {
                tui.leave_alt_screen()?;
            }
            tracing::trace!(session_id = ?session_id, "switch session requested");
            loop_state.session_switch_pending = true;
            tui.replace_inline_session_ui()?;
            worker.switch_session(*session_id)?;
        }
        AppCommand::RollbackToUserTurn { user_turn_index } => {
            loop_state.session_switch_pending = true;
            tui.replace_inline_session_ui()?;
            worker.rollback_to_user_turn(*user_turn_index)?;
        }
        AppCommand::ForkAtUserTurn { user_turn_index } => {
            loop_state.session_switch_pending = true;
            tui.replace_inline_session_ui()?;
            worker.fork_at_user_turn(*user_turn_index)?;
        }
        AppCommand::ListMcpServers => {
            worker.list_mcp_servers()?;
            chat_widget.set_status_message("Loading MCP servers");
        }
        AppCommand::ListMcpTools { name } => {
            worker.list_mcp_tools(name.clone())?;
            chat_widget.set_status_message(format!("Loading tools · {name}"));
        }
        AppCommand::SetMcpServerEnabled { name, enabled } => {
            chat_widget.set_mcp_reopen_detail(Some(name.clone()));
            match worker.set_mcp_server_enabled(name.clone(), *enabled) {
                Ok(()) => {
                    let action = if *enabled { "enabled" } else { "disabled" };
                    chat_widget.set_status_message(format!("Updating MCP `{name}` · {action}"));
                }
                Err(error) => {
                    chat_widget.set_mcp_reopen_detail(None);
                    chat_widget.add_to_history(crate::history_cell::new_error_event_with_hint(
                        format!("Failed to update MCP server `{name}`: {error}"),
                        Some("mcp enable/disable failed".to_string()),
                    ));
                    chat_widget.set_status_message(format!("Failed to update MCP `{name}`"));
                }
            }
        }
        AppCommand::SetSkillEnabled {
            path,
            enabled,
            name,
        } => {
            chat_widget.set_skills_reopen_detail(Some(name.clone()));
            match worker.set_skill_enabled(path.clone(), *enabled) {
                Ok(()) => {
                    let action = if *enabled { "enabled" } else { "disabled" };
                    chat_widget.set_status_message(format!("Skill `{name}` {action}"));
                }
                Err(error) => {
                    chat_widget.set_skills_reopen_detail(None);
                    chat_widget.add_to_history(crate::history_cell::new_error_event_with_hint(
                        format!("Failed to update skill `{name}`: {error}"),
                        Some("skills set_enabled failed".to_string()),
                    ));
                    chat_widget.set_status_message(format!("Failed to update skill `{name}`"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
    use ratatui::text::Line;

    fn transcript_overlay_with_two_users() -> TranscriptOverlay {
        let crate::pager_overlay::Overlay::Transcript(overlay) =
            crate::pager_overlay::Overlay::new_transcript(
                vec![
                    crate::chatwidget::TranscriptOverlayCell {
                        lines: vec![Line::from("assistant 1")],
                        is_stream_continuation: false,
                        user_message: None,
                        is_selected_user: false,
                    },
                    crate::chatwidget::TranscriptOverlayCell {
                        lines: vec![Line::from("user 1")],
                        is_stream_continuation: false,
                        user_message: Some(UserMessage::from("user 1")),
                        is_selected_user: false,
                    },
                    crate::chatwidget::TranscriptOverlayCell {
                        lines: vec![Line::from("assistant 2")],
                        is_stream_continuation: false,
                        user_message: None,
                        is_selected_user: false,
                    },
                    crate::chatwidget::TranscriptOverlayCell {
                        lines: vec![Line::from("user 2")],
                        is_stream_continuation: false,
                        user_message: Some(UserMessage::from("user 2")),
                        is_selected_user: false,
                    },
                ],
                /*width*/ 80,
            )
        else {
            unreachable!("new_transcript should create transcript overlay");
        };
        *overlay
    }

    #[test]
    fn ctrl_c_while_busy_prompts_for_esc_without_arming_exit() {
        let mut loop_state = InteractiveLoopState {
            busy: true,
            last_ctrl_c_at: Some(Instant::now()),
            ..InteractiveLoopState::default()
        };

        let action = handle_ctrl_c_key(&mut loop_state, Instant::now());

        assert_eq!(CtrlCKeyAction::PromptInterruptWithEsc, action);
        assert_eq!(None, loop_state.last_ctrl_c_at);
    }

    #[test]
    fn ctrl_c_when_idle_requires_second_press_to_exit() {
        let now = Instant::now();
        let mut loop_state = InteractiveLoopState::default();

        let first = handle_ctrl_c_key(&mut loop_state, now);
        let second = handle_ctrl_c_key(&mut loop_state, now + Duration::from_secs(1));

        assert_eq!(CtrlCKeyAction::PromptExitConfirmation, first);
        assert_eq!(CtrlCKeyAction::Exit, second);
    }

    #[test]
    fn esc_backtrack_requires_second_press_to_open_overlay() {
        let esc_press = crossterm::event::KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        let esc_release = crossterm::event::KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        };

        assert_eq!(
            determine_esc_backtrack_action(
                esc_press, false, /*is_normal_backtrack_mode*/ true,
                /*composer_is_empty*/ true,
            ),
            EscBacktrackAction::PrimeHint
        );
        assert_eq!(
            determine_esc_backtrack_action(
                esc_release,
                true,
                /*is_normal_backtrack_mode*/ true,
                /*composer_is_empty*/ true,
            ),
            EscBacktrackAction::Noop
        );
        assert_eq!(
            determine_esc_backtrack_action(
                esc_press, true, /*is_normal_backtrack_mode*/ true,
                /*composer_is_empty*/ true,
            ),
            EscBacktrackAction::OpenOverlay
        );
    }

    #[test]
    fn transcript_backtrack_selection_is_noop_without_selected_user() {
        let overlay = transcript_overlay_with_two_users();

        assert_eq!(
            selected_transcript_backtrack_selection(&overlay),
            TranscriptBacktrackSelection::NoSelection
        );
    }

    #[test]
    fn transcript_backtrack_selection_targets_latest_user_turn() {
        let mut overlay = transcript_overlay_with_two_users();

        overlay.begin_backtrack_preview();

        assert_eq!(
            selected_transcript_backtrack_selection(&overlay),
            TranscriptBacktrackSelection::Selected {
                user_message: UserMessage::from("user 2"),
                user_turn_index: 1,
            }
        );
    }

    #[test]
    fn transcript_backtrack_selection_targets_older_user_turn() {
        let mut overlay = transcript_overlay_with_two_users();

        overlay.begin_backtrack_preview();
        overlay.select_prev_user();

        assert_eq!(
            selected_transcript_backtrack_selection(&overlay),
            TranscriptBacktrackSelection::Selected {
                user_message: UserMessage::from("user 1"),
                user_turn_index: 0,
            }
        );
    }

    #[test]
    fn session_activated_updates_loop_state_session_id() {
        let session_id = devo_core::SessionId::new();
        let mut loop_state = InteractiveLoopState::default();
        let worker_event = WorkerEvent::SessionActivated { session_id };

        match &worker_event {
            WorkerEvent::SessionActivated { session_id } => {
                loop_state.session_id = Some(*session_id);
            }
            _ => unreachable!(),
        }

        assert_eq!(loop_state.session_id, Some(session_id));
    }
}
