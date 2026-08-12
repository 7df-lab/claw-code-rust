use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::Usage;
use devo_protocol::native::rpc_session::GoalIfExists;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

use support::BudgetWrapupPendingProvider;
use support::PendingProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::create_goal;
use support::initialize_connection;
use support::read_goal;
use support::start_session;
use support::transition_goal;
use support::wait_for_notification;
use support::wait_for_request_count;

#[tokio::test]
async fn goal_clear_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let goal = start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "clear should stop hidden turn",
        GoalIfExists::Reject,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 120,
                "method": "session/goal/clear",
                "params": {
                    "sessionId": session_id,
                    "expectedGoalId": goal.id
                }
            }),
        )
        .await
        .context("goal/clear response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_complete_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let goal = start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "complete should stop hidden turn",
        GoalIfExists::Reject,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 121,
                "method": "session/goal/complete",
                "params": {
                    "sessionId": session_id,
                    "expectedGoalId": goal.id
                }
            }),
        )
        .await
        .context("goal/complete response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_cancel_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let goal = start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "cancel should stop hidden turn",
        GoalIfExists::Reject,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;
    let cancel_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 126,
                "method": "session/goal/cancel",
                "params": {
                    "sessionId": session_id,
                    "expectedGoalId": goal.id
                }
            }),
        )
        .await
        .context("session/goal/cancel response")?;
    let _response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionGoalTransitionResult,
    > = serde_json::from_value(cancel_response)?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn replacing_goal_interrupts_old_hidden_turn_and_starts_new_goal_cleanly() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "old hidden goal",
        GoalIfExists::Reject,
    )
    .await?;
    let first_turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let first_turn_id =
        notification_turn_id(&first_turn_started).context("first hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "new replacement goal",
        GoalIfExists::Replace,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &first_turn_id);
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;

    let goal = read_goal(&runtime, connection_id, session_id)
        .await?
        .context("goal")?;
    assert_eq!(
        (goal.objective, goal.status, goal.tokens_used),
        (
            "new replacement goal".to_string(),
            devo_protocol::native::goal::GoalStatus::Active,
            0,
        )
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn pausing_budget_limited_wrapup_preserves_budget_limited_status() -> Result<()> {
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

    let goal = create_goal(
        &runtime,
        connection_id,
        session_id,
        "preserve budget-limited status",
        Some(80),
        GoalIfExists::Reject,
        "goal-budget-lifecycle",
    )
    .await?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("budget wrap-up turn id")?;
    wait_for_notification(&mut notifications_rx, "item/assistantMessage/delta").await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;

    let response = transition_goal(
        &runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    assert_eq!(
        response.status,
        devo_protocol::native::goal::GoalStatus::BudgetLimited
    );

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    Ok(())
}

async fn start_created_goal(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    objective: &str,
    if_exists: GoalIfExists,
) -> Result<devo_protocol::native::goal::Goal> {
    create_goal(
        runtime,
        connection_id,
        session_id,
        objective,
        None,
        if_exists,
        &format!("goal-lifecycle-{objective}"),
    )
    .await
}

fn notification_turn_id(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("id"))
        .cloned()
}

fn assert_turn_interrupted(notifications: &[serde_json::Value], turn_id: &serde_json::Value) {
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/completed"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("id"))
                    == Some(turn_id)
                && value["params"]["turn"]["status"] == serde_json::json!("interrupted")
        }),
        "expected interrupted turn/completed for {turn_id}"
    );
}
