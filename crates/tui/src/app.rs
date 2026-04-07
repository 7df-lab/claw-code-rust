use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Result;
use clawcr_core::{BuiltinModelCatalog, ModelCatalog, SessionId};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::{
    events::{ModelListEntry, SessionListEntry, TranscriptItem, TranscriptItemKind, WorkerEvent},
    input::InputBuffer,
    paste_burst::PasteBurst,
    render,
    slash::{matching_slash_commands, SlashCommandSpec},
    terminal::ManagedTerminal,
    worker::{QueryWorkerConfig, QueryWorkerHandle},
};

/// Summary returned when the interactive TUI exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExit {
    /// Total turns completed in the session.
    pub turn_count: usize,
    /// Total input tokens accumulated in the session.
    pub total_input_tokens: usize,
    /// Total output tokens accumulated in the session.
    pub total_output_tokens: usize,
}

/// Temporary auxiliary panel rendered below the composer for non-transcript information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuxPanel {
    /// Short title shown above the panel body.
    pub(crate) title: String,
    /// Structured panel content rendered below the composer.
    pub(crate) content: AuxPanelContent,
}

/// One supported content shape for the temporary auxiliary bottom panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuxPanelContent {
    /// Plain informational text for commands like `/model` and `/status`.
    Text(String),
    /// Selectable session list shown after `/sessions`.
    SessionList(Vec<SessionListEntry>),
    /// Selectable model list shown after `/model` or onboarding.
    ModelList(Vec<ModelListEntry>),
}

/// In-memory application state for the interactive terminal UI.
pub(crate) struct TuiApp {
    /// Model identifier shown in the header.
    pub(crate) model: String,
    /// Current working directory shown in the header.
    pub(crate) cwd: PathBuf,
    /// Scrollable chat history pane.
    pub(crate) transcript: Vec<TranscriptItem>,
    /// Current composer buffer.
    pub(crate) input: InputBuffer,
    /// Current status bar text.
    pub(crate) status_message: String,
    /// Whether the model is currently producing output.
    pub(crate) busy: bool,
    /// Current spinner frame index.
    pub(crate) spinner_index: usize,
    /// Manual transcript scroll offset when follow mode is disabled.
    pub(crate) scroll: u16,
    /// Whether the transcript should stay pinned to the latest output.
    pub(crate) follow_output: bool,
    /// Total turns completed in the session.
    pub(crate) turn_count: usize,
    /// Total input tokens accumulated in the session.
    pub(crate) total_input_tokens: usize,
    /// Total output tokens accumulated in the session.
    pub(crate) total_output_tokens: usize,
    /// Currently selected slash-command suggestion row.
    pub(crate) slash_selection: usize,
    /// Temporary auxiliary panel rendered below the composer, when visible.
    pub(crate) aux_panel: Option<AuxPanel>,
    /// Selected session row when the session picker panel is visible.
    pub(crate) aux_panel_selection: usize,
    /// Index of the current turn status line rendered below the latest user message.
    pending_status_index: Option<usize>,
    /// Index of the assistant transcript item currently receiving streamed text.
    pending_assistant_index: Option<usize>,
    /// Background query worker owned by the UI.
    worker: QueryWorkerHandle,
    /// Built-in model catalog used for onboarding and model selection.
    model_catalog: BuiltinModelCatalog,
    /// Whether the app should open the model picker on startup.
    show_model_onboarding: bool,
    /// Whether onboarding completion has already been announced.
    onboarding_announced: bool,
    /// Timestamp of the most recent Ctrl+C press used for interrupt/exit confirmation.
    last_ctrl_c_at: Option<Instant>,
    /// Buffered rapid keypresses that should be applied as one pasted string.
    paste_burst: PasteBurst,
    /// Whether the app should exit after the current loop iteration.
    should_quit: bool,
}

/// Immutable configuration used to launch the interactive terminal UI.
pub struct InteractiveTuiConfig {
    /// Model identifier used for requests and shown in the header.
    pub model: String,
    /// Working directory shown in the header and passed to the session.
    pub cwd: PathBuf,
    /// Environment overrides applied to the spawned stdio server process.
    pub server_env: Vec<(String, String)>,
    /// Optional prompt submitted immediately after the UI opens.
    pub startup_prompt: Option<String>,
    /// Built-in model catalog used for onboarding and model selection.
    pub model_catalog: BuiltinModelCatalog,
    /// Whether to open the model picker on startup.
    pub show_model_onboarding: bool,
}

impl TuiApp {
    /// Runs the full interactive UI until the user exits.
    pub(crate) async fn run(config: InteractiveTuiConfig) -> Result<AppExit> {
        let startup_prompt = config.startup_prompt.clone();
        let worker = QueryWorkerHandle::spawn(QueryWorkerConfig {
            model: config.model.clone(),
            cwd: config.cwd.clone(),
            server_env: config.server_env,
        });

        let mut app = Self {
            model: config.model,
            cwd: config.cwd,
            transcript: Vec::new(),
            input: InputBuffer::new(),
            status_message: "Ready".to_string(),
            busy: false,
            spinner_index: 0,
            scroll: 0,
            follow_output: true,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            slash_selection: 0,
            aux_panel: None,
            pending_status_index: None,
            pending_assistant_index: None,
            worker,
            model_catalog: config.model_catalog,
            show_model_onboarding: config.show_model_onboarding,
            onboarding_announced: false,
            aux_panel_selection: 0,
            last_ctrl_c_at: None,
            paste_burst: PasteBurst::default(),
            should_quit: false,
        };

        if app.show_model_onboarding {
            app.show_model_panel();
            app.status_message = "Choose a builtin model to start".to_string();
        }

        if let Some(prompt) = startup_prompt {
            app.submit_prompt(prompt)?;
        }

        let mut terminal = ManagedTerminal::new()?;
        let mut event_stream = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(80));
        let mut needs_redraw = true;

        loop {
            if needs_redraw {
                terminal
                    .terminal_mut()
                    .draw(|frame| render::draw(frame, &app))?;
                needs_redraw = false;
            }

            if app.should_quit {
                break;
            }

            tokio::select! {
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            app.handle_terminal_event(event, terminal.area())?;
                            needs_redraw = true;
                        }
                        Some(Err(error)) => {
                            app.push_item(
                                TranscriptItemKind::Error,
                                "Terminal error",
                                error.to_string(),
                            );
                            app.status_message = "Terminal input error".to_string();
                            needs_redraw = true;
                        }
                        None => break,
                    }
                }
                maybe_event = app.worker.event_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            app.handle_worker_event(event);
                            needs_redraw = true;
                        }
                        None => {
                            app.status_message = "Background worker stopped".to_string();
                            break;
                        }
                    }
                }
                _ = tick.tick() => {
                    let mut redraw = app.advance_transcript_folds(Instant::now());
                    if app.busy {
                        app.spinner_index = app.spinner_index.wrapping_add(1);
                        redraw = true;
                    }
                    if app.flush_pending_paste_burst(false) {
                        redraw = true;
                    }
                    if redraw {
                        needs_redraw = true;
                    }
                }
            }
        }

        app.worker.shutdown().await?;
        Ok(AppExit {
            turn_count: app.turn_count,
            total_input_tokens: app.total_input_tokens,
            total_output_tokens: app.total_output_tokens,
        })
    }

    fn transcript_area(&self, full_area: Rect) -> Rect {
        let content_area = render::centered_content_area(full_area);
        let composer_height = render::composer_height(self, content_area);
        let transcript_height = render::transcript_height(self, content_area);
        let [transcript_area, _, _, _] = Layout::vertical([
            Constraint::Length(transcript_height),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .areas(content_area);
        transcript_area
    }

    fn handle_terminal_event(&mut self, event: Event, terminal_area: Rect) -> Result<()> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if !matches!(key.code, KeyCode::Char(_) | KeyCode::Enter) {
                    self.flush_pending_paste_burst(true);
                }
                self.handle_key(key, terminal_area)
            }
            Event::Paste(text) => {
                self.flush_pending_paste_burst(true);
                self.input.insert_str(&text);
                self.reset_slash_selection();
                self.aux_panel = None;
                self.aux_panel_selection = 0;
            }
            Event::Resize(_, _) => {}
            Event::Mouse(mouse) => {
                self.flush_pending_paste_burst(true);
                use crossterm::event::MouseEventKind;
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if self.follow_output {
                            self.scroll =
                                render::get_max_scroll(self, self.transcript_area(terminal_area));
                            self.follow_output = false;
                        }
                        self.scroll = self.scroll.saturating_add(1);
                    }
                    MouseEventKind::ScrollUp => {
                        if self.follow_output {
                            self.scroll =
                                render::get_max_scroll(self, self.transcript_area(terminal_area));
                            self.follow_output = false;
                        }
                        self.scroll = self.scroll.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal_area: Rect) {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.handle_ctrl_c();
            }
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.transcript.clear();
                self.pending_status_index = None;
                self.pending_assistant_index = None;
                self.status_message = "Transcript cleared".to_string();
            }
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.flush_pending_paste_burst(true);
                self.input.insert_char('\n');
            }
            KeyCode::Enter if !self.busy => {
                if self.paste_burst.push_newline(Instant::now()) {
                    return;
                }
                self.flush_pending_paste_burst(true);
                if self.has_slash_suggestions() && self.try_apply_slash_suggestion() {
                    let prompt = self.input.take();
                    if let Err(error) = self.handle_submission(prompt) {
                        self.push_item(
                            TranscriptItemKind::Error,
                            "Submit failed",
                            error.to_string(),
                        );
                        self.status_message = "Failed to submit prompt".to_string();
                    }
                    return;
                }
                if self.try_accept_aux_panel_selection() {
                    return;
                }
                let prompt = self.input.take();
                if let Err(error) = self.handle_submission(prompt) {
                    self.push_item(
                        TranscriptItemKind::Error,
                        "Submit failed",
                        error.to_string(),
                    );
                    self.status_message = "Failed to submit prompt".to_string();
                }
            }
            KeyCode::Backspace => {
                self.flush_pending_paste_burst(true);
                self.input.backspace();
                self.reset_slash_selection();
                self.aux_panel = None;
                self.aux_panel_selection = 0;
            }
            KeyCode::Delete => {
                self.flush_pending_paste_burst(true);
                self.input.delete();
                self.reset_slash_selection();
                self.aux_panel = None;
                self.aux_panel_selection = 0;
            }
            KeyCode::Tab if self.try_apply_slash_suggestion() => {}
            KeyCode::Left => {
                self.flush_pending_paste_burst(true);
                self.input.move_left();
            }
            KeyCode::Right => {
                self.flush_pending_paste_burst(true);
                self.input.move_right();
            }
            KeyCode::Home => {
                self.flush_pending_paste_burst(true);
                self.input.move_home();
                self.scroll = 0;
                self.follow_output = false;
            }
            KeyCode::End => {
                self.flush_pending_paste_burst(true);
                self.input.move_end();
                self.follow_output = true;
            }
            KeyCode::Up => {
                if self.has_selectable_aux_panel() {
                    self.move_aux_panel_selection(-1);
                } else if self.has_slash_suggestions() {
                    self.move_slash_selection(-1);
                } else {
                    if self.follow_output {
                        self.scroll =
                            render::get_max_scroll(self, self.transcript_area(terminal_area));
                        self.follow_output = false;
                    }
                    self.scroll = self.scroll.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.has_selectable_aux_panel() {
                    self.move_aux_panel_selection(1);
                } else if self.has_slash_suggestions() {
                    self.move_slash_selection(1);
                } else {
                    if self.follow_output {
                        self.scroll =
                            render::get_max_scroll(self, self.transcript_area(terminal_area));
                        self.follow_output = false;
                    }
                    self.scroll = self.scroll.saturating_add(1);
                }
            }
            KeyCode::PageUp => {
                if self.follow_output {
                    self.scroll = render::get_max_scroll(self, self.transcript_area(terminal_area));
                    self.follow_output = false;
                }
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                if self.follow_output {
                    self.scroll = render::get_max_scroll(self, self.transcript_area(terminal_area));
                    self.follow_output = false;
                }
                self.scroll = self.scroll.saturating_add(10);
            }
            KeyCode::Esc => {
                self.flush_pending_paste_burst(true);
                self.input.clear();
                self.reset_slash_selection();
                self.aux_panel = None;
                self.aux_panel_selection = 0;
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.paste_burst.push_char(ch, Instant::now()) {
                    return;
                }
                self.input.insert_char(ch);
                self.reset_slash_selection();
                self.aux_panel = None;
                self.aux_panel_selection = 0;
            }
            _ => {}
        }
    }

    fn flush_pending_paste_burst(&mut self, force: bool) -> bool {
        let Some(text) = self.paste_burst.take_if_due(Instant::now(), force) else {
            return false;
        };
        self.input.insert_str(&text);
        self.reset_slash_selection();
        self.aux_panel = None;
        self.aux_panel_selection = 0;
        true
    }

    fn handle_ctrl_c(&mut self) {
        const EXIT_CONFIRM_WINDOW: Duration = Duration::from_secs(2);

        let now = Instant::now();
        if self
            .last_ctrl_c_at
            .is_some_and(|previous| now.duration_since(previous) <= EXIT_CONFIRM_WINDOW)
        {
            self.should_quit = true;
            self.status_message = "Exiting".to_string();
            return;
        }

        self.last_ctrl_c_at = Some(now);
        if self.busy {
            if let Err(error) = self.worker.interrupt_turn() {
                self.push_item(
                    TranscriptItemKind::Error,
                    "Interrupt failed",
                    error.to_string(),
                );
                self.status_message = "Failed to interrupt active turn".to_string();
                return;
            }
            self.status_message =
                "Interrupt requested. Press Ctrl+C again within 2s to exit.".to_string();
        } else {
            self.status_message = "Press Ctrl+C again within 2s to exit.".to_string();
        }
    }

    fn handle_submission(&mut self, prompt: String) -> Result<()> {
        if prompt.trim_start().starts_with('/') {
            return self.handle_slash_command(prompt);
        }
        self.submit_prompt(prompt)
    }

    fn submit_prompt(&mut self, prompt: String) -> Result<()> {
        if self.input.is_blank() && prompt.trim().is_empty() {
            return Ok(());
        }

        self.push_item(TranscriptItemKind::User, "You", prompt.clone());
        self.pending_status_index =
            Some(self.push_item(TranscriptItemKind::System, "Thinking", ""));
        self.follow_output = true;
        self.busy = true;
        self.reset_slash_selection();
        self.aux_panel = None;
        self.aux_panel_selection = 0;
        self.pending_assistant_index = None;
        self.status_message = "Waiting for model response".to_string();
        self.worker.submit_prompt(prompt)
    }

    fn show_aux_panel(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.aux_panel = Some(AuxPanel {
            title: title.into(),
            content: AuxPanelContent::Text(body.into()),
        });
        self.aux_panel_selection = 0;
    }

    fn show_session_panel(&mut self, sessions: Vec<SessionListEntry>) {
        self.aux_panel_selection = sessions
            .iter()
            .position(|session| session.is_active)
            .unwrap_or(0);
        self.aux_panel = Some(AuxPanel {
            title: "Sessions".to_string(),
            content: AuxPanelContent::SessionList(sessions),
        });
    }

    fn show_model_panel(&mut self) {
        let entries = self.model_picker_entries();
        self.aux_panel_selection = entries
            .iter()
            .position(|entry| entry.is_current)
            .unwrap_or(0);
        self.aux_panel = Some(AuxPanel {
            title: if self.show_model_onboarding {
                "✨".to_string()
            } else {
                "Models".to_string()
            },
            content: AuxPanelContent::ModelList(entries),
        });
    }

    fn model_picker_entries(&self) -> Vec<ModelListEntry> {
        let mut entries = Vec::new();

        for model in self.model_catalog.list_visible() {
            entries.push(ModelListEntry {
                slug: model.slug.clone(),
                display_name: model.display_name.clone(),
                description: model.description.clone(),
                is_current: model.slug == self.model,
                is_builtin: true,
            });
        }

        if !entries.iter().any(|entry| entry.slug == self.model) {
            entries.insert(
                0,
                ModelListEntry {
                    slug: self.model.clone(),
                    display_name: self.model.clone(),
                    description: Some("current model".to_string()),
                    is_current: true,
                    is_builtin: false,
                },
            );
        }

        if entries.is_empty() {
            entries.push(ModelListEntry {
                slug: self.model.clone(),
                display_name: self.model.clone(),
                description: Some("current model".to_string()),
                is_current: true,
                is_builtin: false,
            });
        }

        entries
    }

    fn set_model(&mut self, model: String) -> Result<()> {
        self.worker.set_model(model.clone())?;
        self.model = model;
        Ok(())
    }

    fn handle_slash_command(&mut self, prompt: String) -> Result<()> {
        let trimmed = prompt.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let argument = parts.next().map(str::trim).unwrap_or_default();

        match command {
            "/exit" => {
                self.push_item(
                    TranscriptItemKind::System,
                    "Command",
                    "Exiting interactive session",
                );
                self.status_message = "Exiting".to_string();
                self.should_quit = true;
                Ok(())
            }
            "/status" => {
                self.show_aux_panel(
                    "Status",
                    format!(
                        "turns: {}\nmodel: {}\ntokens: {} in / {} out\nbusy: {}",
                        self.turn_count,
                        self.model,
                        self.total_input_tokens,
                        self.total_output_tokens,
                        self.busy
                    ),
                );
                self.status_message = "Session status shown".to_string();
                Ok(())
            }
            "/sessions" => {
                self.worker.list_sessions()?;
                self.status_message = "Loading sessions".to_string();
                Ok(())
            }
            "/new" => {
                self.worker.start_new_session()?;
                self.aux_panel = None;
                self.aux_panel_selection = 0;
                self.status_message = "New session ready; send a prompt to start it".to_string();
                Ok(())
            }
            "/rename" => {
                if argument.is_empty() {
                    anyhow::bail!("usage: /rename <new title>");
                }
                self.worker.rename_session(argument.to_string())?;
                self.status_message = "Renaming current session".to_string();
                Ok(())
            }
            "/session" => {
                if argument.is_empty() || argument == "list" {
                    self.worker.list_sessions()?;
                    self.status_message = "Loading sessions".to_string();
                    return Ok(());
                }

                let mut session_parts = argument.splitn(2, char::is_whitespace);
                let subcommand = session_parts.next().unwrap_or_default();
                let rest = session_parts.next().map(str::trim).unwrap_or_default();

                match subcommand {
                    "new" => {
                        self.worker.start_new_session()?;
                        self.aux_panel = None;
                        self.aux_panel_selection = 0;
                        self.status_message =
                            "New session ready; send a prompt to start it".to_string();
                        Ok(())
                    }
                    "rename" => {
                        if rest.is_empty() {
                            anyhow::bail!("usage: /session rename <new title>");
                        }
                        self.worker.rename_session(rest.to_string())?;
                        self.status_message = "Renaming current session".to_string();
                        Ok(())
                    }
                    "switch" => {
                        if rest.is_empty() {
                            anyhow::bail!("usage: /session switch <session_id>");
                        }
                        let session_id = rest.parse::<SessionId>().map_err(|error| {
                            anyhow::anyhow!("invalid session id `{rest}`: {error}")
                        })?;
                        self.worker.switch_session(session_id)?;
                        self.status_message = format!("Switching to session {rest}");
                        Ok(())
                    }
                    _ => {
                        let session_id = argument.parse::<SessionId>().map_err(|error| {
                            anyhow::anyhow!("invalid session command `{argument}`: {error}")
                        })?;
                        self.worker.switch_session(session_id)?;
                        self.status_message = format!("Switching to session {argument}");
                        Ok(())
                    }
                }
            }
            "/model" => {
                if argument.is_empty() {
                    self.show_model_panel();
                    self.status_message = "Current model shown".to_string();
                    return Ok(());
                }

                self.set_model(argument.to_string())?;
                self.aux_panel = None;
                self.aux_panel_selection = 0;
                self.status_message = format!("Model set to {}", self.model);
                Ok(())
            }
            _ => self.submit_prompt(prompt),
        }
    }

    fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::TurnStarted => {
                self.busy = true;
                self.set_turn_status_line("Thinking");
                self.status_message = "Thinking".to_string();
                self.pending_assistant_index = None;
            }
            WorkerEvent::TextDelta(text) => {
                let index = self.ensure_assistant_item();
                self.transcript[index].body.push_str(&text);
                self.status_message = "Streaming response".to_string();
                if self.follow_output {
                    self.scroll = 0;
                }
            }
            WorkerEvent::ToolCall { summary, detail } => {
                self.pending_assistant_index = None;
                self.push_item(
                    TranscriptItemKind::ToolCall,
                    summary.clone(),
                    detail.as_deref().unwrap_or("").trim().to_string(),
                );
                if self.busy {
                    self.show_turn_status_line("Thinking");
                }
                self.status_message = format!("{summary}...");
            }
            WorkerEvent::ToolResult {
                preview,
                is_error,
                truncated: _,
            } => {
                let kind = if is_error {
                    TranscriptItemKind::Error
                } else {
                    TranscriptItemKind::ToolResult
                };
                let title = if is_error {
                    "Tool error"
                } else {
                    "Tool output"
                };
                let body = preview.trim().to_string();
                if kind == TranscriptItemKind::ToolResult {
                    self.transcript
                        .push(TranscriptItem::new(kind, title, body).with_tool_fold());
                    if self.follow_output {
                        self.scroll = 0;
                    }
                } else {
                    self.push_item(kind, title, body);
                }
                if self.busy {
                    self.show_turn_status_line("Thinking");
                }
                self.status_message = if is_error {
                    "Tool returned an error".to_string()
                } else {
                    "Tool completed".to_string()
                };
            }
            WorkerEvent::TurnFinished {
                stop_reason,
                turn_count,
                total_input_tokens,
                total_output_tokens,
            } => {
                self.busy = false;
                self.clear_turn_status_line();
                self.pending_assistant_index = None;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.last_ctrl_c_at = None;
                if stop_reason == "Interrupted" {
                    self.push_item(TranscriptItemKind::System, "Interrupted", "");
                } else {
                    self.push_item(TranscriptItemKind::System, "Complete", "");
                }
                self.status_message = format!("Turn completed ({stop_reason})");
            }
            WorkerEvent::TurnFailed {
                message,
                turn_count,
                total_input_tokens,
                total_output_tokens,
            } => {
                self.busy = false;
                self.clear_turn_status_line();
                self.pending_assistant_index = None;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.last_ctrl_c_at = None;
                self.push_item(TranscriptItemKind::Error, "Error", message);
                self.status_message = "Query failed".to_string();
            }
            WorkerEvent::SessionsListed { sessions } => {
                self.show_session_panel(sessions);
                self.status_message = "Sessions loaded".to_string();
            }
            WorkerEvent::NewSessionPrepared => {
                self.aux_panel = None;
                self.aux_panel_selection = 0;
                self.pending_status_index = None;
                self.pending_assistant_index = None;
                self.busy = false;
                self.transcript.clear();
                self.follow_output = true;
                self.scroll = 0;
                self.status_message = "New session ready; send a prompt to start it".to_string();
            }
            WorkerEvent::SessionSwitched {
                session_id,
                title,
                model,
                history_items,
                loaded_item_count,
            } => {
                if let Some(model) = model {
                    self.model = model;
                }
                self.aux_panel = None;
                self.aux_panel_selection = 0;
                self.pending_status_index = None;
                self.pending_assistant_index = None;
                self.busy = false;
                self.transcript = history_items;
                self.follow_output = true;
                self.scroll = 0;
                self.status_message = format!("Active session: {session_id}");
                if self.transcript.is_empty() {
                    self.push_item(
                        TranscriptItemKind::System,
                        "Session",
                        format!(
                            "switched to {}\ntitle: {}\nloaded items: {}",
                            session_id,
                            title.unwrap_or_else(|| "(untitled)".to_string()),
                            loaded_item_count
                        ),
                    );
                }
            }
            WorkerEvent::SessionRenamed { session_id, title } => {
                self.push_item(
                    TranscriptItemKind::System,
                    "Session",
                    format!("renamed {} to {}", session_id, title),
                );
                self.status_message = "Session renamed".to_string();
            }
            WorkerEvent::SessionTitleUpdated { session_id, title } => {
                if let Some(AuxPanel {
                    content: AuxPanelContent::SessionList(entries),
                    ..
                }) = self.aux_panel.as_mut()
                {
                    if let Some(entry) = entries
                        .iter_mut()
                        .find(|entry| entry.session_id.to_string() == session_id)
                    {
                        entry.title = title.clone();
                    }
                }
                self.status_message = format!("Session titled: {title}");
            }
        }
    }

    fn ensure_assistant_item(&mut self) -> usize {
        if let Some(index) = self.pending_assistant_index {
            return index;
        }

        self.transcript.push(TranscriptItem::new(
            TranscriptItemKind::Assistant,
            "Assistant",
            String::new(),
        ));
        let index = self.transcript.len() - 1;
        self.pending_assistant_index = Some(index);
        index
    }

    fn push_item(
        &mut self,
        kind: TranscriptItemKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> usize {
        self.transcript.push(TranscriptItem::new(kind, title, body));
        if self.follow_output {
            self.scroll = 0;
        }
        self.transcript.len() - 1
    }

    fn advance_transcript_folds(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for item in &mut self.transcript {
            if item.advance_fold(now) {
                changed = true;
            }
        }
        changed
    }

    fn set_turn_status_line(&mut self, title: impl Into<String>) {
        if let Some(index) = self.pending_status_index {
            if let Some(item) = self.transcript.get_mut(index) {
                item.title = title.into();
                item.body.clear();
            }
        }
    }

    fn show_turn_status_line(&mut self, title: impl Into<String>) {
        self.clear_turn_status_line();
        self.pending_status_index =
            Some(self.push_item(TranscriptItemKind::System, title.into(), ""));
    }

    fn clear_turn_status_line(&mut self) {
        if let Some(index) = self.pending_status_index.take() {
            if index < self.transcript.len() {
                self.transcript.remove(index);
            }
            if let Some(pending_assistant_index) = self.pending_assistant_index {
                if pending_assistant_index > index {
                    self.pending_assistant_index = Some(pending_assistant_index - 1);
                } else if pending_assistant_index == index {
                    self.pending_assistant_index = None;
                }
            }
        }
    }

    pub(crate) fn slash_suggestions(&self) -> Vec<SlashCommandSpec> {
        matching_slash_commands(self.input.text())
    }

    fn has_slash_suggestions(&self) -> bool {
        !self.slash_suggestions().is_empty()
    }

    pub(crate) fn has_selectable_aux_panel(&self) -> bool {
        matches!(
            self.aux_panel.as_ref().map(|panel| &panel.content),
            Some(AuxPanelContent::SessionList(_) | AuxPanelContent::ModelList(_))
        )
    }

    fn reset_slash_selection(&mut self) {
        self.slash_selection = 0;
    }

    fn move_slash_selection(&mut self, delta: isize) {
        let suggestions = self.slash_suggestions();
        if suggestions.is_empty() {
            self.slash_selection = 0;
            return;
        }
        let len = suggestions.len() as isize;
        let next = (self.slash_selection as isize + delta).clamp(0, len - 1);
        self.slash_selection = next as usize;
    }

    fn try_apply_slash_suggestion(&mut self) -> bool {
        let suggestions = self.slash_suggestions();
        if suggestions.is_empty() {
            return false;
        }
        let selected = suggestions[self.slash_selection.min(suggestions.len() - 1)];
        self.input.replace(selected.name);
        self.reset_slash_selection();
        true
    }

    fn move_aux_panel_selection(&mut self, delta: isize) {
        let len = self
            .aux_panel
            .as_ref()
            .map(|panel| match &panel.content {
                AuxPanelContent::SessionList(sessions) => sessions.len(),
                AuxPanelContent::ModelList(models) => models.len(),
                AuxPanelContent::Text(_) => 0,
            })
            .unwrap_or(0);
        if len == 0 {
            self.aux_panel_selection = 0;
            return;
        }

        let len = len as isize;
        let next = (self.aux_panel_selection as isize + delta).clamp(0, len - 1);
        self.aux_panel_selection = next as usize;
    }

    fn try_accept_aux_panel_selection(&mut self) -> bool {
        let Some(panel) = self.aux_panel.as_ref() else {
            return false;
        };
        if !self.input.is_blank() {
            return false;
        }

        match &panel.content {
            AuxPanelContent::SessionList(sessions) => {
                if sessions.is_empty() {
                    return false;
                }
                let selected =
                    sessions[self.aux_panel_selection.min(sessions.len() - 1)].session_id;
                if let Err(error) = self.worker.switch_session(selected) {
                    self.push_item(
                        TranscriptItemKind::Error,
                        "Switch failed",
                        error.to_string(),
                    );
                    self.status_message = "Failed to switch session".to_string();
                } else {
                    self.status_message = format!("Switching to session {selected}");
                }
                true
            }
            AuxPanelContent::ModelList(models) => {
                if models.is_empty() {
                    return false;
                }
                let selected = models[self.aux_panel_selection.min(models.len() - 1)]
                    .slug
                    .clone();
                if let Err(error) = self.set_model(selected.clone()) {
                    self.push_item(
                        TranscriptItemKind::Error,
                        "Model switch failed",
                        error.to_string(),
                    );
                    self.status_message = "Failed to switch model".to_string();
                } else {
                    self.aux_panel = None;
                    self.aux_panel_selection = 0;
                    self.status_message = format!("Model set to {selected}");
                    if self.show_model_onboarding && !self.onboarding_announced {
                        self.push_item(
                            TranscriptItemKind::System,
                            "Onboarding",
                            "Onboarding complete. Run `clawcr onboard` any time to revisit builtin models.",
                        );
                        self.onboarding_announced = true;
                        self.show_model_onboarding = false;
                    }
                }
                true
            }
            AuxPanelContent::Text(_) => false,
        }
    }
}

/// Runs the interactive alternate-screen terminal UI until the user exits.
pub async fn run_interactive_tui(config: InteractiveTuiConfig) -> Result<AppExit> {
    TuiApp::run(config).await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use clawcr_core::{BuiltinModelCatalog, SessionId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pretty_assertions::assert_eq;
    use ratatui::layout::Rect;

    use super::{AuxPanelContent, TuiApp};
    use crate::{
        events::{SessionListEntry, TranscriptItem, TranscriptItemKind, WorkerEvent},
        input::InputBuffer,
        render,
        worker::QueryWorkerHandle,
    };

    fn test_app() -> TuiApp {
        TuiApp {
            model: "test-model".to_string(),
            cwd: PathBuf::from("."),
            transcript: Vec::new(),
            input: InputBuffer::new(),
            status_message: "Ready".to_string(),
            busy: false,
            spinner_index: 0,
            scroll: 0,
            follow_output: true,
            turn_count: 3,
            total_input_tokens: 10,
            total_output_tokens: 20,
            slash_selection: 0,
            pending_status_index: None,
            pending_assistant_index: None,
            worker: QueryWorkerHandle::stub(),
            model_catalog: BuiltinModelCatalog::default(),
            show_model_onboarding: false,
            onboarding_announced: false,
            aux_panel: None,
            aux_panel_selection: 0,
            last_ctrl_c_at: None,
            paste_burst: crate::paste_burst::PasteBurst::default(),
            should_quit: false,
        }
    }

    #[tokio::test]
    async fn assistant_text_deltas_append_to_same_item() {
        let mut app = test_app();
        app.handle_worker_event(WorkerEvent::TextDelta("hel".to_string()));
        app.handle_worker_event(WorkerEvent::TextDelta("lo".to_string()));

        assert_eq!(app.transcript.len(), 1);
        assert_eq!(app.transcript[0].kind, TranscriptItemKind::Assistant);
        assert_eq!(app.transcript[0].body, "hello");
    }

    #[tokio::test]
    async fn tool_results_create_separate_items() {
        let mut app = test_app();
        app.handle_worker_event(WorkerEvent::ToolResult {
            preview: "done".to_string(),
            is_error: false,
            truncated: false,
        });

        assert_eq!(app.transcript.len(), 1);
        assert_eq!(app.transcript[0].kind, TranscriptItemKind::ToolResult);
        assert_eq!(app.transcript[0].body, "done");
    }

    #[tokio::test]
    async fn tool_result_fold_progresses_to_three_line_compact_state() {
        let mut item = TranscriptItem::new(
            TranscriptItemKind::ToolResult,
            "Tool output",
            (1..=12)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .with_tool_fold();

        let first = item.fold_next_at.expect("fold deadline");
        assert!(!item.advance_fold(Instant::now()));
        assert!(item.advance_fold(first));
        assert_eq!(item.fold_stage, 1);

        let second = item.fold_next_at.expect("second fold deadline");
        assert!(item.advance_fold(second));
        assert_eq!(item.fold_stage, 2);
        assert!(item.fold_next_at.is_none());
        assert!(!item.advance_fold(second));
    }

    #[tokio::test]
    async fn slash_status_shows_bottom_panel() {
        let mut app = test_app();

        app.handle_slash_command("/status".to_string())
            .expect("status command should succeed");

        assert!(app.transcript.is_empty());
        assert_eq!(
            app.aux_panel.as_ref().map(|panel| panel.title.as_str()),
            Some("Status")
        );
        assert!(app
            .aux_panel
            .as_ref()
            .is_some_and(|panel| matches!(&panel.content, AuxPanelContent::Text(body) if body.contains("turns: 3"))));
    }

    #[tokio::test]
    async fn slash_sessions_requests_listing() {
        let mut app = test_app();

        app.handle_slash_command("/sessions".to_string())
            .expect("sessions command should succeed");

        assert_eq!(app.status_message, "Loading sessions");
    }

    #[tokio::test]
    async fn slash_new_requests_new_session() {
        let mut app = test_app();

        app.handle_slash_command("/new".to_string())
            .expect("new command should succeed");

        assert_eq!(
            app.status_message,
            "New session ready; send a prompt to start it"
        );
    }

    #[tokio::test]
    async fn slash_model_shows_bottom_panel() {
        let mut app = test_app();

        app.handle_slash_command("/model".to_string())
            .expect("model command should succeed");

        assert!(app.transcript.is_empty());
        assert_eq!(
            app.aux_panel.as_ref().map(|panel| panel.title.as_str()),
            Some("Models")
        );
        assert!(app
            .aux_panel
            .as_ref()
            .is_some_and(|panel| matches!(&panel.content, AuxPanelContent::ModelList(entries) if entries.iter().any(|entry| entry.slug == "test-model"))));
    }

    #[tokio::test]
    async fn slash_rename_requires_title() {
        let mut app = test_app();

        assert!(app.handle_slash_command("/rename".to_string()).is_err());
    }

    #[tokio::test]
    async fn slash_exit_requests_shutdown() {
        let mut app = test_app();

        app.handle_slash_command("/exit".to_string())
            .expect("exit command should succeed");

        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_requires_confirmation_when_idle() {
        let mut app = test_app();

        app.handle_ctrl_c();
        assert!(!app.should_quit);
        assert_eq!(app.status_message, "Press Ctrl+C again within 2s to exit.");

        app.handle_ctrl_c();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_requests_interrupt_before_exit_when_busy() {
        let mut app = test_app();
        app.busy = true;

        app.handle_ctrl_c();
        assert!(!app.should_quit);
        assert_eq!(
            app.status_message,
            "Interrupt requested. Press Ctrl+C again within 2s to exit."
        );

        app.handle_ctrl_c();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn slash_completion_applies_selected_command() {
        let mut app = test_app();
        app.input.replace("/e");

        assert!(app.try_apply_slash_suggestion());
        assert_eq!(app.input.text(), "/exit");
    }

    #[tokio::test]
    async fn enter_executes_highlighted_slash_command() {
        let mut app = test_app();
        app.input.replace("/");
        app.slash_selection = 5;

        app.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Rect::default(),
        );

        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn model_panel_selection_updates_model() {
        let mut app = test_app();

        app.handle_slash_command("/model".to_string())
            .expect("model command should succeed");
        app.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Rect::default(),
        );

        assert_eq!(app.model, "test-model");
    }

    #[tokio::test]
    async fn session_new_command_updates_status() {
        let mut app = test_app();

        app.handle_slash_command("/new".to_string())
            .expect("slash command should succeed");

        assert_eq!(
            app.status_message,
            "New session ready; send a prompt to start it"
        );
        assert_eq!(app.aux_panel, None);
    }

    #[tokio::test]
    async fn new_session_prepared_clears_transcript_and_busy_state() {
        let mut app = test_app();
        app.busy = true;
        app.transcript.push(TranscriptItem::new(
            TranscriptItemKind::User,
            "You",
            "old session",
        ));
        app.pending_status_index = Some(0);

        app.handle_worker_event(WorkerEvent::NewSessionPrepared);

        assert!(app.transcript.is_empty());
        assert!(!app.busy);
        assert_eq!(
            app.status_message,
            "New session ready; send a prompt to start it"
        );
    }

    #[tokio::test]
    async fn tool_call_breaks_assistant_stream_into_new_segment() {
        let mut app = test_app();
        app.handle_worker_event(WorkerEvent::TextDelta("before".to_string()));
        app.handle_worker_event(WorkerEvent::ToolCall {
            summary: "Ran date".to_string(),
            detail: Some("{\n  \"command\": \"date\"\n}".to_string()),
        });
        app.handle_worker_event(WorkerEvent::TextDelta("after".to_string()));

        assert_eq!(
            app.transcript,
            vec![
                TranscriptItem::new(TranscriptItemKind::Assistant, "Assistant", "before"),
                TranscriptItem::new(
                    TranscriptItemKind::ToolCall,
                    "Ran date",
                    "{\n  \"command\": \"date\"\n}"
                ),
                TranscriptItem::new(TranscriptItemKind::Assistant, "Assistant", "after"),
            ]
        );
    }

    #[tokio::test]
    async fn tool_result_readds_thinking_while_turn_is_still_busy() {
        let mut app = test_app();
        app.busy = true;
        app.pending_status_index = Some(app.push_item(TranscriptItemKind::System, "Thinking", ""));

        app.handle_worker_event(WorkerEvent::ToolResult {
            preview: "2026-04-06 23:58:56".to_string(),
            is_error: false,
            truncated: false,
        });

        assert_eq!(app.transcript.len(), 2);
        assert_eq!(app.transcript[0].kind, TranscriptItemKind::ToolResult);
        assert_eq!(app.transcript[0].title, "Tool output");
        assert_eq!(app.transcript[0].body, "2026-04-06 23:58:56");
        assert_eq!(app.transcript[0].fold_stage, 0);
        assert!(app.transcript[0].fold_next_at.is_some());
        assert_eq!(
            app.transcript[1],
            TranscriptItem::new(TranscriptItemKind::System, "Thinking", "")
        );
    }

    #[tokio::test]
    async fn submit_prompt_inserts_status_line_below_user_message() {
        let mut app = test_app();

        app.submit_prompt("hello".to_string())
            .expect("submit should succeed");

        assert_eq!(
            app.transcript,
            vec![
                TranscriptItem::new(TranscriptItemKind::User, "You", "hello"),
                TranscriptItem::new(TranscriptItemKind::System, "Thinking", ""),
            ]
        );
    }

    #[tokio::test]
    async fn transcript_area_tracks_content_height_when_short() {
        let app = test_app();
        let area = Rect::new(0, 0, 80, 24);

        assert_eq!(render::transcript_height(&app, area), 7);
        assert!(app.transcript_area(area).height < area.height);
    }

    #[tokio::test]
    async fn session_switched_event_updates_model_and_transcript() {
        let mut app = test_app();

        app.handle_worker_event(WorkerEvent::SessionSwitched {
            session_id: "00000000-0000-0000-0000-000000000001".to_string(),
            title: Some("Saved session".to_string()),
            model: Some("restored-model".to_string()),
            history_items: vec![TranscriptItem::new(
                TranscriptItemKind::User,
                "You",
                "restored prompt",
            )],
            loaded_item_count: 7,
        });

        assert_eq!(app.model, "restored-model");
        assert_eq!(app.transcript.len(), 1);
        assert_eq!(app.transcript[0].kind, TranscriptItemKind::User);
        assert_eq!(app.transcript[0].body, "restored prompt");
    }

    #[tokio::test]
    async fn session_renamed_event_adds_transcript_note() {
        let mut app = test_app();

        app.handle_worker_event(WorkerEvent::SessionRenamed {
            session_id: "00000000-0000-0000-0000-000000000001".to_string(),
            title: "Renamed session".to_string(),
        });

        assert_eq!(app.status_message, "Session renamed");
        assert_eq!(app.transcript.len(), 1);
        assert!(app.transcript[0].body.contains("Renamed session"));
    }

    #[tokio::test]
    async fn session_title_updated_event_refreshes_visible_session_list() {
        let mut app = test_app();
        let session_id = SessionId::new();
        app.show_session_panel(vec![SessionListEntry {
            session_id,
            title: "(untitled)".to_string(),
            updated_at: "2026-04-06 08:00:00 UTC".to_string(),
            is_active: true,
        }]);

        app.handle_worker_event(WorkerEvent::SessionTitleUpdated {
            session_id: session_id.to_string(),
            title: "Generated title".to_string(),
        });

        assert_eq!(app.status_message, "Session titled: Generated title");
        assert!(app.aux_panel.as_ref().is_some_and(|panel| {
            matches!(
                &panel.content,
                AuxPanelContent::SessionList(entries)
                    if entries.iter().any(|entry| entry.title == "Generated title")
            )
        }));
    }

    #[tokio::test]
    async fn sessions_listed_event_updates_bottom_panel_not_transcript() {
        let mut app = test_app();

        app.handle_worker_event(WorkerEvent::SessionsListed {
            sessions: vec![SessionListEntry {
                session_id: SessionId::new(),
                title: "Saved conversation".to_string(),
                updated_at: "2026-04-06 08:00:00 UTC".to_string(),
                is_active: true,
            }],
        });

        assert!(app.transcript.is_empty());
        assert_eq!(
            app.aux_panel.as_ref().map(|panel| panel.title.as_str()),
            Some("Sessions")
        );
        assert!(app
            .aux_panel
            .as_ref()
            .is_some_and(|panel| matches!(&panel.content, AuxPanelContent::SessionList(entries) if entries.iter().any(|entry| entry.title == "Saved conversation"))));
    }

    #[tokio::test]
    async fn session_panel_selection_moves_with_up_and_down() {
        let mut app = test_app();
        app.show_session_panel(vec![
            SessionListEntry {
                session_id: SessionId::new(),
                title: "First".to_string(),
                updated_at: "2026-04-06 08:00:00 UTC".to_string(),
                is_active: true,
            },
            SessionListEntry {
                session_id: SessionId::new(),
                title: "Second".to_string(),
                updated_at: "2026-04-06 09:00:00 UTC".to_string(),
                is_active: false,
            },
        ]);

        app.move_aux_panel_selection(1);
        assert_eq!(app.aux_panel_selection, 1);

        app.move_aux_panel_selection(-1);
        assert_eq!(app.aux_panel_selection, 0);
    }

    #[tokio::test]
    async fn interrupted_turn_adds_status_line_to_transcript() {
        let mut app = test_app();
        app.pending_status_index = Some(app.push_item(TranscriptItemKind::System, "Thinking", ""));
        app.busy = true;

        app.handle_worker_event(WorkerEvent::TurnFinished {
            stop_reason: "Interrupted".to_string(),
            turn_count: 1,
            total_input_tokens: 0,
            total_output_tokens: 0,
        });

        assert_eq!(
            app.transcript,
            vec![TranscriptItem::new(
                TranscriptItemKind::System,
                "Interrupted",
                "",
            )]
        );
    }

    #[tokio::test]
    async fn completed_turn_adds_complete_status_line_to_transcript() {
        let mut app = test_app();
        app.pending_status_index = Some(app.push_item(TranscriptItemKind::System, "Thinking", ""));
        app.busy = true;

        app.handle_worker_event(WorkerEvent::TurnFinished {
            stop_reason: "Completed".to_string(),
            turn_count: 1,
            total_input_tokens: 0,
            total_output_tokens: 0,
        });

        assert_eq!(
            app.transcript,
            vec![TranscriptItem::new(
                TranscriptItemKind::System,
                "Complete",
                ""
            )]
        );
    }
}
