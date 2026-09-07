//! Compaction — summarise conversation history via a separate LLM call
//! when the token budget is exceeded.
//!
//! Two compaction modes (`CompactionKind`) choose different preserve strategies:
//!
//! * **Auto** — token-budget threshold in the query loop (`query.rs`).
//!   Preserves a tail window of roughly `COMPACT_USER_MESSAGE_MAX_TOKENS`
//!   estimated tokens via `split_by_user_message_budget`, regardless of user
//!   message boundaries. Example: `[user1, asst1, user2, asst2, user3]` may
//!   become `[summary, asst2, user3]` when `asst2` and `user3` fit the tail
//!   budget but `user2` does not.
//! * **Proactive** — `/compact` or provider `context_too_long` retry.
//!   Preserves from the latest user message onward via
//!   `preserve_suffix_from_latest_user_message`. Example: the same history
//!   becomes `[summary, user3]` only.
//!
//! The compaction flow:
//!
//! 1. Filter out `Reason` items (reasoning text is not useful for summaries).
//! 2. Separate items into a "to‑compact" prefix and "to‑preserve" suffix.
//!    Auto uses a tail token budget; Proactive uses the latest-user suffix.
//! 3. Call the summarizer LLM with the `prompt.md` template appended as the
//!    last developer message after the to-compact history.
//! 4. Wrap the returned summary with the `summary_prefix.md` template.
//! 5. Build a new history: `[summary_msg, …preserved_items]`.
//! 6. If the summarizer LLM call fails with a context‑length error, move the
//!    newest to‑compact item back into the preserve set and retry.

use devo_protocol::approx_tokens_from_byte_count;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::context::ContextualUserFragment;
use crate::context::TokenBudget;
use crate::context::compaction_summary::CompactionSummary;
use crate::response_item::ResponseItem;

use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::RequestRole;
use devo_protocol::normalize_tool_result_messages;

use super::TokenInfo;

const SUMMARIZATION_PROMPT: &str = include_str!("../../prompts/compact/prompt.md");
/// Tail preserve budget for [`CompactionKind::Auto`]: walk backward from the
/// end of history and keep items until this estimated-token budget is full.
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

// ---------------------------------------------------------------------------
// CompactionError
// ---------------------------------------------------------------------------

/// Errors that can occur during history compaction.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum CompactionError {
    /// The summarizer provider call failed.
    #[error("summarization failed: {message}")]
    SummarizationFailed {
        /// Human-readable failure description.
        message: String,
    },
    /// The summarizer's context window was exceeded by the input.
    #[error("summarizer context window exceeded")]
    ContextTooLong,
    /// The summarizer returned an empty response.
    #[error("summarizer returned empty response")]
    EmptyResponse,
    /// Compaction was canceled by the caller.
    #[error("compaction canceled")]
    Canceled,
    /// Compaction is not possible after exhausting retries.
    #[error("compaction not possible after {retries} retries")]
    NotPossible {
        /// Number of retries attempted.
        retries: u32,
    },
}

// ---------------------------------------------------------------------------
// HistorySummarizer trait
// ---------------------------------------------------------------------------

/// Pluggable interface for the LLM call that produces a compaction summary.
///
/// Implementations are provided by the caller (e.g. the query loop) so that
/// this module does not depend directly on a specific provider SDK.
#[async_trait]
pub trait HistorySummarizer: Send + Sync {
    /// Send `messages` (to-compact history followed by a developer compaction
    /// prompt) to the model and return the generated summary text.
    ///
    /// When `cancel_token` is set, implementations should return
    /// [`CompactionError::Canceled`] promptly if it fires while the provider
    /// call is in flight (typically by racing the request against
    /// `cancel_token.cancelled()`).
    async fn summarize(
        &self,
        messages: Vec<RequestMessage>,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<String, CompactionError>;
}

// ---------------------------------------------------------------------------
// CompactionConfig
// ---------------------------------------------------------------------------

/// Configuration for the compaction process.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Token budget used to decide whether compaction is needed.
    pub budget: TokenBudget,
    /// How compaction was triggered — automatic or proactive.
    pub kind: CompactionKind,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            budget: TokenBudget::default(),
            kind: CompactionKind::Auto,
        }
    }
}

// ---------------------------------------------------------------------------
// CompactionKind — how compaction was triggered
// ---------------------------------------------------------------------------

/// Whether compaction was triggered automatically or proactively.
///
/// Call-site mapping:
/// - [`CompactionKind::Auto`]: `query.rs` token-budget threshold before a turn.
/// - [`CompactionKind::Proactive`]: `/compact` (`server/.../compaction.rs`) and
///   provider `context_too_long` retry in `query.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionKind {
    /// Automatic compaction when context pressure is high.
    ///
    /// Skips when [`should_compact`] says the session is already within budget.
    /// Preserve strategy: `split_by_user_message_budget` over the tail
    /// `COMPACT_USER_MESSAGE_MAX_TOKENS` window (items, not user turns).
    Auto,
    /// Forced compaction that always runs.
    ///
    /// Preserve strategy: `preserve_suffix_from_latest_user_message` — from
    /// the last `Role::User` item through the end of history.
    Proactive,
}

// ---------------------------------------------------------------------------
// CompactAction — describes how to act on a compaction decision
// ---------------------------------------------------------------------------

/// Describes the outcome of a single compaction attempt.
#[derive(Debug)]
pub enum CompactAction {
    /// Compaction succeeded, yielding items to replace the history.
    Replaced(Vec<ResponseItem>),
    /// Compaction was not needed (history is within budget).
    Skipped,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Determine whether compaction should run for the given token info.
pub fn should_compact(token_info: &TokenInfo, budget: &TokenBudget) -> bool {
    if token_info.input_tokens == 0 && token_info.cached_input_tokens == 0 {
        return false;
    }
    let current = token_info
        .input_tokens
        .saturating_add(token_info.cached_input_tokens)
        .saturating_add(token_info.output_tokens);
    budget.should_compact(current)
}

/// Compact the history using an LLM-backed summarizer.
pub async fn compact_history(
    items: &[ResponseItem],
    token_info: &TokenInfo,
    summarizer: &dyn HistorySummarizer,
    config: &CompactionConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<CompactAction, CompactionError> {
    // For auto compaction, skip if already within budget.
    // Proactive compaction always proceeds regardless of budget.
    if config.kind == CompactionKind::Auto && !should_compact(token_info, &config.budget) {
        return Ok(CompactAction::Skipped);
    }

    // Never normalize away pending intents or sever a completed tool batch.
    let proposed = if config.kind == CompactionKind::Proactive {
        items.len() - preserve_suffix_from_latest_user_message(items).len()
    } else {
        split_by_user_message_budget(items, COMPACT_USER_MESSAGE_MAX_TOKENS)
            .0
            .len()
    };
    let boundary = super::transactions::preserve_boundary(items, proposed);
    let mut to_compact = items[..boundary].to_vec();
    let mut preserved = items[boundary..].to_vec();

    if to_compact.is_empty() {
        // Nothing to compact — everything is within the preserve budget.
        return Ok(CompactAction::Skipped);
    }

    // 3. Attempt compaction with retry.
    //
    //    * The summarizer LLM may fail with `ContextTooLong` when the
    //      formatted history text exceeds its context window.  In that case
    //      we move the newest to‑compact item into the preserve set
    //      (reducing what the summarizer has to process) and retry
    //      immediately — this always converges because `to_compact`
    //      shrinks with each iteration.
    //    * Other errors (network blips, rate limits) are retried with
    //      exponential backoff up to 5 attempts.
    let mut transient_retries = 0u32;
    const MAX_TRANSIENT_RETRIES: u32 = 5;

    loop {
        if cancel_token.is_some_and(CancellationToken::is_cancelled) {
            return Err(CompactionError::Canceled);
        }

        let messages = summarizer_request_messages(&to_compact);

        let summary = match summarizer.summarize(messages, cancel_token).await {
            Ok(s) => s,
            Err(CompactionError::Canceled) => return Err(CompactionError::Canceled),
            Err(CompactionError::ContextTooLong) => {
                if to_compact.is_empty() {
                    // All items were moved to preserve — nothing to compact.
                    return Ok(CompactAction::Skipped);
                }
                // Move the newest to‑compact item into the preserve set
                // so the summarizer receives less input on the next try.
                let boundary =
                    super::transactions::preserve_boundary(&to_compact, to_compact.len() - 1);
                let mut moved = to_compact.split_off(boundary);
                moved.append(&mut preserved);
                preserved = moved;
                if to_compact.is_empty() {
                    return Ok(CompactAction::Skipped);
                }
                continue;
            }
            Err(e) => {
                transient_retries += 1;
                if transient_retries >= MAX_TRANSIENT_RETRIES {
                    return Err(e);
                }
                // Exponential backoff: 2^(retries) * 100ms
                let delay = Duration::from_millis(100 * (1 << transient_retries));
                sleep_cancellable(delay, cancel_token).await?;
                continue;
            }
        };

        if summary.trim().is_empty() {
            return Err(CompactionError::EmptyResponse);
        }

        // Build the compacted items using CompactionSummary.
        let summary_fragment = CompactionSummary::new(summary);
        let mut compacted = Vec::with_capacity(preserved.len().saturating_add(1));
        compacted.push(summary_fragment.to_response_item());
        compacted.extend(preserved);

        return Ok(CompactAction::Replaced(compacted));
    }
}

async fn sleep_cancellable(
    delay: Duration,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), CompactionError> {
    let Some(cancel_token) = cancel_token else {
        sleep(delay).await;
        return Ok(());
    };
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => Err(CompactionError::Canceled),
        () = sleep(delay) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Splits items into a "to compact" prefix and a "to preserve" suffix.
///
/// Used by [`CompactionKind::Auto`]. Walks backward from the end, accumulating
/// per-item token estimates until `budget_tokens` would be exceeded.
///
/// # Example
///
/// History `[user1, asst1, user2, asst2(large), user3]` with a small budget
/// that fits only `asst2` and `user3`:
/// - `to_compact` = `[user1, asst1, user2]`
/// - `preserve` = `[asst2, user3]`
/// - result after compaction = `[summary, asst2, user3]`
///
/// Items are [`ResponseItem`] records (messages, tool calls, tool outputs), not
/// whole turns. At least the last item is preserved when history is non-empty.
fn split_by_user_message_budget(
    items: &[ResponseItem],
    budget_tokens: usize,
) -> (Vec<ResponseItem>, Vec<ResponseItem>) {
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut used_tokens: usize = 0;
    let mut preserve_from: usize = 0;

    for i in (0..items.len()).rev() {
        let item_tokens = estimate_item_tokens(&items[i]);

        if used_tokens + item_tokens > budget_tokens && i + 1 < items.len() {
            // Budget exhausted — split after this item.
            // Only split if there would still be something to preserve.
            preserve_from = i + 1;
            break;
        }

        used_tokens += item_tokens;
        preserve_from = i;
    }

    if preserve_from == 0 {
        // Everything fits in the preserve budget (or the budget is very
        // large) — nothing to compact.
        (Vec::new(), items.to_vec())
    } else {
        let compact = items[..preserve_from].to_vec();
        let preserve = items[preserve_from..].to_vec();
        (compact, preserve)
    }
}

fn summarizer_request_messages(to_compact: &[ResponseItem]) -> Vec<RequestMessage> {
    let mut messages: Vec<RequestMessage> = to_compact.iter().map(RequestMessage::from).collect();
    merge_consecutive_assistant_messages(&mut messages);
    normalize_tool_result_messages(&mut messages);
    messages.push(RequestMessage {
        role: RequestRole::Developer.as_str().to_string(),
        content: vec![RequestContent::Text {
            text: SUMMARIZATION_PROMPT.trim().to_string(),
        }],
    });
    messages
}

/// Preserves history from the latest user message through the end.
///
/// Used by [`CompactionKind::Proactive`] (`/compact` and `context_too_long`
/// retry). Assistant and tool items after that user are kept; everything
/// before the user is summarized.
///
/// # Example
///
/// History `[user1, asst1, user2, asst2, user3]` always yields:
/// - `preserve` = `[user3]`
/// - result after compaction = `[summary, user3]`
///
/// Returns an empty vector when no user message exists.
fn preserve_suffix_from_latest_user_message(items: &[ResponseItem]) -> Vec<ResponseItem> {
    let Some(latest_user_index) = items.iter().rposition(
        |item| matches!(item, ResponseItem::Message(msg) if msg.role == devo_protocol::Role::User),
    ) else {
        return Vec::new();
    };

    items[latest_user_index..].to_vec()
}

/// Merges consecutive assistant `RequestMessage`s into a single message by
/// concatenating their content arrays.
///
/// This mirrors the same logic in `history/mod.rs` and is needed because
/// `ResponseItem` → `RequestMessage` conversion can split a single assistant
/// turn (text + tool calls) into multiple consecutive assistant messages,
/// which provider APIs reject.
fn merge_consecutive_assistant_messages(messages: &mut Vec<RequestMessage>) {
    let assistant_role = "assistant";
    let capacity = messages.len();
    let previous = std::mem::replace(messages, Vec::with_capacity(capacity));
    for mut message in previous {
        match messages.last_mut() {
            Some(last) if last.role == assistant_role && message.role == assistant_role => {
                last.content.append(&mut message.content);
            }
            _ => messages.push(message),
        }
    }
}

struct JsonByteCounter {
    bytes: usize,
}

impl std::io::Write for JsonByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Estimates the byte-length-based token count for a single item.
fn estimate_item_tokens(item: &ResponseItem) -> usize {
    let bytes = match item {
        ResponseItem::Reason { text } => text.len(),
        ResponseItem::Message(msg) => {
            let mut bytes = 0;
            let mut text_blocks = 0;
            for block in &msg.content {
                let text = match block {
                    devo_protocol::ContentBlock::Text { text }
                    | devo_protocol::ContentBlock::Reasoning { text } => text,
                    devo_protocol::ContentBlock::ProviderReasoning { .. }
                    | devo_protocol::ContentBlock::ToolUse { .. }
                    | devo_protocol::ContentBlock::HostedToolUse { .. }
                    | devo_protocol::ContentBlock::ToolResult { .. } => continue,
                };
                if text_blocks > 0 {
                    bytes += 1;
                }
                bytes += text.len();
                text_blocks += 1;
            }
            bytes
        }
        ResponseItem::ToolCall { name, input, .. } => {
            let mut counter = JsonByteCounter { bytes: 0 };
            serde_json::to_writer(&mut counter, input)
                .expect("serializing serde_json::Value to byte counter should not fail");
            name.len() + 2 + counter.bytes
        }
        ResponseItem::ToolCallOutput { content, .. } => content.len(),
    };
    approx_tokens_from_byte_count(bytes)
        .try_into()
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use devo_protocol::Message;
    use devo_protocol::RequestContent;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::context::TokenBudget;
    use crate::response_item::ResponseItem;

    #[test]
    fn compact_builds_merged_messages() {
        let items = [
            ResponseItem::Message(Message::user("hello")),
            // Split assistant turn — text + two tool calls
            ResponseItem::Message(Message::assistant_text("ok")),
            ResponseItem::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                input: serde_json::json!({"filePath": "/tmp/a.txt"}),
            },
            ResponseItem::ToolCall {
                id: "call-2".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "date"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".into(),
                content: "content".into(),
                is_error: false,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-2".into(),
                content: "output".into(),
                is_error: false,
            },
        ];

        let mut messages: Vec<RequestMessage> = items.iter().map(RequestMessage::from).collect();
        merge_consecutive_assistant_messages(&mut messages);

        // user, assistant(merged), user, user
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content.len(), 1);

        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content.len(), 3);
        assert!(matches!(
            &messages[1].content[0],
            RequestContent::Text { .. }
        ));
        assert!(
            matches!(&messages[1].content[1], RequestContent::ToolUse { id, .. } if id == "call-1")
        );
        assert!(
            matches!(&messages[1].content[2], RequestContent::ToolUse { id, .. } if id == "call-2")
        );

        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].content.len(), 1);
        assert!(
            matches!(&messages[2].content[0], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-1")
        );
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].content.len(), 1);
        assert!(
            matches!(&messages[3].content[0], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-2")
        );
    }

    #[test]
    fn compaction_summarizer_messages_put_prompt_last() {
        let items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::Message(Message::assistant_text("ok")),
            ResponseItem::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                input: serde_json::json!({"filePath": "/tmp/a.txt"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".into(),
                content: "content".into(),
                is_error: false,
            },
        ];

        let messages = summarizer_request_messages(&items);
        let last = messages
            .last()
            .expect("summarizer messages should not be empty");

        assert_eq!(last.role, RequestRole::Developer.as_str());
        assert!(
            matches!(&last.content[0], RequestContent::Text { text } if text.contains("CONTEXT CHECKPOINT COMPACTION"))
        );
        assert_ne!(messages[0].role, RequestRole::Developer.as_str());
    }

    #[test]
    fn should_compact_false_when_no_tokens() {
        let info = TokenInfo::default();
        let budget = TokenBudget::new(200_000, 8192);
        assert!(!should_compact(&info, &budget));
    }

    #[test]
    fn split_by_user_message_budget_all_preserved() {
        let items = vec![
            ResponseItem::Message(Message::user("short")),
            ResponseItem::Message(Message::assistant_text("ok")),
        ];
        let (compact, preserve) = split_by_user_message_budget(&items, 10_000);
        assert!(compact.is_empty());
        assert_eq!(preserve.len(), 2);
    }

    #[test]
    fn split_by_user_message_budget_boundary() {
        let items = vec![
            ResponseItem::Message(Message::user("a".repeat(400))), // ~100 tokens
            ResponseItem::Message(Message::assistant_text("b".repeat(400))),
            ResponseItem::Message(Message::user("c".repeat(400))),
            ResponseItem::Message(Message::assistant_text("d".repeat(400))),
        ];

        // Budget enough for about 2 items.
        let (compact, preserve) = split_by_user_message_budget(&items, 200);
        assert!(!preserve.is_empty());

        // The preserved part should be the tail.
        assert_eq!(preserve.len() + compact.len(), items.len());
    }

    #[tokio::test]
    async fn auto_compaction_preserves_tail_by_token_budget_not_latest_user_only() {
        struct StubSummarizer;

        #[async_trait]
        impl HistorySummarizer for StubSummarizer {
            async fn summarize(
                &self,
                _messages: Vec<RequestMessage>,
                _cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                Ok("summary".to_string())
            }
        }

        let large_tail = "x".repeat(40_000);
        let items = vec![
            ResponseItem::Message(Message::user("old user")),
            ResponseItem::Message(Message::assistant_text("old assistant")),
            ResponseItem::Message(Message::user(large_tail.clone())),
            ResponseItem::Message(Message::assistant_text(large_tail)),
            ResponseItem::Message(Message::user("latest user")),
        ];
        let token_info = TokenInfo {
            input_tokens: 200,
            cached_input_tokens: 0,
            output_tokens: 0,
        };
        let config = CompactionConfig {
            budget: TokenBudget {
                auto_compact_token_limit: Some(100),
                ..TokenBudget::new(200_000, 8192)
            },
            kind: CompactionKind::Auto,
        };

        let action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &config,
            /*cancel_token*/ None,
        )
        .await
        .expect("auto compaction should succeed");

        let proactive_config = CompactionConfig {
            budget: config.budget.clone(),
            kind: CompactionKind::Proactive,
        };
        let proactive_action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &proactive_config,
            /*cancel_token*/ None,
        )
        .await
        .expect("proactive compaction should succeed");

        match (action, proactive_action) {
            (
                CompactAction::Replaced(auto_compacted),
                CompactAction::Replaced(proactive_compacted),
            ) => {
                assert_eq!(
                    proactive_compacted[1..],
                    [ResponseItem::Message(Message::user("latest user"))]
                );
                assert!(
                    auto_compacted.len() > proactive_compacted.len(),
                    "auto compaction should preserve more tail items than proactive latest-user suffix"
                );
                assert_eq!(
                    auto_compacted.last(),
                    Some(&ResponseItem::Message(Message::user("latest user")))
                );
                assert!(
                    auto_compacted.iter().any(|item| {
                        matches!(
                            item,
                            ResponseItem::Message(msg)
                                if msg.role == devo_protocol::Role::Assistant
                        )
                    }),
                    "auto compaction should preserve assistant tail items beyond the latest user message"
                );
            }
            _ => panic!("expected both compaction modes to replace history"),
        }
    }

    /// Trace: L2-DES-AGENT-002
    /// Verifies: cancel token aborts an in-flight history summarizer.
    #[tokio::test]
    async fn compact_history_returns_canceled_when_token_fires() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;

        struct HangingSummarizer {
            entered: Arc<AtomicBool>,
        }

        #[async_trait]
        impl HistorySummarizer for HangingSummarizer {
            async fn summarize(
                &self,
                _messages: Vec<RequestMessage>,
                cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                self.entered.store(true, Ordering::SeqCst);
                let Some(cancel_token) = cancel_token else {
                    std::future::pending::<()>().await;
                    unreachable!("summarizer should be canceled")
                };
                cancel_token.cancelled().await;
                Err(CompactionError::Canceled)
            }
        }

        let items = vec![
            ResponseItem::Message(Message::user("first")),
            ResponseItem::Message(Message::assistant_text("reply")),
            ResponseItem::Message(Message::user("latest")),
        ];
        let token_info = TokenInfo {
            input_tokens: 10,
            cached_input_tokens: 0,
            output_tokens: 5,
        };
        let config = CompactionConfig {
            budget: TokenBudget::new(200_000, 8192),
            kind: CompactionKind::Proactive,
        };
        let entered = Arc::new(AtomicBool::new(false));
        let summarizer = HangingSummarizer {
            entered: Arc::clone(&entered),
        };
        let cancel_token = CancellationToken::new();
        let cancel_for_task = cancel_token.clone();
        let compact = tokio::spawn(async move {
            compact_history(
                &items,
                &token_info,
                &summarizer,
                &config,
                Some(&cancel_for_task),
            )
            .await
        });

        for _ in 0..50 {
            if entered.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            entered.load(Ordering::SeqCst),
            "summarizer should start before cancel"
        );
        cancel_token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), compact)
            .await
            .expect("compaction should finish after cancel")
            .expect("compaction task should not panic");
        assert!(matches!(result, Err(CompactionError::Canceled)));
    }

    #[tokio::test]
    async fn proactive_compaction_summarizes_all_history_and_preserves_latest_user_suffix() {
        struct StubSummarizer;

        #[async_trait]
        impl HistorySummarizer for StubSummarizer {
            async fn summarize(
                &self,
                messages: Vec<RequestMessage>,
                _cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                assert_eq!(messages.len(), 3);
                let last = messages
                    .last()
                    .expect("summarizer messages should not be empty");
                assert_eq!(last.role, RequestRole::Developer.as_str());
                assert!(
                    matches!(&last.content[0], RequestContent::Text { text } if text.contains("CONTEXT CHECKPOINT COMPACTION"))
                );
                Ok("summary".to_string())
            }
        }

        let items = vec![
            ResponseItem::Message(Message::user("first")),
            ResponseItem::Message(Message::assistant_text("reply")),
            ResponseItem::Message(Message::user("latest user")),
        ];
        let token_info = TokenInfo {
            input_tokens: 10,
            cached_input_tokens: 0,
            output_tokens: 5,
        };
        let config = CompactionConfig {
            budget: TokenBudget::new(200_000, 8192),
            kind: CompactionKind::Proactive,
        };

        let action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &config,
            /*cancel_token*/ None,
        )
        .await
        .expect("proactive compaction should succeed");

        match action {
            CompactAction::Replaced(compacted) => {
                assert_eq!(compacted.len(), 2);
                assert!(matches!(compacted[0], ResponseItem::Message(_)));
                assert_eq!(
                    compacted[1],
                    ResponseItem::Message(Message::user("latest user"))
                );
            }
            CompactAction::Skipped => panic!("expected proactive compaction to replace history"),
        }
    }

    #[test]
    fn compact_action_debug() {
        let action = CompactAction::Skipped;
        assert_eq!(format!("{:?}", action), "Skipped");
    }

    #[test]
    fn estimate_item_tokens_for_different_variants() {
        let reason = ResponseItem::Reason {
            text: "thinking deeply".into(),
        };
        assert!(estimate_item_tokens(&reason) > 0);

        let msg = ResponseItem::Message(Message::user("hello world"));
        assert!(estimate_item_tokens(&msg) > 0);

        let tc = ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        assert!(estimate_item_tokens(&tc) > 0);

        let tco = ResponseItem::ToolCallOutput {
            tool_use_id: "tc-1".into(),
            content: "done".into(),
            is_error: false,
        };
        assert!(estimate_item_tokens(&tco) > 0);
    }

    #[test]
    fn estimate_item_tokens_for_tool_call_matches_serialized_input_len() {
        let input = serde_json::json!({
            "cmd": "printf 'hello'",
            "cwd": "/tmp",
            "timeout_ms": 1000,
            "labels": ["a", "b"]
        });
        let item = ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "shell_command".into(),
            input: input.clone(),
        };

        let expected_bytes = "shell_command".len() + 2 + input.to_string().len();
        assert_eq!(
            estimate_item_tokens(&item),
            approx_tokens_from_byte_count(expected_bytes) as usize
        );
    }

    #[test]
    #[ignore]
    fn bench_estimate_item_tokens_for_tool_calls() {
        let items = (0..500)
            .map(|index| ResponseItem::ToolCall {
                id: format!("tc-{index}"),
                name: "shell_command".into(),
                input: serde_json::json!({
                    "cmd": "rg --json \"ResponseItem::ToolCall\" crates/core/src -g !target",
                    "cwd": "/Users/tsiao/Desktop/devo-opt",
                    "timeout_ms": 10000,
                    "metadata": {
                        "index": index,
                        "capture_stdout": true,
                        "capture_stderr": true,
                        "labels": [
                            "history",
                            "compaction",
                            "token-estimate",
                            "tool-call"
                        ]
                    }
                }),
            })
            .collect::<Vec<_>>();

        let started = Instant::now();
        let mut total_tokens = 0;
        for _ in 0..2_000 {
            for item in &items {
                total_tokens += black_box(estimate_item_tokens(black_box(item)));
            }
        }
        let elapsed = started.elapsed();

        assert!(total_tokens > 0);
        println!(
            "estimate_item_tokens_for_tool_calls iterations=1000000 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 1_000_000.0
        );
    }
}
