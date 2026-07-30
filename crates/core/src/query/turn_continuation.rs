//! Model-turn continuation policy for the agent loop.
//!
//! After a model response is assembled, the query loop asks this module whether
//! to execute local tools, inject a continuation message, end the turn, or fail.
//! Provider-specific quirks (DeepSeek thinking-only, residual DSML text) live here
//! so the main loop stays free of model-name branches.

use crate::AgentError;
use crate::ContentBlock;
use crate::Message;
use devo_protocol::HostedToolDefinition;
use devo_protocol::StopReason;
use devo_protocol::ToolDefinition;

pub(crate) const DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT: &str = "Your previous response contained only hidden reasoning and no user-visible answer. Provide the final answer to the user's original request now. Do not reveal or summarize hidden reasoning; return only user-visible content.";
const MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS: usize = 3;
const DSML_TEXT_TOOL_CALL_CONTINUATION_REMINDER: &str = "Your previous assistant message contained DSML tagged tool-call text. Those tags were emitted as ordinary text and no tool was executed. Do not repeat the DSML block. Continue now by using the provider's native hosted tool interface when you need a hosted tool, by invoking one of the available local tools when appropriate, or by producing normal prose if no tool is needed.";
const DSML_TOOL_CALL_MARKERS: [&str; 4] = [
    "<｜DSML｜tool_calls>",
    "<｜｜DSML｜｜tool_calls>",
    "<|DSML|tool_calls>",
    "<||DSML||tool_calls>",
];
const MAX_TOKENS_CONTINUATION_PROMPT: &str = "Please continue from where you left off.";

/// Snapshot of one completed model turn used to decide loop continuation.
pub(crate) struct ModelTurnSnapshot<'a> {
    pub stop_reason: Option<StopReason>,
    pub assistant_content: &'a [ContentBlock],
    pub has_visible_assistant_text: bool,
    pub has_local_tool_calls: bool,
    pub has_hosted_tool_uses: bool,
    pub has_provider_reasoning: bool,
    pub request_tools: &'a [ToolDefinition],
    pub hosted_tools: &'a [HostedToolDefinition],
}

/// Decision returned to the query loop after a model turn.
pub(crate) enum TurnContinuation {
    /// Local tool calls are present; caller should execute them.
    RunTools,
    /// Continue the query loop without injecting an extra message.
    Continue,
    /// Inject a user/system message, then continue the query loop.
    ContinueWithMessage(Message),
    /// No further model calls; optionally emit turn-complete.
    Complete { stop_reason: Option<StopReason> },
    /// Unrecoverable model/provider quirk.
    Fail(AgentError),
}

/// Stateful policy that owns per-turn counters for model quirks.
pub(crate) struct TurnContinuationPolicy {
    thinking_only_enabled: bool,
    thinking_only_used: bool,
    dsml_text_continuations: usize,
}

impl TurnContinuationPolicy {
    pub(crate) fn for_models(model_slug: &str, request_model: &str) -> Self {
        Self {
            thinking_only_enabled: model_enables_thinking_only_continuation(model_slug)
                || model_enables_thinking_only_continuation(request_model),
            thinking_only_used: false,
            dsml_text_continuations: 0,
        }
    }

    pub(crate) fn decide(&mut self, snap: ModelTurnSnapshot<'_>) -> TurnContinuation {
        if snap.has_local_tool_calls {
            return TurnContinuation::RunTools;
        }

        if self.thinking_only_enabled
            && snap.stop_reason == Some(StopReason::EndTurn)
            && !snap.has_visible_assistant_text
            && !snap.has_hosted_tool_uses
            && snap.has_provider_reasoning
        {
            if self.thinking_only_used {
                return TurnContinuation::Fail(AgentError::Provider(anyhow::anyhow!(
                    "deepseek-v4 returned thinking-only end_turn after continuation; no user-visible text was produced"
                )));
            }
            self.thinking_only_used = true;
            tracing::debug!(
                "deepseek-v4 returned thinking-only end_turn; injecting continuation prompt"
            );
            return TurnContinuation::ContinueWithMessage(Message::user(
                DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT,
            ));
        }

        if snap.has_hosted_tool_uses && snap.stop_reason == Some(StopReason::ToolUse) {
            tracing::debug!("hosted tool use returned without local calls, continuing query loop");
            return TurnContinuation::Continue;
        }

        if assistant_content_contains_dsml_tool_call_text(snap.assistant_content) {
            if self.dsml_text_continuations >= MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS {
                return TurnContinuation::Fail(AgentError::Provider(anyhow::anyhow!(
                    "provider returned DSML text tool calls {MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS} times without structured or hosted tool results"
                )));
            }
            self.dsml_text_continuations += 1;
            tracing::debug!(
                "DSML text tool call returned without structured tool result; continuing query loop"
            );
            return TurnContinuation::ContinueWithMessage(
                dsml_text_tool_call_continuation_message(snap.request_tools, snap.hosted_tools),
            );
        }

        if snap.stop_reason == Some(StopReason::MaxTokens) {
            tracing::debug!("max_tokens reached injecting continuation prompt");
            return TurnContinuation::ContinueWithMessage(Message::user(
                MAX_TOKENS_CONTINUATION_PROMPT,
            ));
        }

        TurnContinuation::Complete {
            stop_reason: snap.stop_reason,
        }
    }
}

fn model_enables_thinking_only_continuation(model: &str) -> bool {
    model.starts_with("deepseek-v4-")
}

pub(crate) fn assistant_content_has_visible_content(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Text { text }
        | ContentBlock::Reasoning { text }
        | ContentBlock::ToolResult { content: text, .. } => !text.trim().is_empty(),
        ContentBlock::ProviderReasoning { .. }
        | ContentBlock::ToolUse { .. }
        | ContentBlock::HostedToolUse { .. } => true,
    })
}

fn assistant_content_contains_dsml_tool_call_text(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Text { text } => DSML_TOOL_CALL_MARKERS
            .iter()
            .any(|marker| text.contains(marker)),
        ContentBlock::Reasoning { .. }
        | ContentBlock::ProviderReasoning { .. }
        | ContentBlock::ToolUse { .. }
        | ContentBlock::HostedToolUse { .. }
        | ContentBlock::ToolResult { .. } => false,
    })
}

fn dsml_text_tool_call_continuation_message(
    request_tools: &[ToolDefinition],
    hosted_tools: &[HostedToolDefinition],
) -> Message {
    let mut reminder = String::from("<system-reminder>\n");
    reminder.push_str(DSML_TEXT_TOOL_CALL_CONTINUATION_REMINDER);
    let local_tool_names = request_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    if !local_tool_names.is_empty() {
        reminder.push_str("\n\nAvailable local tools: ");
        reminder.push_str(&local_tool_names.join(", "));
        reminder.push('.');
    }
    let hosted_tool_names = hosted_tools
        .iter()
        .map(hosted_tool_name_for_reminder)
        .collect::<Vec<_>>();
    if !hosted_tool_names.is_empty() {
        reminder.push_str("\nAvailable hosted tools: ");
        reminder.push_str(&hosted_tool_names.join(", "));
        reminder.push_str(". Hosted tools must be invoked through provider-native server tool calls, not by writing DSML tags in text.");
    }
    if local_tool_names.contains(&"spawn_agent") && local_tool_names.contains(&"await_task") {
        reminder.push_str("\nFor research work with separable subtasks, prefer spawning independent agents first and then waiting for their results.");
    }
    reminder.push_str("\n</system-reminder>");
    Message::user(reminder)
}

fn hosted_tool_name_for_reminder(tool: &HostedToolDefinition) -> &'static str {
    match tool {
        HostedToolDefinition::WebSearch(_) => "web_search",
        HostedToolDefinition::WebFetch(_) => "web_fetch",
    }
}
