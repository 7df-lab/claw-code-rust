//! Reasoning-view picker data for the chat widget.
//!
//! Controls whether reasoning content is shown in full or collapsed in the
//! main transcript viewport.

use std::path::Path;
use std::path::PathBuf;

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::app_event::AppEvent;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::history_cell::ReasoningViewportMode;
use crate::history_cell::collapse_consecutive_blank_lines;
use crate::markdown::append_markdown;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

/// Maximum *visual* terminal rows for collapsed reasoning **body** content.
///
/// Counted after wrap, not by markdown/logical newlines — a single long
/// paragraph that wraps to many rows still counts toward this budget.
/// Live streaming keeps a sticky `▌ Thinking:` heading above these body rows.
pub(super) const COLLAPSED_REASONING_LIVE_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollapsedReasoningMode {
    /// Streaming: sticky Thinking heading + latest wrapped body rows.
    Live,
    /// Committed: short bodies stay full; longer bodies compact to Thought.
    Completed,
}

/// Width-aware collapsed reasoning cell.
///
/// Truncates by wrapped visual height so one long logical line cannot expand
/// past [`COLLAPSED_REASONING_LIVE_LINES`] rows in the main viewport.
#[derive(Debug)]
struct CollapsedReasoningCell {
    content: String,
    cwd: PathBuf,
    heading: String,
    heading_style: Style,
    text_style: Style,
    initial_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
    mode: CollapsedReasoningMode,
}

impl CollapsedReasoningCell {
    fn body_lines(&self) -> Vec<Line<'static>> {
        let mut body_lines = Vec::new();
        append_markdown(
            &self.content,
            /*width*/ None,
            Some(self.cwd.as_path()),
            &mut body_lines,
        );
        for line in &mut body_lines {
            line.spans = line
                .spans
                .iter()
                .cloned()
                .map(|span| span.patch_style(self.text_style))
                .collect();
        }
        body_lines
    }

    /// Sticky live header: `▌ Thinking:` — never scrolls away with the body tail.
    fn sticky_heading_line(&self) -> Line<'static> {
        let mut spans = self.initial_prefix.spans.clone();
        spans.push(Span::styled(self.heading.clone(), self.heading_style));
        Line {
            style: self.initial_prefix.style,
            alignment: self.initial_prefix.alignment,
            spans,
        }
    }

    /// Body-only wrap for live streaming (heading is rendered separately).
    fn wrap_body_only(&self, width: u16) -> Vec<Line<'static>> {
        let body_lines = self.body_lines();
        collapse_consecutive_blank_lines(adaptive_wrap_lines(
            &body_lines,
            RtOptions::new(width as usize)
                .initial_indent(self.subsequent_prefix.clone())
                .subsequent_indent(self.subsequent_prefix.clone()),
        ))
    }

    fn wrap_body_with_heading(&self, width: u16) -> Vec<Line<'static>> {
        let mut body_lines = self.body_lines();
        if let Some(first_line) = body_lines.first_mut() {
            first_line
                .spans
                .insert(0, Span::styled(self.heading.clone(), self.heading_style));
        }
        collapse_consecutive_blank_lines(adaptive_wrap_lines(
            &body_lines,
            RtOptions::new(width as usize)
                .initial_indent(self.initial_prefix.clone())
                .subsequent_indent(self.subsequent_prefix.clone()),
        ))
    }

    fn take_last_visual_rows(mut lines: Vec<Line<'static>>, max_rows: usize) -> Vec<Line<'static>> {
        if lines.len() > max_rows {
            lines = lines.split_off(lines.len() - max_rows);
        }
        lines
    }

    fn compact_lines(&self, width: u16) -> Vec<Line<'static>> {
        history_cell::ReasoningSummaryCell::new(
            String::new(),
            self.content.clone(),
            &self.cwd,
            ReasoningViewportMode::Compact,
        )
        .display_lines(width)
    }

    fn live_display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![self.sticky_heading_line()];
        lines.extend(Self::take_last_visual_rows(
            self.wrap_body_only(width),
            COLLAPSED_REASONING_LIVE_LINES,
        ));
        lines.push(history_cell::reasoning_transcript_hint_line());
        lines
    }
}

impl HistoryCell for CollapsedReasoningCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self.mode {
            CollapsedReasoningMode::Live => self.live_display_lines(width),
            CollapsedReasoningMode::Completed => {
                let wrapped = self.wrap_body_with_heading(width);
                if wrapped.len() <= COLLAPSED_REASONING_LIVE_LINES {
                    let mut lines = wrapped;
                    lines.push(history_cell::reasoning_transcript_hint_line());
                    lines
                } else {
                    self.compact_lines(width)
                }
            }
        }
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        history_cell::ReasoningSummaryCell::new(
            String::new(),
            self.content.clone(),
            &self.cwd,
            ReasoningViewportMode::Full,
        )
        .transcript_lines(width)
    }
}

pub(super) fn reasoning_view_items(collapse_reasoning: bool) -> Vec<SelectionItem> {
    [(true, "Collapsed"), (false, "Full")]
        .into_iter()
        .map(|(collapsed, label)| SelectionItem {
            name: label.to_string(),
            description: None,
            is_current: collapsed == collapse_reasoning,
            dismiss_on_select: true,
            actions: vec![Box::new(move |app_event_tx| {
                app_event_tx.send(AppEvent::CollapseReasoningSelected { collapsed });
            })],
            ..Default::default()
        })
        .collect()
}

pub(super) fn reasoning_view_label(collapse_reasoning: bool) -> &'static str {
    if collapse_reasoning {
        "Collapsed"
    } else {
        "Full"
    }
}

/// Live streaming cell for collapsed reasoning (latest visual rows only).
pub(super) fn collapsed_reasoning_live_cell(
    content: String,
    cwd: &Path,
    status_heading: &str,
    status_heading_style: Style,
    reasoning_text_style: Style,
    dot_prefix: Line<'static>,
) -> Box<dyn HistoryCell> {
    Box::new(CollapsedReasoningCell {
        content,
        cwd: cwd.to_path_buf(),
        heading: status_heading.to_string(),
        heading_style: status_heading_style,
        text_style: reasoning_text_style,
        initial_prefix: dot_prefix,
        subsequent_prefix: "  ".into(),
        mode: CollapsedReasoningMode::Live,
    })
}

/// Build the committed reasoning cell for collapsed mode.
///
/// Short reasoning (≤ [`COLLAPSED_REASONING_LIVE_LINES`] *visual* rows after
/// wrap) stays fully visible. Longer reasoning becomes a one-line Thought
/// summary in the main viewport, with the full body kept for Ctrl+T.
pub(super) fn collapsed_reasoning_history_cell(
    content: String,
    cwd: &Path,
    status_heading: &str,
    status_heading_style: Style,
    reasoning_text_style: Style,
    dot_prefix: Line<'static>,
) -> Box<dyn HistoryCell> {
    Box::new(CollapsedReasoningCell {
        content,
        cwd: cwd.to_path_buf(),
        heading: status_heading.to_string(),
        heading_style: status_heading_style,
        text_style: reasoning_text_style,
        initial_prefix: dot_prefix,
        subsequent_prefix: "  ".into(),
        mode: CollapsedReasoningMode::Completed,
    })
}
