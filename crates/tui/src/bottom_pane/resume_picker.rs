//! Inline session picker shown in the bottom pane for `/resume`.
//!
//! The view owns search, workspace filtering, preview, rename, and delete
//! interaction state. Network operations leave through [`ResumePickerAction`]
//! and return through the refresh hooks on [`BottomPaneView`].

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use devo_core::SessionId;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::bottom_pane::BottomPaneView;
use crate::events::SessionListEntry;
use crate::events::SessionPreviewMessage;
use crate::events::SessionPreviewRole;
use crate::wrapping::word_wrap_lines;

mod interaction;
mod render;
#[cfg(test)]
mod tests;

use render::format_bytes;
use render::relative_time;
use render::truncate_display;

const VIEW_ID: &str = "resume_picker";
const DESIRED_HEIGHT: u16 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumePickerAction {
    Resume {
        session_id: SessionId,
    },
    Preview {
        session_id: SessionId,
    },
    Rename {
        session_id: SessionId,
        title: String,
    },
    Delete {
        session_id: SessionId,
        is_active: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreviewState {
    Loading,
    Loaded(Vec<SessionPreviewMessage>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditMode {
    Browse,
    Rename {
        session_id: SessionId,
        text: String,
        pending: bool,
        error: Option<String>,
    },
    Delete {
        session_id: SessionId,
        confirm_delete: bool,
        pending: bool,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoadState {
    Loading,
    Ready,
    Failed(String),
}

pub(crate) struct ResumePickerView {
    current_cwd: PathBuf,
    sessions: Vec<SessionListEntry>,
    load_state: LoadState,
    show_all_projects: bool,
    search_query: String,
    selected_session_id: Option<SessionId>,
    expanded_preview: Option<SessionId>,
    previews: HashMap<SessionId, PreviewState>,
    edit_mode: EditMode,
    pending_action: Option<ResumePickerAction>,
    complete: bool,
    accent_color: Color,
    scroll_offset: Cell<usize>,
}

impl ResumePickerView {
    pub(crate) fn loading(current_cwd: PathBuf, accent_color: Color) -> Self {
        Self {
            current_cwd,
            sessions: Vec::new(),
            load_state: LoadState::Loading,
            show_all_projects: false,
            search_query: String::new(),
            selected_session_id: None,
            expanded_preview: None,
            previews: HashMap::new(),
            edit_mode: EditMode::Browse,
            pending_action: None,
            complete: false,
            accent_color,
            scroll_offset: Cell::new(0),
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.search_query.to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| {
                (self.show_all_projects || session.cwd == self.current_cwd)
                    && (query.is_empty()
                        || session.title.to_lowercase().contains(&query)
                        || session.preview.to_lowercase().contains(&query)
                        || session
                            .cwd
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query)
                        || session
                            .branch
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&query))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_entry(&self) -> Option<&SessionListEntry> {
        let selected = self.selected_session_id?;
        self.sessions
            .iter()
            .find(|session| session.session_id == selected)
    }

    fn normalize_selection(&mut self) {
        let filtered = self.filtered_indices();
        let still_visible = self.selected_session_id.is_some_and(|selected| {
            filtered
                .iter()
                .any(|index| self.sessions[*index].session_id == selected)
        });
        if !still_visible {
            self.selected_session_id = filtered
                .first()
                .map(|index| self.sessions[*index].session_id);
        }
        self.expanded_preview = self
            .expanded_preview
            .filter(|expanded| Some(*expanded) == self.selected_session_id);
        self.scroll_offset.set(0);
    }

    fn list_lines(&self, width: usize) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
        let filtered = self.filtered_indices();
        let mut lines = Vec::new();
        if !self.show_all_projects {
            let workspace = self
                .current_cwd
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| self.current_cwd.display().to_string());
            lines.push(Line::from(truncate_display(&workspace, width)).dim());
            lines.push(Line::from(""));
        }
        if filtered.is_empty() {
            lines.push(Line::from("  No saved sessions found.").dim());
            return (lines, None);
        }
        let mut selected_range = None;
        for index in filtered {
            let session = &self.sessions[index];
            let selected = Some(session.session_id) == self.selected_session_id;
            let start = lines.len();
            let marker = if selected { "❯" } else { " " };
            let title = truncate_display(&session.title, width.saturating_sub(2));
            let title_line = Line::from(format!("{marker} {title}"));
            lines.push(if selected {
                title_line.style(Style::default().fg(self.accent_color).bold())
            } else {
                title_line
            });
            let mut metadata = format!(
                "  {} · {}",
                relative_time(session.last_activity_at),
                format_bytes(session.transcript_size_bytes)
            );
            if self.show_all_projects {
                metadata.push_str(" · ");
                metadata.push_str(&session.cwd.display().to_string());
            }
            lines.push(Line::from(truncate_display(&metadata, width)).dim());
            if self.expanded_preview == Some(session.session_id) {
                self.extend_preview_lines(&mut lines, session.session_id, width);
            }
            lines.push(Line::from(""));
            if selected {
                selected_range = Some((start, lines.len()));
            }
        }
        (lines, selected_range)
    }

    fn extend_preview_lines(
        &self,
        lines: &mut Vec<Line<'static>>,
        session_id: SessionId,
        width: usize,
    ) {
        let indent = "    ";
        match self.previews.get(&session_id) {
            Some(PreviewState::Loading) | None => {
                lines.push(Line::from(format!("{indent}Loading preview…")).dim());
            }
            Some(PreviewState::Failed(message)) => {
                lines.push(
                    Line::from(format!(
                        "{indent}Preview failed: {}",
                        truncate_display(message, width.saturating_sub(indent.len() + 16))
                    ))
                    .red(),
                );
            }
            Some(PreviewState::Loaded(messages)) if messages.is_empty() => {
                lines.push(Line::from(format!("{indent}No conversation content.")).dim());
            }
            Some(PreviewState::Loaded(messages)) => {
                let wrap_width = width.saturating_sub(indent.len() + 4).max(1);
                for message in messages {
                    let role = match message.role {
                        SessionPreviewRole::User => "You",
                        SessionPreviewRole::Assistant => "Devo",
                    };
                    let wrapped =
                        word_wrap_lines([Line::from(message.text.replace('\n', " "))], wrap_width);
                    for (line_index, line) in wrapped.into_iter().take(2).enumerate() {
                        let prefix = if line_index == 0 {
                            format!("{indent}{role}: ")
                        } else {
                            format!("{indent}{}", " ".repeat(role.len() + 2))
                        };
                        let mut spans = vec![Span::raw(prefix).dim()];
                        spans.extend(line.spans);
                        lines.push(Line::from(spans));
                    }
                }
            }
        }
    }

    fn input_text(&self) -> (&str, &str, Option<&str>) {
        match &self.edit_mode {
            EditMode::Browse => ("Search", &self.search_query, None),
            EditMode::Rename {
                text,
                pending,
                error,
                ..
            } => (
                if *pending {
                    "Renaming"
                } else {
                    "Rename session"
                },
                text,
                error.as_deref(),
            ),
            EditMode::Delete { error, .. } => ("Search", &self.search_query, error.as_deref()),
        }
    }

    fn footer_line(&self) -> Line<'static> {
        match &self.edit_mode {
            EditMode::Rename { pending: true, .. } => Line::from("Renaming… · Esc to cancel").dim(),
            EditMode::Rename { .. } => Line::from("Enter to rename · Esc to cancel").dim(),
            EditMode::Delete {
                confirm_delete,
                pending,
                ..
            } => {
                if *pending {
                    return Line::from("Deleting… · Esc to cancel").dim();
                }
                let cancel = if *confirm_delete { "Cancel".dim() } else { "Cancel".bold() };
                let delete = if *confirm_delete { "Delete".bold().red() } else { "Delete".dim() };
                Line::from(vec![
                    Span::raw("←/→ choose · Enter confirm · Esc cancel  ["),
                    cancel,
                    Span::raw("] ["),
                    delete,
                    Span::raw("]"),
                ])
            }
            EditMode::Browse => Line::from(if self.show_all_projects {
                "Ctrl+A current workspace · Space preview · Ctrl+R rename · Ctrl+D delete · Esc cancel"
            } else {
                "Ctrl+A all projects · Space preview · Ctrl+R rename · Ctrl+D delete · Esc cancel"
            })
            .dim(),
        }
    }
}

impl BottomPaneView for ResumePickerView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if !matches!(self.load_state, LoadState::Ready) {
            if key_event.code == KeyCode::Esc {
                self.complete = true;
            }
            return;
        }
        match self.edit_mode {
            EditMode::Browse => self.handle_browse_key(key_event),
            EditMode::Rename { .. } => self.handle_rename_key(key_event),
            EditMode::Delete { .. } => self.handle_delete_key(key_event),
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(VIEW_ID)
    }

    fn take_resume_action(&mut self) -> Option<ResumePickerAction> {
        self.pending_action.take()
    }

    fn update_resume_sessions(&mut self, sessions: Vec<SessionListEntry>) -> bool {
        self.sessions = sessions;
        self.load_state = LoadState::Ready;
        self.selected_session_id = self
            .sessions
            .iter()
            .find(|session| session.is_active && session.cwd == self.current_cwd)
            .map(|session| session.session_id);
        self.normalize_selection();
        true
    }

    fn update_resume_list_error(&mut self, message: String) -> bool {
        self.load_state = LoadState::Failed(message);
        true
    }

    fn update_resume_preview(
        &mut self,
        session_id: SessionId,
        result: Result<Vec<SessionPreviewMessage>, String>,
    ) -> bool {
        if !self.previews.contains_key(&session_id) {
            return false;
        }
        self.previews.insert(
            session_id,
            match result {
                Ok(messages) => PreviewState::Loaded(messages),
                Err(message) => PreviewState::Failed(message),
            },
        );
        true
    }

    fn update_resume_rename(
        &mut self,
        session_id: Option<SessionId>,
        result: Result<String, String>,
    ) -> bool {
        let EditMode::Rename {
            session_id: editing_id,
            pending,
            error,
            ..
        } = &mut self.edit_mode
        else {
            return false;
        };
        if session_id.is_some_and(|id| id != *editing_id) {
            return false;
        }
        match result {
            Ok(title) => {
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == *editing_id)
                {
                    session.title = title;
                }
                self.edit_mode = EditMode::Browse;
            }
            Err(message) => {
                *pending = false;
                *error = Some(message);
            }
        }
        true
    }

    fn update_resume_delete(
        &mut self,
        session_id: Option<SessionId>,
        result: Result<(), String>,
    ) -> bool {
        let EditMode::Delete {
            session_id: deleting_id,
            pending,
            error,
            ..
        } = &mut self.edit_mode
        else {
            return false;
        };
        if session_id.is_some_and(|id| id != *deleting_id) {
            return false;
        }
        match result {
            Ok(()) => {
                self.sessions
                    .retain(|session| session.session_id != *deleting_id);
                self.edit_mode = EditMode::Browse;
                self.normalize_selection();
            }
            Err(message) => {
                *pending = false;
                *error = Some(message);
            }
        }
        true
    }

    fn set_accent_color(&mut self, color: Color) {
        self.accent_color = color;
    }

    fn replaces_composer(&self) -> bool {
        true
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }

    #[cfg(test)]
    fn resume_selection_for_test(&self) -> Option<usize> {
        let selected = self.selected_session_id?;
        self.filtered_indices()
            .iter()
            .position(|index| self.sessions[*index].session_id == selected)
    }

    #[cfg(test)]
    fn resume_scroll_offset_for_test(&self) -> Option<usize> {
        Some(self.scroll_offset.get())
    }

    #[cfg(test)]
    fn resume_pending_delete_for_test(&self) -> Option<SessionId> {
        match self.edit_mode {
            EditMode::Delete { session_id, .. } => Some(session_id),
            EditMode::Browse | EditMode::Rename { .. } => None,
        }
    }
}
