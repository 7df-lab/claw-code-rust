//! Horizontal chip strip: left/right selection with effort-style chips.
//!
//! Selected chips render as `[label]` with accent + bold + underline; others
//! are dim. When options do not fit, a sliding window shows `‹` / `›` markers.

use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

/// Horizontal selectable chip strip (←/→), matching `/model` effort visuals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HorizontalChipStrip {
    options: Vec<String>,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChipWindow {
    indices: Vec<usize>,
    show_left_more: bool,
    show_right_more: bool,
}

impl HorizontalChipStrip {
    /// Creates a strip. Empty `options` is ignored and yields an empty strip.
    pub(crate) fn new(options: Vec<String>, selected: usize) -> Self {
        let selected = if options.is_empty() {
            0
        } else {
            selected.min(options.len() - 1)
        };
        Self { options, selected }
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.selected
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    /// Move selection left, wrapping at the start.
    pub(crate) fn move_left(&mut self) {
        self.move_by(-1);
    }

    /// Move selection right, wrapping at the end.
    pub(crate) fn move_right(&mut self) {
        self.move_by(1);
    }

    fn move_by(&mut self, delta: isize) {
        if self.options.is_empty() {
            return;
        }
        let len = self.options.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    /// Render the chip line for `available_width` content columns (no left pad).
    pub(crate) fn render_line(&self, available_width: usize, accent: Color) -> Line<'static> {
        if self.options.is_empty() {
            return Line::from("");
        }

        let window = chip_window(&self.options, self.selected, available_width);
        let mut spans = Vec::new();
        if window.show_left_more {
            spans.push(Span::styled("‹ ".to_string(), Style::default().dim()));
        }
        for (idx_in_window, option_idx) in window.indices.iter().enumerate() {
            if idx_in_window > 0 {
                spans.push(Span::raw("  "));
            }
            let label = &self.options[*option_idx];
            let is_selected = *option_idx == self.selected;
            let text = if is_selected {
                format!("[{label}]")
            } else {
                label.clone()
            };
            let style = if is_selected {
                Style::default().fg(accent).bold().underlined()
            } else {
                Style::default().dim()
            };
            spans.push(Span::styled(text, style));
        }
        if window.show_right_more {
            spans.push(Span::styled(" ›".to_string(), Style::default().dim()));
        }
        Line::from(spans)
    }
}

fn chip_width(label: &str, selected: bool) -> usize {
    if selected {
        UnicodeWidthStr::width(label) + 2
    } else {
        UnicodeWidthStr::width(label)
    }
}

fn chip_window(options: &[String], selected_idx: usize, available_width: usize) -> ChipWindow {
    if options.is_empty() {
        return ChipWindow {
            indices: Vec::new(),
            show_left_more: false,
            show_right_more: false,
        };
    }

    let selected_idx = selected_idx.min(options.len() - 1);

    let all_width = options
        .iter()
        .enumerate()
        .map(|(idx, label)| {
            let chip = chip_width(label, idx == selected_idx);
            if idx == 0 { chip } else { chip + 2 }
        })
        .sum::<usize>();
    if all_width <= available_width {
        return ChipWindow {
            indices: (0..options.len()).collect(),
            show_left_more: false,
            show_right_more: false,
        };
    }

    // Sliding window centered on the selected option.
    let marker_budget = 4; // "‹ " + " ›"
    let content_budget = available_width.saturating_sub(marker_budget);
    let mut start = selected_idx;
    let mut end = selected_idx;
    let mut used = chip_width(&options[selected_idx], true);

    loop {
        let can_grow_left = start > 0;
        let can_grow_right = end + 1 < options.len();
        if !can_grow_left && !can_grow_right {
            break;
        }

        let mut grew = false;
        if can_grow_left {
            let next = start - 1;
            let extra = chip_width(&options[next], false) + 2;
            if used + extra <= content_budget {
                start = next;
                used += extra;
                grew = true;
            }
        }
        if can_grow_right {
            let next = end + 1;
            let extra = chip_width(&options[next], false) + 2;
            if used + extra <= content_budget {
                end = next;
                used += extra;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    ChipWindow {
        indices: (start..=end).collect(),
        show_left_more: start > 0,
        show_right_more: end + 1 < options.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;

    #[test]
    fn move_left_right_wraps() {
        let mut strip = HorizontalChipStrip::new(
            vec!["Cancel".to_string(), "Delete".to_string()],
            /*selected*/ 0,
        );
        assert_eq!(strip.selected_index(), 0);
        strip.move_right();
        assert_eq!(strip.selected_index(), 1);
        strip.move_right();
        assert_eq!(strip.selected_index(), 0);
        strip.move_left();
        assert_eq!(strip.selected_index(), 1);
    }

    #[test]
    fn render_selected_uses_brackets() {
        let strip = HorizontalChipStrip::new(
            vec!["Cancel".to_string(), "Delete".to_string()],
            /*selected*/ 0,
        );
        let line = strip.render_line(80, Color::Cyan);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[Cancel]  Delete");
    }

    #[test]
    fn window_slides_when_narrow() {
        let options = vec![
            "Minimal".to_string(),
            "Low".to_string(),
            "Medium".to_string(),
            "High".to_string(),
            "XHigh".to_string(),
        ];
        let window = chip_window(
            &options, /*selected_idx*/ 2, /*available_width*/ 20,
        );
        assert!(
            window.show_left_more || window.show_right_more || window.indices.len() < options.len()
        );
        assert!(window.indices.contains(&2));
    }
}
