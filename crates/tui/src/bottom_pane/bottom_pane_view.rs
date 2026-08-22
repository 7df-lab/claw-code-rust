use crate::render::renderable::Renderable;
use crossterm::event::KeyEvent;

use super::CancellationEvent;
use super::ModelPickerSelection;
use super::resume_picker::ResumePickerAction;

/// Trait implemented by every view that can be shown in the bottom pane.
pub(crate) trait BottomPaneView: Renderable {
    /// Handle a key event while the view is active. A redraw is always
    /// scheduled after this call.
    fn handle_key_event(&mut self, _key_event: KeyEvent) {}

    /// Return `true` if the view has finished and should be removed.
    fn is_complete(&self) -> bool {
        false
    }

    #[allow(dead_code)]
    /// Stable identifier for views that need external refreshes while open.
    fn view_id(&self) -> Option<&'static str> {
        None
    }

    /// Approval identifier for overlays that may be resolved by another controller.
    fn approval_id(&self) -> Option<&str> {
        None
    }

    /// User-input request identifier for overlays that may be resolved by
    /// another controller or a lifecycle event.
    fn user_input_request_id(&self) -> Option<&str> {
        None
    }

    #[allow(dead_code)]
    /// Actual item index for list-based views that want to preserve selection
    /// across external refreshes.
    fn selected_index(&self) -> Option<usize> {
        None
    }

    fn take_model_selection(&mut self) -> Option<ModelPickerSelection> {
        None
    }

    fn take_theme_selection(&mut self) -> Option<String> {
        None
    }

    fn take_resume_action(&mut self) -> Option<ResumePickerAction> {
        None
    }

    fn update_resume_sessions(&mut self, _sessions: Vec<crate::events::SessionListEntry>) -> bool {
        false
    }

    fn update_resume_list_error(&mut self, _message: String) -> bool {
        false
    }

    fn update_resume_preview(
        &mut self,
        _session_id: devo_core::SessionId,
        _result: Result<Vec<crate::events::SessionPreviewMessage>, String>,
    ) -> bool {
        false
    }

    fn update_resume_rename(
        &mut self,
        _session_id: Option<devo_core::SessionId>,
        _result: Result<String, String>,
    ) -> bool {
        false
    }

    fn update_resume_delete(
        &mut self,
        _session_id: Option<devo_core::SessionId>,
        _result: Result<(), String>,
    ) -> bool {
        false
    }

    #[cfg(test)]
    fn resume_selection_for_test(&self) -> Option<usize> {
        None
    }

    #[cfg(test)]
    fn resume_scroll_offset_for_test(&self) -> Option<usize> {
        None
    }

    #[cfg(test)]
    fn resume_pending_delete_for_test(&self) -> Option<devo_core::SessionId> {
        None
    }

    /// Refresh Settings Hub rows when nested editors change current values.
    fn update_settings_hub_snapshot(
        &mut self,
        _snapshot: super::settings_hub_view::SettingsHubSnapshot,
    ) -> bool {
        false
    }

    /// Refresh an open `/status` panel when occupancy or session totals change.
    fn update_status_panel(
        &mut self,
        _occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
        _session: super::SessionTokenTotals,
    ) -> bool {
        false
    }

    /// Propagate the active UI accent into open views (for example Settings Hub tabs).
    fn set_accent_color(&mut self, _color: ratatui::style::Color) {}

    /// When true, the view replaces the composer input area instead of stacking
    /// below a visible draft.
    ///
    /// Views that replace the composer (`true`) should still go through
    /// [`super::selection_popup_common::render_menu_surface`] for consistent
    /// inset padding. Stacked overlays (`false`) may also use it when they need
    /// the same chrome (for example `/status`); otherwise they stay visually
    /// light so the draft input remains the primary surface.
    fn replaces_composer(&self) -> bool {
        false
    }

    /// Handle Ctrl-C while this view is active.
    fn on_ctrl_c(&mut self) -> CancellationEvent {
        CancellationEvent::NotHandled
    }

    /// Return true if Esc should be routed through `handle_key_event` instead
    /// of the `on_ctrl_c` cancellation path.
    fn prefer_esc_to_handle_key_event(&self) -> bool {
        false
    }

    #[allow(dead_code)]
    /// Optional paste handler. Return true if the view modified its state and
    /// needs a redraw.
    fn handle_paste(&mut self, _pasted: String) -> bool {
        false
    }

    #[allow(dead_code)]
    /// Flush any pending paste-burst state. Return true if state changed.
    ///
    /// This lets a modal that reuses `ChatComposer` participate in the same
    /// time-based paste burst flushing as the primary composer.
    fn flush_paste_burst_if_due(&mut self) -> bool {
        false
    }

    /// Whether the view is currently holding paste-burst transient state.
    ///
    /// When `true`, the bottom pane will schedule a short delayed redraw to
    /// give the burst time window a chance to flush.
    fn is_in_paste_burst(&self) -> bool {
        false
    }
}
