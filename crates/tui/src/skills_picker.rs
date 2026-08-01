//! Interactive `/skills` picker helpers.

use std::path::PathBuf;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;

/// One skill row for the interactive `/skills` flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillPickerEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) enabled: bool,
    pub(crate) source: String,
    pub(crate) path: PathBuf,
}

pub(crate) fn skill_picker_entry_from_record(skill: &devo_server::SkillRecord) -> SkillPickerEntry {
    SkillPickerEntry {
        id: skill.id.clone(),
        name: skill.name.clone(),
        description: skill
            .short_description
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| skill.description.clone()),
        enabled: skill.enabled,
        source: skill_source_label(&skill.source),
        path: skill.path.clone(),
    }
}

fn skill_source_label(source: &devo_server::SkillSource) -> String {
    match source {
        devo_server::SkillSource::User => "user".to_string(),
        devo_server::SkillSource::Workspace { cwd } => format!("workspace ({})", cwd.display()),
        devo_server::SkillSource::Plugin { plugin_id } => format!("plugin ({plugin_id})"),
        devo_server::SkillSource::System => "system".to_string(),
        devo_server::SkillSource::Admin => "admin".to_string(),
    }
}

fn compact_source_label(source: &str) -> &str {
    if source.starts_with("workspace") {
        "workspace"
    } else if source.starts_with("plugin") {
        "plugin"
    } else {
        source
    }
}

/// Searchable list of configured skills.
pub(crate) fn skills_list_params(skills: &[SkillPickerEntry]) -> SelectionViewParams {
    let items = if skills.is_empty() {
        vec![SelectionItem {
            name: "No skills found".to_string(),
            description: Some("Add skills under ~/.devo/skills or the workspace.".to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..SelectionItem::default()
        }]
    } else {
        skills
            .iter()
            .map(|skill| {
                let name = skill.name.clone();
                let meta = if skill.enabled {
                    compact_source_label(&skill.source).to_string()
                } else {
                    format!("{} · disabled", compact_source_label(&skill.source))
                };
                SelectionItem {
                    name: skill.name.clone(),
                    description: Some(meta),
                    search_value: Some(format!(
                        "{} {} {}",
                        skill.name, skill.description, skill.source
                    )),
                    dismiss_on_select: true,
                    actions: vec![Box::new(move |tx: &AppEventSender| {
                        tx.send(AppEvent::SkillSelected { name: name.clone() });
                    })],
                    ..SelectionItem::default()
                }
            })
            .collect()
    };

    SelectionViewParams {
        title: Some("Skills".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to search".to_string()),
        ..SelectionViewParams::default()
    }
}

/// Detail actions for one skill.
pub(crate) fn skill_detail_params(skill: &SkillPickerEntry) -> SelectionViewParams {
    let insert_name = skill.name.clone();
    let path = skill.path.clone();
    let enabled = skill.enabled;
    let toggle_name = skill.name.clone();
    let back_name = skill.name.clone();

    let mut items = vec![SelectionItem {
        name: "Insert into prompt".to_string(),
        description: Some(format!("Append @{insert_name}")),
        dismiss_on_select: true,
        actions: vec![Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::InsertComposerText {
                text: format!("@{insert_name}"),
                binding: None,
            });
        })],
        ..SelectionItem::default()
    }];

    items.push(SelectionItem {
        name: if enabled {
            "Disable".to_string()
        } else {
            "Enable".to_string()
        },
        description: Some("Update enabled state for this session".to_string()),
        dismiss_on_select: true,
        actions: vec![Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::Command(AppCommand::SetSkillEnabled {
                path: path.clone(),
                enabled: !enabled,
                name: toggle_name.clone(),
            }));
        })],
        ..SelectionItem::default()
    });

    items.push(SelectionItem {
        name: "Back".to_string(),
        description: Some("Return to skills list".to_string()),
        dismiss_on_select: true,
        actions: vec![Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::SkillOpenList);
        })],
        ..SelectionItem::default()
    });

    let status = if skill.enabled { "enabled" } else { "disabled" };
    let subtitle = format!(
        "{}\n\nSource   {}\nStatus   {status}\nPath     {}",
        skill.description.trim(),
        skill.source,
        skill.path.display()
    );

    SelectionViewParams {
        title: Some(skill.name.clone()),
        subtitle: Some(subtitle),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        on_cancel: Some(Box::new(move |tx: &AppEventSender| {
            let _ = back_name;
            tx.send(AppEvent::SkillOpenList);
        })),
        ..SelectionViewParams::default()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;

    use super::*;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;

    #[test]
    fn skills_list_params_select_emits_skill_selected() {
        let skills = vec![SkillPickerEntry {
            id: "docs".to_string(),
            name: "docs".to_string(),
            description: "Docs skill".to_string(),
            enabled: true,
            source: "user".to_string(),
            path: PathBuf::from("/tmp/docs/SKILL.md"),
        }];
        let params = skills_list_params(&skills);
        assert_eq!(params.title.as_deref(), Some("Skills"));
        assert!(params.items[0].name_prefix_spans.is_empty());
        assert_eq!(params.items[0].description.as_deref(), Some("user"));
        let (tx, mut rx) = mpsc::unbounded_channel();
        params.items[0].actions[0](&AppEventSender::new(tx));
        assert_eq!(
            rx.try_recv().expect("event"),
            AppEvent::SkillSelected {
                name: "docs".to_string(),
            }
        );
    }
}
