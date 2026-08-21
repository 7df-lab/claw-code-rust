//! Rendering and display formatting for the inline resume picker.

use chrono::DateTime;
use chrono::Utc;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::render::renderable::Renderable;

use super::DESIRED_HEIGHT;
use super::EditMode;
use super::LoadState;
use super::ResumePickerView;

impl Renderable for ResumePickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let filtered = self.filtered_indices();
        let filtered_count = filtered.len();
        let position = self
            .selected_session_id
            .and_then(|selected| {
                filtered
                    .iter()
                    .position(|index| self.sessions[*index].session_id == selected)
            })
            .map_or(0, |index| index + 1);
        Paragraph::new(Line::from(format!(
            "Resume session ({position} of {filtered_count})"
        )))
        .render(Rect::new(area.x, area.y, area.width, 1), buf);

        let input_area = Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            3.min(area.height),
        );
        let (input_label, input_text, input_error) = self.input_text();
        let input_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(format!(" {input_label} "))
            .border_style(Style::default().fg(self.accent_color));
        Paragraph::new(truncate_display(
            input_text,
            usize::from(area.width.saturating_sub(4)),
        ))
        .block(input_block)
        .render(input_area, buf);

        let body_y = area.y.saturating_add(4);
        let body_height = area.bottom().saturating_sub(body_y).saturating_sub(1);
        let body_area = Rect::new(area.x, body_y, area.width, body_height);
        match &self.load_state {
            LoadState::Loading => {
                Paragraph::new("Loading saved sessions…".dim()).render(body_area, buf)
            }
            LoadState::Failed(message) => {
                Paragraph::new(Line::from(format!("Failed to load sessions: {message}")).red())
                    .render(body_area, buf)
            }
            LoadState::Ready => {
                let (lines, selected_range) = self.list_lines(usize::from(body_area.width));
                let mut offset = self.scroll_offset.get().min(lines.len().saturating_sub(1));
                if let Some((start, end)) = selected_range {
                    let capacity = usize::from(body_area.height).max(1);
                    if end.saturating_sub(start) >= capacity || start < offset {
                        offset = start;
                    } else if end > offset.saturating_add(capacity) {
                        offset = end.saturating_sub(capacity);
                    }
                }
                self.scroll_offset.set(offset);
                Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<_>>())
                    .render(body_area, buf);
            }
        }
        let footer_y = area.bottom().saturating_sub(1);
        let footer = input_error
            .map(|error| Line::from(truncate_display(error, usize::from(area.width))).red())
            .unwrap_or_else(|| self.footer_line());
        Paragraph::new(footer).render(Rect::new(area.x, footer_y, area.width, 1), buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        DESIRED_HEIGHT
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !matches!(self.load_state, LoadState::Ready)
            || matches!(self.edit_mode, EditMode::Delete { .. })
        {
            return None;
        }
        let (_, text, _) = self.input_text();
        let text_width = u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX);
        Some((
            area.x
                .saturating_add(2)
                .saturating_add(text_width)
                .min(area.right().saturating_sub(2)),
            area.y.saturating_add(2),
        ))
    }
}

pub(super) fn relative_time(timestamp: DateTime<Utc>) -> String {
    let seconds = Utc::now()
        .signed_duration_since(timestamp)
        .num_seconds()
        .max(0);
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{} minutes ago", seconds / 60),
        3_600..=86_399 => format!("{} hours ago", seconds / 3_600),
        86_400..=172_799 => "1 day ago".to_string(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

pub(super) fn format_bytes(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "unknown size".to_string();
    };
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

pub(super) fn truncate_display(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let mut output = String::new();
    let target = max_width - 3;
    let mut width: usize = 0;
    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(0);
        if width.saturating_add(char_width) > target {
            break;
        }
        output.push(ch);
        width += char_width;
    }
    output.push_str("...");
    output
}
