//! A live task status row rendered above the composer while the agent is busy.
//!
//! The row owns spinner timing, the optional interrupt hint, short inline
//! context (for example, the unified-exec background-process summary), and a
//! single rotating tip line.

use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use unicode_width::UnicodeWidthStr;

use crate::app_event_sender::AppEventSender;
use crate::exec_cell::spinner;
use crate::key_hint;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::line_utils::prefix_lines;
use crate::render::renderable::Renderable;
use crate::shimmer::shimmer_spans;
use crate::text_formatting::capitalize_first;
use crate::tui::frame_requester::FrameRequester;
use crate::ui_consts::LIVE_PREFIX_COLS;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

pub(crate) const STATUS_DETAILS_DEFAULT_MAX_LINES: usize = 3;
const DETAILS_PREFIX: &str = "  └ ";
const STATUS_ANIMATION_INTERVAL: Duration = Duration::from_millis(80);
pub(crate) const TIP_ROTATION_INTERVAL: Duration = Duration::from_secs(6);
pub(crate) const WORKING_TIPS: &[&str] = &[
    "You can type your next message while Devo is working; it will be queued.",
    "Press ESC twice to stop the current turn.",
    "Use /model to switch models for the next turn.",
    "Use /compact when a long session starts losing focus.",
    "Enter '@' to mention file paths when you want Devo to edit specific files.",
    "@: mention files | ! to run a shell command",
    "Queue follow-up instructions while Devo is working; they will run next.",
    "Use /resume to continue a previous session.",
    "Use /new to start fresh when the current session has too much context.",
    "Keep prompts narrow when you want a small, low-risk code change.",
    "Press SHIFT+TAB to switch modes.",
    "Enter '!' to enter SHELL mode; press SHIFT+TAB to switch modes.",
    "Press Ctrl+S to add guidance without waiting for the turn to finish.",
    "You can also send messages while Devo is working; they enter the wait queue.",
    "Use /status to check current token and context usage.",
    "Use /settings to change the compact limit, theme, and more.",
];

/// Tip text for `elapsed` wall time, rotating every [`TIP_ROTATION_INTERVAL`].
pub(crate) fn rotating_working_tip(elapsed: Duration) -> Option<&'static str> {
    if WORKING_TIPS.is_empty() {
        return None;
    }
    let interval_secs = TIP_ROTATION_INTERVAL.as_secs().max(1);
    let index = (elapsed.as_secs() / interval_secs) as usize % WORKING_TIPS.len();
    Some(WORKING_TIPS[index])
}

/// Composer placeholder string for the tip at `elapsed` (`Tip: …`).
pub(crate) fn composer_tip_placeholder(elapsed: Duration) -> Option<String> {
    rotating_working_tip(elapsed).map(|tip| format!("Tip: {tip}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusDetailsCapitalization {
    CapitalizeFirst,
    Preserve,
}

/// Displays a single-line in-progress status with optional wrapped details.
pub(crate) struct StatusIndicatorWidget {
    /// Animated header text (defaults to "Working").
    header: String,
    details: Option<String>,
    details_max_lines: usize,
    /// Optional suffix rendered after the elapsed/interrupt segment.
    inline_message: Option<String>,
    show_interrupt_hint: bool,
    show_working_tip: bool,
    subagent_hint_visible: bool,

    elapsed_running: Duration,
    last_resume_at: Instant,
    is_paused: bool,
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
}

// Format elapsed seconds into a compact human-friendly form used by the status line.
// Examples: 0s, 59s, 1m 00s, 59m 59s, 1h 00m 00s, 2h 03m 09s
pub fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

impl StatusIndicatorWidget {
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        frame_requester: FrameRequester,
        animations_enabled: bool,
    ) -> Self {
        Self {
            header: String::from("Working"),
            details: None,
            details_max_lines: STATUS_DETAILS_DEFAULT_MAX_LINES,
            inline_message: None,
            show_interrupt_hint: true,
            show_working_tip: true,
            subagent_hint_visible: false,
            elapsed_running: Duration::ZERO,
            last_resume_at: Instant::now(),
            is_paused: false,

            app_event_tx,
            frame_requester,
            animations_enabled,
        }
    }

    /// Update the animated header label (left of the brackets).
    pub(crate) fn update_header(&mut self, header: String) {
        self.header = header;
    }

    /// Update the details text shown below the header.
    pub(crate) fn update_details(
        &mut self,
        details: Option<String>,
        capitalization: StatusDetailsCapitalization,
        max_lines: usize,
    ) {
        self.details_max_lines = max_lines.max(1);
        self.details = details
            .filter(|details| !details.is_empty())
            .map(|details| {
                let trimmed = details.trim_start();
                match capitalization {
                    StatusDetailsCapitalization::CapitalizeFirst => capitalize_first(trimmed),
                    StatusDetailsCapitalization::Preserve => trimmed.to_string(),
                }
            });
    }

    /// Update the inline suffix text shown after `({elapsed} • esc to interrupt)`.
    ///
    /// Callers should provide plain, already-contextualized text. Passing
    /// verbose status prose here can cause frequent width truncation and hide
    /// the more important elapsed/interrupt hint.
    pub(crate) fn update_inline_message(&mut self, message: Option<String>) {
        self.inline_message = message
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty());
    }

    #[cfg(test)]
    pub(crate) fn header(&self) -> &str {
        &self.header
    }

    #[cfg(test)]
    pub(crate) fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }

    pub(crate) fn set_interrupt_hint_visible(&mut self, visible: bool) {
        self.show_interrupt_hint = visible;
    }

    pub(crate) fn set_working_tip_visible(&mut self, visible: bool) {
        self.show_working_tip = visible;
    }

    pub(crate) fn set_subagent_hint_visible(&mut self, visible: bool) {
        self.subagent_hint_visible = visible;
    }

    #[cfg(test)]
    pub(crate) fn interrupt_hint_visible(&self) -> bool {
        self.show_interrupt_hint
    }

    pub(crate) fn pause_timer(&mut self) {
        self.pause_timer_at(Instant::now());
    }

    pub(crate) fn resume_timer(&mut self) {
        self.resume_timer_at(Instant::now());
    }

    pub(crate) fn pause_timer_at(&mut self, now: Instant) {
        if self.is_paused {
            return;
        }
        self.elapsed_running += now.saturating_duration_since(self.last_resume_at);
        self.is_paused = true;
    }

    pub(crate) fn resume_timer_at(&mut self, now: Instant) {
        if !self.is_paused {
            return;
        }
        self.last_resume_at = now;
        self.is_paused = false;
        self.frame_requester.schedule_frame();
    }

    fn elapsed_duration_at(&self, now: Instant) -> Duration {
        let mut elapsed = self.elapsed_running;
        if !self.is_paused {
            elapsed += now.saturating_duration_since(self.last_resume_at);
        }
        elapsed
    }

    fn elapsed_seconds_at(&self, now: Instant) -> u64 {
        self.elapsed_duration_at(now).as_secs()
    }

    pub fn elapsed_seconds(&self) -> u64 {
        self.elapsed_seconds_at(Instant::now())
    }

    /// Wrap the details text into a fixed width and return the lines, truncating if necessary.
    fn wrapped_details_lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(details) = self.details.as_deref() else {
            return Vec::new();
        };
        if width == 0 {
            return Vec::new();
        }

        let prefix_width = UnicodeWidthStr::width(DETAILS_PREFIX);
        let opts = RtOptions::new(usize::from(width))
            .initial_indent(Line::from(DETAILS_PREFIX.dim()))
            .subsequent_indent(Line::from(Span::from(" ".repeat(prefix_width)).dim()))
            .break_words(/*break_words*/ true);

        let mut out = word_wrap_lines(details.lines().map(|line| vec![line.dim()]), opts);

        if out.len() > self.details_max_lines {
            out.truncate(self.details_max_lines);
            let content_width = usize::from(width).saturating_sub(prefix_width).max(1);
            let max_base_len = content_width.saturating_sub(1);
            if let Some(last) = out.last_mut()
                && let Some(span) = last.spans.last_mut()
            {
                let trimmed: String = span.content.as_ref().chars().take(max_base_len).collect();
                *span = format!("{trimmed}…").dim();
            }
        }

        out
    }

    fn working_tip_at(&self, elapsed: Duration) -> Option<&'static str> {
        if !self.show_working_tip {
            return None;
        }
        rotating_working_tip(elapsed)
    }

    fn working_tip_line(&self, width: u16, elapsed: Duration) -> Option<Line<'static>> {
        if width == 0 {
            return None;
        }

        let tip = self.working_tip_at(elapsed)?;
        Some(truncate_line_with_ellipsis_if_overflow(
            Line::from(vec![DETAILS_PREFIX.dim(), "Tip: ".dim(), tip.dim()]),
            usize::from(width),
        ))
    }
}

impl Renderable for StatusIndicatorWidget {
    fn desired_height(&self, width: u16) -> u16 {
        let content_width = width.saturating_sub(LIVE_PREFIX_COLS);
        let details_height =
            u16::try_from(self.wrapped_details_lines(content_width).len()).unwrap_or(0);
        let tip_height = u16::from(
            content_width > 0
                && self
                    .working_tip_at(self.elapsed_duration_at(Instant::now()))
                    .is_some(),
        );
        1 + details_height + tip_height
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        if self.animations_enabled {
            // Schedule next animation frame.
            self.frame_requester
                .schedule_frame_in(STATUS_ANIMATION_INTERVAL);
        }
        let now = Instant::now();
        let elapsed_duration = self.elapsed_duration_at(now);
        let pretty_elapsed = fmt_elapsed_compact(elapsed_duration.as_secs());

        let mut spans = Vec::with_capacity(12);
        spans.push(spinner(Some(self.last_resume_at), self.animations_enabled));
        spans.push(" ".into());
        if self.animations_enabled {
            spans.extend(shimmer_spans(&self.header));
        } else if !self.header.is_empty() {
            spans.push(self.header.clone().into());
        }
        spans.push(" ".into());
        if self.show_interrupt_hint {
            spans.push(format!("({pretty_elapsed} • ").dim());
            spans.push(key_hint::plain(KeyCode::Esc).into());
            spans.push(" to interrupt)".dim());
        } else {
            spans.push(format!("({pretty_elapsed})").dim());
        }
        if let Some(message) = &self.inline_message {
            // Keep optional context after elapsed/interrupt text so that core
            // interrupt affordances stay in a fixed visual location.
            spans.push(" · ".dim());
            spans.push(message.clone().dim());
        }
        if self.subagent_hint_visible {
            spans.push(" · ".dim());
            spans.push(key_hint::ctrl(KeyCode::Char('x')).into());
            spans.push(" agents".dim());
        }

        let content_width = area.width.saturating_sub(LIVE_PREFIX_COLS);
        let mut lines = Vec::new();
        lines.push(truncate_line_with_ellipsis_if_overflow(
            Line::from(spans),
            usize::from(content_width),
        ));
        if area.height > 1 {
            // If there is enough space, add the details lines below the header.
            let details = self.wrapped_details_lines(content_width);
            let max_details = usize::from(area.height.saturating_sub(1));
            lines.extend(details.into_iter().take(max_details));
        }
        let remaining_height = usize::from(area.height).saturating_sub(lines.len());
        if remaining_height > 0
            && let Some(tip_line) = self.working_tip_line(content_width, elapsed_duration)
        {
            lines.push(tip_line);
        }

        debug_assert_eq!(LIVE_PREFIX_COLS, 2);
        let left_padding = Span::raw(" ".repeat(LIVE_PREFIX_COLS as usize));
        let lines = prefix_lines(lines, left_padding.clone(), left_padding);
        Paragraph::new(Text::from(lines)).render_ref(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use super::*;
    use crate::app_event_sender::AppEventSender;
    use crate::tui::frame_requester::FrameRequester;
    use pretty_assertions::assert_eq;

    fn row_text(buf: &Buffer, width: u16, row: u16) -> String {
        (0..width).map(|col| buf[(col, row)].symbol()).collect()
    }

    #[test]
    fn status_indicator_renders_header_and_working_tip() {
        let (app_event_tx, _app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let widget = StatusIndicatorWidget::new(
            AppEventSender::new(app_event_tx),
            FrameRequester::test_dummy(),
            false,
        );

        assert_eq!(widget.desired_height(80), 2);

        let area = Rect::new(0, 0, 100, 2);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let top_row = row_text(&buf, area.width, 0);
        let tip_row = row_text(&buf, area.width, 1);

        assert_eq!(top_row.get(..2), Some("  "));
        assert!(top_row.contains("Working"));
        assert!(tip_row.contains("└ Tip: "));
        assert!(tip_row.contains(WORKING_TIPS[0]));
    }

    #[test]
    fn status_indicator_can_hide_working_tip() {
        let (app_event_tx, _app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut widget = StatusIndicatorWidget::new(
            AppEventSender::new(app_event_tx),
            FrameRequester::test_dummy(),
            false,
        );
        widget.update_header("Resuming session...".to_string());
        widget.set_working_tip_visible(false);

        assert_eq!(widget.desired_height(80), 1);

        let area = Rect::new(0, 0, 100, 2);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let rendered = [row_text(&buf, area.width, 0), row_text(&buf, area.width, 1)].join("\n");
        assert!(rendered.contains("Resuming session..."));
        assert!(!rendered.contains("Tip:"));
    }

    #[test]
    fn status_indicator_rotates_working_tip_every_six_seconds() {
        let (app_event_tx, _app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let widget = StatusIndicatorWidget::new(
            AppEventSender::new(app_event_tx),
            FrameRequester::test_dummy(),
            false,
        );

        assert_eq!(
            widget.working_tip_at(Duration::from_secs(5)),
            Some(WORKING_TIPS[0])
        );
        assert_eq!(
            widget.working_tip_at(Duration::from_secs(6)),
            Some(WORKING_TIPS[1])
        );
    }

    #[test]
    fn composer_tip_placeholder_reuses_working_tips() {
        assert_eq!(
            composer_tip_placeholder(Duration::ZERO),
            Some(format!("Tip: {}", WORKING_TIPS[0]))
        );
        assert_eq!(
            composer_tip_placeholder(Duration::from_secs(6)),
            Some(format!("Tip: {}", WORKING_TIPS[1]))
        );
    }

    #[test]
    #[ignore]
    fn bench_status_indicator_render_without_details() {
        let (app_event_tx, _app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut widget = StatusIndicatorWidget::new(
            AppEventSender::new(app_event_tx),
            FrameRequester::test_dummy(),
            false,
        );
        widget.update_inline_message(Some("running shell command".to_string()));

        let area = Rect::new(0, 0, 120, 1);
        let mut buf = Buffer::empty(area);
        let started = Instant::now();
        for _ in 0..50_000 {
            widget.render(black_box(area), black_box(&mut buf));
        }
        let elapsed = started.elapsed();

        assert_eq!(buf.area, area);
        println!(
            "status_indicator_render_without_details iterations=50000 width=120 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 50_000.0
        );
    }
}
