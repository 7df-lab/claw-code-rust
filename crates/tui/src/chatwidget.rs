//! Devo TUI chat surface.
//!
//! `ChatWidget` owns the v2 conversation surface: committed history cells, the
//! active bottom input pane, and the Claw-local app events produced by user
//! interaction. Protocol reasoning choices come from `devo_protocol`
//! through `Model` instead of a TUI-local reasoning enum.

use std::cell::Cell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use devo_core::ItemId;
use devo_protocol::AcpAvailableCommand;
use devo_protocol::AcpCost;
use devo_protocol::AcpSessionConfigOption;
use devo_protocol::Model;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::user_input::TextElement;
use ratatui::style::Color;
use ratatui::text::Line;

use devo_protocol::TurnId;

use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPane;
use crate::bottom_pane::BottomPaneParams;
use crate::bottom_pane::InputMode;
use crate::bottom_pane::LocalImageAttachment;
use crate::bottom_pane::MentionBinding;
use crate::events::SavedModelEntry;
use crate::history_cell::HistoryCell;
use crate::onboarding_widget::OnboardingWidget;
use crate::startup_header::STARTUP_HEADER_ANIMATION_INTERVAL;
use crate::startup_logo_cell::StartupLogoCell;
use crate::streaming::chunking::AdaptiveChunkingPolicy;
use crate::theme::ThemeSet;
use crate::tui::frame_requester::FrameRequester;

mod diff_rules;

mod configuration;

mod goal;

mod input;

mod render;

mod session_history;

mod selection;

mod slash_commands;

mod restored_session;

mod session_header;

mod subagent_monitor;

mod subagent_debug;
mod subagent_live_list;

mod permission_presets;

mod resume_browser;

mod sandbox_profiles;

mod text_stream;

mod transcript_view;

mod reasoning_effort;

mod reasoning_view;

mod mcp_picker;

mod skills_picker;

mod worker_events;

use self::permission_presets::permission_preset_items;
use self::permission_presets::permission_preset_label;
use self::resume_browser::ResumeBrowserState;
use self::sandbox_profiles::sandbox_profile_label;
use self::session_header::SessionHeaderParams;
use self::subagent_monitor::SubagentMonitorState;

use self::text_stream::ActiveTextItem;

pub(crate) const MCP_SERVERS_TRANSCRIPT_TITLE: &str = "⬡  MCP Servers";

#[cfg(test)]
pub(crate) use self::reasoning_effort::ReasoningEffortListEntry;
pub(crate) use self::transcript_view::ActiveCellTranscriptKey;
pub(crate) use self::transcript_view::TranscriptOverlayCell;

/// Common initialization parameters shared by `ChatWidget` constructors.
pub(crate) struct ChatWidgetInit {
    pub(crate) frame_requester: FrameRequester,
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) initial_session: TuiSessionState,
    pub(crate) initial_reasoning_effort_selection: Option<String>,
    pub(crate) initial_permission_preset: devo_protocol::PermissionPreset,
    pub(crate) initial_sandbox_profile: Option<String>,
    pub(crate) initial_user_message: Option<UserMessage>,
    pub(crate) enhanced_keys_supported: bool,
    pub(crate) is_first_run: bool,
    pub(crate) available_models: Vec<Model>,
    /// Configured model bindings from config.toml used by the /model picker.
    pub(crate) saved_models: Vec<SavedModelEntry>,
    pub(crate) show_model_onboarding: bool,
    pub(crate) exit_after_onboarding: bool,
    pub(crate) startup_tooltip_override: Option<String>,
    pub(crate) initial_theme_name: Option<String>,
    pub(crate) initial_collapse_reasoning: bool,
}

/// Resolved runtime session projection owned by the chat widget.
///
/// Unlike `InitialTuiSession`, this is internal TUI state: the model slug has already been resolved
/// into model metadata when available, and provider is derived from that projection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TuiSessionState {
    pub(crate) cwd: PathBuf,
    pub(crate) model: Option<Model>,
    pub(crate) request_model: Option<String>,
    pub(crate) model_binding_id: Option<String>,
    pub(crate) provider: Option<ProviderWireApi>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) active_agent_label: Option<String>,
}

impl TuiSessionState {
    pub(crate) fn new(cwd: PathBuf, model: Option<Model>) -> Self {
        let provider = model.as_ref().map(Model::provider_wire_api);
        Self {
            cwd,
            model,
            request_model: None,
            model_binding_id: None,
            provider,
            reasoning_effort: None,
            active_agent_label: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ExternalEditorState {
    #[default]
    Closed,
    Requested,
    Active,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct UserMessage {
    pub(crate) text: String,
    pub(crate) local_images: Vec<LocalImageAttachment>,
    pub(crate) remote_image_urls: Vec<String>,
    pub(crate) text_elements: Vec<TextElement>,
    pub(crate) mention_bindings: Vec<MentionBinding>,
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

impl From<&str> for UserMessage {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum OnboardingStep {
    ModelName,
    BaseUrl {
        model: String,
    },
    ApiKey {
        model: String,
        base_url: Option<String>,
    },
    Validating {
        model: String,
        base_url: Option<String>,
        api_key: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct ActiveToolCall {
    tool_use_id: String,
    tool_name: Option<String>,
    seq: u64,
    input: Option<serde_json::Value>,
    title: String,
    lines: Vec<Line<'static>>,
    output: String,
    exec_like: bool,
    start_time: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DotStatus {
    Pending,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingApprovalRequest {
    session_id: devo_protocol::SessionId,
    turn_id: TurnId,
    approval_id: String,
    action_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveProposedPlan {
    item_id: ItemId,
    text: String,
}

pub(crate) struct ChatWidget {
    // App event, such as UserTurn, List Sessions, New Session, Onboard or Browser Input History
    app_event_tx: AppEventSender,
    // Frame requester for scheduling future frame draws on the TUI event loop.
    frame_requester: FrameRequester,
    // The session state utlized for TUI rendering, currently simple: cwd, Model, ProviderWireApi
    // TODO: Shoule expland the session state, and move reasoning_effort_selection into session state.
    session: TuiSessionState,
    reasoning_effort_selection: Option<String>,
    // sub widget, bottom pane, including such input textarea, slash command popup, status summary.
    bottom_pane: BottomPane,
    active_cell: Option<Box<dyn HistoryCell>>,
    active_cell_revision: u64,
    last_terminal_assistant_visible_hash: Option<(String, u64)>,
    active_tool_calls: HashMap<String, ActiveToolCall>,
    pending_tool_calls: Vec<ActiveToolCall>,
    history: Vec<Box<dyn HistoryCell>>,
    next_history_flush_index: usize,
    queued_user_messages: VecDeque<UserMessage>,
    external_editor_state: ExternalEditorState,
    status_message: String,
    active_text_items: Vec<ActiveTextItem>,
    stream_chunking_policy: AdaptiveChunkingPolicy,
    available_models: Vec<Model>,
    saved_models: Vec<SavedModelEntry>,
    current_model_binding_id: Option<String>,
    acp_available_commands: Vec<AcpAvailableCommand>,
    acp_current_mode_id: Option<String>,
    acp_config_options: Vec<AcpSessionConfigOption>,
    acp_usage: Option<(u64, u64, Option<AcpCost>)>,
    onboarding: Option<OnboardingWidget>,
    exit_after_onboarding: bool,
    resume_browser: Option<ResumeBrowserState>,
    resume_browser_loading: bool,
    resuming_session: bool,
    subagent_monitor: SubagentMonitorState,
    theme_set: ThemeSet,
    active_theme_name: String,
    collapse_reasoning: bool,
    resume_browser_last_height: Cell<u16>,
    turn_count: usize,
    total_input_tokens: usize,
    total_output_tokens: usize,
    total_cache_read_tokens: usize,
    prompt_token_estimate: usize,
    last_query_input_tokens: usize,
    last_query_total_tokens: usize,
    last_context_occupancy: Option<devo_protocol::canonical::item::ContextOccupancy>,
    last_plan_progress: Option<(usize, usize)>,
    queued_count: usize,
    queued_input_modes: VecDeque<InputMode>,
    promoted_input_modes: VecDeque<InputMode>,
    active_turn_id: Option<TurnId>,
    failed_turn_visually_finalized: bool,
    current_turn_mode: InputMode,
    committed_server_assistant_in_turn: bool,
    boundary_committed_assistant_items: HashSet<ItemId>,
    current_turn_has_user_shell_command: bool,
    pending_approval: Option<PendingApprovalRequest>,
    active_proposed_plan: Option<ActiveProposedPlan>,
    pending_proposed_plan_actions: bool,
    permission_preset: devo_protocol::PermissionPreset,
    sandbox_profile: Option<String>,
    busy: bool,
    selection_mode: bool,
    selected_user_cell_index: Option<usize>,
    user_cell_history_indices: Vec<usize>,
    startup_header_mascot_frame_index: usize,
    startup_header_next_animation_at: Instant,
    next_seq: u64,
    /// Merged config + runtime snapshot for the interactive `/mcps` flow.
    mcp_servers_snapshot: Option<Vec<crate::mcp_picker::McpPickerServer>>,
    /// After enable/disable, reopen this server's detail once list refreshes.
    mcp_reopen_detail: Option<String>,
    /// Snapshot for the interactive `/skills` flow.
    skills_snapshot: Option<Vec<crate::skills_picker::SkillPickerEntry>>,
    /// After enable/disable, reopen this skill's detail once list refreshes.
    skills_reopen_detail: Option<String>,
}

impl ChatWidget {
    fn reserve_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    pub(super) fn begin_session_resume(&mut self) {
        self.resuming_session = true;
        self.bottom_pane.ensure_status_indicator();
        if let Some(status) = self.bottom_pane.status_widget_mut() {
            status.update_header("Resuming session...".to_string());
            status.set_interrupt_hint_visible(false);
            status.set_working_tip_visible(false);
            status.update_inline_message(None);
        }
        self.bottom_pane.set_composer_input_enabled(
            /*enabled*/ false,
            Some("Resuming session...".to_string()),
        );
        self.set_status_message("Resuming session...");
    }

    pub(super) fn finish_session_resume(&mut self) {
        if !self.resuming_session {
            return;
        }
        self.resuming_session = false;
        self.bottom_pane.hide_status_indicator();
        self.bottom_pane
            .set_composer_input_enabled(/*enabled*/ true, /*placeholder*/ None);
    }

    pub(super) fn block_input_during_resume(&mut self) -> bool {
        if !self.resuming_session {
            return false;
        }
        self.set_status_message("Cannot send while resuming session");
        true
    }

    #[cfg(test)]
    pub(crate) fn is_resuming_session_for_test(&self) -> bool {
        self.resuming_session
    }

    #[cfg(test)]
    pub(crate) fn status_message_for_test(&self) -> &str {
        &self.status_message
    }

    #[cfg(test)]
    pub(crate) fn status_indicator_header_for_test(&self) -> Option<&str> {
        self.bottom_pane
            .status_widget()
            .map(crate::status_indicator_widget::StatusIndicatorWidget::header)
    }

    #[cfg(test)]
    pub(crate) fn has_bottom_pane_view_for_test(&self) -> bool {
        self.bottom_pane.has_view_for_test()
    }
}

impl ChatWidget {
    fn format_git_diff_result(result: std::io::Result<(bool, String)>) -> String {
        diff_rules::format_git_diff_result(result)
    }

    pub(crate) fn should_auto_show_git_diff(tool_title: &str, is_error: bool) -> bool {
        diff_rules::should_auto_show_git_diff(tool_title, is_error)
    }

    pub(crate) fn should_auto_show_git_diff_for_turn(
        &self,
        tool_title: &str,
        is_error: bool,
    ) -> bool {
        diff_rules::should_auto_show_git_diff(tool_title, is_error)
    }
    pub(crate) fn new_with_app_event(common: ChatWidgetInit) -> Self {
        // Pull the constructor inputs apart up front so the setup below reads in stages.
        let ChatWidgetInit {
            frame_requester,
            app_event_tx,
            initial_session,
            initial_reasoning_effort_selection,
            initial_permission_preset,
            initial_sandbox_profile,
            initial_user_message,
            enhanced_keys_supported,
            is_first_run,
            available_models,
            saved_models,
            show_model_onboarding,
            exit_after_onboarding,
            startup_tooltip_override,
            initial_theme_name,
            initial_collapse_reasoning,
        } = common;

        // Prefer an explicit startup selection, but fall back to the model's default reasoning effort.
        let reasoning_effort_selection = initial_reasoning_effort_selection.or_else(|| {
            initial_session
                .model
                .as_ref()
                .and_then(Model::default_reasoning_effort_selection)
        });

        // Queue any startup user message so it is processed through the same path as normal input.
        let mut queued_user_messages = VecDeque::new();
        if let Some(initial_user_message) = initial_user_message {
            queued_user_messages.push_back(initial_user_message);
        }

        let theme_set = ThemeSet::default();
        let active_theme_name = initial_theme_name
            .filter(|name| theme_set.find(name).is_some())
            .unwrap_or_else(|| ThemeSet::default_theme().to_string());
        let initial_accent_color = theme_set
            .find(&active_theme_name)
            .map(|t| t.accent_color)
            .unwrap_or(Color::Cyan);

        // Build the bottom composer first, since the widget delegates all live input handling there.
        let mut bottom_pane = BottomPane::new(BottomPaneParams {
            app_event_tx: app_event_tx.clone(),
            frame_requester: frame_requester.clone(),
            has_input_focus: true,
            enhanced_keys_supported,
            placeholder_text: "Ask Devo".to_string(),
            disable_paste_burst: false,
            skills: None,
            animations_enabled: true,
        });
        bottom_pane.set_accent_color(initial_accent_color);
        bottom_pane.set_active_agent_label(initial_session.active_agent_label.clone());

        let history: Vec<Box<dyn HistoryCell>> = if show_model_onboarding {
            vec![Box::new(StartupLogoCell::new(initial_accent_color))]
        } else {
            vec![Self::build_header_box(SessionHeaderParams {
                cwd: &initial_session.cwd,
                model: initial_session.model.as_ref(),
                request_model: initial_session.request_model.as_deref(),
                reasoning_effort_selection: reasoning_effort_selection.as_deref(),
                is_first_run,
                startup_tooltip_override,
                accent_color: initial_accent_color,
                mascot_frame_index: 0,
            })]
        };

        let current_model_binding_id = initial_session.model_binding_id.clone().or_else(|| {
            saved_models.iter().find_map(|entry| {
                let model = initial_session.model.as_ref()?;
                (entry.model == model.slug && entry.request_model == initial_session.request_model)
                    .then(|| entry.binding_id.clone())
                    .flatten()
            })
        });

        // Assemble the full widget state from the initial session, composer, history, and queues.
        let mut widget = Self {
            app_event_tx,
            frame_requester,
            session: initial_session,
            reasoning_effort_selection,
            bottom_pane,
            active_cell: None,
            active_cell_revision: 0,
            last_terminal_assistant_visible_hash: None,
            active_tool_calls: HashMap::new(),
            pending_tool_calls: Vec::new(),
            history,
            next_history_flush_index: 0,
            queued_user_messages,
            external_editor_state: ExternalEditorState::Closed,
            status_message: "Ready".to_string(),
            active_text_items: Vec::new(),
            stream_chunking_policy: AdaptiveChunkingPolicy::default(),
            available_models,
            current_model_binding_id,
            saved_models,
            acp_available_commands: Vec::new(),
            acp_current_mode_id: None,
            acp_config_options: Vec::new(),
            acp_usage: None,
            onboarding: None,
            exit_after_onboarding,
            resume_browser: None,
            resume_browser_loading: false,
            resuming_session: false,
            subagent_monitor: SubagentMonitorState::default(),
            theme_set,
            active_theme_name,
            collapse_reasoning: initial_collapse_reasoning,
            resume_browser_last_height: Cell::new(0),
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            last_query_input_tokens: 0,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            last_plan_progress: None,
            queued_count: 0,
            queued_input_modes: VecDeque::new(),
            promoted_input_modes: VecDeque::new(),
            active_turn_id: None,
            failed_turn_visually_finalized: false,
            current_turn_mode: InputMode::Build,
            committed_server_assistant_in_turn: false,
            boundary_committed_assistant_items: HashSet::new(),
            current_turn_has_user_shell_command: false,
            pending_approval: None,
            active_proposed_plan: None,
            pending_proposed_plan_actions: false,
            permission_preset: initial_permission_preset,
            sandbox_profile: initial_sandbox_profile,
            busy: false,
            selection_mode: false,
            selected_user_cell_index: None,
            user_cell_history_indices: Vec::new(),
            startup_header_mascot_frame_index: 0,
            startup_header_next_animation_at: Instant::now() + STARTUP_HEADER_ANIMATION_INTERVAL,
            next_seq: 0,
            mcp_servers_snapshot: None,
            mcp_reopen_detail: None,
            skills_snapshot: None,
            skills_reopen_detail: None,
        };

        // Model onboarding can inject additional startup UI before the first frame is drawn.
        if show_model_onboarding {
            widget.onboarding = Some(OnboardingWidget::new(
                &widget.available_models,
                widget.app_event_tx.clone(),
                widget.frame_requester.clone(),
                true, /* animations_enabled */
            ));
            widget.bottom_pane.set_composer_input_enabled(
                /*enabled*/ false,
                Some("Complete onboarding to start chatting".to_string()),
            );
            widget.set_status_message("Onboarding");
        }

        // Keep the bottom pane summary in sync with the assembled widget state.
        widget.sync_bottom_pane_summary();
        widget.maybe_start_subagent_debug_scenario();
        widget
    }
}
