//! Transcript overlay and live viewport projections for `ChatWidget`.
//!
//! This module converts committed and active history cells into the line
//! snapshots consumed by the Ctrl+T overlay, scrollback drain, and live view.

use ratatui::text::Line;

use crate::agent_tool_cell::AgentToolCell;
use crate::agent_tool_cell::is_agent_task_tool_name;
use crate::events::TextItemKind;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::history_cell::ScrollbackLine;
use crate::render::line_utils::is_horizontal_rule_line;
use crate::tool_io_cell::ToolIoCell;
use crate::tool_io_cell::ToolIoCellOptions;
use crate::transcript::model::ToolPhase;
use crate::transcript::presentation::tool_title_line;
use crate::transcript::presentation::tool_title_parts;

use super::ChatWidget;
use super::UserMessage;

/// Snapshot of active-cell state that affects transcript overlay rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveCellTranscriptKey {
    pub(crate) revision: u64,
    pub(crate) is_stream_continuation: bool,
    pub(crate) animation_tick: Option<u64>,
}

/// Snapshot of one committed transcript cell for the Ctrl+T overlay.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptOverlayCell {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) is_stream_continuation: bool,
    pub(crate) user_message: Option<UserMessage>,
    pub(crate) is_selected_user: bool,
}

enum LiveViewportLineMode {
    Display,
    Transcript,
}

#[allow(clippy::large_enum_variant)]
enum LiveItem {
    Text(usize),
    Tool(String),
}

impl ChatWidget {
    pub(crate) fn active_cell_transcript_key(&self) -> Option<ActiveCellTranscriptKey> {
        let active_cell = self.active_cell.as_ref()?;
        Some(ActiveCellTranscriptKey {
            revision: self.active_cell_revision,
            is_stream_continuation: active_cell.is_stream_continuation(),
            animation_tick: active_cell.transcript_animation_tick(),
        })
    }

    pub(crate) fn active_cell_transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.active_cell
            .as_ref()
            .map(|cell| cell.transcript_lines(width))
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn active_cell_display_lines_for_test(&self, width: u16) -> Vec<Line<'static>> {
        self.active_cell
            .as_ref()
            .map(|cell| cell.display_lines(width))
            .unwrap_or_default()
    }

    pub(crate) fn transcript_overlay_cell_count(&self) -> usize {
        self.history.len()
    }

    pub(crate) fn transcript_overlay_cells(&self, width: u16) -> Vec<TranscriptOverlayCell> {
        let width = width.max(1);
        self.history
            .iter()
            .map(|cell| {
                let user_message = cell
                    .as_any()
                    .downcast_ref::<history_cell::UserHistoryCell>()
                    .map(|user| UserMessage {
                        text: user.message.clone(),
                        local_images: user
                            .local_image_paths
                            .iter()
                            .cloned()
                            .map(|path| crate::bottom_pane::LocalImageAttachment {
                                path,
                                placeholder: String::new(),
                            })
                            .collect(),
                        remote_image_urls: user.remote_image_urls.clone(),
                        text_elements: user.text_elements.clone(),
                        mention_bindings: Vec::new(),
                    });
                TranscriptOverlayCell {
                    lines: cell.transcript_lines(width),
                    is_stream_continuation: cell.is_stream_continuation(),
                    user_message,
                    is_selected_user: false,
                }
            })
            .collect()
    }

    pub(crate) fn transcript_overlay_live_tail_key(&self) -> Option<ActiveCellTranscriptKey> {
        if !self.transcript_overlay_has_live_tail() {
            return None;
        }

        let active_cell = self.active_cell.as_ref();
        Some(ActiveCellTranscriptKey {
            revision: self.active_cell_revision,
            is_stream_continuation: active_cell.is_some_and(|cell| cell.is_stream_continuation()),
            animation_tick: active_cell.and_then(|cell| cell.transcript_animation_tick()),
        })
    }

    pub(crate) fn transcript_overlay_live_tail_lines(
        &self,
        width: u16,
    ) -> Option<Vec<Line<'static>>> {
        self.transcript_overlay_has_live_tail()
            .then(|| self.live_transcript_lines(width.max(1)))
    }

    pub(crate) fn transcript_overlay_lines(&self, width: u16) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for cell in &self.history {
            Self::extend_lines_with_separator(&mut lines, cell.transcript_lines(width));
        }
        Self::extend_lines_with_separator(&mut lines, self.live_transcript_lines(width));
        Self::trim_trailing_blank_lines(&mut lines);
        lines
    }

    pub(crate) fn transcript_overlay_has_live_tail(&self) -> bool {
        self.active_cell.is_some()
            || !self.active_text_items.is_empty()
            || !self.active_tool_calls.is_empty()
            || !self.pending_tool_calls.is_empty()
    }

    pub(crate) fn active_viewport_lines_for_test(&self, width: u16) -> Vec<Line<'static>> {
        self.active_viewport_lines(width)
    }

    pub(crate) fn active_viewport_lines_for_area_for_test(
        &self,
        width: u16,
        height: u16,
    ) -> Vec<Line<'static>> {
        self.active_viewport_lines_for_area(width, height)
    }

    pub(super) fn active_viewport_lines_for_area(
        &self,
        width: u16,
        height: u16,
    ) -> Vec<Line<'static>> {
        tail_visible_lines(self.active_viewport_lines(width), height)
    }

    pub(super) fn active_viewport_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.live_viewport_lines(width, LiveViewportLineMode::Display)
    }

    fn live_viewport_lines(&self, width: u16, mode: LiveViewportLineMode) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let cell_lines = |cell: &dyn history_cell::HistoryCell| match mode {
            LiveViewportLineMode::Display => cell.display_lines(width),
            LiveViewportLineMode::Transcript => cell.transcript_lines(width),
        };
        if let Some(cell) = &self.active_cell {
            Self::extend_lines_with_separator(&mut lines, cell_lines(cell.as_ref()));
        }

        let mut items: Vec<(u64, LiveItem)> = Vec::new();
        for (idx, item) in self.active_text_items.iter().enumerate() {
            if item.cell.is_some() {
                items.push((item.seq, LiveItem::Text(idx)));
            }
        }
        for tool_call in self.active_tool_calls.values() {
            if tool_call.exec_like {
                continue;
            }
            items.push((tool_call.seq, LiveItem::Tool(tool_call.tool_use_id.clone())));
        }
        items.sort_by(|(seq_a, item_a), (seq_b, item_b)| {
            Self::compare_live_viewport_items(
                &self.active_text_items,
                *seq_a,
                item_a,
                *seq_b,
                item_b,
            )
        });

        for (_, item) in items {
            match item {
                LiveItem::Text(idx) => {
                    if let Some(cell) = &self.active_text_items[idx].cell {
                        Self::extend_lines_with_separator(&mut lines, cell_lines(cell.as_ref()));
                    }
                }
                LiveItem::Tool(tool_use_id) => {
                    if let Some(tool_call) = self.active_tool_calls.get(&tool_use_id) {
                        let tool_lines = match mode {
                            LiveViewportLineMode::Display => {
                                Self::live_tool_display_lines(width, tool_call)
                            }
                            LiveViewportLineMode::Transcript => {
                                Self::live_tool_transcript_lines(width, tool_call)
                            }
                        };
                        Self::extend_lines_with_separator(&mut lines, tool_lines);
                    }
                }
            }
        }
        for pending in &self.pending_tool_calls {
            let title_line = tool_title_line(
                ToolPhase::Preparing,
                &tool_title_parts(
                    ToolPhase::Preparing,
                    pending.tool_name.as_deref(),
                    pending.input.as_ref(),
                    &pending.parsed_commands,
                    false,
                    &pending.title,
                ),
            );
            let pending_lines = vec![title_line];
            Self::extend_lines_with_separator(
                &mut lines,
                match mode {
                    LiveViewportLineMode::Display => {
                        history_cell::AgentMessageCell::new_with_prefix(
                            pending_lines,
                            Self::tool_dot_prefix(),
                            "  ",
                            false,
                        )
                        .display_lines(width)
                    }
                    LiveViewportLineMode::Transcript => {
                        history_cell::AgentMessageCell::new_with_prefix(
                            pending_lines,
                            Self::tool_dot_prefix(),
                            "  ",
                            false,
                        )
                        .transcript_lines(width)
                    }
                },
            );
        }
        Self::trim_trailing_blank_lines(&mut lines);
        lines
    }

    fn live_tool_display_lines(
        width: u16,
        tool_call: &super::ActiveToolCall,
    ) -> Vec<Line<'static>> {
        match (&tool_call.tool_name, &tool_call.input) {
            (Some(tool_name), Some(input)) if is_agent_task_tool_name(tool_name) => {
                AgentToolCell::new(
                    tool_name.clone(),
                    ToolPhase::Running,
                    Some(input.clone()),
                    None,
                    tool_call.output.clone(),
                    Self::tool_dot_prefix(),
                )
                .display_lines(width)
            }
            (Some(tool_name), Some(input)) => {
                let title_line = tool_title_line(
                    ToolPhase::Running,
                    &tool_title_parts(
                        ToolPhase::Running,
                        Some(tool_name.as_str()),
                        Some(input),
                        &tool_call.parsed_commands,
                        false,
                        &tool_call.title,
                    ),
                );
                ToolIoCell::from_text_output(
                    ToolIoCellOptions {
                        title_line: Some(title_line),
                        dot_prefix: Self::tool_dot_prefix(),
                        subsequent_prefix: "  ".into(),
                        output_style: Self::tool_text_style(),
                        show_empty_ellipsis: false,
                    },
                    tool_name.clone(),
                    input.clone(),
                    tool_call.output.clone(),
                )
                .display_lines(width)
            }
            _ => {
                let title_line = tool_title_line(
                    ToolPhase::Running,
                    &tool_title_parts(
                        ToolPhase::Running,
                        tool_call.tool_name.as_deref(),
                        tool_call.input.as_ref(),
                        &tool_call.parsed_commands,
                        false,
                        &tool_call.title,
                    ),
                );
                let mut lines = vec![title_line];
                lines.extend(tool_call.lines.clone());
                history_cell::AgentMessageCell::new_with_prefix(
                    lines,
                    Self::tool_dot_prefix(),
                    "  ",
                    false,
                )
                .display_lines(width)
            }
        }
    }

    fn live_tool_transcript_lines(
        width: u16,
        tool_call: &super::ActiveToolCall,
    ) -> Vec<Line<'static>> {
        match (&tool_call.tool_name, &tool_call.input) {
            (Some(tool_name), Some(input)) if is_agent_task_tool_name(tool_name) => {
                AgentToolCell::new(
                    tool_name.clone(),
                    ToolPhase::Running,
                    Some(input.clone()),
                    None,
                    tool_call.output.clone(),
                    Self::tool_dot_prefix(),
                )
                .transcript_lines(width)
            }
            (Some(tool_name), Some(input)) => {
                let title_line = tool_title_line(
                    ToolPhase::Running,
                    &tool_title_parts(
                        ToolPhase::Running,
                        Some(tool_name.as_str()),
                        Some(input),
                        &tool_call.parsed_commands,
                        false,
                        &tool_call.title,
                    ),
                );
                ToolIoCell::from_text_output(
                    ToolIoCellOptions {
                        title_line: Some(title_line),
                        dot_prefix: Self::tool_dot_prefix(),
                        subsequent_prefix: "  ".into(),
                        output_style: Self::tool_text_style(),
                        show_empty_ellipsis: false,
                    },
                    tool_name.clone(),
                    input.clone(),
                    tool_call.output.clone(),
                )
                .transcript_lines(width)
            }
            _ => {
                let title_line = tool_title_line(
                    ToolPhase::Running,
                    &tool_title_parts(
                        ToolPhase::Running,
                        tool_call.tool_name.as_deref(),
                        tool_call.input.as_ref(),
                        &tool_call.parsed_commands,
                        false,
                        &tool_call.title,
                    ),
                );
                let mut lines = vec![title_line];
                lines.extend(tool_call.lines.clone());
                history_cell::AgentMessageCell::new_with_prefix(
                    lines,
                    Self::tool_dot_prefix(),
                    "  ",
                    false,
                )
                .transcript_lines(width)
            }
        }
    }

    fn live_transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.live_viewport_lines(width, LiveViewportLineMode::Transcript)
    }

    fn text_item_precedes_assistant(kind: TextItemKind) -> bool {
        matches!(kind, TextItemKind::Reasoning)
    }

    fn compare_live_viewport_items(
        active_text_items: &[super::text_stream::ActiveTextItem],
        seq_a: u64,
        item_a: &LiveItem,
        seq_b: u64,
        item_b: &LiveItem,
    ) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        let text_kind = |item: &LiveItem| match item {
            LiveItem::Text(idx) => active_text_items.get(*idx).map(|item| item.kind),
            LiveItem::Tool(_) => None,
        };
        if let (Some(kind_a), Some(kind_b)) = (text_kind(item_a), text_kind(item_b)) {
            if Self::text_item_precedes_assistant(kind_a) && kind_b == TextItemKind::Assistant {
                return Ordering::Less;
            }
            if kind_a == TextItemKind::Assistant && Self::text_item_precedes_assistant(kind_b) {
                return Ordering::Greater;
            }
        }
        seq_a.cmp(&seq_b)
    }

    fn extend_lines_with_separator(target: &mut Vec<Line<'static>>, mut next: Vec<Line<'static>>) {
        if next.is_empty() {
            return;
        }

        let should_insert_separator = !target.is_empty()
            && target
                .last()
                .is_some_and(|line| !Self::is_blank_line(line) && !is_horizontal_rule_line(line))
            && next
                .first()
                .is_some_and(|line| !Self::is_blank_line(line) && !is_horizontal_rule_line(line));
        if should_insert_separator {
            target.push(Line::from(""));
        }
        target.append(&mut next);
    }

    pub(super) fn active_viewport_scroll_offset(line_count: usize, height: u16) -> usize {
        line_count.saturating_sub(height as usize)
    }

    pub(crate) fn drain_scrollback_lines(&mut self, width: u16) -> Vec<ScrollbackLine> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for (index, cell) in self
            .history
            .iter()
            .skip(self.next_history_flush_index)
            .enumerate()
        {
            let cell_lines = cell.display_lines(width);
            let should_insert_separator = index > 0
                && !cell_lines.is_empty()
                && !lines.is_empty()
                && lines.last().is_some_and(|line: &ScrollbackLine| {
                    !Self::is_blank_line(&line.line) && !is_horizontal_rule_line(&line.line)
                })
                && cell_lines.first().is_some_and(|line| {
                    !Self::is_blank_line(line) && !is_horizontal_rule_line(line)
                });
            if should_insert_separator {
                lines.push(ScrollbackLine::new(Line::from("")));
            }
            lines.extend(cell_lines.into_iter().map(ScrollbackLine::new));
        }
        self.next_history_flush_index = self.history.len();
        // Rule-lined cells (e.g. user prompts) already provide visual separation; avoid an
        // extra blank row after them so adjacent content sits flush against the rule.
        if !lines.is_empty()
            && lines
                .last()
                .is_some_and(|line| !is_horizontal_rule_line(&line.line))
        {
            lines.push(ScrollbackLine::new(Line::from("")));
        }
        lines
    }
}

fn tail_visible_lines(mut lines: Vec<Line<'static>>, height: u16) -> Vec<Line<'static>> {
    let height = height as usize;
    if height == 0 || lines.len() <= height {
        return lines;
    }
    lines.split_off(lines.len() - height)
}
