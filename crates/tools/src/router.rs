use std::sync::Arc;

use futures::future::join_all;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::invocation::{ToolCallId, ToolContent, ToolInvocation, ToolName};
use crate::registry::ToolRegistry;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub content: ToolContent,
    pub is_error: bool,
}

impl ToolCallResult {
    pub fn success(tool_use_id: &str, content: ToolContent) -> Self {
        ToolCallResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: false,
        }
    }

    pub fn error(tool_use_id: &str, message: &str) -> Self {
        ToolCallResult {
            tool_use_id: tool_use_id.to_string(),
            content: ToolContent::Text(message.to_string()),
            is_error: true,
        }
    }
}

pub struct ToolRuntime {
    registry: Arc<ToolRegistry>,
    permission: PermissionChecker,
    gate: RwLock<()>,
}

impl ToolRuntime {
    pub fn new(registry: Arc<ToolRegistry>, permission: PermissionChecker) -> Self {
        ToolRuntime {
            registry,
            permission,
            gate: RwLock::new(()),
        }
    }

    pub fn new_without_permissions(registry: Arc<ToolRegistry>) -> Self {
        ToolRuntime {
            registry,
            permission: PermissionChecker::always_allow(),
            gate: RwLock::new(()),
        }
    }

    pub async fn execute_batch(&self, calls: &[ToolCall]) -> Vec<ToolCallResult> {
        self.execute_batch_inner(calls, None).await
    }

    pub async fn execute_batch_streaming(
        &self,
        calls: &[ToolCall],
        on_progress: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Vec<ToolCallResult> {
        self.execute_batch_inner(calls, Some(Box::new(on_progress)))
            .await
    }

    async fn execute_batch_inner(
        &self,
        calls: &[ToolCall],
        on_progress: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,
    ) -> Vec<ToolCallResult> {
        // Wrap the Box in an Arc so it can be shared across spawned tasks
        let on_progress: Option<Arc<dyn Fn(&str, &str) + Send + Sync>> =
            on_progress.map(|f| Arc::from(f));

        let mut results = Vec::with_capacity(calls.len());

        let (parallel, exclusive): (Vec<_>, Vec<_>) = calls
            .iter()
            .partition(|call| self.registry.supports_parallel(&call.name));

        if !parallel.is_empty() {
            let _guard = self.gate.read().await;
            let futures: Vec<_> = parallel
                .iter()
                .map(|call| self.execute_single(call, &on_progress))
                .collect();
            let parallel_results = join_all(futures).await;
            results.extend(parallel_results);
        }

        for call in &exclusive {
            let _guard = self.gate.write().await;
            let result = self.execute_single(call, &on_progress).await;
            results.push(result);
        }

        results
    }

    pub(crate) async fn execute_single(
        &self,
        call: &ToolCall,
        on_progress: &Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    ) -> ToolCallResult {
        let tool = match self.registry.get(&call.name) {
            Some(t) => t.clone(),
            None => {
                warn!(tool = %call.name, "tool not found");
                return ToolCallResult::error(&call.id, &format!("unknown tool: {}", call.name));
            }
        };

        if !self.registry.is_read_only(&call.name) {
            match self.permission.check(&call.name).await {
                Ok(()) => {}
                Err(reason) => {
                    return ToolCallResult::error(
                        &call.id,
                        &format!("permission denied: {}", reason),
                    );
                }
            }
        }

        info!(tool = %call.name, id = %call.id, "executing tool");

        let invocation = ToolInvocation {
            call_id: ToolCallId(call.id.clone()),
            tool_name: ToolName(call.name.clone().into()),
            session_id: String::new(),
            cwd: std::path::PathBuf::new(),
            input: call.input.clone(),
        };

        let progress_sender = on_progress.as_ref().map(|cb| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let call_id = call.id.clone();
            let cb = Arc::clone(cb);
            tokio::spawn(async move {
                while let Some(chunk) = rx.recv().await {
                    cb(&call_id, &chunk);
                }
            });
            tx
        });

        match tool.handle(invocation, progress_sender).await {
            Ok(output) => {
                let is_error = output.is_error();
                let content = output.to_content();
                ToolCallResult {
                    tool_use_id: call.id.clone(),
                    content,
                    is_error,
                }
            }
            Err(e) => ToolCallResult::error(&call.id, &e.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct PermissionChecker {
    inner:
        Arc<dyn Fn(&str) -> futures::future::BoxFuture<'static, Result<(), String>> + Send + Sync>,
}

impl PermissionChecker {
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(&str) -> futures::future::BoxFuture<'static, Result<(), String>>
            + Send
            + Sync
            + 'static,
    {
        PermissionChecker {
            inner: Arc::new(check),
        }
    }

    pub fn always_allow() -> Self {
        PermissionChecker::new(|_| Box::pin(async { Ok(()) }))
    }

    pub async fn check(&self, tool_name: &str) -> Result<(), String> {
        (self.inner)(tool_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ToolExecutionError;
    use crate::events::ToolProgressSender;
    use crate::handler_kind::ToolHandlerKind;
    use crate::invocation::{FunctionToolOutput, ToolOutput};
    use crate::json_schema::JsonSchema;
    use crate::registry::ToolRegistryBuilder;
    use crate::tool_handler::ToolHandler;
    use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};
    use async_trait::async_trait;

    struct ReadOnlyTool;

    #[async_trait]
    impl ToolHandler for ReadOnlyTool {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Read
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Ok(Box::new(FunctionToolOutput::success("read ok")))
        }
    }

    struct WriteTool;

    #[async_trait]
    impl ToolHandler for WriteTool {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Write
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Ok(Box::new(FunctionToolOutput::success("write ok")))
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("read_tool", Arc::new(ReadOnlyTool));
        builder.push_spec(ToolSpec {
            name: "read_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
        });
        builder.register_handler("write_tool", Arc::new(WriteTool));
        builder.push_spec(ToolSpec {
            name: "write_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
        });
        Arc::new(builder.build())
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "nonexistent".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(result.is_error);
        assert!(result.content.into_string().contains("unknown tool"));
    }

    #[tokio::test]
    async fn read_only_tool_succeeds() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn execute_batch_runs_all_tools() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let calls = vec![
            ToolCall {
                id: "c1".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".into(),
                name: "write_tool".into(),
                input: serde_json::json!({}),
            },
        ];
        let results = runtime.execute_batch(&calls).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.is_error));
    }

    #[tokio::test]
    async fn permission_checker_allow() {
        let checker = PermissionChecker::always_allow();
        assert!(checker.check("any_tool").await.is_ok());
    }

    #[tokio::test]
    async fn permission_checker_deny() {
        let checker = PermissionChecker::new(|name| {
            let n = name.to_string();
            Box::pin(async move {
                if n == "blocked" {
                    Err("blocked".into())
                } else {
                    Ok(())
                }
            })
        });
        assert!(checker.check("allowed").await.is_ok());
        assert!(checker.check("blocked").await.is_err());
    }

    #[tokio::test]
    async fn runtime_denies_mutating_with_deny_checker() {
        let registry = make_registry();
        let checker = PermissionChecker::new(|name| {
            let n = name.to_string();
            Box::pin(async move { Err(format!("{n} denied")) })
        });
        let runtime = ToolRuntime::new(registry, checker);
        // Read-only tool should succeed (no permission check)
        let read_call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let read_result = runtime.execute_single(&read_call, &None).await;
        assert!(
            !read_result.is_error,
            "read-only tool should bypass permission check"
        );

        // Mutating tool should be denied
        let write_call = ToolCall {
            id: "c2".into(),
            name: "write_tool".into(),
            input: serde_json::json!({}),
        };
        let write_result = runtime.execute_single(&write_call, &None).await;
        assert!(write_result.is_error, "mutating tool should be denied");
        assert!(
            write_result
                .content
                .into_string()
                .contains("permission denied")
        );
    }

    #[tokio::test]
    async fn runtime_concurrent_then_sequential() {
        // Two parallel tools followed by a sequential tool should still work
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let calls = vec![
            ToolCall {
                id: "r1".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "r2".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "w1".into(),
                name: "write_tool".into(),
                input: serde_json::json!({}),
            },
        ];
        let results = runtime.execute_batch(&calls).await;
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| !r.is_error));
        // Order should be preserved (parallel tools first, then sequential)
        assert_eq!(results[0].tool_use_id, "r1".to_string());
        assert_eq!(results[1].tool_use_id, "r2".to_string());
    }

    #[tokio::test]
    async fn runtime_empty_batch() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let results = runtime.execute_batch(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn runtime_single_tool() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
        assert_eq!(result.tool_use_id, "c1");
    }

    // --- Streaming tests ---

    struct StreamingHandler {
        chunks: Vec<String>,
    }

    #[async_trait]
    impl ToolHandler for StreamingHandler {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Write
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
            progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            // Send chunks through progress, then return
            if let Some(sender) = progress {
                for chunk in &self.chunks {
                    let _ = sender.send(chunk.clone());
                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                }
            }
            Ok(Box::new(FunctionToolOutput::success(self.chunks.join(""))))
        }
    }

    fn make_streaming_registry() -> Arc<ToolRegistry> {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "stream_tool",
            Arc::new(StreamingHandler {
                chunks: vec!["hello ".into(), "world".into()],
            }),
        );
        builder.push_spec(ToolSpec {
            name: "stream_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
        });
        Arc::new(builder.build())
    }

    #[tokio::test]
    async fn execute_single_receives_progress() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "s1".into(),
            name: "stream_tool".into(),
            input: serde_json::json!({}),
        };

        let collected = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let collected_clone = Arc::clone(&collected);
        let cb: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(move |_, chunk| {
            let c = collected_clone.clone();
            let chunk = chunk.to_string();
            tokio::spawn(async move {
                c.lock().await.push(chunk);
            });
        });

        let result = runtime.execute_single(&call, &Some(cb.clone())).await;
        assert!(!result.is_error);
        assert_eq!(result.content.into_string(), "hello world");

        // Give the spawned tasks time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let final_chunks = collected.lock().await;
        assert_eq!(final_chunks.len(), 2, "should have received 2 chunks");
        assert!(final_chunks.iter().any(|c| c == "hello "));
        assert!(final_chunks.iter().any(|c| c == "world"));
    }

    #[tokio::test]
    async fn execute_batch_streaming_receives_progress() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "s1".into(),
            name: "stream_tool".into(),
            input: serde_json::json!({}),
        };

        let collected = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let collected_clone = Arc::clone(&collected);

        let results = runtime
            .execute_batch_streaming(&[call], move |_id, chunk| {
                let c = collected_clone.clone();
                let chunk = chunk.to_string();
                tokio::spawn(async move {
                    c.lock().await.push(chunk);
                });
            })
            .await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let final_chunks = collected.lock().await;
        assert_eq!(
            final_chunks.len(),
            2,
            "streaming callback should have 2 chunks"
        );
    }

    #[tokio::test]
    async fn execute_batch_streaming_empty() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let results = runtime.execute_batch_streaming(&[], |_, _| {}).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn execute_batch_streaming_unknown_tool() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "x1".into(),
            name: "nonexistent".into(),
            input: serde_json::json!({}),
        };
        let results = runtime.execute_batch_streaming(&[call], |_, _| {}).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
    }
}
