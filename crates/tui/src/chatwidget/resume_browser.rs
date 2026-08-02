//! Resume-session browser rendering and navigation for `ChatWidget`.
//!
//! The chat widget owns resume-browser state while this module keeps the
//! popup-style list rendering and key handling separate from the main surface.

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
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::HorizontalChipStrip;
use crate::events::SessionListEntry;
use crate::key_hint;
use crate::ui_consts::FOOTER_INDENT_COLS;
use devo_core::SessionId;

use super::ChatWidget;

const DELETE_CONFIRM_CANCEL: usize = 0;
const DELETE_CONFIRM_DELETE: usize = 1;

#[derive(Debug, Clone)]
pub(super) struct ResumeBrowserState {
    pub(super) sessions: Vec<SessionListEntry>,
    pub(super) selection: usize,
    pub(super) scroll_offset: usize,
    /// When set, the footer shows Cancel/Delete chips for this session.
    pub(super) pending_delete_session_id: Option<SessionId>,
    /// Chip selection for the pending delete confirm (default Cancel).
    pub(super) pending_delete_chips: HorizontalChipStrip,
}

impl ChatWidget {
    pub(super) fn open_resume_browser(&mut self, sessions: Vec<SessionListEntry>) {
        self.resume_browser_loading = false;
        let selection = sessions
            .iter()
            .position(|session| session.is_active)
            .unwrap_or(0);
        self.resume_browser = Some(ResumeBrowserState {
            sessions,
            selection,
            scroll_offset: 0,
            pending_delete_session_id: None,
            pending_delete_chips: Self::new_delete_confirm_chips(),
        });
        self.set_status_message("Resume session");
    }

    fn new_delete_confirm_chips() -> HorizontalChipStrip {
        HorizontalChipStrip::new(
            vec!["Cancel".to_string(), "Delete".to_string()],
            /*selected*/ DELETE_CONFIRM_CANCEL,
        )
    }

    pub(super) fn handle_resume_browser_key_event(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press) {
            return;
        }
        let Some(browser) = self.resume_browser.as_mut() else {
            return;
        };
        let confirming_delete = browser.pending_delete_session_id.is_some();
        let page_step = Self::resume_browser_visible_capacity(
            self.resume_browser_last_height.get(),
            !browser.sessions.is_empty(),
            confirming_delete,
        )
        .max(1);

        if confirming_delete {
            match key.code {
                KeyCode::Esc => {
                    browser.pending_delete_session_id = None;
                    browser.pending_delete_chips = Self::new_delete_confirm_chips();
                    self.set_status_message("Resume session");
                    self.frame_requester.schedule_frame();
                }
                KeyCode::Char('q') => {
                    self.resume_browser = None;
                    self.resume_browser_loading = false;
                    self.set_status_message("Ready");
                    self.frame_requester.schedule_frame();
                }
                KeyCode::Left => {
                    browser.pending_delete_chips.move_left();
                    self.frame_requester.schedule_frame();
                }
                KeyCode::Right => {
                    browser.pending_delete_chips.move_right();
                    self.frame_requester.schedule_frame();
                }
                KeyCode::Enter => {
                    let selected = browser.pending_delete_chips.selected_index();
                    if selected != DELETE_CONFIRM_DELETE {
                        browser.pending_delete_session_id = None;
                        browser.pending_delete_chips = Self::new_delete_confirm_chips();
                        self.set_status_message("Resume session");
                        self.frame_requester.schedule_frame();
                        return;
                    }
                    let Some(session_id) = browser.pending_delete_session_id.take() else {
                        return;
                    };
                    browser.pending_delete_chips = Self::new_delete_confirm_chips();
                    let deleting_active = browser
                        .sessions
                        .iter()
                        .find(|session| session.session_id == session_id)
                        .is_some_and(|session| session.is_active);
                    if deleting_active {
                        self.resume_browser = None;
                        self.clear_for_session_switch();
                    }
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::delete_session_by_id(
                            session_id,
                        )));
                    self.set_status_message("Deleting session");
                    self.frame_requester.schedule_frame();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.resume_browser = None;
                self.resume_browser_loading = false;
                self.set_status_message("Ready");
                self.frame_requester.schedule_frame();
            }
            KeyCode::Up => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else if browser.selection > 0 {
                    browser.selection -= 1;
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Down => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else if browser.selection + 1 < browser.sessions.len() {
                    browser.selection += 1;
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::PageUp => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser.selection.saturating_sub(page_step);
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::PageDown => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser
                        .selection
                        .saturating_add(page_step)
                        .min(browser.sessions.len().saturating_sub(1));
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Home => {
                browser.selection = 0;
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::End => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser.sessions.len().saturating_sub(1);
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(selected) = browser.sessions.get(browser.selection) {
                    browser.pending_delete_session_id = Some(selected.session_id);
                    browser.pending_delete_chips = Self::new_delete_confirm_chips();
                    self.set_status_message("Confirm session delete");
                    self.frame_requester.schedule_frame();
                }
            }
            KeyCode::Enter => {
                if let Some(selected) = browser.sessions.get(browser.selection) {
                    let session_id = selected.session_id;
                    self.resume_browser = None;
                    self.clear_for_session_switch();
                    self.begin_session_resume();
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::switch_session(session_id)));
                }
            }
            KeyCode::Backspace
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char(_)
            | KeyCode::F(_)
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Insert
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => {}
        }
    }

    pub(crate) fn is_resume_browser_open(&self) -> bool {
        self.resume_browser_loading || self.resume_browser.is_some()
    }

    /// Drop a deleted session from an open resume browser, if present.
    pub(super) fn remove_session_from_resume_browser(&mut self, session_id: &str) {
        let Some(browser) = self.resume_browser.as_mut() else {
            return;
        };
        let before = browser.sessions.len();
        browser
            .sessions
            .retain(|session| session.session_id.to_string() != session_id);
        if browser.sessions.len() == before {
            return;
        }
        browser.pending_delete_session_id = None;
        browser.pending_delete_chips = Self::new_delete_confirm_chips();
        if browser.sessions.is_empty() {
            browser.selection = 0;
            browser.scroll_offset = 0;
        } else {
            browser.selection = browser
                .selection
                .min(browser.sessions.len().saturating_sub(1));
            self.ensure_resume_selection_visible(u16::MAX);
        }
        self.frame_requester.schedule_frame();
    }

    fn resume_browser_entry_height() -> usize {
        1
    }

    fn resume_browser_chrome_height(has_sessions: bool, confirming_delete: bool) -> usize {
        let base: usize = if has_sessions { 7 } else { 6 };
        // Pending delete adds a chip row between the prompt and the hint.
        if confirming_delete {
            base.saturating_add(1)
        } else {
            base
        }
    }

    fn resume_browser_visible_capacity(
        area_height: u16,
        has_sessions: bool,
        confirming_delete: bool,
    ) -> usize {
        area_height
            .saturating_sub(
                Self::resume_browser_chrome_height(has_sessions, confirming_delete) as u16,
            ) as usize
    }

    fn resume_browser_window(
        sessions_len: usize,
        selection: usize,
        requested_offset: usize,
        area_height: u16,
        confirming_delete: bool,
    ) -> (usize, usize, bool, bool) {
        if sessions_len == 0 {
            return (0, 0, false, false);
        }
        let list_window =
            Self::resume_browser_visible_capacity(area_height, true, confirming_delete);
        if list_window == 0 {
            return (selection.min(sessions_len.saturating_sub(1)), 0, true, true);
        }

        let selection = selection.min(sessions_len.saturating_sub(1));
        let mut start = requested_offset.min(sessions_len.saturating_sub(1));
        let mut slots = list_window;

        loop {
            if slots == 0 {
                return (selection, 0, start > 0, selection + 1 < sessions_len);
            }
            let end = (start + slots).min(sessions_len);
            let has_above = start > 0;
            let has_below = end < sessions_len;
            let indicator_rows = usize::from(has_above) + usize::from(has_below);
            let session_slots = list_window.saturating_sub(indicator_rows);
            if session_slots == slots {
                let end = (start + session_slots).min(sessions_len);
                let has_above = start > 0;
                let has_below = end < sessions_len;
                return (start, end, has_above, has_below);
            }
            slots = session_slots;
            if selection < start {
                start = selection;
            } else if selection >= start + slots {
                start = selection + 1 - slots;
            }
            start = start.min(sessions_len.saturating_sub(slots.max(1)));
        }
    }

    fn resume_browser_delete_footer_hint() -> Line<'static> {
        Line::from(vec![
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

    fn pad_resume_line(line: Line<'static>) -> Line<'static> {
        let mut spans = vec![Span::raw(" ".repeat(FOOTER_INDENT_COLS))];
        spans.extend(line.spans);
        Line::from(spans)
    }

    fn resume_browser_footer_lines(
        has_sessions: bool,
        pending_delete: Option<(&str, &HorizontalChipStrip)>,
        accent: Color,
        chip_width: usize,
    ) -> Vec<Line<'static>> {
        if let Some((title, chips)) = pending_delete {
            let available = chip_width.saturating_sub(FOOTER_INDENT_COLS);
            return vec![
                Self::pad_resume_line(Line::from(format!("Delete \"{title}\"?").bold())),
                Self::pad_resume_line(chips.render_line(available, accent)),
                Self::pad_resume_line(Self::resume_browser_delete_footer_hint()),
            ];
        }
        if has_sessions {
            vec![
                Self::pad_resume_line(Line::from(
                    "↑/↓ select  pgup/pgdn page  home/end jump".dim(),
                )),
                Self::pad_resume_line(Line::from("enter resume  d delete  q back".dim())),
            ]
        } else {
            vec![Self::pad_resume_line(Line::from("q back".dim()))]
        }
    }

    fn resume_browser_progress_label(
        selection: usize,
        sessions_len: usize,
        rendered_start: usize,
        area_height: u16,
        confirming_delete: bool,
    ) -> String {
        if sessions_len == 0 {
            return " 0 / 0 · 100% ".to_string();
        }
        let position = selection.saturating_add(1);
        let total = sessions_len;
        let capacity = Self::resume_browser_visible_capacity(area_height, true, confirming_delete);
        let max_scroll = sessions_len.saturating_sub(capacity.max(1));
        let percent = if max_scroll == 0 {
            100
        } else {
            ((rendered_start.min(max_scroll) as f32 / max_scroll as f32) * 100.0).round() as usize
        };
        format!(" {position} / {total} · {percent}% ")
    }

    fn ensure_resume_selection_visible(&mut self, area_height: u16) {
        let Some(browser) = self.resume_browser.as_mut() else {
            return;
        };
        if browser.sessions.is_empty() {
            browser.selection = 0;
            browser.scroll_offset = 0;
            return;
        }
        browser.selection = browser
            .selection
            .min(browser.sessions.len().saturating_sub(1));
        let confirming_delete = browser.pending_delete_session_id.is_some();
        let capacity = Self::resume_browser_visible_capacity(area_height, true, confirming_delete);
        if capacity == 0 {
            browser.scroll_offset = browser.selection;
            return;
        }
        let selection = browser.selection;
        let mut offset = browser.scroll_offset;
        if selection < offset {
            offset = selection;
        } else {
            let selection_bottom = selection + Self::resume_browser_entry_height();
            let visible_end = offset + capacity;
            if selection_bottom > visible_end {
                offset = selection_bottom.saturating_sub(capacity);
            }
        }
        let max_offset = browser.sessions.len().saturating_sub(capacity.max(1));
        browser.scroll_offset = offset.min(max_offset);
    }

    pub(super) fn render_resume_browser_if_open(&self, area: Rect, buf: &mut Buffer) -> bool {
        if self.resume_browser_loading {
            let lines = vec![
                Self::pad_resume_line(Line::from("Resume Session".bold())),
                Self::pad_resume_line(Line::from("Loading saved sessions...".dim())),
                Line::from(""),
                Self::pad_resume_line(Line::from("Please wait.".dim())),
            ];
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .render(area, buf);
            return true;
        }

        let Some(browser) = &self.resume_browser else {
            return false;
        };

        self.resume_browser_last_height.set(area.height);
        Block::default().style(Style::default()).render(area, buf);
        let confirming_delete = browser.pending_delete_session_id.is_some();
        let (scroll_offset, end, has_above, has_below) = Self::resume_browser_window(
            browser.sessions.len(),
            browser.selection,
            browser.scroll_offset,
            area.height,
            confirming_delete,
        );
        let title_width = browser
            .sessions
            .iter()
            .map(|session| unicode_width::UnicodeWidthStr::width(session.title.as_str()))
            .max()
            .unwrap_or(5)
            .clamp(5, 48);
        let progress = Self::resume_browser_progress_label(
            browser.selection,
            browser.sessions.len(),
            scroll_offset,
            area.height,
            confirming_delete,
        );
        let pending_delete = browser.pending_delete_session_id.and_then(|session_id| {
            browser
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .map(|session| (session.title.as_str(), &browser.pending_delete_chips))
        });
        let mut lines = vec![Self::pad_resume_line(Line::from(vec![
            Span::styled("Resume Session", Style::default().bold()),
            Span::raw(" "),
            Span::styled(progress, Style::default().dim()),
        ]))];
        if browser.sessions.is_empty() {
            lines.push(Self::pad_resume_line(Line::from(
                "No saved sessions found.".dim(),
            )));
        } else {
            // Indent column headers under the title text (after marker + space).
            let col_pad = " ".repeat(2);
            lines.push(Self::pad_resume_line(
                Line::from(format!(
                    "{col_pad}{:title_width$}  {:<36}  {}",
                    "Title",
                    "Session ID",
                    "Updated",
                    title_width = title_width
                ))
                .dim(),
            ));
            lines.push(Self::pad_resume_line(
                Line::from(format!(
                    "{col_pad}{}  {}  {}",
                    "-".repeat(title_width),
                    "-".repeat(36),
                    "-".repeat(23)
                ))
                .dim(),
            ));
            if has_above {
                lines.push(Self::pad_resume_line(Line::from(
                    format!("{col_pad}↑ more").dim(),
                )));
            }
            for (index, session) in browser
                .sessions
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(end.saturating_sub(scroll_offset))
            {
                let is_selected = index == browser.selection;
                let marker = if session.is_active {
                    "●"
                } else if is_selected {
                    ">"
                } else {
                    " "
                };
                let display_title = Self::pad_display_text(
                    &Self::truncate_display_text(&session.title, title_width),
                    title_width,
                );
                let line = format!(
                    "{marker} {}  {:<16}  {}",
                    display_title, session.session_id, session.updated_at
                );
                lines.push(Self::pad_resume_line(if is_selected {
                    Line::from(line).bold()
                } else if session.is_active {
                    Line::from(line).style(Style::default().fg(self.active_accent_color()))
                } else {
                    Line::from(line)
                }));
            }
            if has_below {
                lines.push(Self::pad_resume_line(Line::from(
                    format!("{col_pad}↓ more").dim(),
                )));
            }
        }
        lines.extend(Self::resume_browser_footer_lines(
            !browser.sessions.is_empty(),
            pending_delete,
            self.active_accent_color(),
            usize::from(area.width.saturating_sub(2)),
        ));
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .render(area, buf);
        true
    }

    #[cfg(test)]
    pub(crate) fn resume_browser_selection_for_test(&self) -> Option<usize> {
        self.resume_browser
            .as_ref()
            .map(|browser| browser.selection)
    }

    #[cfg(test)]
    pub(crate) fn resume_browser_scroll_offset_for_test(&self) -> Option<usize> {
        self.resume_browser
            .as_ref()
            .map(|browser| browser.scroll_offset)
    }

    #[cfg(test)]
    pub(crate) fn open_resume_browser_for_test(&mut self, sessions: Vec<SessionListEntry>) {
        self.open_resume_browser(sessions);
    }

    #[cfg(test)]
    pub(crate) fn resume_browser_pending_delete_for_test(&self) -> Option<SessionId> {
        self.resume_browser
            .as_ref()
            .and_then(|browser| browser.pending_delete_session_id)
    }
}
