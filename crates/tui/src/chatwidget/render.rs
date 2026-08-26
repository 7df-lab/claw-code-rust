//! Layout rendering glue for `ChatWidget`.
//!
//! This module owns only the ratatui `Renderable` implementation so the root
//! chat widget file can focus on state construction and module wiring.

use std::sync::OnceLock;
use std::time::Instant;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::events::TextItemKind;
use crate::render::renderable::Renderable;

use super::ChatWidget;
use super::text_stream::assistant_token_log_preview;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveAssistantRenderSnapshot {
    pub(crate) item_id: String,
    pub(crate) active_cell_revision: u64,
    pub(crate) viewport_width: u16,
    pub(crate) viewport_height: u16,
    pub(crate) viewport_scroll_offset: usize,
    pub(crate) card_line_count: usize,
    pub(crate) visible_card_line_count: usize,
    pub(crate) viewport_row_start: usize,
    pub(crate) viewport_row_end_exclusive: usize,
    pub(crate) terminal_row_start: u16,
    pub(crate) terminal_row_end_exclusive: u16,
    pub(crate) visible_text_hash: u64,
    pub(crate) visible_text_preview: Option<String>,
}

impl Renderable for ChatWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(onboarding) = &self.onboarding {
            onboarding.render(area, buf);
            return;
        }

        let (history_area, subagent_area, bottom_area) = self.chat_layout_areas(area);

        {
            if tracing::enabled!(tracing::Level::DEBUG)
                && let Some(snapshot) = self.active_assistant_render_snapshot(area)
            {
                if let Some(visible_text_preview) = snapshot.visible_text_preview.as_deref() {
                    tracing::debug!(
                        stream_elapsed_ms = render_trace_elapsed_ms(),
                        item_id = %snapshot.item_id,
                        active_cell_revision = snapshot.active_cell_revision,
                        viewport_width = snapshot.viewport_width,
                        viewport_height = snapshot.viewport_height,
                        viewport_scroll_offset = snapshot.viewport_scroll_offset,
                        card_line_count = snapshot.card_line_count,
                        visible_card_line_count = snapshot.visible_card_line_count,
                        viewport_row_start = snapshot.viewport_row_start,
                        viewport_row_end_exclusive = snapshot.viewport_row_end_exclusive,
                        terminal_row_start = snapshot.terminal_row_start,
                        terminal_row_end_exclusive = snapshot.terminal_row_end_exclusive,
                        visible_text_hash = snapshot.visible_text_hash,
                        visible_text_preview,
                        "active assistant render"
                    );
                } else {
                    tracing::debug!(
                        stream_elapsed_ms = render_trace_elapsed_ms(),
                        item_id = %snapshot.item_id,
                        active_cell_revision = snapshot.active_cell_revision,
                        viewport_width = snapshot.viewport_width,
                        viewport_height = snapshot.viewport_height,
                        viewport_scroll_offset = snapshot.viewport_scroll_offset,
                        card_line_count = snapshot.card_line_count,
                        visible_card_line_count = snapshot.visible_card_line_count,
                        viewport_row_start = snapshot.viewport_row_start,
                        viewport_row_end_exclusive = snapshot.viewport_row_end_exclusive,
                        terminal_row_start = snapshot.terminal_row_start,
                        terminal_row_end_exclusive = snapshot.terminal_row_end_exclusive,
                        visible_text_hash = snapshot.visible_text_hash,
                        "active assistant render"
                    );
                }
            }
            let viewport_lines =
                self.active_viewport_lines_for_area(history_area.width, history_area.height);
            if !viewport_lines.is_empty() {
                Paragraph::new(Text::from(viewport_lines)).render(history_area, buf);
            }
        }

        self.render_subagent_live_list(subagent_area, buf);
        self.bottom_pane.render(bottom_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if let Some(onboarding) = &self.onboarding {
            return onboarding.desired_height(width.max(1));
        }
        let history_height =
            u16::try_from(self.active_viewport_lines(width.max(1)).len()).unwrap_or(u16::MAX);
        history_height
            .saturating_add(self.subagent_live_list_desired_height())
            .saturating_add(self.bottom_pane.desired_height(width))
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if self.is_subagent_live_list_focused() {
            return None;
        }
        if let Some(onboarding) = &self.onboarding {
            return onboarding.cursor_pos(area);
        }
        let (_history_area, _subagent_area, bottom_area) = self.chat_layout_areas(area);
        self.bottom_pane.cursor_pos(bottom_area)
    }
}

impl ChatWidget {
    fn chat_layout_areas(&self, area: Rect) -> (Rect, Rect, Rect) {
        let bottom_height = self.bottom_pane.desired_height(area.width).min(
            if self.bottom_pane.is_resume_picker_open() {
                area.height.saturating_sub(3).max(3)
            } else {
                area.height
            },
        );
        let subagent_height = self
            .subagent_live_list_desired_height()
            .min(area.height.saturating_sub(bottom_height));
        let history_height = u16::try_from(self.active_viewport_lines(area.width.max(1)).len())
            .unwrap_or(u16::MAX)
            .min(
                area.height
                    .saturating_sub(bottom_height)
                    .saturating_sub(subagent_height),
            );
        let [history_area, subagent_area, bottom_area] = Layout::vertical([
            Constraint::Length(history_height),
            Constraint::Length(subagent_height),
            Constraint::Length(bottom_height),
        ])
        .areas(area);
        (history_area, subagent_area, bottom_area)
    }

    pub(crate) fn active_assistant_render_snapshot(
        &self,
        area: Rect,
    ) -> Option<ActiveAssistantRenderSnapshot> {
        let (history_area, _subagent_area, _bottom_area) = self.chat_layout_areas(area);
        let width = history_area.width.max(1);
        let full_viewport_line_count = self.active_viewport_lines(width).len();
        let viewport_scroll_offset =
            Self::active_viewport_scroll_offset(full_viewport_line_count, history_area.height);
        let visible_window_start = viewport_scroll_offset;
        let visible_window_end =
            viewport_scroll_offset.saturating_add(history_area.height as usize);
        let mut prefix_lines = Vec::new();
        if let Some(cell) = &self.active_cell {
            extend_render_lines_with_separator(&mut prefix_lines, cell.display_lines(width));
        }
        for item in &self.active_text_items {
            let Some(cell) = &item.cell else {
                continue;
            };
            let item_lines = cell.display_lines(width);
            if item.kind == TextItemKind::Assistant {
                let separator_lines = if should_insert_render_separator(&prefix_lines, &item_lines)
                {
                    1
                } else {
                    0
                };
                let viewport_row_start = prefix_lines.len().saturating_add(separator_lines);
                let viewport_row_end_exclusive =
                    viewport_row_start.saturating_add(item_lines.len());
                let visible_start = viewport_row_start.max(visible_window_start);
                let visible_end = viewport_row_end_exclusive.min(visible_window_end);
                let visible_lines = if visible_start < visible_end {
                    &item_lines
                        [visible_start - viewport_row_start..visible_end - viewport_row_start]
                } else {
                    &[]
                };
                return Some(ActiveAssistantRenderSnapshot {
                    item_id: item.item_id.log_label(),
                    active_cell_revision: self.active_cell_revision,
                    viewport_width: width,
                    viewport_height: history_area.height,
                    viewport_scroll_offset,
                    card_line_count: item_lines.len(),
                    visible_card_line_count: visible_lines.len(),
                    viewport_row_start,
                    viewport_row_end_exclusive,
                    terminal_row_start:
                        history_area.y.saturating_add(
                            visible_start.saturating_sub(viewport_scroll_offset) as u16,
                        ),
                    terminal_row_end_exclusive: history_area
                        .y
                        .saturating_add(visible_end.saturating_sub(viewport_scroll_offset) as u16),
                    visible_text_hash: visible_text_hash(visible_lines),
                    visible_text_preview: assistant_visible_text_preview(visible_lines),
                });
            }
            extend_render_lines_with_separator(&mut prefix_lines, item_lines);
        }
        None
    }

    pub(crate) fn note_active_assistant_terminal_flush(
        &mut self,
        snapshot: Option<&ActiveAssistantRenderSnapshot>,
        flush_stats: crate::custom_terminal::FlushStats,
    ) {
        let Some(snapshot) = snapshot else {
            self.last_terminal_assistant_visible_hash = None;
            return;
        };
        let visible_key = (snapshot.item_id.clone(), snapshot.visible_text_hash);
        let visible_changed =
            self.last_terminal_assistant_visible_hash.as_ref() != Some(&visible_key);
        if visible_changed && snapshot.visible_card_line_count > 0 && flush_stats.put_commands == 0
        {
            tracing::warn!(
                stream_elapsed_ms = render_trace_elapsed_ms(),
                item_id = %snapshot.item_id,
                active_cell_revision = snapshot.active_cell_revision,
                visible_text_hash = snapshot.visible_text_hash,
                frame_seq = flush_stats.frame_seq,
                diff_commands = flush_stats.diff_commands,
                put_commands = flush_stats.put_commands,
                clear_to_end_commands = flush_stats.clear_to_end_commands,
                changed_row_start = ?flush_stats.changed_row_start,
                changed_row_end_exclusive = ?flush_stats.changed_row_end_exclusive,
                terminal_row_start = snapshot.terminal_row_start,
                terminal_row_end_exclusive = snapshot.terminal_row_end_exclusive,
                "active assistant visible text changed but terminal flush emitted no visible cell updates"
            );
        }
        self.last_terminal_assistant_visible_hash = Some(visible_key);
    }
}

fn extend_render_lines_with_separator(
    target: &mut Vec<Line<'static>>,
    mut next: Vec<Line<'static>>,
) {
    if next.is_empty() {
        return;
    }
    if should_insert_render_separator(target, &next) {
        target.push(Line::from(""));
    }
    target.append(&mut next);
}

fn should_insert_render_separator(target: &[Line<'static>], next: &[Line<'static>]) -> bool {
    !target.is_empty()
        && target
            .last()
            .is_some_and(|line| !ChatWidget::is_blank_line(line))
        && next
            .first()
            .is_some_and(|line| !ChatWidget::is_blank_line(line))
}

fn assistant_visible_text_preview(lines: &[Line<'static>]) -> Option<String> {
    if lines.is_empty() {
        return assistant_token_log_preview("");
    }
    let text = lines
        .iter()
        .map(line_plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    assistant_token_log_preview(&text)
}

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn visible_text_hash(lines: &[Line<'static>]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for line in lines {
        line_plain_text(line).hash(&mut hasher);
        '\n'.hash(&mut hasher);
    }
    hasher.finish()
}

fn render_trace_elapsed_ms() -> u128 {
    static RENDER_TRACE_START: OnceLock<Instant> = OnceLock::new();
    RENDER_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}
