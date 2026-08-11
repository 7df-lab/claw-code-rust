//! Secondary confirmation for `/delete`: title + horizontal Cancel/Delete chips.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::horizontal_chip_strip::HorizontalChipStrip;
use super::list_selection_view::SelectionAction;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;

const CANCEL_LABEL: &str = "Cancel";
const DELETE_LABEL: &str = "Delete";

/// Bottom-pane confirmation for deleting the current session.
pub(crate) struct DeleteSessionConfirmView {
    app_event_tx: AppEventSender,
    accent_color: Color,
    chips: HorizontalChipStrip,
    on_delete: SelectionAction,
    on_cancel: SelectionAction,
    complete: bool,
}

impl DeleteSessionConfirmView {
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        accent_color: Color,
        on_delete: SelectionAction,
        on_cancel: SelectionAction,
    ) -> Self {
        Self {
            app_event_tx,
            accent_color,
            // Default to Cancel so Enter is safe.
            chips: HorizontalChipStrip::new(
                vec![CANCEL_LABEL.to_string(), DELETE_LABEL.to_string()],
                /*selected*/ 0,
            ),
            on_delete,
            on_cancel,
            complete: false,
        }
    }

    fn accept(&mut self) {
        match self.chips.selected_index() {
            0 => (self.on_cancel)(&self.app_event_tx),
            _ => (self.on_delete)(&self.app_event_tx),
        }
        self.complete = true;
    }

    fn footer_hint_line() -> Line<'static> {
        Line::from(vec![
            Span::raw(" ".repeat(crate::ui_consts::FOOTER_INDENT_COLS)),
            key_hint::plain(KeyCode::Left).into(),
            Span::raw("/"),
            key_hint::plain(KeyCode::Right).into(),
            Span::raw(" choose  "),
            key_hint::plain(KeyCode::Enter).into(),
            Span::raw(" confirm  "),
            key_hint::plain(KeyCode::Esc).into(),
            Span::raw(" cancel"),
        ])
        .dim()
    }
}

impl BottomPaneView for DeleteSessionConfirmView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.chips.move_left(),
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.chips.move_right(),
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.accept(),
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                (self.on_cancel)(&self.app_event_tx);
                self.complete = true;
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        (self.on_cancel)(&self.app_event_tx);
        self.complete = true;
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        // Route Esc through handle_key_event so it cancels the same way as the
        // Cancel chip / footer hint, rather than only via on_ctrl_c.
        true
    }

    fn replaces_composer(&self) -> bool {
        // Match other selection-style confirmations: stack under the draft.
        false
    }
}

impl Renderable for DeleteSessionConfirmView {
    fn desired_height(&self, _width: u16) -> u16 {
        // menu padding + title + subtitle + gap + chips + gap + footer
        menu_surface_padding_height()
            .saturating_add(2) // title + subtitle
            .saturating_add(1) // gap
            .saturating_add(1) // chips
            .saturating_add(1) // gap before footer
            .saturating_add(1) // footer
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let content_area = render_menu_surface(content_area, buf);

        let [title_area, subtitle_area, _, chips_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(content_area);

        Paragraph::new(Line::from("Delete session?".bold())).render(title_area, buf);
        Paragraph::new(Line::from(
            "This permanently removes the current session history.".dim(),
        ))
        .render(subtitle_area, buf);

        let chip_line = self
            .chips
            .render_line(usize::from(chips_area.width), self.accent_color);
        Paragraph::new(chip_line).render(chips_area, buf);

        if footer_area.height > 0 {
            Paragraph::new(Self::footer_hint_line()).render(footer_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn view_with_rx() -> (
        DeleteSessionConfirmView,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) {
        let (tx, rx) = unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let view = DeleteSessionConfirmView::new(
            app_event_tx.clone(),
            Color::Cyan,
            Box::new(|tx: &AppEventSender| {
                tx.send(AppEvent::StatusMessageChanged {
                    message: "deleted".to_string(),
                });
            }),
            Box::new(|tx: &AppEventSender| {
                tx.send(AppEvent::StatusMessageChanged {
                    message: "cancelled".to_string(),
                });
            }),
        );
        (view, rx)
    }

    #[test]
    fn enter_on_cancel_does_not_delete() {
        let (mut view, mut rx) = view_with_rx();
        view.handle_key_event(key(KeyCode::Enter));
        assert!(view.is_complete());
        assert_eq!(
            rx.try_recv().expect("cancel status"),
            AppEvent::StatusMessageChanged {
                message: "cancelled".to_string(),
            }
        );
    }

    #[test]
    fn right_then_enter_deletes() {
        let (mut view, mut rx) = view_with_rx();
        view.handle_key_event(key(KeyCode::Right));
        view.handle_key_event(key(KeyCode::Enter));
        assert!(view.is_complete());
        assert_eq!(
            rx.try_recv().expect("delete status"),
            AppEvent::StatusMessageChanged {
                message: "deleted".to_string(),
            }
        );
    }

    #[test]
    fn esc_cancels() {
        let (mut view, mut rx) = view_with_rx();
        view.handle_key_event(key(KeyCode::Esc));
        assert!(view.is_complete());
        assert_eq!(
            rx.try_recv().expect("cancel status"),
            AppEvent::StatusMessageChanged {
                message: "cancelled".to_string(),
            }
        );
    }
}
