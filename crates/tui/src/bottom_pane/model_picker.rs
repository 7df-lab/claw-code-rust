//! Combined model + reasoning-effort picker for `/model`.
//!
//! Shows a vertically scrollable model list with a horizontal reasoning-effort
//! strip below it. Up/Down (and j/k) move the model selection; Left/Right cycle
//! effort for the focused model; Enter applies both in one step.
//!
//! Replaces the composer input area, so it paints the shared menu-surface
//! background — see [`BottomPaneView::replaces_composer`].

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::key_hint;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

/// Marker column + following space before the model name.
const MARKER_COLS: usize = 2;

/// One reasoning-effort chip shown in the horizontal strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPickerEffortOption {
    pub(crate) label: String,
    pub(crate) value: String,
}

/// One row in the `/model` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPickerEntry {
    pub(crate) selection_value: String,
    pub(crate) display_name: String,
    pub(crate) right_hint: Option<String>,
    pub(crate) is_current: bool,
    pub(crate) effort_options: Vec<ModelPickerEffortOption>,
    /// Initial effort for this model when the picker opened (session-resolved).
    pub(crate) selected_effort: Option<String>,
}

/// Result returned when the user confirms a model picker selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPickerSelection {
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<String>,
}

pub(crate) struct ModelPickerView {
    entries: Vec<ModelPickerEntry>,
    state: ScrollState,
    /// Currently selected effort value for the focused model.
    effort_selection: Option<String>,
    complete: bool,
    selected: Option<ModelPickerSelection>,
    accent_color: Color,
}

impl ModelPickerView {
    pub(crate) fn new(entries: Vec<ModelPickerEntry>, accent_color: Color) -> Self {
        let mut state = ScrollState::new();
        let initial_idx = entries
            .iter()
            .position(|entry| entry.is_current)
            .unwrap_or(0);
        if entries.is_empty() {
            state.selected_idx = None;
        } else {
            state.selected_idx = Some(initial_idx.min(entries.len() - 1));
        }
        let visible = MAX_POPUP_ROWS.min(entries.len());
        state.ensure_visible(entries.len(), visible);

        let mut view = Self {
            entries,
            state,
            effort_selection: None,
            complete: false,
            selected: None,
            accent_color,
        };
        view.sync_effort_for_current_model();
        view
    }

    fn visible_rows(&self) -> usize {
        MAX_POPUP_ROWS.min(self.entries.len())
    }

    fn has_more_above(&self) -> bool {
        self.state.scroll_top > 0
    }

    fn has_more_below(&self) -> bool {
        let len = self.entries.len();
        let visible = self.visible_rows();
        visible > 0 && self.state.scroll_top + visible < len
    }

    fn has_effort_row(&self) -> bool {
        self.current_entry()
            .is_some_and(|entry| !entry.effort_options.is_empty())
    }

    /// Height of the picker without building styled lines (avoids duplicate work
    /// across `desired_height` and `render` in the same frame).
    fn computed_height(&self) -> u16 {
        let mut height = self.visible_rows();
        if self.has_more_above() {
            height = height.saturating_add(1);
        }
        if self.has_more_below() {
            height = height.saturating_add(1);
        }
        if self.has_effort_row() {
            // blank separator + effort chips
            height = height.saturating_add(2);
        }
        // blank separator + footer
        height = height.saturating_add(2);
        u16::try_from(height).unwrap_or(u16::MAX)
    }

    fn current_entry(&self) -> Option<&ModelPickerEntry> {
        self.state
            .selected_idx
            .and_then(|idx| self.entries.get(idx))
    }

    fn sync_effort_for_current_model(&mut self) {
        let Some(entry) = self.current_entry() else {
            self.effort_selection = None;
            return;
        };
        if entry.effort_options.is_empty() {
            self.effort_selection = None;
            return;
        }
        if let Some(current) = self.effort_selection.as_deref()
            && entry
                .effort_options
                .iter()
                .any(|option| option.value == current)
        {
            return;
        }
        self.effort_selection = entry.selected_effort.clone().or_else(|| {
            entry
                .effort_options
                .first()
                .map(|option| option.value.clone())
        });
    }

    fn move_model_selection(&mut self, delta: isize) {
        let len = self.entries.len();
        if delta < 0 {
            self.state.move_up_wrap(len);
        } else if delta > 0 {
            self.state.move_down_wrap(len);
        }
        self.state.ensure_visible(len, self.visible_rows());
        self.sync_effort_for_current_model();
    }

    fn move_effort(&mut self, delta: isize) {
        let Some(entry) = self.current_entry() else {
            return;
        };
        let options = &entry.effort_options;
        if options.is_empty() {
            return;
        }
        let current_idx = self
            .effort_selection
            .as_ref()
            .and_then(|value| options.iter().position(|option| &option.value == value))
            .unwrap_or(0);
        let new_idx = (current_idx as isize + delta).rem_euclid(options.len() as isize) as usize;
        self.effort_selection = Some(options[new_idx].value.clone());
    }

    fn accept(&mut self) {
        let Some(entry) = self.current_entry() else {
            self.complete = true;
            return;
        };
        let reasoning_effort = if entry.effort_options.is_empty() {
            None
        } else {
            self.effort_selection.clone()
        };
        self.selected = Some(ModelPickerSelection {
            model: entry.selection_value.clone(),
            reasoning_effort,
        });
        self.complete = true;
    }

    /// Width of the name cell (display name + optional ` ‹`), shared so provider
    /// hints start in one column like a two-column table.
    fn name_column_width(&self, total_width: u16) -> usize {
        let prefix_width = MARKER_COLS; // marker + space
        let gap_width = 2;
        // Reserve a modest amount for the provider column when present.
        let has_any_provider = self.entries.iter().any(|entry| {
            entry
                .right_hint
                .as_deref()
                .map(str::trim)
                .is_some_and(|hint| !hint.is_empty())
        });
        let provider_reserve = if has_any_provider {
            self.entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .right_hint
                        .as_deref()
                        .map(str::trim)
                        .filter(|hint| !hint.is_empty())
                        .map(UnicodeWidthStr::width)
                })
                .max()
                .unwrap_or(0)
                .clamp(8, 16)
        } else {
            0
        };
        let available = usize::from(total_width)
            .saturating_sub(prefix_width)
            .saturating_sub(if has_any_provider {
                gap_width + provider_reserve
            } else {
                0
            });
        let natural = self
            .entries
            .iter()
            .map(|entry| {
                let mut width = UnicodeWidthStr::width(entry.display_name.as_str());
                if entry.is_current {
                    width = width.saturating_add(2); // " ‹"
                }
                width
            })
            .max()
            .unwrap_or(0);
        natural.min(available).max(1)
    }

    fn render_model_row(
        &self,
        index: usize,
        entry: &ModelPickerEntry,
        width: u16,
    ) -> Line<'static> {
        let is_selected = self.state.selected_idx == Some(index);
        let marker = if is_selected { "›" } else { " " };
        let marker_style = if is_selected {
            Style::default().fg(self.accent_color).bold()
        } else {
            Style::default()
        };
        // Focused rows use underline in addition to accent+bold so the selection
        // stays readable on themes where the accent is close to the default fg.
        let label_style = if is_selected {
            Style::default().fg(self.accent_color).bold().underlined()
        } else if !entry.is_current {
            Style::default().dim()
        } else {
            Style::default()
        };
        let name_col = self.name_column_width(width);
        let mut name_width = UnicodeWidthStr::width(entry.display_name.as_str());
        let mut title_spans = vec![
            Span::styled(marker.to_string(), marker_style),
            Span::raw(" "),
            Span::styled(entry.display_name.clone(), label_style),
        ];
        // Current-session mark sits immediately after the model name.
        if entry.is_current {
            title_spans.push(Span::raw(" "));
            title_spans.push(Span::styled(
                "‹".to_string(),
                Style::default().fg(self.accent_color).bold(),
            ));
            name_width = name_width.saturating_add(2);
        }
        if name_width < name_col {
            title_spans.push(Span::raw(" ".repeat(name_col - name_width)));
        }
        if let Some(right_hint) = entry
            .right_hint
            .as_deref()
            .map(str::trim)
            .filter(|right_hint| !right_hint.is_empty())
        {
            title_spans.push(Span::raw("  "));
            title_spans.push(Span::styled(right_hint.to_string(), Style::default().dim()));
        }
        truncate_line_with_ellipsis_if_overflow(Line::from(title_spans), usize::from(width))
    }

    fn scroll_overflow_line(&self, more_above: bool) -> Line<'static> {
        // Align with model names: marker column + following space.
        let label = if more_above { "↑ more" } else { "↓ more" };
        Line::from(vec![
            Span::raw(" ".repeat(MARKER_COLS)),
            Span::styled(label.to_string(), Style::default().dim()),
        ])
    }

    fn render_effort_line(&self, width: u16) -> Option<Line<'static>> {
        let entry = self.current_entry()?;
        if entry.effort_options.is_empty() {
            return None;
        }

        let available = usize::from(width);
        let window = effort_window(
            &entry.effort_options,
            self.effort_selection.as_deref(),
            available,
        );

        let mut spans = Vec::new();
        if window.show_left_more {
            spans.push(Span::styled("‹ ".to_string(), Style::default().dim()));
        }
        for (idx_in_window, option_idx) in window.indices.iter().enumerate() {
            if idx_in_window > 0 {
                spans.push(Span::raw("  "));
            }
            let option = &entry.effort_options[*option_idx];
            let is_selected = self.effort_selection.as_deref() == Some(option.value.as_str());
            let label = if is_selected {
                format!("[{}]", option.label)
            } else {
                option.label.clone()
            };
            let style = if is_selected {
                Style::default().fg(self.accent_color).bold().underlined()
            } else {
                Style::default().dim()
            };
            spans.push(Span::styled(label, style));
        }
        if window.show_right_more {
            spans.push(Span::styled(" ›".to_string(), Style::default().dim()));
        }
        Some(Line::from(spans))
    }

    fn footer_hint_line(&self) -> Line<'static> {
        let has_effort = self
            .current_entry()
            .is_some_and(|entry| !entry.effort_options.is_empty());
        let mut spans = vec![
            key_hint::plain(KeyCode::Up).into(),
            Span::raw("/"),
            key_hint::plain(KeyCode::Down).into(),
            Span::raw(" model"),
        ];
        if has_effort {
            spans.extend([
                Span::raw("  "),
                key_hint::plain(KeyCode::Left).into(),
                Span::raw("/"),
                key_hint::plain(KeyCode::Right).into(),
                Span::raw(" effort"),
            ]);
        }
        spans.extend([
            Span::raw("  "),
            key_hint::plain(KeyCode::Enter).into(),
            Span::raw(" confirm  "),
            key_hint::plain(KeyCode::Esc).into(),
            Span::raw(" cancel"),
        ]);
        Line::from(spans).dim()
    }

    fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let len = self.entries.len();
        let visible = self.visible_rows();

        if self.has_more_above() {
            lines.push(self.scroll_overflow_line(/*more_above*/ true));
        }
        if visible > 0 {
            let start = self.state.scroll_top;
            let end = (start + visible).min(len);
            for index in start..end {
                if let Some(entry) = self.entries.get(index) {
                    lines.push(self.render_model_row(index, entry, width));
                }
            }
        }
        if self.has_more_below() {
            lines.push(self.scroll_overflow_line(/*more_above*/ false));
        }

        if let Some(effort_line) = self.render_effort_line(width) {
            lines.push(Line::from(""));
            lines.push(effort_line);
        }

        lines.push(Line::from(""));
        lines.push(self.footer_hint_line());
        debug_assert_eq!(
            lines.len(),
            usize::from(self.computed_height()),
            "render_lines length must match computed_height"
        );
        lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffortWindow {
    indices: Vec<usize>,
    show_left_more: bool,
    show_right_more: bool,
}

fn option_chip_width(option: &ModelPickerEffortOption, selected: bool) -> usize {
    if selected {
        UnicodeWidthStr::width(option.label.as_str()) + 2
    } else {
        UnicodeWidthStr::width(option.label.as_str())
    }
}

fn effort_window(
    options: &[ModelPickerEffortOption],
    selected_value: Option<&str>,
    available_width: usize,
) -> EffortWindow {
    if options.is_empty() {
        return EffortWindow {
            indices: Vec::new(),
            show_left_more: false,
            show_right_more: false,
        };
    }

    let selected_idx = selected_value
        .and_then(|value| options.iter().position(|option| option.value == value))
        .unwrap_or(0);

    // Try to fit all options first (with gaps and optional edge markers budget).
    let all_width = options
        .iter()
        .enumerate()
        .map(|(idx, option)| {
            let chip = option_chip_width(option, idx == selected_idx);
            if idx == 0 { chip } else { chip + 2 }
        })
        .sum::<usize>();
    if all_width <= available_width {
        return EffortWindow {
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
    let mut used = option_chip_width(&options[selected_idx], true);

    loop {
        let can_grow_left = start > 0;
        let can_grow_right = end + 1 < options.len();
        if !can_grow_left && !can_grow_right {
            break;
        }

        let mut grew = false;
        if can_grow_left {
            let next = start - 1;
            let extra = option_chip_width(&options[next], false) + 2;
            if used + extra <= content_budget {
                start = next;
                used += extra;
                grew = true;
            }
        }
        if can_grow_right {
            let next = end + 1;
            let extra = option_chip_width(&options[next], false) + 2;
            if used + extra <= content_budget {
                end = next;
                used += extra;
                grew = true;
            }
        }
        if !grew {
            // Prefer expanding toward the side that still has room if one side
            // failed only because the other already took the budget — stop.
            break;
        }
    }

    EffortWindow {
        indices: (start..=end).collect(),
        show_left_more: start > 0,
        show_right_more: end + 1 < options.len(),
    }
}

impl BottomPaneView for ModelPickerView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => self.complete = true,
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_model_selection(-1),
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_model_selection(1),
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => self.move_effort(-1),
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => self.move_effort(1),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.accept(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn replaces_composer(&self) -> bool {
        // Replaces the input → paint menu-surface background in `render`.
        true
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }

    fn take_model_selection(&mut self) -> Option<ModelPickerSelection> {
        self.selected.take()
    }
}

impl Renderable for ModelPickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        Paragraph::new(self.render_lines(content_area.width)).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        menu_surface_padding_height().saturating_add(self.computed_height())
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;

    use super::*;

    fn entry(
        value: &str,
        name: &str,
        is_current: bool,
        efforts: &[(&str, &str)],
        selected_effort: Option<&str>,
    ) -> ModelPickerEntry {
        ModelPickerEntry {
            selection_value: value.to_string(),
            display_name: name.to_string(),
            right_hint: None,
            is_current,
            effort_options: efforts
                .iter()
                .map(|(label, value)| ModelPickerEffortOption {
                    label: (*label).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
            selected_effort: selected_effort.map(str::to_string),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn caps_visible_rows_and_scrolls_with_selection() {
        let entries: Vec<_> = (0..12)
            .map(|i| entry(&format!("m{i}"), &format!("Model {i}"), i == 0, &[], None))
            .collect();
        let mut view = ModelPickerView::new(entries, Color::Cyan);

        assert_eq!(view.visible_rows(), MAX_POPUP_ROWS);
        // 8 models + more-below + blank + footer, plus menu-surface padding
        assert_eq!(
            view.desired_height(80),
            menu_surface_padding_height() + MAX_POPUP_ROWS as u16 + 3
        );
        assert_eq!(
            view.render_lines(80).len(),
            usize::from(view.computed_height())
        );

        for _ in 0..10 {
            view.handle_key_event(key(KeyCode::Down));
        }
        assert_eq!(view.state.selected_idx, Some(10));
        assert!(view.state.scroll_top + view.visible_rows() > 10);
        assert!(view.state.selected_idx.unwrap() >= view.state.scroll_top);
        assert!(view.state.selected_idx.unwrap() < view.state.scroll_top + view.visible_rows());
        assert!(view.has_more_above());
        // Near the end: may still have more below depending on scroll window.
        let lines = view.render_lines(80);
        assert!(
            lines
                .iter()
                .any(|line| line.to_string().contains("↓ more")
                    || line.to_string().contains("↑ more")),
            "expected scroll overflow marker:\n{}",
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(lines.len(), usize::from(view.computed_height()));
    }

    #[test]
    fn focused_row_uses_chevron_and_current_uses_right_mark() {
        let view = ModelPickerView::new(
            vec![
                ModelPickerEntry {
                    selection_value: "a".to_string(),
                    display_name: "Current".to_string(),
                    right_hint: Some("OpenAI".to_string()),
                    is_current: true,
                    effort_options: Vec::new(),
                    selected_effort: None,
                },
                entry("b", "Other", false, &[], None),
            ],
            Color::Cyan,
        );
        let focused_current = view.render_model_row(0, &view.entries[0], 80).to_string();
        assert!(
            focused_current.contains('›'),
            "expected › on focused row: {focused_current:?}"
        );
        assert!(
            focused_current.contains("Current ‹"),
            "‹ should sit immediately after the model name: {focused_current:?}"
        );
        assert!(
            focused_current.contains("OpenAI"),
            "provider hint should remain: {focused_current:?}"
        );

        let mut view = view;
        view.handle_key_event(key(KeyCode::Down));
        let current_unfocused = view.render_model_row(0, &view.entries[0], 80).to_string();
        let focused_other = view.render_model_row(1, &view.entries[1], 80).to_string();
        assert!(
            !current_unfocused.contains('›') && current_unfocused.contains("Current ‹"),
            "unfocused current keeps ‹ after name: {current_unfocused:?}"
        );
        assert!(
            focused_other.contains('›') && !focused_other.contains('‹'),
            "focused non-current: {focused_other:?}"
        );
    }

    #[test]
    fn provider_column_aligns_across_rows() {
        let view = ModelPickerView::new(
            vec![
                ModelPickerEntry {
                    selection_value: "a".to_string(),
                    display_name: "Short".to_string(),
                    right_hint: Some("OpenAI".to_string()),
                    is_current: false,
                    effort_options: Vec::new(),
                    selected_effort: None,
                },
                ModelPickerEntry {
                    selection_value: "b".to_string(),
                    display_name: "A Longer Model Name".to_string(),
                    right_hint: Some("Anthropic".to_string()),
                    is_current: true,
                    effort_options: Vec::new(),
                    selected_effort: None,
                },
            ],
            Color::Cyan,
        );
        let short = view.render_model_row(0, &view.entries[0], 80).to_string();
        let longer = view.render_model_row(1, &view.entries[1], 80).to_string();
        let provider_display_col = |line: &str, provider: &str| {
            let byte_idx = line.find(provider).expect("provider present");
            UnicodeWidthStr::width(&line[..byte_idx])
        };
        assert_eq!(
            provider_display_col(&short, "OpenAI"),
            provider_display_col(&longer, "Anthropic"),
            "providers should share a column\nshort={short:?}\nlonger={longer:?}"
        );
    }

    #[test]
    fn scroll_overflow_markers_appear_when_list_is_long() {
        let entries: Vec<_> = (0..12)
            .map(|i| entry(&format!("m{i}"), &format!("Model {i}"), i == 0, &[], None))
            .collect();
        let mut view = ModelPickerView::new(entries, Color::Cyan);

        let at_top = view.render_lines(80);
        assert!(
            !view.has_more_above() && view.has_more_below(),
            "start at top of long list"
        );
        assert!(
            at_top.iter().any(|line| {
                let text = line.to_string();
                text.contains("↓ more") && text.starts_with("  ↓ more")
            }),
            "overflow marker should align under model names:\n{}",
            at_top
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );

        for _ in 0..11 {
            view.handle_key_event(key(KeyCode::Down));
        }
        assert!(view.has_more_above());
        assert!(!view.has_more_below() || view.state.scroll_top + view.visible_rows() < 12);
        let at_bottomish = view.render_lines(80);
        assert!(
            at_bottomish
                .iter()
                .any(|line| line.to_string().contains("↑ more")),
            "expected upper overflow marker near bottom"
        );
    }

    #[test]
    fn left_right_cycle_effort_for_focused_model() {
        let mut view = ModelPickerView::new(
            vec![entry(
                "gpt",
                "GPT",
                true,
                &[("Low", "low"), ("Medium", "medium"), ("High", "high")],
                Some("medium"),
            )],
            Color::Cyan,
        );
        assert_eq!(view.effort_selection.as_deref(), Some("medium"));

        view.handle_key_event(key(KeyCode::Right));
        assert_eq!(view.effort_selection.as_deref(), Some("high"));
        view.handle_key_event(key(KeyCode::Right));
        assert_eq!(view.effort_selection.as_deref(), Some("low"));
        view.handle_key_event(key(KeyCode::Left));
        assert_eq!(view.effort_selection.as_deref(), Some("high"));
    }

    #[test]
    fn switching_models_preserves_effort_when_supported() {
        let mut view = ModelPickerView::new(
            vec![
                entry(
                    "a",
                    "A",
                    true,
                    &[("Low", "low"), ("High", "high")],
                    Some("low"),
                ),
                entry(
                    "b",
                    "B",
                    false,
                    &[("High", "high"), ("Max", "max")],
                    Some("max"),
                ),
            ],
            Color::Cyan,
        );
        view.handle_key_event(key(KeyCode::Right));
        assert_eq!(view.effort_selection.as_deref(), Some("high"));

        view.handle_key_event(key(KeyCode::Down));
        assert_eq!(view.state.selected_idx, Some(1));
        assert_eq!(view.effort_selection.as_deref(), Some("high"));
    }

    #[test]
    fn switching_models_falls_back_when_effort_unsupported() {
        let mut view = ModelPickerView::new(
            vec![
                entry(
                    "a",
                    "A",
                    true,
                    &[("Low", "low"), ("High", "high")],
                    Some("low"),
                ),
                entry("b", "B", false, &[("Max", "max")], Some("max")),
            ],
            Color::Cyan,
        );
        assert_eq!(view.effort_selection.as_deref(), Some("low"));
        view.handle_key_event(key(KeyCode::Down));
        assert_eq!(view.effort_selection.as_deref(), Some("max"));
    }

    #[test]
    fn hides_effort_row_when_unsupported() {
        let view =
            ModelPickerView::new(vec![entry("plain", "Plain", true, &[], None)], Color::Cyan);
        let lines = view.render_lines(80);
        assert!(
            lines
                .iter()
                .all(|line| !line.to_string().contains("Reasoning"))
        );
        // model row + blank + footer
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn effort_line_omits_reasoning_label() {
        let view = ModelPickerView::new(
            vec![entry(
                "gpt",
                "GPT",
                true,
                &[("Low", "low"), ("High", "high")],
                Some("low"),
            )],
            Color::Cyan,
        );
        let effort = view
            .render_effort_line(80)
            .expect("effort line")
            .to_string();
        assert!(!effort.contains("Reasoning"));
        assert!(effort.contains("[Low]") || effort.contains("Low"));
        assert!(
            effort.starts_with('[') || effort.starts_with('L') || effort.starts_with('‹'),
            "effort line should start at content edge (menu surface supplies inset): {effort:?}"
        );
    }

    #[test]
    fn content_lines_use_menu_surface_inset_without_extra_left_pad() {
        let view = ModelPickerView::new(
            vec![entry(
                "gpt",
                "GPT",
                true,
                &[("Off", "disabled"), ("High", "high")],
                Some("disabled"),
            )],
            Color::Cyan,
        );
        let lines = view.render_lines(80);
        let model = lines[0].to_string();
        let effort = lines
            .iter()
            .find(|line| line.to_string().contains("[Off]"))
            .expect("effort line")
            .to_string();
        let footer = lines.last().expect("footer").to_string();
        // Marker (›/ /·) then name — no extra FOOTER_INDENT on top of menu surface.
        assert!(
            model.starts_with('›') || model.starts_with(' ') || model.starts_with('·'),
            "model row: {model:?}"
        );
        assert!(
            !model.starts_with("  ›") && !model.starts_with("   "),
            "model row should not double-pad: {model:?}"
        );
        assert!(
            effort.starts_with('[') || effort.starts_with('O'),
            "effort row: {effort:?}"
        );
        assert!(
            !footer.starts_with("  "),
            "footer should not double-pad: {footer:?}"
        );
    }

    #[test]
    fn left_right_noop_without_effort_options() {
        let mut view =
            ModelPickerView::new(vec![entry("plain", "Plain", true, &[], None)], Color::Cyan);
        view.handle_key_event(key(KeyCode::Left));
        view.handle_key_event(key(KeyCode::Right));
        assert_eq!(view.effort_selection, None);
        assert!(!view.is_complete());
    }

    #[test]
    fn enter_returns_model_and_effort() {
        let mut view = ModelPickerView::new(
            vec![entry(
                "gpt",
                "GPT",
                true,
                &[("Low", "low"), ("High", "high")],
                Some("low"),
            )],
            Color::Cyan,
        );
        view.handle_key_event(key(KeyCode::Right));
        view.handle_key_event(key(KeyCode::Enter));
        assert!(view.is_complete());
        assert_eq!(
            view.take_model_selection(),
            Some(ModelPickerSelection {
                model: "gpt".to_string(),
                reasoning_effort: Some("high".to_string()),
            })
        );
    }

    #[test]
    fn esc_completes_without_selection() {
        let mut view = ModelPickerView::new(
            vec![entry("gpt", "GPT", true, &[("High", "high")], Some("high"))],
            Color::Cyan,
        );
        view.handle_key_event(key(KeyCode::Esc));
        assert!(view.is_complete());
        assert_eq!(view.take_model_selection(), None);
    }

    #[test]
    fn effort_window_slides_when_narrow() {
        let options = vec![
            ModelPickerEffortOption {
                label: "Minimal".to_string(),
                value: "minimal".to_string(),
            },
            ModelPickerEffortOption {
                label: "Low".to_string(),
                value: "low".to_string(),
            },
            ModelPickerEffortOption {
                label: "Medium".to_string(),
                value: "medium".to_string(),
            },
            ModelPickerEffortOption {
                label: "High".to_string(),
                value: "high".to_string(),
            },
            ModelPickerEffortOption {
                label: "XHigh".to_string(),
                value: "xhigh".to_string(),
            },
        ];
        let window = effort_window(&options, Some("medium"), 20);
        assert!(
            window.show_left_more || window.show_right_more || window.indices.len() < options.len()
        );
        assert!(window.indices.contains(&2));
    }
}
