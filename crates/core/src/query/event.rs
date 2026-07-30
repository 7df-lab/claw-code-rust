//! Public observation surface for the query loop.
//!
//! Callers (CLI/UI/server) subscribe to a single `QueryEvent` stream covering
//! model deltas, tool progress, compaction, and provider retries.

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolContent;
use devo_protocol::StopReason;
use devo_provider::ModelProviderSDK;

/// Events emitted during a query for the caller (CLI/UI) to observe.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Provider request retry status.
    ProviderRetryStatus(ProviderRetryStatus),
    /// Context compaction is about to begin.
    ContextCompactionStarted,
    /// Context compaction replaced the current prompt history.
    ContextCompactionCompleted,
    /// Context compaction did not replace the current prompt history.
    ContextCompactionFailed {
        /// Human-readable reason the compaction did not complete.
        message: String,
    },
    /// Incremental text from the assistant.
    TextDelta(String),
    /// Incremental reasoning text from the assistant.
    ReasoningDelta(String),
    /// Current reasoning block completed.
    ReasoningCompleted,
    /// Incremental token usage update from the provider stream.
    /// TODO: Review the mechanism from the OpenAI API / Anthropic API documentation.
    UsageDelta { usage: devo_protocol::Usage },
    /// The assistant started a tool call.
    ToolUseStart {
        /// Stable provider-issued tool use identifier.
        id: String,
        /// Tool name selected by the model.
        name: String,
        /// Fully decoded tool input payload, when available.
        input: serde_json::Value,
    },
    /// A locally executed tool has passed permission checks and started running.
    ToolExecutionStart {
        /// Stable provider-issued tool use identifier.
        id: String,
    },
    /// Incremental output delta from a running tool.
    ToolProgress {
        tool_use_id: String,
        progress: crate::tools::ToolProgress,
    },
    /// A tool call completed.
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        content: ToolContent,
        display_content: Option<String>,
        is_error: bool,
        /// Human-readable summary for client-side rendering (e.g. "bash: npm run dev").
        summary: String,
    },
    /// A turn is complete (model stopped generating).
    TurnComplete { stop_reason: StopReason },
    /// Token usage update.
    Usage { usage: devo_protocol::Usage },
}

/// Async sink for streaming `QueryEvent`s out of the core query loop.
///
/// The type is intentionally erased so `query()` can accept callbacks from tests, the server
/// runtime, and tool-progress plumbing without knowing their concrete future types:
///
/// - `Arc`: shared, cheap-to-clone ownership. The same callback is cloned into model-stream and
///   tool-progress paths that may outlive the immediate stack frame.
/// - `dyn Fn(QueryEvent)`: dynamic callback interface. Callers provide any closure that accepts one
///   event and can be invoked repeatedly.
/// - `BoxFuture<'static, ()>`: boxed async work returned by the callback. Boxing hides the
///   closure's concrete future type behind one trait-object shape; `'static` prevents borrowed
///   stack data from escaping into spawned or delayed event paths.
/// - `Send + Sync`: the callback can be shared and awaited across Tokio tasks and worker threads.
///
/// Awaiting this future is what lets callers use bounded async channels for backpressure instead of
/// the old synchronous callback bridge.
pub type EventCallback = Arc<dyn Fn(QueryEvent) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryStatus {
    pub provider: String,
    pub model: String,
    pub attempt: usize,
    pub backoff_ms: u64,
    pub phase: QueryProviderRetryPhase,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryProviderRetryPhase {
    Scheduled,
    Resumed,
}

#[derive(Clone, Default)]
pub struct QueryOptions {
    pub cancel_token: Option<CancellationToken>,
    /// Optional provider used only for compaction summaries. Servers use this
    /// seam to attach Compaction metering without misclassifying the main
    /// streaming query as compaction overhead.
    pub compaction_provider: Option<Arc<dyn ModelProviderSDK>>,
}

impl std::fmt::Debug for QueryOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueryOptions")
            .field("cancel_token", &self.cancel_token)
            .field(
                "compaction_provider",
                &self
                    .compaction_provider
                    .as_ref()
                    .map(|provider| provider.name()),
            )
            .finish()
    }
}

pub(crate) async fn emit_query_event(on_event: &Option<EventCallback>, event: QueryEvent) {
    if let Some(callback) = on_event {
        callback(event).await;
    }
}
