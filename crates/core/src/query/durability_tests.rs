//! Crash-boundary checks for acknowledged execution.

use super::*;
use crate::durable_execution::{ExecutionRecord, ExecutionReplay, ToolIntentJournal};
use pretty_assertions::assert_eq;

#[derive(Clone, Copy)]
enum Fault {
    Intent,
    Outcome,
}

struct Journal {
    state: Mutex<ExecutionReplay>,
    fault: Option<Fault>,
}

#[async_trait]
impl ToolIntentJournal for Journal {
    async fn commit(&self, record: ExecutionRecord) -> Result<()> {
        if matches!(
            (&self.fault, &record),
            (Some(Fault::Intent), ExecutionRecord::IntentBatch { .. })
                | (Some(Fault::Outcome), ExecutionRecord::Outcomes { .. })
        ) {
            anyhow::bail!("injected durable write failure");
        }
        self.state.lock().unwrap().apply(&record)
    }

    async fn replay(&self) -> Result<ExecutionReplay> {
        Ok(self.state.lock().unwrap().clone())
    }
}

struct CountedTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for CountedTool {
    fn spec(&self) -> &ToolSpec {
        MutatingTool.spec()
    }

    async fn handle(
        &self,
        context: crate::tools::ToolContext,
        input: serde_json::Value,
        progress: Option<crate::tools::ToolProgressSender>,
    ) -> Result<crate::tools::ToolResult, crate::tools::ToolCallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        MutatingTool.handle(context, input, progress).await
    }
}

/// Trace: L2-DES-CONTEXT-004. Persistence failure never causes automatic retry.
#[tokio::test]
async fn journal_failures_stop_dispatch_or_model_continuation() {
    for (fault, expected_calls) in [(Fault::Intent, 0), (Fault::Outcome, 1)] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "mutating_tool",
            Arc::new(CountedTool {
                calls: calls.clone(),
            }),
        );
        builder.push_spec(ToolSpec::new(
            "mutating_tool",
            "test mutation",
            JsonSchema::object(Default::default(), None, None),
        ));
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(registry.clone());
        let provider = Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        });
        let journal = Arc::new(Journal {
            state: Mutex::new(ExecutionReplay::default()),
            fault: Some(fault),
        });
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("execute"));
        let result = query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider.clone(),
            registry,
            &runtime,
            None,
            QueryOptions {
                journal: Some(journal),
                ..QueryOptions::default()
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            (
                calls.load(Ordering::SeqCst),
                provider.requests.load(Ordering::SeqCst)
            ),
            (expected_calls, 1)
        );
    }
}

/// Trace: L2-DES-CONTEXT-004. Finalization after a crash does not call a model.
#[tokio::test]
async fn acknowledged_final_response_only_finishes_bookkeeping() {
    let messages = vec![
        Message::user("hello"),
        Message::assistant_text("saved answer"),
    ];
    let mut replay = ExecutionReplay::default();
    replay
        .apply(&ExecutionRecord::ModelCompleted {
            items: messages
                .iter()
                .cloned()
                .flat_map(crate::message_to_response_items)
                .collect(),
            stop_reason: Some(StopReason::EndTurn),
        })
        .unwrap();
    let journal = Arc::new(Journal {
        state: Mutex::new(replay),
        fault: None,
    });
    let provider = Arc::new(SingleToolUseProvider {
        requests: AtomicUsize::new(0),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(registry.clone());
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        registry,
        &runtime,
        None,
        QueryOptions {
            journal: Some(journal),
            ..QueryOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (
            provider.requests.load(Ordering::SeqCst),
            session.prompt_source_messages()
        ),
        (0, messages.as_slice())
    );
}
