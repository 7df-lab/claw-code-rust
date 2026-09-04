//! Absolute-token compaction threshold picker (legacy).
//!
//! The Settings › Compaction UI is removed; usable context is edited per model.
//! This module still exports [`format_token_limit`] for status/settings labels.

#![allow(dead_code)]

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::render::renderable::Renderable;
use crate::ui_consts::FOOTER_INDENT_COLS;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

const LEFT_PAD: usize = FOOTER_INDENT_COLS;

/// Snapshot used to open the compaction threshold picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionThresholdSnapshot {
    pub(crate) model_label: String,
    pub(crate) context_window_tokens: u64,
    pub(crate) recommended_token_limit: u64,
    pub(crate) current_token_limit: u64,
}

/// Absolute-token list for choosing the global auto-compaction threshold.
pub(crate) struct CompactionThresholdView {
    model_label: String,
    context_window_tokens: u64,
    recommended_token_limit: u64,
    current_token_limit: u64,
    options: Vec<u64>,
    state: ScrollState,
    complete: bool,
    app_event_tx: AppEventSender,
    accent_color: Color,
}

impl CompactionThresholdView {
    pub(crate) fn new(
        snapshot: CompactionThresholdSnapshot,
        app_event_tx: AppEventSender,
        accent_color: Color,
    ) -> Self {
        let options = compaction_threshold_presets(
            snapshot.context_window_tokens,
            snapshot.recommended_token_limit,
        );
        let mut state = ScrollState::new();
        let initial_idx = options
            .iter()
            .position(|value| *value == snapshot.current_token_limit)
            .or_else(|| {
                options.iter().position(|value| {
                    token_limits_display_equal(*value, snapshot.current_token_limit)
                })
            })
            .or_else(|| {
                options
                    .iter()
                    .position(|value| *value == snapshot.recommended_token_limit)
            })
            .unwrap_or(0);
        if options.is_empty() {
            state.selected_idx = None;
        } else {
            state.selected_idx = Some(initial_idx.min(options.len() - 1));
        }
        let visible = MAX_POPUP_ROWS.min(options.len());
        state.ensure_visible(options.len(), visible);

        Self {
            model_label: snapshot.model_label,
            context_window_tokens: snapshot.context_window_tokens,
            recommended_token_limit: snapshot.recommended_token_limit,
            current_token_limit: snapshot.current_token_limit,
            options,
            state,
            complete: false,
            app_event_tx,
            accent_color,
        }
    }

    fn dismiss(&mut self) {
        self.complete = true;
    }

    fn visible_rows(&self) -> usize {
        MAX_POPUP_ROWS.min(self.options.len())
    }

    fn apply_selection(&mut self) {
        let Some(idx) = self.state.selected_idx else {
            return;
        };
        let Some(limit) = self.options.get(idx).copied() else {
            return;
        };
        self.app_event_tx.send(AppEvent::Command(
            AppCommand::UpdateEffectiveContextWindow {
                effective_context_window: limit,
            },
        ));
        self.dismiss();
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let pad = " ".repeat(LEFT_PAD);

        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("Settings › Compaction".to_string(), Style::default().bold()),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("Model".to_string(), Style::default().dim()),
            Span::raw("     ".to_string()),
            Span::raw(self.model_label.clone()),
        ]));
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("Window".to_string(), Style::default().dim()),
            Span::raw("    ".to_string()),
            Span::raw(format_token_limit(self.context_window_tokens)),
        ]));
        lines.push(Line::from(""));

        if self.options.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(
                    "No compaction thresholds available for this model.".to_string(),
                    Style::default().dim(),
                ),
            ]));
        } else {
            let visible = self.visible_rows();
            let start = self.state.scroll_top;
            let end = (start + visible).min(self.options.len());
            if start > 0 {
                lines.push(Line::from(vec![
                    Span::raw(pad.clone()),
                    Span::styled("↑ more".to_string(), Style::default().dim()),
                ]));
            }
            for idx in start..end {
                let value = self.options[idx];
                let focused = self.state.selected_idx == Some(idx);
                lines.push(self.option_line(&pad, value, focused));
            }
            if end < self.options.len() {
                lines.push(Line::from(vec![
                    Span::raw(pad.clone()),
                    Span::styled("↓ more".to_string(), Style::default().dim()),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(self.footer_line(&pad));
        lines
    }

    fn option_line(&self, pad: &str, value: u64, focused: bool) -> Line<'static> {
        let marker = if focused { "> " } else { "  " };
        let mut annotations = Vec::new();
        if value == self.recommended_token_limit {
            annotations.push("(recommended)");
        }
        let is_current = value == self.current_token_limit
            || token_limits_display_equal(value, self.current_token_limit);
        if is_current {
            annotations.push("(current)");
        }
        let annotation = if annotations.is_empty() {
            String::new()
        } else {
            format!("  {}", annotations.join(" "))
        };
        let style = if focused {
            Style::default().fg(self.accent_color).bold()
        } else {
            Style::default().dim()
        };
        Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled(
                format!("{marker}{}{annotation}", format_token_limit(value)),
                style,
            ),
        ])
    }

    fn footer_line(&self, pad: &str) -> Line<'static> {
        Line::from(vec![
            Span::raw(pad.to_string()),
            key_hint::plain(KeyCode::Up).into(),
            Span::raw("/".to_string()),
            key_hint::plain(KeyCode::Down).into(),
            Span::raw(" navigate  ".to_string()),
            key_hint::plain(KeyCode::Enter).into(),
            Span::raw(" apply  ".to_string()),
            key_hint::plain(KeyCode::Esc).into(),
            Span::raw(" back".to_string()),
        ])
        .dim()
    }
}

impl BottomPaneView for CompactionThresholdView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let len = self.options.len();
        let visible = self.visible_rows();
        match key_event.code {
            KeyCode::Esc => self.dismiss(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.move_up_wrap(len);
                self.state.ensure_visible(len, visible);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.move_down_wrap(len);
                self.state.ensure_visible(len, visible);
            }
            KeyCode::Enter => self.apply_selection(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some("settings_compaction")
    }

    fn replaces_composer(&self) -> bool {
        true
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.dismiss();
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }
}

impl Renderable for CompactionThresholdView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        Paragraph::new(self.render_lines()).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let list_rows = self.visible_rows().saturating_add(
            usize::from(self.state.scroll_top > 0)
                + usize::from(self.state.scroll_top + self.visible_rows() < self.options.len()),
        );
        // title + blank + model + window + blank + list + blank + footer
        let content = u16::try_from(8usize.saturating_add(list_rows.max(1))).unwrap_or(u16::MAX);
        menu_surface_padding_height().saturating_add(content)
    }
}

/// Builds the Settings compaction preset ladder.
///
/// The list uses round product values only (`100K`…`1M`, plus `250K`). It is
/// intentionally **not** keyed off the raw model `context_window` — that value
/// is informational in the header, and the server clamps on apply. This avoids
/// duplicate labels such as two rows both rendering as `1M`.
pub(crate) fn compaction_threshold_presets(_context_window: u64, recommended: u64) -> Vec<u64> {
    let mut values = vec![
        100_000, 200_000, 250_000, 300_000, 400_000, 500_000, 600_000, 700_000, 800_000, 900_000,
        1_000_000,
    ];
    if recommended > 0 {
        values.push(recommended);
    }
    values.sort_unstable();
    values.dedup();
    dedupe_presets_by_display_label(values)
}

fn dedupe_presets_by_display_label(values: Vec<u64>) -> Vec<u64> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        if out
            .last()
            .is_some_and(|prev| token_limits_display_equal(*prev, value))
        {
            // Prefer the rounder canonical step already kept in `out`.
            continue;
        }
        out.push(value);
    }
    out
}

/// Formats absolute token counts for Settings labels.
///
/// Near-million values (for example `996147`) render as `1M` so catalog windows
/// that are slightly under a round million still read cleanly.
pub(crate) fn format_token_limit(tokens: u64) -> String {
    if tokens >= 950_000 {
        let millions = ((tokens as f64) / 1_000_000.0).round().max(1.0) as u64;
        return format!("{millions}M");
    }
    if tokens >= 1_000 && tokens.is_multiple_of(1_000) {
        format!("{}K", tokens / 1_000)
    } else if tokens >= 1_000 {
        let whole = tokens / 1_000;
        let frac = (tokens % 1_000) / 100;
        if frac == 0 {
            format!("{whole}K")
        } else {
            format!("{whole}.{frac}K")
        }
    } else {
        tokens.to_string()
    }
}

fn token_limits_display_equal(left: u64, right: u64) -> bool {
    format_token_limit(left) == format_token_limit(right)
}

/// Product-facing recommended compaction threshold for the Settings picker.
///
/// Fixed at `250K`; apply still clamps to the active model context window.
pub(crate) fn recommended_compaction_token_limit(
    _context_window: u64,
    _model_effective: u64,
) -> u64 {
    250_000
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn presets_use_round_ladder_without_raw_window_duplicate() {
        let presets = compaction_threshold_presets(1_048_576, 250_000);
        assert_eq!(
            presets,
            vec![
                100_000, 200_000, 250_000, 300_000, 400_000, 500_000, 600_000, 700_000, 800_000,
                900_000, 1_000_000,
            ]
        );
        let labels: Vec<_> = presets.iter().copied().map(format_token_limit).collect();
        assert_eq!(labels.iter().filter(|label| *label == "1M").count(), 1);
        assert_eq!(
            recommended_compaction_token_limit(1_048_576, 996_147),
            250_000
        );
    }

    #[test]
    fn format_token_limit_uses_k_suffix() {
        assert_eq!(format_token_limit(190_000), "190K");
        assert_eq!(format_token_limit(1_000_000), "1M");
        assert_eq!(format_token_limit(996_147), "1M");
    }
}
