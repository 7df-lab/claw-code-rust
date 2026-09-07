//! Server-owned autonomous continuation for active thread goals.
//!
//! The TUI persists goal state through `goal/set`; this module decides when an
//! active goal is eligible to become an internal Build turn and launches that
//! turn without adding a synthetic user message to the transcript.

use super::*;
use crate::goal::GoalStatus;
use crate::runtime::session_actor::SessionHandle;
use futures::future::BoxFuture;

struct GoalContinuationCandidate {
    goal_id: GoalId,
    goal: Goal,
}

impl ServerRuntime {
    pub(super) fn maybe_start_goal_continuation_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(session_handle) = self.session(session_id).await else {
                return;
            };
            if self
                .pause_goal_continuation_after_failed_turn(session_id, &session_handle)
                .await
            {
                return;
            }
            if !self.session_allows_goal_continuation(session_id).await {
                return;
            }
            let Some(candidate) = self.goal_continuation_candidate(session_id).await else {
                return;
            };
            if !self.session_allows_goal_continuation(session_id).await {
                return;
            }

            let _state_change_guard = session_handle.lock_state_change().await;
            let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
                return;
            };
            let requested_model = session_model_selection(&reservation.summary);
            let requested_reasoning_effort_selection =
                reservation.summary.reasoning_effort_selection.clone();
            let turn_config = reservation
                .runtime_context
                .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let now = Utc::now();
            let turn = TurnMetadata {
                turn_id: TurnId::new(),
                session_id,
                sequence: reservation
                    .latest_turn
                    .as_ref()
                    .map_or(1, |turn| turn.sequence + 1),
                status: TurnStatus::Running,
                kind: devo_core::TurnKind::Regular,
                model: turn_config.model.slug.clone(),
                model_binding_id: turn_config.model_binding_id.clone(),
                reasoning_effort_selection: turn_config.reasoning_effort_selection.clone(),
                reasoning_effort: resolved_request.effective_reasoning_effort,
                request_model,
                request_thinking: resolved_request.request_thinking.clone(),
                started_at: now,
                completed_at: None,
                usage: None,
                stop_reason: None,
                failure_reason: None,
            };
            if !session_handle
                .try_begin_active_turn(turn.clone(), turn_config.clone())
                .await
                .unwrap_or(false)
            {
                return;
            }
            self.active_goal_continuation_turns
                .lock()
                .await
                .insert(session_id, turn.turn_id);
            self.goal_continuation_turn_goals
                .lock()
                .await
                .insert(turn.turn_id, candidate.goal_id.clone());
            if !self
                .mark_goal_continuation_turn_started(session_id, &candidate.goal_id, turn.turn_id)
                .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
                return;
            }

            if let Err(error) = self
                .append_goal_continuation_turn_start(session_id, &turn)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    error = %error,
                    "failed to persist goal continuation turn start"
                );
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
                return;
            }
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn.turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
                return;
            }

            self.broadcast_event(ServerEvent::SessionStatusChanged(
                SessionStatusChangedPayload {
                    session_id,
                    status: SessionRuntimeStatus::ActiveTurn,
                },
            ))
            .await;
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn.turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
                return;
            }
            self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
                session_id,
                turn: turn.clone(),
            }))
            .await;
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn.turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
                return;
            }

            let cancel_token = CancellationToken::new();
            let runtime = Arc::clone(self);
            let task_turn = turn.clone();
            let task_turn_config = turn_config.clone();
            let task_goal = candidate.goal.to_thread_goal();
            let task_started = {
                let tracked_turns = self.active_goal_continuation_turns.lock().await;
                let still_reserved = tracked_turns
                    .get(&session_id)
                    .is_some_and(|tracked_turn_id| *tracked_turn_id == turn.turn_id);
                if !still_reserved {
                    false
                } else {
                    let still_active_turn = session_handle
                        .active_turn_id()
                        .await
                        .is_some_and(|active_turn_id| active_turn_id == Some(turn.turn_id));
                    if !still_active_turn {
                        false
                    } else {
                        self.active_turns
                            .insert_cancel_token(session_id, cancel_token.clone())
                            .await;
                        self.register_runtime_active_turn(session_id, turn.clone())
                            .await;
                        let task = tokio::spawn(async move {
                            runtime
                                .execute_turn(ExecuteTurnRequest {
                                    session_id,
                                    turn: task_turn,
                                    turn_config: task_turn_config,
                                    display_input: String::new(),
                                    input: String::new(),
                                    input_messages: Vec::new(),
                                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                                    input_mode: TurnInputMode::HiddenGoalContinuation {
                                        goal: task_goal,
                                    },
                                })
                                .await;
                        });
                        self.active_turns
                            .set_abort_handle(session_id, task.abort_handle())
                            .await;
                        true
                    }
                }
            };
            if !task_started {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn.turn_id)
                    .await;
            }
        })
    }

    async fn session_allows_goal_continuation(&self, session_id: SessionId) -> bool {
        if self
            .session_interactive
            .has_pending_interactive(session_id)
            .await
        {
            return false;
        }
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return false;
        };
        if reservation.active_turn.is_some() {
            return false;
        }
        if session_handle.collaboration_mode().await == Some(devo_protocol::CollaborationMode::Plan)
        {
            return false;
        }
        let Some(snapshot) = session_handle.pending_queue_snapshot().await else {
            return false;
        };
        snapshot.pending_count == 0
    }

    async fn goal_continuation_candidate(
        &self,
        session_id: SessionId,
    ) -> Option<GoalContinuationCandidate> {
        let stores = self.goal_stores.lock().await;
        let goal = stores.get(&session_id)?.get()?.clone();
        if !goal.check_continuation().should_continue {
            return None;
        }
        Some(GoalContinuationCandidate {
            goal_id: goal.goal_id.clone(),
            goal,
        })
    }

    /// Pause an active goal when the latest turn failed after the goal was last
    /// updated. Exposed so post-turn scheduling can still pause goals even when
    /// recovery availability blocks auto-continuation.
    pub(crate) async fn pause_goal_continuation_after_failed_turn(
        &self,
        session_id: SessionId,
        session_handle: &SessionHandle,
    ) -> bool {
        let (latest_turn, failure_message) = {
            let reservation = session_handle.turn_reservation_snapshot().await;
            let snapshot = session_handle.spawn_snapshot().await;
            match (reservation, snapshot) {
                (Some(reservation), Some(snapshot)) => {
                    let latest_turn = reservation.latest_turn;
                    let failure_message = latest_turn.as_ref().and_then(|turn| {
                        latest_failed_turn_error_message(&snapshot.stable_items, turn)
                    });
                    (latest_turn, failure_message)
                }
                _ => (None, None),
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(goal) = stores.get_mut(&session_id).and_then(GoalStore::get_mut) else {
            return false;
        };
        if goal.status != GoalStatus::Active
            || !failed_turn_should_suppress_goal(goal.updated_at, latest_turn.as_ref())
        {
            return false;
        }

        let previous_status = goal.status;
        goal.status = GoalStatus::Paused;
        goal.blocker_summary = Some(goal_failure_blocker_summary(failure_message.as_deref()));
        goal.updated_at = Utc::now();
        let durable_goal = goal.clone();
        drop(stores);

        if let Err(error) = self
            .goal_durable_store
            .append_status_changed(
                &durable_goal,
                previous_status,
                durable_goal.blocker_summary.clone(),
            )
            .await
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist failed-turn goal pause record");
        }
        self.sync_core_session_goal(session_id, None).await;
        tracing::warn!(
            session_id = %session_id,
            "paused active goal continuation after failed turn"
        );
        true
    }

    async fn append_goal_continuation_turn_start(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &TurnMetadata,
    ) -> anyhow::Result<()> {
        let Some(session_handle) = self.session(session_id).await else {
            return Ok(());
        };
        let Some(persistence) = session_handle.turn_persistence_snapshot().await else {
            return Ok(());
        };
        if persistence.record.is_some() {
            self.persist_turn_line_deduped(session_id, turn).await?;
        }
        let goal = {
            let stores = self.goal_stores.lock().await;
            stores.get(&session_id).and_then(GoalStore::get).cloned()
        };
        if let Some(goal) = goal
            && let Some(prompt) = goal.continuation_prompt()
            && let Err(error) = self
                .goal_durable_store
                .append_context_snapshot(&goal, format!("turn-{}", turn.turn_id), prompt)
                .await
        {
            tracing::warn!(session_id = %session_id, turn_id = %turn.turn_id, error = %error, "failed to persist goal context snapshot");
        }
        Ok(())
    }

    async fn mark_goal_continuation_turn_started(
        &self,
        session_id: SessionId,
        goal_id: &GoalId,
        turn_id: TurnId,
    ) -> bool {
        let mut stores = self.goal_stores.lock().await;
        let goal = stores
            .get_mut(&session_id)
            .and_then(GoalStore::get_mut)
            .filter(|goal| &goal.goal_id == goal_id)
            .filter(|goal| goal.check_continuation().should_continue)
            .map(|goal| {
                let previous_status = goal.status;
                goal.usage.record_turn();
                if goal.token_budget_exhausted() {
                    goal.status = GoalStatus::BudgetLimited;
                    goal.blocker_summary = Some(
                        "Goal token budget reached; launched a budget-limit wrap-up turn."
                            .to_string(),
                    );
                }
                goal.updated_at = Utc::now();
                (goal.clone(), previous_status)
            });
        drop(stores);

        let Some((goal, previous_status)) = goal else {
            return false;
        };
        if let Err(error) = self
            .goal_durable_store
            .append_budget_accounted(
                &goal, turn_id, /*token_delta*/ 0, /*turn_delta*/ 1,
                /*duration_delta_seconds*/ 0,
            )
            .await
        {
            tracing::warn!(session_id = %session_id, turn_id = %turn_id, error = %error, "failed to persist goal turn accounting record");
        }
        if goal.status != previous_status {
            if let Err(error) = self
                .goal_durable_store
                .append_status_changed(&goal, previous_status, goal.blocker_summary.clone())
                .await
            {
                tracing::warn!(session_id = %session_id, turn_id = %turn_id, error = %error, "failed to persist goal budget-limit status record");
            }
            self.sync_core_session_goal(session_id, None).await;
        }
        true
    }

    async fn clear_goal_continuation_turn_reservation(
        &self,
        session_handle: &SessionHandle,
        turn_id: TurnId,
    ) {
        let session_id = {
            let mut tracked_turns = self.active_goal_continuation_turns.lock().await;
            let session_id = tracked_turns
                .iter()
                .find_map(|(session_id, tracked_turn_id)| {
                    (*tracked_turn_id == turn_id).then_some(*session_id)
                });
            if let Some(session_id) = session_id {
                tracked_turns.remove(&session_id);
            }
            session_id
        };
        if let Some(session_id) = session_id {
            self.active_turns.remove_cancel_token(session_id).await;
            self.active_turns.remove_abort_handle(session_id).await;
        }
        self.goal_continuation_turn_goals
            .lock()
            .await
            .remove(&turn_id);
        session_handle.clear_active_turn_if_matches(turn_id).await;
    }

    pub(super) async fn interrupt_active_goal_continuation_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        reason: &str,
    ) -> bool {
        let Some(turn_id) = self
            .active_goal_continuation_turns
            .lock()
            .await
            .remove(&session_id)
        else {
            return false;
        };
        self.goal_continuation_turn_goals
            .lock()
            .await
            .remove(&turn_id);

        // Interrupt via the cancel token and wait for
        // `finalize_executed_turn` / `MergeTurn` to emit lifecycle events.
        if self.runtime_active_turn_id(session_id).await != Some(turn_id) {
            let already_terminal = self.recent_terminal_turn_status(turn_id).await.is_some();
            if already_terminal {
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    reason,
                    "goal continuation turn already terminal"
                );
            }
            return already_terminal;
        }

        let terminal_rx = self.subscribe_terminal_turn_status(turn_id).await;
        if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
            self.record_terminal_turn_status(turn_id, snapshot).await;
            tracing::info!(
                session_id = %session_id,
                turn_id = %turn_id,
                reason,
                "goal continuation turn already terminal"
            );
            return true;
        }

        // Cancel via a clone rather than `remove`: see the comment in
        // `interrupt_child_runtime_work` for why removing here races with
        // `run_turn_model_query` fetching the same token.
        self.signal_active_turn_interrupt(session_id).await;

        let completed =
            match tokio::time::timeout(std::time::Duration::from_secs(5), terminal_rx).await {
                Ok(Ok(_)) => true,
                Ok(Err(_)) | Err(_) => self.recent_terminal_turn_status(turn_id).await.is_some(),
            };
        if completed {
            tracing::info!(
                session_id = %session_id,
                turn_id = %turn_id,
                reason,
                "interrupted active goal continuation turn"
            );
        }
        completed
    }
}

async fn goal_continuation_turn_still_current(
    runtime: &ServerRuntime,
    session_handle: &SessionHandle,
    session_id: SessionId,
    turn_id: TurnId,
    goal_id: &GoalId,
) -> bool {
    let still_reserved = runtime
        .active_goal_continuation_turns
        .lock()
        .await
        .get(&session_id)
        .is_some_and(|tracked_turn_id| *tracked_turn_id == turn_id);
    if !still_reserved {
        return false;
    }
    let still_active_turn = session_handle
        .active_turn_id()
        .await
        .is_some_and(|active_turn_id| active_turn_id == Some(turn_id));
    if !still_active_turn {
        return false;
    }
    let stores = runtime.goal_stores.lock().await;
    stores
        .get(&session_id)
        .and_then(GoalStore::get)
        .is_some_and(|goal| {
            &goal.goal_id == goal_id
                && matches!(goal.status, GoalStatus::Active | GoalStatus::BudgetLimited)
        })
}

fn failed_turn_should_suppress_goal(
    goal_updated_at: chrono::DateTime<Utc>,
    latest_turn: Option<&TurnMetadata>,
) -> bool {
    let Some(turn) = latest_turn else {
        return false;
    };
    if turn.status != TurnStatus::Failed {
        return false;
    }
    turn.completed_at.unwrap_or(turn.started_at) > goal_updated_at
}

fn latest_failed_turn_error_message(
    items: &[crate::execution::PersistedTurnItem],
    latest_turn: &TurnMetadata,
) -> Option<String> {
    if latest_turn.status != TurnStatus::Failed {
        return None;
    }
    items.iter().rev().find_map(|item| {
        match (item.turn_id == latest_turn.turn_id, &item.turn_item) {
            (true, TurnItem::AgentMessage(TextItem { text })) if !text.trim().is_empty() => {
                Some(text.clone())
            }
            _ => None,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoalFailureClass {
    ToolCallAdjacency,
    ProviderParameter,
    Authentication,
    Permission,
    Other,
}

fn classify_goal_failure(message: Option<&str>) -> GoalFailureClass {
    let Some(message) = message else {
        return GoalFailureClass::Other;
    };
    if (contains_ascii_case_insensitive(message, "tool_calls")
        || contains_ascii_case_insensitive(message, "tool calls"))
        && (contains_ascii_case_insensitive(message, "tool_call_id")
            || contains_ascii_case_insensitive(message, "tool messages")
            || contains_ascii_case_insensitive(message, "insufficient tool messages"))
    {
        return GoalFailureClass::ToolCallAdjacency;
    }
    if contains_ascii_case_insensitive(message, "401")
        || contains_ascii_case_insensitive(message, "unauthorized")
        || contains_ascii_case_insensitive(message, "authentication")
        || contains_ascii_case_insensitive(message, "api key")
        || contains_ascii_case_insensitive(message, "token timeout")
    {
        return GoalFailureClass::Authentication;
    }
    if contains_ascii_case_insensitive(message, "403")
        || contains_ascii_case_insensitive(message, "434")
        || contains_ascii_case_insensitive(message, "forbidden")
        || contains_ascii_case_insensitive(message, "permission")
        || contains_ascii_case_insensitive(message, "no api permission")
    {
        return GoalFailureClass::Permission;
    }
    if contains_ascii_case_insensitive(message, "400")
        || contains_ascii_case_insensitive(message, "bad request")
        || contains_ascii_case_insensitive(message, "invalid request")
        || contains_ascii_case_insensitive(message, "invalid_request_error")
        || contains_ascii_case_insensitive(message, "invalid parameter")
        || contains_ascii_case_insensitive(message, "parameter error")
    {
        return GoalFailureClass::ProviderParameter;
    }
    GoalFailureClass::Other
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn goal_failure_blocker_summary(message: Option<&str>) -> String {
    match classify_goal_failure(message) {
        GoalFailureClass::ToolCallAdjacency => {
            "Goal continuation paused after an unrecoverable provider/protocol error: the provider rejected a tool-call transcript because assistant tool_calls were not followed by matching tool messages."
        }
        GoalFailureClass::ProviderParameter => {
            "Goal continuation paused after an unrecoverable provider parameter error, such as a 400 Bad Request or invalid request payload."
        }
        GoalFailureClass::Authentication => {
            "Goal continuation paused after an authentication error. Fix the provider credentials before resuming the goal."
        }
        GoalFailureClass::Permission => {
            "Goal continuation paused after a provider permission error. Fix model or account permissions before resuming the goal."
        }
        GoalFailureClass::Other => {
            "Goal continuation paused because the previous turn failed before the goal could continue."
        }
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(status: TurnStatus, completed_at: chrono::DateTime<Utc>) -> TurnMetadata {
        TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status,
            kind: devo_core::TurnKind::Regular,
            model: "model-a".into(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "provider/model-a".into(),
            request_thinking: None,
            started_at: completed_at - chrono::Duration::seconds(1),
            completed_at: Some(completed_at),
            usage: None,
            stop_reason: None,
            failure_reason: None,
        }
    }

    #[test]
    fn failed_turn_after_goal_update_suppresses_continuation() {
        // Trace: L2-DES-GOAL-001
        let goal_updated_at = Utc::now();
        let latest_turn = turn(
            TurnStatus::Failed,
            goal_updated_at + chrono::Duration::seconds(1),
        );

        assert!(failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn failed_turn_before_goal_update_allows_manual_resume() {
        // Trace: L2-DES-GOAL-001
        let latest_turn_completed_at = Utc::now();
        let goal_updated_at = latest_turn_completed_at + chrono::Duration::seconds(1);
        let latest_turn = turn(TurnStatus::Failed, latest_turn_completed_at);

        assert!(!failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn completed_turn_does_not_suppress_continuation() {
        // Trace: L2-DES-GOAL-001
        let goal_updated_at = Utc::now();
        let latest_turn = turn(
            TurnStatus::Completed,
            goal_updated_at + chrono::Duration::seconds(1),
        );

        assert!(!failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn classifies_tool_call_adjacency_failure() {
        // Trace: L2-DES-GOAL-001
        let message = "Invalid status code: 400 Bad Request; response body: assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'";

        assert_eq!(
            classify_goal_failure(Some(message)),
            GoalFailureClass::ToolCallAdjacency
        );
        assert!(goal_failure_blocker_summary(Some(message)).contains("assistant tool_calls"),);
    }

    #[test]
    fn classifies_provider_parameter_failure() {
        // Trace: L2-DES-GOAL-001
        assert_eq!(
            classify_goal_failure(Some("400 BAD REQUEST invalid_request_error")),
            GoalFailureClass::ProviderParameter
        );
    }

    #[test]
    fn classifies_authentication_and_permission_failures() {
        // Trace: L2-DES-GOAL-001
        assert_eq!(
            classify_goal_failure(Some("401 unauthorized invalid api key")),
            GoalFailureClass::Authentication
        );
        assert_eq!(
            classify_goal_failure(Some("403 forbidden no api permission")),
            GoalFailureClass::Permission
        );
    }
}
