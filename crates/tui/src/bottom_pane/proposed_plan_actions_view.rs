//! Proposed Plan action menu: implement the plan or type inline revise feedback.
//!
//! Shown after a Proposed Plan completes. Option 1 submits a Build turn; option 2
//! is an inline input — when highlighted, the user types feedback and presses
//! Enter to submit a Plan-mode revise turn.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::app_event_sender::AppEventSender;
use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::standard_popup_hint_line;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

const IMPLEMENT_NAME: &str = "Implement Plan";
const IMPLEMENT_DESCRIPTION: &str = "Switch to Build mode and start implementing this plan.";
const REVISE_NAME: &str = "Revise Plan";
const REVISE_PLACEHOLDER: &str = "Type feedback and press Enter";
const REVISE_DESCRIPTION: &str = "Keep Plan mode and revise this plan.";

/// Callback invoked when the user chooses Implement Plan.
pub(crate) type ImplementAction = Box<dyn Fn(&AppEventSender) + Send + Sync>;

/// Callback invoked when the user submits non-empty revise feedback.
pub(crate) type ReviseAction = Box<dyn Fn(&AppEventSender, String) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectedRow {
    Implement,
    Revise,
}

/// Bottom-pane popup for Proposed Plan follow-up actions.
pub(crate) struct ProposedPlanActionsView {
    app_event_tx: AppEventSender,
    accent_color: Color,
    selected: SelectedRow,
    revise_text: String,
    on_implement: ImplementAction,
    on_revise: ReviseAction,
    complete: bool,
}

impl ProposedPlanActionsView {
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        accent_color: Color,
        on_implement: ImplementAction,
        on_revise: ReviseAction,
    ) -> Self {
        Self {
            app_event_tx,
            accent_color,
            selected: SelectedRow::Implement,
            revise_text: String::new(),
            on_implement,
            on_revise,
            complete: false,
        }
    }

    fn move_up(&mut self) {
        self.selected = SelectedRow::Implement;
    }

    fn move_down(&mut self) {
        self.selected = SelectedRow::Revise;
    }

    fn accept(&mut self) {
        match self.selected {
            SelectedRow::Implement => {
                (self.on_implement)(&self.app_event_tx);
                self.complete = true;
            }
            SelectedRow::Revise => {
                let text = self.revise_text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                (self.on_revise)(&self.app_event_tx, text);
                self.complete = true;
            }
        }
    }

    fn append_revise_char(&mut self, c: char) {
        if !c.is_control() {
            self.revise_text.push(c);
        }
    }

    fn revise_row_label(&self, selected: bool) -> (String, bool) {
        if selected {
            if self.revise_text.is_empty() {
                (REVISE_PLACEHOLDER.to_string(), /*is_placeholder*/ true)
            } else {
                (self.revise_text.clone(), /*is_placeholder*/ false)
            }
        } else {
            (REVISE_NAME.to_string(), /*is_placeholder*/ false)
        }
    }
}

impl BottomPaneView for ProposedPlanActionsView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('\u{0010}'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_up(),
            KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } if self.selected == SelectedRow::Implement => self.move_up(),
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
                code: KeyCode::Char('\u{000e}'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } if self.selected == SelectedRow::Implement => self.move_down(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } if self.selected == SelectedRow::Revise => {
                self.revise_text.pop();
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.accept(),
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if self.selected == SelectedRow::Revise
                && !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
            {
                self.append_revise_char(c);
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if self.selected == SelectedRow::Implement
                && !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
            {
                match c {
                    '1' => {
                        self.selected = SelectedRow::Implement;
                        self.accept();
                    }
                    '2' => {
                        self.selected = SelectedRow::Revise;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if self.selected != SelectedRow::Revise || pasted.is_empty() {
            return false;
        }
        let cleaned: String = pasted.chars().filter(|c| !c.is_control()).collect();
        if cleaned.is_empty() {
            return false;
        }
        self.revise_text.push_str(&cleaned);
        true
    }

    fn selected_index(&self) -> Option<usize> {
        Some(match self.selected {
            SelectedRow::Implement => 0,
            SelectedRow::Revise => 1,
        })
    }
}

impl Renderable for ProposedPlanActionsView {
    fn desired_height(&self, _width: u16) -> u16 {
        // title + subtitle + gap + implement (2) + revise (1 or 2) + gap before footer + footer
        let revise_lines: u16 = if self.selected == SelectedRow::Revise {
            1
        } else {
            2
        };
        menu_surface_padding_height()
            .saturating_add(2) // title + subtitle
            .saturating_add(1) // header/list gap
            .saturating_add(2) // implement name + description
            .saturating_add(revise_lines)
            .saturating_add(1) // gap before footer
            .saturating_add(1) // footer hint
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let content_area = render_menu_surface(content_area, buf);

        let revise_lines: u16 = if self.selected == SelectedRow::Revise {
            1
        } else {
            2
        };
        let [title_area, subtitle_area, _, implement_area, revise_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(revise_lines),
        ])
        .areas(content_area);

        Paragraph::new(Line::from("Proposed Plan".bold())).render(title_area, buf);
        Paragraph::new(Line::from("Choose how to continue.".dim())).render(subtitle_area, buf);

        render_static_row(
            implement_area,
            buf,
            /*index*/ 1,
            IMPLEMENT_NAME,
            Some(IMPLEMENT_DESCRIPTION),
            self.selected == SelectedRow::Implement,
            /*is_placeholder*/ false,
            self.accent_color,
        );

        let (revise_label, is_placeholder) =
            self.revise_row_label(self.selected == SelectedRow::Revise);
        let revise_description =
            (self.selected != SelectedRow::Revise).then_some(REVISE_DESCRIPTION);
        render_static_row(
            revise_area,
            buf,
            /*index*/ 2,
            &revise_label,
            revise_description,
            self.selected == SelectedRow::Revise,
            is_placeholder,
            self.accent_color,
        );

        if footer_area.height > 0 {
            Paragraph::new(standard_popup_hint_line()).render(footer_area, buf);
        }
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if self.selected != SelectedRow::Revise || area.height == 0 || area.width == 0 {
            return None;
        }

        let [content_area, _] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let content_area = super::selection_popup_common::menu_surface_inset(content_area);
        let revise_lines: u16 = 1;
        let [_, _, _, _, revise_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(revise_lines),
        ])
        .areas(content_area);

        let prefix = "› 2. ";
        let text_width = UnicodeWidthStr::width(self.revise_text.as_str()) as u16;
        let x = revise_area
            .x
            .saturating_add(UnicodeWidthStr::width(prefix) as u16)
            .saturating_add(text_width)
            .min(
                revise_area
                    .x
                    .saturating_add(revise_area.width.saturating_sub(1)),
            );
        Some((x, revise_area.y))
    }
}

#[allow(clippy::too_many_arguments)]
fn render_static_row(
    area: Rect,
    buf: &mut Buffer,
    index: usize,
    name: &str,
    description: Option<&str>,
    selected: bool,
    is_placeholder: bool,
    accent_color: Color,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let marker = if selected { '›' } else { ' ' };
    let marker_style = if selected {
        Style::default().fg(accent_color).bold()
    } else {
        Style::default()
    };
    let name_style = if is_placeholder {
        Style::default().dim()
    } else if selected {
        Style::default().bold()
    } else {
        Style::default()
    };

    let name_line = Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::raw(format!(" {index}. ")),
        Span::styled(name.to_string(), name_style),
    ]);
    Paragraph::new(name_line).render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        buf,
    );

    if let Some(description) = description
        && area.height > 1
    {
        let indent = "      ";
        Paragraph::new(Line::from(format!("{indent}{description}").dim())).render(
            Rect {
                x: area.x,
                y: area.y.saturating_add(1),
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}
