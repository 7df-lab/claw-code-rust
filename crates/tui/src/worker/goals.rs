//! Goal lifecycle helpers for session leave and restore.

use anyhow::Context;
use anyhow::Result;
use devo_core::SessionId;
use devo_core::TurnId;
use devo_protocol::ThreadGoalStatus;
use devo_server::StdioServerClient;
use tokio::sync::mpsc;

use crate::events::WorkerEvent;

use super::session_restore;

pub(crate) fn thread_goal_from_native(
    goal: &devo_protocol::native::goal::Goal,
) -> devo_protocol::ThreadGoal {
    let status = match goal.status {
        devo_protocol::native::goal::GoalStatus::Active => ThreadGoalStatus::Active,
        devo_protocol::native::goal::GoalStatus::Paused
        | devo_protocol::native::goal::GoalStatus::Blocked
        | devo_protocol::native::goal::GoalStatus::UsageLimited => ThreadGoalStatus::Paused,
        devo_protocol::native::goal::GoalStatus::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        devo_protocol::native::goal::GoalStatus::Completed
        | devo_protocol::native::goal::GoalStatus::Failed
        | devo_protocol::native::goal::GoalStatus::Canceled => ThreadGoalStatus::Complete,
    };
    let Ok(thread_id) = SessionId::try_from(goal.session_id.as_str()) else {
        unreachable!("canonical goal carries a legacy session id");
    };
    devo_protocol::ThreadGoal {
        thread_id,
        objective: goal.objective.clone(),
        status,
        token_budget: goal
            .token_budget
            .and_then(|budget| i64::try_from(budget).ok()),
        tokens_used: i64::try_from(goal.tokens_used).unwrap_or(i64::MAX),
        time_used_seconds: i64::try_from(goal.time_used_seconds).unwrap_or(i64::MAX),
        created_at: goal.created_at.timestamp(),
        updated_at: goal.updated_at.timestamp(),
    }
}

pub(crate) async fn pause_active_goal_before_session_leave(
    client: &mut StdioServerClient,
    session_id: SessionId,
    active_turn_id: Option<TurnId>,
) -> Result<()> {
    let goal_status = client
        .session_goal_read_native(session_id)
        .await
        .context("failed to load goal before leaving session")?;
    let goal = goal_status.goal.as_ref().map(thread_goal_from_native);
    if !should_pause_goal_before_session_leave(goal.as_ref()) {
        return Ok(());
    }

    let goal_id = goal_status
        .goal
        .as_ref()
        .map(|goal| goal.id.clone())
        .context("goal disappeared before pause")?;
    client
        .session_goal_transition_native(
            session_id,
            &goal_id,
            devo_client::GoalLifecycleTransition::Pause,
        )
        .await
        .context("failed to pause active goal before leaving session")?;

    if active_turn_id.is_some()
        && let Err(error) = client
            .session_interrupt_native(
                devo_protocol::native::rpc_session::SessionInterruptScope::Session {
                    session_id: session_restore::native_session_id(session_id),
                },
            )
            .await
    {
        return Err(error).context("failed to interrupt active goal work before leaving session");
    }

    Ok(())
}

pub(crate) fn should_pause_goal_before_session_leave(
    goal: Option<&devo_protocol::ThreadGoal>,
) -> bool {
    goal.is_some_and(|goal| {
        matches!(
            goal.status,
            ThreadGoalStatus::Active | ThreadGoalStatus::BudgetLimited
        )
    })
}

pub(crate) fn emit_goal_leave_failure(
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    error: anyhow::Error,
) {
    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
        message: error.to_string(),
    });
}
