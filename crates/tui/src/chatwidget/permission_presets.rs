//! Permission preset picker data for the chat widget.
//!
//! The chat widget owns the selected permission preset, while this module keeps
//! the label and picker-item mapping out of the main conversation surface.

use devo_protocol::PermissionPreset;

use crate::app_command::AppCommand;
use crate::app_command::PersistScope;
use crate::app_event::AppEvent;
use crate::bottom_pane::list_selection_view::SelectionItem;

pub(super) fn permission_preset_items(
    current: PermissionPreset,
    persist_scope: PersistScope,
) -> Vec<SelectionItem> {
    [
        (
            PermissionPreset::Default,
            "Ask for approval",
            "Workspace sandbox; read, write, and run commands in workspace; network blocked. You approve sensitive tools.",
        ),
        (
            PermissionPreset::AutoReview,
            "Approve for me",
            "Same sandbox as Ask for approval. An AI reviewer may approve low-risk tools; uncertain ones still ask you.",
        ),
        (
            PermissionPreset::FullAccess,
            "Full access",
            "No OS sandbox and no approval prompts; use with caution.",
        ),
    ]
    .into_iter()
    .map(|(preset, label, description)| SelectionItem {
        name: label.to_string(),
        description: Some(description.to_string()),
        is_current: preset == current,
        dismiss_on_select: true,
        actions: vec![Box::new(move |app_event_tx| {
            app_event_tx.send(AppEvent::Command(AppCommand::update_permissions(
                preset,
                persist_scope,
            )));
        })],
        ..Default::default()
    })
    .collect()
}

pub(super) fn permission_preset_label(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::Default => "Ask for approval",
        PermissionPreset::AutoReview => "Approve for me",
        PermissionPreset::FullAccess => "Full access",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn permission_preset_labels_are_stable() {
        let actual = [
            permission_preset_label(PermissionPreset::Default),
            permission_preset_label(PermissionPreset::AutoReview),
            permission_preset_label(PermissionPreset::FullAccess),
        ];

        assert_eq!(
            actual,
            ["Ask for approval", "Approve for me", "Full access"]
        );
    }

    #[test]
    fn permission_preset_items_mark_current_selection() {
        let items = permission_preset_items(PermissionPreset::AutoReview, PersistScope::Session);
        let actual: Vec<_> = items
            .iter()
            .map(|item| {
                (
                    item.name.as_str(),
                    item.description.is_some(),
                    item.is_current,
                    item.dismiss_on_select,
                    item.actions.len(),
                )
            })
            .collect();

        assert_eq!(
            actual,
            vec![
                ("Ask for approval", true, false, true, 1),
                ("Approve for me", true, true, true, 1),
                ("Full access", true, false, true, 1),
            ]
        );
    }
}
