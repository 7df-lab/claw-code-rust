//! Resume turns blocked on interactive approval after a process restart.

use std::sync::Arc;

use devo_core::tools::{
    AgentToolCoordinator, ClientFilesystem, PermissionChecker, ToolAgentScope, ToolCall,
    ToolExecutionOptions, ToolRuntime, ToolRuntimeContext,
};
use devo_core::{
    ContentBlock, ItemId, Message, Role, TurnApprovalCheckpointRecordedRecord, TurnConfig, TurnId,
    TurnStatus,
};
use devo_protocol::native::item::Item;
use devo_protocol::{ApprovalDecisionValue, CollaborationMode, SessionId};
use tokio::sync::mpsc;

use super::super::ServerRuntime;
use super::super::approval::{
    approved_permission_grant_for_request, native_approval_scope, native_approval_target,
    native_decided_approval_item,
};
use super::super::approval_checkpoint::{
    collaboration_mode_from_checkpoint, host_session_id_from_checkpoint,
    permission_request_from_messages, tool_permission_request_from_checkpoint,
};
use super::super::interaction_items::core_item_id_from_native;
use super::tool_display::without_agent_coordination_tools;
use super::tool_results::emit_tool_result_item;
use super::{
    FinalizeTurnParams, QUERY_EVENT_CHANNEL_CAPACITY, TurnModelQueryParams,
    spawn_post_turn_scheduling, spawn_turn_event_stream,
};
use crate::execution::PersistedLivingItem;
use crate::runtime::TurnInputMode;

impl ServerRuntime {
    /// Handles a native approval response on a restored lane (no live turn task).
    pub(crate) async fn resolve_approval_from_control_response(
        self: &Arc<Self>,
        host_session_id: SessionId,
        owner_session_id: SessionId,
        turn_id: TurnId,
        approval_id: &str,
        decision: ApprovalDecisionValue,
        scope: devo_protocol::ApprovalScopeValue,
    ) {
        let checkpoint = self
            .checkpoint_for_approval(owner_session_id, approval_id)
            .await;
        let request = match checkpoint
            .as_ref()
            .and_then(tool_permission_request_from_checkpoint)
        {
            Some(request) => request,
            None => {
                if let Some(request) = self
                    .reconstructed_permission_request(approval_id, &checkpoint)
                    .await
                {
                    request
                } else {
                    tracing::warn!(
                        session_id = %owner_session_id,
                        approval_id,
                        "approval response without reconstructable permission request"
                    );
                    return;
                }
            }
        };
        let available_scopes = super::super::approval::approval_scopes_for_request(&request);
        let mut persisted = self
            .session_interactive
            .pending_snapshot(owner_session_id)
            .await
            .approvals
            .into_iter()
            .find(|pending| pending.approval_id == approval_id)
            .and_then(|pending| pending.persisted);
        if persisted.is_none() {
            persisted = self
                .persisted_approval_item_from_rollout(owner_session_id, approval_id)
                .await;
        }
        let Some(persisted) = persisted else {
            tracing::warn!(
                session_id = %owner_session_id,
                approval_id,
                "approval response without persisted approval item"
            );
            return;
        };
        self.resolve_approval_and_resume_turn(
            host_session_id,
            owner_session_id,
            turn_id,
            approval_id,
            decision,
            scope,
            &request,
            &available_scopes,
            &persisted,
            checkpoint,
        )
        .await;
    }

    async fn reconstructed_permission_request(
        &self,
        approval_id: &str,
        checkpoint: &Option<TurnApprovalCheckpointRecordedRecord>,
    ) -> Option<devo_core::tools::ToolPermissionRequest> {
        let checkpoint = checkpoint.as_ref()?;
        if let Some(request) = tool_permission_request_from_checkpoint(checkpoint) {
            return Some(request);
        }
        let handle = self.session(checkpoint.owner_session_id).await?;
        let cwd = handle
            .turn_reservation_snapshot()
            .await?
            .summary
            .cwd
            .clone();
        permission_request_from_messages(
            &checkpoint.messages,
            approval_id,
            checkpoint.owner_session_id,
            checkpoint.turn_id,
            &cwd,
        )
    }

    async fn persisted_approval_item_from_rollout(
        &self,
        owner_session_id: SessionId,
        approval_id: &str,
    ) -> Option<PersistedLivingItem> {
        let handle = self.session(owner_session_id).await?;
        let record = handle.record().await??;
        let history = devo_core::read_canonical_history(&record.rollout_path).ok()?;
        let checkpoints =
            super::super::approval_checkpoint::latest_approval_checkpoints_for_rollout(
                &record.rollout_path,
            );
        let recovered =
            super::super::interaction_items::latest_waiting_approvals(&history.items, &checkpoints)
                .into_iter()
                .chain(super::super::interaction_items::latest_decided_approvals(
                    &history.items,
                    &checkpoints,
                ))
                .find(|item| item.approval_id == approval_id)?;
        Some(recovered.persisted)
    }

    async fn checkpoint_for_approval(
        &self,
        session_id: SessionId,
        approval_id: &str,
    ) -> Option<TurnApprovalCheckpointRecordedRecord> {
        let handle = self.session(session_id).await?;
        let record = handle.record().await??;
        super::super::approval_checkpoint::latest_approval_checkpoints_for_rollout(
            &record.rollout_path,
        )
        .remove(approval_id)
    }

    /// Handles an approval decision from a restored or live interactive lane.
    pub(crate) async fn resolve_approval_and_resume_turn(
        self: &Arc<Self>,
        host_session_id: SessionId,
        owner_session_id: SessionId,
        turn_id: TurnId,
        approval_id: &str,
        decision: ApprovalDecisionValue,
        scope: devo_protocol::ApprovalScopeValue,
        request: &devo_core::tools::ToolPermissionRequest,
        available_scopes: &[String],
        persisted: &PersistedLivingItem,
        checkpoint: Option<TurnApprovalCheckpointRecordedRecord>,
    ) {
        use devo_protocol::native::item::{ApprovalDecision, ApprovalDecisionSource};
        let native_decision = match &decision {
            ApprovalDecisionValue::Approve => {
                devo_protocol::native::item::ApprovalDecisionKind::Approved
            }
            ApprovalDecisionValue::Deny => {
                devo_protocol::native::item::ApprovalDecisionKind::Denied
            }
            ApprovalDecisionValue::Cancel => {
                devo_protocol::native::item::ApprovalDecisionKind::Cancelled
            }
        };
        self.persist_resolved_approval_item(
            owner_session_id,
            turn_id,
            request,
            available_scopes,
            native_decision,
            native_approval_scope(&scope),
            ApprovalDecisionSource::User,
            persisted,
        )
        .await;
        let item_id = core_item_id_from_native(&persisted.item_id).unwrap_or_else(|| {
            tracing::warn!(
                session_id = %owner_session_id,
                approval_id,
                item_id = %persisted.item_id.as_str(),
                "failed to map native approval item id; emitting completion with new id"
            );
            ItemId::new()
        });
        self.emit_native_item_completed(
            owner_session_id,
            turn_id,
            item_id,
            Some(persisted.seq),
            native_decided_approval_item(
                approval_id,
                request,
                available_scopes,
                native_approval_target(request),
                ApprovalDecision {
                    decision: native_decision,
                    scope: native_approval_scope(&scope),
                    decision_source: ApprovalDecisionSource::User,
                    decided_at: chrono::Utc::now(),
                },
            ),
        )
        .await;

        let pending_checkpoint = if let Some(pending) = self
            .session_interactive
            .take_pending_approval(host_session_id, approval_id)
            .await
        {
            let checkpoint = pending.checkpoint.or(checkpoint);
            let _ = pending.tx.send(decision.clone());
            if matches!(decision, ApprovalDecisionValue::Approve) {
                let (scope_tx, _) = tokio::sync::oneshot::channel();
                let pending_for_scope = crate::execution::PendingApproval {
                    owner_session_id: pending.owner_session_id,
                    turn_id: pending.turn_id,
                    tool_name: pending.tool_name,
                    resource: pending.resource,
                    path: pending.path,
                    host: pending.host,
                    command_prefix: pending.command_prefix,
                    command_pattern: pending.command_pattern,
                    requests_escalation: pending.requests_escalation,
                    command: pending.command,
                    cwd: pending.cwd,
                    sandbox_permissions: pending.sandbox_permissions,
                    persisted: pending.persisted,
                    checkpoint: checkpoint.clone(),
                    tx: scope_tx,
                };
                self.apply_approval_scope_to_turn_inline(
                    host_session_id,
                    &scope,
                    &pending_for_scope,
                )
                .await;
                if let Some(session_handle) = self.session(host_session_id).await {
                    let prefix_to_persist = (scope
                        == devo_protocol::ApprovalScopeValue::CommandPrefixPersist)
                        .then(|| pending_for_scope.command_prefix.clone())
                        .flatten();
                    session_handle
                        .apply_approval_scope(scope, pending_for_scope)
                        .await;
                    if let Some(prefix) = prefix_to_persist
                        && let Err(error) = self.persist_command_prefix_rule(&prefix).await
                    {
                        tracing::warn!(
                            session_id = %host_session_id,
                            error = %error,
                            "failed to persist command prefix rule"
                        );
                    }
                }
            }
            checkpoint
        } else {
            checkpoint
        };

        if !matches!(decision, ApprovalDecisionValue::Approve) {
            return;
        }
        if self.active_turns.has_session(owner_session_id).await {
            return;
        }
        let Some(checkpoint) = pending_checkpoint else {
            tracing::warn!(
                session_id = %owner_session_id,
                approval_id,
                "approved without live turn or durable checkpoint; cannot resume"
            );
            return;
        };
        let runtime = Arc::clone(self);
        let approval_id = approval_id.to_string();
        let request = request.clone();
        let host_session_id = host_session_id_from_checkpoint(&checkpoint);
        tokio::spawn(async move {
            if let Err(error) = runtime
                .spawn_approval_continuation_task(
                    host_session_id,
                    owner_session_id,
                    checkpoint,
                    &request,
                    &approval_id,
                )
                .await
            {
                tracing::warn!(
                    session_id = %owner_session_id,
                    approval_id,
                    error = %error,
                    "approval continuation failed"
                );
            }
        });
    }

    async fn spawn_approval_continuation_task(
        self: &Arc<Self>,
        host_session_id: SessionId,
        owner_session_id: SessionId,
        checkpoint: TurnApprovalCheckpointRecordedRecord,
        request: &devo_core::tools::ToolPermissionRequest,
        approval_id: &str,
    ) -> Result<(), String> {
        let session_id = owner_session_id;
        let handle = self
            .session(session_id)
            .await
            .ok_or_else(|| "session not found".to_string())?;
        let turn = {
            let reservation = handle
                .turn_reservation_snapshot()
                .await
                .ok_or_else(|| "turn reservation snapshot unavailable".to_string())?;
            reservation
                .active_turn
                .filter(|turn| turn.turn_id == checkpoint.turn_id)
                .or_else(|| {
                    reservation
                        .latest_turn
                        .filter(|turn| turn.turn_id == checkpoint.turn_id)
                })
                .ok_or_else(|| "turn metadata not found".to_string())?
        };
        if turn.status == TurnStatus::Completed {
            return Ok(());
        }
        let collaboration_mode = collaboration_mode_from_checkpoint(&checkpoint);
        let turn_config = self
            .turn_config_from_checkpoint(&checkpoint, &handle)
            .await?;
        if !self
            .active_turns
            .try_claim_session(session_id, turn.clone())
            .await
        {
            return Err("turn already active".to_string());
        }
        let cancel_token = self
            .register_active_turn_execution(session_id, turn.clone(), None)
            .await;
        let began = handle
            .try_begin_active_turn(turn.clone(), turn_config.clone())
            .await
            .unwrap_or(false);
        if !began {
            self.clear_active_turn_runtime_handles(session_id).await;
            return Err("failed to begin active turn".to_string());
        }
        let turn_id = checkpoint.turn_id;
        let continuation_result = self
            .run_approval_continuation(
                host_session_id,
                session_id,
                turn.clone(),
                checkpoint,
                request,
                approval_id,
                collaboration_mode,
                turn_config,
                &handle,
            )
            .await;
        if continuation_result.is_err() {
            let _ = handle.interrupt_active_turn().await;
        }
        self.clear_turn_spawn_snapshot(session_id, turn_id).await;
        self.unregister_active_stream(session_id).await;
        self.clear_active_turn_interrupt_handles(session_id).await;
        self.clear_active_turn_runtime_handles(session_id).await;
        cancel_token.cancel();
        continuation_result
    }

    async fn run_approval_continuation(
        self: &Arc<Self>,
        _host_session_id: SessionId,
        session_id: SessionId,
        turn: crate::TurnMetadata,
        checkpoint: TurnApprovalCheckpointRecordedRecord,
        request: &devo_core::tools::ToolPermissionRequest,
        approval_id: &str,
        collaboration_mode: CollaborationMode,
        turn_config: TurnConfig,
        handle: &super::super::session_actor::SessionHandle,
    ) -> Result<(), String> {
        let mut working = handle
            .checkout_turn_working_set(turn.clone())
            .await
            .ok_or_else(|| "failed to checkout turn working set".to_string())?;
        working.state.core.messages = checkpoint.messages.clone();
        working.state.core.collaboration_mode = collaboration_mode;
        working.state.summary.collaboration_mode = collaboration_mode;

        let spawn_snapshot = Arc::new(working.state.spawn_snapshot());
        self.register_turn_spawn_snapshot(session_id, turn.turn_id, Arc::clone(&spawn_snapshot))
            .await;
        self.register_active_stream(session_id, Arc::clone(&working.state.stream))
            .await;

        if !self
            .turn_has_tool_result(session_id, turn.turn_id, approval_id)
            .await
        {
            Self::execute_approved_tool_for_resume(
                Arc::clone(self),
                session_id,
                turn.turn_id,
                &turn_config,
                collaboration_mode,
                request,
                &mut working,
            )
            .await?;
        }

        let (event_tx, event_rx) = mpsc::channel(QUERY_EVENT_CHANNEL_CAPACITY);
        let event_tool_registry = if working.state.summary.parent_session_id.is_some() {
            Arc::new(without_agent_coordination_tools(
                &self.tool_registry_for_actor_state(&working.state),
            ))
        } else {
            self.tool_registry_for_actor_state(&working.state)
        };
        let usage_parent_session_id = working.state.parent_session_id();
        let usage_context_window = Some(turn_config.model.context_window as u64);
        let stream = Arc::clone(&working.state.stream);
        let event_task = spawn_turn_event_stream(
            Arc::clone(self),
            stream,
            session_id,
            turn.clone(),
            collaboration_mode,
            event_tool_registry,
            usage_parent_session_id,
            usage_context_window,
            event_rx,
        );
        let query_outcome = self
            .run_turn_model_query(TurnModelQueryParams {
                state: &mut working.state,
                turn_id: turn.turn_id,
                turn_config: &turn_config,
                input: "",
                input_messages: &[],
                collaboration_mode,
                input_mode: TurnInputMode::ApprovalResume,
                usage_parent_session_id,
                event_tx,
            })
            .await;
        let event_summary = event_task.await.ok();
        self.finalize_executed_turn(FinalizeTurnParams {
            state: &mut working.state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id,
        })
        .await;
        let inline = {
            let mut stream = working.state.stream.lock().await;
            stream.turn_inline.take()
        };
        if let Some(inline) = inline {
            inline.merge_into(&mut working.state);
        }
        let should_auto_continue_goal =
            working.state.latest_turn.as_ref().is_some_and(|turn| {
                matches!(turn.status, TurnStatus::Completed | TurnStatus::Failed)
            });
        if let Some(handle) = self.session(session_id).await {
            handle.merge_turn(working).await;
        }
        spawn_post_turn_scheduling(Arc::clone(self), session_id, should_auto_continue_goal);
        Ok(())
    }

    async fn turn_config_from_checkpoint(
        &self,
        checkpoint: &TurnApprovalCheckpointRecordedRecord,
        handle: &super::super::session_actor::SessionHandle,
    ) -> Result<TurnConfig, String> {
        let snapshot = handle
            .turn_reservation_snapshot()
            .await
            .ok_or_else(|| "turn reservation snapshot unavailable".to_string())?;
        let requested_model = checkpoint
            .turn_config
            .get("modelBindingId")
            .and_then(|value| value.as_str())
            .or_else(|| {
                checkpoint
                    .turn_config
                    .get("modelSlug")
                    .and_then(|value| value.as_str())
            })
            .or(snapshot.summary.model_binding_id.as_deref())
            .or(snapshot.summary.model.as_deref());
        let reasoning_effort = checkpoint
            .turn_config
            .get("reasoningEffortSelection")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or(snapshot.summary.reasoning_effort_selection.clone());
        Ok(snapshot
            .runtime_context
            .resolve_turn_config(requested_model, reasoning_effort))
    }

    async fn turn_has_tool_result(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        approval_id: &str,
    ) -> bool {
        let handle = match self.session(session_id).await {
            Some(handle) => handle,
            None => return false,
        };
        let record = match handle.record().await {
            Some(Some(record)) => record,
            _ => return false,
        };
        let Ok(history) = devo_core::read_canonical_history(&record.rollout_path) else {
            return false;
        };
        let turn_id = turn_id.to_string();
        history.items.iter().any(|item| {
            item.turn_id.as_str() == turn_id
                && matches!(
                    &item.item,
                    Item::ToolResult { call_id, .. }
                        | Item::CommandExecution { call_id, .. }
                        if call_id == approval_id
                )
        })
    }

    async fn execute_approved_tool_for_resume(
        runtime: Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        turn_config: &TurnConfig,
        collaboration_mode: CollaborationMode,
        request: &devo_core::tools::ToolPermissionRequest,
        working: &mut super::super::session_actor::TurnWorkingSet,
    ) -> Result<(), String> {
        let agent_scope = if working.state.summary.parent_session_id.is_some() {
            ToolAgentScope::Subagent
        } else {
            ToolAgentScope::Parent
        };
        let session_tool_registry = runtime.tool_registry_for_actor_state(&working.state);
        let registry = Arc::clone(&session_tool_registry);
        let approval_id = request.tool_call_id.clone();
        let request_for_checker = request.clone();
        let preapproved_checker = PermissionChecker::new(move |incoming| {
            let approval_id = approval_id.clone();
            let request = request_for_checker.clone();
            Box::pin(async move {
                if incoming.tool_call_id == approval_id {
                    Ok(approved_permission_grant_for_request(&request))
                } else {
                    Err("unexpected tool approval during resume".to_string())
                }
            })
        });
        let tool_runtime = ToolRuntime::new_with_context_and_options(
            registry,
            preapproved_checker,
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                cwd: working.state.core.cwd.clone(),
                agent_scope,
                collaboration_mode,
                agent_coordinator: Some(Arc::clone(&runtime) as Arc<dyn AgentToolCoordinator>),
                client_filesystem: Some(Arc::clone(&runtime) as Arc<dyn ClientFilesystem>),
                file_read_ledger: Arc::clone(&working.state.file_read_ledger),
                local_web_search: match &turn_config.web_search {
                    devo_core::ResolvedWebSearchConfig::Local(config) => Some(config.clone()),
                    devo_core::ResolvedWebSearchConfig::Disabled
                    | devo_core::ResolvedWebSearchConfig::Provider => None,
                },
                hooks: ServerRuntime::hook_context_from_actor_state(&working.state, session_id),
                network_proxy: None,
                network_no_proxy: None,
                sandbox_profile: working.state.core.config.sandbox_profile.clone(),
                sandbox_profile_live: None,
            },
            ToolExecutionOptions::default(),
        );
        let call = ToolCall {
            id: request.tool_call_id.clone(),
            name: request.tool_name.clone(),
            input: request.input.clone(),
        };
        let results = tool_runtime.execute_batch(&[call]).await;
        let result = results
            .into_iter()
            .next()
            .ok_or_else(|| "tool execution produced no result".to_string())?;
        let content_str = result
            .display_content
            .clone()
            .unwrap_or_else(|| format!("{:?}", result.content));
        working.state.core.push_message(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: result.tool_use_id.clone(),
                content: content_str.clone(),
                is_error: result.is_error,
            }],
        });
        emit_tool_result_item(
            &runtime,
            session_id,
            turn_id,
            result.tool_use_id,
            Some(request.tool_name.clone()),
            None,
            result.content,
            result.display_content,
            result.is_error,
            "approval resume tool result".to_string(),
        )
        .await;
        Ok(())
    }

    /// Reconcile orphan waiting-approval turns after hydrate.
    pub(super) async fn reconcile_waiting_approval_turns_after_hydrate(
        self: &Arc<Self>,
        session_id: SessionId,
        rollout_path: &std::path::Path,
    ) {
        let Ok(history) = devo_core::read_canonical_history(rollout_path) else {
            return;
        };
        let checkpoints =
            super::super::approval_checkpoint::latest_approval_checkpoints_for_rollout(
                rollout_path,
            );
        let Some(latest_turn) = history.turns.last() else {
            return;
        };
        if matches!(
            latest_turn.status,
            devo_protocol::native::turn::TurnStatus::Completed
                | devo_protocol::native::turn::TurnStatus::Failed
                | devo_protocol::native::turn::TurnStatus::Interrupted
        ) {
            return;
        }
        if self.active_turns.has_session(session_id).await {
            return;
        }
        for recovered in
            super::super::interaction_items::latest_decided_approvals(&history.items, &checkpoints)
        {
            if recovered.owner_session_id != session_id {
                continue;
            }
            let Some(checkpoint) = checkpoints.get(&recovered.approval_id).cloned() else {
                continue;
            };
            if let Some(decision) = recovered.decided_approval()
                && matches!(decision, ApprovalDecisionValue::Approve)
                && let Some(request) = tool_permission_request_from_checkpoint(&checkpoint)
            {
                let host_session_id = host_session_id_from_checkpoint(&checkpoint);
                self.resolve_approval_and_resume_turn(
                    host_session_id,
                    recovered.owner_session_id,
                    recovered.turn_id,
                    &recovered.approval_id,
                    decision,
                    recovered.scope(),
                    &request,
                    &recovered.available_scopes,
                    &recovered.persisted,
                    Some(checkpoint),
                )
                .await;
            }
        }
    }

    /// Reconstructs answerable approval lanes from persisted Waiting items.
    pub(crate) async fn restore_waiting_approvals_from_rollout(
        self: &Arc<Self>,
        session_id: SessionId,
        _host_session_id: SessionId,
        rollout_path: &std::path::Path,
    ) {
        let Ok(history) = devo_core::read_canonical_history(rollout_path) else {
            return;
        };
        let checkpoints =
            super::super::approval_checkpoint::latest_approval_checkpoints_for_rollout(
                rollout_path,
            );
        for recovered in
            super::super::interaction_items::latest_waiting_approvals(&history.items, &checkpoints)
        {
            if recovered.owner_session_id != session_id {
                continue;
            }
            if recovered.decision.is_some() {
                continue;
            }
            if self
                .session_interactive
                .has_pending_approval(&recovered.approval_id)
                .await
            {
                continue;
            }
            let checkpoint = checkpoints.get(&recovered.approval_id).cloned();
            let lane_host = checkpoint
                .as_ref()
                .map(host_session_id_from_checkpoint)
                .unwrap_or(recovered.host_session_id);
            let (tx, _rx) = tokio::sync::oneshot::channel();
            let (controller_tx, _controller_rx) = tokio::sync::mpsc::unbounded_channel();
            let request = checkpoint
                .as_ref()
                .and_then(tool_permission_request_from_checkpoint);
            self.session_interactive
                .register_pending_approval(
                    lane_host,
                    recovered.approval_id.clone(),
                    crate::execution::PendingApproval {
                        owner_session_id: recovered.owner_session_id,
                        turn_id: recovered.turn_id,
                        tool_name: request
                            .as_ref()
                            .map(|request| request.tool_name.clone())
                            .unwrap_or_default(),
                        resource: request.as_ref().map(|request| request.resource.clone()),
                        path: request.as_ref().and_then(|request| request.path.clone()),
                        host: request.as_ref().and_then(|request| request.host.clone()),
                        command_prefix: request
                            .as_ref()
                            .and_then(|request| request.command_prefix.clone()),
                        command_pattern: request
                            .as_ref()
                            .and_then(|request| request.command_pattern.clone()),
                        requests_escalation: request.as_ref().is_some_and(|request| {
                            request.sandbox_permissions.requests_escalation()
                        }),
                        command: request
                            .as_ref()
                            .and_then(devo_core::tools::command_str_for_permission_request),
                        cwd: request
                            .as_ref()
                            .map(|request| request.cwd.clone())
                            .unwrap_or_default(),
                        sandbox_permissions: request
                            .as_ref()
                            .map(|request| {
                                devo_core::tools::sandbox_permission_cache_key_from_input(
                                    &request.input,
                                )
                            })
                            .unwrap_or_default(),
                        persisted: Some(recovered.persisted),
                        checkpoint,
                        tx,
                    },
                    controller_tx,
                    recovered.available_scopes,
                )
                .await;
        }
        self.reconcile_waiting_approval_turns_after_hydrate(session_id, rollout_path)
            .await;
    }
}
