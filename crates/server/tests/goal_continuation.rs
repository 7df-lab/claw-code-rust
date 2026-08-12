use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::Usage;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;

#[path = "support/goal_continuation.rs"]
mod support;

use support::BudgetWrapupPendingProvider;
use support::CapturingProvider;
use support::FailingProvider;
use support::PendingProvider;
use support::QueuedPriorityProvider;
use support::UsageProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::create_goal;
use support::initialize_connection;
use support::is_user_message_item;
use support::pause_goal_and_interrupt_session;
use support::read_goal;
use support::request_contains_text;
use support::request_last_message_contains_text;
use support::start_session;
use support::wait_for_captured_request_count;
use support::wait_for_notification;
use support::wait_for_request_count;

use devo_protocol::native::rpc_session::GoalIfExists;

#[tokio::test]
async fn goal_token_budget_reached_after_turn_enters_budget_limited() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(UsageProvider {
        requests: std::sync::atomic::AtomicUsize::new(0),
        captured_requests: Mutex::new(Vec::new()),
        usage: Usage {
            input_tokens: 120,
            output_tokens: 30,
            cache_creation_input_tokens: Some(40),
            cache_read_input_tokens: Some(70),
            reasoning_output_tokens: None,
            total_tokens: None,
        },
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "use budget",
        Some(80),
        GoalIfExists::Reject,
        "goal-budget",
    )
    .await?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    let goal = read_goal(&runtime, connection_id, session_id)
        .await?
        .context("goal")?;
    assert_eq!(
        goal.status,
        devo_protocol::native::goal::GoalStatus::BudgetLimited
    );
    assert_eq!(goal.tokens_used, 160);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    let requests = provider.captured_requests.lock().expect("lock requests");
    assert!(
        request_contains_text(&requests[1], "has reached its token budget")
            && request_contains_text(&requests[1], "do not start new substantive work"),
        "budget-limited goal should receive a wrap-up prompt"
    );
    Ok(())
}

#[tokio::test]
async fn budget_limited_goal_pause_interrupts_pending_wrapup_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(BudgetWrapupPendingProvider {
        requests: std::sync::atomic::AtomicUsize::new(0),
        captured_requests: Mutex::new(Vec::new()),
        usage: Usage {
            input_tokens: 120,
            output_tokens: 30,
            cache_creation_input_tokens: Some(40),
            cache_read_input_tokens: Some(70),
            reasoning_output_tokens: None,
            total_tokens: None,
        },
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "pause during budget wrap-up",
        Some(80),
        GoalIfExists::Reject,
        "goal-budget-wrapup",
    )
    .await?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = turn_started
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("id"))
        .cloned()
        .context("turn id in budget wrap-up turn/started")?;
    wait_for_notification(&mut notifications_rx, "item/assistantMessage/delta").await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;

    let goal = read_goal(&runtime, connection_id, session_id)
        .await?
        .context("goal to pause")?;
    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/completed"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("id"))
                    == Some(&turn_id)
                && value["params"]["turn"]["status"] == serde_json::json!("interrupted")
        }),
        "pausing a budget-limited goal should interrupt the pending wrap-up turn"
    );
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("item/completed"))
                && value
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("item"))
                    .and_then(|item| item.get("type"))
                    == Some(&serde_json::json!("assistantMessage"))
                && value
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("item"))
                    .and_then(|item| item.get("text"))
                    == Some(&serde_json::json!("Budget wrap-up started."))
        }),
        "interrupting the wrap-up turn should complete deferred assistant text"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn persisted_paused_goal_replays_without_continuation() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "persist paused goal",
        None,
        GoalIfExists::Reject,
        "goal-persist-paused",
    )
    .await?;
    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 0);

    let replay_provider = Arc::new(PendingProvider::default());
    let replayed_runtime = build_runtime(data_root.path(), replay_provider.clone())?;
    replayed_runtime.load_persisted_sessions().await?;
    let (replayed_connection_id, _replayed_notifications_rx) =
        initialize_connection(&replayed_runtime).await?;

    assert_eq!(
        read_goal(&replayed_runtime, replayed_connection_id, session_id)
            .await?
            .map(|goal| goal.status),
        Some(devo_protocol::native::goal::GoalStatus::Paused)
    );
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(replay_provider.requests.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn persisted_active_goal_pauses_on_restart_without_continuation() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 13,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "plan before setting goal" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "plan"
                }
            }),
        )
        .await
        .context("plan turn/start response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "persist active goal without restart loop",
        None,
        GoalIfExists::Reject,
        "goal-persist-active",
    )
    .await?;
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);

    let replay_provider = Arc::new(PendingProvider::default());
    let replayed_runtime = build_runtime(data_root.path(), replay_provider.clone())?;
    replayed_runtime.load_persisted_sessions().await?;
    let (replayed_connection_id, _replayed_notifications_rx) =
        initialize_connection(&replayed_runtime).await?;

    assert_eq!(
        read_goal(&replayed_runtime, replayed_connection_id, session_id)
            .await?
            .map(|goal| goal.status),
        Some(devo_protocol::native::goal::GoalStatus::Paused)
    );
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(replay_provider.requests.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn goal_pause_interrupts_active_hidden_continuation_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "pause an active continuation",
        None,
        GoalIfExists::Reject,
        "goal-pause-active",
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = turn_started
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("id"))
        .cloned()
        .context("turn id in turn/started")?;
    wait_for_request_count(&provider.requests, 1).await?;

    let goal = read_goal(&runtime, connection_id, session_id)
        .await?
        .context("goal to pause")?;
    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/completed"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("id"))
                    == Some(&turn_id)
                && value["params"]["turn"]["status"] == serde_json::json!("interrupted")
        }),
        "pausing an active goal should interrupt the hidden continuation turn"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn provider_400_tool_call_adjacency_failure_pauses_goal_without_looping() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(FailingProvider {
        requests: std::sync::atomic::AtomicUsize::new(0),
        message: "Invalid status code: 400 Bad Request;".to_string(),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "do not loop after bad request",
        None,
        GoalIfExists::Reject,
        "goal-provider-failure",
    )
    .await?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;

    assert_eq!(
        read_goal(&runtime, connection_id, session_id)
            .await?
            .map(|goal| goal.status),
        Some(devo_protocol::native::goal::GoalStatus::Paused)
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_set_starts_hidden_continuation_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 19,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "previous visible prompt" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("prior turn/start response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "write a benchmark note",
        None,
        GoalIfExists::Reject,
        "goal-hidden-continuation",
    )
    .await?;
    assert_eq!(goal.objective, "write a benchmark note");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        loop {
            if provider.requests.lock().expect("lock requests").len() >= 2 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .context("timed out waiting for captured provider request")??;

    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications
            .iter()
            .any(|value| value.get("method") == Some(&serde_json::json!("turn/started"))),
        "goal continuation should start a turn"
    );
    assert!(
        !notifications.iter().any(is_user_message_item),
        "goal continuation must not emit a synthetic user message item"
    );

    let requests = provider.requests.lock().expect("lock requests");
    assert!(requests.len() >= 2);
    assert!(
        request_contains_text(&requests[1], "Completion audit:")
            && request_contains_text(&requests[1], "write a benchmark note"),
        "goal continuation request should include hidden goal context"
    );
    assert!(
        request_last_message_contains_text(&requests[1], "Completion audit:"),
        "autonomous goal context should be the latest request message"
    );

    Ok(())
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_turn_is_active() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 30,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "keep this turn active" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "continue after this turn",
        None,
        GoalIfExists::Reject,
        "goal-while-turn-active",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 33,
                "method": "session/interrupt",
                "params": {
                    "scope": {
                        "scope": "session",
                        "sessionId": session_id
                    }
                }
            }),
        )
        .await
        .context("session/interrupt response")?;

    Ok(())
}

#[tokio::test]
async fn goal_create_starts_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "created goal should run",
        None,
        GoalIfExists::Reject,
        "goal-create",
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    let turn_id: devo_protocol::TurnId =
        serde_json::from_value(turn_started["params"]["turn"]["id"].clone())?;
    pause_goal_and_interrupt_session(&runtime, connection_id, session_id, turn_id).await?;
    Ok(())
}

#[tokio::test]
async fn goal_resume_starts_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "paused goal should resume",
        None,
        GoalIfExists::Reject,
        "goal-resume",
    )
    .await?;
    wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;
    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    let _ = support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/resume",
        &goal.id,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;

    let turn_id: devo_protocol::TurnId =
        serde_json::from_value(turn_started["params"]["turn"]["id"].clone())?;
    pause_goal_and_interrupt_session(&runtime, connection_id, session_id, turn_id).await?;
    Ok(())
}

#[tokio::test]
async fn queued_user_turn_runs_before_goal_continuation() -> Result<()> {
    let data_root = TempDir::new()?;
    let release_first = Arc::new(Notify::new());
    let provider = Arc::new(QueuedPriorityProvider {
        requests: Mutex::new(Vec::new()),
        release_first: Arc::clone(&release_first),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let active_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 40,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "hold the first turn" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("active turn/start response")?;
    let active_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(active_response)?;
    let active_turn_id = active_result
        .result
        .turn_id()
        .expect("active turn/start should start a turn");
    wait_for_captured_request_count(&provider.requests, /*expected*/ 1).await?;
    wait_for_notification(&mut notifications_rx, "turn/started").await?;

    let queued_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 41,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "queued user input wins" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("queued turn/start response")?;
    let queued_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(queued_response)?;
    let devo_server::TurnStartResult::Queued {
        active_turn_id: queued_active_turn_id,
        queued_input_id,
        status,
        ..
    } = queued_result.result
    else {
        panic!("expected queued turn/start result");
    };
    assert_eq!(queued_active_turn_id, active_turn_id);
    assert_ne!(queued_input_id.to_string(), active_turn_id.to_string());
    assert_eq!(status, devo_core::TurnStatus::Pending);

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "do not skip queued input",
        None,
        GoalIfExists::Reject,
        "goal-queued-input",
    )
    .await?;

    release_first.notify_one();
    wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_captured_request_count(&provider.requests, /*expected*/ 2).await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    {
        let requests = provider.requests.lock().expect("lock requests");
        assert_eq!(requests.len(), 2);
        assert!(
            request_contains_text(&requests[1], "queued user input wins"),
            "queued user turn should be the next provider request"
        );
    }

    support::transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 44,
                "method": "session/interrupt",
                "params": {
                    "scope": {
                        "scope": "session",
                        "sessionId": session_id
                    }
                }
            }),
        )
        .await;

    Ok(())
}
