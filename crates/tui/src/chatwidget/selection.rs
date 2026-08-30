//! User-turn selection mode for `ChatWidget`.
//!
//! Selection mode lets the user choose an earlier prompt and open rollback,
//! fork, or cancel actions without mixing that state into normal input flow.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::history_cell;
use crate::history_cell::HistoryCell;

use super::ChatWidget;

impl ChatWidget {
    pub(super) fn handle_selection_mode_key(&mut self, key: KeyEvent) -> bool {
        let alt_up = key.code == KeyCode::Up && key.modifiers.contains(KeyModifiers::ALT);
        let alt_down = key.code == KeyCode::Down && key.modifiers.contains(KeyModifiers::ALT);

        if !alt_up && !alt_down {
            if !self.selection_mode {
                return false;
            }
            match key.code {
                KeyCode::Esc => {
                    self.exit_selection_mode();
                    return true;
                }
                KeyCode::Enter => {
                    self.open_selection_action_menu();
                    return true;
                }
                _ => return false,
            }
        }

        if self.busy {
            return false;
        }

        self.refresh_user_cell_indices();
        let len = self.user_cell_history_indices.len();
        if len == 0 {
            return false;
        }

        if !self.selection_mode {
            self.selection_mode = true;
            // Start from the last user cell if no selection yet
            self.selected_user_cell_index = Some(len - 1);
            self.sync_selected_user_cell_highlight();
            self.update_selection_status();
            self.frame_requester.schedule_frame();
            return true;
        }

        let current = self.selected_user_cell_index.unwrap_or(0);
        let new = if alt_up {
            current.saturating_sub(1)
        } else {
            (current + 1).min(len - 1)
        };
        if new != current {
            self.selected_user_cell_index = Some(new);
            self.sync_selected_user_cell_highlight();
            self.update_selection_status();
            self.frame_requester.schedule_frame();
        }
        true
    }

    pub(super) fn exit_selection_mode(&mut self) {
        self.selection_mode = false;
        self.selected_user_cell_index = None;
        self.sync_selected_user_cell_highlight();
        self.sync_bottom_pane_summary();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn update_selection_status(&mut self) {
        if let Some(idx) = self.selected_user_cell_index {
            let turn_num = idx + 1;
            self.bottom_pane.set_status_line(Some(
                Line::from(format!(
                    "Selected turn {turn_num} · Enter to act  Esc to cancel"
                ))
                .dim(),
            ));
            self.bottom_pane.set_status_line_enabled(true);
        }
    }

    pub(super) fn refresh_user_cell_indices(&mut self) {
        self.user_cell_history_indices = self
            .history
            .iter()
            .enumerate()
            .filter_map(|(i, cell)| {
                let cell_ref: &dyn HistoryCell = cell.as_ref();
                cell_ref
                    .as_any()
                    .downcast_ref::<history_cell::UserHistoryCell>()
                    .map(|_| i)
            })
            .collect();
    }

    pub(super) fn sync_selected_user_cell_highlight(&mut self) {
        for (history_idx, cell) in self.history.iter_mut().enumerate() {
            let Some(user_cell) = cell
                .as_mut()
                .as_any_mut()
                .downcast_mut::<history_cell::UserHistoryCell>()
            else {
                continue;
            };
            let is_selected = self.selection_mode
                && self
                    .selected_user_cell_index
                    .and_then(|selected_idx| self.user_cell_history_indices.get(selected_idx))
                    .is_some_and(|selected_history_idx| *selected_history_idx == history_idx);
            user_cell.selected = is_selected;
        }
    }

    pub(super) fn open_selection_action_menu(&mut self) {
        if !self.selection_mode {
            return;
        }
        let Some(selected_idx) = self.selected_user_cell_index else {
            self.exit_selection_mode();
            return;
        };
        let Some(history_idx) = self.user_cell_history_indices.get(selected_idx).copied() else {
            self.exit_selection_mode();
            return;
        };
        let Some(user_cell) = self.history.get(history_idx).and_then(|cell| {
            cell.as_ref()
                .as_any()
                .downcast_ref::<history_cell::UserHistoryCell>()
        }) else {
            self.exit_selection_mode();
            return;
        };

        let is_latest_user_turn = selected_idx + 1 == self.user_cell_history_indices.len();
        let selected_turn_index = u32::try_from(selected_idx).unwrap_or(u32::MAX);
        let selected_text = user_cell.message.clone();
        let mut items = Vec::new();

        items.push(SelectionItem {
            name: "Rollback".to_string(),
            description: None,
            selected_description: None,
            is_current: false,
            is_default: false,
            is_disabled: is_latest_user_turn,
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::Command(AppCommand::rollback_to_user_turn(
                    selected_turn_index,
                )));
            })],
            dismiss_on_select: true,
            search_value: None,
            disabled_reason: is_latest_user_turn
                .then_some("Latest user turn cannot be rolled back".to_string()),
            ..SelectionItem::default()
        });

        let fork_turn_index = selected_turn_index;
        items.push(SelectionItem {
            name: "Fork".to_string(),
            description: None,
            selected_description: None,
            is_current: false,
            is_default: false,
            is_disabled: is_latest_user_turn,
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::Command(AppCommand::fork_at_user_turn(
                    fork_turn_index,
                    devo_protocol::native::rpc_session::SessionForkCut::Through,
                )));
            })],
            dismiss_on_select: true,
            search_value: None,
            disabled_reason: is_latest_user_turn
                .then_some("Latest user turn cannot be forked".to_string()),
            ..SelectionItem::default()
        });

        let edit_turn_index = selected_turn_index;
        items.push(SelectionItem {
            name: "Edit".to_string(),
            description: Some(
                "Fork before this turn and keep the text in the composer".to_string(),
            ),
            selected_description: None,
            is_current: false,
            is_default: false,
            is_disabled: is_latest_user_turn,
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::Command(AppCommand::fork_at_user_turn(
                    edit_turn_index,
                    devo_protocol::native::rpc_session::SessionForkCut::Before,
                )));
            })],
            dismiss_on_select: true,
            search_value: None,
            disabled_reason: is_latest_user_turn
                .then_some("Edit the latest user turn from the composer instead".to_string()),
            ..SelectionItem::default()
        });

        items.push(SelectionItem {
            name: "Cancel".to_string(),
            description: None,
            selected_description: None,
            is_current: false,
            is_default: false,
            is_disabled: false,
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::StatusMessageChanged {
                    message: "Selection cancelled".to_string(),
                });
            })],
            dismiss_on_select: true,
            search_value: None,
            disabled_reason: None,
            ..SelectionItem::default()
        });

        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                SelectionViewParams {
                    items,
                    ..SelectionViewParams::default()
                },
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.bottom_pane
            .restore_input_from_history(Some(selected_text));
        self.set_status_message("Select an action");
    }
}
