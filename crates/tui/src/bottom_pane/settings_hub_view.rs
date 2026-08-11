//! Settings Hub bottom-pane view for `/settings`.
//!
//! Tabbed overview (Session / Appearance / Agent) that deep-links into existing
//! pickers and cycles Theme with ←/→ via [`AppEvent`] actions.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
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

use crate::app_event::AppEvent;
use crate::app_event::SettingsCycleDirection;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::InputMode;
use crate::key_hint;
use crate::render::renderable::Renderable;
use crate::ui_consts::FOOTER_INDENT_COLS;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

const LEFT_PAD: usize = FOOTER_INDENT_COLS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsHubTab {
    Session,
    Appearance,
    Agent,
}

impl SettingsHubTab {
    fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Appearance => "Appearance",
            Self::Agent => "Agent",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Session => Self::Appearance,
            Self::Appearance => Self::Agent,
            Self::Agent => Self::Session,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Session => Self::Agent,
            Self::Appearance => Self::Session,
            Self::Agent => Self::Appearance,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsHubSnapshot {
    pub(crate) model_label: String,
    pub(crate) permissions_label: String,
    pub(crate) mode: InputMode,
    pub(crate) compaction_threshold_label: String,
    pub(crate) theme_label: String,
    pub(crate) reasoning_view_label: String,
}

/// Interactive Settings Hub panel.
pub(crate) struct SettingsHubView {
    snapshot: SettingsHubSnapshot,
    tab: SettingsHubTab,
    selected_row: usize,
    complete: bool,
    app_event_tx: AppEventSender,
    accent_color: Color,
}

impl SettingsHubView {
    pub(crate) fn new(
        snapshot: SettingsHubSnapshot,
        app_event_tx: AppEventSender,
        accent_color: Color,
    ) -> Self {
        Self {
            snapshot,
            tab: SettingsHubTab::Session,
            selected_row: 0,
            complete: false,
            app_event_tx,
            accent_color,
        }
    }

    pub(crate) fn with_tab(mut self, tab: SettingsHubTab) -> Self {
        self.tab = tab;
        self.selected_row = 0;
        self
    }

    pub(crate) fn update_snapshot(&mut self, snapshot: SettingsHubSnapshot) {
        self.snapshot = snapshot;
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        let len = self.row_count().max(1);
        if self.selected_row >= len {
            self.selected_row = len - 1;
        }
    }

    fn row_count(&self) -> usize {
        match self.tab {
            SettingsHubTab::Session => 4,
            SettingsHubTab::Appearance => 2,
            SettingsHubTab::Agent => 0,
        }
    }

    fn theme_row_focused(&self) -> bool {
        self.tab == SettingsHubTab::Appearance && self.selected_row == 0
    }

    fn dismiss(&mut self) {
        self.complete = true;
    }

    fn cycle_theme(&mut self, direction: SettingsCycleDirection) {
        self.app_event_tx
            .send(AppEvent::SettingsCycleTheme { direction });
    }

    fn activate(&mut self) {
        match self.tab {
            SettingsHubTab::Session => match self.selected_row {
                0 => self.app_event_tx.send(AppEvent::SettingsOpenModel),
                1 => self.app_event_tx.send(AppEvent::SettingsOpenPermissions),
                2 => self.app_event_tx.send(AppEvent::SettingsCycleMode),
                3 => self.app_event_tx.send(AppEvent::SettingsOpenCompaction),
                _ => {}
            },
            SettingsHubTab::Appearance => match self.selected_row {
                0 => self.cycle_theme(SettingsCycleDirection::Next),
                1 => self.app_event_tx.send(AppEvent::SettingsOpenReasoning),
                _ => {}
            },
            SettingsHubTab::Agent => {}
        }
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let pad = " ".repeat(LEFT_PAD);

        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("Settings".to_string(), Style::default().bold()),
        ]));
        lines.push(Line::from(""));
        lines.push(self.tab_line(&pad));
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("─".repeat(40), Style::default().dim()),
        ]));
        lines.push(Line::from(""));

        match self.tab {
            SettingsHubTab::Session => {
                lines.push(self.setting_row(
                    &pad,
                    "Default Model",
                    &self.snapshot.model_label,
                    self.selected_row == 0,
                    /*cycleable*/ false,
                ));
                lines.push(self.setting_row(
                    &pad,
                    "Default Permissions",
                    &self.snapshot.permissions_label,
                    self.selected_row == 1,
                    /*cycleable*/ false,
                ));
                lines.push(self.setting_row(
                    &pad,
                    "Default Mode",
                    self.snapshot.mode.label(),
                    self.selected_row == 2,
                    /*cycleable*/ false,
                ));
                lines.push(Line::from(""));
                lines.push(self.setting_row(
                    &pad,
                    "Default Compaction Limit",
                    &self.snapshot.compaction_threshold_label,
                    self.selected_row == 3,
                    /*cycleable*/ false,
                ));
            }
            SettingsHubTab::Appearance => {
                lines.push(self.setting_row(
                    &pad,
                    "Theme",
                    &self.snapshot.theme_label,
                    self.selected_row == 0,
                    /*cycleable*/ true,
                ));
                lines.push(self.setting_row(
                    &pad,
                    "Show reasoning",
                    &self.snapshot.reasoning_view_label,
                    self.selected_row == 1,
                    /*cycleable*/ false,
                ));
            }
            SettingsHubTab::Agent => {
                lines.push(Line::from(vec![
                    Span::raw(pad.clone()),
                    Span::styled("Coming soon".to_string(), Style::default().dim()),
                ]));
                lines.push(Line::from(vec![
                    Span::raw(pad.clone()),
                    Span::styled(
                        "Personality and agent style settings will live here.".to_string(),
                        Style::default().dim(),
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(self.footer_line(&pad));
        lines
    }

    fn tab_line(&self, pad: &str) -> Line<'static> {
        let tabs = [
            SettingsHubTab::Session,
            SettingsHubTab::Appearance,
            SettingsHubTab::Agent,
        ];
        let mut spans = vec![Span::raw(pad.to_string())];
        for (idx, tab) in tabs.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::raw("    ".to_string()));
            }
            let active = *tab == self.tab;
            let marker = if active { "● " } else { "○ " };
            let style = if active {
                Style::default().fg(self.accent_color).bold()
            } else {
                Style::default().dim()
            };
            spans.push(Span::styled(format!("{marker}{}", tab.label()), style));
        }
        Line::from(spans)
    }

    fn setting_row(
        &self,
        pad: &str,
        label: &str,
        value: &str,
        focused: bool,
        cycleable: bool,
    ) -> Line<'static> {
        let label_width = 26usize;
        let label_text = format!("{label:<label_width$}");
        let style = if focused {
            Style::default().bold()
        } else {
            Style::default().dim()
        };
        let value_text = if focused && cycleable {
            format!("◀ {value} ▶")
        } else {
            value.to_string()
        };
        Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled(label_text, style),
            Span::styled(value_text, style),
        ])
    }

    fn footer_line(&self, pad: &str) -> Line<'static> {
        let mut spans = vec![
            Span::raw(pad.to_string()),
            key_hint::plain(KeyCode::Up).into(),
            Span::raw("/".to_string()),
            key_hint::plain(KeyCode::Down).into(),
            Span::raw(" navigate  ".to_string()),
        ];
        if self.theme_row_focused() {
            spans.push(key_hint::plain(KeyCode::Left).into());
            spans.push(Span::raw("/".to_string()));
            spans.push(key_hint::plain(KeyCode::Right).into());
            spans.push(Span::raw(" theme  ".to_string()));
        } else if self.tab != SettingsHubTab::Agent {
            spans.push(key_hint::plain(KeyCode::Enter).into());
            spans.push(Span::raw(" open  ".to_string()));
        }
        spans.push(key_hint::plain(KeyCode::Tab).into());
        spans.push(Span::raw(" switch tab  ".to_string()));
        spans.push(key_hint::plain(KeyCode::Esc).into());
        spans.push(Span::raw(" close".to_string()));
        Line::from(spans).dim()
    }
}

impl BottomPaneView for SettingsHubView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        match key_event.code {
            KeyCode::Esc => self.dismiss(),
            KeyCode::Tab if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
                self.tab = self.tab.previous();
                self.selected_row = 0;
            }
            KeyCode::BackTab => {
                self.tab = self.tab.previous();
                self.selected_row = 0;
            }
            KeyCode::Tab => {
                self.tab = self.tab.next();
                self.selected_row = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.row_count() == 0 {
                    return;
                }
                if self.selected_row == 0 {
                    self.selected_row = self.row_count() - 1;
                } else {
                    self.selected_row -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.row_count() == 0 {
                    return;
                }
                self.selected_row = (self.selected_row + 1) % self.row_count();
            }
            KeyCode::Left | KeyCode::Char('h') if self.theme_row_focused() => {
                self.cycle_theme(SettingsCycleDirection::Previous);
            }
            KeyCode::Right | KeyCode::Char('l') if self.theme_row_focused() => {
                self.cycle_theme(SettingsCycleDirection::Next);
            }
            KeyCode::Enter => self.activate(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some("settings_hub")
    }

    fn update_settings_hub_snapshot(&mut self, snapshot: SettingsHubSnapshot) -> bool {
        self.update_snapshot(snapshot);
        true
    }

    fn set_accent_color(&mut self, color: Color) {
        self.accent_color = color;
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

impl Renderable for SettingsHubView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        Paragraph::new(self.render_lines()).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // title + blank + tabs + separator + blank + content + blank + footer
        let content = match self.tab {
            SettingsHubTab::Session => 13,
            SettingsHubTab::Appearance => 10,
            SettingsHubTab::Agent => 10,
        };
        menu_surface_padding_height().saturating_add(content)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::app_event_sender::AppEventSender;

    fn view() -> (
        SettingsHubView,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) {
        let (tx, rx) = unbounded_channel();
        let view = SettingsHubView::new(
            SettingsHubSnapshot {
                model_label: "deepseek-v4-flash".into(),
                permissions_label: "default".into(),
                mode: InputMode::Build,
                compaction_threshold_label: "250K".into(),
                theme_label: "devo (default)".into(),
                reasoning_view_label: "Collapsed".into(),
            },
            AppEventSender::new(tx),
            Color::Cyan,
        );
        (view, rx)
    }

    #[test]
    fn session_rows_keep_space_between_label_and_value() {
        let (view, _rx) = view();
        let text = view
            .render_lines()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Default Compaction Limit  250K")
                || text.contains("Default Compaction Limit 250K"),
            "label must not run into value: {text}"
        );
        assert!(
            !text.contains("Default Compaction Limit250K"),
            "missing gap between compaction label and value: {text}"
        );
    }

    #[test]
    fn tab_cycles_with_tab_key() {
        let (mut view, _rx) = view();
        assert_eq!(view.tab, SettingsHubTab::Session);
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));
        assert_eq!(view.tab, SettingsHubTab::Appearance);
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));
        assert_eq!(view.tab, SettingsHubTab::Agent);
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));
        assert_eq!(view.tab, SettingsHubTab::Session);
    }

    #[test]
    fn enter_on_model_emits_open_model_event() {
        let (mut view, mut rx) = view();
        view.handle_key_event(KeyEvent::from(KeyCode::Enter));
        assert_eq!(rx.try_recv().ok(), Some(AppEvent::SettingsOpenModel));
    }

    #[test]
    fn left_right_on_theme_emits_cycle_events() {
        let (mut view, mut rx) = view();
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));
        assert_eq!(view.tab, SettingsHubTab::Appearance);
        view.handle_key_event(KeyEvent::from(KeyCode::Right));
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppEvent::SettingsCycleTheme {
                direction: SettingsCycleDirection::Next
            })
        );
        view.handle_key_event(KeyEvent::from(KeyCode::Left));
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppEvent::SettingsCycleTheme {
                direction: SettingsCycleDirection::Previous
            })
        );
    }

    #[test]
    fn theme_row_renders_cycle_markers_when_focused() {
        let (mut view, _rx) = view();
        view.handle_key_event(KeyEvent::from(KeyCode::Tab));
        let text = view
            .render_lines()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("◀ devo (default) ▶"));
    }
}
