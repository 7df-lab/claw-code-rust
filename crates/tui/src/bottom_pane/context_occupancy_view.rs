//! Read-only context occupancy panel for `/context`.
//!
//! Shows effective window usage, category shares from [`ContextOccupancy`],
//! and session-cumulative input / output / cache totals. Stacks below the
//! composer (does not replace the input) and paints the shared menu-surface
//! background. Esc (or Enter) dismisses the panel.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::key_hint;
use crate::render::renderable::Renderable;
use devo_protocol::canonical::item::ContextCategoryId;
use devo_protocol::canonical::item::ContextCategoryUsage;
use devo_protocol::canonical::item::ContextOccupancy;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

const BAR_WIDTH: usize = 28;
const SESSION_LABEL_WIDTH: usize = 10;
const CATEGORY_COUNT: usize = 5;

/// Session-cumulative token accounting shown beneath window occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SessionTokenTotals {
    pub(crate) input: usize,
    pub(crate) output: usize,
    pub(crate) cache_read: usize,
}

impl SessionTokenTotals {
    fn cache_hit_percent(self) -> usize {
        if self.input == 0 {
            0
        } else {
            (self.cache_read.saturating_mul(100) + self.input / 2) / self.input
        }
    }
}

pub(crate) struct ContextOccupancyView {
    occupancy: Option<ContextOccupancy>,
    session: SessionTokenTotals,
    complete: bool,
}

impl ContextOccupancyView {
    pub(crate) fn new(occupancy: Option<ContextOccupancy>, session: SessionTokenTotals) -> Self {
        Self {
            occupancy,
            session,
            complete: false,
        }
    }

    fn dismiss(&mut self) {
        self.complete = true;
    }

    fn occupancy_or_zero(&self) -> ContextOccupancy {
        self.occupancy.clone().unwrap_or_else(|| {
            ContextOccupancy::from_category_tokens(
                /*context_window_tokens*/ 0, /*base*/ 0, /*skills*/ 0,
                /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 0,
            )
        })
    }

    fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let occupancy = self.occupancy_or_zero();

        lines.push(Line::from(Span::styled(
            "Context Usage",
            Style::default().bold(),
        )));
        lines.push(Line::from(""));
        lines.extend(window_summary_lines(&occupancy));
        lines.push(Line::from(""));
        lines.extend(category_lines(&occupancy));
        lines.push(Line::from(""));
        lines.push(section_title("Session"));
        lines.extend(session_lines(self.session));
        lines.push(Line::from(""));
        lines.push(self.footer_line());
        lines
    }

    fn footer_line(&self) -> Line<'static> {
        Line::from(vec![
            key_hint::plain(KeyCode::Esc).into(),
            Span::raw(" close"),
        ])
        .dim()
    }

    fn content_height(&self) -> u16 {
        // title + blank + summary + bar + blank + categories + blank + Session
        // + input + output + cache + blank + footer
        u16::try_from(
            1usize
                .saturating_add(4)
                .saturating_add(CATEGORY_COUNT)
                .saturating_add(5)
                .saturating_add(2),
        )
        .unwrap_or(u16::MAX)
    }
}

fn section_title(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), Style::default().bold()))
}

fn window_summary_lines(occupancy: &ContextOccupancy) -> Vec<Line<'static>> {
    let used = occupancy.total_tokens;
    let window = occupancy.context_window_tokens;
    let denom = window.max(1);
    let percent = ((used as f64 / denom as f64) * 100.0)
        .clamp(0.0, 100.0)
        .round() as u64;
    vec![
        Line::from(vec![
            Span::raw(format_tokens(used)),
            Span::styled(" / ", Style::default().dim()),
            Span::raw(format_tokens(window)),
            Span::styled(format!("  ·  {percent}%"), Style::default().dim()),
        ]),
        Line::from(Span::raw(render_bar(used, denom, BAR_WIDTH))),
    ]
}

fn category_lines(occupancy: &ContextOccupancy) -> Vec<Line<'static>> {
    let categories = display_categories(occupancy);
    let label_width = categories
        .iter()
        .map(|category| category_label(category.id).len())
        .max()
        .unwrap_or(0)
        .max(12);
    categories
        .iter()
        .map(|category| category_line(category, label_width))
        .collect()
}

fn display_categories(occupancy: &ContextOccupancy) -> Vec<ContextCategoryUsage> {
    const ORDER: [ContextCategoryId; CATEGORY_COUNT] = [
        ContextCategoryId::Base,
        ContextCategoryId::Skills,
        ContextCategoryId::ToolsBuiltin,
        ContextCategoryId::ToolsMcp,
        ContextCategoryId::Conversation,
    ];
    ORDER
        .into_iter()
        .map(|id| {
            occupancy
                .categories
                .iter()
                .find(|category| category.id == id)
                .cloned()
                .unwrap_or(ContextCategoryUsage {
                    id,
                    tokens: 0,
                    share_bps: 0,
                })
        })
        .collect()
}

fn session_lines(session: SessionTokenTotals) -> Vec<Line<'static>> {
    let cache_pct = session.cache_hit_percent();
    vec![
        metric_line("↑ input", format_tokens(session.input as u64)),
        metric_line("↓ output", format_tokens(session.output as u64)),
        Line::from(vec![
            Span::styled(
                format!("{:<SESSION_LABEL_WIDTH$}", "cache"),
                Style::default().dim(),
            ),
            Span::raw("  "),
            Span::raw(format_tokens(session.cache_read as u64)),
            Span::styled(
                format!("  ·  {cache_pct}% of input"),
                Style::default().dim(),
            ),
        ]),
    ]
}

fn metric_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<SESSION_LABEL_WIDTH$}"),
            Style::default().dim(),
        ),
        Span::raw("  "),
        Span::raw(value),
    ])
}

fn category_label(id: ContextCategoryId) -> &'static str {
    match id {
        ContextCategoryId::Base => "base",
        ContextCategoryId::Skills => "skills",
        ContextCategoryId::ToolsBuiltin => "tools (builtin)",
        ContextCategoryId::ToolsMcp => "tools (mcp)",
        ContextCategoryId::Conversation => "conversation",
    }
}

fn category_line(category: &ContextCategoryUsage, label_width: usize) -> Line<'static> {
    let label = category_label(category.id);
    let share_pct = u32::from(category.share_bps) / 100;
    Line::from(vec![
        Span::raw(format!("{label:<label_width$}")),
        Span::raw("  "),
        Span::raw(format!("{:>6}", format_tokens(category.tokens))),
        Span::styled(format!("  {share_pct:>3}%"), Style::default().dim()),
    ])
}

fn format_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn render_bar(used: u64, total: u64, width: usize) -> String {
    if width == 0 || total == 0 {
        return String::new();
    }
    let ratio = (used as f64 / total as f64).clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    let mut bar = String::with_capacity(width.saturating_mul(3));
    for _ in 0..filled {
        bar.push('▰');
    }
    for _ in 0..empty {
        bar.push('▱');
    }
    bar
}

impl BottomPaneView for ContextOccupancyView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc | KeyCode::Enter => self.dismiss(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.dismiss();
        CancellationEvent::Handled
    }
}

impl Renderable for ContextOccupancyView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        Paragraph::new(self.render_lines(content_area.width)).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        menu_surface_padding_height().saturating_add(self.content_height())
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn empty_state_renders_zeroed_layout() {
        let view = ContextOccupancyView::new(None, SessionTokenTotals::default());
        let text = view
            .render_lines(80)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Context Usage"));
        assert!(text.contains("0 / 0"));
        assert!(text.contains("base"));
        assert!(text.contains("conversation"));
        assert!(text.contains("Session"));
        assert!(text.contains("↑ input"));
        assert!(text.contains("esc close"));
        assert!(!text.contains("No occupancy data yet"));
    }

    #[test]
    fn session_totals_render_without_cache_bar() {
        let view = ContextOccupancyView::new(
            None,
            SessionTokenTotals {
                input: 124_000,
                output: 10_000,
                cache_read: 82_000,
            },
        );
        let text = view
            .render_lines(100)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Session"));
        assert!(text.contains("↑ input"));
        assert!(text.contains("↓ output"));
        assert!(text.contains("124.0k"));
        assert!(text.contains("10.0k"));
        assert!(text.contains("82.0k"));
        assert!(text.contains("66% of input"));
        assert!(!text.contains("Window"));
        assert!(text.contains("Context Usage"));
    }

    #[test]
    fn populated_occupancy_and_session_render_together() {
        let occupancy = ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 5_000,
            /*tools_builtin*/ 20_000, /*tools_mcp*/ 15_000, /*conversation*/ 50_000,
        );
        let view = ContextOccupancyView::new(
            Some(occupancy),
            SessionTokenTotals {
                input: 50_000,
                output: 2_000,
                cache_read: 25_000,
            },
        );
        let text = view
            .render_lines(100)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Context Usage"));
        assert!(!text.contains("Window"));
        assert!(text.contains("base"));
        assert!(text.contains("conversation"));
        assert!(text.contains("Session"));
        assert!(text.contains("50% of input"));
        assert!(text.contains('▰'));
        // Category rows keep tokens + percent, but not per-category bars.
        let base_line = text
            .lines()
            .find(|line| line.contains("base"))
            .expect("base category");
        assert!(base_line.contains('%'));
        assert!(!base_line.contains('▰'));
        assert!(!base_line.contains('▱'));
    }

    #[test]
    fn esc_and_enter_dismiss() {
        let mut view = ContextOccupancyView::new(None, SessionTokenTotals::default());
        assert!(!view.is_complete());
        view.handle_key_event(KeyEvent::from(KeyCode::Enter));
        assert_eq!(view.is_complete(), true);

        let mut view = ContextOccupancyView::new(None, SessionTokenTotals::default());
        view.handle_key_event(KeyEvent::from(KeyCode::Esc));
        assert_eq!(view.is_complete(), true);
    }

    #[test]
    fn stacks_under_composer_with_menu_surface_padding() {
        let view = ContextOccupancyView::new(None, SessionTokenTotals::default());
        assert!(!view.replaces_composer());
        assert_eq!(
            view.desired_height(/*width*/ 80),
            menu_surface_padding_height().saturating_add(17)
        );
    }
}
