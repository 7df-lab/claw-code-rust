//! Helpers for canonical session queue entries in the TUI worker/UI.

use devo_protocol::InputItem;
use devo_protocol::canonical::item::UserInput;
use devo_protocol::canonical::queue::QueueEntry;

/// Map legacy [`InputItem`] values into canonical [`UserInput`] for queue RPCs.
pub(crate) fn user_input_from_input_items(items: &[InputItem]) -> Vec<UserInput> {
    items
        .iter()
        .map(|item| match item {
            InputItem::Text { text } => UserInput::Text { text: text.clone() },
            InputItem::Skill { name, .. } => UserInput::Skill { name: name.clone() },
            InputItem::LocalImage { path } => UserInput::LocalImage {
                path: path.clone(),
                detail: None,
            },
            InputItem::Mention { path, .. } => UserInput::Mention { uri: path.clone() },
        })
        .collect()
}

/// Extract full editable text from a queue entry (display-only join of text parts).
pub(crate) fn queue_entry_text(entry: &QueueEntry) -> String {
    let texts: Vec<&str> = entry
        .input
        .iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if !texts.is_empty() {
        return texts.join("\n");
    }
    entry.preview.clone()
}

/// Collapse newlines to spaces for a single-line queue preview (render only).
pub(crate) fn queue_render_preview(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
