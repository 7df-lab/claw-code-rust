use std::collections::HashMap;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use devo_protocol::RequestUserInputAnswer;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::RequestUserInputResponse;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::render::renderable::Renderable;
use crate::ui_consts::LIVE_PREFIX_COLS;

#[derive(Clone, Debug, PartialEq, Eq)]
struct QuestionChoice {
    label: String,
    description: String,
    is_other: bool,
}

pub(crate) struct RequestUserInputOverlay {
    session_id: SessionId,
    turn_id: TurnId,
    request_id: String,
    questions: Vec<RequestUserInputQuestion>,
    question_index: usize,
    selected_index: usize,
    freeform_text: String,
    answers: HashMap<String, RequestUserInputAnswer>,
    app_event_tx: AppEventSender,
    complete: bool,
    accent_color: Color,
}

impl RequestUserInputOverlay {
    pub(crate) fn new(
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: Vec<RequestUserInputQuestion>,
        app_event_tx: AppEventSender,
        accent_color: Color,
    ) -> Self {
        Self {
            session_id,
            turn_id,
            request_id,
            questions,
            question_index: 0,
            selected_index: 0,
            freeform_text: String::new(),
            answers: HashMap::new(),
            app_event_tx,
            complete: false,
            accent_color,
        }
    }

    fn current_question(&self) -> Option<&RequestUserInputQuestion> {
        self.questions.get(self.question_index)
    }

    fn choices(&self) -> Vec<QuestionChoice> {
        let Some(question) = self.current_question() else {
            return vec![other_choice()];
        };
        let mut choices = question
            .options
            .as_ref()
            .into_iter()
            .flatten()
            .map(|option| QuestionChoice {
                label: option.label.clone(),
                description: option.description.clone(),
                is_other: false,
            })
            .collect::<Vec<_>>();
        if choices.is_empty() || question.is_other {
            choices.push(other_choice());
        }
        choices
    }

    fn selected_choice_is_other(&self) -> bool {
        self.choices()
            .get(self.selected_index)
            .is_some_and(|choice| choice.is_other)
    }

    fn clamp_selection(&mut self) {
        let choice_count = self.choices().len();
        if choice_count == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.min(choice_count - 1);
        }
    }

    fn accept_current(&mut self) {
        let Some(question) = self.current_question().cloned() else {
            self.finish();
            return;
        };
        let choices = self.choices();
        let answer = choices
            .get(self.selected_index)
            .map(|choice| {
                if choice.is_other && !self.freeform_text.trim().is_empty() {
                    self.freeform_text.clone()
                } else {
                    choice.label.clone()
                }
            })
            .unwrap_or_default();
        self.answers.insert(
            question.id,
            RequestUserInputAnswer {
                answers: vec![answer],
            },
        );

        if self.question_index + 1 >= self.questions.len() {
            self.finish();
        } else {
            self.question_index += 1;
            self.selected_index = 0;
            self.freeform_text.clear();
        }
    }

    fn finish(&mut self) {
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::RequestUserInputRespond {
                session_id: self.session_id,
                turn_id: self.turn_id,
                request_id: self.request_id.clone(),
                response: RequestUserInputResponse {
                    answers: self.answers.clone(),
                },
            }));
        self.complete = true;
    }

    fn cancel(&mut self) {
        self.finish();
    }

    fn display_text(&self) -> String {
        if self
            .current_question()
            .is_some_and(|question| question.is_secret)
        {
            "*".repeat(self.freeform_text.chars().count())
        } else {
            self.freeform_text.clone()
        }
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let total = self.questions.len().max(1);
        lines.push(Line::from(vec![
            Span::styled("Input requested", Style::default().bold()),
            Span::raw(" "),
            Span::styled(
                format!("{}/{}", self.question_index + 1, total),
                Style::default().dim(),
            ),
        ]));

        if let Some(question) = self.current_question() {
            if !question.header.trim().is_empty() {
                lines.push(Line::from(question.header.clone()).bold());
            }
            lines.push(Line::from(question.question.clone()));
        }
        lines.push(Line::from(""));

        for (index, choice) in self.choices().iter().enumerate() {
            let selected = index == self.selected_index;
            let marker = if selected { "›" } else { " " };
            let marker_style = if selected {
                Style::default().fg(self.accent_color).bold()
            } else {
                Style::default().dim()
            };
            let label_style = if selected {
                Style::default().bold()
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(format!("{}. ", index + 1), Style::default().dim()),
                Span::styled(choice.label.clone(), label_style),
            ]));
            if !choice.description.trim().is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(choice.description.clone(), Style::default().dim()),
                ]));
            }
            if selected && choice.is_other {
                let text = self.display_text();
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        if text.is_empty() {
                            "Type answer...".to_string()
                        } else {
                            text
                        },
                        Style::default().fg(self.accent_color),
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from("↑/↓ choose · type for Other · Enter confirm · Esc cancel").dim());
        lines
    }
}

impl BottomPaneView for RequestUserInputOverlay {
    fn user_input_request_id(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            KeyCode::Down => {
                self.selected_index = self.selected_index.saturating_add(1);
                self.clamp_selection();
            }
            KeyCode::Char(c) if key_event.modifiers.is_empty() => {
                if let Some(digit) = c.to_digit(10) {
                    let index = digit.saturating_sub(1) as usize;
                    if index < self.choices().len() {
                        self.selected_index = index;
                        return;
                    }
                }
                if self.selected_choice_is_other() {
                    self.freeform_text.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.selected_choice_is_other() {
                    self.freeform_text.pop();
                }
            }
            KeyCode::Enter => {
                self.accept_current();
            }
            KeyCode::Esc => {
                self.cancel();
            }
            KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel();
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.cancel();
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }
}

impl Renderable for RequestUserInputOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let area = inset_request_user_input_area(area);
        Paragraph::new(self.lines())
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let width = width.saturating_sub(LIVE_PREFIX_COLS).max(1);
        Paragraph::new(self.lines())
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }
}

fn inset_request_user_input_area(area: Rect) -> Rect {
    let left = LIVE_PREFIX_COLS.min(area.width);
    Rect {
        x: area.x.saturating_add(left),
        y: area.y,
        width: area.width.saturating_sub(left),
        height: area.height,
    }
}

fn other_choice() -> QuestionChoice {
    QuestionChoice {
        label: "Other".to_string(),
        description: "Type a custom answer before confirming.".to_string(),
        is_other: true,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use devo_protocol::RequestUserInputOption;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn confirm_selected_option_sends_response() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let mut overlay = RequestUserInputOverlay::new(
            session_id,
            turn_id,
            "call-1".to_string(),
            vec![RequestUserInputQuestion {
                id: "scope".to_string(),
                header: "Scope".to_string(),
                question: "Which scope should the plan cover?".to_string(),
                is_other: true,
                is_secret: false,
                options: Some(vec![
                    RequestUserInputOption {
                        label: "Narrow".to_string(),
                        description: "Only the requested file.".to_string(),
                    },
                    RequestUserInputOption {
                        label: "Broad".to_string(),
                        description: "Include adjacent behavior.".to_string(),
                    },
                ]),
            }],
            AppEventSender::new(tx),
            Color::Cyan,
        );

        overlay.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        overlay.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let AppEvent::Command(AppCommand::RequestUserInputRespond {
            session_id: actual_session_id,
            turn_id: actual_turn_id,
            request_id,
            response,
        }) = rx.try_recv().expect("response event")
        else {
            panic!("expected request_user_input response command");
        };
        assert_eq!(actual_session_id, session_id);
        assert_eq!(actual_turn_id, turn_id);
        assert_eq!(request_id, "call-1");
        assert_eq!(
            response.answers.get("scope"),
            Some(&RequestUserInputAnswer {
                answers: vec!["Broad".to_string()],
            })
        );
        assert!(overlay.is_complete());
    }
}
