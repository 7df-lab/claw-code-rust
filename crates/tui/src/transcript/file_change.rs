//! Helpers for file-change transcript projection.

use std::collections::HashMap;
use std::path::PathBuf;

use devo_protocol::protocol::FileChange;

pub(crate) fn has_visible_file_changes(changes: &HashMap<PathBuf, FileChange>) -> bool {
    changes.values().any(|change| match change {
        FileChange::Add { content } | FileChange::Delete { content } => !content.trim().is_empty(),
        FileChange::Update {
            unified_diff,
            old_text,
            new_text,
            move_path,
        } => !unified_diff.trim().is_empty() || old_text != new_text || move_path.is_some(),
    })
}
