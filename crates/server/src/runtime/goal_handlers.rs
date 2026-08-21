use super::*;

/// Projects the server-internal goal into the canonical goal shape
/// (L2-DES-APP-008 Phase B, goal domain). `Cleared` goals are filtered by
/// callers (canonical `goal/read` answers `None` for them).
fn native_goal_from_internal(goal: &crate::goal::Goal) -> devo_protocol::native::goal::Goal {
    use devo_protocol::native::goal::GoalStatus as NativeStatus;
    let status = match goal.status {
        crate::goal::GoalStatus::Active => NativeStatus::Active,
        crate::goal::GoalStatus::Paused => NativeStatus::Paused,
        crate::goal::GoalStatus::Blocked => NativeStatus::Blocked,
        crate::goal::GoalStatus::BudgetLimited => NativeStatus::BudgetLimited,
        crate::goal::GoalStatus::Completed => NativeStatus::Completed,
        crate::goal::GoalStatus::Failed => NativeStatus::Failed,
        crate::goal::GoalStatus::Canceled | crate::goal::GoalStatus::Cleared => {
            NativeStatus::Canceled
        }
    };
    devo_protocol::native::goal::Goal {
        id: devo_protocol::native::ids::GoalId::from_string(format!(
            "goal_{}",
            goal.durable_goal_id.0
        )),
        session_id: devo_protocol::native::ids::SessionId::from_string(goal.session_id.to_string()),
        objective: goal.prompt.clone(),
        status,
        token_budget: goal
            .budget
            .max_tokens
            .and_then(|budget| u64::try_from(budget).ok()),
        tokens_used: u64::try_from(goal.usage.tokens_used).unwrap_or(0),
        time_used_seconds: goal.usage.duration_seconds,
        progress_summary: goal.progress_summary.clone(),
        created_at: goal.created_at,
        updated_at: goal.updated_at,
    }
}

impl ServerRuntime {
    // ── Goal Handlers ─────────────────────────────────────────────────

    /// Native `session/goal/set` (L2-DES-APP-008 Phase B): creates the
    /// session goal with `ifExists` semantics and idempotency-key replay.
    pub(super) async fn handle_native_session_goal_set(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalSetParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/set params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        if let Some(goal) = self
            .goal_set_idempotency
            .lock()
            .await
            .get(&(legacy_session_id, params.idempotency_key.clone()))
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalSetResult { goal },
            })
            .expect("serialize canonical session/goal/set response");
        }
        let legacy_params = devo_protocol::GoalCreateParams {
            session_id: legacy_session_id,
            objective: params.objective.clone(),
            token_budget: params
                .token_budget
                .and_then(|budget| i64::try_from(budget).ok()),
            replace_existing: matches!(
                params.if_exists,
                devo_protocol::native::rpc_session::GoalIfExists::Replace
            ),
        };
        let response = self
            .handle_goal_create(
                request_id.clone(),
                serde_json::to_value(&legacy_params).expect("serialize legacy goal params"),
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        let Some(native_goal) = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get().map(native_goal_from_internal))
        else {
            return response;
        };
        self.goal_set_idempotency.lock().await.insert(
            (legacy_session_id, params.idempotency_key),
            native_goal.clone(),
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalSetResult { goal: native_goal },
        })
        .expect("serialize canonical session/goal/set response")
    }

    /// Native `session/goal/update` (ratified #3): in-place edit of the
    /// current goal preserving id, usage stats, and continuation linkage.
    /// Translates the patch into the legacy in-place `goal/set` path.
    pub(super) async fn handle_native_session_goal_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalUpdateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/update params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        // Precondition: an active goal exists and matches expectedGoalId.
        let current = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get().map(native_goal_from_internal));
        let Some(current) = current else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "no active goal to update",
            );
        };
        if let Some(expected) = params.expected_goal_id.as_ref()
            && *expected != current.id
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "goal was replaced; refetch before editing",
            );
        }
        if let Some(goal) = self
            .goal_update_idempotency
            .lock()
            .await
            .get(&(legacy_session_id, params.idempotency_key.clone()))
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalUpdateResult { goal },
            })
            .expect("serialize canonical session/goal/update replay response");
        }

        let status = match params.patch.status {
            None => None,
            Some(devo_protocol::native::goal::GoalStatus::Active) => {
                Some(devo_protocol::ThreadGoalStatus::Active)
            }
            Some(devo_protocol::native::goal::GoalStatus::Paused) => {
                Some(devo_protocol::ThreadGoalStatus::Paused)
            }
            Some(devo_protocol::native::goal::GoalStatus::Completed) => {
                Some(devo_protocol::ThreadGoalStatus::Complete)
            }
            Some(system_controlled) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("goal status {system_controlled:?} is system-computed, not editable"),
                );
            }
        };
        let token_budget = match params.patch.token_budget {
            devo_protocol::native::patch::PatchField::Missing => None,
            devo_protocol::native::patch::PatchField::Null => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "clearing the token budget is not in the vocabulary",
                );
            }
            devo_protocol::native::patch::PatchField::Value(budget) => Some(budget),
        };
        let legacy_params = devo_protocol::GoalSetParams {
            session_id: legacy_session_id,
            objective: params.patch.objective.clone(),
            status,
            token_budget,
        };
        let response = self
            .handle_goal_set(
                request_id.clone(),
                serde_json::to_value(&legacy_params).expect("serialize legacy goal/set params"),
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        let Some(native_goal) = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get().map(native_goal_from_internal))
        else {
            return response;
        };
        self.goal_update_idempotency.lock().await.insert(
            (legacy_session_id, params.idempotency_key),
            native_goal.clone(),
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalUpdateResult {
                goal: native_goal,
            },
        })
        .expect("serialize canonical session/goal/update response")
    }

    /// Native `session/goal/read`: the session's current goal, or `null`
    /// when none (including cleared goals).
    pub(super) async fn handle_native_session_goal_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/read params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        let goal = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get())
            .filter(|goal| goal.status != crate::goal::GoalStatus::Cleared)
            .map(native_goal_from_internal);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalReadResult { goal },
        })
        .expect("serialize canonical session/goal/read response")
    }

    /// Native goal lifecycle transitions (`session/goal/pause|resume|
    /// complete|cancel|clear`). `expectedGoalId` is a precondition against
    /// acting on a concurrently replaced goal.
    pub(super) async fn handle_native_session_goal_transition(
        self: &Arc<Self>,
        method: &str,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalTransitionParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical {method} params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        let current_goal = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get().cloned());
        let Some(current_goal) = current_goal else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "session has no goal",
            );
        };
        if params.expected_goal_id.as_str() != format!("goal_{}", current_goal.durable_goal_id.0) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "expected goal id does not match the session's current goal",
            );
        }
        let legacy_status_params = |status: devo_protocol::ThreadGoalStatus| {
            serde_json::json!({
                "sessionId": legacy_session_id,
                "status": status,
            })
        };
        let response = match method {
            "session/goal/pause" => {
                self.handle_goal_pause(
                    request_id.clone(),
                    legacy_status_params(devo_protocol::ThreadGoalStatus::Paused),
                )
                .await
            }
            "session/goal/resume" => {
                self.handle_goal_resume(
                    request_id.clone(),
                    legacy_status_params(devo_protocol::ThreadGoalStatus::Active),
                )
                .await
            }
            "session/goal/complete" => {
                self.handle_goal_complete(
                    request_id.clone(),
                    legacy_status_params(devo_protocol::ThreadGoalStatus::Complete),
                )
                .await
            }
            "session/goal/cancel" => {
                self.handle_goal_cancel(
                    request_id.clone(),
                    serde_json::json!({
                        "sessionId": legacy_session_id,
                        "goalId": current_goal.goal_id.0,
                    }),
                )
                .await
            }
            "session/goal/clear" => {
                let response = self
                    .handle_goal_clear(
                        request_id.clone(),
                        serde_json::json!({ "sessionId": legacy_session_id }),
                    )
                    .await;
                if response.get("error").is_some() {
                    return response;
                }
                return serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::native::rpc_session::SessionGoalClearResult {},
                })
                .expect("serialize canonical session/goal/clear response");
            }
            _ => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("unknown goal transition method '{method}'"),
                );
            }
        };
        if response.get("error").is_some() {
            return response;
        }
        let Some(goal) = self
            .goal_stores
            .lock()
            .await
            .get(&legacy_session_id)
            .and_then(|store| store.get().map(native_goal_from_internal))
        else {
            return response;
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalTransitionResult { goal },
        })
        .expect("serialize canonical goal transition response")
    }

    // ── Goal Handlers ─────────────────────────────────────────────────

    pub(super) async fn handle_goal_create(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalCreateParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/create params: {e}"),
                );
            }
        };
        let session_id = params.session_id;
        let replace_existing = params.replace_existing;
        let title_input = params.objective.trim().to_string();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        match store.create(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalCreateResult { goal: thread_goal },
                })
                .expect("serialize goal create result");
                drop(stores);
                if let Err(error) = self
                    .goal_durable_store
                    .append_goal_created(&durable_goal)
                    .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal create record");
                }
                // Interrupt before any session-actor mailbox round-trip: the actor
                // may be blocked inside an in-flight continuation turn.
                if replace_existing {
                    self.interrupt_active_goal_continuation_turn(session_id, "goal replaced")
                        .await;
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                self.schedule_goal_followup_work(session_id, Some(title_input), should_continue)
                    .await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal creation failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_set(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/set params: {e}"),
                );
            }
        };
        let session_id = params.session_id;
        let requested_status = params.status;
        let title_input = params
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .map(str::to_string);
        let only_pause_budget_limited = requested_status
            == Some(devo_protocol::ThreadGoalStatus::Paused)
            && params.objective.is_none()
            && params.token_budget.is_none();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        let previous_status = store.get().map(|goal| goal.status);
        if previous_status == Some(crate::goal::GoalStatus::BudgetLimited)
            && only_pause_budget_limited
            && let Some(goal) = store.get().cloned()
        {
            let thread_goal = goal.to_thread_goal();
            let result = serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetResult { goal: thread_goal },
            })
            .expect("serialize budget-limited goal pause result");
            drop(stores);
            self.interrupt_active_goal_continuation_turn(
                session_id,
                "budget-limited goal wrap-up stopped",
            )
            .await;
            self.sync_core_session_goal(session_id, None).await;
            return result;
        }
        match store.set(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let should_interrupt_continuation = previous_status.is_some_and(|status| {
                    matches!(
                        status,
                        crate::goal::GoalStatus::Active | crate::goal::GoalStatus::BudgetLimited
                    )
                }) && !should_continue;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetResult { goal: thread_goal },
                })
                .expect("serialize goal set result");
                drop(stores);
                if let Err(error) = self
                    .goal_durable_store
                    .append_goal_created(&durable_goal)
                    .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal set record");
                }
                let status_record_base = previous_status.unwrap_or(crate::goal::GoalStatus::Active);
                if status_record_base != durable_goal.status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, status_record_base, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal status record");
                }
                if should_interrupt_continuation {
                    self.interrupt_active_goal_continuation_turn(
                        session_id,
                        "goal status changed from active",
                    )
                    .await;
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                self.schedule_goal_followup_work(session_id, title_input, should_continue)
                    .await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal set failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_pause(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/pause params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        let should_interrupt_continuation = previous_status.is_some_and(|status| {
            matches!(
                status,
                crate::goal::GoalStatus::Active | crate::goal::GoalStatus::BudgetLimited
            )
        });
        if previous_status == Some(crate::goal::GoalStatus::BudgetLimited)
            && let Some(goal) = store.get().cloned()
        {
            let thread_goal = goal.to_thread_goal();
            let result = serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
            })
            .expect("serialize budget-limited goal pause result");
            let session_id = params.session_id;
            drop(stores);
            self.interrupt_active_goal_continuation_turn(
                session_id,
                "budget-limited goal wrap-up stopped",
            )
            .await;
            self.sync_core_session_goal(session_id, None).await;
            return result;
        }
        match store.set_status(devo_protocol::ThreadGoalStatus::Paused) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal pause result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal pause record");
                }
                if should_interrupt_continuation {
                    self.interrupt_active_goal_continuation_turn(session_id, "goal paused")
                        .await;
                }
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal pause failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_resume(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/resume params: {e}"),
                );
            }
        };
        let session_id = params.session_id;

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(devo_protocol::ThreadGoalStatus::Active) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal resume result");
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal resume record");
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                self.schedule_goal_followup_work(
                    session_id,
                    /*title_input*/ None,
                    should_continue,
                )
                .await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal resume failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_complete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/complete params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(devo_protocol::ThreadGoalStatus::Complete) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal complete result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal complete record");
                }
                self.interrupt_active_goal_continuation_turn(session_id, "goal completed")
                    .await;
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal complete failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_cancel(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalCancelParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/cancel params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Cancel,
        }) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal cancel result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal cancel record");
                }
                self.interrupt_active_goal_continuation_turn(session_id, "goal canceled")
                    .await;
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal cancel failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_clear(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalClearParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/clear params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let cleared_goal_id = stores
            .get(&params.session_id)
            .and_then(GoalStore::get)
            .map(|goal| goal.durable_goal_id);
        let cleared = stores
            .get_mut(&params.session_id)
            .is_some_and(GoalStore::clear);
        drop(stores);
        if cleared {
            if let Some(goal_id) = cleared_goal_id
                && let Err(error) = self
                    .goal_durable_store
                    .append_goal_cleared(params.session_id, goal_id, Some("user clear".to_string()))
                    .await
            {
                tracing::warn!(session_id = %params.session_id, error = %error, "failed to persist goal clear record");
            }
            self.interrupt_active_goal_continuation_turn(params.session_id, "goal cleared")
                .await;
            self.sync_core_session_goal(params.session_id, None).await;
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::GoalClearResult { cleared },
        })
        .expect("serialize goal clear result")
    }

    pub(super) async fn handle_goal_status(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/status params: {e}"),
                );
            }
        };

        let stores = self.goal_stores.lock().await;
        let goal_store: Option<&GoalStore> = stores.get(&params.session_id);
        let projection = goal_store
            .and_then(|store| store.get())
            .map(Goal::to_thread_goal);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::GoalStatusResult { goal: projection },
        })
        .expect("serialize goal status result")
    }

    pub(super) async fn sync_core_session_goal(
        &self,
        session_id: SessionId,
        goal: Option<devo_protocol::ThreadGoal>,
    ) {
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        if self.runtime_active_turn_id(session_id).await.is_some() {
            // Queue without blocking the goal handler; the actor applies this once
            // the in-flight turn releases the mailbox.
            let _ = session_handle.try_set_active_goal(goal);
            return;
        }
        session_handle.set_active_goal(goal).await;
    }

    /// Title generation needs session-actor mailbox replies. When a turn is
    /// already running inline on that actor, awaiting those replies deadlocks
    /// the goal handler. Defer title work to a task and rely on the post-turn
    /// hook as a fallback while a turn is active.
    async fn schedule_goal_followup_work(
        self: &Arc<Self>,
        session_id: SessionId,
        title_input: Option<String>,
        should_continue: bool,
    ) {
        let turn_active = self.runtime_active_turn_id(session_id).await.is_some();
        if let Some(title_input) = title_input {
            if turn_active {
                let runtime = Arc::clone(self);
                tokio::spawn(async move {
                    runtime
                        .maybe_start_title_generation_from_user_input(session_id, &title_input)
                        .await;
                });
            } else {
                self.maybe_start_title_generation_from_user_input(session_id, &title_input)
                    .await;
            }
        }
        if !should_continue {
            return;
        }
        if turn_active {
            return;
        }
        self.maybe_start_goal_continuation_turn(session_id).await;
    }
}
