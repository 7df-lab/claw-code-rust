//! Active assistant/reasoning text view state for `ChatWidget`.
//!
//! Text bodies live in [`TranscriptProjector`] only; this module tracks ordering,
//! live-cell rendering, and commit-to-history behavior.

use std::sync::OnceLock;
use std::time::Instant;

use devo_core::ItemId;
use ratatui::text::Span;

use crate::events::TextItemKind;
use crate::history_cell;
use crate::markdown::append_markdown;
use crate::transcript::lifecycle::ItemLifecycleEvent;

use super::ChatWidget;
use super::DotStatus;

pub(super) struct ActiveTextItem {
    pub(super) item_id: ActiveTextItemId,
    pub(super) kind: TextItemKind,
    pub(super) seq: u64,
    pub(super) status: DotStatus,
    commit_text: Option<String>,
    pub(super) cell: Option<Box<dyn history_cell::HistoryCell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ActiveTextItemId(pub(crate) ItemId);

impl ActiveTextItemId {
    pub(super) fn item_id(self) -> ItemId {
        self.0
    }

    pub(super) fn log_label(self) -> String {
        self.0.to_string()
    }
}

impl ChatWidget {
    pub(super) fn is_legacy_text_item(&self, item_id: ItemId) -> bool {
        item_id == self.legacy_assistant_item_id || item_id == self.legacy_reasoning_item_id
    }

    pub(super) fn legacy_text_item_id(&self, kind: TextItemKind) -> ItemId {
        match kind {
            TextItemKind::Assistant => self.legacy_assistant_item_id,
            TextItemKind::Reasoning => self.legacy_reasoning_item_id,
        }
    }

    pub(super) fn live_text_body(&self, item_id: ItemId) -> &str {
        self.transcript_projector
            .live_text_for(item_id)
            .unwrap_or("")
    }

    pub(super) fn has_native_text_item(&self, kind: TextItemKind) -> bool {
        self.transcript_projector
            .live_text_items()
            .any(|live| live.kind == kind && !self.is_legacy_text_item(live.item_id))
    }

    pub(super) fn apply_legacy_text_delta(&mut self, kind: TextItemKind, delta: String) {
        if self.has_native_text_item(kind) {
            return;
        }
        self.flush_active_cell();
        let item_id = self.legacy_text_item_id(kind);
        if !self.transcript_projector.has_live_text(item_id) {
            self.apply_item_lifecycle(ItemLifecycleEvent::TextStarted { item_id, kind });
        }
        self.apply_item_lifecycle(ItemLifecycleEvent::TextDelta {
            item_id,
            kind,
            delta,
        });
    }

    pub(super) fn apply_legacy_text_completed(&mut self, kind: TextItemKind, final_text: String) {
        if self.has_native_text_item(kind) {
            return;
        }
        let item_id = self.legacy_text_item_id(kind);
        self.apply_item_lifecycle(ItemLifecycleEvent::TextCompleted {
            item_id,
            kind,
            final_text,
        });
    }
    pub(super) fn commit_active_streams(&mut self, status: DotStatus) {
        tracing::debug!(
            status = ?status,
            active_items = ?self.active_text_item_log_order(),
            "committing all active text items"
        );
        for item in &self.active_text_items {
            if item.kind == TextItemKind::Assistant
                && !self.is_legacy_text_item(item.item_id.item_id())
            {
                self.boundary_committed_assistant_items
                    .insert(item.item_id.item_id());
                self.committed_server_assistant_in_turn = true;
            }
        }
        while !self.active_text_items.is_empty() {
            self.commit_text_item_at(0, status);
        }
    }

    pub(super) fn commit_assistant_text_before_proposed_plan(&mut self) {
        let mut index = 0;
        while index < self.active_text_items.len() {
            let item = &self.active_text_items[index];
            if item.kind != TextItemKind::Assistant {
                index += 1;
                continue;
            }
            if !self.is_legacy_text_item(item.item_id.item_id()) {
                self.boundary_committed_assistant_items
                    .insert(item.item_id.item_id());
                self.committed_server_assistant_in_turn = true;
            }
            self.commit_text_item_at(index, DotStatus::Completed);
        }
        self.frame_requester.schedule_frame();
    }

    pub(super) fn start_text_item(&mut self, item_id: ActiveTextItemId, kind: TextItemKind) {
        if self
            .active_text_items
            .iter()
            .any(|item| item.item_id == item_id)
        {
            return;
        }

        if kind == TextItemKind::Reasoning {
            self.commit_completed_assistant_before_next_reasoning();
        }

        let seq = self.reserve_seq();
        let insert_index = self.active_text_item_insert_index(kind);
        tracing::debug!(
            item_id = %item_id.log_label(),
            kind = ?kind,
            insert_index,
            before = ?self.active_text_item_log_order(),
            "starting active text item"
        );
        self.active_text_items.insert(
            insert_index,
            ActiveTextItem {
                item_id,
                kind,
                seq,
                status: DotStatus::Pending,
                commit_text: None,
                cell: None,
            },
        );
        tracing::trace!(
            after = ?self.active_text_item_log_order(),
            "active text item order after start"
        );
    }

    pub(super) fn sync_live_text_item(&mut self, item_id: ActiveTextItemId) {
        let Some(index) = self
            .active_text_items
            .iter()
            .position(|item| item.item_id == item_id)
        else {
            return;
        };
        self.sync_text_item_cell(index);
    }

    pub(super) fn complete_text_item(
        &mut self,
        item_id: ActiveTextItemId,
        kind: TextItemKind,
        final_text: String,
    ) {
        let boundary_committed = matches!(
            (item_id, kind),
            (_, TextItemKind::Assistant)
                if self
                    .boundary_committed_assistant_items
                    .contains(&item_id.item_id())
                    && !self.is_legacy_text_item(item_id.item_id())
        );
        let index = if boundary_committed {
            let Some(index) = self
                .active_text_items
                .iter()
                .position(|item| item.item_id == item_id)
            else {
                self.committed_server_assistant_in_turn = true;
                return;
            };
            index
        } else {
            self.ensure_text_item(item_id, kind)
        };
        tracing::debug!(
            item_id = %item_id.log_label(),
            kind = ?kind,
            final_text_len = final_text.len(),
            active_items = ?self.active_text_item_log_order(),
            "completed active text item"
        );
        self.active_text_items[index].status = DotStatus::Completed;
        if !boundary_committed && !final_text.trim().is_empty() {
            self.active_text_items[index].commit_text = Some(final_text);
        }
        self.sync_text_item_cell(index);
        self.commit_completed_text_items();
        if !self.is_legacy_text_item(item_id.item_id()) && kind == TextItemKind::Assistant {
            self.committed_server_assistant_in_turn = true;
        }
    }

    fn ensure_text_item(&mut self, item_id: ActiveTextItemId, kind: TextItemKind) -> usize {
        if let Some(index) = self
            .active_text_items
            .iter()
            .position(|item| item.item_id == item_id)
        {
            return index;
        }

        self.start_text_item(item_id, kind);
        self.active_text_items
            .iter()
            .position(|item| item.item_id == item_id)
            .unwrap_or_else(|| self.active_text_items.len().saturating_sub(1))
    }

    pub(super) fn has_server_active_item(&self, kind: TextItemKind) -> bool {
        self.has_native_text_item(kind)
    }

    fn commit_text_item_at(&mut self, index: usize, status: DotStatus) {
        if index >= self.active_text_items.len() {
            return;
        }

        let mut item = self.active_text_items.remove(index);
        let body = item
            .commit_text
            .take()
            .unwrap_or_else(|| self.live_text_body(item.item_id.item_id()).to_string());
        self.transcript_projector
            .drop_live_text(item.item_id.item_id());
        tracing::debug!(
            item_id = %item.item_id.log_label(),
            kind = ?item.kind,
            status = ?status,
            remaining = ?self.active_text_item_log_order(),
            "committing active text item"
        );
        match item.kind {
            TextItemKind::Assistant => {
                if !body.trim().is_empty() {
                    self.add_markdown_history_with_status_without_redraw(
                        "Assistant",
                        &body,
                        status,
                    );
                }
            }
            TextItemKind::Reasoning => {
                if !body.trim().is_empty() {
                    if self.collapse_reasoning {
                        self.add_history_entry_without_redraw(
                            super::reasoning_view::collapsed_reasoning_history_cell(
                                body,
                                &self.session.cwd,
                                "Thought: ",
                                Self::reasoning_completed_heading_style(),
                                Self::reasoning_text_style(),
                                Self::reasoning_completed_dot_prefix(),
                            ),
                        );
                    } else {
                        self.add_markdown_history_with_status("Reasoning", &body, status);
                    }
                }
            }
        }
    }

    fn active_text_item_insert_index(&self, kind: TextItemKind) -> usize {
        match kind {
            TextItemKind::Reasoning => self
                .active_text_items
                .iter()
                .position(|item| item.kind == TextItemKind::Assistant)
                .unwrap_or(self.active_text_items.len()),
            TextItemKind::Assistant => self.active_text_items.len(),
        }
    }

    fn commit_completed_text_items(&mut self) {
        let mut index = 0;
        while index < self.active_text_items.len() {
            let item = &self.active_text_items[index];
            if item.status != DotStatus::Completed {
                index += 1;
                continue;
            }

            if item.kind == TextItemKind::Assistant
                && self.active_text_items[..index]
                    .iter()
                    .any(|prior| prior.kind == TextItemKind::Reasoning)
            {
                tracing::debug!(
                    item_id = %item.item_id.log_label(),
                    active_items = ?self.active_text_item_log_order(),
                    "deferring assistant commit until prior reasoning item commits"
                );
                index += 1;
                continue;
            }

            self.commit_text_item_at(index, DotStatus::Completed);
        }
    }

    fn commit_completed_assistant_before_next_reasoning(&mut self) {
        self.commit_completed_text_items();
        while let Some(assistant_index) = self.active_text_items.iter().position(|item| {
            item.kind == TextItemKind::Assistant && item.status == DotStatus::Completed
        }) {
            let Some(reasoning_index) =
                self.active_text_items[..assistant_index]
                    .iter()
                    .position(|item| {
                        item.kind == TextItemKind::Reasoning && item.status == DotStatus::Pending
                    })
            else {
                break;
            };
            self.commit_text_item_at(reasoning_index, DotStatus::Completed);
        }
        self.commit_completed_text_items();
    }

    fn active_text_item_log_order(&self) -> Vec<String> {
        self.active_text_items
            .iter()
            .map(|item| {
                format!(
                    "{:?}:{}:{:?}",
                    item.kind,
                    item.item_id.log_label(),
                    item.status
                )
            })
            .collect()
    }

    pub(super) fn run_stream_commit_tick(&mut self) {}

    pub(super) fn sync_text_item_cell(&mut self, index: usize) {
        if index >= self.active_text_items.len() {
            return;
        }

        let cell = match self.active_text_items[index].kind {
            TextItemKind::Assistant => self.assistant_active_cell(&self.active_text_items[index]),
            TextItemKind::Reasoning => self.reasoning_active_cell(&self.active_text_items[index]),
        };
        self.active_text_items[index].cell = cell;
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
    }

    fn assistant_active_cell(
        &self,
        item: &ActiveTextItem,
    ) -> Option<Box<dyn history_cell::HistoryCell>> {
        let body = self.live_text_body(item.item_id.item_id());
        if body.trim().is_empty() {
            return None;
        }
        Some(Box::new(
            self.bulleted_markdown_cell(body, Self::reply_dot_prefix()),
        ))
    }

    fn reasoning_active_cell(
        &self,
        item: &ActiveTextItem,
    ) -> Option<Box<dyn history_cell::HistoryCell>> {
        let body = self.live_text_body(item.item_id.item_id());
        if body.trim().is_empty() {
            return None;
        }

        if self.collapse_reasoning {
            return Some(super::reasoning_view::collapsed_reasoning_live_cell(
                body.to_string(),
                &self.session.cwd,
                "Thinking: ",
                Self::reasoning_heading_style(),
                Self::reasoning_text_style(),
                Self::reasoning_dot_prefix(item.status),
            ));
        }

        let mut body_lines = Vec::new();
        append_markdown(body, None, Some(&self.session.cwd), &mut body_lines);
        Self::patch_lines_style(&mut body_lines, Self::reasoning_text_style());
        if let Some(first_line) = body_lines.first_mut() {
            first_line.spans.insert(
                0,
                Span::styled("Thinking: ", Self::reasoning_heading_style()),
            );
        }
        Some(Box::new(
            history_cell::AgentMessageCell::new_ai_response_with_prefix(
                body_lines,
                Self::reasoning_dot_prefix(item.status),
                "  ",
                false,
            ),
        ))
    }
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

pub(super) fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_log_preview_with_enabled(
        text,
        assistant_token_logging_enabled(),
        assistant_token_log_max_chars(),
    )
}

fn assistant_token_log_preview_with_enabled(
    text: &str,
    enabled: bool,
    max_chars: usize,
) -> Option<String> {
    enabled.then(|| format_assistant_token_log_preview(text, max_chars))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    if let Some(preview) = ascii_log_preview_fast_path(text, max_chars) {
        return preview;
    }

    let escaped_capacity = max_chars
        .min(text.len())
        .saturating_mul(2)
        .saturating_add(3);
    let mut preview = String::with_capacity(escaped_capacity);
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

fn ascii_log_preview_fast_path(text: &str, max_chars: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let prefix_len = bytes.len().min(max_chars);
    if bytes[..prefix_len]
        .iter()
        .any(|byte| !matches!(*byte, b' '..=b'~') || matches!(*byte, b'\\' | b'\'' | b'"'))
    {
        return None;
    }

    if bytes.len() <= max_chars {
        return Some(text.to_string());
    }

    let mut preview = String::with_capacity(prefix_len + 3);
    preview.push_str(&text[..prefix_len]);
    preview.push_str("...");
    Some(preview)
}

fn single_line_text_preview(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::hint::black_box;
    use std::time::Instant;

    use super::assistant_token_log_preview_with_enabled;
    use super::format_assistant_token_log_preview;

    #[test]
    fn assistant_token_log_preview_escapes_and_truncates_text() {
        assert_eq!(
            format_assistant_token_log_preview("a\n\tbc", 3),
            "a\\n\\t..."
        );
    }

    #[test]
    fn assistant_token_log_preview_treats_zero_limit_as_one_char() {
        assert_eq!(format_assistant_token_log_preview("ab", 0), "a...");
    }

    #[test]
    fn assistant_token_log_preview_returns_none_when_disabled() {
        assert_eq!(
            assistant_token_log_preview_with_enabled("token", false, 10),
            None
        );
    }

    #[test]
    #[ignore]
    fn bench_assistant_token_log_preview_ascii_no_truncation() {
        let text = "assistant token delta text without escapes";
        let iterations = 500_000;
        let expected_len = format_assistant_token_log_preview(text, 128).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(format_assistant_token_log_preview(
                black_box(text),
                black_box(128),
            ))
            .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "assistant_token_log_preview_ascii_no_truncation iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_assistant_token_log_preview_escaped_truncation() {
        let text = "line\n\twith\\escapes and more text".repeat(64);
        let iterations = 200_000;
        let expected_len = format_assistant_token_log_preview(&text, 80).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(format_assistant_token_log_preview(
                black_box(&text),
                black_box(80),
            ))
            .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "assistant_token_log_preview_escaped_truncation iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
