use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::AgentInfo;
use devo_protocol::AgentOutputEventKind;
use devo_protocol::AgentTaskMetadata;
use devo_protocol::AwaitTaskParams;
use devo_protocol::AwaitTaskResult;
use devo_protocol::CancelTaskParams;
use devo_protocol::ErrorResponse;
use devo_protocol::ListTasksParams;
use devo_protocol::ModelRequest;
use devo_protocol::ParentAgentOutputEvent;
use devo_protocol::ProtocolErrorCode;
use devo_protocol::TaskInfo;
use devo_protocol::TaskKind;
use devo_protocol::TaskState;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

use support::ScriptedProvider;
use support::StreamScript;
use support::build_runtime;
use support::initialize_connection;
use support::message_texts;
use support::request_agent_close;
use support::request_agent_list;
use support::request_agent_send_message;
use support::request_agent_wait;
use support::request_agent_wait_with;
use support::spawn_child;
use support::spawn_child_with;
use support::start_parent_session;
use support::start_turn;
use support::start_turn_with_approval_policy;
use support::wait_for_child_turn_started;
use support::wait_for_parent_turn_completed;
use support::wait_for_session_notification;
use support::wait_for_stream_calls;

const ADJECTIVES: &[&str] = &[
    "brave", "clever", "silent", "happy", "gentle", "swift", "bright", "lazy", "wild", "calm",
    "fuzzy", "tiny", "bold", "lucky", "mighty",
];
const NOUNS: &[&str] = &[
    "apple", "banana", "orange", "peach", "mango", "tiger", "panda", "fox", "rabbit", "eagle",
    "koala", "lion", "whale", "otter", "wolf",
];

#[tokio::test]
async fn spawn_agent_generates_unique_child_name() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        StreamScript::Pending,
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let first = timeout(
        Duration::from_millis(250),
        spawn_child(&runtime, connection_id, parent_session_id),
    )
    .await
    .context("spawn_agent should return without waiting for child completion")??;
    let second = spawn_child_with(
        &runtime,
        connection_id,
        parent_session_id,
        "review another area",
        Some("none"),
    )
    .await?;

    wait_for_child_turn_started(&mut notifications_rx, first.child_session_id).await?;
    wait_for_child_turn_started(&mut notifications_rx, second.child_session_id).await?;

    assert_generated_name(&first.agent_nickname);
    assert_generated_name(&second.agent_nickname);
    assert_ne!(first.agent_nickname, second.agent_nickname);
    assert_eq!(first.agent_path, format!("root/{}", first.agent_nickname));
    assert_eq!(second.agent_path, format!("root/{}", second.agent_nickname));

    let agents = request_agent_list(&runtime, connection_id, parent_session_id).await?;
    assert_eq!(agents.agents.len(), 2);
    assert!(agents.agents.contains(&AgentInfo {
        session_id: first.child_session_id,
        parent_session_id: Some(parent_session_id),
        agent_path: first.agent_path.clone(),
        agent_nickname: first.agent_nickname.clone(),
        agent_role: "default".to_string(),
        status: "running".to_string(),
        last_task_message: Some("review the current changes".to_string()),
    }));
    assert!(agents.agents.contains(&AgentInfo {
        session_id: second.child_session_id,
        parent_session_id: Some(parent_session_id),
        agent_path: second.agent_path.clone(),
        agent_nickname: second.agent_nickname.clone(),
        agent_role: "default".to_string(),
        status: "running".to_string(),
        last_task_message: Some("review another area".to_string()),
    }));

    Ok(())
}

#[tokio::test]
async fn spawn_agent_tool_call_does_not_deadlock_parent_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::spawn_agent_tool_call("verify parent spawn tool call returns", "none"),
        ScriptedProvider::completed("child finished"),
        ScriptedProvider::completed("spawn tool result observed"),
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    start_turn_with_approval_policy(
        &runtime,
        connection_id,
        parent_session_id,
        "spawn a child using the spawn_agent tool",
        Some("never"),
    )
    .await?;

    wait_for_parent_turn_completed(&mut notifications_rx, parent_session_id).await?;
    wait_for_stream_calls(&provider, 3).await?;

    let agents = request_agent_list(&runtime, connection_id, parent_session_id).await?;
    assert_eq!(agents.agents.len(), 1);
    assert_generated_name(&agents.agents[0].agent_nickname);
    assert_eq!(
        agents.agents[0].last_task_message.as_deref(),
        Some("verify parent spawn tool call returns")
    );

    Ok(())
}

#[tokio::test]
async fn dual_spawn_agent_tool_calls_in_one_response_do_not_deadlock() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::dual_spawn_agent_tool_calls(
            "first delegated worker task",
            "second delegated worker task",
            "none",
        ),
        ScriptedProvider::completed("first child finished"),
        ScriptedProvider::completed("second child finished"),
        ScriptedProvider::completed("both children spawned successfully"),
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let drain_notifications =
        tokio::spawn(async move { while notifications_rx.recv().await.is_some() {} });

    start_turn_with_approval_policy(
        &runtime,
        connection_id,
        parent_session_id,
        "spawn two children in one tool batch",
        Some("never"),
    )
    .await?;

    timeout(Duration::from_secs(15), async {
        loop {
            let agents = request_agent_list(&runtime, connection_id, parent_session_id).await?;
            if agents.agents.len() >= 2 {
                return Ok::<_, anyhow::Error>(agents);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("timed out waiting for both child agents to register")??;

    drain_notifications.abort();

    Ok(())
}

#[tokio::test]
async fn wait_agent_reports_child_output_and_terminal_status() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "child finished review",
    )]));
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let spawn_result = spawn_child(&runtime, connection_id, parent_session_id).await?;

    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        spawn_result.child_session_id,
    )
    .await?;
    let wait_result = request_agent_wait(&runtime, connection_id, parent_session_id, 1).await?;

    assert_eq!(wait_result.timed_out, false);
    assert_eq!(wait_result.next_sequence, 3);
    assert_eq!(
        wait_result.events,
        vec![
            ParentAgentOutputEvent {
                sequence: 1,
                agent_path: spawn_result.agent_path.clone(),
                agent_nickname: spawn_result.agent_nickname.clone(),
                kind: AgentOutputEventKind::AssistantMessage,
                text: Some("child finished review".to_string()),
                status: None,
            },
            ParentAgentOutputEvent {
                sequence: 2,
                agent_path: spawn_result.agent_path.clone(),
                agent_nickname: spawn_result.agent_nickname.clone(),
                kind: AgentOutputEventKind::Status,
                text: None,
                status: Some("completed".to_string()),
            },
        ]
    );

    let agents = request_agent_list(&runtime, connection_id, parent_session_id).await?;
    assert_eq!(
        agents
            .agents
            .iter()
            .find(|agent| agent.session_id == spawn_result.child_session_id)
            .map(|agent| agent.status.as_str()),
        Some("completed")
    );

    Ok(())
}

#[tokio::test]
async fn task_tools_include_agent_metadata_and_cancel_closes_agent() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "child task result",
    )]));
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child(&runtime, connection_id, parent_session_id).await?;
    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;

    let awaited = Arc::clone(&runtime)
        .await_task(AwaitTaskParams {
            session_id: parent_session_id,
            task_id: child.task_id.clone(),
            timeout_secs: Some(1),
        })
        .await?;
    let expected_task = TaskInfo {
        task_id: child.task_id.clone(),
        kind: TaskKind::Agent,
        state: TaskState::Completed,
        agent: Some(AgentTaskMetadata {
            session_id: child.child_session_id,
            parent_session_id: Some(parent_session_id),
            agent_path: child.agent_path.clone(),
            agent_nickname: child.agent_nickname.clone(),
            agent_role: "default".to_string(),
            last_task_message: Some("review the current changes".to_string()),
        }),
        command: None,
    };
    assert_eq!(
        awaited,
        AwaitTaskResult::Terminal {
            task: expected_task.clone(),
            output: Some("child task result".to_string()),
        }
    );

    let listed = Arc::clone(&runtime)
        .list_tasks(ListTasksParams {
            session_id: parent_session_id,
            path_prefix: None,
        })
        .await?;
    assert_eq!(listed.tasks, vec![expected_task]);

    let canceled = Arc::clone(&runtime)
        .cancel_task(CancelTaskParams {
            session_id: parent_session_id,
            task_id: child.task_id,
        })
        .await?;
    assert_eq!(canceled.task.state, TaskState::Canceled);
    assert_eq!(
        canceled.task.agent.as_ref().map(|agent| agent.session_id),
        Some(child.child_session_id)
    );

    Ok(())
}

#[tokio::test]
async fn wait_agent_returns_accumulated_output_with_terminal_status() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed_with_deltas(&["alpha ", "beta"]),
    ]));
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let spawn_result = spawn_child(&runtime, connection_id, parent_session_id).await?;

    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        spawn_result.child_session_id,
    )
    .await?;
    let first_poll = request_agent_wait_with(
        &runtime,
        connection_id,
        parent_session_id,
        Some(spawn_result.child_session_id),
        Some(0),
        1,
    )
    .await?;
    assert_eq!(
        first_poll
            .events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        vec!["alpha beta"]
    );
    assert_eq!(
        first_poll.events[0].kind,
        AgentOutputEventKind::AssistantMessage
    );

    let second_poll = request_agent_wait_with(
        &runtime,
        connection_id,
        parent_session_id,
        Some(spawn_result.child_session_id),
        Some(first_poll.events[0].sequence),
        1,
    )
    .await?;
    assert_eq!(
        second_poll
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(second_poll.events[0].status.as_deref(), Some("completed"));

    Ok(())
}

#[tokio::test]
async fn wait_agent_preserves_full_child_report_for_parent_model() -> Result<()> {
    let data_root = TempDir::new()?;
    let full_report = format!("{}END_OF_LONG_SURVEY_REPORT", "survey finding ".repeat(900));
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed(&full_report),
        ScriptedProvider::wait_agent_tool_call(120),
        ScriptedProvider::completed("parent consumed child report"),
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let spawn_result = spawn_child(&runtime, connection_id, parent_session_id).await?;

    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        spawn_result.child_session_id,
    )
    .await?;
    start_turn_with_approval_policy(
        &runtime,
        connection_id,
        parent_session_id,
        "collect the subagent report",
        Some("never"),
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications_rx, parent_session_id).await?;

    let requests = provider.requests();
    let parent_followup_request = requests
        .last()
        .context("expected parent follow-up model request after wait_agent result")?;
    let tool_result_content = parent_followup_request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|content| match content {
            devo_protocol::RequestContent::ToolResult { content, .. } => Some(content.as_str()),
            devo_protocol::RequestContent::Text { .. }
            | devo_protocol::RequestContent::Reasoning { .. }
            | devo_protocol::RequestContent::ProviderReasoning { .. }
            | devo_protocol::RequestContent::ToolUse { .. }
            | devo_protocol::RequestContent::HostedToolUse { .. } => None,
        })
        .context("expected wait_agent tool result in parent follow-up request")?;

    assert!(tool_result_content.contains("END_OF_LONG_SURVEY_REPORT"));
    assert!(!tool_result_content.contains("...[truncated]"));

    Ok(())
}

#[tokio::test]
async fn send_message_to_idle_child_starts_user_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("initial child result"),
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child(&runtime, connection_id, parent_session_id).await?;

    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;
    let delivered = request_agent_send_message(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
        "start follow-up",
    )
    .await?;
    assert_eq!(delivered.task_id, child.task_id);
    assert!(delivered.delivered);
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;
    wait_for_stream_calls(&provider, 2).await?;

    let requests = provider.requests();
    let followup_texts = message_texts(&requests[1]);
    assert!(
        followup_texts
            .iter()
            .any(|text| text.contains("start follow-up")),
        "follow-up child request should include parent message: {followup_texts:?}"
    );

    request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn send_message_to_active_child_drains_after_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed_after(Duration::from_millis(200), "initial child result"),
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child(&runtime, connection_id, parent_session_id).await?;
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;
    wait_for_stream_calls(&provider, 1).await?;

    let delivered = request_agent_send_message(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
        "queue while busy",
    )
    .await?;
    assert_eq!(delivered.task_id, child.task_id);
    assert!(delivered.delivered);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(provider.stream_calls(), 1);

    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;
    wait_for_stream_calls(&provider, 2).await?;

    let requests = provider.requests();
    let queued_texts = message_texts(&requests[1]);
    assert!(
        queued_texts
            .iter()
            .any(|text| text.contains("queue while busy")),
        "queued child request should include parent message: {queued_texts:?}"
    );

    request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn child_to_parent_message_is_rejected() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "child done",
    )]));
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child(&runtime, connection_id, parent_session_id).await?;
    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;

    for target in [
        "parent".to_string(),
        "root".to_string(),
        parent_session_id.to_string(),
    ] {
        let error = Arc::clone(&runtime)
            .send_message(devo_protocol::AgentMessageParams {
                session_id: child.child_session_id,
                target,
                message: "child report to parent".to_string(),
            })
            .await
            .expect_err("child send_message should be rejected");
        assert!(error.to_string().contains("agent not found:"));
    }

    Ok(())
}

#[tokio::test]
async fn close_agent_records_closed_output_event_once() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::pending());
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child(&runtime, connection_id, parent_session_id).await?;
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;

    let close_result = request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
    )
    .await?;
    assert_eq!(close_result.status, "closed");
    let wait_result = request_agent_wait(&runtime, connection_id, parent_session_id, 1).await?;
    assert_eq!(
        wait_result
            .events
            .iter()
            .filter(|event| event.status.as_deref() == Some("closed"))
            .count(),
        1
    );

    let second_close = request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
    )
    .await?;
    assert_eq!(second_close.status, "closed");
    let second_wait = request_agent_wait_with(
        &runtime,
        connection_id,
        parent_session_id,
        Some(child.child_session_id),
        Some(wait_result.next_sequence.saturating_sub(1)),
        1,
    )
    .await?;
    assert_eq!(second_wait.events, Vec::new());
    assert_eq!(second_wait.timed_out, true);

    Ok(())
}

#[tokio::test]
async fn invalid_agent_requests_return_invalid_params() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([]));
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, _) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    for (request, expected_code, expected_message) in [
        (
            serde_json::json!({
                "id": 10,
                "method": "task/start",
                "params": {
                    "kind": "agent",
                    "sessionId": parent_session_id,
                    "input": [{"type": "text", "text": "bad fork"}],
                    "forkTurns": "2",
                    "idempotencyKey": "bad-fork"
                }
            }),
            ProtocolErrorCode::InvalidParams,
            "fork_turns must be \"none\" or \"all\"",
        ),
        (
            serde_json::json!({
                "id": 12,
                "method": "agent/message",
                "params": {
                    "itemId": "item_missing",
                    "input": [{"type": "text", "text": "hello"}]
                }
            }),
            ProtocolErrorCode::SessionNotFound,
            "agent item is not addressable",
        ),
        (
            serde_json::json!({
                "id": 13,
                "method": "task/start",
                "params": {
                    "kind": "agent",
                    "sessionId": devo_protocol::SessionId::new(),
                    "input": [{"type": "text", "text": "missing parent"}],
                    "idempotencyKey": "missing-parent"
                }
            }),
            ProtocolErrorCode::InvalidParams,
            "session not found:",
        ),
        (
            serde_json::json!({
                "id": 13,
                "method": "agent/unsupported",
                "params": {
                    "sessionId": parent_session_id,
                    "target": "missing",
                    "message": "hello"
                }
            }),
            ProtocolErrorCode::InvalidParams,
            "unknown method: agent/unsupported",
        ),
    ] {
        let response = runtime
            .handle_incoming(connection_id, request)
            .await
            .context("agent error response")?;
        let error: ErrorResponse = serde_json::from_value(response)?;
        assert_eq!(error.error.code, expected_code);
        assert!(
            error.error.message.contains(expected_message),
            "expected {expected_message:?} in {:?}",
            error.error.message
        );
    }

    Ok(())
}

#[tokio::test]
async fn ephemeral_deny_all_child_agent_has_no_tools_and_one_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("side answer"),
        ScriptedProvider::completed("should not run"),
    ]));
    let runtime = build_runtime(data_root.path(), provider.clone() as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let child = Arc::clone(&runtime)
        .spawn_agent(devo_protocol::SpawnAgentParams {
            session_id: parent_session_id,
            message: "answer this side question".to_string(),
            fork_turns: Some("all".to_string()),
            max_turns: Some(1),
            tool_policy: devo_protocol::AgentToolPolicy::DenyAll,
            ephemeral: true,
        })
        .await?;
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;
    wait_for_stream_calls(&provider, 1).await?;
    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].tools.as_ref().map(Vec::len), Some(0));

    let error = Arc::clone(&runtime)
        .send_message(devo_protocol::AgentMessageParams {
            session_id: parent_session_id,
            target: child.child_session_id.to_string(),
            message: "try a second turn".to_string(),
        })
        .await
        .expect_err("second turn should be rejected");
    assert!(
        error
            .to_string()
            .contains("agent maximum turn count reached")
    );
    assert_eq!(provider.stream_calls(), 1);

    Ok(())
}

#[tokio::test]
async fn fork_all_inherits_stable_parent_context() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("stable assistant answer"),
        ScriptedProvider::completed("child saw inherited context"),
        StreamScript::Pending,
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let _ = start_turn(
        &runtime,
        connection_id,
        parent_session_id,
        "remember stable context",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications_rx, parent_session_id).await?;

    let child = spawn_child_with(
        &runtime,
        connection_id,
        parent_session_id,
        "use inherited context",
        None,
    )
    .await?;
    wait_for_session_notification(
        &mut notifications_rx,
        "turn/completed",
        child.child_session_id,
    )
    .await?;

    let _ = start_turn(
        &runtime,
        connection_id,
        parent_session_id,
        "active parent text should not be inherited yet",
    )
    .await?;
    wait_for_session_notification(&mut notifications_rx, "turn/started", parent_session_id).await?;
    let active_child = spawn_child_with(
        &runtime,
        connection_id,
        parent_session_id,
        "fork while parent active",
        None,
    )
    .await?;
    wait_for_child_turn_started(&mut notifications_rx, active_child.child_session_id).await?;
    wait_for_stream_calls(&provider, 4).await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 4);
    // Parent turn 2 and the active-fork child start concurrently, so provider
    // request order is not stable; locate each turn by its task text.
    let completed_child_request = find_unique_request_with_text(&requests, "use inherited context");
    let active_child_request = find_unique_request_with_text(&requests, "fork while parent active");
    let parent_active_turn_request =
        find_unique_request_with_text(&requests, "active parent text should not be inherited yet");
    assert!(
        parent_active_turn_request
            .tools
            .as_ref()
            .is_some_and(|tools| { tools.iter().any(|tool| tool.name == "spawn_agent") }),
        "parent active turn should expose agent coordination tools"
    );

    let completed_child_texts = message_texts(completed_child_request);
    assert_subagent_request_hides_agent_tools(completed_child_request);
    assert!(
        completed_child_texts
            .iter()
            .any(|text| text.contains("remember stable context")),
        "child request should include stable parent user context: {completed_child_texts:?}"
    );
    assert!(
        completed_child_texts
            .iter()
            .any(|text| text.contains("stable assistant answer")),
        "child request should include stable parent assistant context: {completed_child_texts:?}"
    );
    assert!(
        completed_child_texts
            .iter()
            .any(|text| text.contains("use inherited context")),
        "child request should include child task input: {completed_child_texts:?}"
    );
    assert_text_order(
        &completed_child_texts,
        "stable assistant answer",
        "You are running as a sub-agent",
    );
    assert_subagent_reminder_before_task(&completed_child_texts, "use inherited context");

    let active_child_texts = message_texts(active_child_request);
    assert_subagent_request_hides_agent_tools(active_child_request);
    assert!(
        active_child_texts
            .iter()
            .any(|text| text.contains("remember stable context")),
        "active fork should include prior stable context: {active_child_texts:?}"
    );
    assert!(
        !active_child_texts
            .iter()
            .any(|text| text.contains("active parent text should not be inherited yet")),
        "active fork should exclude the parent's active turn: {active_child_texts:?}"
    );
    assert!(
        active_child_texts
            .iter()
            .any(|text| text.contains("fork while parent active")),
        "active fork should include child task input: {active_child_texts:?}"
    );
    assert_text_order(
        &active_child_texts,
        "remember stable context",
        "You are running as a sub-agent",
    );
    assert_subagent_reminder_before_task(&active_child_texts, "fork while parent active");

    request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        active_child.child_session_id,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn fork_none_omits_parent_context_and_places_reminder_before_task() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("stable assistant answer"),
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let parent_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let _ = start_turn(
        &runtime,
        connection_id,
        parent_session_id,
        "remember stable context",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications_rx, parent_session_id).await?;

    let child = spawn_child_with(
        &runtime,
        connection_id,
        parent_session_id,
        "clean child task",
        Some("none"),
    )
    .await?;
    wait_for_child_turn_started(&mut notifications_rx, child.child_session_id).await?;
    wait_for_stream_calls(&provider, 2).await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    let child_texts = message_texts(&requests[1]);
    assert_subagent_request_hides_agent_tools(&requests[1]);
    assert!(
        !child_texts
            .iter()
            .any(|text| text.contains("remember stable context")),
        "fork none should exclude parent user context: {child_texts:?}"
    );
    assert!(
        !child_texts
            .iter()
            .any(|text| text.contains("stable assistant answer")),
        "fork none should exclude parent assistant context: {child_texts:?}"
    );
    assert_subagent_reminder_before_task(&child_texts, "clean child task");

    request_agent_close(
        &runtime,
        connection_id,
        parent_session_id,
        child.child_session_id,
    )
    .await?;

    Ok(())
}

fn request_contains_text(request: &ModelRequest, needle: &str) -> bool {
    message_texts(request)
        .iter()
        .any(|text| text.contains(needle))
}

fn find_unique_request_with_text<'a>(
    requests: &'a [ModelRequest],
    needle: &str,
) -> &'a ModelRequest {
    let matches: Vec<_> = requests
        .iter()
        .filter(|request| request_contains_text(request, needle))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one request containing {needle:?}, got {} matching request(s)",
        matches.len(),
    );
    matches[0]
}

fn assert_generated_name(name: &str) {
    let Some((adjective, noun)) = name.split_once('-') else {
        panic!("generated name should be adjective-noun: {name}");
    };
    assert!(ADJECTIVES.contains(&adjective));
    assert!(NOUNS.contains(&noun));
}

fn assert_subagent_request_hides_agent_tools(request: &ModelRequest) {
    let tools = request.tools.as_ref().expect("child request tools");
    for name in [
        "spawn_agent",
        "send_message",
        "await_task",
        "list_tasks",
        "cancel_task",
        "wait_agent",
        "list_agents",
        "close_agent",
    ] {
        assert!(
            tools.iter().all(|tool| tool.name != name),
            "child request should not expose {name}: {:?}",
            tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()
        );
        assert!(
            !request.system.as_deref().unwrap_or_default().contains(name),
            "child request system prompt should not mention hidden agent tool {name}"
        );
    }

    let system = request.system.as_deref().unwrap_or_default();
    assert!(
        !system.contains("<system-reminder>"),
        "child request system prompt should remain base-only"
    );
    assert!(
        !system.contains("You are running as a sub-agent"),
        "child request system prompt should not include request-only reminders"
    );
}

fn assert_subagent_reminder_before_task(texts: &[String], task: &str) {
    assert_text_order(texts, "You are running as a sub-agent", task);
    assert_text_order(
        texts,
        "Do not call agent coordination tools such as spawn_agent",
        task,
    );
}

fn assert_text_order(texts: &[String], before: &str, after: &str) {
    let before_index = text_index_containing(texts, before);
    let after_index = text_index_containing(texts, after);
    assert!(
        before_index < after_index,
        "expected {before:?} before {after:?}: {texts:?}"
    );
}

fn text_index_containing(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .position(|text| text.contains(needle))
        .unwrap_or_else(|| panic!("expected text containing {needle:?}: {texts:?}"))
}
