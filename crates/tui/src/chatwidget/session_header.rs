//! Session header, status summary, and shared line-formatting helpers for `ChatWidget`.
//!
//! The chat widget owns the session state, while this module keeps header refresh,
//! token summary text, and small shared rendering helpers out of the root file.

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use devo_protocol::Model;
use devo_protocol::ProviderWireApi;
use devo_util_git::current_branch_name;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::app_event::AppEvent;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::startup_header::STARTUP_HEADER_ANIMATION_INTERVAL;
use crate::ui_consts::COMPLETED_COLOR;
use crate::ui_consts::REASONING_ACCENT_COLOR;

use super::ChatWidget;
use super::DotStatus;
use super::STATUS_LINE_BRANCH_REFRESH_INTERVAL;

use crate::ui_consts::REPLY_MARKER_COLOR;
/// Blue used for running/active state text.
pub(super) const RUNNING_COLOR: Color = Color::Rgb(106, 200, 255);
/// Red used for failed/interrupted state.
pub(super) const FAILED_COLOR: Color = Color::Rgb(255, 100, 100);
/// Muted grey for secondary/unknown status text.
pub(super) const MUTED_COLOR: Color = Color::Rgb(160, 163, 168);
/// Subtle grey for preview text (only in subagent live list).
pub(super) const PREVIEW_COLOR: Color = Color::Rgb(176, 184, 196);

pub(super) struct SessionHeaderParams<'a> {
    pub cwd: &'a Path,
    pub model: Option<&'a Model>,
    pub request_model: Option<&'a str>,
    pub reasoning_effort_selection: Option<&'a str>,
    pub is_first_run: bool,
    pub startup_tooltip_override: Option<String>,
    pub accent_color: Color,
    pub mascot_frame_index: usize,
}

pub(super) fn is_web_search_title(title: &str) -> bool {
    title.starts_with("Web Search(") || title.starts_with("Web Fetch(")
}

impl ChatWidget {
    pub(super) fn is_blank_line(line: &Line<'_>) -> bool {
        line.spans.iter().all(|span| span.content.trim().is_empty())
    }

    pub(super) fn build_header_box(params: SessionHeaderParams<'_>) -> Box<dyn HistoryCell> {
        let model = params.model.cloned().unwrap_or_else(|| Model {
            slug: "unknown".to_string(),
            display_name: "unknown".to_string(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            ..Model::default()
        });
        let header_model = params
            .request_model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .or_else(|| {
                let display_name = model.display_name.trim();
                (!display_name.is_empty()).then_some(display_name)
            })
            .unwrap_or(model.slug.as_str())
            .to_string();
        Box::new(history_cell::new_session_info(
            params.cwd,
            header_model.as_str(),
            header_model.clone(),
            header_model.clone(),
            model.reasoning_capability.clone(),
            model
                .resolve_reasoning_effort_selection(params.reasoning_effort_selection)
                .effective_reasoning_effort,
            model.reasoning_implementation.clone(),
            params.is_first_run,
            params.startup_tooltip_override,
            /*show_fast_status*/ false,
            params.accent_color,
            params.mascot_frame_index,
        ))
    }

    pub(super) fn trim_trailing_blank_lines(lines: &mut Vec<Line<'static>>) {
        while lines
            .last()
            .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        {
            lines.pop();
        }
    }

    pub(super) fn muted_dot_prefix() -> Line<'static> {
        Line::from(vec![Span::styled("▌", Style::default().dim()), " ".into()])
    }

    /// Accent marker for assistant reply text (live and committed).
    pub(super) fn reply_dot_prefix() -> Line<'static> {
        Self::pending_dot_prefix()
    }

    pub(super) fn completed_dot_prefix() -> Line<'static> {
        Self::muted_dot_prefix()
    }

    pub(super) fn pending_dot_prefix() -> Line<'static> {
        Line::from(vec![
            Span::styled("▌", Style::default().fg(REPLY_MARKER_COLOR)),
            " ".into(),
        ])
    }

    pub(super) fn reasoning_dot_prefix(_status: DotStatus) -> Line<'static> {
        Self::muted_dot_prefix()
    }

    pub(super) fn truncate_display_text(value: &str, max_width: usize) -> String {
        let total_width = unicode_width::UnicodeWidthStr::width(value);
        if total_width <= max_width {
            return value.to_string();
        }
        if max_width == 0 {
            return String::new();
        }
        if max_width <= 3 {
            return ".".repeat(max_width);
        }

        let target_width = max_width.saturating_sub(3);
        let mut rendered = String::new();
        let mut rendered_width = 0usize;
        for ch in value.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if rendered_width.saturating_add(ch_width) > target_width {
                break;
            }
            rendered.push(ch);
            rendered_width = rendered_width.saturating_add(ch_width);
        }
        rendered.push_str("...");
        rendered
    }

    pub(super) fn pad_display_text(value: &str, target_width: usize) -> String {
        let width = unicode_width::UnicodeWidthStr::width(value);
        if width >= target_width {
            return value.to_string();
        }
        format!("{value}{}", " ".repeat(target_width - width))
    }

    pub(super) fn tool_text_style() -> Style {
        Style::default().fg(MUTED_COLOR)
    }

    pub(super) fn tool_status_running_style() -> Style {
        Style::default().fg(RUNNING_COLOR).bold()
    }

    pub(super) fn tool_status_done_style() -> Style {
        Style::default().fg(COMPLETED_COLOR).bold()
    }

    pub(super) fn running_tool_line(title: &str) -> Line<'static> {
        let normalized = title
            .strip_prefix("Running ")
            .or_else(|| title.strip_prefix("Ran "))
            .unwrap_or(title);
        if is_web_search_title(normalized) {
            return Line::from(Span::styled(
                normalized.to_string(),
                Self::tool_text_style(),
            ));
        }
        Line::from(vec![
            Span::styled("Running ", Self::tool_status_running_style()),
            Span::styled(normalized.to_string(), Self::tool_text_style()),
        ])
    }

    pub(super) fn ran_tool_line(title: &str) -> Line<'static> {
        let normalized = title
            .strip_prefix("Running ")
            .or_else(|| title.strip_prefix("Ran "))
            .unwrap_or(title);
        if is_web_search_title(normalized) {
            return Line::from(Span::styled(
                normalized.to_string(),
                Self::tool_text_style(),
            ));
        }
        Line::from(vec![
            Span::styled("Ran ", Self::tool_status_done_style()),
            Span::styled(normalized.to_string(), Self::tool_text_style()),
        ])
    }

    pub(super) fn tool_dot_prefix() -> Line<'static> {
        Self::muted_dot_prefix()
    }

    pub(super) fn failed_dot_prefix() -> Line<'static> {
        Self::muted_dot_prefix()
    }

    pub(super) fn dot_prefix(&self, status: DotStatus) -> Line<'static> {
        match status {
            DotStatus::Pending => Self::reply_dot_prefix(),
            DotStatus::Completed => Self::reply_dot_prefix(),
            DotStatus::Failed => Self::reply_dot_prefix(),
        }
    }

    pub(super) fn format_token_count(value: usize) -> String {
        if value >= 1_000_000 {
            format!("{:.1}M", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.1}k", value as f64 / 1_000.0)
        } else {
            value.to_string()
        }
    }

    /// Context length for the status bar.
    ///
    /// `used` follows the latest query display total from `TurnUsageUpdated` so
    /// the bar moves mid-turn. The denominator prefers the live session
    /// effective-context override (Settings › Compaction threshold), then the
    /// occupancy snapshot window, then the model effective window. Occupancy
    /// `total_tokens` is only a fallback when no last-query usage has arrived
    /// yet (for example a hydrate that set occupancy alone).
    pub(super) fn context_usage(&self) -> Option<(usize, usize, usize)> {
        let total = if let Some(limit) = self.effective_context_window {
            limit as usize
        } else if let Some(occupancy) = self.last_context_occupancy.as_ref() {
            occupancy.context_window_tokens as usize
        } else {
            self.session.model.as_ref()?.effective_context_window() as usize
        };
        let used = if self.last_query_total_tokens > 0 {
            self.last_query_total_tokens
        } else if self.prompt_token_estimate > 0 {
            self.prompt_token_estimate
        } else if let Some(occupancy) = self.last_context_occupancy.as_ref() {
            occupancy.total_tokens as usize
        } else {
            0
        };
        let percent = Self::usage_percent(used, total);
        Some((used, total, percent))
    }

    fn usage_percent(used: usize, total: usize) -> usize {
        if total == 0 {
            0
        } else {
            ((used as f64 / total as f64) * 100.0)
                .clamp(0.0, 100.0)
                .round() as usize
        }
    }

    pub(super) fn format_compact_token_count(value: usize) -> String {
        let mut rendered = String::new();
        Self::push_compact_token_count(&mut rendered, value);
        rendered
    }

    fn push_compact_token_count(rendered: &mut String, value: usize) {
        if value >= 1_000_000 {
            write!(rendered, "{:.1}M", value as f64 / 1_000_000.0)
                .expect("writing to String should not fail");
        } else if value >= 1_000 {
            write!(rendered, "{:.1}k", value as f64 / 1_000.0)
                .expect("writing to String should not fail");
        } else {
            write!(rendered, "{value}").expect("writing to String should not fail");
        }
    }

    pub(super) fn render_progress_bar(used: usize, total: usize, bar_width: usize) -> String {
        let mut rendered = String::with_capacity(bar_width.saturating_mul(3).saturating_add(5));
        Self::push_progress_bar(&mut rendered, used, total, bar_width);
        rendered
    }

    fn push_progress_bar(rendered: &mut String, used: usize, total: usize, bar_width: usize) {
        if total == 0 {
            return;
        }
        let ratio = (used as f64 / total as f64).clamp(0.0, 1.0);
        let filled = (ratio * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        for _ in 0..filled {
            rendered.push('▰');
        }
        for _ in 0..empty {
            rendered.push('▱');
        }
        let pct = (ratio * 100.0).round() as usize;
        write!(rendered, " {pct}%").expect("writing to String should not fail");
    }

    pub(super) fn session_summary_text(&self) -> String {
        self.session_summary_text_with_context(/*include_context*/ true)
    }

    fn session_summary_text_with_context(&self, include_context: bool) -> String {
        let model = self.model_display_name();
        let mut summary = String::with_capacity(model.len().saturating_add(64));
        summary.push_str(model);
        if let Some(branch) = self.status_line_branch.as_deref() {
            summary.push_str(" · ");
            summary.push_str(branch);
        }

        if include_context && let Some(context) = self.session_context_summary_text() {
            summary.push_str("  ");
            summary.push_str(&context);
        }
        summary
    }

    fn session_context_summary_text(&self) -> Option<String> {
        let (used, total, _percent) = self.context_usage()?;
        let mut context = String::with_capacity(48);
        Self::push_progress_bar(&mut context, used, total, 10);
        context.push(' ');
        Self::push_compact_token_count(&mut context, used);
        context.push('/');
        Self::push_compact_token_count(&mut context, total);
        Some(context)
    }

    pub(super) fn model_display_name(&self) -> &str {
        self.session
            .request_model
            .as_deref()
            .or_else(|| {
                self.session
                    .model
                    .as_ref()
                    .map(|model| model.display_name.as_str())
            })
            .unwrap_or("unknown")
    }

    pub(super) fn sync_bottom_pane_summary(&mut self) {
        // Left: mode · model · branch. Right: context meter (right-aligned).
        let summary = self.session_summary_text_with_context(/*include_context*/ false);
        self.bottom_pane
            .set_status_line(Some(Line::from(summary).dim()));
        self.bottom_pane.set_status_line_enabled(true);
        let context_label = if self.session.active_agent_label.is_none() {
            self.session_context_summary_text()
        } else {
            None
        };
        self.bottom_pane.set_context_window_label(context_label);
    }

    fn status_line_cwd(&self) -> &Path {
        self.session.cwd.as_path()
    }

    /// Forces a git-branch refresh for the footer status line.
    pub(super) fn request_status_line_branch_refresh(&mut self) {
        let cwd = self.status_line_cwd().to_path_buf();
        if self
            .status_line_branch_cwd
            .as_ref()
            .is_none_or(|path| path != &cwd)
        {
            self.status_line_branch_cwd = Some(cwd.clone());
            self.status_line_branch = None;
            self.status_line_branch_pending = false;
        }
        self.request_status_line_branch(cwd);
    }

    /// Light periodic refresh so external `git checkout` stays visible.
    pub(super) fn maybe_refresh_status_line_branch(&mut self) {
        let now = Instant::now();
        if now < self.status_line_branch_next_refresh_at {
            return;
        }
        self.status_line_branch_next_refresh_at = now + STATUS_LINE_BRANCH_REFRESH_INTERVAL;
        self.request_status_line_branch_refresh();
    }

    fn request_status_line_branch(&mut self, cwd: PathBuf) {
        if self.status_line_branch_pending {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // Unit tests construct widgets without a Tokio runtime.
            return;
        };
        self.status_line_branch_pending = true;
        let tx = self.app_event_tx.clone();
        handle.spawn(async move {
            let branch = current_branch_name(&cwd).await;
            tx.send(AppEvent::StatusLineBranchUpdated { cwd, branch });
        });
    }

    pub(super) fn apply_status_line_branch_update(&mut self, cwd: PathBuf, branch: Option<String>) {
        if self
            .status_line_branch_cwd
            .as_ref()
            .is_some_and(|path| path != &cwd)
        {
            // Stale completion after a cwd change.
            return;
        }
        self.status_line_branch_pending = false;
        self.status_line_branch_cwd = Some(cwd);
        if self.status_line_branch == branch {
            return;
        }
        self.status_line_branch = branch;
        self.sync_bottom_pane_summary();
    }

    pub(super) fn push_session_header(
        &mut self,
        is_first_run: bool,
        startup_tooltip_override: Option<String>,
    ) {
        self.history
            .push(self.build_current_header_box(is_first_run, startup_tooltip_override));
    }

    pub(super) fn build_current_header_box(
        &self,
        is_first_run: bool,
        startup_tooltip_override: Option<String>,
    ) -> Box<dyn HistoryCell> {
        let accent = self.active_accent_color();
        Self::build_header_box(SessionHeaderParams {
            cwd: &self.session.cwd,
            model: self.session.model.as_ref(),
            request_model: self.session.request_model.as_deref(),
            reasoning_effort_selection: self.reasoning_effort_selection.as_deref(),
            is_first_run,
            startup_tooltip_override,
            accent_color: accent,
            mascot_frame_index: self.startup_header_mascot_frame_index,
        })
    }

    pub(super) fn history_has_non_header_content(&self) -> bool {
        self.history.iter().any(|cell| {
            cell.as_any()
                .downcast_ref::<history_cell::SessionInfoCell>()
                .is_none()
        })
    }

    pub(super) fn last_known_width(&self) -> u16 {
        crossterm::terminal::size()
            .map(|(width, _height)| width)
            .unwrap_or(80)
    }

    pub(super) fn refresh_header_box(&mut self) {
        if self
            .history
            .first()
            .and_then(|cell| {
                cell.as_any()
                    .downcast_ref::<history_cell::SessionInfoCell>()
            })
            .is_none()
        {
            return;
        }
        let accent = self.active_accent_color();
        self.history[0] = Self::build_header_box(SessionHeaderParams {
            cwd: &self.session.cwd,
            model: self.session.model.as_ref(),
            request_model: self.session.request_model.as_deref(),
            reasoning_effort_selection: self.reasoning_effort_selection.as_deref(),
            is_first_run: false,
            startup_tooltip_override: None,
            accent_color: accent,
            mascot_frame_index: self.startup_header_mascot_frame_index,
        });
    }

    pub(super) fn advance_startup_header_animation(&mut self) {
        let now = Instant::now();
        if self
            .history
            .first()
            .and_then(|cell| {
                cell.as_any()
                    .downcast_ref::<history_cell::SessionInfoCell>()
            })
            .is_none()
        {
            return;
        }

        self.frame_requester
            .schedule_frame_in(STARTUP_HEADER_ANIMATION_INTERVAL);
        if now < self.startup_header_next_animation_at {
            return;
        }

        self.startup_header_mascot_frame_index = (self.startup_header_mascot_frame_index + 1) % 3;
        self.startup_header_next_animation_at = now + STARTUP_HEADER_ANIMATION_INTERVAL;
        self.refresh_header_box();
    }

    pub(crate) fn current_model(&self) -> Option<&Model> {
        self.session.model.as_ref()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn current_cwd(&self) -> &std::path::Path {
        &self.session.cwd
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn startup_header_mascot_frame_index(&self) -> usize {
        self.startup_header_mascot_frame_index
    }

    #[cfg(test)]
    pub(crate) fn has_live_assistant_text(&self) -> bool {
        self.active_text_items
            .iter()
            .any(|item| item.kind == crate::events::TextItemKind::Assistant)
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn force_startup_header_animation_due(&mut self) {
        self.startup_header_next_animation_at = Instant::now();
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn force_task_elapsed_seconds(&mut self, secs: u64) {
        self.bottom_pane.set_task_running(true);
        if let Some(status) = self.bottom_pane.status_widget_mut() {
            let now = Instant::now();
            status.pause_timer_at(now);
            let resume_at = now
                .checked_sub(std::time::Duration::from_secs(secs))
                .unwrap_or(now);
            status.resume_timer_at(resume_at);
        }
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn placeholder_text(&self) -> &str {
        self.bottom_pane.placeholder_text()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn status_summary_text(&self) -> String {
        self.session_summary_text()
    }

    pub(super) fn reasoning_text_style() -> Style {
        Style::default().dim()
    }

    pub(super) fn reasoning_heading_style() -> Style {
        Style::default().italic().fg(REASONING_ACCENT_COLOR)
    }

    /// Completed reasoning uses a muted style so finished Thought cells read as
    /// settled history, while the live "Thinking" heading keeps the accent color
    /// and the live body stays dim.
    pub(super) fn reasoning_completed_heading_style() -> Style {
        Style::default().dim().italic()
    }

    pub(super) fn reasoning_completed_dot_prefix() -> Line<'static> {
        Self::muted_dot_prefix()
    }

    pub(super) fn patch_lines_style(lines: &mut [Line<'static>], style: Style) {
        if style == Style::default() {
            return;
        }

        for line in lines {
            line.spans = line
                .spans
                .drain(..)
                .map(|span| span.patch_style(style))
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::PathBuf;
    use std::time::Instant;

    use devo_protocol::PermissionPreset;
    use devo_protocol::ReasoningCapability;
    use devo_protocol::ReasoningEffort;
    use devo_protocol::native::item::ContextOccupancy;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;

    use crate::app_event_sender::AppEventSender;
    use crate::chatwidget::ChatWidgetInit;
    use crate::chatwidget::TuiSessionState;
    use crate::tui::frame_requester::FrameRequester;

    use super::*;

    fn widget_for_summary_bench() -> ChatWidget {
        let model = Model {
            slug: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            context_window: 200_000,
            effective_context_window_percent: Some(95),
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
        widget.total_input_tokens = 124_000;
        widget.total_cache_read_tokens = 82_000;
        widget.total_output_tokens = 9_600;
        widget.last_query_input_tokens = 157_000;
        widget.last_query_total_tokens = 157_000;
        widget
    }

    #[test]
    fn session_summary_resolves_default_reasoning_for_capable_model() {
        let model = Model {
            slug: "deepseek-v4-flash".to_string(),
            display_name: "deepseek-v4-flash".to_string(),
            reasoning_capability: ReasoningCapability::ToggleWithLevels(vec![
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]),
            default_reasoning_effort: Some(ReasoningEffort::High),
            ..Model::default()
        };
        let (app_event_tx, _app_event_rx) = mpsc::unbounded_channel();
        let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
            frame_requester: FrameRequester::test_dummy(),
            app_event_tx: AppEventSender::new(app_event_tx),
            initial_session: TuiSessionState::new(PathBuf::from("."), Some(model)),
            initial_reasoning_effort_selection: None,
            initial_permission_preset: PermissionPreset::Default,
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

        let summary = widget.status_summary_text();

        assert_eq!(summary.contains("high"), false);
        assert_eq!(summary.contains("default"), false);
        assert_eq!(summary.starts_with("deepseek-v4-flash"), true);
    }

    #[test]
    fn session_summary_text_formats_token_context() {
        let widget = widget_for_summary_bench();

        assert_eq!(
            widget.status_summary_text(),
            "Test Model  ▰▰▰▰▰▰▰▰▱▱ 83% 157.0k/190.0k"
        );
    }

    #[test]
    fn session_summary_text_includes_git_branch_before_context() {
        let mut widget = widget_for_summary_bench();
        widget.status_line_branch = Some("feat/status-bar".to_string());

        assert_eq!(
            widget.status_summary_text(),
            "Test Model · feat/status-bar  ▰▰▰▰▰▰▰▰▱▱ 83% 157.0k/190.0k"
        );
        assert_eq!(
            widget.session_summary_text_with_context(/*include_context*/ false),
            "Test Model · feat/status-bar"
        );
    }

    #[test]
    fn context_usage_uses_latest_turn_total_tokens() {
        let mut widget = widget_for_summary_bench();
        widget.last_query_input_tokens = 7;
        widget.last_query_total_tokens = 9;

        assert_eq!(widget.context_usage(), Some((9, 190_000, 0)));
    }

    #[test]
    fn session_switched_restores_context_usage_from_resume_payload() {
        let mut widget = widget_for_summary_bench();
        let occupancy = ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 190_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 48_000,
        );

        widget.handle_worker_event(crate::events::WorkerEvent::SessionSwitched {
            session_id: "session-1".to_string(),
            cwd: PathBuf::from("."),
            title: Some("Resumed".to_string()),
            model: Some("test-model".to_string()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            active_agent_label: None,
            total_input_tokens: 1_000,
            total_output_tokens: 200,
            total_tokens: 1_200,
            total_cache_read_tokens: 0,
            last_query_total_tokens: 58_000,
            last_query_input_tokens: 40_000,
            prompt_token_estimate: 40_000,
            history_items: Vec::new(),
            rich_history_items: Vec::new(),
            loaded_item_count: 0,
            pending_texts: Vec::new(),
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            permission_preset: None,
            effective_context_window: None,
            last_context_occupancy: Some(occupancy),
        });

        assert_eq!(widget.context_usage(), Some((58_000, 190_000, 31)));
        assert!(widget.status_summary_text().contains("58.0k/190.0k"));
    }

    #[test]
    fn context_usage_uses_last_query_with_occupancy_window() {
        let mut widget = widget_for_summary_bench();
        widget.last_query_total_tokens = 99_000;
        widget.last_context_occupancy = Some(ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 996_147, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 6_700,
        ));

        // Mid-turn TurnUsageUpdated advances last_query while occupancy lags
        // until turn finalize; the status bar must follow last_query.
        assert_eq!(widget.context_usage(), Some((99_000, 996_147, 10)));
        assert_eq!(
            widget.status_summary_text(),
            "Test Model  ▰▱▱▱▱▱▱▱▱▱ 10% 99.0k/996.1k"
        );
    }

    #[test]
    fn context_usage_prefers_session_effective_window_over_occupancy() {
        let mut widget = widget_for_summary_bench();
        widget.last_query_total_tokens = 50_000;
        widget.last_context_occupancy = Some(ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 996_147, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
        ));
        widget.effective_context_window = Some(250_000);

        assert_eq!(widget.context_usage(), Some((50_000, 250_000, 20)));
        assert!(widget.status_summary_text().contains("50.0k/250.0k"));
    }

    #[test]
    fn effective_context_window_update_rewrites_occupancy_and_status_bar() {
        let mut widget = widget_for_summary_bench();
        widget.last_query_total_tokens = 50_000;
        widget.last_context_occupancy = Some(ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 996_147, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
        ));

        assert_eq!(widget.context_usage(), Some((50_000, 996_147, 5)));

        widget.note_effective_context_window_updated(250_000);

        assert_eq!(widget.effective_context_window, Some(250_000));
        assert_eq!(
            widget
                .last_context_occupancy
                .as_ref()
                .map(|o| o.context_window_tokens),
            Some(250_000)
        );
        assert_eq!(widget.context_usage(), Some((50_000, 250_000, 20)));
        assert!(widget.status_summary_text().contains("50.0k/250.0k"));
    }

    #[test]
    fn usage_updated_moves_context_bar_even_when_occupancy_is_stale() {
        let mut widget = widget_for_summary_bench();
        widget.last_query_total_tokens = 16_700;
        widget.last_context_occupancy = Some(ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 190_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 6_700,
        ));
        assert_eq!(widget.context_usage(), Some((16_700, 190_000, 9)));

        widget.handle_worker_event(crate::events::WorkerEvent::UsageUpdated {
            total_input_tokens: 550,
            total_output_tokens: 110,
            total_tokens: 660,
            total_cache_read_tokens: 60,
            last_query_total_tokens: 48_000,
            last_query_input_tokens: 40_000,
        });

        assert_eq!(widget.last_query_total_tokens, 48_000);
        assert_eq!(widget.context_usage(), Some((48_000, 190_000, 25)));
        assert!(widget.status_summary_text().contains("48.0k/190.0k"));
    }

    #[test]
    fn usage_updated_prompt_estimate_uses_last_query_input_not_cumulative_total() {
        let mut widget = widget_for_summary_bench();
        widget.total_input_tokens = 500;
        widget.last_query_total_tokens = 42;
        widget.last_query_input_tokens = 30;
        widget.prompt_token_estimate = 30;

        widget.handle_worker_event(crate::events::WorkerEvent::UsageUpdated {
            total_input_tokens: 550,
            total_output_tokens: 110,
            total_tokens: 660,
            total_cache_read_tokens: 60,
            last_query_total_tokens: 48,
            last_query_input_tokens: 35,
        });

        assert_eq!(widget.total_input_tokens, 550);
        assert_eq!(widget.last_query_total_tokens, 48);
        assert_eq!(widget.last_query_input_tokens, 35);
        assert_eq!(widget.prompt_token_estimate, 35);
        assert_eq!(widget.context_usage(), Some((48, 190_000, 0)));
    }

    #[test]
    #[ignore]
    fn bench_render_progress_bar() {
        let iterations = 500_000;
        let expected_len = ChatWidget::render_progress_bar(157_000, 200_000, 10).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(ChatWidget::render_progress_bar(
                black_box(157_000),
                black_box(200_000),
                black_box(10),
            ))
            .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "render_progress_bar iterations={iterations} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_session_summary_text() {
        let widget = widget_for_summary_bench();
        let iterations = 200_000;
        let expected_len = widget.status_summary_text().len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(widget.status_summary_text()).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "session_summary_text iterations={iterations} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
