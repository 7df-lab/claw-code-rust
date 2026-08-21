use std::path::PathBuf;

use chrono::Utc;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use devo_core::SessionId;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::bottom_pane::BottomPaneView;
use crate::events::SessionListEntry;
use crate::events::SessionPreviewMessage;
use crate::events::SessionPreviewRole;
use crate::render::renderable::Renderable;

use super::PreviewState;
use super::ResumePickerAction;
use super::ResumePickerView;
use super::render::format_bytes;

fn session(title: &str, cwd: &str, branch: &str, is_active: bool) -> SessionListEntry {
    SessionListEntry {
        session_id: SessionId::new(),
        title: title.to_string(),
        preview: format!("preview for {title}"),
        cwd: PathBuf::from(cwd),
        branch: Some(branch.to_string()),
        last_activity_at: Utc::now(),
        transcript_size_bytes: Some(10_300),
        is_active,
    }
}

fn ready_picker(current_cwd: &str, sessions: Vec<SessionListEntry>) -> ResumePickerView {
    let mut picker = ResumePickerView::loading(PathBuf::from(current_cwd), Color::Cyan);
    assert!(picker.update_resume_sessions(sessions));
    picker
}

fn press(picker: &mut ResumePickerView, code: KeyCode, modifiers: KeyModifiers) {
    picker.handle_key_event(KeyEvent::new(code, modifiers));
}

#[test]
fn defaults_to_exact_cwd_and_ctrl_a_reveals_other_projects() {
    let local = session("local", "workspace", "main", false);
    let remote = session("remote", "other", "feature", true);
    let local_id = local.session_id;
    let mut picker = ready_picker("workspace", vec![local, remote]);

    assert_eq!(picker.filtered_indices(), vec![0]);
    assert_eq!(picker.selected_session_id, Some(local_id));

    press(&mut picker, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(picker.filtered_indices(), vec![0, 1]);
    assert_eq!(picker.selected_session_id, Some(local_id));
    assert!(picker.show_all_projects);
}

#[test]
fn search_matches_metadata_and_escape_clears_before_closing() {
    let first = session("alpha", "workspace", "main", true);
    let second = session("beta", "workspace", "release", false);
    let second_id = second.session_id;
    let mut picker = ready_picker("workspace", vec![first, second]);

    for ch in "release".chars() {
        press(&mut picker, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    assert_eq!(picker.filtered_indices(), vec![1]);
    assert_eq!(picker.selected_session_id, Some(second_id));

    press(&mut picker, KeyCode::Esc, KeyModifiers::NONE);
    assert!(picker.search_query.is_empty());
    assert!(!picker.is_complete());
    press(&mut picker, KeyCode::Esc, KeyModifiers::NONE);
    assert!(picker.is_complete());
}

#[test]
fn preview_results_are_correlated_by_session_id() {
    let selected = session("selected", "workspace", "main", true);
    let selected_id = selected.session_id;
    let stale_id = SessionId::new();
    let mut picker = ready_picker("workspace", vec![selected]);

    press(&mut picker, KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(
        picker.take_resume_action(),
        Some(ResumePickerAction::Preview {
            session_id: selected_id
        })
    );
    assert!(!picker.update_resume_preview(stale_id, Ok(Vec::new())));

    let messages = vec![SessionPreviewMessage {
        role: SessionPreviewRole::User,
        text: "hello".to_string(),
    }];
    assert!(picker.update_resume_preview(selected_id, Ok(messages.clone())));
    assert_eq!(
        picker.previews.get(&selected_id),
        Some(&PreviewState::Loaded(messages))
    );
}

#[test]
fn rename_action_targets_the_selected_session_id() {
    let selected = session("x", "workspace", "main", true);
    let selected_id = selected.session_id;
    let mut picker = ready_picker("workspace", vec![selected]);

    press(&mut picker, KeyCode::Char('r'), KeyModifiers::CONTROL);
    press(&mut picker, KeyCode::Backspace, KeyModifiers::NONE);
    for ch in "renamed".chars() {
        press(&mut picker, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut picker, KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(
        picker.take_resume_action(),
        Some(ResumePickerAction::Rename {
            session_id: selected_id,
            title: "renamed".to_string(),
        })
    );
    assert!(picker.update_resume_rename(selected_id.into(), Ok("renamed".to_string())));
    assert_eq!(picker.sessions[0].title, "renamed");
}

#[test]
fn transcript_sizes_use_decimal_units() {
    assert_eq!(format_bytes(Some(999)), "999B");
    assert_eq!(format_bytes(Some(10_300)), "10.3KB");
    assert_eq!(format_bytes(Some(1_250_000)), "1.2MB");
    assert_eq!(format_bytes(None), "unknown size");
}

#[test]
fn metadata_row_omits_git_branch() {
    let picker = ready_picker(
        "workspace",
        vec![session("entry", "workspace", "main", true)],
    );
    let area = Rect::new(0, 0, 80, 12);
    let mut buffer = Buffer::empty(area);
    picker.render(area, &mut buffer);
    let rendered = (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("10.3KB"), "{rendered}");
    assert!(!rendered.contains("main"), "{rendered}");
}
