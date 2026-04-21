use std::collections::HashMap;
use std::path::PathBuf;

use crate::v2::app_event::AppEvent;
use crate::v2::app_event_sender::AppEventSender;
use crate::v2::bottom_pane::MentionBinding;
use devo_protocol::user_input::TextElement;

struct DecodedHistoryMentions {
    text: String,
    mentions: Vec<DecodedHistoryMention>,
}

struct DecodedHistoryMention {
    mention: String,
    path: String,
}

fn decode_history_mentions(text: &str) -> DecodedHistoryMentions {
    DecodedHistoryMentions {
        text: text.to_string(),
        mentions: Vec::new(),
    }
}

/// A composer history entry that can rehydrate draft state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HistoryEntry {
    /// Raw text stored in history (may include placeholder strings).
    pub(crate) text: String,
    /// Text element ranges for placeholders inside `text`.
    pub(crate) text_elements: Vec<TextElement>,
    /// Local image paths captured alongside `text_elements`.
    pub(crate) local_image_paths: Vec<PathBuf>,
    /// Remote image URLs restored with this draft.
    pub(crate) remote_image_urls: Vec<String>,
    /// Mention bindings for tool/app/skill references inside `text`.
    pub(crate) mention_bindings: Vec<MentionBinding>,
    /// Placeholder-to-payload pairs used to restore large paste content.
    pub(crate) pending_pastes: Vec<(String, String)>,
}

impl HistoryEntry {
    pub(crate) fn new(text: String) -> Self {
        let decoded = decode_history_mentions(&text);
        Self {
            text: decoded.text,
            text_elements: Vec::new(),
            local_image_paths: Vec::new(),
            remote_image_urls: Vec::new(),
            mention_bindings: decoded
                .mentions
                .into_iter()
                .map(|mention| MentionBinding {
                    mention: mention.mention,
                    path: mention.path,
                })
                .collect(),
            pending_pastes: Vec::new(),
        }
    }

    #[allow(dead_code)]
    #[cfg(test)]
    pub(crate) fn with_pending(
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
        pending_pastes: Vec<(String, String)>,
    ) -> Self {
        Self {
            text,
            text_elements,
            local_image_paths,
            remote_image_urls: Vec::new(),
            mention_bindings: Vec::new(),
            pending_pastes,
        }
    }

    #[allow(dead_code)]
    #[cfg(test)]
    pub(crate) fn with_pending_and_remote(
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
        pending_pastes: Vec<(String, String)>,
        remote_image_urls: Vec<String>,
    ) -> Self {
        Self {
            text,
            text_elements,
            local_image_paths,
            remote_image_urls,
            mention_bindings: Vec::new(),
            pending_pastes,
        }
    }
}

/// State machine that manages shell-style history navigation (Up/Down) inside
/// the chat composer. This struct is intentionally decoupled from the
/// rendering widget so the logic remains isolated and easier to test.
pub(crate) struct ChatComposerHistory {
    /// Identifier of the history log as reported by `SessionConfiguredEvent`.
    history_log_id: Option<u64>,
    /// Number of entries already present in the persistent cross-session
    /// history file when the session started.
    history_entry_count: usize,

    /// Messages submitted by the user *during this UI session* (newest at END).
    /// Local entries retain full draft state (text elements, image paths, pending pastes, remote image URLs).
    local_history: Vec<HistoryEntry>,

    /// Cache of persistent history entries fetched on-demand (text-only).
    fetched_history: HashMap<usize, HistoryEntry>,

    /// Current cursor within the combined (persistent + local) history. `None`
    /// indicates the user is *not* currently browsing history.
    history_cursor: Option<isize>,

    /// The text that was last inserted into the composer as a result of
    /// history navigation. Used to decide if further Up/Down presses should be
    /// treated as navigation versus normal cursor movement, together with the
    /// "cursor at line boundary" check in [`Self::should_handle_navigation`].
    last_history_text: Option<String>,
}

impl ChatComposerHistory {
    pub fn new() -> Self {
        Self {
            history_log_id: None,
            history_entry_count: 0,
            local_history: Vec::new(),
            fetched_history: HashMap::new(),
            history_cursor: None,
            last_history_text: None,
        }
    }

    #[allow(dead_code)]
    /// Update metadata when a new session is configured.
    pub fn set_metadata(&mut self, log_id: u64, entry_count: usize) {
        self.history_log_id = Some(log_id);
        self.history_entry_count = entry_count;
        self.fetched_history.clear();
        self.local_history.clear();
        self.history_cursor = None;
        self.last_history_text = None;
    }

    /// Record a message submitted by the user in the current session so it can
    /// be recalled later.
    pub fn record_local_submission(&mut self, entry: HistoryEntry) {
        if entry.text.is_empty()
            && entry.text_elements.is_empty()
            && entry.local_image_paths.is_empty()
            && entry.remote_image_urls.is_empty()
            && entry.mention_bindings.is_empty()
            && entry.pending_pastes.is_empty()
        {
            return;
        }
        self.history_cursor = None;
        self.last_history_text = None;

        // Avoid inserting a duplicate if identical to the previous entry.
        if self.local_history.last().is_some_and(|prev| prev == &entry) {
            return;
        }

        self.local_history.push(entry);
    }

    #[allow(dead_code)]
    /// Reset navigation tracking so the next Up key resumes from the latest entry.
    pub fn reset_navigation(&mut self) {
        self.history_cursor = None;
        self.last_history_text = None;
    }

    /// Returns whether Up/Down should navigate history for the current textarea state.
    ///
    /// Empty text always enables history traversal. For non-empty text, this requires both:
    ///
    /// - the current text exactly matching the last recalled history entry, and
    /// - the cursor being at a line boundary (start or end).
    ///
    /// This boundary gate keeps multiline cursor movement usable while preserving shell-like
    /// history recall. If callers moved the cursor into the middle of a recalled entry and still
    /// forced navigation, users would lose normal vertical movement within the draft.
    pub fn should_handle_navigation(&self, text: &str, cursor: usize) -> bool {
        if self.history_entry_count == 0 && self.local_history.is_empty() {
            return false;
        }

        if text.is_empty() {
            return true;
        }

        // Textarea is not empty – only navigate when text matches the last
        // recalled history entry and the cursor is at a line boundary. This
        // keeps shell-like Up/Down recall working while still allowing normal
        // multiline cursor movement from interior positions.
        if cursor != 0 && cursor != text.len() {
            return false;
        }

        matches!(&self.last_history_text, Some(prev) if prev == text)
    }

    /// Handle <Up>. Returns true when the key was consumed and the caller
    /// should request a redraw.
    pub fn navigate_up(&mut self, app_event_tx: &AppEventSender) -> Option<HistoryEntry> {
        let total_entries = self.history_entry_count + self.local_history.len();
        if total_entries == 0 {
            return None;
        }

        let next_idx = match self.history_cursor {
            None => (total_entries as isize) - 1,
            Some(0) => return None, // already at oldest
            Some(idx) => idx - 1,
        };

        self.history_cursor = Some(next_idx);
        self.populate_history_at_index(next_idx as usize, app_event_tx)
    }

    /// Handle <Down>.
    pub fn navigate_down(&mut self, app_event_tx: &AppEventSender) -> Option<HistoryEntry> {
        let total_entries = self.history_entry_count + self.local_history.len();
        if total_entries == 0 {
            return None;
        }

        let next_idx_opt = match self.history_cursor {
            None => return None, // not browsing
            Some(idx) if (idx as usize) + 1 >= total_entries => None,
            Some(idx) => Some(idx + 1),
        };

        match next_idx_opt {
            Some(idx) => {
                self.history_cursor = Some(idx);
                self.populate_history_at_index(idx as usize, app_event_tx)
            }
            None => {
                // Past newest – clear and exit browsing mode.
                self.history_cursor = None;
                self.last_history_text = None;
                Some(HistoryEntry::new(String::new()))
            }
        }
    }

    #[allow(dead_code)]
    /// Integrate a GetHistoryEntryResponse event.
    pub fn on_entry_response(
        &mut self,
        log_id: u64,
        offset: usize,
        entry: Option<String>,
    ) -> Option<HistoryEntry> {
        if self.history_log_id != Some(log_id) {
            return None;
        }
        let entry = HistoryEntry::new(entry?);
        self.fetched_history.insert(offset, entry.clone());

        if self.history_cursor == Some(offset as isize) {
            self.last_history_text = Some(entry.text.clone());
            return Some(entry);
        }
        None
    }

    // ---------------------------------------------------------------------
    // Internal helpers
    // ---------------------------------------------------------------------

    fn populate_history_at_index(
        &mut self,
        global_idx: usize,
        app_event_tx: &AppEventSender,
    ) -> Option<HistoryEntry> {
        if global_idx >= self.history_entry_count {
            // Local entry.
            if let Some(entry) = self
                .local_history
                .get(global_idx - self.history_entry_count)
                .cloned()
            {
                self.last_history_text = Some(entry.text.clone());
                return Some(entry);
            }
        } else if let Some(entry) = self.fetched_history.get(&global_idx).cloned() {
            self.last_history_text = Some(entry.text.clone());
            return Some(entry);
        } else if let Some(log_id) = self.history_log_id {
            app_event_tx.send(AppEvent::HistoryEntryRequested {
                log_id,
                offset: global_idx,
            });
        }
        None
    }
}
