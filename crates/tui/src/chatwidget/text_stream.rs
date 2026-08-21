//! Active assistant/reasoning text stream lifecycle for `ChatWidget`.
//!
//! This module owns the ordering, live-cell synchronization, and final commit
//! behavior for streaming text items while `ChatWidget` keeps the actual state.

use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use devo_core::ItemId;
use ratatui::text::Span;

use crate::events::TextItemKind;
use crate::history_cell;
use crate::markdown::append_markdown;
use crate::streaming::commit_tick::CommitTickScope;
use crate::streaming::commit_tick::run_commit_tick;
use crate::streaming::controller::StreamController;

use super::ChatWidget;
use super::DotStatus;

pub(super) struct ActiveTextItem {
    pub(super) item_id: ActiveTextItemId,
    pub(super) kind: TextItemKind,
    pub(super) seq: u64,
    pub(super) status: DotStatus,
    pub(super) stream_controller: Option<StreamController>,
    last_renderable_delta_at: Option<Instant>,
    last_stream_commit_at: Option<Instant>,
    stream_stall_warned: bool,
    delta_seq: u64,
    raw_text: String,
    pub(super) cell: Option<Box<dyn history_cell::HistoryCell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveTextItemId {
    Server(ItemId),
    Legacy(TextItemKind),
}

impl ActiveTextItemId {
    pub(super) fn log_label(self) -> String {
        match self {
            Self::Server(item_id) => item_id.to_string(),
            Self::Legacy(kind) => format!("legacy-{kind:?}"),
        }
    }
}

impl ChatWidget {
    pub(super) fn commit_active_streams(&mut self, status: DotStatus) {
        tracing::debug!(
            status = ?status,
            active_items = ?self.active_text_item_log_order(),
            "committing all active text items"
        );
        for item in &self.active_text_items {
            if item.kind == TextItemKind::Assistant
                && let ActiveTextItemId::Server(item_id) = item.item_id
            {
                self.boundary_committed_assistant_items.insert(item_id);
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
            if let ActiveTextItemId::Server(item_id) = item.item_id {
                self.boundary_committed_assistant_items.insert(item_id);
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
        let stream_controller = if kind == TextItemKind::Assistant {
            Some(StreamController::new(None, &self.session.cwd))
        } else {
            None
        };
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
                stream_controller,
                last_renderable_delta_at: None,
                last_stream_commit_at: None,
                stream_stall_warned: false,
                delta_seq: 0,
                raw_text: String::new(),
                cell: None,
            },
        );
        tracing::trace!(
            after = ?self.active_text_item_log_order(),
            "active text item order after start"
        );
        self.stream_chunking_policy.reset();
    }

    pub(super) fn push_text_item_delta(
        &mut self,
        item_id: ActiveTextItemId,
        kind: TextItemKind,
        delta: &str,
    ) {
        let index = self.ensure_text_item(item_id, kind);
        let active_items = self.active_text_item_log_order();
        let active_cell_revision_before = self.active_cell_revision;
        let delta_seq = {
            let item = &mut self.active_text_items[index];
            item.delta_seq = item.delta_seq.saturating_add(1);
            item.delta_seq
        };
        let queued_lines_before = self.active_text_items[index]
            .stream_controller
            .as_ref()
            .map(StreamController::queued_lines);
        if let Some(assistant_token_text) = (kind == TextItemKind::Assistant)
            .then(|| assistant_token_log_preview(delta))
            .flatten()
        {
            tracing::debug!(
                stream_elapsed_ms = stream_trace_elapsed_ms(),
                item_id = %item_id.log_label(),
                kind = ?kind,
                delta_seq,
                delta_len = delta.len(),
                queued_lines_before = ?queued_lines_before,
                active_cell_revision_before,
                active_items = ?active_items,
                assistant_token_text = %assistant_token_text,
                "received active text item delta"
            );
        } else {
            tracing::debug!(
                stream_elapsed_ms = stream_trace_elapsed_ms(),
                item_id = %item_id.log_label(),
                kind = ?kind,
                delta_seq,
                delta_len = delta.len(),
                queued_lines_before = ?queued_lines_before,
                active_cell_revision_before,
                active_items = ?active_items,
                "received active text item delta"
            );
        }
        match kind {
            TextItemKind::Assistant => {
                if let Some(controller) = self.active_text_items[index].stream_controller.as_mut() {
                    let produced_renderable_lines = controller.push(delta);
                    if produced_renderable_lines {
                        let item = &mut self.active_text_items[index];
                        item.last_renderable_delta_at = Some(Instant::now());
                        item.stream_stall_warned = false;
                    }
                }
            }
            TextItemKind::Reasoning => {
                self.active_text_items[index].raw_text.push_str(delta);
            }
        }
        self.sync_text_item_cell(index);
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            item_id = %item_id.log_label(),
            kind = ?kind,
            delta_seq,
            queued_lines_after = ?self.active_text_items[index]
                .stream_controller
                .as_ref()
                .map(StreamController::queued_lines),
            active_cell_revision_after = self.active_cell_revision,
            "active text item delta synced"
        );
        self.frame_requester.schedule_frame();
    }

    pub(super) fn complete_text_item(
        &mut self,
        item_id: ActiveTextItemId,
        kind: TextItemKind,
        final_text: String,
    ) {
        let boundary_committed = matches!(
            (item_id, kind),
            (ActiveTextItemId::Server(item_id), TextItemKind::Assistant)
                if self.boundary_committed_assistant_items.contains(&item_id)
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
            self.active_text_items[index].raw_text = final_text;
        }
        self.sync_text_item_cell(index);
        self.commit_completed_text_items();
        if matches!(item_id, ActiveTextItemId::Server(_)) && kind == TextItemKind::Assistant {
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
        self.active_text_items
            .iter()
            .any(|item| matches!(item.item_id, ActiveTextItemId::Server(_)) && item.kind == kind)
    }

    #[cfg(test)]
    pub(crate) fn assistant_stream_queued_lines_for_test(&self) -> usize {
        self.active_text_items
            .iter()
            .filter(|item| item.kind == TextItemKind::Assistant)
            .filter_map(|item| item.stream_controller.as_ref())
            .map(StreamController::queued_lines)
            .sum()
    }

    fn commit_text_item_at(&mut self, index: usize, status: DotStatus) {
        if index >= self.active_text_items.len() {
            return;
        }

        let mut item = self.active_text_items.remove(index);
        tracing::debug!(
            item_id = %item.item_id.log_label(),
            kind = ?item.kind,
            status = ?status,
            remaining = ?self.active_text_item_log_order(),
            "committing active text item"
        );
        match item.kind {
            TextItemKind::Assistant => {
                if let Some(controller) = item.stream_controller.as_mut() {
                    let (_cell, source) = controller.finalize();
                    if let Some(source) = source {
                        self.add_assistant_markdown_source(source, status);
                    } else if !item.raw_text.trim().is_empty() {
                        self.add_markdown_history_with_status_without_redraw(
                            "Assistant",
                            &item.raw_text,
                            status,
                        );
                    }
                } else if !item.raw_text.trim().is_empty() {
                    self.add_markdown_history_with_status_without_redraw(
                        "Assistant",
                        &item.raw_text,
                        status,
                    );
                }
            }
            TextItemKind::Reasoning => {
                if !item.raw_text.trim().is_empty() {
                    if self.collapse_reasoning {
                        self.add_history_entry_without_redraw(
                            super::reasoning_view::collapsed_reasoning_history_cell(
                                item.raw_text,
                                &self.session.cwd,
                                "Thought: ",
                                Self::reasoning_completed_heading_style(),
                                Self::reasoning_text_style(),
                                Self::reasoning_completed_dot_prefix(),
                            ),
                        );
                    } else {
                        self.add_markdown_history_with_status("Reasoning", &item.raw_text, status);
                    }
                }
            }
        }
        self.stream_chunking_policy.reset();
    }

    fn add_assistant_markdown_source(&mut self, source: String, status: DotStatus) {
        if source.trim().is_empty() {
            return;
        }

        self.add_history_entry_without_redraw(Box::new(history_cell::AgentMarkdownCell::new(
            source,
            &self.session.cwd,
            self.dot_prefix(status),
            "  ",
        )));
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

    pub(super) fn run_stream_commit_tick(&mut self) {
        let now = Instant::now();
        let mut output_cells = Vec::new();
        let mut needs_followup = false;
        let mut changed_indexes = Vec::new();

        for (index, item) in self.active_text_items.iter_mut().enumerate() {
            let Some(controller) = item.stream_controller.as_mut() else {
                continue;
            };
            let queued_lines_before = controller.queued_lines();
            let output = run_commit_tick(
                &mut self.stream_chunking_policy,
                Some(controller),
                CommitTickScope::AnyMode,
                now,
            );
            let queued_lines_after = controller.queued_lines();
            let emitted_cells = output.cells.len();
            tracing::debug!(
                stream_elapsed_ms = stream_trace_elapsed_ms(),
                item_id = %item.item_id.log_label(),
                kind = ?item.kind,
                delta_seq = item.delta_seq,
                queued_lines_before,
                queued_lines_after,
                emitted_cells,
                all_idle = output.all_idle,
                "stream commit tick processed active text item"
            );
            if matches!(item.kind, TextItemKind::Assistant) {
                if !output.cells.is_empty() {
                    changed_indexes.push(index);
                    item.last_stream_commit_at = Some(now);
                    item.stream_stall_warned = false;
                } else if item.kind == TextItemKind::Assistant {
                    maybe_warn_stream_commit_stall(item, queued_lines_after, now);
                }
                if !output.all_idle {
                    needs_followup = true;
                }
                continue;
            }
            if !output.cells.is_empty() {
                output_cells.extend(output.cells);
                changed_indexes.push(index);
            }
            if !output.all_idle {
                needs_followup = true;
            }
        }

        for cell in output_cells {
            self.add_history_entry_without_redraw(cell);
        }
        for index in changed_indexes {
            self.sync_text_item_cell(index);
        }
        if needs_followup {
            self.frame_requester
                .schedule_frame_in(std::time::Duration::from_millis(16));
        }
        if !self.active_text_items.is_empty() {
            self.frame_requester.schedule_frame();
        }
    }

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
        if let Some(controller) = &item.stream_controller {
            let lines = controller.live_rendered_lines();
            if lines.iter().any(|line| !Self::is_blank_line(&line.line)) {
                return Some(Box::new(
                    history_cell::AgentMessageCell::new_with_rendered_lines(
                        lines,
                        Self::pending_dot_prefix(),
                        "  ",
                        false,
                    ),
                ));
            }
        } else if !item.raw_text.trim().is_empty() {
            return Some(Box::new(
                self.bulleted_markdown_cell(&item.raw_text, Self::pending_dot_prefix()),
            ));
        }
        None
    }

    fn reasoning_active_cell(
        &self,
        item: &ActiveTextItem,
    ) -> Option<Box<dyn history_cell::HistoryCell>> {
        if item.raw_text.trim().is_empty() {
            return None;
        }

        if self.collapse_reasoning {
            // Width-aware live window: cap by wrapped visual rows, not logical
            // newlines, so one long paragraph cannot blow past the budget.
            return Some(super::reasoning_view::collapsed_reasoning_live_cell(
                item.raw_text.clone(),
                &self.session.cwd,
                "Thinking: ",
                Self::reasoning_heading_style(),
                Self::reasoning_text_style(),
                Self::reasoning_dot_prefix(item.status),
            ));
        }

        let mut body_lines = Vec::new();
        append_markdown(
            &item.raw_text,
            None,
            Some(&self.session.cwd),
            &mut body_lines,
        );
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

fn maybe_warn_stream_commit_stall(item: &mut ActiveTextItem, queued_lines: usize, now: Instant) {
    if item.kind != TextItemKind::Assistant || item.stream_stall_warned || queued_lines == 0 {
        return;
    }
    let Some(last_renderable_delta_at) = item.last_renderable_delta_at else {
        return;
    };
    let threshold = stream_stall_warning_threshold();
    let age = now.saturating_duration_since(last_renderable_delta_at);
    if age < threshold {
        return;
    }
    tracing::warn!(
        stream_elapsed_ms = stream_trace_elapsed_ms(),
        item_id = %item.item_id.log_label(),
        queued_lines,
        stalled_ms = age.as_millis(),
        threshold_ms = threshold.as_millis(),
        last_stream_commit_age_ms = item
            .last_stream_commit_at
            .map(|last_commit| now.saturating_duration_since(last_commit).as_millis()),
        "assistant stream has queued renderable lines but no visible commit"
    );
    item.stream_stall_warned = true;
}

fn stream_stall_warning_threshold() -> Duration {
    static STREAM_STALL_WARNING_THRESHOLD: OnceLock<Duration> = OnceLock::new();
    *STREAM_STALL_WARNING_THRESHOLD.get_or_init(|| {
        std::env::var("DEVO_TUI_STREAM_STALL_WARN_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_millis(750))
    })
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

    use crate::events::TextItemKind;

    use super::ActiveTextItemId;
    use super::assistant_token_log_preview_with_enabled;
    use super::format_assistant_token_log_preview;

    #[test]
    fn legacy_text_item_id_log_label_includes_kind() {
        assert_eq!(
            ActiveTextItemId::Legacy(TextItemKind::Assistant).log_label(),
            "legacy-Assistant"
        );
    }

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
