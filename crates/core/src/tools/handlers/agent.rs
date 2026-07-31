use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::AgentListParams;
use devo_protocol::AgentMessageParams;
use devo_protocol::AgentToolPolicy;
use devo_protocol::AwaitTaskParams;
use devo_protocol::CancelTaskParams;
use devo_protocol::CloseAgentParams;
use devo_protocol::ListTasksParams;
use devo_protocol::ParentAgentInfo;
use devo_protocol::ParentAgentListResult;
use devo_protocol::ParentSpawnAgentResult;
use devo_protocol::SessionId;
use devo_protocol::SpawnAgentParams;
use devo_protocol::TaskId;
use devo_protocol::WaitAgentParams;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgress;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::registry::ToolExposure;
use crate::registry::ToolRegistryBuilder;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolPreparationFeedback;
use crate::tool_spec::ToolSpec;
use crate::tools::background_tasks::BackgroundTaskStore;

#[derive(Clone, Copy)]
enum AgentToolKind {
    Spawn,
    SendMessage,
    Wait,
    List,
    Close,
    AwaitTask,
    ListTasks,
    CancelTask,
}

pub struct AgentToolHandler {
    spec: ToolSpec,
    kind: AgentToolKind,
    background_tasks: Arc<BackgroundTaskStore>,
}

impl AgentToolHandler {
    fn new(
        spec: ToolSpec,
        kind: AgentToolKind,
        background_tasks: Arc<BackgroundTaskStore>,
    ) -> Self {
        Self {
            spec,
            kind,
            background_tasks,
        }
    }
}

pub fn register_agent_tools(
    builder: &mut ToolRegistryBuilder,
    background_tasks: Arc<BackgroundTaskStore>,
) {
    let spawn = Arc::new(AgentToolHandler::new(
        spawn_spec(),
        AgentToolKind::Spawn,
        Arc::clone(&background_tasks),
    ));
    let send = Arc::new(AgentToolHandler::new(
        send_message_spec(),
        AgentToolKind::SendMessage,
        Arc::clone(&background_tasks),
    ));
    let wait = Arc::new(AgentToolHandler::new(
        wait_agent_spec(),
        AgentToolKind::Wait,
        Arc::clone(&background_tasks),
    ));
    let list = Arc::new(AgentToolHandler::new(
        list_agents_spec(),
        AgentToolKind::List,
        Arc::clone(&background_tasks),
    ));
    let close = Arc::new(AgentToolHandler::new(
        close_agent_spec(),
        AgentToolKind::Close,
        Arc::clone(&background_tasks),
    ));
    let await_task = Arc::new(AgentToolHandler::new(
        await_task_spec(),
        AgentToolKind::AwaitTask,
        Arc::clone(&background_tasks),
    ));
    let list_tasks = Arc::new(AgentToolHandler::new(
        list_tasks_spec(),
        AgentToolKind::ListTasks,
        Arc::clone(&background_tasks),
    ));
    let cancel_task = Arc::new(AgentToolHandler::new(
        cancel_task_spec(),
        AgentToolKind::CancelTask,
        background_tasks,
    ));

    register(builder, spawn, &["spawn_subagent", "subagent", "delegate"]);
    register(builder, send, &[]);
    register(builder, await_task, &[]);
    register(builder, list_tasks, &[]);
    register(builder, cancel_task, &[]);
    register_compatibility_handler(builder, wait, &["subagent_result"]);
    register_compatibility_handler(builder, list, &["subagent_status"]);
    register_compatibility_handler(builder, close, &[]);
}

fn register(builder: &mut ToolRegistryBuilder, handler: Arc<AgentToolHandler>, aliases: &[&str]) {
    builder.push_spec_with_exposure(handler.spec().clone(), ToolExposure::Direct);
    let handler: Arc<dyn ToolHandler> = handler;
    let name = handler.spec().name.clone();
    builder.register_handler(&name, Arc::clone(&handler));
    for alias in aliases {
        builder.register_handler(alias, Arc::clone(&handler));
    }
}

fn register_compatibility_handler(
    builder: &mut ToolRegistryBuilder,
    handler: Arc<AgentToolHandler>,
    aliases: &[&str],
) {
    let handler: Arc<dyn ToolHandler> = handler;
    let name = handler.spec().name.clone();
    builder.register_handler(&name, Arc::clone(&handler));
    for alias in aliases {
        builder.register_handler(alias, Arc::clone(&handler));
    }
}

#[async_trait]
impl ToolHandler for AgentToolHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let Some(coordinator) = ctx.agent_coordinator.clone() else {
            return Err(ToolCallError::NeedsConfiguration(
                "child agent coordination is not configured".to_string(),
            ));
        };
        let session_id = current_session_id(&ctx)?;
        match self.kind {
            AgentToolKind::Spawn => {
                let input: SpawnAgentInput = parse_input(input)?;
                let result = coordinator
                    .spawn_agent(SpawnAgentParams {
                        session_id,
                        message: input.message,
                        fork_turns: input.fork_turns,
                        max_turns: None,
                        tool_policy: AgentToolPolicy::Inherit,
                        ephemeral: false,
                    })
                    .await?;
                json_result(ParentSpawnAgentResult::from(result), "agent spawned")
            }
            AgentToolKind::SendMessage => {
                let input: AgentMessageInput = parse_input(input)?;
                let result = coordinator
                    .send_message(AgentMessageParams {
                        session_id,
                        target: input.target,
                        message: input.message,
                    })
                    .await?;
                json_result(result, "message delivered")
            }
            AgentToolKind::Wait => {
                if let Some(progress) = progress {
                    let _ = progress.send(ToolProgress::StatusUpdate {
                        message: "Waiting for subagent messages...".to_string(),
                        percent: None,
                    });
                }
                let input: WaitAgentInput = parse_input(input)?;
                let params = WaitAgentParams {
                    session_id,
                    target: input.target,
                    after_sequence: input.after_sequence,
                    timeout_secs: input.timeout_secs,
                };
                let result = tokio::select! {
                    result = coordinator.wait_agent(params) => result?,
                    _ = ctx.cancel_token.cancelled() => return Err(ToolCallError::Cancelled),
                };
                json_result(result, "wait completed")
            }
            AgentToolKind::List => {
                let input: ListAgentsInput = parse_input(input)?;
                let agents = coordinator
                    .list_agents(AgentListParams {
                        session_id,
                        path_prefix: input.path_prefix,
                    })
                    .await?;
                json_result(
                    ParentAgentListResult {
                        agents: agents.into_iter().map(ParentAgentInfo::from).collect(),
                    },
                    "agents listed",
                )
            }
            AgentToolKind::Close => {
                let input: CloseAgentInput = parse_input(input)?;
                let result = coordinator
                    .close_agent(CloseAgentParams {
                        session_id,
                        target: input.target,
                    })
                    .await?;
                json_result(result, "agent closed")
            }
            AgentToolKind::AwaitTask => {
                if let Some(progress) = progress {
                    let _ = progress.send(ToolProgress::StatusUpdate {
                        message: "Waiting for task completion...".to_string(),
                        percent: None,
                    });
                }
                let input: AwaitTaskInput = parse_input(input)?;
                if let Some(result) = self
                    .background_tasks
                    .await_task(
                        session_id,
                        &input.task_id,
                        std::time::Duration::from_secs(devo_protocol::resolve_wait_agent_timeout(
                            input.timeout_secs,
                        )),
                    )
                    .await
                {
                    return json_result(result, "task wait completed");
                }
                if input.task_id.0.starts_with("task-") {
                    return Err(ToolCallError::InvalidInput(format!(
                        "task not found: {}",
                        input.task_id.0
                    )));
                }
                let result = tokio::select! {
                    result = coordinator.await_task(AwaitTaskParams {
                        session_id,
                        task_id: input.task_id,
                        timeout_secs: input.timeout_secs,
                    }) => result?,
                    _ = ctx.cancel_token.cancelled() => return Err(ToolCallError::Cancelled),
                };
                json_result(result, "task wait completed")
            }
            AgentToolKind::ListTasks => {
                let input: ListTasksInput = parse_input(input)?;
                let mut result = coordinator
                    .list_tasks(ListTasksParams {
                        session_id,
                        path_prefix: input.path_prefix,
                    })
                    .await?;
                result
                    .tasks
                    .extend(self.background_tasks.list(session_id).await);
                result
                    .tasks
                    .sort_by(|left, right| left.task_id.0.cmp(&right.task_id.0));
                json_result(result, "tasks listed")
            }
            AgentToolKind::CancelTask => {
                let input: CancelTaskInput = parse_input(input)?;
                if let Some(task) = self
                    .background_tasks
                    .cancel(session_id, &input.task_id)
                    .await
                {
                    return json_result(devo_protocol::CancelTaskResult { task }, "task canceled");
                }
                if input.task_id.0.starts_with("task-") {
                    return Err(ToolCallError::InvalidInput(format!(
                        "task not found: {}",
                        input.task_id.0
                    )));
                }
                let result = coordinator
                    .cancel_task(CancelTaskParams {
                        session_id,
                        task_id: input.task_id,
                    })
                    .await?;
                json_result(result, "task canceled")
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct SpawnAgentInput {
    message: String,
    #[serde(default)]
    fork_turns: Option<String>,
}

#[derive(serde::Deserialize)]
struct AgentMessageInput {
    target: String,
    message: String,
}

#[derive(serde::Deserialize)]
struct WaitAgentInput {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    after_sequence: Option<u64>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
struct ListAgentsInput {
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(serde::Deserialize)]
struct CloseAgentInput {
    target: String,
}

#[derive(serde::Deserialize)]
struct AwaitTaskInput {
    task_id: TaskId,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
struct ListTasksInput {
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(serde::Deserialize)]
struct CancelTaskInput {
    task_id: TaskId,
}

fn current_session_id(ctx: &ToolContext) -> Result<SessionId, ToolCallError> {
    SessionId::try_from(ctx.session_id.clone()).map_err(|error| {
        ToolCallError::InvalidInput(format!("invalid current session id: {error}"))
    })
}

fn parse_input<T: serde::de::DeserializeOwned>(
    input: serde_json::Value,
) -> Result<T, ToolCallError> {
    serde_json::from_value(input).map_err(|error| ToolCallError::InvalidInput(error.to_string()))
}

fn json_result(
    value: impl serde::Serialize,
    summary: impl Into<String>,
) -> Result<ToolResult, ToolCallError> {
    let value = serde_json::to_value(value)
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    Ok(ToolResult::success(ToolResultContent::Json(value), summary))
}

fn spec(name: &str, description: &str, schema: JsonSchema) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
        output_mode: ToolOutputMode::StructuredJson,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn spawn_spec() -> ToolSpec {
    spec(
        "spawn_agent",
        "Launch a child-agent task for complex multi-step work. Task coordination tools (spawn_agent, send_message, await_task, list_tasks, cancel_task) are parent-only.\n\nTypical flow: spawn_agent -> await_task until terminal -> optionally send_message for a follow-up turn -> await_task again. Use list_tasks for nonblocking status and cancel_task to stop and close a task. Parallelize independent work whenever possible.\n\nChild output becomes model-visible through await_task only after the child turn reaches a terminal state.\n\nWriting the prompt:\n- Brief the agent like a colleague who just arrived: no shared conversation unless fork_turns provides history.\n- State goal, why it matters, what you already ruled out, and the expected deliverable.\n- Lookups: give exact commands. Investigations: give the question, not a brittle script.\n- Never delegate understanding with phrases like \"based on your findings, fix it.\" Include concrete paths, symbols, and constraints.\n\nWhen not to use:\n- Reading a known file path -> read tool.\n- Searching a symbol or string -> grep/code search.\n- Small scoped file reads -> read directly.\n\nExample: spawn_agent({message:\"Survey crates/server for await_task usage and summarize call sites.\"})",
        JsonSchema::object(
            BTreeMap::from([
                (
                    "message".to_string(),
                    JsonSchema::string(Some(
                        "Initial child task. Include goal, scope, relevant paths, and expected result format.",
                    )),
                ),
                (
                    "fork_turns".to_string(),
                    JsonSchema::string(Some(
                        "\"all\" (default): stable completed parent history, excludes the active parent turn. \"none\": only the task message.",
                    )),
                ),
            ]),
            Some(vec!["message".to_string()]),
            Some(false),
        ),
    )
}

fn send_message_spec() -> ToolSpec {
    spec(
        "send_message",
        "Send more input to an existing child-agent task. Completed tasks start a new child turn immediately; running tasks queue the message for the next turn.\n\nWhen to use:\n- Follow up after a completed turn on the same child.\n- Correct or narrow the task without spawning a duplicate worker.\n\nWhen not to use:\n- Collecting output -> await_task.\n- Checking task state -> list_tasks.\n- Stopping and closing a task -> cancel_task.\n\nEach send_message reactivates the same task id. After sending, call await_task again before treating the new output as final.\n\nExample: send_message({target:\"brave-apple\", message:\"Also check error paths in coordinator.rs.\"})",
        message_schema(),
    )
}

fn await_task_spec() -> ToolSpec {
    spec(
        "await_task",
        "Wait for one task to reach Completed, Failed, or Canceled. Streaming progress remains visible to the UI but does not wake the model. The timeout is an absolute deadline, not an inactivity timer. A timed-out result contains current task metadata without partial assistant output.",
        JsonSchema::object(
            BTreeMap::from([
                (
                    "task_id".to_string(),
                    JsonSchema::string(Some("Task id returned by spawn_agent or send_message.")),
                ),
                (
                    "timeout_secs".to_string(),
                    JsonSchema::integer(Some("Maximum seconds to wait (default 5, max 120).")),
                ),
            ]),
            Some(vec!["task_id".to_string()]),
            Some(false),
        ),
    )
}

fn list_tasks_spec() -> ToolSpec {
    spec(
        "list_tasks",
        "Return a nonblocking snapshot of child tasks. Agent tasks include session id, parent session id, path, nickname, role, and last task message. Public states are waiting_approval, running, completed, failed, and canceled.",
        JsonSchema::object(
            BTreeMap::from([(
                "path_prefix".to_string(),
                JsonSchema::string(Some("Optional agent_path prefix filter.")),
            )]),
            None,
            Some(false),
        ),
    )
}

fn cancel_task_spec() -> ToolSpec {
    spec(
        "cancel_task",
        "Cancel a child task, interrupt active work, close its agent session and spawn edge, and return the final task metadata.",
        JsonSchema::object(
            BTreeMap::from([(
                "task_id".to_string(),
                JsonSchema::string(Some("Task id returned by spawn_agent or send_message.")),
            )]),
            Some(vec!["task_id".to_string()]),
            Some(false),
        ),
    )
}

fn wait_agent_spec() -> ToolSpec {
    spec(
        "wait_agent",
        "Collect child assistant output and turn-completion status. Each assistant_message event is the full accumulated text for that turn, not token deltas.\n\nDecision tree:\n1. After spawn_agent or send_message -> wait_agent with a longer timeout_secs (e.g. 60-120) until a status event (completed/failed/interrupted/closed).\n2. If timed_out with no events -> list_agents; if still running, wait_agent again with a short timeout_secs (e.g. 2-5).\n3. If completed with assistant_message -> use the output; send_message only if more work is needed.\n4. If off-track, stuck, or user wants to stop -> close_agent.\n\nDo not read child transcript files mid-flight. Do not fabricate child results. If the user asks early, report list_agents status instead of guessing.\n\nExample: wait_agent({\"target\":\"brave-apple\",\"timeout_secs\":90}) -> on timed_out with no events, list_agents({}) then wait_agent({\"target\":\"brave-apple\",\"timeout_secs\":3})",
        JsonSchema::object(
            BTreeMap::from([
                (
                    "target".to_string(),
                    JsonSchema::string(Some(
                        "Child agent_nickname or agent_path from spawn_agent, list_agents, or prior wait_agent output. Omit to poll all direct children.",
                    )),
                ),
                (
                    "after_sequence".to_string(),
                    JsonSchema::integer(Some(
                        "Return events with sequence greater than this value. Omit on first poll to use the runtime cursor.",
                    )),
                ),
                (
                    "timeout_secs".to_string(),
                    JsonSchema::integer(Some("Wait up to this many seconds (default 5, max 120).")),
                ),
            ]),
            None,
            Some(false),
        ),
    )
}

fn list_agents_spec() -> ToolSpec {
    spec(
        "list_agents",
        "Lightweight status snapshot for child agents. Does not return assistant text and does not block.\n\nWhen to use:\n- Right after spawn_agent to confirm registration and copy agent_nickname/agent_path.\n- After wait_agent timed_out with no events to see if the child is still running.\n- Before send_message or close_agent when multiple children exist.\n- When the user asks for progress without needing findings yet.\n\nWhen not to use:\n- Collecting child findings -> wait_agent.\n- Stopping a child -> close_agent.\n\nStatus values: running, completed, failed, interrupted, closed, spawning. running with an empty wait_agent poll usually means the child is still working.\n\nExample: list_agents({})",
        JsonSchema::object(
            BTreeMap::from([(
                "path_prefix".to_string(),
                JsonSchema::string(Some("Optional agent_path prefix filter.")),
            )]),
            None,
            Some(false),
        ),
    )
}

fn close_agent_spec() -> ToolSpec {
    spec(
        "close_agent",
        "Stop a child agent, interrupt active work if needed, and record a terminal status event.\n\nWhen to use:\n- Child is off-track or producing useless work.\n- Child stays running with no useful progress after list_agents + short wait_agent polls.\n- User asks to cancel or you no longer need the worker.\n- You spawned the wrong worker and will not use its output.\n\nWhen not to use:\n- Collecting output from a healthy completed child -> wait_agent first, then close if cleanup is needed.\n- Checking status -> list_agents.\n- Sending corrections -> send_message.\n\nExample: close_agent({target:\"brave-apple\"})",
        JsonSchema::object(
            BTreeMap::from([(
                "target".to_string(),
                JsonSchema::string(Some(
                    "Child agent_nickname or agent_path from spawn_agent or list_agents.",
                )),
            )]),
            Some(vec!["target".to_string()]),
            Some(false),
        ),
    )
}

fn message_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "target".to_string(),
                JsonSchema::string(Some(
                    "Child agent_nickname or agent_path from spawn_agent or list_agents.",
                )),
            ),
            (
                "message".to_string(),
                JsonSchema::string(Some("Follow-up user message for the child agent.")),
            ),
        ]),
        Some(vec!["target".to_string(), "message".to_string()]),
        Some(false),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::contracts::ToolBudgets;

    #[derive(Debug, Default)]
    struct FakeAgentCoordinator {
        spawned: Mutex<Vec<SpawnAgentParams>>,
        messages: Mutex<Vec<AgentMessageParams>>,
    }

    #[derive(Debug, Default)]
    struct BlockingWaitCoordinator;

    #[async_trait]
    impl devo_tools::AgentToolCoordinator for FakeAgentCoordinator {
        async fn spawn_agent(
            self: Arc<Self>,
            params: SpawnAgentParams,
        ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
            self.spawned.lock().await.push(params);
            let child_session_id = SessionId::new();
            Ok(devo_protocol::SpawnAgentResult {
                task_id: TaskId::from(child_session_id),
                child_session_id,
                agent_path: "root/reviewer".to_string(),
                agent_nickname: "reviewer".to_string(),
                status: "running".to_string(),
            })
        }

        async fn send_message(
            self: Arc<Self>,
            params: AgentMessageParams,
        ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
            let child_session_id = SessionId::try_from(params.target.as_str())
                .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
            self.messages.lock().await.push(params);
            Ok(devo_protocol::AgentMessageResult {
                delivered: true,
                task_id: TaskId::from(child_session_id),
            })
        }

        async fn wait_agent(
            self: Arc<Self>,
            _params: devo_protocol::WaitAgentParams,
        ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
            Ok(devo_protocol::WaitAgentResult {
                events: Vec::new(),
                next_sequence: 1,
                timed_out: false,
            })
        }

        async fn list_agents(
            self: Arc<Self>,
            _params: AgentListParams,
        ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
            Ok(Vec::new())
        }

        async fn close_agent(
            self: Arc<Self>,
            _params: CloseAgentParams,
        ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
            Ok(devo_protocol::CloseAgentResult {
                closed: true,
                status: "closed".to_string(),
            })
        }
    }

    #[async_trait]
    impl devo_tools::AgentToolCoordinator for BlockingWaitCoordinator {
        async fn spawn_agent(
            self: Arc<Self>,
            _params: SpawnAgentParams,
        ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn send_message(
            self: Arc<Self>,
            _params: AgentMessageParams,
        ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn wait_agent(
            self: Arc<Self>,
            _params: devo_protocol::WaitAgentParams,
        ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
            std::future::pending().await
        }

        async fn list_agents(
            self: Arc<Self>,
            _params: AgentListParams,
        ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn close_agent(
            self: Arc<Self>,
            _params: CloseAgentParams,
        ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }
    }

    #[tokio::test]
    async fn spawn_handler_delegates_to_coordinator() {
        let session_id = SessionId::new();
        let coordinator = Arc::new(FakeAgentCoordinator::default());
        let handler =
            AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn, test_background_tasks());
        let result = handler
            .handle(
                tool_context(
                    session_id,
                    Some(coordinator.clone() as Arc<dyn devo_tools::AgentToolCoordinator>),
                ),
                serde_json::json!({
                    "message": "review this",
                    "fork_turns": "all"
                }),
                None,
            )
            .await
            .expect("spawn succeeds");

        assert_eq!(result.result_summary, "agent spawned");
        assert_eq!(
            coordinator.spawned.lock().await.as_slice(),
            &[SpawnAgentParams {
                session_id,
                message: "review this".to_string(),
                fork_turns: Some("all".to_string()),
                max_turns: None,
                tool_policy: AgentToolPolicy::Inherit,
                ephemeral: false,
            }]
        );
    }

    #[tokio::test]
    async fn send_message_handler_delivers_parent_message_to_child_task() {
        let session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let coordinator = Arc::new(FakeAgentCoordinator::default());
        let handler = AgentToolHandler::new(
            send_message_spec(),
            AgentToolKind::SendMessage,
            test_background_tasks(),
        );

        let result = handler
            .handle(
                tool_context(
                    session_id,
                    Some(coordinator.clone() as Arc<dyn devo_tools::AgentToolCoordinator>),
                ),
                serde_json::json!({
                    "target": child_session_id.to_string(),
                    "message": "inspect the follow-up failure"
                }),
                None,
            )
            .await
            .expect("send_message succeeds");

        assert_eq!(result.result_summary, "message delivered");
        assert_eq!(
            coordinator.messages.lock().await.as_slice(),
            &[AgentMessageParams {
                session_id,
                target: child_session_id.to_string(),
                message: "inspect the follow-up failure".to_string(),
            }]
        );
    }

    #[test]
    fn registry_exposes_generic_task_tools_and_hides_legacy_agent_adapters() {
        let mut builder = ToolRegistryBuilder::new();
        register_agent_tools(&mut builder, test_background_tasks());
        let names = builder
            .tool_definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "spawn_agent".to_string(),
                "send_message".to_string(),
                "await_task".to_string(),
                "list_tasks".to_string(),
                "cancel_task".to_string(),
            ]
        );
    }

    #[test]
    fn agent_tool_schemas_explain_subagent_workflow() {
        let spawn = spawn_spec();
        let schema = spawn.input_schema.to_json_value();
        let fork_description = schema["properties"]["fork_turns"]["description"]
            .as_str()
            .expect("fork_turns description");

        assert!(spawn.description.contains("parent-only"));
        assert!(spawn.description.contains("await_task"));
        assert!(
            spawn
                .description
                .contains("model-visible through await_task")
        );
        assert!(fork_description.contains("\"all\" (default)"));
        assert!(fork_description.contains("stable completed parent history"));
        assert!(fork_description.contains("excludes the active parent turn"));
        assert!(fork_description.contains("\"none\""));

        let send = send_message_spec();
        assert!(!send.description.contains("Parent-only"));
        assert!(
            send.description
                .contains("queue the message for the next turn")
        );
        assert!(send.description.contains("reactivates the same task id"));
        assert!(send.description.contains("await_task again"));

        let await_task = await_task_spec();
        assert!(await_task.description.contains("absolute deadline"));
        assert!(await_task.description.contains("does not wake the model"));
        let await_schema = await_task.input_schema.to_json_value();
        assert!(await_schema["properties"].get("task_id").is_some());
        assert!(await_schema["properties"].get("timeout_secs").is_some());

        let list_tasks = list_tasks_spec();
        assert!(list_tasks.description.contains("parent session id"));
        assert!(list_tasks.description.contains("waiting_approval"));

        let cancel_task = cancel_task_spec();
        assert!(cancel_task.description.contains("close its agent session"));

        let wait = wait_agent_spec();
        assert!(!wait.description.contains("Parent-only"));
        assert!(wait.description.contains("Decision tree"));
        assert!(wait.description.contains(r#"wait_agent({"target""#));
        assert!(wait.description.contains("assistant_message"));
        assert!(wait.description.contains("not token deltas"));
        assert!(wait.description.contains("list_agents"));
        assert!(wait.description.contains("close_agent"));
        let wait_schema = wait.input_schema.to_json_value();
        let after_sequence_description = wait_schema["properties"]["after_sequence"]["description"]
            .as_str()
            .expect("after_sequence description");
        assert!(after_sequence_description.contains("runtime cursor"));
        assert!(wait_schema["properties"].get("timeout_secs").is_some());

        let list = list_agents_spec();
        assert!(!list.description.contains("Parent-only"));
        assert!(list.description.contains("timed_out with no events"));
        assert!(list.description.contains("Does not return assistant text"));
        assert!(list.description.contains("agent_nickname"));

        let close = close_agent_spec();
        assert!(!close.description.contains("Parent-only"));
        assert!(close.description.contains("off-track"));
        assert!(close.description.contains("terminal status event"));
    }

    #[tokio::test]
    async fn agent_handler_requires_configured_coordinator() {
        let handler =
            AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn, test_background_tasks());
        let error = handler
            .handle(
                tool_context(SessionId::new(), None),
                serde_json::json!({
                    "message": "review this"
                }),
                None,
            )
            .await
            .expect_err("missing coordinator should fail");

        assert!(matches!(
            error,
            ToolCallError::NeedsConfiguration(message)
                if message == "child agent coordination is not configured"
        ));
    }

    #[tokio::test]
    async fn wait_handler_stops_when_context_is_cancelled() {
        let session_id = SessionId::new();
        let coordinator = Arc::new(BlockingWaitCoordinator);
        let handler = AgentToolHandler::new(
            wait_agent_spec(),
            AgentToolKind::Wait,
            test_background_tasks(),
        );
        let cancel_token = CancellationToken::new();
        let ctx = tool_context_with_cancel_token(
            session_id,
            Some(coordinator as Arc<dyn devo_tools::AgentToolCoordinator>),
            cancel_token.clone(),
        );
        cancel_token.cancel();

        let error = handler
            .handle(ctx, serde_json::json!({}), None)
            .await
            .expect_err("cancelled wait should fail");

        assert!(matches!(error, ToolCallError::Cancelled));
    }

    fn tool_context(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
    ) -> ToolContext {
        tool_context_with_cancel_token(session_id, agent_coordinator, CancellationToken::new())
    }

    fn test_background_tasks() -> Arc<BackgroundTaskStore> {
        let process_store = Arc::new(crate::unified_exec::store::ProcessStore::new());
        Arc::new(BackgroundTaskStore::new(process_store))
    }

    fn tool_context_with_cancel_token(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
        cancel_token: CancellationToken,
    ) -> ToolContext {
        ToolContext {
            tool_call_id: crate::invocation::ToolCallId("tool-call".to_string()),
            session_id: session_id.to_string(),
            turn_id: None,
            workspace_root: ".".into(),
            budgets: ToolBudgets {
                output_limit_bytes: 1024,
                wall_time_limit_ms: None,
            },
            cancel_token,
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_profile: None,
        }
    }
}
