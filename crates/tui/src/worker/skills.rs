//! Skills list loading for the TUI worker.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_server::SkillRecord;
use devo_server::SkillSource;
use devo_server::StdioServerClient;
use tokio::sync::mpsc;

use crate::bottom_pane::SkillInterfaceMetadata;
use crate::bottom_pane::SkillMetadata;
use crate::events::WorkerEvent;

pub(crate) async fn emit_skills_list(
    client: &mut StdioServerClient,
    cwd: &Path,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    open_picker: bool,
) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.skill_list_native(Some(cwd.to_path_buf()), false),
    )
    .await
    .context("skills list request timed out")??;
    emit_skills_list_result(
        result.skills.into_iter().map(SkillRecord::from).collect(),
        event_tx,
        open_picker,
    );
    Ok(())
}

pub(crate) fn emit_skills_list_result(
    skills: Vec<SkillRecord>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    open_picker: bool,
) {
    let picker_skills = skills
        .iter()
        .map(crate::skills_picker::skill_picker_entry_from_record)
        .collect();
    let skills = skills
        .iter()
        .filter(|skill| skill.enabled)
        .map(skill_metadata_from_record)
        .collect();
    let _ = event_tx.send(WorkerEvent::SkillsListed {
        skills,
        picker_skills,
        open_picker,
    });
}

pub(crate) fn render_skill_list_body(skills: &[SkillRecord]) -> String {
    if skills.is_empty() {
        return "_No skills found._".to_string();
    }

    skills
        .iter()
        .map(|skill| {
            let enabled = if skill.enabled { "yes" } else { "no" };
            format!(
                "- `{}` - {}\n  enabled: {}\n  source: {}\n  path: `{}`",
                skill.name,
                skill.description,
                enabled,
                render_skill_source(&skill.source),
                skill.path.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn skill_metadata_from_record(skill: &SkillRecord) -> SkillMetadata {
    SkillMetadata {
        name: skill.name.clone(),
        description: skill.description.clone(),
        short_description: skill.short_description.clone(),
        interface: skill
            .interface
            .as_ref()
            .map(|interface| SkillInterfaceMetadata {
                display_name: interface.display_name.clone(),
                short_description: interface.short_description.clone(),
            }),
        path_to_skills_md: skill.path.clone(),
    }
}

fn render_skill_source(source: &SkillSource) -> String {
    match source {
        SkillSource::User => "user".to_string(),
        SkillSource::Workspace { cwd } => format!("workspace ({})", cwd.display()),
        SkillSource::Plugin { plugin_id } => format!("plugin ({plugin_id})"),
        SkillSource::System => "system".to_string(),
        SkillSource::Admin => "admin".to_string(),
    }
}
