use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use devo_protocol::CollaborationMode;
use devo_protocol::ReferenceSearchSnapshot;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use devo_protocol::user_input::TextElement;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;

mod approval_overlay;
pub(crate) mod bottom_pane_view;
mod chat_composer;
mod chat_composer_history;
mod command_popup;
mod compaction_threshold_view;
mod context_occupancy_view;
mod custom_prompt_view;
mod delete_session_confirm_view;
mod footer;
mod horizontal_chip_strip;
mod input_mode;
pub(crate) mod list_selection_view;
mod model_picker;
mod paste_burst;
mod pending_queue;
mod pending_thread_approvals;
pub(crate) mod popup_consts;
mod prompt_args;
mod proposed_plan_actions_view;
mod reference_popup;
mod request_user_input_overlay;
mod resume_picker;
pub(crate) mod scroll_state;
mod selection_popup_common;
mod settings_hub_view;
pub(crate) mod slash_commands;
pub(crate) mod textarea;
mod theme_picker;
mod unified_exec_footer;

pub(crate) use approval_overlay::ApprovalOverlay;
pub(crate) use approval_overlay::ApprovalOverlayRequest;
pub(crate) use chat_composer::ChatComposer;
use chat_composer::ChatComposerConfig;
use chat_composer::InputResult as ComposerInputResult;
pub(crate) use compaction_threshold_view::CompactionThresholdSnapshot;
use compaction_threshold_view::CompactionThresholdView;
pub(crate) use compaction_threshold_view::format_token_limit;
pub(crate) use compaction_threshold_view::recommended_compaction_token_limit;
use context_occupancy_view::ContextOccupancyView;
pub(crate) use context_occupancy_view::SessionTokenTotals;
pub(crate) use context_occupancy_view::StatusPanelSnapshot;
pub(crate) use custom_prompt_view::CustomPromptView;
pub(crate) use delete_session_confirm_view::DeleteSessionConfirmView;
pub(crate) use input_mode::InputMode;
pub(crate) use model_picker::ModelPickerEffortOption;
pub(crate) use model_picker::ModelPickerEntry;
pub(crate) use model_picker::ModelPickerSelection;
use model_picker::ModelPickerView;
pub(crate) use proposed_plan_actions_view::ProposedPlanActionsView;
pub(crate) use resume_picker::ResumePickerAction;
pub(crate) use settings_hub_view::SettingsHubSnapshot;
pub(crate) use settings_hub_view::SettingsHubTab;
use settings_hub_view::SettingsHubView;

use crate::app_command::AppCommand;
use crate::app_command::InputHistoryDirection;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
pub(crate) use crate::bottom_pane::pending_queue::PendingQueueItem;
use crate::bottom_pane::pending_queue::PendingQueueList;
use crate::bottom_pane::pending_queue::PendingQueueState;
pub(crate) use crate::bottom_pane::pending_queue::QueueNavResult;
use crate::bottom_pane::pending_thread_approvals::PendingThreadApprovals;
use crate::bottom_pane::request_user_input_overlay::RequestUserInputOverlay;
use crate::bottom_pane::unified_exec_footer::UnifiedExecFooter;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;
use crate::status_indicator_widget::StatusIndicatorWidget;
use crate::status_indicator_widget::TIP_ROTATION_INTERVAL;
use crate::status_indicator_widget::composer_tip_placeholder;
use crate::tui::frame_requester::FrameRequester;

pub(crate) const QUIT_SHORTCUT_TIMEOUT: Duration = Duration::from_secs(2);

const STATUS_SEPARATOR: BlankLineSpacer = BlankLineSpacer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CancellationEvent {
    Handled,
    NotHandled,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LocalImageAttachment {
    pub(crate) placeholder: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MentionBinding {
    pub(crate) mention: String,
    pub(crate) path: String,
}

fn is_input_mode_cycle_key(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        && (key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT)))
}

fn is_bare_shell_mode_trigger(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        && key.code == KeyCode::Char('!')
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn one_shot_shell_command(text: &str) -> Option<String> {
    text.strip_prefix('!').map(str::to_string)
}

fn escaped_bang_submission(
    text: String,
    text_elements: Vec<TextElement>,
) -> (String, Vec<TextElement>) {
    let Some(rest) = text.strip_prefix("\\!") else {
        return (text, text_elements);
    };
    let mut adjusted = Vec::new();
    for element in text_elements {
        if element.byte_range.start == 0 {
            continue;
        }
        adjusted.push(
            element.map_range(|range| devo_protocol::user_input::Utf8ByteSpan {
                start: range.start.saturating_sub(1),
                end: range.end.saturating_sub(1),
            }),
        );
    }
    (format!("!{rest}"), adjusted)
}

#[derive(Debug, Default)]
struct BlankLineSpacer;

impl Renderable for BlankLineSpacer {
    fn render(&self, _area: Rect, _buf: &mut Buffer) {}

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SkillInterfaceMetadata {
    pub(crate) display_name: Option<String>,
    pub(crate) short_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) short_description: Option<String>,
    pub(crate) interface: Option<SkillInterfaceMetadata>,
    pub(crate) path_to_skills_md: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginCapabilitySummary {
    pub(crate) config_name: String,
    pub(crate) display_name: String,
    pub(crate) description: Option<String>,
    pub(crate) has_skills: bool,
    pub(crate) mcp_server_names: Vec<String>,
    pub(crate) app_connector_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InputResult {
    Submitted {
        text: String,
        text_elements: Vec<TextElement>,
        local_images: Vec<LocalImageAttachment>,
        mention_bindings: Vec<MentionBinding>,
        input_mode: InputMode,
        collaboration_mode: CollaborationMode,
    },
    ShellCommand {
        command: String,
    },
    ShellInput {
        command: String,
    },
    Command {
        command: SlashCommand,
        argument: String,
    },
    ModelSelected {
        model: String,
        reasoning_effort: Option<String>,
    },
    ThemeSelected {
        name: String,
    },
    /// Steer the selected queued item into the active turn.
    QueueSteer {
        queue_item_id: String,
    },
    /// Remove the selected queued item and load its text into the composer.
    QueueEdit {
        queue_item_id: String,
        text: String,
    },
    /// Remove the selected queued item without loading it into the composer.
    QueueRemove {
        queue_item_id: String,
    },
    InputModeChanged {
        input_mode: InputMode,
    },
    ResumeAction(ResumePickerAction),
    None,
}

pub(crate) struct BottomPaneParams {
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) frame_requester: FrameRequester,
    pub(crate) has_input_focus: bool,
    pub(crate) enhanced_keys_supported: bool,
    pub(crate) placeholder_text: String,
    pub(crate) disable_paste_burst: bool,
    pub(crate) skills: Option<Vec<SkillMetadata>>,
    pub(crate) animations_enabled: bool,
}

pub(crate) struct BottomPane {
    composer: ChatComposer,
    view_stack: Vec<Box<dyn BottomPaneView>>,
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    unified_exec_footer: UnifiedExecFooter,
    pending_thread_approvals: PendingThreadApprovals,
    /// User messages queued while a turn was active, shown below the composer.
    pending_queue: PendingQueueState,
    placeholder_text: String,
    /// Wall-clock start for rotating composer placeholder tips (`Tip: …`).
    placeholder_tips_started_at: Instant,
    /// Status indicator shown above the composer while a task is running.
    status: Option<StatusIndicatorWidget>,
    subagent_hint_visible: bool,
    is_task_running: bool,
    pending_interrupt_esc: bool,
    interrupt_requested: bool,
    animations_enabled: bool,
    has_input_focus: bool,
    allow_empty_submit: bool,
    external_history_active: bool,
    external_history_draft: Option<String>,
    input_mode: InputMode,
    accent_color: Color,
}

impl BottomPane {
    pub(crate) fn new(params: BottomPaneParams) -> Self {
        let BottomPaneParams {
            app_event_tx,
            frame_requester,
            has_input_focus,
            enhanced_keys_supported,
            placeholder_text,
            disable_paste_burst,
            skills,
            animations_enabled,
        } = params;
        let mut composer = ChatComposer::new_with_config(
            has_input_focus,
            app_event_tx.clone(),
            enhanced_keys_supported,
            placeholder_text.clone(),
            disable_paste_burst,
            ChatComposerConfig::default(),
        );
        composer.set_frame_requester(frame_requester.clone());
        composer.set_skill_mentions(skills);
        let placeholder_tips_started_at = Instant::now();
        let placeholder_text = composer_tip_placeholder(Duration::ZERO).unwrap_or(placeholder_text);
        composer.set_placeholder_text(placeholder_text.clone());
        let pane = Self {
            composer,
            view_stack: Vec::new(),
            app_event_tx,
            frame_requester,
            unified_exec_footer: UnifiedExecFooter::new(),
            pending_thread_approvals: PendingThreadApprovals::new(),
            pending_queue: PendingQueueState::default(),
            placeholder_text,
            placeholder_tips_started_at,
            status: None,
            subagent_hint_visible: false,
            is_task_running: false,
            pending_interrupt_esc: false,
            interrupt_requested: false,
            animations_enabled,
            has_input_focus,
            allow_empty_submit: false,
            external_history_active: false,
            external_history_draft: None,
            input_mode: InputMode::Build,
            accent_color: Color::Cyan,
        };
        pane.schedule_placeholder_tip_redraw();
        pane
    }

    pub(crate) fn set_accent_color(&mut self, color: Color) {
        self.accent_color = color;
        self.composer.set_accent_color(color);
        for view in &mut self.view_stack {
            view.set_accent_color(color);
        }
        self.request_redraw();
    }
    pub(crate) fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    pub(crate) fn set_input_mode(&mut self, mode: InputMode) {
        if self.input_mode == mode {
            return;
        }
        self.input_mode = mode;
        self.composer.set_input_mode_indicator(Some(mode));
        self.request_redraw();
    }

    fn cycle_input_mode(&mut self) {
        self.set_input_mode(self.input_mode.next());
    }

    pub(crate) fn cycle_build_plan_mode(&mut self) {
        let next = match self.input_mode {
            InputMode::Build => InputMode::Plan,
            InputMode::Plan | InputMode::Shell => InputMode::Build,
        };
        self.set_input_mode(next);
    }

    pub(crate) fn set_skill_mentions(&mut self, skills: Option<Vec<SkillMetadata>>) {
        self.composer.set_skill_mentions(skills);
        self.request_redraw();
    }

    pub(crate) fn on_reference_search_result(&mut self, snapshot: ReferenceSearchSnapshot) {
        self.composer.on_reference_search_result(snapshot);
        self.request_redraw();
    }

    pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> InputResult {
        if !self.view_stack.is_empty() {
            return self.handle_view_key_event(key);
        }

        if is_input_mode_cycle_key(key) && !self.composer.popup_active() {
            let previous = self.input_mode;
            self.cycle_input_mode();
            if self.input_mode != previous {
                return InputResult::InputModeChanged {
                    input_mode: self.input_mode,
                };
            }
            return InputResult::None;
        }

        if is_bare_shell_mode_trigger(key) && self.composer.is_empty() {
            let previous = self.input_mode;
            self.set_input_mode(InputMode::Shell);
            if self.input_mode != previous {
                return InputResult::InputModeChanged {
                    input_mode: self.input_mode,
                };
            }
            return InputResult::None;
        }

        if self.pending_queue.focused()
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            return self.handle_pending_queue_key(key);
        }

        if key.code == KeyCode::Down
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && key.modifiers == KeyModifiers::NONE
            && self.composer.is_empty()
            && self.has_pending_cells()
            && !self.composer.popup_active()
        {
            self.focus_pending_queue();
            return InputResult::None;
        }

        if key.code == KeyCode::Esc
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && self.is_task_running
            && !self.interrupt_requested
            && !self.composer.popup_active()
        {
            if self.pending_interrupt_esc {
                self.pending_interrupt_esc = false;
                self.app_event_tx.send(AppEvent::Interrupt);
            } else {
                self.pending_interrupt_esc = true;
                if let Some(status) = self.status.as_mut() {
                    status.set_interrupt_hint_visible(false);
                    status.update_inline_message(Some("Press ESC again to stop".to_string()));
                }
            }
            self.request_redraw();
            return InputResult::None;
        }

        if self.should_route_external_history(key) {
            return self.request_external_history(key);
        }

        if self.allow_empty_submit
            && key.code == KeyCode::Enter
            && key.modifiers == KeyModifiers::NONE
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && self.composer.is_empty()
        {
            self.reset_external_history_navigation();
            return InputResult::Submitted {
                text: String::new(),
                text_elements: Vec::new(),
                local_images: Vec::new(),
                mention_bindings: Vec::new(),
                input_mode: InputMode::Build,
                collaboration_mode: CollaborationMode::Build,
            };
        }

        let (input_result, needs_redraw) = self.composer.handle_key_event(key);
        if needs_redraw {
            self.request_redraw();
        }
        if self.composer.is_in_paste_burst() {
            self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
        }
        self.map_composer_input_result(input_result)
    }

    fn handle_pending_queue_key(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Down => {
                self.pending_queue_select_next();
                InputResult::None
            }
            KeyCode::Up => {
                let _ = self.pending_queue_select_prev();
                InputResult::None
            }
            KeyCode::Esc => {
                self.clear_pending_queue_focus();
                InputResult::None
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let Some(item) = self.selected_pending_queue_item() else {
                    return InputResult::None;
                };
                InputResult::QueueSteer {
                    queue_item_id: item.queue_item_id.clone(),
                }
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let Some(item) = self.selected_pending_queue_item() else {
                    return InputResult::None;
                };
                InputResult::QueueEdit {
                    queue_item_id: item.queue_item_id.clone(),
                    text: item.text.clone(),
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let Some(item) = self.selected_pending_queue_item() else {
                    return InputResult::None;
                };
                InputResult::QueueRemove {
                    queue_item_id: item.queue_item_id.clone(),
                }
            }
            _ => InputResult::None,
        }
    }

    pub fn handle_paste(&mut self, pasted: String) {
        if !self.view_stack.is_empty() {
            let (needs_redraw, view_complete) = {
                let last_index = self.view_stack.len() - 1;
                let view = &mut self.view_stack[last_index];
                (view.handle_paste(pasted), view.is_complete())
            };
            if view_complete {
                self.view_stack.clear();
                self.on_active_view_complete();
            }
            if needs_redraw {
                self.request_redraw();
            }
        } else {
            let needs_redraw = self.composer.handle_paste(pasted);
            self.composer.sync_popups();
            if needs_redraw {
                self.request_redraw();
            }
        }
    }

    fn on_active_view_complete(&mut self) {
        self.set_composer_input_enabled(/*enabled*/ true, /*placeholder*/ None);
    }

    pub(crate) fn set_composer_input_enabled(
        &mut self,
        enabled: bool,
        placeholder: Option<String>,
    ) {
        self.composer.set_input_enabled(enabled, placeholder);
        self.request_redraw();
    }

    pub(crate) fn pre_draw_tick(&mut self) {
        self.sync_placeholder_tip();
        self.composer.sync_popups();
        if self.composer.flush_paste_burst_if_due() {
            self.request_redraw();
        } else if self.composer.is_in_paste_burst() {
            self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
        }
    }

    pub(crate) fn set_placeholder_text(&mut self, placeholder: impl Into<String>) {
        let placeholder = placeholder.into();
        self.placeholder_text = placeholder.clone();
        self.composer.set_placeholder_text(placeholder);
        self.request_redraw();
    }

    /// Restore the rotating `Tip: …` composer placeholder (replaces a fixed default).
    pub(crate) fn set_default_placeholder(&mut self) {
        self.sync_placeholder_tip();
        self.schedule_placeholder_tip_redraw();
    }

    fn sync_placeholder_tip(&mut self) {
        let Some(text) = composer_tip_placeholder(self.placeholder_tips_started_at.elapsed())
        else {
            return;
        };
        if self.placeholder_text == text {
            self.schedule_placeholder_tip_redraw();
            return;
        }
        self.placeholder_text = text.clone();
        self.composer.set_placeholder_text(text);
        self.request_redraw();
        self.schedule_placeholder_tip_redraw();
    }

    fn schedule_placeholder_tip_redraw(&self) {
        if !self.animations_enabled {
            return;
        }
        let elapsed = self.placeholder_tips_started_at.elapsed();
        let interval_secs = TIP_ROTATION_INTERVAL.as_secs().max(1);
        let into = Duration::from_secs(elapsed.as_secs() % interval_secs);
        let until_next = TIP_ROTATION_INTERVAL.saturating_sub(into);
        self.request_redraw_in(until_next.max(Duration::from_millis(50)));
    }

    #[cfg(test)]
    pub(crate) fn set_placeholder_tips_elapsed_for_test(&mut self, elapsed: Duration) {
        self.placeholder_tips_started_at = Instant::now()
            .checked_sub(elapsed)
            .unwrap_or_else(Instant::now);
        self.sync_placeholder_tip();
    }

    pub(crate) fn clear_composer(&mut self) {
        self.composer
            .set_text_content(String::new(), Vec::new(), Vec::new());
        self.external_history_active = false;
        self.external_history_draft = None;
        self.request_redraw();
    }

    pub(crate) fn composer_text(&self) -> String {
        self.composer.current_text()
    }

    /// Insert text at the composer cursor, adding a leading space when needed.
    pub(crate) fn insert_composer_text(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let current = self.composer.current_text();
        let needs_leading_space =
            !current.is_empty() && !current.ends_with(|ch: char| ch.is_whitespace());
        if needs_leading_space {
            self.composer.insert_str(&format!(" {text} "));
        } else {
            self.composer.insert_str(&format!("{text} "));
        }
        self.request_redraw();
    }

    /// Insert a highlighted chip whose model-facing value differs from the display text.
    pub(crate) fn insert_composer_bound_text(&mut self, display: &str, binding: &str) {
        let display = display.trim();
        let binding = binding.trim();
        if display.is_empty() || binding.is_empty() {
            return;
        }
        let current = self.composer.current_text();
        let needs_leading_space =
            !current.is_empty() && !current.ends_with(|ch: char| ch.is_whitespace());
        if needs_leading_space {
            self.composer.insert_str(" ");
        }
        self.composer.insert_bound_element(display, binding);
        self.request_redraw();
    }

    #[cfg(test)]
    pub(crate) fn placeholder_text(&self) -> &str {
        &self.placeholder_text
    }

    pub(crate) fn set_allow_empty_submit(&mut self, enabled: bool) {
        self.allow_empty_submit = enabled;
    }

    pub(crate) fn open_model_picker(&mut self, entries: Vec<ModelPickerEntry>) {
        self.push_view(Box::new(ModelPickerView::new(entries, self.accent_color)));
    }

    pub(crate) fn open_status_panel(
        &mut self,
        occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
        session: SessionTokenTotals,
        status: StatusPanelSnapshot,
    ) {
        self.push_view(Box::new(ContextOccupancyView::new(
            occupancy, session, status,
        )));
    }

    pub(crate) fn open_theme_picker(
        &mut self,
        themes: &[crate::theme::Theme],
        current_name: String,
    ) {
        self.push_view(Box::new(theme_picker::ThemePickerView::new(
            themes,
            current_name,
        )));
    }

    pub(crate) fn open_settings_hub(&mut self, snapshot: SettingsHubSnapshot) {
        self.push_view(Box::new(SettingsHubView::new(
            snapshot,
            self.app_event_tx.clone(),
            self.accent_color,
        )));
    }

    pub(crate) fn open_settings_hub_on_tab(
        &mut self,
        snapshot: SettingsHubSnapshot,
        tab: settings_hub_view::SettingsHubTab,
    ) {
        self.push_view(Box::new(
            SettingsHubView::new(snapshot, self.app_event_tx.clone(), self.accent_color)
                .with_tab(tab),
        ));
    }

    pub(crate) fn open_compaction_threshold(&mut self, snapshot: CompactionThresholdSnapshot) {
        self.push_view(Box::new(CompactionThresholdView::new(
            snapshot,
            self.app_event_tx.clone(),
            self.accent_color,
        )));
    }

    pub(crate) fn refresh_settings_hub(&mut self, snapshot: SettingsHubSnapshot) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_settings_hub_snapshot(snapshot.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn refresh_status_panel(
        &mut self,
        occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
        session: SessionTokenTotals,
    ) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_status_panel(occupancy.clone(), session) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn open_popup_view(&mut self, view: Box<dyn BottomPaneView>) {
        self.push_view(view);
    }

    #[cfg(test)]
    pub(crate) fn has_view_for_test(&self) -> bool {
        !self.view_stack.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn resume_selection_for_test(&self) -> Option<usize> {
        self.active_view()
            .and_then(BottomPaneView::resume_selection_for_test)
    }

    #[cfg(test)]
    pub(crate) fn resume_scroll_offset_for_test(&self) -> Option<usize> {
        self.active_view()
            .and_then(BottomPaneView::resume_scroll_offset_for_test)
    }

    #[cfg(test)]
    pub(crate) fn resume_pending_delete_for_test(&self) -> Option<SessionId> {
        self.active_view()
            .and_then(BottomPaneView::resume_pending_delete_for_test)
    }

    pub(crate) fn open_request_user_input(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: Vec<RequestUserInputQuestion>,
    ) {
        self.push_view(Box::new(RequestUserInputOverlay::new(
            session_id,
            turn_id,
            request_id,
            questions,
            self.app_event_tx.clone(),
            self.accent_color,
        )));
    }

    pub(crate) fn restore_input_from_history(&mut self, text: Option<String>) {
        match text {
            Some(text) => {
                self.composer.set_text_content(text, Vec::new(), Vec::new());
                self.external_history_active = true;
            }
            None => {
                let draft = self.external_history_draft.take().unwrap_or_default();
                self.composer
                    .set_text_content(draft, Vec::new(), Vec::new());
                self.external_history_active = false;
            }
        }
        self.request_redraw();
    }

    pub(crate) fn current_text(&self) -> String {
        self.composer.current_text()
    }

    pub(crate) fn set_remote_image_urls(&mut self, urls: Vec<String>) {
        self.composer.set_remote_image_urls(urls);
        self.request_redraw();
    }

    pub(crate) fn set_text_content(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
    ) {
        self.composer
            .set_text_content(text, text_elements, local_image_paths);
        self.request_redraw();
    }

    /// Move the composer cursor to the end of its text (e.g. after loading a
    /// queued message for editing, so new input appends instead of
    /// prepending).
    pub(crate) fn move_composer_cursor_to_end(&mut self) {
        self.composer.move_cursor_to_end();
        self.request_redraw();
    }

    pub(crate) fn is_normal_backtrack_mode(&self) -> bool {
        self.active_view().is_none()
            && !self.is_task_running
            && !self.composer.popup_active()
            && !self.external_history_active
    }

    pub(crate) fn show_esc_backtrack_hint(&mut self) {
        self.composer.set_esc_backtrack_hint(/*show*/ true);
        self.request_redraw();
    }

    pub(crate) fn clear_esc_backtrack_hint(&mut self) {
        self.composer.set_esc_backtrack_hint(/*show*/ false);
        self.request_redraw();
    }

    #[allow(dead_code)]
    pub(crate) fn set_status_line(&mut self, status_line: Option<Line<'static>>) {
        if self.composer.set_status_line(status_line) {
            self.request_redraw();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_status_line_enabled(&mut self, enabled: bool) {
        if self.composer.set_status_line_enabled(enabled) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_context_window_label(&mut self, label: Option<String>) {
        if self.composer.set_context_window_label(label) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_active_agent_label(&mut self, active_agent_label: Option<String>) {
        if self.composer.set_active_agent_label(active_agent_label) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_subagent_hint_visible(&mut self, visible: bool) {
        if self.subagent_hint_visible == visible {
            return;
        }
        self.subagent_hint_visible = visible;
        self.sync_subagent_hint_surface();
        self.request_redraw();
    }

    fn sync_subagent_hint_surface(&mut self) {
        let status_visible = self.status.is_some();
        if let Some(status) = self.status.as_mut() {
            status.set_subagent_hint_visible(self.subagent_hint_visible);
        }
        self.composer
            .set_subagent_hint_visible(self.subagent_hint_visible && !status_visible);
    }

    pub(crate) fn is_task_running(&self) -> bool {
        self.is_task_running
    }

    pub(crate) fn set_task_running(&mut self, running: bool) {
        let was_running = self.is_task_running;
        self.is_task_running = running;
        self.composer.set_task_running(running);
        if running {
            self.pending_interrupt_esc = false;
            self.interrupt_requested = false;
            if !was_running {
                if self.status.is_none() {
                    self.status = Some(StatusIndicatorWidget::new(
                        self.app_event_tx.clone(),
                        self.frame_requester.clone(),
                        self.animations_enabled,
                    ));
                }
                if let Some(status) = self.status.as_mut() {
                    status.set_interrupt_hint_visible(true);
                }
                self.sync_subagent_hint_surface();
                self.request_redraw();
            }
        } else {
            self.interrupt_requested = false;
            self.hide_status_indicator();
        }
    }

    pub(crate) fn try_begin_interrupt(&mut self) -> bool {
        if self.interrupt_requested {
            return false;
        }
        self.interrupt_requested = true;
        if let Some(status) = self.status.as_mut() {
            status.update_header("Stopping…".to_string());
            status.set_interrupt_hint_visible(false);
            status.set_working_tip_visible(false);
            status.pause_timer();
            status.update_inline_message(None);
        }
        self.request_redraw();
        true
    }

    pub(crate) fn open_resume_picker(&mut self, current_cwd: PathBuf) {
        self.view_stack.clear();
        self.push_view(Box::new(resume_picker::ResumePickerView::loading(
            current_cwd,
            self.accent_color,
        )));
    }

    pub(crate) fn is_resume_picker_open(&self) -> bool {
        self.active_view().and_then(BottomPaneView::view_id) == Some("resume_picker")
    }

    pub(crate) fn update_resume_sessions(
        &mut self,
        sessions: Vec<crate::events::SessionListEntry>,
    ) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_resume_sessions(sessions.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn update_resume_list_error(&mut self, message: String) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_resume_list_error(message.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn update_resume_preview(
        &mut self,
        session_id: SessionId,
        result: Result<Vec<crate::events::SessionPreviewMessage>, String>,
    ) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_resume_preview(session_id, result.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn update_resume_rename(
        &mut self,
        session_id: Option<SessionId>,
        result: Result<String, String>,
    ) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_resume_rename(session_id, result.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn update_resume_delete(
        &mut self,
        session_id: Option<SessionId>,
        result: Result<(), String>,
    ) {
        for view in self.view_stack.iter_mut().rev() {
            if view.update_resume_delete(session_id, result.clone()) {
                self.request_redraw();
                break;
            }
        }
    }

    pub(crate) fn interrupt_failed(&mut self) {
        self.interrupt_requested = false;
        if let Some(status) = self.status.as_mut() {
            status.update_header("Working".to_string());
            status.set_interrupt_hint_visible(true);
            status.set_working_tip_visible(true);
            status.resume_timer();
        }
        self.request_redraw();
    }

    pub(crate) fn hide_status_indicator(&mut self) {
        if self.status.take().is_some() {
            self.pending_interrupt_esc = false;
            self.interrupt_requested = false;
            self.sync_subagent_hint_surface();
            self.request_redraw();
        }
    }

    pub(crate) fn replace_pending_queue(&mut self, items: Vec<PendingQueueItem>) {
        self.pending_queue.replace_items(items);
        self.request_redraw();
    }

    pub(crate) fn pending_queue_items(&self) -> &[PendingQueueItem] {
        self.pending_queue.items()
    }

    pub(crate) fn pending_queue_focused(&self) -> bool {
        self.pending_queue.focused()
    }

    pub(crate) fn focus_pending_queue(&mut self) -> bool {
        let focused = self.pending_queue.focus_first();
        if focused {
            self.request_redraw();
        }
        focused
    }

    pub(crate) fn clear_pending_queue_focus(&mut self) {
        if self.pending_queue.focused() {
            self.pending_queue.clear_focus();
            self.request_redraw();
        }
    }

    pub(crate) fn pending_queue_select_next(&mut self) -> bool {
        let handled = self.pending_queue.select_next();
        if handled {
            self.request_redraw();
        }
        handled
    }

    pub(crate) fn pending_queue_select_prev(&mut self) -> QueueNavResult {
        let result = self.pending_queue.select_prev();
        if result != QueueNavResult::Ignored {
            self.request_redraw();
        }
        result
    }

    pub(crate) fn selected_pending_queue_item(&self) -> Option<&PendingQueueItem> {
        self.pending_queue.selected_item()
    }

    /// Pop the oldest pending cell (FIFO). Returns its text, or None if empty.
    pub(crate) fn pop_oldest_pending_cell(&mut self) -> Option<String> {
        if self.pending_queue.is_empty() {
            return None;
        }
        let mut items = self.pending_queue.items().to_vec();
        let removed = items.remove(0);
        self.pending_queue.replace_items(items);
        self.request_redraw();
        Some(removed.text)
    }

    pub(crate) fn has_pending_cells(&self) -> bool {
        !self.pending_queue.is_empty()
    }

    pub(crate) fn clear_pending_cells(&mut self) {
        if !self.pending_queue.is_empty() {
            self.pending_queue.clear();
            self.request_redraw();
        }
    }

    pub(crate) fn ensure_status_indicator(&mut self) {
        if self.status.is_none() {
            self.status = Some(StatusIndicatorWidget::new(
                self.app_event_tx.clone(),
                self.frame_requester.clone(),
                self.animations_enabled,
            ));
            self.sync_subagent_hint_surface();
            self.request_redraw();
        }
    }

    pub(crate) fn status_widget(&self) -> Option<&StatusIndicatorWidget> {
        self.status.as_ref()
    }

    pub(crate) fn status_widget_mut(&mut self) -> Option<&mut StatusIndicatorWidget> {
        self.status.as_mut()
    }

    #[cfg(test)]
    pub(crate) fn status_indicator_visible(&self) -> bool {
        self.status.is_some()
    }

    fn active_view(&self) -> Option<&dyn BottomPaneView> {
        self.view_stack.last().map(std::convert::AsRef::as_ref)
    }

    /// Children for an open bottom-pane view: optional status, optionally the
    /// composer, then the view. Views that [`BottomPaneView::replaces_composer`]
    /// occupy the input area instead of stacking below a draft.
    ///
    /// Those replacing views also paint a menu-surface background; stacked
    /// views may paint one when they need panel chrome (see
    /// `replaces_composer` docs).
    fn active_view_layout_children<'a>(
        &'a self,
        view: &'a dyn BottomPaneView,
    ) -> Vec<&'a dyn Renderable> {
        let mut children: Vec<&dyn Renderable> = Vec::with_capacity(4);
        if let Some(status) = &self.status {
            children.push(&STATUS_SEPARATOR);
            children.push(status);
        }
        if !view.replaces_composer() {
            children.push(&self.composer);
        }
        children.push(view);
        children
    }

    fn push_view(&mut self, view: Box<dyn BottomPaneView>) {
        self.view_stack.push(view);
        self.request_redraw();
    }

    fn handle_view_key_event(&mut self, key: KeyEvent) -> InputResult {
        if matches!(key.kind, KeyEventKind::Release) {
            return InputResult::None;
        }

        let last_index = self.view_stack.len() - 1;
        let view = &mut self.view_stack[last_index];
        let prefer_esc = key.code == KeyCode::Esc && view.prefer_esc_to_handle_key_event();
        let completed_by_cancel = key.code == KeyCode::Esc
            && !prefer_esc
            && matches!(view.on_ctrl_c(), CancellationEvent::Handled)
            && view.is_complete();
        if !completed_by_cancel {
            view.handle_key_event(key);
        }
        let resume_action = view.take_resume_action();

        let view_complete = self
            .view_stack
            .last()
            .is_some_and(|view| view.is_complete());
        let view_in_paste_burst = self
            .view_stack
            .last()
            .is_some_and(|view| view.is_in_paste_burst());

        if view_complete {
            let mut view = self.view_stack.pop().expect("active view exists");
            let selected_model = view.take_model_selection();
            let selected_theme = view.take_theme_selection();
            self.request_redraw();
            if let Some(selection) = selected_model {
                return InputResult::ModelSelected {
                    model: selection.model,
                    reasoning_effort: selection.reasoning_effort,
                };
            }
            if let Some(name) = selected_theme {
                return InputResult::ThemeSelected { name };
            }
            if let Some(action) = resume_action {
                return InputResult::ResumeAction(action);
            }
            return InputResult::None;
        }

        if let Some(action) = resume_action {
            return InputResult::ResumeAction(action);
        }

        if view_in_paste_burst {
            self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
        }
        self.request_redraw();
        InputResult::None
    }

    fn map_composer_input_result(&mut self, input_result: ComposerInputResult) -> InputResult {
        match input_result {
            ComposerInputResult::Submitted {
                raw_text,
                text,
                text_elements,
            }
            | ComposerInputResult::Queued {
                raw_text,
                text,
                text_elements,
            } => {
                self.reset_external_history_navigation();
                self.map_text_submission(raw_text, text, text_elements)
            }
            ComposerInputResult::Command(command) => {
                self.reset_external_history_navigation();
                InputResult::Command {
                    command,
                    argument: String::new(),
                }
            }
            ComposerInputResult::CommandWithArgs(command, argument, _text_elements) => {
                self.reset_external_history_navigation();
                InputResult::Command { command, argument }
            }
            ComposerInputResult::None => InputResult::None,
        }
    }

    fn map_text_submission(
        &mut self,
        raw_text: String,
        text: String,
        text_elements: Vec<TextElement>,
    ) -> InputResult {
        let local_images = self
            .composer
            .take_recent_submission_images_with_placeholders();
        let mention_bindings = self.composer.take_recent_submission_mention_bindings();

        if self.input_mode == InputMode::Shell {
            return InputResult::ShellInput { command: text };
        }

        if let Some(command) = one_shot_shell_command(&raw_text) {
            return if command.trim().is_empty() {
                self.set_input_mode(InputMode::Shell);
                InputResult::None
            } else {
                self.set_input_mode(InputMode::Build);
                InputResult::ShellCommand { command }
            };
        }

        let (text, text_elements) = if raw_text.starts_with("\\!") {
            escaped_bang_submission(text, text_elements)
        } else {
            (text, text_elements)
        };
        let input_mode = self.input_mode;
        InputResult::Submitted {
            text,
            text_elements,
            local_images,
            mention_bindings,
            input_mode,
            collaboration_mode: input_mode.collaboration_mode(),
        }
    }

    fn should_route_external_history(&self, key: KeyEvent) -> bool {
        if self.composer.popup_active() {
            return false;
        }
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return false;
        }
        matches!(key.code, KeyCode::Up | KeyCode::Down)
            && (self.composer.is_empty() || self.external_history_active)
    }

    fn request_external_history(&mut self, key: KeyEvent) -> InputResult {
        if !self.external_history_active {
            self.external_history_draft = Some(self.composer.current_text());
        }
        let direction = match key.code {
            KeyCode::Up => InputHistoryDirection::Previous,
            KeyCode::Down => InputHistoryDirection::Next,
            _ => return InputResult::None,
        };
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::browse_input_history(
                direction,
            )));
        InputResult::None
    }

    fn reset_external_history_navigation(&mut self) {
        self.external_history_active = false;
        self.external_history_draft = None;
    }

    fn render_children(&self, area: Rect, buf: &mut Buffer, children: &[&dyn Renderable]) {
        let mut y = area.y;
        for child in children {
            let height = child.desired_height(area.width);
            if height == 0 {
                continue;
            }
            let child_area = Rect::new(area.x, y, area.width, height).intersection(area);
            if !child_area.is_empty() {
                child.render(child_area, buf);
            }
            y = y.saturating_add(height);
            if y >= area.bottom() {
                break;
            }
        }
    }

    fn desired_children_height(&self, width: u16, children: &[&dyn Renderable]) -> u16 {
        children.iter().fold(0u16, |height, child| {
            height.saturating_add(child.desired_height(width))
        })
    }

    fn child_cursor_pos(&self, area: Rect, children: &[&dyn Renderable]) -> Option<(u16, u16)> {
        let mut y = area.y;
        for child in children {
            let height = child.desired_height(area.width);
            if height == 0 {
                continue;
            }
            let child_area = Rect::new(area.x, y, area.width, height).intersection(area);
            if let Some(cursor) = child.cursor_pos(child_area) {
                return Some(cursor);
            }
            y = y.saturating_add(height);
        }
        None
    }

    fn request_redraw(&self) {
        self.frame_requester.schedule_frame();
    }

    fn request_redraw_in(&self, dur: Duration) {
        self.frame_requester.schedule_frame_in(dur);
    }
}

impl Renderable for BottomPane {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        if let Some(view) = self.active_view() {
            let children = self.active_view_layout_children(view);
            self.render_children(area, buf, &children);
            return;
        }
        let mut children: Vec<&dyn Renderable> = Vec::with_capacity(5);
        // Status indicator above the composer while a task is running.
        if let Some(status) = &self.status {
            children.push(&STATUS_SEPARATOR);
            children.push(status);
        }
        // Avoid double-surfacing the unified-exec summary when the status row is active.
        if self.status.is_none() && !self.unified_exec_footer.is_empty() {
            children.push(&self.unified_exec_footer);
        }
        children.push(&self.pending_thread_approvals);
        children.push(&self.composer);
        let pending_queue = PendingQueueList::new(&self.pending_queue);
        if pending_queue.desired_height(area.width) > 0 {
            children.push(&pending_queue);
        }
        self.render_children(area, buf, &children);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if let Some(view) = self.active_view() {
            let children = self.active_view_layout_children(view);
            return self.desired_children_height(width, &children);
        }
        let mut children: Vec<&dyn Renderable> = Vec::with_capacity(5);
        if let Some(status) = &self.status {
            children.push(&STATUS_SEPARATOR);
            children.push(status);
        }
        if self.status.is_none() && !self.unified_exec_footer.is_empty() {
            children.push(&self.unified_exec_footer);
        }
        children.push(&self.pending_thread_approvals);
        children.push(&self.composer);
        let pending_queue = PendingQueueList::new(&self.pending_queue);
        if pending_queue.desired_height(width) > 0 {
            children.push(&pending_queue);
        }
        self.desired_children_height(width, &children)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if let Some(view) = self.active_view() {
            // When the view stacks below the composer, include composer height for
            // vertical offset but skip its caret so an unfocused draft does not
            // steal focus. Views that replace the composer start at the input area.
            let mut y = area.y;
            if let Some(status) = &self.status {
                for child in [&STATUS_SEPARATOR as &dyn Renderable, status] {
                    let height = child.desired_height(area.width);
                    y = y.saturating_add(height);
                }
            }
            if !view.replaces_composer() {
                y = y.saturating_add(self.composer.desired_height(area.width));
            }
            let view_area = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y))
                .intersection(area);
            return view.cursor_pos(view_area);
        }
        let mut children: Vec<&dyn Renderable> = Vec::with_capacity(5);
        if let Some(status) = &self.status {
            children.push(&STATUS_SEPARATOR);
            children.push(status);
        }
        if self.status.is_none() && !self.unified_exec_footer.is_empty() {
            children.push(&self.unified_exec_footer);
        }
        children.push(&self.pending_thread_approvals);
        children.push(&self.composer);
        // Queue renders below the composer and does not own the caret.
        self.child_cursor_pos(area, &children)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;

    use super::*;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;
    use crate::bottom_pane::list_selection_view::ListSelectionView;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::tui::frame_requester::FrameRequester;

    fn test_bottom_pane() -> BottomPane {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        BottomPane::new(BottomPaneParams {
            app_event_tx: AppEventSender::new(tx),
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: true,
            placeholder_text: "Ask Devo".to_string(),
            disable_paste_burst: true,
            skills: None,
            animations_enabled: false,
        })
    }

    fn render_bottom_pane(pane: &BottomPane, width: u16) -> String {
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        (0..area.height)
            .map(|row| {
                let mut line = String::new();
                for col in 0..area.width {
                    let symbol = buf[(area.x + col, area.y + row)].symbol();
                    if symbol.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(symbol);
                    }
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn active_view_stacks_below_composer_draft() {
        let mut pane = test_bottom_pane();
        let draft = "keep this draft visible";
        pane.set_text_content(draft.to_string(), Vec::new(), Vec::new());
        let composer_only_height = pane.desired_height(/*width*/ 80);
        let app_event_tx = pane.app_event_tx.clone();
        let accent = pane.accent_color;

        pane.open_popup_view(Box::new(ListSelectionView::new(
            SelectionViewParams {
                title: Some("Test Picker".to_string()),
                items: vec![SelectionItem {
                    name: "Option A".to_string(),
                    dismiss_on_select: true,
                    ..SelectionItem::default()
                }],
                ..SelectionViewParams::default()
            },
            app_event_tx,
            accent,
        )));

        let stacked_height = pane.desired_height(/*width*/ 80);
        assert!(
            stacked_height > composer_only_height,
            "stacked height {stacked_height} should exceed composer-only {composer_only_height}"
        );

        let rendered = render_bottom_pane(&pane, /*width*/ 80);
        let draft_row = rendered
            .lines()
            .position(|line| line.contains(draft))
            .expect("missing composer draft");
        let picker_row = rendered
            .lines()
            .position(|line| line.contains("Test Picker"))
            .expect("missing picker title");
        assert!(
            picker_row > draft_row,
            "picker should render below composer draft; draft_row={draft_row} picker_row={picker_row}\n{rendered}"
        );

        // Cursor must not land on the unfocused composer while a view is open.
        let area = Rect::new(0, 0, 80, stacked_height);
        let cursor = pane.cursor_pos(area);
        let composer_height = pane.composer.desired_height(80);
        if let Some((_, cursor_y)) = cursor {
            assert!(
                cursor_y >= composer_height,
                "cursor y={cursor_y} should stay in the panel below composer height {composer_height}"
            );
        } else {
            // Selection views may not expose a caret; None is fine.
            assert_eq!(cursor, None);
        }
    }

    #[test]
    fn model_picker_replaces_composer_input_area() {
        let mut pane = test_bottom_pane();
        let draft = "draft should not appear while model picker is open";
        pane.set_text_content(draft.to_string(), Vec::new(), Vec::new());

        pane.open_model_picker(vec![ModelPickerEntry {
            selection_value: "gpt".to_string(),
            display_name: "GPT".to_string(),
            right_hint: Some("OpenAI".to_string()),
            is_current: true,
            effort_options: vec![ModelPickerEffortOption {
                label: "High".to_string(),
                value: "high".to_string(),
            }],
            selected_effort: Some("high".to_string()),
        }]);

        let picker_height = pane.desired_height(/*width*/ 80);
        assert!(picker_height > 0);

        let rendered = render_bottom_pane(&pane, /*width*/ 80);
        assert!(rendered.contains("GPT"), "model row missing:\n{rendered}");
        assert!(
            !rendered.contains(draft),
            "composer draft should be hidden while model picker replaces input:\n{rendered}"
        );
        assert!(
            rendered.contains("[High]") || rendered.contains("High"),
            "effort chips missing:\n{rendered}"
        );
        assert!(
            !rendered.contains("Reasoning"),
            "effort strip should not show Reasoning label:\n{rendered}"
        );
    }

    #[test]
    fn context_occupancy_stacks_below_composer_draft() {
        let mut pane = test_bottom_pane();
        let draft = "keep draft while context panel is open";
        pane.set_text_content(draft.to_string(), Vec::new(), Vec::new());
        let composer_only_height = pane.desired_height(/*width*/ 80);

        pane.open_status_panel(
            None,
            SessionTokenTotals {
                input: 1_000,
                output: 100,
                cache_read: 500,
            },
            StatusPanelSnapshot {
                cwd: "/tmp/project".to_string(),
                permissions_label: "Ask for approval".to_string(),
            },
        );

        let stacked_height = pane.desired_height(/*width*/ 80);
        assert!(
            stacked_height > composer_only_height,
            "stacked height {stacked_height} should exceed composer-only {composer_only_height}"
        );

        let rendered = render_bottom_pane(&pane, /*width*/ 80);
        assert!(
            rendered.contains(draft),
            "composer draft should stay visible:\n{rendered}"
        );
        assert!(
            rendered.contains("Status"),
            "status panel title missing:\n{rendered}"
        );
        assert!(
            rendered.contains("Context Usage"),
            "context usage section missing:\n{rendered}"
        );
        assert!(
            rendered.contains("Token Usage"),
            "token usage section missing:\n{rendered}"
        );
        assert!(
            !rendered.lines().any(|line| line.trim() == "Session"),
            "Session heading should be gone:\n{rendered}"
        );
        let draft_row = rendered
            .lines()
            .position(|line| line.contains(draft))
            .expect("missing composer draft");
        let panel_row = rendered
            .lines()
            .position(|line| line.contains("Status"))
            .expect("missing status title");
        assert!(
            panel_row > draft_row,
            "status panel should render below composer; draft_row={draft_row} panel_row={panel_row}\n{rendered}"
        );
    }

    #[test]
    fn pending_queue_stacks_below_composer_and_supports_edit_key() {
        let mut pane = test_bottom_pane();
        let draft = "composer draft";
        pane.set_text_content(draft.to_string(), Vec::new(), Vec::new());
        pane.replace_pending_queue(vec![PendingQueueItem {
            queue_item_id: "q1".into(),
            text: "queued\nmulti".into(),
        }]);
        let rendered = render_bottom_pane(&pane, /*width*/ 80);
        let draft_row = rendered
            .lines()
            .position(|line| line.contains(draft))
            .expect("missing composer draft");
        let queue_row = rendered
            .lines()
            .position(|line| line.contains("1 ›") && line.contains("queued multi"))
            .expect("missing queue row");
        assert!(
            queue_row > draft_row,
            "queue should render below composer; draft_row={draft_row} queue_row={queue_row}\n{rendered}"
        );

        pane.set_text_content(String::new(), Vec::new(), Vec::new());
        assert_eq!(
            pane.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            InputResult::None
        );
        assert!(pane.pending_queue_focused());
        assert_eq!(
            pane.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            InputResult::QueueSteer {
                queue_item_id: "q1".into(),
            }
        );
        assert_eq!(
            pane.handle_key_event(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            InputResult::QueueEdit {
                queue_item_id: "q1".into(),
                text: "queued\nmulti".into(),
            }
        );
        assert_eq!(
            pane.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            InputResult::QueueRemove {
                queue_item_id: "q1".into(),
            }
        );
    }

    #[test]
    fn composer_placeholder_rotates_working_tips() {
        use crate::status_indicator_widget::WORKING_TIPS;

        let mut pane = test_bottom_pane();
        assert_eq!(pane.placeholder_text(), format!("Tip: {}", WORKING_TIPS[0]));

        pane.set_placeholder_tips_elapsed_for_test(Duration::from_secs(6));
        assert_eq!(pane.placeholder_text(), format!("Tip: {}", WORKING_TIPS[1]));

        pane.set_default_placeholder();
        assert_eq!(
            pane.placeholder_text(),
            format!("Tip: {}", WORKING_TIPS[1]),
            "default placeholder should keep the current rotating tip"
        );
    }
}
