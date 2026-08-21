use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::EditMode;
use super::PreviewState;
use super::ResumePickerAction;
use super::ResumePickerView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionMovement {
    Previous,
    Next,
    PagePrevious,
    PageNext,
    First,
    Last,
}

impl ResumePickerView {
    fn move_selection(&mut self, movement: SelectionMovement) {
        let filtered = self.filtered_indices();
        if filtered.is_empty() {
            self.selected_session_id = None;
            return;
        }
        let current = self
            .selected_session_id
            .and_then(|selected| {
                filtered
                    .iter()
                    .position(|index| self.sessions[*index].session_id == selected)
            })
            .unwrap_or(0);
        let next = match movement {
            SelectionMovement::Previous => current.saturating_sub(1),
            SelectionMovement::Next => current.saturating_add(1).min(filtered.len() - 1),
            SelectionMovement::PagePrevious => current.saturating_sub(5),
            SelectionMovement::PageNext => current.saturating_add(5).min(filtered.len() - 1),
            SelectionMovement::First => 0,
            SelectionMovement::Last => filtered.len() - 1,
        };
        self.selected_session_id = Some(self.sessions[filtered[next]].session_id);
        self.expanded_preview = None;
    }

    pub(super) fn handle_browse_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') => {
                    self.show_all_projects = !self.show_all_projects;
                    self.normalize_selection();
                }
                KeyCode::Char('r') => {
                    if let Some(session) = self.selected_entry() {
                        self.edit_mode = EditMode::Rename {
                            session_id: session.session_id,
                            text: session.title.clone(),
                            pending: false,
                            error: None,
                        };
                    }
                }
                KeyCode::Char('d') => {
                    if let Some(session) = self.selected_entry() {
                        self.edit_mode = EditMode::Delete {
                            session_id: session.session_id,
                            confirm_delete: false,
                            pending: false,
                            error: None,
                        };
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.search_query.is_empty() {
                    self.complete = true;
                } else {
                    self.search_query.clear();
                    self.normalize_selection();
                }
            }
            KeyCode::Up => self.move_selection(SelectionMovement::Previous),
            KeyCode::Down => self.move_selection(SelectionMovement::Next),
            KeyCode::PageUp => self.move_selection(SelectionMovement::PagePrevious),
            KeyCode::PageDown => self.move_selection(SelectionMovement::PageNext),
            KeyCode::Home => self.move_selection(SelectionMovement::First),
            KeyCode::End => self.move_selection(SelectionMovement::Last),
            KeyCode::Enter => {
                if let Some(session_id) = self.selected_session_id {
                    self.pending_action = Some(ResumePickerAction::Resume { session_id });
                    self.complete = true;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(session_id) = self.selected_session_id {
                    if self.expanded_preview == Some(session_id) {
                        self.expanded_preview = None;
                    } else {
                        self.expanded_preview = Some(session_id);
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            self.previews.entry(session_id)
                        {
                            entry.insert(PreviewState::Loading);
                            self.pending_action = Some(ResumePickerAction::Preview { session_id });
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.normalize_selection();
            }
            KeyCode::Char(ch) if !ch.is_control() => {
                self.search_query.push(ch);
                self.normalize_selection();
            }
            _ => {}
        }
    }

    pub(super) fn handle_rename_key(&mut self, key: KeyEvent) {
        let EditMode::Rename {
            session_id,
            text,
            pending,
            error,
        } = &mut self.edit_mode
        else {
            return;
        };
        if *pending {
            if key.code == KeyCode::Esc {
                self.edit_mode = EditMode::Browse;
            }
            return;
        }
        match key.code {
            KeyCode::Esc => self.edit_mode = EditMode::Browse,
            KeyCode::Backspace => {
                text.pop();
                *error = None;
            }
            KeyCode::Enter => {
                let title = text.trim().to_string();
                if title.is_empty() {
                    *error = Some("Title cannot be empty".to_string());
                } else {
                    *pending = true;
                    *error = None;
                    self.pending_action = Some(ResumePickerAction::Rename {
                        session_id: *session_id,
                        title,
                    });
                }
            }
            KeyCode::Char(ch)
                if !ch.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                text.push(ch);
                *error = None;
            }
            _ => {}
        }
    }

    pub(super) fn handle_delete_key(&mut self, key: KeyEvent) {
        let EditMode::Delete {
            session_id,
            confirm_delete,
            pending,
            error,
        } = &mut self.edit_mode
        else {
            return;
        };
        if *pending {
            if key.code == KeyCode::Esc {
                self.edit_mode = EditMode::Browse;
            }
            return;
        }
        match key.code {
            KeyCode::Esc => self.edit_mode = EditMode::Browse,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                *confirm_delete = !*confirm_delete;
                *error = None;
            }
            KeyCode::Enter if *confirm_delete => {
                let is_active = self
                    .sessions
                    .iter()
                    .find(|session| session.session_id == *session_id)
                    .is_some_and(|session| session.is_active);
                *pending = true;
                *error = None;
                self.pending_action = Some(ResumePickerAction::Delete {
                    session_id: *session_id,
                    is_active,
                });
            }
            KeyCode::Enter => self.edit_mode = EditMode::Browse,
            _ => {}
        }
    }
}
