//! Composer-adjacent pending input queue (canonical `session/queue/*`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::queue_ops::queue_render_preview;
use crate::render::renderable::Renderable;

/// One queued prompt shown under the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingQueueItem {
    pub queue_item_id: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub(crate) struct PendingQueueState {
    items: Vec<PendingQueueItem>,
    /// Selected index when queue focus is active.
    selected: Option<usize>,
    focused: bool,
}

impl PendingQueueState {
    pub(crate) fn items(&self) -> &[PendingQueueItem] {
        &self.items
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn focused(&self) -> bool {
        self.focused
    }

    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected_item(&self) -> Option<&PendingQueueItem> {
        self.selected.and_then(|idx| self.items.get(idx))
    }

    pub(crate) fn replace_items(&mut self, items: Vec<PendingQueueItem>) {
        let prev_selected_id = self
            .selected
            .and_then(|idx| self.items.get(idx))
            .map(|item| item.queue_item_id.clone());
        self.items = items;
        if self.items.is_empty() {
            self.selected = None;
            self.focused = false;
            return;
        }
        if let Some(id) = prev_selected_id
            && let Some(idx) = self.items.iter().position(|item| item.queue_item_id == id)
        {
            self.selected = Some(idx);
        } else if self.focused {
            self.selected = Some(self.selected.unwrap_or(0).min(self.items.len() - 1));
        } else {
            self.selected = None;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.selected = None;
        self.focused = false;
    }

    pub(crate) fn focus_first(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        self.focused = true;
        self.selected = Some(0);
        true
    }

    pub(crate) fn clear_focus(&mut self) {
        self.focused = false;
        self.selected = None;
    }

    /// Move selection down. Returns true if handled.
    pub(crate) fn select_next(&mut self) -> bool {
        if !self.focused || self.items.is_empty() {
            return false;
        }
        let idx = self.selected.unwrap_or(0);
        if idx + 1 < self.items.len() {
            self.selected = Some(idx + 1);
        }
        true
    }

    /// Move selection up. Returns `LeftQueue` when leaving to the composer.
    pub(crate) fn select_prev(&mut self) -> QueueNavResult {
        if !self.focused || self.items.is_empty() {
            return QueueNavResult::Ignored;
        }
        match self.selected {
            Some(0) | None => {
                self.clear_focus();
                QueueNavResult::ReturnToInput
            }
            Some(idx) => {
                self.selected = Some(idx - 1);
                QueueNavResult::Handled
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueNavResult {
    Ignored,
    Handled,
    ReturnToInput,
}

pub(crate) struct PendingQueueList<'a> {
    state: &'a PendingQueueState,
}

impl<'a> PendingQueueList<'a> {
    pub(crate) fn new(state: &'a PendingQueueState) -> Self {
        Self { state }
    }
}

impl Renderable for PendingQueueList<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || self.state.items.is_empty() {
            return;
        }
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(""));
        lines.push(Line::from(
            format!("  queued ({})", self.state.items.len()).dim(),
        ));
        for (idx, item) in self.state.items.iter().enumerate() {
            let selected = self.state.focused && self.state.selected == Some(idx);
            let preview = queue_render_preview(&item.text);
            let label = if preview.is_empty() {
                "(empty)".to_string()
            } else {
                preview
            };
            let mut row = Line::from(vec![
                Span::styled(format!("  {} › ", idx + 1), Style::default().cyan()),
                Span::raw(label),
            ]);
            if selected {
                row = row.style(
                    Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(ratatui::style::Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            }
            lines.push(truncate_line_with_ellipsis_if_overflow(
                row,
                area.width as usize,
            ));
        }
        if self.state.focused {
            lines.push(Line::from(
                "  ↑/↓ navigate · ctrl+s steer · ctrl+e edit · ctrl+d delete · esc back".dim(),
            ));
        }
        Paragraph::new(lines).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if self.state.items.is_empty() {
            return 0;
        }
        let _ = width;
        // blank + header + one row per item + optional focus hint
        let hint = if self.state.focused { 1 } else { 0 };
        (2 + self.state.items.len() + hint) as u16
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::queue_ops::queue_render_preview;
    use crate::render::renderable::Renderable;

    #[test]
    fn render_preview_collapses_newlines() {
        assert_eq!(queue_render_preview("one\ntwo\n\nthree"), "one two three");
    }

    #[test]
    fn up_on_first_returns_to_input() {
        let mut state = PendingQueueState::default();
        state.replace_items(vec![
            PendingQueueItem {
                queue_item_id: "a".into(),
                text: "a".into(),
            },
            PendingQueueItem {
                queue_item_id: "b".into(),
                text: "b".into(),
            },
        ]);
        assert!(state.focus_first());
        assert_eq!(state.select_prev(), QueueNavResult::ReturnToInput);
        assert!(!state.focused());
    }

    #[test]
    fn pending_queue_renders_one_visual_row_per_item() {
        let mut state = PendingQueueState::default();
        state.replace_items(vec![PendingQueueItem {
            queue_item_id: "q1".into(),
            text: "hello\nworld\nand more".into(),
        }]);
        let list = PendingQueueList::new(&state);
        assert_eq!(list.desired_height(/*width*/ 40), 3);

        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf);
        let header: String = (0..40)
            .map(|x| buf[(x, 1)].symbol().to_string())
            .collect::<String>();
        assert!(header.contains("queued (1)"), "header={header}");
        let row: String = (0..40)
            .map(|x| buf[(x, 2)].symbol().to_string())
            .collect::<String>();
        assert!(row.contains("1 ›"), "row={row}");
        assert!(row.contains("hello world and more"), "row={row}");
    }

    #[test]
    fn pending_queue_header_counts_items_and_shows_focus_hint() {
        let mut state = PendingQueueState::default();
        state.replace_items(vec![
            PendingQueueItem {
                queue_item_id: "q1".into(),
                text: "first".into(),
            },
            PendingQueueItem {
                queue_item_id: "q2".into(),
                text: "second".into(),
            },
        ]);
        assert!(state.focus_first());
        let list = PendingQueueList::new(&state);
        assert_eq!(list.desired_height(/*width*/ 60), 5);

        let area = Rect::new(0, 0, 60, 5);
        let mut buf = Buffer::empty(area);
        list.render(area, &mut buf);
        let rendered: Vec<String> = (0..5)
            .map(|y| {
                (0..60)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        assert!(rendered[1].contains("queued (2)"), "header={}", rendered[1]);
        assert!(rendered[2].contains("1 › first"), "row={}", rendered[2]);
        assert!(rendered[3].contains("2 › second"), "row={}", rendered[3]);
        assert!(
            rendered[4].contains("ctrl+d delete"),
            "hint={}",
            rendered[4]
        );
    }
}
