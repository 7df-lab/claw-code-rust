use std::sync::OnceLock;
use std::time::Instant;

use devo_core::QueryEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryEventDeliveryPolicy {
    MustDeliver,
    BestEffort,
}

pub(super) fn query_event_delivery_policy(event: &QueryEvent) -> QueryEventDeliveryPolicy {
    match event {
        QueryEvent::ToolProgress {
            progress:
                devo_core::tools::ToolProgress::OutputDelta { .. }
                | devo_core::tools::ToolProgress::StatusUpdate { .. }
                | devo_core::tools::ToolProgress::Completion { .. },
            ..
        }
        | QueryEvent::UsageDelta { .. }
        | QueryEvent::Usage { .. } => QueryEventDeliveryPolicy::BestEffort,
        QueryEvent::ProviderRetryStatus(_)
        | QueryEvent::ContextCompactionStarted
        | QueryEvent::ContextCompactionCompleted
        | QueryEvent::ContextCompactionFailed { .. }
        | QueryEvent::TextDelta(_)
        | QueryEvent::ReasoningDelta(_)
        | QueryEvent::ReasoningCompleted
        | QueryEvent::ToolUseStart { .. }
        | QueryEvent::ToolExecutionStart { .. }
        | QueryEvent::ToolResult { .. }
        | QueryEvent::TurnComplete { .. } => QueryEventDeliveryPolicy::MustDeliver,
    }
}

pub(super) fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

pub(super) fn query_event_trace_kind(event: &QueryEvent) -> &'static str {
    match event {
        QueryEvent::ProviderRetryStatus(_) => "provider_retry_status",
        QueryEvent::ContextCompactionStarted => "context_compaction_started",
        QueryEvent::ContextCompactionCompleted => "context_compaction_completed",
        QueryEvent::ContextCompactionFailed { .. } => "context_compaction_failed",
        QueryEvent::TextDelta(_) => "text_delta",
        QueryEvent::ReasoningDelta(_) => "reasoning_delta",
        QueryEvent::ReasoningCompleted => "reasoning_completed",
        QueryEvent::ToolUseStart { .. } => "tool_use_start",
        QueryEvent::ToolExecutionStart { .. } => "tool_execution_start",
        QueryEvent::ToolResult { .. } => "tool_result",
        QueryEvent::ToolProgress { .. } => "tool_progress",
        QueryEvent::UsageDelta { .. } => "usage_delta",
        QueryEvent::Usage { .. } => "usage",
        QueryEvent::TurnComplete { .. } => "turn_complete",
    }
}

pub(super) fn query_event_trace_delta_len(event: &QueryEvent) -> usize {
    match event {
        QueryEvent::TextDelta(text) | QueryEvent::ReasoningDelta(text) => text.len(),
        QueryEvent::ToolProgress {
            progress:
                devo_core::tools::ToolProgress::OutputDelta { delta }
                | devo_core::tools::ToolProgress::StatusUpdate { message: delta, .. }
                | devo_core::tools::ToolProgress::Completion { summary: delta },
            ..
        } => delta.len(),
        QueryEvent::ProviderRetryStatus(_)
        | QueryEvent::ContextCompactionStarted
        | QueryEvent::ContextCompactionCompleted
        | QueryEvent::ContextCompactionFailed { .. }
        | QueryEvent::ReasoningCompleted
        | QueryEvent::ToolUseStart { .. }
        | QueryEvent::ToolExecutionStart { .. }
        | QueryEvent::ToolResult { .. }
        | QueryEvent::UsageDelta { .. }
        | QueryEvent::Usage { .. }
        | QueryEvent::TurnComplete { .. } => 0,
    }
}

pub(super) fn query_event_trace_token_preview(event: &QueryEvent) -> Option<String> {
    match event {
        QueryEvent::TextDelta(text) => assistant_token_log_preview(text),
        QueryEvent::ProviderRetryStatus(_)
        | QueryEvent::ContextCompactionStarted
        | QueryEvent::ContextCompactionCompleted
        | QueryEvent::ContextCompactionFailed { .. }
        | QueryEvent::ReasoningDelta(_)
        | QueryEvent::ReasoningCompleted
        | QueryEvent::ToolUseStart { .. }
        | QueryEvent::ToolExecutionStart { .. }
        | QueryEvent::ToolProgress { .. }
        | QueryEvent::ToolResult { .. }
        | QueryEvent::TurnComplete { .. }
        | QueryEvent::UsageDelta { .. }
        | QueryEvent::Usage { .. } => None,
    }
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKENS")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKENS_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|&max_chars| max_chars > 0)
            .unwrap_or(120)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        preview.push('…');
    }
    preview
}
