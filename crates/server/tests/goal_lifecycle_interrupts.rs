use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_core::InternalRecordV2;
use devo_core::ParsedRolloutLine;
use devo_core::RolloutLineV2;
use devo_core::parse_rollout_line;
use devo_protocol::Usage;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

use support::BudgetWrapupPendingProvider;
use support::PendingProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::initialize_connection;
use support::start_session;
use support::wait_for_notification;
use support::wait_for_request_count;

#[tokio::test]
async fn goal_clear_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "clear should stop hidden turn",
        /*replace_existing*/ false,
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
                "method": "_devo/goal/clear",
                "params": {
                    "sessionId": session_id
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

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "complete should stop hidden turn",
        /*replace_existing*/ false,
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
                "method": "_devo/goal/complete",
                "params": {
                    "sessionId": session_id,
                    "status": "complete"
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

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "cancel should stop hidden turn",
        /*replace_existing*/ false,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;
    let goal_id = persisted_goal_id(data_root.path(), session_id)?;

    let cancel_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 126,
                "method": "goal/cancel",
                "params": {
                    "sessionId": session_id,
                    "goalId": goal_id
                }
            }),
        )
        .await
        .context("goal/cancel response")?;
    let _response: devo_server::SuccessResponse<devo_protocol::GoalSetStatusResult> =
        serde_json::from_value(cancel_response)?;

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
        /*replace_existing*/ false,
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
        /*replace_existing*/ true,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &first_turn_id);
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;

    let status_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 122,
                "method": "_devo/goal/status",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("goal/status response")?;
    let response: devo_server::SuccessResponse<devo_protocol::GoalStatusResult> =
        serde_json::from_value(status_response)?;
    assert_eq!(
        response
            .result
            .goal
            .map(|goal| { (goal.objective, goal.status, goal.tokens_used,) }),
        Some((
            "new replacement goal".to_string(),
            devo_protocol::ThreadGoalStatus::Active,
            0,
        ))
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

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 123,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "preserve budget-limited status",
                    "status": "active",
                    "tokenBudget": 80
                }
            }),
        )
        .await
        .context("goal/set response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("budget wrap-up turn id")?;
    wait_for_notification(&mut notifications_rx, "item/agentMessage/delta").await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;

    let pause_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 124,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;
    let response: devo_server::SuccessResponse<devo_protocol::GoalSetResult> =
        serde_json::from_value(pause_response)?;
    assert_eq!(
        response.result.goal.status,
        devo_protocol::ThreadGoalStatus::BudgetLimited
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
    replace_existing: bool,
) -> Result<()> {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 125,
                "method": "_devo/goal/create",
                "params": {
                    "sessionId": session_id,
                    "objective": objective,
                    "replaceExisting": replace_existing
                }
            }),
        )
        .await
        .context("goal/create response")?;
    Ok(())
}

fn notification_turn_id(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("turn_id"))
        .cloned()
}

fn assert_turn_interrupted(notifications: &[serde_json::Value], turn_id: &serde_json::Value) {
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/interrupted"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("turn_id"))
                    == Some(turn_id)
        }),
        "expected turn/interrupted for {turn_id}"
    );
}

fn persisted_goal_id(
    data_root: &std::path::Path,
    session_id: devo_protocol::SessionId,
) -> Result<String> {
    let mut directories = vec![data_root.join("sessions")];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)
            .with_context(|| format!("read rollout directory {}", directory.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&format!("-{session_id}.jsonl")))
            {
                continue;
            }
            let contents = std::fs::read_to_string(&path).context("read session rollout")?;
            for line in contents.lines().rev() {
                let ParsedRolloutLine::V2(line) =
                    parse_rollout_line(line).context("parse rollout line")?
                else {
                    continue;
                };
                let RolloutLineV2::Internal {
                    entry:
                        InternalRecordV2::GoalState {
                            goal: Some(goal), ..
                        },
                    ..
                } = *line
                else {
                    continue;
                };
                let goal: devo_server::goal::Goal =
                    serde_json::from_value(goal).context("parse goal snapshot")?;
                return Ok(goal.goal_id.to_string());
            }
        }
    }
    anyhow::bail!("goal id not found in session rollout")
}
