use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Result;
use async_trait::async_trait;
use devo_core::EventCallback;
use devo_core::Message;
use devo_core::Model;
use devo_core::ModelRequest;
use devo_core::ModelResponse;
use devo_core::QueryEvent;
use devo_core::QueryOptions;
use devo_core::ResponseContent;
use devo_core::SessionConfig;
use devo_core::SessionState;
use devo_core::StopReason;
use devo_core::StreamEvent;
use devo_core::TurnConfig;
use devo_core::Usage;
use devo_core::query;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolRuntime;
use devo_provider::ModelProviderSDK;
use devo_provider::error::ProviderError;
use futures::Stream;
use pretty_assertions::assert_eq;

const CONTEXT_LIMIT_MESSAGE: &str = "This model's maximum context length is 1048565 tokens. However, you request 1058357 tokens (674357 in the messages, 384000 in the completion). Please reduce the length of the messages or completion.";

struct ContextLimitThenSuccessProvider {
    stream_attempts: AtomicUsize,
    compaction_calls: AtomicUsize,
}

#[async_trait]
impl ModelProviderSDK for ContextLimitThenSuccessProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.compaction_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse {
            id: "compaction-response".to_string(),
            content: vec![ResponseContent::Text("Earlier work summary".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: Default::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        if self.stream_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(anyhow::Error::new(ProviderError::ContextLimitError {
                message: CONTEXT_LIMIT_MESSAGE.to_string(),
                current_tokens: None,
                limit: None,
            }));
        }

        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "final-response".to_string(),
                    content: vec![ResponseContent::Text("done".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "context-limit-then-success"
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CompactionEvent {
    Started,
    Completed,
}

#[tokio::test]
async fn context_limit_error_compacts_and_retries_query() {
    let provider = Arc::new(ContextLimitThenSuccessProvider {
        stream_attempts: AtomicUsize::new(0),
        compaction_calls: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut session = SessionState::new(SessionConfig::default(), tempdir.path().to_path_buf());
    session.push_message(Message::user("earlier user request"));
    session.push_message(Message::assistant_text("earlier assistant response"));
    session.push_message(Message::user("latest user request"));
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let callback: EventCallback = Arc::new(move |event| {
        let captured_events = Arc::clone(&captured_events);
        Box::pin(async move {
            captured_events.lock().expect("lock events").push(event);
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider_sdk,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should compact and retry");

    let compaction_events = events
        .lock()
        .expect("lock events")
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ContextCompactionStarted => Some(CompactionEvent::Started),
            QueryEvent::ContextCompactionCompleted { .. } => Some(CompactionEvent::Completed),
            QueryEvent::ContextCompactionFailed { .. }
            | QueryEvent::ProviderRetryStatus(_)
            | QueryEvent::TextDelta(_)
            | QueryEvent::ReasoningDelta(_)
            | QueryEvent::ReasoningCompleted
            | QueryEvent::ContextEstimate { .. }
            | QueryEvent::UsageDelta { .. }
            | QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolUseInputDelta { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. }
            | QueryEvent::TurnComplete { .. }
            | QueryEvent::Usage { .. } => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        compaction_events,
        vec![CompactionEvent::Started, CompactionEvent::Completed]
    );
    assert_eq!(provider.stream_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(provider.compaction_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.prompt_source_messages().last(),
        Some(&Message::assistant_text("done"))
    );
}
