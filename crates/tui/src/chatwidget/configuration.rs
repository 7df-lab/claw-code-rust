//! Model, reasoning effort, theme, and permission configuration flows for `ChatWidget`.
//!
//! Picker construction and selection application live here so configuration UI
//! changes stay separate from transcript and input handling.

use devo_protocol::Model;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use ratatui::style::Color;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::ModelPickerEffortOption;
use crate::bottom_pane::ModelPickerEntry;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::events::SavedModelEntry;
use crate::history_cell;

use super::ChatWidget;
use super::permission_preset_items;
use super::permission_preset_label;
use super::reasoning_effort;
use super::reasoning_effort::ReasoningEffortListEntry;

impl ChatWidget {
    pub(crate) fn set_model(&mut self, model: Model) {
        self.reasoning_effort_selection =
            reasoning_effort::current_reasoning_effort_selection_for_model(
                &model,
                self.reasoning_effort_selection.as_deref(),
            );
        self.session.reasoning_effort = model
            .resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
            .effective_reasoning_effort;
        self.session.provider = Some(model.provider_wire_api());
        self.session.model = Some(model);
        self.session.request_model = None;
        self.session.model_binding_id = None;
        self.current_model_binding_id = None;
        self.set_default_placeholder();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn update_session_request_model(&mut self, slug: String) {
        self.session.request_model = None;
        if let Some(entry) = self.saved_model_entry_by_binding_id(&slug).cloned() {
            self.apply_saved_model_entry_to_session(&entry);
            return;
        }
        self.current_model_binding_id = None;
        self.session.model_binding_id = None;
        self.sync_session_catalog_model(slug);
    }

    pub(super) fn update_session_model_selection(
        &mut self,
        slug: String,
        model_binding_id: Option<String>,
    ) {
        if let Some(binding_id) = model_binding_id {
            self.session.request_model = None;
            if let Some(entry) = self.saved_model_entry_by_binding_id(&binding_id).cloned() {
                self.apply_saved_model_entry_to_session(&entry);
                return;
            }
            self.sync_session_catalog_model(slug);
            self.current_model_binding_id = Some(binding_id.clone());
            self.session.model_binding_id = Some(binding_id);
            return;
        }

        self.update_session_request_model(slug);
    }

    pub(super) fn sync_session_catalog_model(&mut self, slug: String) {
        if let Some(model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        {
            self.session.reasoning_effort = model
                .resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
                .effective_reasoning_effort;
            self.session.provider = Some(model.provider_wire_api());
            self.session.model = Some(model);
            return;
        }

        if let Some(model) = self.session.model.as_mut() {
            let display_name = if model.slug == slug && !model.display_name.is_empty() {
                model.display_name.clone()
            } else {
                slug.clone()
            };
            model.slug = slug.clone();
            model.display_name = display_name;
            self.session.reasoning_effort = model
                .resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
                .effective_reasoning_effort;
            return;
        }

        self.session.model = Some(Model {
            slug: slug.clone(),
            display_name: slug,
            provider: self
                .session
                .provider
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
            ..Model::default()
        });
        self.session.reasoning_effort = self
            .session
            .model
            .as_ref()
            .map(|model| {
                model.resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
            })
            .and_then(|resolved| resolved.effective_reasoning_effort);
    }

    pub(super) fn apply_session_request_model(
        &mut self,
        model_slug: String,
        request_model: String,
        display_name: String,
    ) {
        self.sync_session_catalog_model(model_slug.clone());
        self.session.request_model = if request_model == model_slug {
            None
        } else {
            Some(request_model)
        };
        let display_name = display_name.trim();
        if !display_name.is_empty()
            && let Some(model) = self.session.model.as_mut()
        {
            model.display_name = display_name.to_string();
        }
        if self.onboarding.is_none() {
            self.refresh_header_box();
        }
        self.sync_bottom_pane_summary();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn apply_session_model_binding(&mut self, binding: &ProviderModelBinding) {
        self.apply_session_request_model(
            binding.model_slug.clone(),
            binding.request_model.clone(),
            binding
                .display_name
                .clone()
                .unwrap_or_else(|| binding.request_model.clone()),
        );
    }

    pub(super) fn user_turn_model(&self) -> Option<String> {
        self.session
            .request_model
            .clone()
            .or_else(|| self.session.model.as_ref().map(|model| model.slug.clone()))
    }

    pub(super) fn user_turn_model_binding_id(&self) -> Option<String> {
        self.session.model_binding_id.clone()
    }

    pub(crate) fn set_reasoning_effort_selection(&mut self, selection: Option<String>) {
        self.reasoning_effort_selection = selection;
        self.session.reasoning_effort = self
            .session
            .model
            .as_ref()
            .map(|model| {
                model.resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
            })
            .and_then(|resolved| resolved.effective_reasoning_effort);
        self.refresh_header_box();
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn current_reasoning_effort_selection(&self) -> Option<&str> {
        self.reasoning_effort_selection.as_deref()
    }

    pub(crate) fn current_reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.session.reasoning_effort.or_else(|| {
            self.session
                .model
                .as_ref()
                .map(|model| {
                    model.resolve_reasoning_effort_selection(
                        self.reasoning_effort_selection.as_deref(),
                    )
                })
                .and_then(|resolved| resolved.effective_reasoning_effort)
        })
    }

    pub(super) fn normalized_reasoning_effort_selection_for_display(
        &self,
        model: &Model,
    ) -> Option<String> {
        reasoning_effort::current_reasoning_effort_selection_for_model(
            model,
            self.reasoning_effort_selection.as_deref(),
        )
    }

    pub(super) fn display_reasoning_effort_selection(&self) -> Option<String> {
        let model = self.session.model.as_ref()?;
        self.normalized_reasoning_effort_selection_for_display(model)
    }

    pub(crate) fn reasoning_effort_entries(&self) -> Vec<ReasoningEffortListEntry> {
        let Some(model) = &self.session.model else {
            return Vec::new();
        };

        reasoning_effort::reasoning_effort_entries_for_model(
            model,
            self.reasoning_effort_selection.as_deref(),
        )
    }

    fn saved_model_selection_value(entry: &SavedModelEntry) -> &str {
        entry.binding_id.as_deref().unwrap_or(entry.model.as_str())
    }

    fn saved_model_display_name(entry: &SavedModelEntry) -> Option<String> {
        entry
            .display_name
            .as_deref()
            .or(entry.request_model.as_deref())
            .map(str::trim)
            .filter(|display_name| !display_name.is_empty())
            .map(ToOwned::to_owned)
    }

    fn saved_model_display_label(&self, entry: &SavedModelEntry) -> String {
        Self::saved_model_display_name(entry)
            .or_else(|| {
                self.available_models
                    .iter()
                    .find(|model| model.slug == entry.model)
                    .map(|model| model.display_name.clone())
            })
            .unwrap_or_else(|| entry.model.clone())
    }

    fn saved_model_provider_name(entry: &SavedModelEntry) -> Option<String> {
        entry
            .provider_name
            .as_deref()
            .or(entry.provider_id.as_deref())
            .map(str::trim)
            .filter(|provider_name| !provider_name.is_empty())
            .map(ToOwned::to_owned)
    }

    fn saved_model_entry_by_binding_id(&self, binding_id: &str) -> Option<&SavedModelEntry> {
        self.saved_models
            .iter()
            .find(|entry| entry.binding_id.as_deref() == Some(binding_id))
    }

    fn saved_model_entry_for_selection(&self, selection: &str) -> Option<&SavedModelEntry> {
        self.saved_model_entry_by_binding_id(selection).or_else(|| {
            self.saved_models.iter().find(|entry| {
                entry.model == selection || entry.request_model.as_deref() == Some(selection)
            })
        })
    }

    fn saved_model_entry_is_current(&self, entry: &SavedModelEntry) -> bool {
        if entry.binding_id.is_some()
            && self.current_model_binding_id.as_deref() == entry.binding_id.as_deref()
        {
            return true;
        }
        if self.current_model_binding_id.is_some() {
            return false;
        }
        self.session
            .model
            .as_ref()
            .is_some_and(|model| model.slug == entry.model)
            && self.session.request_model == entry.request_model
    }

    fn model_for_saved_entry(&self, entry: &SavedModelEntry) -> Model {
        let mut model = self
            .available_models
            .iter()
            .find(|model| model.slug == entry.model)
            .cloned()
            .unwrap_or_else(|| Model {
                slug: entry.model.clone(),
                ..Model::default()
            });
        if let Some(display_name) = Self::saved_model_display_name(entry) {
            model.display_name = display_name;
        }
        model.provider = entry.wire_api;
        model
    }

    fn apply_saved_model_entry_to_session(&mut self, entry: &SavedModelEntry) -> Model {
        let model = self.model_for_saved_entry(entry);
        self.session.provider = Some(entry.wire_api);
        self.session.model = Some(model.clone());
        self.session.request_model = entry.request_model.clone();
        self.session.model_binding_id = entry.binding_id.clone();
        self.current_model_binding_id = entry.binding_id.clone();
        model
    }

    pub(super) fn open_model_picker(&mut self) {
        self.open_model_picker_with_scope(crate::app_command::PersistScope::Session);
    }

    pub(super) fn open_model_picker_for_defaults(&mut self) {
        self.open_model_picker_with_scope(crate::app_command::PersistScope::Default);
    }

    fn open_model_picker_with_scope(&mut self, persist_scope: crate::app_command::PersistScope) {
        self.settings_picker_persist_scope = persist_scope;
        let session_effort = self.reasoning_effort_selection.clone();
        let entries = self
            .saved_models
            .iter()
            .map(|entry| {
                let model = self.model_for_saved_entry(entry);
                let effort_entries = reasoning_effort::reasoning_effort_entries_for_model(
                    &model,
                    session_effort.as_deref(),
                );
                let selected_effort = effort_entries
                    .iter()
                    .find(|option| option.is_current)
                    .map(|option| option.value.clone());
                ModelPickerEntry {
                    selection_value: Self::saved_model_selection_value(entry).to_string(),
                    display_name: self.saved_model_display_label(entry),
                    right_hint: Self::saved_model_provider_name(entry),
                    is_current: self.saved_model_entry_is_current(entry),
                    effort_options: effort_entries
                        .into_iter()
                        .map(|option| ModelPickerEffortOption {
                            label: option.label,
                            value: option.value,
                        })
                        .collect(),
                    selected_effort,
                }
            })
            .collect();
        self.bottom_pane.open_model_picker(entries);
        self.set_status_message("Select a model");
    }

    pub(super) fn handle_model_picker_selection(
        &mut self,
        slug: String,
        reasoning_effort: Option<String>,
    ) {
        if let Some(entry) = self.saved_model_entry_for_selection(&slug).cloned() {
            let selected_model = self.apply_saved_model_entry_to_session(&entry);
            let display_name = self.saved_model_display_label(&entry);
            let selection = Self::saved_model_selection_value(&entry).to_string();
            self.apply_model_and_effort(selection, display_name, &selected_model, reasoning_effort);
            return;
        }

        let Some(selected_model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        else {
            self.apply_model_selection(slug);
            return;
        };

        self.current_model_binding_id = None;
        self.session.model_binding_id = None;
        self.session.provider = Some(selected_model.provider);
        self.session.model = Some(selected_model.clone());
        self.session.request_model = None;
        let display_name = selected_model.display_name.clone();
        let selection = selected_model.slug.clone();
        self.apply_model_and_effort(selection, display_name, &selected_model, reasoning_effort);
    }

    fn apply_model_and_effort(
        &mut self,
        selection: String,
        display_name: String,
        model: &Model,
        reasoning_effort: Option<String>,
    ) {
        self.reasoning_effort_selection =
            if model.effective_reasoning_capability().options().is_empty() {
                None
            } else {
                reasoning_effort::current_reasoning_effort_selection_for_model(
                    model,
                    reasoning_effort
                        .as_deref()
                        .or(self.reasoning_effort_selection.as_deref()),
                )
            };
        self.session.reasoning_effort = model
            .resolve_reasoning_effort_selection(self.reasoning_effort_selection.as_deref())
            .effective_reasoning_effort;
        self.refresh_header_box();
        self.app_event_tx.send(AppEvent::Command(
            AppCommand::override_turn_context_with_scope(
                /*cwd*/ None,
                Some(selection),
                Some(self.reasoning_effort_selection.clone()),
                /*sandbox*/ None,
                /*approval_policy*/ None,
                self.settings_picker_persist_scope,
            ),
        ));
        self.settings_picker_persist_scope = crate::app_command::PersistScope::Session;
        self.set_status_message(format!("Model set to {display_name}"));
    }

    pub(super) fn open_theme_picker(&mut self) {
        self.bottom_pane
            .open_theme_picker(&self.theme_set.themes, self.active_theme_name.clone());
        self.set_status_message("Select a theme");
    }

    pub(super) fn open_permissions_picker(&mut self) {
        self.open_permissions_picker_with_scope(crate::app_command::PersistScope::Session);
    }

    pub(super) fn open_permissions_picker_for_defaults(&mut self) {
        self.open_permissions_picker_with_scope(crate::app_command::PersistScope::Default);
    }

    fn open_permissions_picker_with_scope(
        &mut self,
        persist_scope: crate::app_command::PersistScope,
    ) {
        self.settings_picker_persist_scope = persist_scope;
        let current = self.permission_preset;
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                SelectionViewParams {
                    title: Some("Update Permissions".to_string()),
                    footer_hint: Some(Line::from("Press enter to confirm or esc to go back")),
                    items: permission_preset_items(current, persist_scope),
                    ..SelectionViewParams::default()
                },
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.set_status_message("Select permissions");
    }

    pub(super) fn open_reasoning_view_picker(&mut self) {
        let current = self.collapse_reasoning;
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                SelectionViewParams {
                    title: Some("Show Reasoning".to_string()),
                    footer_hint: Some(Line::from("Press enter to confirm or esc to go back")),
                    items: super::reasoning_view::reasoning_view_items(current),
                    ..SelectionViewParams::default()
                },
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.set_status_message("Select reasoning view");
    }

    pub(crate) fn apply_collapse_reasoning(&mut self, collapsed: bool) {
        self.collapse_reasoning = collapsed;
        let _ = crate::onboarding::save_collapse_reasoning(collapsed);
        let label = super::reasoning_view::reasoning_view_label(collapsed);
        self.add_to_history(history_cell::new_info_event(
            format!("Show reasoning set to {label}"),
            None,
        ));
        self.set_status_message(format!("Show reasoning set to {label}"));
        // Refresh the live reasoning cell if one is streaming.
        if let Some(index) = self
            .active_text_items
            .iter()
            .position(|item| item.kind == crate::events::TextItemKind::Reasoning)
        {
            self.sync_text_item_cell(index);
        }
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn apply_collaboration_mode(
        &mut self,
        collaboration_mode: devo_protocol::CollaborationMode,
        persist_scope: crate::app_command::PersistScope,
    ) {
        let input_mode = crate::bottom_pane::InputMode::from_collaboration_mode(collaboration_mode);
        if persist_scope == crate::app_command::PersistScope::Default {
            self.default_collaboration_mode = collaboration_mode;
        }
        self.current_turn_mode = input_mode;
        self.bottom_pane.set_input_mode(input_mode);
        self.refresh_settings_hub_if_open();
    }

    pub(crate) fn note_permissions_updated(&mut self, preset: devo_protocol::PermissionPreset) {
        self.permission_preset = preset;
        self.sandbox_profile = Some(
            match preset {
                devo_protocol::PermissionPreset::FullAccess => "off",
                devo_protocol::PermissionPreset::Default
                | devo_protocol::PermissionPreset::AutoReview => "workspace",
            }
            .to_string(),
        );
        let label = permission_preset_label(preset);
        self.add_to_history(history_cell::new_info_event(
            format!("Permissions updated to {label}"),
            None,
        ));
        self.set_status_message(format!("Permissions updated to {label}"));
        self.refresh_settings_hub_if_open();
    }

    pub(crate) fn note_effective_context_window_updated(&mut self, effective_context_window: u64) {
        self.effective_context_window = Some(effective_context_window);
        self.default_compaction_token_limit = Some(effective_context_window);
        if let Some(occupancy) = self.last_context_occupancy.as_mut() {
            occupancy.context_window_tokens = effective_context_window;
        }
        let label = crate::bottom_pane::format_token_limit(effective_context_window);
        self.add_to_history(history_cell::new_info_event(
            format!("Compaction threshold updated to {label}"),
            None,
        ));
        self.set_status_message(format!("Compaction threshold updated to {label}"));
        self.sync_bottom_pane_summary();
        self.refresh_status_panel_if_open();
        self.refresh_settings_hub_if_open();
    }

    fn refresh_status_panel_if_open(&mut self) {
        self.bottom_pane.refresh_status_panel(
            self.last_context_occupancy.clone(),
            crate::bottom_pane::SessionTokenTotals {
                input: self.total_input_tokens,
                output: self.total_output_tokens,
                cache_read: self.total_cache_read_tokens,
            },
        );
    }

    pub(crate) fn note_sandbox_profile_updated(&mut self, profile: String) {
        // Sandbox follows the permission preset (or an explicit /sandbox pick). Keep local
        // state in sync for settings/status, but do not emit a transcript or status line —
        // "Sandbox profile updated to …" is noise next to "Permissions updated to …".
        self.sandbox_profile = Some(profile);
        self.refresh_settings_hub_if_open();
    }

    pub(super) fn apply_theme_selection(&mut self, name: String) {
        if let Some(theme) = self.theme_set.find(&name).cloned() {
            if self.active_theme_name == name {
                self.refresh_settings_hub_if_open();
                return;
            }
            self.active_theme_name = name.clone();
            self.bottom_pane.set_accent_color(theme.accent_color);
            let _ = crate::onboarding::save_theme_selection(&name);
            self.refresh_header_box();
            if self.history.first().is_some_and(|cell| {
                cell.as_any()
                    .is::<crate::startup_logo_cell::StartupLogoCell>()
            }) {
                self.history[0] = Box::new(crate::startup_logo_cell::StartupLogoCell::new(
                    self.active_accent_color(),
                ));
            }
            self.set_status_message(format!("Theme set to {name}"));
            self.refresh_settings_hub_if_open();
            // Header/logo are flushed into terminal scrollback. Reset the flush
            // cursor and ask the host to clear the managed inline area so the
            // next draw re-emits transcript lines with the new accent.
            if self.next_history_flush_index > 0 {
                self.next_history_flush_index = 0;
                self.schedule_debounced_inline_transcript_reload();
            } else {
                self.frame_requester.schedule_frame();
            }
        }
    }

    pub(super) fn cycle_theme(&mut self, direction: crate::app_event::SettingsCycleDirection) {
        let themes = &self.theme_set.themes;
        if themes.is_empty() {
            return;
        }
        let current = themes
            .iter()
            .position(|theme| theme.name == self.active_theme_name)
            .unwrap_or(0);
        let next = match direction {
            crate::app_event::SettingsCycleDirection::Next => (current + 1) % themes.len(),
            crate::app_event::SettingsCycleDirection::Previous => {
                if current == 0 {
                    themes.len() - 1
                } else {
                    current - 1
                }
            }
        };
        let name = themes[next].name.clone();
        self.apply_theme_selection(name);
    }

    fn schedule_debounced_inline_transcript_reload(&mut self) {
        self.theme_reload_epoch = self.theme_reload_epoch.wrapping_add(1);
        let epoch = self.theme_reload_epoch;
        let tx = self.app_event_tx.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    tx.send(crate::app_event::AppEvent::FlushDebouncedThemeReload { epoch });
                });
            }
            Err(_) => {
                // Unit tests may lack a Tokio runtime; reload immediately.
                tx.send(crate::app_event::AppEvent::ReloadInlineTranscript);
            }
        }
    }

    pub(super) fn flush_debounced_theme_reload(&mut self, epoch: u64) {
        if epoch != self.theme_reload_epoch {
            return;
        }
        self.app_event_tx
            .send(crate::app_event::AppEvent::ReloadInlineTranscript);
    }

    pub(super) fn open_settings_hub(&mut self) {
        let snapshot = self.settings_hub_snapshot();
        self.bottom_pane.open_settings_hub(snapshot);
        self.set_status_message("Settings");
    }

    pub(super) fn open_settings_hub_appearance(&mut self) {
        let snapshot = self.settings_hub_snapshot();
        self.bottom_pane
            .open_settings_hub_on_tab(snapshot, crate::bottom_pane::SettingsHubTab::Appearance);
        self.set_status_message("Settings");
    }

    pub(super) fn open_compaction_threshold_picker(&mut self) {
        let snapshot = self.compaction_threshold_snapshot();
        self.bottom_pane.open_compaction_threshold(snapshot);
        self.set_status_message("Select compaction threshold");
    }

    pub(super) fn refresh_settings_hub_if_open(&mut self) {
        let snapshot = self.settings_hub_snapshot();
        self.bottom_pane.refresh_settings_hub(snapshot);
    }

    fn settings_hub_snapshot(&self) -> crate::bottom_pane::SettingsHubSnapshot {
        crate::bottom_pane::SettingsHubSnapshot {
            model_label: self
                .session
                .model
                .as_ref()
                .map(|model| model.slug.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            permissions_label: permission_preset_label(self.permission_preset).to_string(),
            mode: crate::bottom_pane::InputMode::from_collaboration_mode(
                self.default_collaboration_mode,
            ),
            compaction_threshold_label: crate::bottom_pane::format_token_limit(
                self.default_compaction_token_limit
                    .map(|limit| {
                        let model_window = self
                            .session
                            .model
                            .as_ref()
                            .map(|model| u64::from(model.context_window.max(1)))
                            .unwrap_or(u64::MAX);
                        limit.min(model_window).max(1)
                    })
                    .unwrap_or_else(|| self.effective_compaction_threshold_tokens()),
            ),
            theme_label: self.active_theme_name.clone(),
            reasoning_view_label: super::reasoning_view::reasoning_view_label(
                self.collapse_reasoning,
            )
            .to_string(),
        }
    }

    fn compaction_threshold_snapshot(&self) -> crate::bottom_pane::CompactionThresholdSnapshot {
        let model = self.session.model.as_ref();
        let context_window_tokens = model
            .map(|model| u64::from(model.context_window.max(1)))
            .unwrap_or(1);
        let model_effective = model
            .map(|model| u64::from(model.effective_context_window()))
            .unwrap_or(context_window_tokens);
        let recommended_token_limit = crate::bottom_pane::recommended_compaction_token_limit(
            context_window_tokens,
            model_effective,
        );
        // Current / hub label follow the applied (model-clamped) window so they
        // stay consistent with the status bar. The stored global preference is
        // kept separately for picker memory when this client initiated an update.
        let current_token_limit = self
            .effective_context_window
            .unwrap_or(model_effective)
            .min(context_window_tokens)
            .max(1);
        crate::bottom_pane::CompactionThresholdSnapshot {
            model_label: model
                .map(|model| model.slug.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            context_window_tokens,
            recommended_token_limit,
            current_token_limit,
        }
    }

    fn effective_compaction_threshold_tokens(&self) -> u64 {
        let model = self.session.model.as_ref();
        let model_window = model
            .map(|model| u64::from(model.context_window.max(1)))
            .unwrap_or(u64::MAX);
        let model_effective = model
            .map(|model| u64::from(model.effective_context_window()))
            .unwrap_or(1);
        self.effective_context_window
            .unwrap_or(model_effective)
            .min(model_window)
            .max(1)
    }

    pub(super) fn active_accent_color(&self) -> Color {
        self.theme_set
            .find(&self.active_theme_name)
            .map(|t| t.accent_color)
            .unwrap_or(Color::Cyan)
    }

    pub(super) fn active_error_color(&self) -> Color {
        self.theme_set
            .find(&self.active_theme_name)
            .map(|t| t.error_color)
            .unwrap_or(Color::Rgb(0xF8, 0x51, 0x49))
    }

    pub(super) fn apply_model_selection(&mut self, slug: String) {
        if let Some(entry) = self.saved_model_entry_for_selection(&slug).cloned() {
            let selected_model = self.apply_saved_model_entry_to_session(&entry);
            self.reasoning_effort_selection =
                reasoning_effort::current_reasoning_effort_selection_for_model(
                    &selected_model,
                    self.reasoning_effort_selection.as_deref(),
                );
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::override_turn_context(
                    /*cwd*/ None,
                    Some(Self::saved_model_selection_value(&entry).to_string()),
                    Some(self.reasoning_effort_selection.clone()),
                    /*sandbox*/ None,
                    /*approval_policy*/ None,
                )));
            self.set_status_message(format!(
                "Model set to {}",
                self.saved_model_display_label(&entry)
            ));
            return;
        }

        if let Some(selected_model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        {
            self.reasoning_effort_selection =
                reasoning_effort::current_reasoning_effort_selection_for_model(
                    &selected_model,
                    self.reasoning_effort_selection.as_deref(),
                );
            self.session.provider = Some(selected_model.provider);
            self.session.model = Some(selected_model.clone());
            self.session.request_model = None;
            self.session.model_binding_id = None;
            self.current_model_binding_id = None;
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::override_turn_context(
                    /*cwd*/ None,
                    Some(selected_model.slug.clone()),
                    Some(self.reasoning_effort_selection.clone()),
                    /*sandbox*/ None,
                    /*approval_policy*/ None,
                )));
            self.set_status_message(format!("Model set to {}", selected_model.slug));
            return;
        }

        self.update_session_request_model(slug.clone());
        self.reasoning_effort_selection = self.session.model.as_ref().and_then(|model| {
            reasoning_effort::current_reasoning_effort_selection_for_model(
                model,
                self.reasoning_effort_selection.as_deref(),
            )
        });
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::override_turn_context(
                /*cwd*/ None,
                Some(slug.clone()),
                Some(self.reasoning_effort_selection.clone()),
                /*sandbox*/ None,
                /*approval_policy*/ None,
            )));
        self.set_status_message(format!("Model set to {slug}"));
    }

    pub(super) fn apply_reasoning_effort_selection(&mut self, value: String) {
        self.reasoning_effort_selection = Some(value.clone());
        if let Some(model) = self.session.model.as_ref() {
            self.session.reasoning_effort = model
                .resolve_reasoning_effort_selection(Some(value.as_str()))
                .effective_reasoning_effort;
        }
        self.refresh_header_box();
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::override_turn_context(
                /*cwd*/ None,
                /*model*/ None,
                Some(Some(value.clone())),
                /*sandbox*/ None,
                /*approval_policy*/ None,
            )));
        self.set_status_message(format!("Reasoning effort set to {value}"));
    }
}
