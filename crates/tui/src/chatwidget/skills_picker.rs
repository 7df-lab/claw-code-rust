//! Interactive `/skills` picker flow on `ChatWidget`.

use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::skills_picker::SkillPickerEntry;
use crate::skills_picker::skill_detail_params;
use crate::skills_picker::skills_list_params;

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn set_skills_reopen_detail(&mut self, name: Option<String>) {
        self.skills_reopen_detail = name;
    }

    pub(super) fn on_skills_listed_for_picker(&mut self, picker_skills: Vec<SkillPickerEntry>) {
        self.skills_snapshot = Some(picker_skills);
        if let Some(name) = self.skills_reopen_detail.take() {
            self.open_skill_detail(&name);
            return;
        }
        self.open_skills_list();
    }

    pub(super) fn open_skills_list(&mut self) {
        let Some(skills) = self.skills_snapshot.clone() else {
            self.set_status_message("No skills snapshot");
            return;
        };
        if skills.is_empty() {
            self.set_status_message("No skills found");
        } else {
            self.set_status_message("Select a skill");
        }
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                skills_list_params(&skills),
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.frame_requester.schedule_frame();
    }

    pub(super) fn open_skill_detail(&mut self, name: &str) {
        let Some(skill) = self
            .skills_snapshot
            .as_ref()
            .and_then(|skills| skills.iter().find(|skill| skill.name == name))
            .cloned()
        else {
            self.set_status_message(format!("Skill `{name}` not found"));
            self.open_skills_list();
            return;
        };
        self.set_status_message(format!("Skill · {}", skill.name));
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                skill_detail_params(&skill),
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.frame_requester.schedule_frame();
    }
}
