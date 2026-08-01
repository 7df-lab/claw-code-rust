//! Unit tests for the query loop and its submodules.

use devo_protocol::Usage;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::EventCallback;
use crate::ProviderRetryStatus;
use crate::QueryProviderRetryPhase;
use crate::tools::ToolAgentScope;
use crate::tools::ToolContent;
use crate::tools::ToolPreparationFeedback;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use crate::tools::ToolRuntimeContext;
use crate::tools::json_schema::JsonSchema;
use crate::tools::registry::ToolExposure;
use crate::tools::registry::ToolRegistryBuilder;
use crate::tools::router::PermissionChecker;
use crate::tools::router::ToolExecutionOptions;
use crate::tools::tool_handler::ToolHandler;
use crate::tools::tool_spec::ToolExecutionMode;
use crate::tools::tool_spec::ToolOutputMode;
use crate::tools::tool_spec::ToolSpec;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::CollaborationMode;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_provider::ModelProviderSDK;
use devo_safety::PermissionMode;
use futures::Stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::QueryEvent;
use super::QueryOptions;
use super::hosted_tools_for_web_search;
use super::insert_subagent_request_reminders;
use super::query;
use super::test_model_connection;
use super::truncate_tool_result_for_model;
use crate::AgentError;
use crate::ContentBlock;
use crate::Message;
use crate::Model;
use crate::ReasoningEffort;
use crate::Role;
use crate::context::ContextualUserFragment;
use crate::context::compaction_summary::CompactionSummary;
use crate::history::compaction::CompactionKind;
use crate::response_item::ResponseItem;

#[test]
fn assistant_content_visibility_requires_visible_content() {
    assert!(!super::assistant_content_has_visible_content(&[]));
    assert!(!super::assistant_content_has_visible_content(&[
        ContentBlock::Text {
            text: " \n\t".to_string(),
        },
    ]));
    assert!(!super::assistant_content_has_visible_content(&[
        ContentBlock::ToolResult {
            tool_use_id: "call-1".to_string(),
            content: String::new(),
            is_error: false,
        },
    ]));

    for content in [
        vec![ContentBlock::Text {
            text: "visible".to_string(),
        }],
        vec![ContentBlock::Reasoning {
            text: "reasoning".to_string(),
        }],
        vec![ContentBlock::ProviderReasoning {
            provider: "test".to_string(),
            payload: serde_json::json!({"thinking":"hidden"}),
        }],
        vec![ContentBlock::ToolUse {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"filePath":"README.md"}),
        }],
        vec![ContentBlock::HostedToolUse {
            id: "hosted-1".to_string(),
            name: "web_search".to_string(),
            input: serde_json::json!({"query":"docs"}),
            output: None,
            status: None,
        }],
        vec![ContentBlock::ToolResult {
            tool_use_id: "call-1".to_string(),
            content: "result".to_string(),
            is_error: false,
        }],
    ] {
        assert!(super::assistant_content_has_visible_content(&content));
    }
}

#[test]
fn hosted_tools_follow_resolved_web_search_mode() {
    let hosted = hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Provider);
    assert_eq!(hosted.len(), 1);
    assert!(matches!(
        hosted.as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));

    assert_eq!(
        hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Disabled),
        Vec::new()
    );
    assert_eq!(
        hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Local(
            devo_config::ResolvedLocalWebSearchConfig {
                provider_id: "test".to_string(),
                kind: devo_config::LocalWebSearchProviderKind::Exa,
                api_key: "secret".to_string(),
                base_url: None,
                max_results: None,
            },
        )),
        Vec::new()
    );
}
use crate::ReasoningCapability;
use crate::ReasoningImplementation;

#[test]
fn network_errors_are_retryable() {
    let cases = [
        anyhow::anyhow!("request timed out while connecting"),
        anyhow::anyhow!(
            "error sending request for url (https://api.example.test): connection refused"
        ),
        anyhow::anyhow!("dns error: failed to lookup address information"),
        anyhow::anyhow!("network is unreachable"),
        anyhow::anyhow!(
            "anthropic stream error for model deepseek-v4-flash: invalid header value: \"text/html; charset=utf-8\"; debug=InvalidContentType(\"text/html; charset=utf-8\")"
        ),
        anyhow::anyhow!("Invalid status code: 408 Request Timeout"),
        anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "socket timed out",
        )),
        anyhow::Error::new(devo_provider::error::ProviderError::ProviderTimeoutError {
            message: "provider request timed out".into(),
            provider_name: Some("test-provider".into()),
        }),
        anyhow::Error::new(devo_provider::timeout::stream_idle_timeout_provider_error(
            "openai",
            "gpt-test",
            devo_provider::timeout::StreamIdleTimeoutError {
                idle_timeout: std::time::Duration::from_secs(60),
            },
        )),
        anyhow::Error::new(devo_provider::timeout::StreamIdleTimeoutError {
            idle_timeout: std::time::Duration::from_secs(60),
        }),
        anyhow::anyhow!(
            "openai stream idle timeout for model gpt-test: provider stream idle timeout after 60s without receiving data"
        ),
    ];

    for error in cases {
        assert_eq!(
            super::classify_error(&error),
            super::ErrorClass::NetworkError
        );

        let mut retry_count = 0;
        let mut context_compacted = false;
        assert!(matches!(
            super::provider_retry_decision(&error, &mut retry_count, &mut context_compacted),
            super::ProviderRetryDecision::RetryAfter(_)
        ));
        assert_eq!(retry_count, 1);
        assert!(!context_compacted);
    }
}

#[test]
fn token_timeout_remains_authentication_failure() {
    let error = anyhow::anyhow!("token timeout");

    assert_eq!(
        super::classify_error(&error),
        super::ErrorClass::AuthenticationFailure
    );

    let mut retry_count = 0;
    let mut context_compacted = false;
    assert!(matches!(
        super::provider_retry_decision(&error, &mut retry_count, &mut context_compacted),
        super::ProviderRetryDecision::Fail
    ));
    assert_eq!(retry_count, 0);
    assert!(!context_compacted);
}
use crate::ReasoningVariant;
use crate::ReasoningVariantConfig;
use crate::SessionConfig;
use crate::SessionState;
use crate::TruncationMode;
use crate::TruncationPolicyConfig;
use crate::TurnConfig;

#[test]
fn model_tool_result_truncation_preserves_content_within_budget() {
    assert_eq!(
        truncate_tool_result_for_model(
            "short".to_string(),
            Some("read"),
            TruncationPolicyConfig::bytes(100).into(),
        ),
        "short"
    );
}

#[test]
fn model_tool_result_truncation_uses_byte_policy() {
    assert_eq!(
        truncate_tool_result_for_model(
            "abcdefghijklmnopqrstuvwxyz".to_string(),
            Some("read"),
            TruncationPolicyConfig::bytes(20).into(),
        ),
        "abcde\n...[truncated]"
    );
}

#[test]
fn model_tool_result_truncation_uses_token_policy_byte_budget() {
    assert_eq!(
        truncate_tool_result_for_model(
            "abcdefghijklmnopqrstuvwxyz".to_string(),
            Some("read"),
            TruncationPolicyConfig::tokens(5).into(),
        ),
        "abcde\n...[truncated]"
    );
}

#[test]
fn model_tool_result_truncation_preserves_utf8_boundaries() {
    let truncated = truncate_tool_result_for_model(
        "éééééabcdefghij".to_string(),
        Some("read"),
        TruncationPolicyConfig::bytes(18).into(),
    );

    assert_eq!(truncated, "é\n...[truncated]");
    assert!(truncated.len() <= 18);
}

#[test]
fn model_visible_shell_mixed_content_uses_text_only_once() {
    use super::serialize_tool_content_for_model;
    use crate::tools::ToolContent;

    let stream = "hello\nworld".to_string();
    let content = ToolContent::Mixed {
        text: Some(stream.clone()),
        json: Some(serde_json::json!({
            "command": "echo hello",
            "exit": 0,
            "cwd": "/tmp",
            "description": "say hello",
        })),
    };

    let model = serialize_tool_content_for_model(content.clone(), Some("shell_command"));
    let truncated = truncate_tool_result_for_model(
        model.clone(),
        Some("shell_command"),
        TruncationPolicyConfig::bytes(10_000).into(),
    );

    assert_eq!(model, stream);
    assert_eq!(truncated, stream);
    assert_eq!(model.matches("hello").count(), 1);
    assert!(!truncated.contains("\"exit\""));
    assert!(!truncated.contains("\"command\""));
    assert_eq!(
        serialize_tool_content_for_model(content, Some("bash")),
        stream
    );
}

#[test]
fn model_visible_webfetch_mixed_content_keeps_image_json() {
    use super::serialize_tool_content_for_model;
    use crate::tools::ToolContent;

    let content = ToolContent::Mixed {
        text: Some("Image fetched successfully".into()),
        json: Some(serde_json::json!({
            "title": "https://example.com/a.png (image/png)",
            "mime": "image/png",
            "image_base64": "abc123",
        })),
    };

    let model = serialize_tool_content_for_model(content, Some("webfetch"));
    assert!(model.contains("Image fetched successfully"));
    assert!(model.contains("image_base64"));
    assert!(model.contains("abc123"));
}

#[test]
fn model_visible_read_mixed_content_omits_preview_json() {
    use super::serialize_tool_content_for_model;
    use crate::tools::ToolContent;

    let file_body = "line one\nline two\nline three".to_string();
    let text =
        format!("<path>/tmp/a.rs</path>\n<type>file</type>\n<content>\n{file_body}\n</content>");
    let content = ToolContent::Mixed {
        text: Some(text.clone()),
        json: Some(serde_json::json!({
            "preview": "line one\nline two\nline three",
            "truncated": false,
            "loaded": [],
        })),
    };

    let model = serialize_tool_content_for_model(content, Some("read"));
    assert_eq!(model, text);
    assert_eq!(model.matches("line one").count(), 1);
    assert!(!model.contains("\"preview\""));
    assert!(!model.contains("\"truncated\""));
}

#[test]
fn model_tool_result_truncation_preserves_agent_coordination_results() {
    let content = "abcdefghijklmnopqrstuvwxyz".to_string();

    for tool_name in [
        Some("await_task"),
        Some("wait_agent"),
        Some("subagent_result"),
    ] {
        assert_eq!(
            truncate_tool_result_for_model(
                content.clone(),
                tool_name,
                TruncationPolicyConfig::bytes(20).into(),
            ),
            content
        );
    }
}

const HOSTED_DSML_TEXT: &str = "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">current Rust docs</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>";

struct SingleToolUseProvider {
    requests: AtomicUsize,
}

struct CapturingToolUseProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    calls: AtomicUsize,
}

struct InterleavedToolUseProvider {
    requests: AtomicUsize,
}

struct ParallelToolUseProvider {
    requests: AtomicUsize,
}

#[async_trait]
impl devo_provider::ModelProviderSDK for SingleToolUseProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

        let events = if request_number == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: r#"{"value":1}"#.into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".into(),
                            name: "mutating_tool".into(),
                            input: json!({ "value": 1 }),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "done".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        };

        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "test-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for CapturingToolUseProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        let request_number = self.calls.fetch_add(1, Ordering::SeqCst);

        let events = if request_number == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".into(),
                            name: "mutating_tool".into(),
                            input: json!({}),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        } else {
            vec![Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-2".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            })]
        };

        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "capturing-tool-use-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for InterleavedToolUseProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

        let events = if request_number == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallStart {
                    index: 1,
                    id: "tool-2".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: r#"{"value":1}"#.into(),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 1,
                    partial_json: r#"{"value":2}"#.into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![
                            ResponseContent::ToolUse {
                                id: "tool-1".into(),
                                name: "mutating_tool".into(),
                                input: json!({}),
                            },
                            ResponseContent::ToolUse {
                                id: "tool-2".into(),
                                name: "mutating_tool".into(),
                                input: json!({}),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "done".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        };

        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "interleaved-test-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for ParallelToolUseProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

        let events = if request_number == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "slow".into(),
                    name: "parallel_tool".into(),
                    input: json!({
                        "delay_ms": 50,
                        "output": "slow complete",
                    }),
                }),
                Ok(StreamEvent::ToolCallStart {
                    index: 1,
                    id: "fast".into(),
                    name: "parallel_tool".into(),
                    input: json!({
                        "delay_ms": 5,
                        "output": "fast complete",
                    }),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![
                            ResponseContent::ToolUse {
                                id: "slow".into(),
                                name: "parallel_tool".into(),
                                input: json!({
                                    "delay_ms": 50,
                                    "output": "slow complete",
                                }),
                            },
                            ResponseContent::ToolUse {
                                id: "fast".into(),
                                name: "parallel_tool".into(),
                                input: json!({
                                    "delay_ms": 5,
                                    "output": "fast complete",
                                }),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        } else {
            vec![Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-2".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            })]
        };

        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "parallel-tool-provider"
    }
}

struct MutatingTool;

struct CapturingProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

struct OpenAiCapturingProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

struct HostedWebSearchProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

struct HostedDsmlTextProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

struct HostedWebFetchProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

fn final_text_stream(text: &str) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    Box::pin(futures::stream::iter(vec![Ok(StreamEvent::MessageDone {
        response: ModelResponse {
            id: "resp-final".into(),
            content: vec![ResponseContent::Text(text.to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: Default::default(),
        },
    })]))
}

struct TransientStreamCreateProvider {
    attempts: AtomicUsize,
}

struct TransientStreamEventProvider {
    attempts: AtomicUsize,
}

struct RateLimitedStreamCreateProvider {
    attempts: AtomicUsize,
}

enum CompactionProviderOutcome {
    Summary,
    Error,
}

struct CompactionProvider {
    completion_calls: AtomicUsize,
    outcome: CompactionProviderOutcome,
}

#[async_trait]
impl devo_provider::ModelProviderSDK for CapturingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "capturing-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for OpenAiCapturingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for HostedWebSearchProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_count = {
            let mut requests = self.requests.lock().expect("lock requests");
            requests.push(request);
            requests.len()
        };
        if request_count > 1 {
            return Ok(final_text_stream("done"));
        }
        let input = json!({ "query": "current Rust docs" });
        let output = Some(json!({
            "results": [
                {
                    "title": "Rust documentation",
                    "url": "https://example.test/rust"
                }
            ]
        }));
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::HostedToolCallStart {
                index: 0,
                id: "hosted_ws_1".into(),
                name: "web_search".into(),
                input: input.clone(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![
                        ResponseContent::HostedToolUse {
                            id: "hosted_ws_1".into(),
                            name: "web_search".into(),
                            input: input.clone(),
                            output: None,
                            status: None,
                        },
                        ResponseContent::HostedToolUse {
                            id: "hosted_ws_1".into(),
                            name: "web_search".into(),
                            input,
                            output,
                            status: Some("completed".into()),
                        },
                    ],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "hosted-web-search-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for HostedDsmlTextProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_count = {
            let mut requests = self.requests.lock().expect("lock requests");
            requests.push(request);
            requests.len()
        };
        if request_count > 1 {
            return Ok(final_text_stream("done"));
        }
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: HOSTED_DSML_TEXT.to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-dsml".into(),
                    content: vec![ResponseContent::Text(HOSTED_DSML_TEXT.to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "hosted-dsml-text-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for HostedWebFetchProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_count = {
            let mut requests = self.requests.lock().expect("lock requests");
            requests.push(request);
            requests.len()
        };
        if request_count > 1 {
            return Ok(final_text_stream("done"));
        }
        let input = json!({ "url": "https://example.test/docs" });
        let output = Some(json!({
            "title": "Docs",
            "url": "https://example.test/docs"
        }));
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::HostedToolCallStart {
                index: 0,
                id: "hosted_wf_1".into(),
                name: "web_fetch".into(),
                input: input.clone(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![
                        ResponseContent::HostedToolUse {
                            id: "hosted_wf_1".into(),
                            name: "web_fetch".into(),
                            input: input.clone(),
                            output: None,
                            status: None,
                        },
                        ResponseContent::HostedToolUse {
                            id: "hosted_wf_1".into(),
                            name: "web_fetch".into(),
                            input,
                            output,
                            status: Some("completed".into()),
                        },
                    ],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "hosted-web-fetch-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for TransientStreamCreateProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Err(anyhow::anyhow!("503 service unavailable"));
        }

        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "transient-stream-create-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for TransientStreamEventProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Ok(Box::pin(futures::stream::iter(vec![Err(anyhow::anyhow!(
                "500 internal server error"
            ))])));
        }

        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "transient-stream-event-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for RateLimitedStreamCreateProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            return Err(anyhow::anyhow!("429 rate limit exceeded"));
        }

        Ok(final_text_stream("done"))
    }

    fn name(&self) -> &str {
        "rate-limited-stream-create-provider"
    }
}

#[async_trait]
impl devo_provider::ModelProviderSDK for CompactionProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.completion_calls.fetch_add(1, Ordering::SeqCst);
        match &self.outcome {
            CompactionProviderOutcome::Summary => Ok(ModelResponse {
                id: "compaction-response".to_string(),
                content: vec![ResponseContent::Text("summary".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: Default::default(),
            }),
            CompactionProviderOutcome::Error => Err(anyhow::anyhow!("compaction provider failed")),
        }
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        unreachable!("tests call the non-streaming compaction path only")
    }

    fn name(&self) -> &str {
        "compaction-provider"
    }
}

#[async_trait]
impl ToolHandler for MutatingTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        // Leak a static spec for test purposes
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "write",
            "write tool",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        Ok(crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text("ok".into()),
            "ok",
        ))
    }
}

struct DisplayContentTool;

struct LargeToolResultTool {
    content: String,
    display_content: Option<String>,
}

struct CountingWebSearchTool {
    executions: Arc<AtomicUsize>,
}

struct CountingWebFetchTool {
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for CountingWebSearchTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "web_search",
            "Search the web.",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text("local search".into()),
            "local search",
        ))
    }
}

#[async_trait]
impl ToolHandler for CountingWebFetchTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "webfetch",
            "Fetch a URL.",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text("local fetch".into()),
            "local fetch",
        ))
    }
}

#[async_trait]
impl ToolHandler for DisplayContentTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "read",
            "read tool",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        let mut result = crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text("canonical".into()),
            "done",
        );
        result.display_content = Some("display".to_string());
        Ok(result)
    }
}

#[async_trait]
impl ToolHandler for LargeToolResultTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "read",
            "read tool",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        let mut result = crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text(self.content.clone()),
            "done",
        );
        result.display_content = self.display_content.clone();
        Ok(result)
    }
}

struct StreamingMutatingTool;

struct ParallelDelayTool;

#[async_trait]
impl ToolHandler for StreamingMutatingTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "write",
            "write tool",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        Ok(crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text("stream complete".into()),
            "done",
        ))
    }
}

#[async_trait]
impl ToolHandler for ParallelDelayTool {
    fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
        Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
            "read",
            "read tool",
            crate::tools::JsonSchema::object(Default::default(), None, None),
        )))
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        let delay_ms = input
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        let output = input
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        Ok(crate::tools::contracts::ToolResult::success(
            crate::tools::contracts::ToolResultContent::Text(output.to_string()),
            "done",
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RecordedCompactionEvent {
    Started,
    Completed,
    Failed { message: String },
}

fn recorded_compaction_events(events: &[QueryEvent]) -> Vec<RecordedCompactionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ContextCompactionStarted => Some(RecordedCompactionEvent::Started),
            QueryEvent::ContextCompactionCompleted => Some(RecordedCompactionEvent::Completed),
            QueryEvent::ContextCompactionFailed { message } => {
                Some(RecordedCompactionEvent::Failed {
                    message: message.clone(),
                })
            }
            QueryEvent::ProviderRetryStatus(_)
            | QueryEvent::TextDelta(_)
            | QueryEvent::ReasoningDelta(_)
            | QueryEvent::ReasoningCompleted
            | QueryEvent::UsageDelta { .. }
            | QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. }
            | QueryEvent::TurnComplete { .. }
            | QueryEvent::Usage { .. } => None,
        })
        .collect()
}

fn recording_callback(events: &Arc<Mutex<Vec<QueryEvent>>>) -> EventCallback {
    let captured_events = Arc::clone(events);
    Arc::new(move |event| {
        let captured_events = Arc::clone(&captured_events);
        Box::pin(async move {
            captured_events.lock().expect("lock events").push(event);
        })
    })
}

fn compaction_test_session(total_input_tokens: usize) -> SessionState {
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("x".repeat(80_004)));
    session.push_message(Message::user("latest"));
    session.total_input_tokens = total_input_tokens;
    session
}

#[tokio::test]
async fn automatic_compaction_emits_started_then_completed_when_history_is_replaced() {
    let provider = Arc::new(CompactionProvider {
        completion_calls: AtomicUsize::new(0),
        outcome: CompactionProviderOutcome::Summary,
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let on_event = Some(recording_callback(&events));
    let mut session = compaction_test_session(/*total_input_tokens*/ 200_000);

    super::summarize_and_compact(
        &mut session,
        &on_event,
        &provider_sdk,
        "compaction-model",
        "compaction-request-model",
        /*max_tokens*/ 4096,
        CompactionKind::Auto,
    )
    .await;

    assert_eq!(
        recorded_compaction_events(&events.lock().expect("lock events")),
        vec![
            RecordedCompactionEvent::Started,
            RecordedCompactionEvent::Completed,
        ]
    );
    assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 1);
    let ResponseItem::Message(expected_summary) =
        CompactionSummary::new("summary").to_response_item()
    else {
        unreachable!("compaction summaries are messages");
    };
    assert_eq!(
        session.prompt_source_messages(),
        &[expected_summary, Message::user("latest")]
    );
}

#[tokio::test]
async fn automatic_compaction_emits_failed_when_compaction_is_skipped() {
    let provider = Arc::new(CompactionProvider {
        completion_calls: AtomicUsize::new(0),
        outcome: CompactionProviderOutcome::Summary,
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let on_event = Some(recording_callback(&events));
    let mut session = compaction_test_session(/*total_input_tokens*/ 0);
    let original_messages = session.prompt_source_messages().to_vec();

    super::summarize_and_compact(
        &mut session,
        &on_event,
        &provider_sdk,
        "compaction-model",
        "compaction-request-model",
        /*max_tokens*/ 4096,
        CompactionKind::Auto,
    )
    .await;

    assert_eq!(
        recorded_compaction_events(&events.lock().expect("lock events")),
        vec![
            RecordedCompactionEvent::Started,
            RecordedCompactionEvent::Failed {
                message: "Context compaction skipped: nothing to compact".to_string(),
            },
        ]
    );
    assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 0);
    assert_eq!(session.prompt_source_messages(), original_messages);
}

#[tokio::test(start_paused = true)]
async fn proactive_compaction_emits_failed_when_compaction_errors() {
    let provider = Arc::new(CompactionProvider {
        completion_calls: AtomicUsize::new(0),
        outcome: CompactionProviderOutcome::Error,
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let on_event = Some(recording_callback(&events));
    let mut session = compaction_test_session(/*total_input_tokens*/ 0);
    let original_messages = session.prompt_source_messages().to_vec();

    super::summarize_and_compact(
        &mut session,
        &on_event,
        &provider_sdk,
        "compaction-model",
        "compaction-request-model",
        /*max_tokens*/ 4096,
        CompactionKind::Proactive,
    )
    .await;

    assert_eq!(
        recorded_compaction_events(&events.lock().expect("lock events")),
        vec![
            RecordedCompactionEvent::Started,
            RecordedCompactionEvent::Failed {
                message: "summarization failed: compaction provider failed".to_string(),
            },
        ]
    );
    assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 5);
    assert_eq!(session.prompt_source_messages(), original_messages);
}

#[tokio::test]
async fn query_retries_transient_stream_creation_errors() {
    let provider = Arc::new(TransientStreamCreateProvider {
        attempts: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider_sdk,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should retry and succeed");

    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        session.messages.last(),
        Some(&Message::assistant_text("done"))
    );
}

#[tokio::test(start_paused = true)]
async fn query_retries_transient_stream_event_errors_before_content() {
    let provider = Arc::new(TransientStreamEventProvider {
        attempts: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let turn_config = TurnConfig::new(Model::default(), None);
    let model = turn_config.model.slug.clone();
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
        &turn_config,
        provider_sdk,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should retry and succeed");

    let retry_statuses = events
        .lock()
        .expect("lock events")
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ProviderRetryStatus(status) => Some(status.clone()),
            QueryEvent::ContextCompactionStarted
            | QueryEvent::ContextCompactionCompleted
            | QueryEvent::ContextCompactionFailed { .. }
            | QueryEvent::TextDelta(_)
            | QueryEvent::ReasoningDelta(_)
            | QueryEvent::ReasoningCompleted
            | QueryEvent::UsageDelta { .. }
            | QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. }
            | QueryEvent::TurnComplete { .. }
            | QueryEvent::Usage { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        retry_statuses,
        vec![
            ProviderRetryStatus {
                provider: "transient-stream-event-provider".to_string(),
                model: model.clone(),
                attempt: 1,
                backoff_ms: 250,
                phase: QueryProviderRetryPhase::Scheduled,
                message: "Retrying provider request in 0.2s".to_string(),
            },
            ProviderRetryStatus {
                provider: "transient-stream-event-provider".to_string(),
                model,
                attempt: 1,
                backoff_ms: 0,
                phase: QueryProviderRetryPhase::Resumed,
                message: "Retrying provider request now".to_string(),
            },
        ]
    );
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
    let assistant_messages = session
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages, vec![Message::assistant_text("done")]);
}

#[tokio::test(start_paused = true)]
async fn query_waits_sixty_seconds_for_each_rate_limit_retry() {
    let provider = Arc::new(RateLimitedStreamCreateProvider {
        attempts: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let turn_config = TurnConfig::new(Model::default(), None);
    let model = turn_config.model.slug.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let callback: EventCallback = Arc::new(move |event| {
        let captured_events = Arc::clone(&captured_events);
        Box::pin(async move {
            captured_events.lock().expect("lock events").push(event);
        })
    });
    let started_at = tokio::time::Instant::now();

    query(
        &mut session,
        &turn_config,
        provider_sdk,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should retry and succeed");

    assert_eq!(
        tokio::time::Instant::now().duration_since(started_at),
        std::time::Duration::from_secs(120)
    );
    let retry_statuses = events
        .lock()
        .expect("lock events")
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ProviderRetryStatus(status) => Some(status.clone()),
            QueryEvent::ContextCompactionStarted
            | QueryEvent::ContextCompactionCompleted
            | QueryEvent::ContextCompactionFailed { .. }
            | QueryEvent::TextDelta(_)
            | QueryEvent::ReasoningDelta(_)
            | QueryEvent::ReasoningCompleted
            | QueryEvent::UsageDelta { .. }
            | QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. }
            | QueryEvent::TurnComplete { .. }
            | QueryEvent::Usage { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        retry_statuses,
        vec![
            ProviderRetryStatus {
                provider: "rate-limited-stream-create-provider".to_string(),
                model: model.clone(),
                attempt: 1,
                backoff_ms: 60_000,
                phase: QueryProviderRetryPhase::Scheduled,
                message: "Retrying provider request in 60.0s".to_string(),
            },
            ProviderRetryStatus {
                provider: "rate-limited-stream-create-provider".to_string(),
                model: model.clone(),
                attempt: 1,
                backoff_ms: 0,
                phase: QueryProviderRetryPhase::Resumed,
                message: "Retrying provider request now".to_string(),
            },
            ProviderRetryStatus {
                provider: "rate-limited-stream-create-provider".to_string(),
                model: model.clone(),
                attempt: 2,
                backoff_ms: 60_000,
                phase: QueryProviderRetryPhase::Scheduled,
                message: "Retrying provider request in 60.0s".to_string(),
            },
            ProviderRetryStatus {
                provider: "rate-limited-stream-create-provider".to_string(),
                model,
                attempt: 2,
                backoff_ms: 0,
                phase: QueryProviderRetryPhase::Resumed,
                message: "Retrying provider request now".to_string(),
            },
        ]
    );
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn query_cancels_stream_creation_retry_backoff() {
    let provider = Arc::new(TransientStreamCreateProvider {
        attempts: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let result = query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider_sdk,
        registry,
        &runtime,
        None,
        QueryOptions {
            cancel_token: Some(cancel_token),
            ..QueryOptions::default()
        },
    )
    .await;

    assert!(matches!(result, Err(AgentError::Aborted)));
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn query_cancels_stream_event_retry_backoff() {
    let provider = Arc::new(TransientStreamEventProvider {
        attempts: AtomicUsize::new(0),
    });
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let result = query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider_sdk,
        registry,
        &runtime,
        None,
        QueryOptions {
            cancel_token: Some(cancel_token),
            ..QueryOptions::default()
        },
    )
    .await;

    assert!(matches!(result, Err(AgentError::Aborted)));
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn query_exposes_stable_tools_and_appends_subagent_warning() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let mut builder = ToolRegistryBuilder::new();
    builder.push_spec_with_exposure(
        ToolSpec::new(
            "ToolSearch",
            "Search available tools.",
            JsonSchema::object(Default::default(), None, None),
        ),
        ToolExposure::Direct,
    );
    builder.push_spec_with_exposure(
        ToolSpec::new(
            "web_search",
            "Search the web.",
            JsonSchema::object(Default::default(), None, None),
        ),
        ToolExposure::Direct,
    );
    for (name, description) in [
        ("spawn_agent", "Create a child agent."),
        ("send_message", "Send input to a child agent."),
        ("await_task", "Wait for task completion."),
        ("list_tasks", "List child tasks."),
        ("cancel_task", "Cancel a child task."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(
                name,
                description,
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
    }
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_with_context(
        Arc::clone(&registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext {
            agent_scope: ToolAgentScope::Subagent,
            ..ToolRuntimeContext::default()
        },
    );
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("work on the delegated task"));
    let mut turn_config = TurnConfig::new(
        Model {
            base_instructions: "base system".to_string(),
            ..Model::default()
        },
        None,
    );
    turn_config.web_search =
        devo_config::ResolvedWebSearchConfig::Local(devo_config::ResolvedLocalWebSearchConfig {
            provider_id: "test".to_string(),
            kind: devo_config::LocalWebSearchProviderKind::Exa,
            api_key: "secret".to_string(),
            base_url: None,
            max_results: None,
        });

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let request = &captured[0];
    let tool_names = request
        .tools
        .as_ref()
        .expect("tools should be present")
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["ToolSearch", "web_search"]);
    let system = request.system.as_deref().expect("system prompt");
    let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
    assert!(system.contains("base system"));
    assert!(system.contains(&mode_prompt));
    assert!(system.contains("Sources:"));
    assert!(
        !request
            .system
            .as_deref()
            .unwrap_or_default()
            .contains("web_search")
    );
    assert!(
        !request
            .system
            .as_deref()
            .unwrap_or_default()
            .contains("spawn_agent")
    );

    assert!(
        request
            .messages
            .iter()
            .all(|message| !message_contains(message, "web_search: Search the web."))
    );
    let subagent_reminder_index =
        request_message_index_containing(request, "You are running as a sub-agent");
    let task_index = request_message_index_containing(request, "work on the delegated task");
    assert!(subagent_reminder_index < task_index);
    assert!(
        request
            .messages
            .iter()
            .any(|message| message_contains(message, "<context_changes>"))
    );
}

#[tokio::test]
async fn query_adds_web_search_prompt_for_provider_hosted_search() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("search current docs"));
    let mut turn_config = TurnConfig::new(
        Model {
            base_instructions: "base system".to_string(),
            ..Model::default()
        },
        None,
    );
    turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let request = &captured[0];
    let system = request.system.as_deref().expect("system prompt");

    assert!(system.contains("base system"));
    assert!(system.contains("Sources:"));
    assert!(system.contains("The current month is "));
    assert!(matches!(
        request.hosted_tools.as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
    assert!(
        request
            .tools
            .as_ref()
            .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
    );
}

/// Trace: L2-DES-RESEARCH-001
/// Verifies: provider-hosted web_search emits normal tool events with hosted output.
#[tokio::test]
async fn provider_hosted_web_search_emits_tool_events_without_local_execution() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebSearchProvider {
        requests: Arc::clone(&requests),
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "web_search",
        Arc::new(CountingWebSearchTool {
            executions: Arc::clone(&executions),
        }),
    );
    builder.push_spec(ToolSpec {
        name: "web_search".into(),
        description: "Search the web.".into(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("search current docs"));
    let mut turn_config = TurnConfig::new(Model::default(), None);
    turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            seen_clone.lock().unwrap().push(event);
        })
    });

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let request = &captured[0];
    assert!(matches!(
        request.hosted_tools.as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
    assert!(
        request
            .tools
            .as_ref()
            .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
    );
    let continuation = &captured[1];
    assert!(continuation.messages.iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                RequestContent::HostedToolUse {
                    id,
                    name,
                    input,
                    output: Some(_),
                    status,
                } if id == "hosted_ws_1"
                    && name == "web_search"
                    && input == &json!({ "query": "current Rust docs" })
                    && status.as_deref() == Some("completed")
            )
        })
    }));

    let events = seen.lock().unwrap();
    let starts = events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolUseStart { id, name, input } => {
                Some((id.as_str(), name.as_str(), input.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        vec![(
            "hosted_ws_1",
            "web_search",
            json!({ "query": "current Rust docs" })
        )]
    );
    let results = events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                is_error,
                ..
            } => Some((
                tool_use_id.as_str(),
                tool_name.as_str(),
                input.clone(),
                content,
                *is_error,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    let (tool_use_id, tool_name, input, content, is_error) = &results[0];
    assert_eq!(*tool_use_id, "hosted_ws_1");
    assert_eq!(*tool_name, "web_search");
    assert_eq!(input, &json!({ "query": "current Rust docs" }));
    assert!(!*is_error);
    assert!(matches!(
        *content,
        ToolContent::Mixed {
            text: Some(text),
            json: Some(json),
        } if text == "status: completed"
            && json == &json!({
                "results": [
                    {
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }
                ]
            })
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        QueryEvent::TurnComplete {
            stop_reason: StopReason::EndTurn
        }
    )));
    assert!(session.messages.iter().all(|message| {
        message.content.iter().all(|block| {
            !matches!(
                block,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )
        })
    }));
}

/// Trace: L2-DES-RESEARCH-001
/// Verifies: DSML text that represents a provider-hosted web_search does not end the query loop.
#[tokio::test]
async fn provider_hosted_dsml_text_tool_call_continues_query_loop() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedDsmlTextProvider {
        requests: Arc::clone(&requests),
    });
    let mut builder = ToolRegistryBuilder::new();
    for (name, description) in [
        ("spawn_agent", "Create a child agent."),
        ("await_task", "Wait for task completion."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(
                name,
                description,
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
    }
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("search current docs"));
    let mut turn_config = TurnConfig::new(Model::default(), None);
    turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            seen_clone.lock().unwrap().push(event);
        })
    });

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should continue after DSML text and complete");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let request = &captured[0];
    assert!(matches!(
        request.hosted_tools.as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
    let continuation = &captured[1];
    assert!(continuation.messages.iter().any(|message| {
        message_contains(message, "DSML tagged tool-call text")
            && message_contains(message, "spawn_agent")
            && message_contains(message, "await_task")
            && message_contains(message, "web_search")
    }));

    let assistant_messages = session
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_messages,
        vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: HOSTED_DSML_TEXT.to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
            },
        ]
    );

    let turn_completes = seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            QueryEvent::TurnComplete { stop_reason } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(turn_completes, vec![StopReason::EndTurn]);
}

/// Trace: L2-DES-RESEARCH-001
/// Verifies: provider-hosted web_fetch emits normal tool events with hosted output.
#[tokio::test]
async fn provider_hosted_web_fetch_emits_tool_events_without_local_execution() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebFetchProvider {
        requests: Arc::clone(&requests),
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "webfetch",
        Arc::new(CountingWebFetchTool {
            executions: Arc::clone(&executions),
        }),
    );
    builder.push_spec(ToolSpec {
        name: "webfetch".into(),
        description: "Fetch a URL.".into(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Mixed,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("fetch docs"));
    let mut turn_config = TurnConfig::new(Model::default(), None);
    turn_config.web_fetch = devo_config::ResolvedWebFetchConfig::Provider;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            seen_clone.lock().unwrap().push(event);
        })
    });

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let request = &captured[0];
    assert!(matches!(
        request.hosted_tools.as_slice(),
        [devo_protocol::HostedToolDefinition::WebFetch(_)]
    ));
    assert!(
        request
            .tools
            .as_ref()
            .is_none_or(|tools| tools.iter().all(|tool| tool.name != "webfetch"))
    );
    let continuation = &captured[1];
    assert!(continuation.messages.iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                RequestContent::HostedToolUse {
                    id,
                    name,
                    input,
                    output: Some(_),
                    status,
                } if id == "hosted_wf_1"
                    && name == "web_fetch"
                    && input == &json!({ "url": "https://example.test/docs" })
                    && status.as_deref() == Some("completed")
            )
        })
    }));

    let events = seen.lock().unwrap();
    let starts = events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolUseStart { id, name, input } => {
                Some((id.as_str(), name.as_str(), input.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        vec![(
            "hosted_wf_1",
            "web_fetch",
            json!({ "url": "https://example.test/docs" })
        )]
    );
    let results = events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                is_error,
                ..
            } => Some((
                tool_use_id.as_str(),
                tool_name.as_str(),
                input.clone(),
                content,
                *is_error,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    let (tool_use_id, tool_name, input, content, is_error) = &results[0];
    assert_eq!(*tool_use_id, "hosted_wf_1");
    assert_eq!(*tool_name, "web_fetch");
    assert_eq!(input, &json!({ "url": "https://example.test/docs" }));
    assert!(!*is_error);
    assert!(matches!(
        *content,
        ToolContent::Mixed {
            text: Some(text),
            json: Some(json),
        } if text == "status: completed"
            && json == &json!({
                "title": "Docs",
                "url": "https://example.test/docs"
            })
    ));
}

#[tokio::test]
async fn query_exposes_apply_patch_only_for_openai_channel() {
    // Non-OpenAI models often produce malformed apply_patch input, so the tool
    // is gated to the OpenAI channel only.
    async fn tool_names_for_channel(channel: Option<&str>) -> Vec<String> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "apply_patch",
                "Apply a patch.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "write",
                "Write a file.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            channel: channel.map(str::to_string),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            provider,
            registry,
            &runtime,
            None,
            QueryOptions::default(),
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        captured[0]
            .tools
            .as_ref()
            .expect("tools should be present")
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    }

    assert_eq!(
        tool_names_for_channel(Some("OpenAI")).await,
        vec!["apply_patch".to_string(), "write".to_string()]
    );
    assert_eq!(
        tool_names_for_channel(Some("Poolside")).await,
        vec!["write".to_string()]
    );
    assert_eq!(
        tool_names_for_channel(/*channel*/ None).await,
        vec!["write".to_string()]
    );
}

#[test]
fn subagent_reminder_insertion_preserves_tool_result_adjacency() {
    let mut messages = vec![
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content: vec![RequestContent::Text {
                text: "child task input".to_string(),
            }],
        },
        RequestMessage {
            role: Role::Assistant.as_str().to_string(),
            content: vec![RequestContent::ToolUse {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                input: json!({}),
            }],
        },
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content: vec![RequestContent::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
            }],
        },
    ];

    insert_subagent_request_reminders(&mut messages);

    assert!(message_contains(
        &messages[0],
        "You are running as a sub-agent"
    ));
    assert!(message_contains(&messages[1], "child task input"));
    assert!(
        matches!(messages[2].content.as_slice(), [RequestContent::ToolUse { id, .. }] if id == "tool-1")
    );
    assert!(
        matches!(messages[3].content.as_slice(), [RequestContent::ToolResult { tool_use_id, .. }] if tool_use_id == "tool-1")
    );
}

fn request_message_index_containing(request: &ModelRequest, needle: &str) -> usize {
    request
        .messages
        .iter()
        .position(|message| message_contains(message, needle))
        .unwrap_or_else(|| panic!("expected request message containing {needle:?}: {request:?}"))
}

fn message_contains(message: &RequestMessage, needle: &str) -> bool {
    message
        .content
        .iter()
        .any(|content| matches!(content, RequestContent::Text { text } if text.contains(needle)))
}

fn active_goal(objective: &str) -> ThreadGoal {
    ThreadGoal {
        thread_id: devo_protocol::SessionId::new(),
        objective: objective.to_string(),
        status: ThreadGoalStatus::Active,
        token_budget: Some(10_000),
        tokens_used: 250,
        time_used_seconds: 0,
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn query_uses_session_permission_mode_for_mutating_tools() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(MutatingTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: "A test-only mutating tool.".into(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let deny_checker = PermissionChecker::new(|request| {
        let n = request.tool_name;
        Box::pin(async move { Err(format!("{n} denied")) })
    });
    let runtime = ToolRuntime::new(Arc::clone(&registry), deny_checker);

    let mut session = SessionState::new(
        SessionConfig {
            permission_mode: PermissionMode::Deny,
            ..Default::default()
        },
        std::env::temp_dir(),
    );
    session.push_message(Message::user("run the tool"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete and append a tool_result");

    let tool_result_message = session
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        })
        .expect("tool_result message should be appended");
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
    } = &tool_result_message.content[0]
    else {
        panic!("expected tool_result content block");
    };

    assert_eq!(tool_use_id, "tool-1");
    assert!(
        *is_error,
        "denied permission should surface as a tool error"
    );
    assert!(
        content.contains("permission denied"),
        "expected tool_result to mention permission denial, got: {content}"
    );
}

#[tokio::test]
async fn query_resolves_reasoning_model_variant_before_building_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "kimi-k2.5".into(),
        display_name: "Kimi K2.5".into(),
        provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        description: None,
        reasoning_capability: ReasoningCapability::Toggle,
        default_reasoning_effort: Some(ReasoningEffort::Medium),
        reasoning_implementation: Some(ReasoningImplementation::ModelVariant(
            ReasoningVariantConfig {
                variants: vec![
                    ReasoningVariant {
                        selection_value: "disabled".into(),
                        model_slug: "kimi-k2.5".into(),
                        reasoning_effort: None,
                        label: "Off".into(),
                        description: "Use the standard model".into(),
                        extra_body: None,
                    },
                    ReasoningVariant {
                        selection_value: "enabled".into(),
                        model_slug: "kimi-k2.5-thinking".into(),
                        reasoning_effort: Some(ReasoningEffort::Medium),
                        label: "On".into(),
                        description: "Use the reasoning model".into(),
                        extra_body: None,
                    },
                ],
            },
        )),
        base_instructions: String::new(),
        context_window: 200_000,
        effective_context_window_percent: None,
        truncation_policy: TruncationPolicyConfig {
            mode: TruncationMode::Tokens,
            limit: 10_000,
        },
        input_modalities: vec![],
        supports_image_detail_original: false,
        channel: None,
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));

    query(
        &mut session,
        &TurnConfig::with_request_model(
            model,
            "vendor/kimi-k2.5".into(),
            HashMap::from([(
                "kimi-k2.5-thinking".into(),
                "vendor/kimi-k2.5-thinking".into(),
            )])
            .into(),
            Some("enabled".into()),
        ),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].model, "vendor/kimi-k2.5-thinking");
    assert_eq!(captured[0].request_thinking, None);
}

#[tokio::test]
async fn query_sends_turn_config_request_model_to_provider() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "catalog-slug".into(),
        display_name: "Catalog Model".into(),
        base_instructions: "catalog instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));

    query(
        &mut session,
        &TurnConfig::with_request_model(
            model,
            "vendor/model-name".into(),
            HashMap::new().into(),
            /*reasoning_effort_selection*/ None,
        ),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].model, "vendor/model-name");
    assert_eq!(
        session
            .session_context
            .as_ref()
            .expect("session context")
            .model
            .slug,
        "catalog-slug"
    );
}

/// Trace: L2-DES-CONTEXT-001
/// Verifies: Plan turns append the active Plan collaboration prompt to the provider system prompt.
#[tokio::test]
async fn query_appends_plan_mode_reminder_to_system_prompt() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "model-a".into(),
        base_instructions: "base instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.collaboration_mode = CollaborationMode::Plan;
    session.push_message(Message::user("plan this"));

    query(
        &mut session,
        &TurnConfig::new(model, None),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let system = captured[0].system.as_deref().expect("system prompt");
    let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
    assert!(system.contains("base instructions"));
    assert!(system.contains(&mode_prompt));
    let mode_index = request_message_index_containing(&captured[0], "<collaboration_mode>");
    assert!(message_contains(
        &captured[0].messages[mode_index],
        "<current>plan</current>"
    ));
}

/// Trace: L2-DES-CONTEXT-001
/// Verifies: Returning from Plan to Build uses Build system prompt and a lightweight mode diff.
#[tokio::test]
async fn query_inserts_mode_change_prompt_when_returning_to_build_mode() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "model-a".into(),
        base_instructions: "base instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.collaboration_mode = CollaborationMode::Plan;
    session.push_message(Message::user("plan this"));

    query(
        &mut session,
        &TurnConfig::new(model.clone(), None),
        Arc::clone(&provider),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("plan query should succeed");

    session.collaboration_mode = CollaborationMode::Build;
    session.push_message(Message::user("implement this"));
    query(
        &mut session,
        &TurnConfig::new(model, None),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("build query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].system, captured[1].system);
    let system = captured[1].system.as_deref().expect("system prompt");
    let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
    assert!(system.contains("base instructions"));
    assert!(system.contains(&mode_prompt));

    let mode_change_index =
        request_message_index_containing(&captured[1], "<transition>plan -> build</transition>");
    let request_index = request_message_index_containing(&captured[1], "implement this");
    assert!(mode_change_index < request_index);
    assert!(message_contains(
        &captured[1].messages[mode_change_index],
        "<previous>plan</previous>"
    ));
    assert!(message_contains(
        &captured[1].messages[mode_change_index],
        "<current>build</current>"
    ));
    assert!(message_contains(
        &captured[1].messages[mode_change_index],
        "<note>any previous instructions for other modes (e.g. Plan mode) are no longer active.</note>"
    ));
    assert!(!message_contains(
        &captured[1].messages[mode_change_index],
        "<collaboration_mode_build>"
    ));
    assert!(!message_contains(
        &captured[1].messages[mode_change_index],
        "<collaboration_mode_plan>"
    ));
}

#[tokio::test]
async fn query_inserts_goal_context_before_latest_user_request() {
    // Trace: L2-DES-GOAL-001
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "model-a".into(),
        base_instructions: "base instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.set_active_goal(active_goal("ship /goal"));
    session.push_message(Message::user("finish implementation"));

    query(
        &mut session,
        &TurnConfig::new(model, None),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert!(
        !captured[0]
            .system
            .as_deref()
            .unwrap_or_default()
            .contains("ship /goal")
    );
    let messages = &captured[0].messages;
    let goal_index = messages
        .iter()
        .position(|message| message_contains(message, "ship /goal"))
        .expect("goal context message");
    let request_index = messages
        .iter()
        .position(|message| message_contains(message, "finish implementation"))
        .expect("latest user request message");
    assert!(goal_index < request_index);
}

#[tokio::test]
async fn autonomous_goal_context_is_latest_request_after_completed_turn() {
    // Trace: L2-DES-GOAL-001
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "model-a".into(),
        base_instructions: "base instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.set_active_goal(active_goal("continue the active goal"));
    session.push_message(Message::user("older user prompt"));
    session.push_message(Message::assistant_text("older assistant reply"));

    query(
        &mut session,
        &TurnConfig::new(model, None),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let messages = &captured[0].messages;
    let goal_index = messages
        .iter()
        .position(|message| message_contains(message, "continue the active goal"))
        .expect("goal context message");
    let assistant_index = messages
        .iter()
        .position(|message| message_contains(message, "older assistant reply"))
        .expect("assistant history message");
    assert!(goal_index > assistant_index);
    assert_eq!(goal_index, messages.len() - 1);
}

#[tokio::test]
async fn query_locks_system_prompt_and_environment_prefix_per_session() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let temp_root = std::env::temp_dir().join(format!("devo-query-lock-{}", uuid::Uuid::new_v4()));
    let second_cwd = temp_root.join("nested");
    let first_model = Model {
        slug: "model-a".into(),
        base_instructions: "base-a".into(),
        ..Model::default()
    };
    let second_model = Model {
        slug: "model-b".into(),
        base_instructions: "base-b".into(),
        ..Model::default()
    };

    let mut session = SessionState::new(SessionConfig::default(), temp_root.clone());
    session.push_message(Message::user("hello"));

    query(
        &mut session,
        &TurnConfig::new(first_model, None),
        Arc::clone(&provider),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    session.cwd = second_cwd;
    session.push_message(Message::user("follow up"));

    query(
        &mut session,
        &TurnConfig::new(second_model, Some("enabled".into())),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
    let expected_system = format!("base-a\n\n{mode_prompt}");
    assert_eq!(
        captured[0].system.as_deref(),
        Some(expected_system.as_str())
    );
    assert_eq!(
        captured[1].system.as_deref(),
        Some(expected_system.as_str())
    );

    let first_prefix = &captured[0].messages[0];
    let second_prefix = &captured[1].messages[0];
    assert_eq!(first_prefix.role, second_prefix.role);
    let devo_protocol::RequestContent::Text { text: first_text } = &first_prefix.content[0] else {
        panic!("expected text prefix");
    };
    let devo_protocol::RequestContent::Text { text: second_text } = &second_prefix.content[0]
    else {
        panic!("expected text prefix");
    };
    assert_eq!(first_text, second_text);
}

#[tokio::test]
async fn query_inserts_context_diff_before_changed_turn_input() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    let first_model = Model {
        slug: "model-a".into(),
        ..Model::default()
    };
    let second_model = Model {
        slug: "model-b".into(),
        ..Model::default()
    };

    session.push_message(Message::user("hello"));
    query(
        &mut session,
        &TurnConfig::new(first_model, None),
        Arc::clone(&provider),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(second_model, Some("enabled".into())),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let diff_message = &session.messages[session.messages.len() - 3];
    let user_message = &session.messages[session.messages.len() - 2];
    assert_eq!(user_message, &Message::user("follow up"));
    let ContentBlock::Text { text } = &diff_message.content[0] else {
        panic!("expected text diff message");
    };
    assert!(text.contains("<context_changes>"));
    assert!(text.contains("<metadata>"));
    assert!(text.contains("<name>model</name>"));
    assert!(text.contains("<previous>model-a</previous>"));
    assert!(text.contains("<current>model-b</current>"));
}

#[tokio::test]
async fn query_skips_context_diff_when_turn_metadata_unchanged() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "model-a".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());

    session.push_message(Message::user("hello"));
    query(
        &mut session,
        &TurnConfig::new(model.clone(), None),
        Arc::clone(&provider),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(model, None),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let follow_up_index = request_message_index_containing(&captured[1], "follow up");
    assert!(
        follow_up_index > 0,
        "follow-up user message should not be the first prompt message"
    );
    assert!(
        !message_contains(
            &captured[1].messages[follow_up_index - 1],
            "<context_changes>"
        ),
        "unchanged metadata after a completed turn should not insert a new context_changes before the next user message"
    );
}

#[tokio::test]
async fn query_inserts_interrupted_notice_before_next_user_message() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    session.push_message(Message::assistant_text("partial"));
    session.mark_last_turn_interrupted();
    session.push_message(Message::user("continue please"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let abort_index = session
        .messages
        .iter()
        .position(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text } if text.contains("<turn_aborted>")
                )
            })
        })
        .expect("interrupted notice should be inserted");
    let continue_index = session
        .messages
        .iter()
        .position(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text } if text.contains("continue please")
                )
            })
        })
        .expect("user message should remain");
    assert!(abort_index < continue_index);
    assert!(!session.last_turn_interrupted);
}

#[tokio::test]
async fn query_pairs_interrupted_tool_result_when_cancel_fires_during_tool() {
    #[derive(Debug)]
    struct HangingMutatingTool;

    #[async_trait]
    impl ToolHandler for HangingMutatingTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec {
                name: "mutating_tool".into(),
                description: "hangs until cancelled".into(),
                input_schema: JsonSchema::object(Default::default(), None, None),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![],
                supports_parallel: false,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            }))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            std::future::pending::<()>().await;
            unreachable!("tool should be cancelled")
        }
    }

    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(HangingMutatingTool));
    builder.push_spec(crate::tools::tool_spec::ToolSpec {
        name: "mutating_tool".into(),
        description: "hangs until cancelled".into(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let runtime = ToolRuntime::new_with_context_and_options(
        Arc::clone(&registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext::default(),
        ToolExecutionOptions {
            cancel_token: cancel_token.clone(),
            ..ToolExecutionOptions::default()
        },
    );
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));
    let turn_config = TurnConfig::new(Model::default(), None);
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(SingleToolUseProvider {
        requests: AtomicUsize::new(0),
    });

    let result = {
        let mut query_future = std::pin::pin!(query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
            QueryOptions {
                cancel_token: Some(cancel_token),
                ..QueryOptions::default()
            },
        ));
        tokio::select! {
            result = &mut query_future => {
                panic!("query completed before cancel: {result:?}");
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                cancel_for_task.cancel();
            }
        }
        query_future.await
    };
    assert!(matches!(result, Err(AgentError::Aborted)));

    let tool_use = session.messages.iter().find(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "tool-1"))
    });
    assert!(tool_use.is_some(), "tool call should be retained");

    let tool_result = session.messages.iter().find_map(|message| {
        message.content.iter().find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == "tool-1" => Some((content.clone(), *is_error)),
            _ => None,
        })
    });
    let (content, is_error) = tool_result.expect("interrupted tool result should exist");
    assert!(is_error);
    assert_eq!(content, crate::tools::INTERRUPTED_TOOL_RESULT_MESSAGE);
}

#[tokio::test]
async fn query_drops_orphaned_tool_calls_from_prompt_history() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());

    session.push_message(Message::user("first"));
    session.push_message(Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text {
                text: "Calling tool".into(),
            },
            ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "bash".into(),
                input: json!({ "cmd": "pwd" }),
            },
        ],
    });
    session.push_message(Message::user("follow up"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert!(
        captured[0]
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .all(|content| !matches!(content, devo_protocol::RequestContent::ToolUse { .. })),
        "expected orphaned tool calls to be removed from prompt history"
    );
}

#[tokio::test]
async fn test_model_connection_sends_minimal_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = CapturingProvider {
        requests: Arc::clone(&requests),
    };
    let model = Model {
        slug: "glm-4.5".into(),
        reasoning_capability: devo_protocol::ReasoningCapability::Toggle,
        top_p: Some(0.95),
        ..Model::default()
    };
    let preview = test_model_connection(
        &provider,
        &model,
        devo_protocol::ModelProfileKey::CatalogSlug(model.slug.clone()),
        "renamed-provider-model",
        "Reply with OK only.",
    )
    .await
    .expect("probe request should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(preview, "done");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].model_slug,
        devo_protocol::ModelProfileKey::CatalogSlug("glm-4.5".to_string())
    );
    assert_eq!(captured[0].model, "renamed-provider-model");
    assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
    assert_eq!(captured[0].system, None);
    assert!(captured[0].tools.is_none());
    assert_eq!(captured[0].messages.len(), 1);
    assert_eq!(captured[0].sampling.top_p, Some(0.95));
}

#[tokio::test]
async fn query_persists_streamed_reasoning_for_follow_up_request() {
    struct ReasoningProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for ReasoningProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::ReasoningStart { index: 0 }),
                Ok(StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "plan".into(),
                }),
                Ok(StreamEvent::TextStart { index: 1 }),
                Ok(StreamEvent::TextDelta {
                    index: 1,
                    text: "final".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-3".into(),
                        content: vec![ResponseContent::Text("final".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata {
                            extras: vec![ResponseExtra::ReasoningText {
                                text: "plan".into(),
                            }],
                        },
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "reasoning-provider"
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ReasoningProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let seen_events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&seen_events);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let callback_events = Arc::clone(&callback_events);
        Box::pin(async move {
            callback_events.lock().expect("lock callback").push(event);
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    {
        let events = seen_events.lock().expect("lock events");
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::ReasoningDelta(text) if text == "plan"
        )));
    }

    let assistant_message = session
        .messages
        .iter()
        .find(|message| matches!(message.role, Role::Assistant))
        .expect("assistant message");
    assert_eq!(
        assistant_message,
        &Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "plan".into(),
                },
                ContentBlock::Text {
                    text: "final".into(),
                },
            ],
        }
    );

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let replayed_assistant = captured[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .expect("assistant replay");
    assert_eq!(
        serde_json::to_value(replayed_assistant).expect("serialize assistant replay"),
        json!({
            "role": "assistant",
            "content": [
                { "type": "reasoning", "text": "plan" },
                { "type": "text", "text": "final" }
            ]
        })
    );
}

#[tokio::test]
async fn query_round_trips_provider_reasoning_without_plain_reasoning() {
    struct SignedReasoningProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for SignedReasoningProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let content = if call == 0 {
                vec![
                    ResponseContent::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "signed plan",
                            "signature": "sig_123"
                        }),
                    },
                    ResponseContent::Text("first".into()),
                ]
            } else {
                vec![ResponseContent::Text("second".into())]
            };
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: format!("resp-{call}"),
                        content,
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "signed-reasoning-provider"
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SignedReasoningProvider {
        requests: Arc::clone(&requests),
        calls: AtomicUsize::new(0),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let seen_events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&seen_events);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let callback_events = Arc::clone(&callback_events);
        Box::pin(async move {
            callback_events.lock().expect("lock callback").push(event);
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    {
        let events = seen_events.lock().expect("lock events");
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::ReasoningDelta(text) if text == "signed plan"
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, QueryEvent::ReasoningCompleted))
        );
    }

    let assistant_message = session
        .messages
        .iter()
        .find(|message| matches!(message.role, Role::Assistant))
        .expect("assistant message");
    assert_eq!(
        assistant_message,
        &Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "signed plan",
                        "signature": "sig_123"
                    }),
                },
                ContentBlock::Text {
                    text: "first".into(),
                },
            ],
        }
    );

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let second_request_content = captured[1]
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .collect::<Vec<_>>();
    assert!(second_request_content.iter().any(|content| matches!(
        content,
        RequestContent::ProviderReasoning { provider, payload }
        if provider == "anthropic"
            && payload["thinking"] == json!("signed plan")
            && payload["signature"] == json!("sig_123")
    )));
    assert!(
        second_request_content
            .iter()
            .all(|content| !matches!(content, RequestContent::Reasoning { .. }))
    );
}

#[tokio::test]
async fn query_continues_deepseek_v4_thinking_only_end_turn_once() {
    struct ThinkingOnlyThenTextProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for ThinkingOnlyThenTextProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let content = if call == 0 {
                vec![ResponseContent::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "internal plan",
                        "signature": "sig_plan"
                    }),
                }]
            } else {
                vec![ResponseContent::Text("visible answer".into())]
            };
            let message_done = Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: format!("resp-{call}"),
                    content,
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            });
            let events = if call == 0 {
                vec![message_done]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "visible answer".into(),
                    }),
                    message_done,
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "thinking-only-then-text-provider"
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ThinkingOnlyThenTextProvider {
        requests: Arc::clone(&requests),
        calls: AtomicUsize::new(0),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "deepseek-v4-pro".into(),
        provider: devo_protocol::ProviderWireApi::AnthropicMessages,
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let seen_events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&seen_events);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let callback_events = Arc::clone(&callback_events);
        Box::pin(async move {
            callback_events.lock().expect("lock callback").push(event);
        })
    });

    query(
        &mut session,
        &TurnConfig::new(model, None),
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should continue once and finish with text");

    let session_message_tail = session.messages[session.messages.len() - 4..].to_vec();
    assert_eq!(
        session_message_tail,
        vec![
            Message::user("hello"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "internal plan",
                        "signature": "sig_plan"
                    }),
                }],
            },
            Message::user(super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT),
            Message::assistant_text("visible answer"),
        ]
    );

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let second_request_messages = &captured[1].messages;
    let second_request_tail = &second_request_messages[second_request_messages.len() - 3..];
    assert_eq!(
        serde_json::to_value(second_request_tail).expect("serialize second request messages"),
        json!([
            {
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello"
                }]
            },
            {
                "role": "assistant",
                "content": [{
                    "type": "provider_reasoning",
                    "provider": "anthropic",
                    "payload": {
                        "type": "thinking",
                        "thinking": "internal plan",
                        "signature": "sig_plan"
                    }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT
                }]
            }
        ])
    );

    let events = seen_events.lock().expect("lock events");
    let turn_complete_count = events
        .iter()
        .filter(|event| matches!(event, QueryEvent::TurnComplete { .. }))
        .count();
    assert_eq!(turn_complete_count, 1);
    assert!(events.iter().any(|event| matches!(
        event,
        QueryEvent::TextDelta(text) if text == "visible answer"
    )));
}

#[tokio::test]
async fn query_preserves_provider_reasoning_and_hosted_tool_order() {
    struct OrderedHostedProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for OrderedHostedProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let content = if call == 0 {
                vec![
                    ResponseContent::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "before tool",
                            "signature": "sig_before"
                        }),
                    },
                    ResponseContent::HostedToolUse {
                        id: "srvtool_1".into(),
                        name: "web_search".into(),
                        input: json!({"query": "desktop gui 2026"}),
                        output: None,
                        status: None,
                    },
                    ResponseContent::HostedToolUse {
                        id: "srvtool_1".into(),
                        name: "web_search".into(),
                        input: json!({}),
                        output: Some(json!([{"title": "result"}])),
                        status: Some("completed".into()),
                    },
                    ResponseContent::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "after tool",
                            "signature": "sig_after"
                        }),
                    },
                    ResponseContent::Text("final".into()),
                ]
            } else {
                vec![ResponseContent::Text("second".into())]
            };
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: format!("resp-{call}"),
                        content,
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "ordered-hosted-provider"
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(OrderedHostedProvider {
        requests: Arc::clone(&requests),
        calls: AtomicUsize::new(0),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    let assistant_message = session
        .messages
        .iter()
        .find(|message| matches!(message.role, Role::Assistant))
        .expect("assistant message");
    assert_eq!(
        assistant_message.content,
        vec![
            ContentBlock::ProviderReasoning {
                provider: "anthropic".into(),
                payload: json!({
                    "type": "thinking",
                    "thinking": "before tool",
                    "signature": "sig_before"
                }),
            },
            ContentBlock::HostedToolUse {
                id: "srvtool_1".into(),
                name: "web_search".into(),
                input: json!({"query": "desktop gui 2026"}),
                output: None,
                status: None,
            },
            ContentBlock::HostedToolUse {
                id: "srvtool_1".into(),
                name: "web_search".into(),
                input: json!({"query": "desktop gui 2026"}),
                output: Some(json!([{"title": "result"}])),
                status: Some("completed".into()),
            },
            ContentBlock::ProviderReasoning {
                provider: "anthropic".into(),
                payload: json!({
                    "type": "thinking",
                    "thinking": "after tool",
                    "signature": "sig_after"
                }),
            },
            ContentBlock::Text {
                text: "final".into(),
            },
        ]
    );

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    let replayed_content = captured[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .expect("assistant replay")
        .content
        .clone();
    assert_eq!(
        serde_json::to_value(&replayed_content).expect("serialize replayed content"),
        json!([
            {
                "type": "provider_reasoning",
                "provider": "anthropic",
                "payload": {
                    "type": "thinking",
                    "thinking": "before tool",
                    "signature": "sig_before"
                }
            },
            {
                "type": "hosted_tool_use",
                "id": "srvtool_1",
                "name": "web_search",
                "input": { "query": "desktop gui 2026" }
            },
            {
                "type": "hosted_tool_use",
                "id": "srvtool_1",
                "name": "web_search",
                "input": { "query": "desktop gui 2026" },
                "output": [{ "title": "result" }],
                "status": "completed"
            },
            {
                "type": "provider_reasoning",
                "provider": "anthropic",
                "payload": {
                    "type": "thinking",
                    "thinking": "after tool",
                    "signature": "sig_after"
                }
            },
            {
                "type": "text",
                "text": "final"
            }
        ])
    );
}

#[tokio::test]
async fn query_disables_openai_thinking_when_reasoning_context_is_missing() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(OpenAiCapturingProvider {
        requests: Arc::clone(&requests),
    });
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "deepseek-v4-flash".into(),
        provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        reasoning_capability: ReasoningCapability::Toggle,
        base_instructions: String::new(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::assistant_text("legacy assistant reply"));
    session.push_message(Message::user("follow up"));

    query(
        &mut session,
        &TurnConfig::new(model, Some("enabled".into())),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].model_slug,
        devo_protocol::ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string())
    );
    assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
    // Toggle capability does not set reasoning_effort on the request.
    assert_eq!(captured[0].reasoning_effort, None);
}

#[tokio::test]
async fn query_tool_result_summary_is_set() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(MutatingTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult { summary, .. } = event {
                seen_clone.lock().unwrap().push(summary);
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let summaries = seen.lock().unwrap();
    assert!(
        !summaries.is_empty(),
        "should have at least one ToolResult summary"
    );
    for summary in summaries.iter() {
        assert!(!summary.is_empty(), "summary should not be empty");
    }
}

#[tokio::test]
async fn query_tool_result_event_includes_final_tool_input() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult {
                tool_name, input, ..
            } = event
            {
                seen_clone.lock().unwrap().push((tool_name, input));
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(String::from("mutating_tool"), json!({ "value": 1 }))]
    );
}

#[tokio::test]
async fn query_tool_result_event_matches_input_delta_by_tool_index() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tools"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult {
                tool_use_id, input, ..
            } = event
            {
                seen_clone.lock().unwrap().push((tool_use_id, input));
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(InterleavedToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[
            (String::from("tool-1"), json!({ "value": 1 })),
            (String::from("tool-2"), json!({ "value": 2 })),
        ]
    );
}

#[tokio::test]
async fn query_truncates_model_visible_tool_results_but_emits_raw_tool_result_events() {
    let full_content = "abcdefghijklmnopqrstuvwxyz".to_string();
    let display_content = "raw display abcdefghijklmnopqrstuvwxyz".to_string();
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "mutating_tool",
        Arc::new(LargeToolResultTool {
            content: full_content.clone(),
            display_content: Some(display_content.clone()),
        }),
    );
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult {
                content,
                display_content,
                ..
            } = event
            {
                seen_clone
                    .lock()
                    .expect("lock seen events")
                    .push((content.into_string(), display_content));
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(
            Model {
                truncation_policy: TruncationPolicyConfig::bytes(20),
                ..Model::default()
            },
            None,
        ),
        Arc::new(CapturingToolUseProvider {
            requests: Arc::clone(&requests),
            calls: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock().expect("lock seen events").as_slice(),
        &[(full_content.clone(), Some(display_content))]
    );

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let model_visible_tool_result = captured[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            RequestContent::ToolResult { content, .. } => Some(content.as_str()),
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolUse { .. } => None,
        })
        .expect("continuation request should include tool result");
    assert_eq!(model_visible_tool_result, "abcde\n...[truncated]");
}

#[tokio::test]
async fn query_tool_start_event_includes_final_tool_input() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tools"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolUseStart { id, input, .. } = event {
                seen_clone.lock().unwrap().push((id, input));
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(InterleavedToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[
            (String::from("tool-1"), json!({ "value": 1 })),
            (String::from("tool-2"), json!({ "value": 2 })),
        ]
    );
}

#[tokio::test]
#[ignore = "legacy progress mechanism replaced by L3 contracts"]
async fn query_emits_tool_result_display_content() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult {
                content,
                display_content,
                ..
            } = event
            {
                seen_clone.lock().unwrap().push((content, display_content));
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(matches!(
        &seen[0],
        (crate::tools::ToolContent::Text(text), Some(display))
            if text == "canonical" && display == "display"
    ));
}

#[tokio::test]
#[ignore = "legacy progress mechanism replaced by L3 contracts"]
async fn query_emits_tool_progress_before_tool_result() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(StreamingMutatingTool));
    builder.push_spec(ToolSpec {
        name: "mutating_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            seen_clone.lock().unwrap().push(event);
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let events = seen.lock().unwrap();
    let progress_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                QueryEvent::ToolProgress {
                    tool_use_id,
                    progress: crate::tools::ToolProgress::OutputDelta { delta },
                } if tool_use_id == "tool-1" && delta == "stream chunk\n"
            )
        })
        .expect("tool progress event should be emitted");
    let result_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-1"
                        && matches!(content, crate::tools::ToolContent::Text(text) if text == "stream complete")
                        && !is_error
                )
            })
            .expect("tool result event should be emitted");

    assert!(
        progress_index < result_index,
        "tool progress should arrive before final result"
    );
}

#[tokio::test]
async fn query_emits_parallel_tool_results_as_each_tool_finishes() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("parallel_tool", Arc::new(ParallelDelayTool));
    builder.push_spec(ToolSpec {
        name: "parallel_tool".into(),
        description: String::new(),
        input_schema: JsonSchema::object(Default::default(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    });
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tools"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            match event {
                QueryEvent::ToolUseStart { id, .. } => {
                    seen_clone
                        .lock()
                        .expect("lock events")
                        .push(format!("start:{id}"));
                }
                QueryEvent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let content = content.into_string();
                    seen_clone
                        .lock()
                        .expect("lock events")
                        .push(format!("result:{tool_use_id}:{content}"));
                }
                _ => {}
            }
        })
    });

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(ParallelToolUseProvider {
            requests: AtomicUsize::new(0),
        }),
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock().expect("lock events").as_slice(),
        &[
            "start:slow".to_string(),
            "start:fast".to_string(),
            "result:fast:fast complete".to_string(),
            "result:slow:slow complete".to_string(),
        ]
    );

    let tool_result_ids = session
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_result_ids, vec!["slow", "fast"]);
}
