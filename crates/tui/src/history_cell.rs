//! Transcript/history cells for the Devo TUI.
//!
//! A `HistoryCell` is the unit of display in the conversation UI, representing both committed
//! transcript entries and, transiently, an in-flight active cell that can mutate in place while
//! streaming.
//!
//! The transcript overlay (`Ctrl+T`) appends a cached live tail derived from the active cell, and
//! that cached tail is refreshed based on an active-cell cache key. Cells that change based on
//! elapsed time expose `transcript_animation_tick()`, and code that mutates the active cell in place
//! bumps the active-cell revision tracked by `ChatWidget`, so the cache key changes whenever the
//! rendered transcript output can change.

use crate::bottom_pane::InputMode;
use crate::diff_render::create_diff_summary;
use crate::diff_render::display_path_for;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::OutputLinesParams;
use crate::exec_cell::TOOL_CALL_MAX_LINES;
use crate::exec_cell::output_lines;
use crate::exec_cell::spinner;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::live_wrap::take_prefix_by_width;
use crate::markdown::append_markdown;
use crate::render::line_utils::prefix_lines;
use crate::render::line_utils::push_owned_lines;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;
use crate::startup_header::StartupHeaderData;
use crate::startup_header::build_startup_header;
use crate::style::user_message_style;
use crate::text_formatting::truncate_text;
use crate::theme::ThemeSet;
use crate::ui_consts::LIVE_PREFIX_COLS;
use crate::version::CLI_VERSION;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::adaptive_wrap_lines;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningImplementation;
use devo_protocol::protocol::FileChange;
use devo_protocol::user_input::TextElement;
use image::DynamicImage;
use ratatui::prelude::*;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Styled;
use ratatui::style::Stylize;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::any::Any;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub(crate) struct ScrollbackLine {
    pub(crate) line: Line<'static>,
}

impl ScrollbackLine {
    pub(crate) fn new(line: Line<'static>) -> Self {
        Self { line }
    }
}

impl From<Line<'static>> for ScrollbackLine {
    fn from(line: Line<'static>) -> Self {
        Self::new(line)
    }
}

/// Represents an event to display in the conversation history. Returns its
/// `Vec<Line<'static>>` representation to make it easier to display in a
/// scrollable list.
/// A single renderable unit of conversation history.
///
/// Each cell produces logical `Line`s and reports how many viewport
/// rows those lines occupy at a given terminal width. The default
/// height implementations use `Paragraph::wrap` to account for lines
/// that overflow the viewport width (e.g. long URLs that are kept
/// intact by adaptive wrapping). Concrete types only need to override
/// heights when they apply additional layout logic beyond what
/// `Paragraph::line_count` captures.
pub(crate) trait HistoryCell: std::fmt::Debug + Send + Sync + Any {
    /// Returns the logical lines for the main chat viewport.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    /// Returns the number of viewport rows needed to render this cell.
    ///
    /// The default delegates to `Paragraph::line_count` with
    /// `Wrap { trim: false }`, which measures the actual row count after
    /// ratatui's viewport-level character wrapping. This is critical
    /// for lines containing URL-like tokens that are wider than the
    /// terminal — the logical line count would undercount.
    fn desired_height(&self, width: u16) -> u16 {
        Paragraph::new(Text::from(self.display_lines(width)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    /// Returns lines for the transcript overlay (`Ctrl+T`).
    ///
    /// Defaults to `display_lines`. Override when the transcript
    /// representation differs (e.g. `ExecCell` shows all calls with
    /// `$`-prefixed commands and exit status).
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }

    /// Returns the number of viewport rows for the transcript overlay.
    ///
    /// Uses the same `Paragraph::line_count` measurement as
    /// `desired_height`. Contains a workaround for a ratatui bug where
    /// a single whitespace-only line reports 2 rows instead of 1.
    fn desired_transcript_height(&self, width: u16) -> u16 {
        let lines = self.transcript_lines(width);
        // Workaround: ratatui's line_count returns 2 for a single
        // whitespace-only line. Clamp to 1 in that case.
        if let [line] = &lines[..]
            && line
                .spans
                .iter()
                .all(|s| s.content.chars().all(char::is_whitespace))
        {
            return 1;
        }

        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    fn is_stream_continuation(&self) -> bool {
        false
    }

    /// Returns a coarse "animation tick" when transcript output is time-dependent.
    ///
    /// The transcript overlay caches the rendered output of the in-flight active cell, so cells
    /// that include time-based UI (spinner, shimmer, etc.) should return a tick that changes over
    /// time to signal that the cached tail should be recomputed. Returning `None` means the
    /// transcript lines are stable, while returning `Some(tick)` during an in-flight animation
    /// allows the overlay to keep up with the main viewport.
    ///
    /// If a cell uses time-based visuals but always returns `None`, `Ctrl+T` can appear "frozen" on
    /// the first rendered frame even though the main viewport is animating.
    fn transcript_animation_tick(&self) -> Option<u64> {
        None
    }
}

impl Renderable for Box<dyn HistoryCell> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = self.display_lines(area.width);
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        let y = if area.height == 0 {
            0
        } else {
            let overflow = paragraph
                .line_count(area.width)
                .saturating_sub(usize::from(area.height));
            u16::try_from(overflow).unwrap_or(u16::MAX)
        };
        paragraph.scroll((y, 0)).render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        HistoryCell::desired_height(self.as_ref(), width)
    }
}

impl dyn HistoryCell {
    pub(crate) fn as_any(&self) -> &dyn Any {
        self
    }

    pub(crate) fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug)]
pub(crate) struct UserHistoryCell {
    pub message: String,
    pub text_elements: Vec<TextElement>,
    #[allow(dead_code)]
    pub local_image_paths: Vec<PathBuf>,
    pub remote_image_urls: Vec<String>,
    pub accent_color: Color,
    pub input_mode: InputMode,
    pub selected: bool,
}

/// Build logical lines for a user message with styled text elements.
///
/// This preserves explicit newlines while interleaving element spans and skips
/// malformed byte ranges instead of panicking during history rendering.
#[derive(Clone, Debug)]
struct UserMessageStyleRange {
    byte_range: Range<usize>,
    style: Style,
}

fn leading_slash_command_range(message: &str) -> Option<Range<usize>> {
    let first_line = message.split_once('\n').map_or(message, |(line, _)| line);
    let stripped = first_line.strip_prefix('/')?;
    let name_end = stripped
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, _)| idx)
        .unwrap_or(stripped.len());
    if name_end == 0 {
        return None;
    }
    let name = &stripped[..name_end];
    if name.contains('/') || SlashCommand::from_str(name).is_err() {
        return None;
    }
    Some(0..1 + name.len())
}

fn build_user_message_lines_with_elements(
    message: &str,
    elements: &[TextElement],
    style: Style,
    element_style: Style,
    slash_command_style: Style,
) -> Vec<Line<'static>> {
    let mut style_ranges: Vec<UserMessageStyleRange> = elements
        .iter()
        .map(|element| UserMessageStyleRange {
            byte_range: element.byte_range.start..element.byte_range.end,
            style: element_style,
        })
        .collect();
    if let Some(byte_range) = leading_slash_command_range(message) {
        style_ranges.push(UserMessageStyleRange {
            byte_range,
            style: slash_command_style,
        });
    }
    style_ranges.sort_by_key(|range| range.byte_range.start);
    let mut offset = 0usize;
    let mut raw_lines: Vec<Line<'static>> = Vec::new();
    for line_text in message.split('\n') {
        let line_start = offset;
        let line_end = line_start + line_text.len();
        let mut spans: Vec<Span<'static>> = Vec::new();
        // Track how much of the line we've emitted to interleave plain and styled spans.
        let mut cursor = line_start;
        for style_range in &style_ranges {
            let start = style_range.byte_range.start.max(line_start).max(cursor);
            let end = style_range.byte_range.end.min(line_end);
            if start >= end {
                continue;
            }
            let rel_start = start - line_start;
            let rel_end = end - line_start;
            // Guard against malformed UTF-8 byte ranges from upstream data; skip
            // invalid elements rather than panicking while rendering history.
            if !line_text.is_char_boundary(rel_start) || !line_text.is_char_boundary(rel_end) {
                continue;
            }
            let rel_cursor = cursor - line_start;
            if cursor < start
                && line_text.is_char_boundary(rel_cursor)
                && let Some(segment) = line_text.get(rel_cursor..rel_start)
            {
                spans.push(Span::from(segment.to_string()));
            }
            if let Some(segment) = line_text.get(rel_start..rel_end) {
                spans.push(Span::styled(segment.to_string(), style_range.style));
                cursor = end;
            }
        }
        let rel_cursor = cursor - line_start;
        if cursor < line_end
            && line_text.is_char_boundary(rel_cursor)
            && let Some(segment) = line_text.get(rel_cursor..)
        {
            spans.push(Span::from(segment.to_string()));
        }
        let line = if spans.is_empty() {
            Line::from(line_text.to_string()).style(style)
        } else {
            Line::from(spans).style(style)
        };
        raw_lines.push(line);
        // Split on '\n' so any '\r' stays in the line; advancing by 1 accounts
        // for the separator byte.
        offset = line_end + 1;
    }

    raw_lines
}

fn trim_trailing_blank_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    while lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    {
        lines.pop();
    }
    lines
}

pub(crate) fn collapse_consecutive_blank_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let mut collapsed = Vec::with_capacity(lines.len());
    let mut last_was_blank = false;
    for line in lines {
        let is_blank = line.spans.iter().all(|span| span.content.trim().is_empty());
        if is_blank && last_was_blank {
            continue;
        }
        last_was_blank = is_blank;
        collapsed.push(line);
    }
    collapsed
}

impl HistoryCell for UserHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let wrap_width = width
            .saturating_sub(
                LIVE_PREFIX_COLS + 1, /* keep a one-column right margin for wrapping */
            )
            .max(1);

        let accent = self.accent_color;
        let mode_color = self.input_mode.color();
        let style = if self.selected {
            user_message_style().fg(mode_color)
        } else {
            user_message_style()
        };
        let element_style = style.fg(accent);
        let slash_command_style = style.fg(accent);
        let prefix_style = Style::default().fg(mode_color);
        let blank_prefixed_line = || Line::from(Span::styled("  ", style)).style(style);

        let wrapped_message = if self.message.is_empty() && self.text_elements.is_empty() {
            None
        } else {
            let message = if self.text_elements.is_empty() {
                self.message.trim_end_matches(['\r', '\n'])
            } else {
                &self.message
            };
            let raw_lines = build_user_message_lines_with_elements(
                message,
                &self.text_elements,
                style,
                element_style,
                slash_command_style,
            );
            let wrapped = adaptive_wrap_lines(
                raw_lines,
                RtOptions::new(usize::from(wrap_width))
                    .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
            );
            let wrapped = trim_trailing_blank_lines(wrapped);
            (!wrapped.is_empty()).then_some(wrapped)
        };

        let mut lines = vec![blank_prefixed_line()];
        if let Some(wrapped_message) = wrapped_message {
            lines.extend(prefix_lines(
                wrapped_message,
                Span::styled("▌ ", prefix_style),
                Span::styled("  ", style),
            ));
        }
        lines.push(blank_prefixed_line());
        lines
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasoningViewportMode {
    /// Show the full reasoning body in the main transcript.
    Full,
    /// Show a one-line summary in the main transcript; full body in Ctrl+T.
    Compact,
    /// Hide from the main transcript; full body remains in Ctrl+T.
    TranscriptOnly,
}

pub(crate) const REASONING_TRANSCRIPT_HINT: &str = "(ctrl + t to view transcript)";

pub(crate) fn reasoning_transcript_hint_line() -> Line<'static> {
    Line::from(REASONING_TRANSCRIPT_HINT.dim())
}

#[derive(Debug)]
pub(crate) struct ReasoningSummaryCell {
    _header: String,
    content: String,
    /// Session cwd used to render local file links inside the reasoning body.
    cwd: PathBuf,
    viewport_mode: ReasoningViewportMode,
}

impl ReasoningSummaryCell {
    /// Create a reasoning summary cell that will render local file links relative to the session
    /// cwd active when the summary was recorded.
    pub(crate) fn new(
        header: String,
        content: String,
        cwd: &Path,
        viewport_mode: ReasoningViewportMode,
    ) -> Self {
        Self {
            _header: header,
            content,
            cwd: cwd.to_path_buf(),
            viewport_mode,
        }
    }

    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        append_markdown(
            &self.content,
            Some((width as usize).saturating_sub(2)),
            Some(self.cwd.as_path()),
            &mut lines,
        );
        let summary_style = Style::default().dim().italic();
        let summary_lines = lines
            .into_iter()
            .map(|mut line| {
                line.spans = line
                    .spans
                    .into_iter()
                    .map(|span| span.patch_style(summary_style))
                    .collect();
                line
            })
            .collect::<Vec<_>>();

        adaptive_wrap_lines(
            &summary_lines,
            RtOptions::new(width as usize)
                .initial_indent("▌ ".dim().into())
                .subsequent_indent("  ".into()),
        )
    }

    fn compact_lines(&self, width: u16) -> Vec<Line<'static>> {
        let preview = compact_reasoning_preview(&self.content);
        let muted = Style::default().dim().italic();
        let label = if preview.is_empty() {
            "Thought".to_string()
        } else {
            format!("Thought · {preview}")
        };
        let thought = Line::from(vec![
            Span::styled("▌ ", Style::default().dim()),
            Span::styled(label, muted),
        ]);
        vec![
            crate::line_truncation::truncate_line_with_ellipsis_if_overflow(
                thought,
                width as usize,
            ),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(REASONING_TRANSCRIPT_HINT, Style::default().dim()),
            ]),
        ]
    }
}

fn compact_reasoning_preview(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_matches('*').trim().to_string())
        .unwrap_or_default()
}

impl HistoryCell for ReasoningSummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self.viewport_mode {
            ReasoningViewportMode::Full => self.lines(width),
            ReasoningViewportMode::Compact => self.compact_lines(width),
            ReasoningViewportMode::TranscriptOnly => Vec::new(),
        }
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines(width)
    }
}

#[derive(Debug)]
pub(crate) struct AgentMessageCell {
    lines: Vec<Line<'static>>,
    initial_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
    is_stream_continuation: bool,
}

impl AgentMessageCell {
    pub(crate) fn new(lines: Vec<Line<'static>>, is_first_line: bool) -> Self {
        Self {
            lines,
            initial_prefix: if is_first_line {
                "▌ ".dim().into()
            } else {
                "  ".into()
            },
            subsequent_prefix: "  ".into(),
            is_stream_continuation: !is_first_line,
        }
    }

    pub(crate) fn new_ai_response_with_prefix(
        lines: Vec<Line<'static>>,
        initial_prefix: impl Into<Line<'static>>,
        subsequent_prefix: impl Into<Line<'static>>,
        is_stream_continuation: bool,
    ) -> Self {
        Self {
            lines,
            initial_prefix: initial_prefix.into(),
            subsequent_prefix: subsequent_prefix.into(),
            is_stream_continuation,
        }
    }

    pub(crate) fn new_with_prefix(
        lines: Vec<Line<'static>>,
        initial_prefix: impl Into<Line<'static>>,
        subsequent_prefix: impl Into<Line<'static>>,
        is_stream_continuation: bool,
    ) -> Self {
        Self {
            lines,
            initial_prefix: initial_prefix.into(),
            subsequent_prefix: subsequent_prefix.into(),
            is_stream_continuation,
        }
    }
}

impl HistoryCell for AgentMessageCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        collapse_consecutive_blank_lines(adaptive_wrap_lines(
            &self.lines,
            RtOptions::new(width as usize)
                .initial_indent(self.initial_prefix.clone())
                .subsequent_indent(self.subsequent_prefix.clone()),
        ))
    }

    fn is_stream_continuation(&self) -> bool {
        self.is_stream_continuation
    }
}

#[derive(Debug)]
pub(crate) struct AgentMarkdownCell {
    markdown_source: String,
    cwd: PathBuf,
    initial_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
}

impl AgentMarkdownCell {
    pub(crate) fn new(
        markdown_source: String,
        cwd: &Path,
        initial_prefix: impl Into<Line<'static>>,
        subsequent_prefix: impl Into<Line<'static>>,
    ) -> Self {
        Self {
            markdown_source,
            cwd: cwd.to_path_buf(),
            initial_prefix: initial_prefix.into(),
            subsequent_prefix: subsequent_prefix.into(),
        }
    }
}

impl HistoryCell for AgentMarkdownCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        append_markdown(
            &self.markdown_source,
            /*width*/ None,
            Some(self.cwd.as_path()),
            &mut lines,
        );
        let lines = collapse_consecutive_blank_lines(lines);
        collapse_consecutive_blank_lines(adaptive_wrap_lines(
            &lines,
            RtOptions::new(width as usize)
                .initial_indent(self.initial_prefix.clone())
                .subsequent_indent(self.subsequent_prefix.clone()),
        ))
    }
}

#[derive(Debug)]
pub(crate) struct ProposedPlanCell {
    plan_markdown: String,
    cwd: PathBuf,
}

impl ProposedPlanCell {
    pub(crate) fn new(plan_markdown: String, cwd: &Path) -> Self {
        Self {
            plan_markdown,
            cwd: cwd.to_path_buf(),
        }
    }
}

impl HistoryCell for ProposedPlanCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let content_width = width.saturating_sub(4).max(1) as usize;
        let mut body = Vec::new();
        append_markdown(
            self.plan_markdown.trim(),
            Some(content_width),
            Some(self.cwd.as_path()),
            &mut body,
        );
        if body.is_empty() {
            body.push(Line::from("(empty)").dim());
        }

        prefix_lines(body, Span::raw("  "), Span::raw("  "))
    }
}

pub(crate) fn new_proposed_plan(plan_markdown: String, cwd: &Path) -> ProposedPlanCell {
    ProposedPlanCell::new(plan_markdown, cwd)
}

#[derive(Debug)]
pub(crate) struct PlainHistoryCell {
    lines: Vec<Line<'static>>,
}

impl PlainHistoryCell {
    pub(crate) fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines }
    }
}

impl HistoryCell for PlainHistoryCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.lines.clone()
    }
}

#[derive(Debug)]
pub(crate) struct PrefixedWrappedHistoryCell {
    text: Text<'static>,
    initial_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
}

impl PrefixedWrappedHistoryCell {
    pub(crate) fn new(
        text: impl Into<Text<'static>>,
        initial_prefix: impl Into<Line<'static>>,
        subsequent_prefix: impl Into<Line<'static>>,
    ) -> Self {
        Self {
            text: text.into(),
            initial_prefix: initial_prefix.into(),
            subsequent_prefix: subsequent_prefix.into(),
        }
    }
}

impl HistoryCell for PrefixedWrappedHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }
        let opts = RtOptions::new(width.max(1) as usize)
            .initial_indent(self.initial_prefix.clone())
            .subsequent_indent(self.subsequent_prefix.clone());
        adaptive_wrap_lines(&self.text, opts)
    }
}

#[derive(Debug)]
pub(crate) struct UnifiedExecInteractionCell {
    command_display: Option<String>,
    stdin: String,
}

impl UnifiedExecInteractionCell {
    pub(crate) fn new(command_display: Option<String>, stdin: String) -> Self {
        Self {
            command_display,
            stdin,
        }
    }
}

impl HistoryCell for UnifiedExecInteractionCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }
        let wrap_width = width as usize;
        let waited_only = self.stdin.is_empty();

        let mut header_spans = if waited_only {
            vec!["▌ ".cyan(), "Waited for background terminal".bold()]
        } else {
            vec!["▌ ".cyan(), "Interacted with background terminal".bold()]
        };
        if let Some(command) = &self.command_display
            && !command.is_empty()
        {
            header_spans.push(" · ".dim());
            header_spans.push(command.clone().dim());
        }
        let header = Line::from(header_spans);

        let mut out: Vec<Line<'static>> = Vec::new();
        let header_wrapped = adaptive_wrap_line(&header, RtOptions::new(wrap_width));
        push_owned_lines(&header_wrapped, &mut out);

        if waited_only {
            return out;
        }

        let input_lines: Vec<Line<'static>> =
            self.stdin.lines().map(render_terminal_input).collect();

        let input_wrapped = adaptive_wrap_lines(
            input_lines,
            RtOptions::new(wrap_width)
                .initial_indent(Line::from("  └ ".dim()))
                .subsequent_indent(Line::from("    ".dim())),
        );
        out.extend(input_wrapped);
        out
    }
}

pub(crate) fn new_unified_exec_interaction(
    command_display: Option<String>,
    stdin: String,
) -> UnifiedExecInteractionCell {
    UnifiedExecInteractionCell::new(command_display, stdin)
}

fn render_terminal_input(input: &str) -> Line<'static> {
    if input.is_empty() {
        return Line::from("⏎".dim());
    }
    Line::from(input.to_string())
}

#[derive(Debug)]
struct UnifiedExecProcessesCell {
    processes: Vec<UnifiedExecProcessDetails>,
}

impl UnifiedExecProcessesCell {
    fn new(processes: Vec<UnifiedExecProcessDetails>) -> Self {
        Self { processes }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UnifiedExecProcessDetails {
    pub(crate) command_display: String,
    pub(crate) recent_chunks: Vec<String>,
}

impl HistoryCell for UnifiedExecProcessesCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }

        let wrap_width = width as usize;
        let max_processes = 16usize;
        let mut out: Vec<Line<'static>> = Vec::new();
        out.push(vec!["Background terminals".bold()].into());
        out.push("".into());

        if self.processes.is_empty() {
            out.push("  ▌ No background terminals running.".italic().into());
            return out;
        }

        let prefix = "  ▌ ";
        let prefix_width = UnicodeWidthStr::width(prefix);
        let truncation_suffix = " [...]";
        let truncation_suffix_width = UnicodeWidthStr::width(truncation_suffix);
        let mut shown = 0usize;
        for process in &self.processes {
            if shown >= max_processes {
                break;
            }
            let command = &process.command_display;
            let (snippet, snippet_truncated) = {
                let (first_line, has_more_lines) = match command.split_once('\n') {
                    Some((first, _)) => (first, true),
                    None => (command.as_str(), false),
                };
                let max_graphemes = 80;
                let mut graphemes = first_line.grapheme_indices(true);
                if let Some((byte_index, _)) = graphemes.nth(max_graphemes) {
                    (first_line[..byte_index].to_string(), true)
                } else {
                    (first_line.to_string(), has_more_lines)
                }
            };
            if wrap_width <= prefix_width {
                out.push(Line::from(prefix.dim()));
                shown += 1;
                continue;
            }
            let budget = wrap_width.saturating_sub(prefix_width);
            let mut needs_suffix = snippet_truncated;
            if !needs_suffix {
                let (_, remainder, _) = take_prefix_by_width(&snippet, budget);
                if !remainder.is_empty() {
                    needs_suffix = true;
                }
            }
            if needs_suffix && budget > truncation_suffix_width {
                let available = budget.saturating_sub(truncation_suffix_width);
                let (truncated, _, _) = take_prefix_by_width(&snippet, available);
                out.push(vec![prefix.dim(), truncated.cyan(), truncation_suffix.dim()].into());
            } else {
                let (truncated, _, _) = take_prefix_by_width(&snippet, budget);
                out.push(vec![prefix.dim(), truncated.cyan()].into());
            }

            let chunk_prefix_first = "    ↳ ";
            let chunk_prefix_next = "      ";
            for (idx, chunk) in process.recent_chunks.iter().enumerate() {
                let chunk_prefix = if idx == 0 {
                    chunk_prefix_first
                } else {
                    chunk_prefix_next
                };
                let chunk_prefix_width = UnicodeWidthStr::width(chunk_prefix);
                if wrap_width <= chunk_prefix_width {
                    out.push(Line::from(chunk_prefix.dim()));
                    continue;
                }
                let budget = wrap_width.saturating_sub(chunk_prefix_width);
                let (truncated, remainder, _) = take_prefix_by_width(chunk, budget);
                if !remainder.is_empty() && budget > truncation_suffix_width {
                    let available = budget.saturating_sub(truncation_suffix_width);
                    let (shorter, _, _) = take_prefix_by_width(chunk, available);
                    out.push(
                        vec![chunk_prefix.dim(), shorter.dim(), truncation_suffix.dim()].into(),
                    );
                } else {
                    out.push(vec![chunk_prefix.dim(), truncated.dim()].into());
                }
            }
            shown += 1;
        }

        let remaining = self.processes.len().saturating_sub(shown);
        if remaining > 0 {
            let more_text = format!("... and {remaining} more running");
            if wrap_width <= prefix_width {
                out.push(Line::from(prefix.dim()));
            } else {
                let budget = wrap_width.saturating_sub(prefix_width);
                let (truncated, _, _) = take_prefix_by_width(&more_text, budget);
                out.push(vec![prefix.dim(), truncated.dim()].into());
            }
        }

        out
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.display_lines(width).len() as u16
    }
}

pub(crate) fn new_unified_exec_processes_output(
    processes: Vec<UnifiedExecProcessDetails>,
) -> CompositeHistoryCell {
    let command = PlainHistoryCell::new(vec!["/ps".magenta().into()]);
    let summary = UnifiedExecProcessesCell::new(processes);
    CompositeHistoryCell::new(vec![Box::new(command), Box::new(summary)])
}

fn truncate_exec_snippet(full_cmd: &str) -> String {
    let mut snippet = match full_cmd.split_once('\n') {
        Some((first, _)) => format!("{first} ..."),
        None => full_cmd.to_string(),
    };
    snippet = truncate_text(&snippet, /*max_graphemes*/ 80);
    snippet
}

fn exec_snippet(command: &[String]) -> String {
    let full_cmd = strip_bash_lc_and_escape(command);
    truncate_exec_snippet(&full_cmd)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecisionActor {
    User,
    Guardian,
}

impl ApprovalDecisionActor {
    fn subject(self) -> &'static str {
        match self {
            Self::User => "You ",
            Self::Guardian => "Auto-reviewer ",
        }
    }
}

pub fn new_guardian_denied_patch_request(files: Vec<String>) -> Box<dyn HistoryCell> {
    let mut summary = vec![
        "Request ".into(),
        "denied".bold(),
        " for Devo to apply ".into(),
    ];
    if files.len() == 1 {
        summary.push("a patch touching ".into());
        summary.push(Span::from(files[0].clone()).dim());
    } else {
        summary.push("a patch touching ".into());
        summary.push(Span::from(files.len().to_string()).dim());
        summary.push(" files".into());
    }

    Box::new(PrefixedWrappedHistoryCell::new(
        Line::from(summary),
        "✗ ".red(),
        "  ",
    ))
}

pub(crate) fn new_guardian_denied_action_request(summary: String) -> PrefixedWrappedHistoryCell {
    let line = Line::from(vec![
        "Request ".into(),
        "denied".bold(),
        " for ".into(),
        Span::from(summary).dim(),
    ]);
    PrefixedWrappedHistoryCell::new(line, "✗ ".red(), "  ")
}

pub(crate) fn new_guardian_approved_action_request(summary: String) -> PrefixedWrappedHistoryCell {
    let line = Line::from(vec![
        "Request ".into(),
        "approved".bold(),
        " for ".into(),
        Span::from(summary).dim(),
    ]);
    PrefixedWrappedHistoryCell::new(line, "✔ ".green(), "  ")
}

pub fn new_permission_request_cell(title: String, body: String) -> Box<dyn HistoryCell> {
    let mut lines = vec![Line::from(vec![
        "Permission required: ".yellow().bold(),
        Span::from(title),
    ])];
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        lines.push(Line::from(line.to_string()).dim());
    }
    lines.push(Line::from(vec![
        "Press ".dim(),
        "y".bold(),
        " once, ".dim(),
        "s".bold(),
        " session, ".dim(),
        "n".bold(),
        " deny, ".dim(),
        "Esc".bold(),
        " cancel".dim(),
    ]));
    Box::new(PlainHistoryCell::new(prefix_lines(
        lines,
        "? ".yellow(),
        "  ".into(),
    )))
}

/// Cyan history cell line showing the current review status.
pub(crate) fn new_review_status_line(message: String) -> PlainHistoryCell {
    PlainHistoryCell {
        lines: vec![Line::from(message.cyan())],
    }
}

#[derive(Debug)]
pub(crate) struct PatchHistoryCell {
    changes: HashMap<PathBuf, FileChange>,
    cwd: PathBuf,
}

impl HistoryCell for PatchHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        create_diff_summary(&self.changes, &self.cwd, width as usize)
    }
}

#[derive(Debug)]
struct CompletedMcpToolCallWithImageOutput {
    _image: DynamicImage,
}
impl HistoryCell for CompletedMcpToolCallWithImageOutput {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec!["tool result (image output)".into()]
    }
}

/// Render `lines` inside a border sized to the widest span in the content.
pub(crate) fn with_border(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    with_border_internal(lines, /*forced_inner_width*/ None)
}

/// Render `lines` inside a border whose inner width is at least `inner_width`.
///
/// This is useful when callers have already clamped their content to a
/// specific width and want the border math centralized here instead of
/// duplicating padding logic in the TUI widgets themselves.
pub(crate) fn with_border_with_inner_width(
    lines: Vec<Line<'static>>,
    inner_width: usize,
) -> Vec<Line<'static>> {
    with_border_internal(lines, Some(inner_width))
}

fn with_border_internal(
    lines: Vec<Line<'static>>,
    forced_inner_width: Option<usize>,
) -> Vec<Line<'static>> {
    let max_line_width = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    let content_width = forced_inner_width
        .unwrap_or(max_line_width)
        .max(max_line_width);

    let mut out = Vec::with_capacity(lines.len() + 2);
    let border_inner_width = content_width + 2;
    out.push(vec![format!("╭{}╮", "─".repeat(border_inner_width)).dim()].into());

    for line in lines.into_iter() {
        let used_width: usize = line
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        let span_count = line.spans.len();
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(span_count + 4);
        spans.push(Span::from("│ ").dim());
        spans.extend(line.into_iter());
        if used_width < content_width {
            spans.push(Span::from(" ".repeat(content_width - used_width)).dim());
        }
        spans.push(Span::from(" │").dim());
        out.push(Line::from(spans));
    }

    out.push(vec![format!("╰{}╯", "─".repeat(border_inner_width)).dim()].into());

    out
}

/// Return the emoji followed by a hair space (U+200A).
/// Using only the hair space avoids excessive padding after the emoji while
/// still providing a small visual gap across terminals.
pub(crate) fn padded_emoji(emoji: &str) -> String {
    format!("{emoji}\u{200A}")
}

#[derive(Debug)]
struct TooltipHistoryCell {
    tip: String,
    cwd: PathBuf,
}

impl TooltipHistoryCell {
    fn new(tip: String, cwd: &Path) -> Self {
        Self {
            tip,
            cwd: cwd.to_path_buf(),
        }
    }
}

impl HistoryCell for TooltipHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let indent = "  ";
        let indent_width = UnicodeWidthStr::width(indent);
        let wrap_width = usize::from(width.max(1))
            .saturating_sub(indent_width)
            .max(1);
        let mut lines: Vec<Line<'static>> = Vec::new();
        append_markdown(
            &format!("**Tip:** {}", self.tip),
            Some(wrap_width),
            Some(self.cwd.as_path()),
            &mut lines,
        );

        prefix_lines(lines, indent.into(), indent.into())
    }
}

#[derive(Debug)]
pub struct SessionInfoCell(CompositeHistoryCell);

impl HistoryCell for SessionInfoCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.0.display_lines(width)
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.0.desired_height(width)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.0.transcript_lines(width)
    }
}

#[allow(dead_code)]
fn random_tip() -> String {
    let tips = ThemeSet::tips();
    if tips.is_empty() {
        return String::new();
    }
    use rand::seq::IndexedRandom as _;
    tips.choose(&mut rand::rng())
        .copied()
        .unwrap_or("")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn new_session_info(
    cwd: &Path,
    requested_model: &str,
    header_model: String,
    resolved_model: String,
    reasoning_capability: ReasoningCapability,
    default_reasoning_effort: Option<ReasoningEffort>,
    reasoning_implementation: Option<ReasoningImplementation>,
    is_first_event: bool,
    tooltip_override: Option<String>,
    show_fast_status: bool,
    accent_color: Color,
    mascot_frame_index: usize,
) -> SessionInfoCell {
    // Header box rendered as history (so it appears at the very top)
    let header = HeaderHistoryCell::new(
        header_model.clone(),
        reasoning_capability,
        default_reasoning_effort,
        reasoning_implementation,
        show_fast_status,
        cwd.to_path_buf(),
        CLI_VERSION,
        accent_color,
        mascot_frame_index,
    );
    let mut parts: Vec<Box<dyn HistoryCell>> = vec![Box::new(header)];

    if !is_first_event {
        if let Some(tip) = tooltip_override {
            parts.push(Box::new(TooltipHistoryCell::new(tip, cwd)));
        }
        if requested_model != header_model {
            let lines = vec![
                "model changed:".magenta().bold().into(),
                format!("requested: {requested_model}").into(),
                format!("used: {resolved_model}").into(),
            ];
            parts.push(Box::new(PlainHistoryCell { lines }));
        }
    }

    SessionInfoCell(CompositeHistoryCell { parts })
}

pub(crate) fn new_user_prompt(
    message: String,
    text_elements: Vec<TextElement>,
    local_image_paths: Vec<PathBuf>,
    remote_image_urls: Vec<String>,
    accent_color: Color,
    input_mode: InputMode,
) -> UserHistoryCell {
    UserHistoryCell {
        message,
        text_elements,
        local_image_paths,
        remote_image_urls,
        accent_color,
        input_mode,
        selected: false,
    }
}

#[derive(Debug)]
pub(crate) struct HeaderHistoryCell {
    version: &'static str,
    model: String,
    model_style: Style,
    reasoning_capability: ReasoningCapability,
    default_reasoning_effort: Option<ReasoningEffort>,
    reasoning_implementation: Option<ReasoningImplementation>,
    show_fast_status: bool,
    directory: PathBuf,
    accent_color: Color,
    mascot_frame_index: usize,
}

impl HeaderHistoryCell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        model: String,
        reasoning_capability: ReasoningCapability,
        default_reasoning_effort: Option<ReasoningEffort>,
        reasoning_implementation: Option<ReasoningImplementation>,
        show_fast_status: bool,
        directory: PathBuf,
        version: &'static str,
        accent_color: Color,
        mascot_frame_index: usize,
    ) -> Self {
        Self::new_with_style(
            model,
            Style::default(),
            reasoning_capability,
            default_reasoning_effort,
            reasoning_implementation,
            show_fast_status,
            directory,
            version,
            accent_color,
            mascot_frame_index,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_style(
        model: String,
        model_style: Style,
        reasoning_capability: ReasoningCapability,
        default_reasoning_effort: Option<ReasoningEffort>,
        reasoning_implementation: Option<ReasoningImplementation>,
        show_fast_status: bool,
        directory: PathBuf,
        version: &'static str,
        accent_color: Color,
        mascot_frame_index: usize,
    ) -> Self {
        Self {
            version,
            model,
            model_style,
            reasoning_capability,
            default_reasoning_effort,
            reasoning_implementation,
            show_fast_status,
            directory,
            accent_color,
            mascot_frame_index,
        }
    }

    fn reasoning_label(&self) -> Option<&'static str> {
        if matches!(self.reasoning_capability, ReasoningCapability::Unsupported)
            || matches!(
                self.reasoning_implementation,
                Some(ReasoningImplementation::Disabled)
            )
        {
            return None;
        }

        match &self.reasoning_capability {
            ReasoningCapability::Toggle => Some("reasoning"),
            ReasoningCapability::ToggleWithLevels(levels) => self
                .default_reasoning_effort
                .or_else(|| levels.first().copied())
                .map(|effort| match effort {
                    ReasoningEffort::None => "none",
                    ReasoningEffort::Minimal => "minimal",
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::XHigh => "xhigh",
                    ReasoningEffort::Max => "max",
                }),
            ReasoningCapability::Levels(levels) => self
                .default_reasoning_effort
                .or_else(|| levels.first().copied())
                .map(|effort| match effort {
                    ReasoningEffort::None => "none",
                    ReasoningEffort::Minimal => "minimal",
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::XHigh => "xhigh",
                    ReasoningEffort::Max => "max",
                }),
            ReasoningCapability::Unsupported => None,
        }
    }
}

impl HistoryCell for HeaderHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let reasoning = self.reasoning_label().unwrap_or("unknown");
        let _ = self.show_fast_status;
        let _ = self.model_style;
        build_startup_header(
            StartupHeaderData {
                version: self.version,
                model: &self.model,
                reasoning,
                directory: &self.directory,
                accent_color: self.accent_color,
                mascot_frame_index: self.mascot_frame_index,
            },
            width,
        )
    }
}

#[derive(Debug)]
pub(crate) struct CompositeHistoryCell {
    parts: Vec<Box<dyn HistoryCell>>,
}

impl CompositeHistoryCell {
    pub(crate) fn new(parts: Vec<Box<dyn HistoryCell>>) -> Self {
        Self { parts }
    }
}

impl HistoryCell for CompositeHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut first = true;
        for part in &self.parts {
            let mut lines = part.display_lines(width);
            if !lines.is_empty() {
                if !first {
                    out.push(Line::from(""));
                }
                out.append(&mut lines);
                first = false;
            }
        }
        out
    }
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn new_warning_event(message: String) -> PrefixedWrappedHistoryCell {
    PrefixedWrappedHistoryCell::new(message.yellow(), "⚠ ".yellow(), "  ")
}

#[derive(Debug)]
pub(crate) struct DeprecationNoticeCell {
    summary: String,
    details: Option<String>,
}

pub(crate) fn new_deprecation_notice(
    summary: String,
    details: Option<String>,
) -> DeprecationNoticeCell {
    DeprecationNoticeCell { summary, details }
}

impl HistoryCell for DeprecationNoticeCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(vec!["⚠ ".red().bold(), self.summary.clone().red()].into());

        let wrap_width = width.saturating_sub(4).max(1) as usize;

        if let Some(details) = &self.details {
            let detail_line = Line::from(details.clone().dim());
            let wrapped = adaptive_wrap_line(&detail_line, RtOptions::new(wrap_width));
            push_owned_lines(&wrapped, &mut lines);
        }

        lines
    }
}

pub(crate) fn new_info_event(message: String, hint: Option<String>) -> PlainHistoryCell {
    let mut line = vec!["▌ ".dim(), message.into()];
    if let Some(hint) = hint {
        line.push(" ".into());
        line.push(hint.dark_gray());
    }
    let lines: Vec<Line<'static>> = vec![line.into()];
    PlainHistoryCell { lines }
}

pub(crate) fn new_live_aligned_info_event(
    message: String,
    hint: Option<String>,
) -> PlainHistoryCell {
    let mut line = vec![" ".repeat(LIVE_PREFIX_COLS as usize).into(), message.into()];
    if let Some(hint) = hint {
        line.push(" ".into());
        line.push(hint.dark_gray());
    }
    let lines: Vec<Line<'static>> = vec![line.into()];
    PlainHistoryCell { lines }
}

pub(crate) fn new_error_event(message: String) -> PlainHistoryCell {
    new_error_event_with_hint(message, /*hint*/ None)
}

pub(crate) fn new_error_event_with_hint(message: String, hint: Option<String>) -> PlainHistoryCell {
    // Use a hair space (U+200A) to create a subtle, near-invisible separation
    // before the text. VS16 is intentionally omitted to keep spacing tighter
    // in terminals like Ghostty.
    let mut lines: Vec<Line<'static>> = vec![vec![format!("■ {message}").red()].into()];
    if let Some(hint) = hint {
        lines.push(vec!["  ".into(), hint.dark_gray()].into());
    }
    PlainHistoryCell { lines }
}

pub(crate) fn new_live_aligned_error_event_with_hint(
    message: String,
    hint: Option<String>,
) -> PlainHistoryCell {
    let prefix = " ".repeat(LIVE_PREFIX_COLS as usize);
    let mut lines: Vec<Line<'static>> =
        vec![vec![prefix.into(), format!("■ {message}").red()].into()];
    if let Some(hint) = hint {
        lines.push(
            vec![
                " ".repeat(LIVE_PREFIX_COLS as usize).into(),
                hint.dark_gray(),
            ]
            .into(),
        );
    }
    PlainHistoryCell { lines }
}

/// A transient history cell that shows an animated spinner while the MCP
/// inventory RPC is in flight.
///
/// Inserted as the `active_cell` by `ChatWidget::add_mcp_output()` and removed
/// once the fetch completes. The app removes committed copies from transcript
/// history, while `ChatWidget::clear_mcp_inventory_loading()` only clears the
/// in-flight `active_cell`.
#[derive(Debug)]
pub(crate) struct McpInventoryLoadingCell {
    start_time: Instant,
    animations_enabled: bool,
}

impl McpInventoryLoadingCell {
    pub(crate) fn new(animations_enabled: bool) -> Self {
        Self {
            start_time: Instant::now(),
            animations_enabled,
        }
    }
}

impl HistoryCell for McpInventoryLoadingCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![
            vec![
                spinner(Some(self.start_time), self.animations_enabled),
                " ".into(),
                "Loading MCP inventory".bold(),
                "…".dim(),
            ]
            .into(),
        ]
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        if !self.animations_enabled {
            return None;
        }
        Some((self.start_time.elapsed().as_millis() / 50) as u64)
    }
}

/// Wrap a plain string with textwrap and prefix each line, while applying a style to the content.
fn wrap_with_prefix(
    text: &str,
    width: usize,
    initial_prefix: Span<'static>,
    subsequent_prefix: Span<'static>,
    style: Style,
) -> Vec<Line<'static>> {
    let line = Line::from(vec![Span::from(text.to_string()).set_style(style)]);
    let opts = RtOptions::new(width.max(1))
        .initial_indent(Line::from(vec![initial_prefix]))
        .subsequent_indent(Line::from(vec![subsequent_prefix]));
    let wrapped = adaptive_wrap_line(&line, opts);
    let mut out = Vec::new();
    push_owned_lines(&wrapped, &mut out);
    out
}

/// Create a new `PendingPatch` cell that lists the file‑level summary of
/// a proposed patch. The summary lines should already be formatted (e.g.
/// "A path/to/file.rs").
pub(crate) fn new_patch_event(
    changes: HashMap<PathBuf, FileChange>,
    cwd: &Path,
) -> PatchHistoryCell {
    PatchHistoryCell {
        changes,
        cwd: cwd.to_path_buf(),
    }
}

pub(crate) fn new_patch_apply_failure(stderr: String) -> PlainHistoryCell {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Failure title
    lines.push(Line::from("✘ Failed to apply patch".magenta().bold()));

    if !stderr.trim().is_empty() {
        let output = output_lines(
            Some(&CommandOutput {
                exit_code: 1,
                formatted_output: String::new(),
                aggregated_output: stderr,
            }),
            OutputLinesParams {
                line_limit: TOOL_CALL_MAX_LINES,
                only_err: true,
                include_angle_pipe: true,
                include_prefix: true,
                dim: true,
            },
        );
        lines.extend(output.lines);
    }

    PlainHistoryCell { lines }
}

pub(crate) fn new_view_image_tool_call(path: PathBuf, cwd: &Path) -> PlainHistoryCell {
    let display_path = display_path_for(&path, cwd);

    let lines: Vec<Line<'static>> = vec![
        vec!["▌ ".dim(), "Viewed Image".bold()].into(),
        vec!["  └ ".dim(), display_path.dim()].into(),
    ];

    PlainHistoryCell { lines }
}

pub(crate) fn new_image_generation_call(
    call_id: String,
    revised_prompt: Option<String>,
    saved_path: Option<String>,
) -> PlainHistoryCell {
    let detail = revised_prompt.unwrap_or_else(|| call_id.clone());

    let mut lines: Vec<Line<'static>> = vec![
        vec!["▌ ".dim(), "Generated Image:".bold()].into(),
        vec!["  └ ".dim(), detail.dim()].into(),
    ];
    if let Some(saved_path) = saved_path {
        lines.push(vec!["  └ ".dim(), "Saved to: ".dim(), saved_path.into()].into());
    }

    PlainHistoryCell { lines }
}

/// Create the reasoning history cell emitted at the end of a reasoning block.
///
/// The helper snapshots `cwd` into the returned cell so local file links render the same way they
/// did while the turn was live, even if rendering happens after other app state has advanced.
pub(crate) fn new_reasoning_summary_block(
    full_reasoning_buffer: String,
    cwd: &Path,
) -> Box<dyn HistoryCell> {
    let cwd = cwd.to_path_buf();
    let full_reasoning_buffer = full_reasoning_buffer.trim();
    if let Some(open) = full_reasoning_buffer.find("**") {
        let after_open = &full_reasoning_buffer[(open + 2)..];
        if let Some(close) = after_open.find("**") {
            let after_close_idx = open + 2 + close + 2;
            // if we don't have anything beyond `after_close_idx`
            // then we don't have a summary to inject into history
            if after_close_idx < full_reasoning_buffer.len() {
                let header_buffer = full_reasoning_buffer[..after_close_idx].to_string();
                let summary_buffer = full_reasoning_buffer[after_close_idx..].to_string();
                // Preserve the session cwd so local file links render the same way in the
                // collapsed reasoning block as they did while streaming live content.
                return Box::new(ReasoningSummaryCell::new(
                    header_buffer,
                    summary_buffer,
                    &cwd,
                    ReasoningViewportMode::Full,
                ));
            }
        }
    }
    Box::new(ReasoningSummaryCell::new(
        "".to_string(),
        full_reasoning_buffer.to_string(),
        &cwd,
        ReasoningViewportMode::TranscriptOnly,
    ))
}

#[derive(Debug)]
/// A visual divider between turns, optionally showing how long the assistant "worked for".
///
/// This separator is only emitted for turns that performed concrete work (e.g., running commands,
/// applying patches, making MCP tool calls), so purely conversational turns do not show an empty
/// divider.
pub struct FinalMessageSeparator {
    elapsed_seconds: Option<u64>,
}
impl FinalMessageSeparator {
    /// Creates a separator; `elapsed_seconds` typically comes from the status indicator timer.
    pub(crate) fn new(elapsed_seconds: Option<u64>) -> Self {
        Self { elapsed_seconds }
    }
}
impl HistoryCell for FinalMessageSeparator {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut label_parts = Vec::new();
        if let Some(elapsed_seconds) = self
            .elapsed_seconds
            .filter(|seconds| *seconds > 60)
            .map(super::status_indicator_widget::fmt_elapsed_compact)
        {
            label_parts.push(format!("Worked for {elapsed_seconds}"));
        }

        if label_parts.is_empty() {
            return vec![Line::from_iter(["─".repeat(width as usize).dim()])];
        }

        let label = format!("─ {} ─", label_parts.join(" ▌ "));
        let (label, _suffix, label_width) = take_prefix_by_width(&label, width as usize);
        vec![
            Line::from_iter([
                label,
                "─".repeat((width as usize).saturating_sub(label_width)),
            ])
            .dim(),
        ]
    }
}

/// End-of-turn summary showing ▣ symbol, input mode, model name, and outcome.
///
/// Inspired by opencode's assistant message footer, with Devo's mode label included:
/// `▣ BUILD · model-name · 4m17s`, `▣ PLAN · model-name · interrupted`, or
/// `▣ BUILD · model-name · failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug)]
pub struct TurnSummaryCell {
    pub input_mode: InputMode,
    pub model_name: String,
    pub duration: Option<u64>,
    outcome: TurnOutcome,
}

impl TurnSummaryCell {
    pub(crate) fn new(
        input_mode: InputMode,
        model_name: String,
        duration: Option<u64>,
        _accent_color: Color,
    ) -> Self {
        Self {
            input_mode,
            model_name,
            duration,
            outcome: TurnOutcome::Completed,
        }
    }

    pub(crate) fn new_interrupted(
        input_mode: InputMode,
        model_name: String,
        _accent_color: Color,
    ) -> Self {
        Self {
            input_mode,
            model_name,
            duration: None,
            outcome: TurnOutcome::Interrupted,
        }
    }

    pub(crate) fn new_failed(
        input_mode: InputMode,
        model_name: String,
        _accent_color: Color,
    ) -> Self {
        Self {
            input_mode,
            model_name,
            duration: None,
            outcome: TurnOutcome::Failed,
        }
    }
}

fn format_duration_hms(duration_secs: u64) -> String {
    let hours = duration_secs / 3_600;
    let minutes = (duration_secs % 3_600) / 60;
    let seconds = duration_secs % 60;

    if hours > 0 {
        match (minutes, seconds) {
            (0, 0) => format!("{hours}h"),
            (_, 0) => format!("{hours}h{minutes}m"),
            _ => format!("{hours}h{minutes}m{seconds}s"),
        }
    } else if minutes > 0 {
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m{seconds}s")
        }
    } else {
        format!("{seconds}s")
    }
}

impl HistoryCell for TurnSummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let _ = width;
        let mode_color = self.input_mode.color();
        let mut spans: Vec<Span<'static>> = vec![
            Span::raw(" ".repeat(LIVE_PREFIX_COLS as usize)),
            Span::styled("▣", Style::default().fg(mode_color)),
            Span::styled(" ", Style::default()),
            Span::styled(self.input_mode.label(), Style::default().fg(mode_color)),
            Span::styled(" · ", Style::default().dim()),
            Span::styled(self.model_name.clone(), Style::default().dim()),
        ];
        let outcome_suffix = match self.outcome {
            TurnOutcome::Completed => self.duration.map(format_duration_hms),
            TurnOutcome::Interrupted => Some("interrupted".to_string()),
            TurnOutcome::Failed => Some("failed".to_string()),
        };
        if let Some(outcome_suffix) = outcome_suffix {
            spans.push(Span::styled(" · ", Style::default().dim()));
            spans.push(Span::styled(outcome_suffix, Style::default().dim()));
        }
        vec![Line::from(spans)]
    }
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        let seconds = duration_ms as f64 / 1_000.0;
        format!("{seconds:.1}s")
    } else {
        format!("{duration_ms}ms")
    }
}

fn pluralize(count: u64, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use crate::bottom_pane::InputMode;

    use super::HistoryCell;
    use pretty_assertions::assert_eq;

    use super::format_duration_hms;
    use super::new_user_prompt;

    fn content_spans_for_message(message: &str) -> Vec<(String, Option<Color>)> {
        let cell = new_user_prompt(
            message.to_string(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Color::Yellow,
            InputMode::Build,
        );
        cell.display_lines(80)
            .into_iter()
            .find(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .contains(message)
            })
            .unwrap_or_else(|| panic!("missing rendered message line for {message:?}"))
            .spans
            .into_iter()
            .map(|span| (span.content.to_string(), span.style.fg))
            .collect()
    }

    #[test]
    fn turn_summary_duration_uses_hour_minute_second_units() {
        assert_eq!(format_duration_hms(3), "3s");
        assert_eq!(format_duration_hms(60), "1m");
        assert_eq!(format_duration_hms(257), "4m17s");
        assert_eq!(format_duration_hms(3_600), "1h");
        assert_eq!(format_duration_hms(3_601), "1h0m1s");
        assert_eq!(format_duration_hms(3_723), "1h2m3s");
    }

    #[test]
    fn user_prompt_highlights_leading_slash_command_without_text_element() {
        assert_eq!(
            content_spans_for_message("/btw check this"),
            vec![
                ("▌ ".to_string(), Some(Color::Cyan)),
                ("/btw".to_string(), Some(Color::Yellow)),
                (" check this".to_string(), None),
            ]
        );
    }

    #[test]
    fn user_prompt_does_not_highlight_unknown_slash_command() {
        assert_eq!(
            content_spans_for_message("/unknown check this"),
            vec![
                ("▌ ".to_string(), Some(Color::Cyan)),
                ("/unknown check this".to_string(), None),
            ]
        );
    }

    #[test]
    fn compact_reasoning_summary_truncates_and_hints_transcript() {
        use std::path::Path;

        use super::REASONING_TRANSCRIPT_HINT;
        use super::ReasoningSummaryCell;
        use super::ReasoningViewportMode;

        let cell = ReasoningSummaryCell::new(
            String::new(),
            "abcdefghijklmnopqrstuvwxyz0123456789 long reasoning preview".to_string(),
            Path::new("."),
            ReasoningViewportMode::Compact,
        );
        let lines = cell.display_lines(24);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered.len(), 2);
        assert!(
            rendered[0].ends_with('…'),
            "Thought line should truncate with ellipsis: {}",
            rendered[0]
        );
        assert!(
            !rendered[0].contains("long reasoning"),
            "Thought line should not include the untruncated tail: {}",
            rendered[0]
        );
        assert_eq!(rendered[1].trim(), REASONING_TRANSCRIPT_HINT);
    }
}
