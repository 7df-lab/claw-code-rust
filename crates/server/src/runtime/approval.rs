use super::*;

use crate::execution::PendingApproval;
use crate::runtime::session_actor::approval_scope::{
    apply_approval_scope_to_state, apply_path_scope_to_permission_profile,
};
use crate::runtime::session_interactive::complete_approval_wait;

use std::path::Component;
use std::path::Path;

enum PolicyAuthorization {
    Allow,
    Ask,
    Deny(String),
}

enum AutoReviewOutcome {
    Approve,
    Deny(String),
    AskUser,
}

impl ServerRuntime {
    pub(super) fn build_permission_checker(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        permission_mode: PermissionMode,
        permission_profile: devo_safety::RuntimePermissionProfile,
    ) -> PermissionChecker {
        let runtime = Arc::clone(self);
        PermissionChecker::new(move |request| {
            let runtime = Arc::clone(&runtime);
            let permission_profile = permission_profile.clone();
            Box::pin(async move {
                runtime
                    .authorize_tool_request(
                        session_id,
                        turn_id,
                        permission_mode,
                        permission_profile,
                        request,
                    )
                    .await
            })
        })
    }

    async fn authorize_tool_request(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        permission_mode: PermissionMode,
        permission_profile: devo_safety::RuntimePermissionProfile,
        request: ToolPermissionRequest,
    ) -> Result<PermissionGrant, String> {
        if let Some(result) = permission_mode_authorization(permission_mode, &request) {
            return match result {
                Ok(grant) => {
                    if grant.bypass_sandbox
                        && let Err(reason) = self
                            .check_escalation_unsandboxed_forbidden(session_id, &request.cwd)
                            .await
                    {
                        self.run_permission_denied_hook(session_id, &request, &reason)
                            .await;
                        return Err(reason);
                    }
                    Ok(grant)
                }
                Err(reason) => {
                    self.run_permission_denied_hook(session_id, &request, &reason)
                        .await;
                    Err(reason)
                }
            };
        }
        if request.requests_escalation
            && let Err(reason) = self
                .check_escalation_unsandboxed_forbidden(session_id, &request.cwd)
                .await
        {
            self.run_permission_denied_hook(session_id, &request, &reason)
                .await;
            return Err(reason);
        }
        if let Some(grant) = self.approval_cache_grant(session_id, &request).await {
            return Ok(grant);
        }
        let permission_profile = self
            .live_permission_profile(session_id)
            .await
            .unwrap_or(permission_profile);
        let policy = policy_decision(
            &permission_profile,
            &request,
            self.user_exec_policy
                .lock()
                .expect("user exec policy lock poisoned")
                .as_ref(),
        );
        match policy {
            PolicyAuthorization::Allow => Ok(escalation_permission_grant(&request)),
            PolicyAuthorization::Deny(reason) => {
                self.run_permission_denied_hook(session_id, &request, &reason)
                    .await;
                Err(reason)
            }
            PolicyAuthorization::Ask => {
                if let Some(reason) = self
                    .permission_request_hook_block_reason(session_id, &request)
                    .await
                {
                    let message = format!("blocked by PermissionRequest hook: {reason}");
                    self.run_permission_denied_hook(session_id, &request, &message)
                        .await;
                    return Err(message);
                }
                if matches!(
                    permission_profile.reviewer,
                    devo_safety::ApprovalsReviewer::AutoReview
                ) {
                    // Explicit sandbox escalation must always reach a human.
                    if !request.requests_escalation {
                        match self
                            .auto_review_tool_request(
                                session_id,
                                turn_id,
                                &request,
                                &permission_profile,
                            )
                            .await
                        {
                            AutoReviewOutcome::Approve => {
                                return Ok(approved_permission_grant(&request));
                            }
                            AutoReviewOutcome::Deny(reason) => {
                                self.run_permission_denied_hook(session_id, &request, &reason)
                                    .await;
                                return Err(format!("rejected by auto-reviewer: {reason}"));
                            }
                            AutoReviewOutcome::AskUser => {}
                        }
                    }
                }
                let result = self
                    .request_tool_approval(session_id, request.clone())
                    .await;
                if let Err(reason) = &result {
                    self.run_permission_denied_hook(session_id, &request, reason)
                        .await;
                }
                result
            }
        }
    }

    async fn permission_request_hook_block_reason(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<String> {
        let report = self
            .run_session_hook(
                session_id,
                devo_core::HookEvent::PermissionRequest,
                permission_tool_extra(request),
            )
            .await;
        report.first_blocking_reason().map(str::to_string)
    }

    async fn run_permission_denied_hook(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
        reason: &str,
    ) {
        let mut extra = permission_tool_extra(request);
        extra.insert(
            "tool_use_id".to_string(),
            serde_json::json!(request.tool_call_id),
        );
        extra.insert("reason".to_string(), serde_json::json!(reason));
        self.run_session_hook(session_id, devo_core::HookEvent::PermissionDenied, extra)
            .await;
    }

    async fn auto_review_tool_request(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
        permission_profile: &devo_safety::RuntimePermissionProfile,
    ) -> AutoReviewOutcome {
        let (model, runtime_context) = {
            let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
                return AutoReviewOutcome::AskUser;
            };
            let runtime_context = reservation.runtime_context;
            (
                reservation
                    .summary
                    .model
                    .clone()
                    .unwrap_or_else(|| runtime_context.default_model.clone()),
                runtime_context,
            )
        };

        let context = self
            .build_approval_review_context(
                session_id,
                request,
                permission_profile,
                &runtime_context,
            )
            .await;
        let response = match runtime_context
            .provider
            .completion(build_approval_review_request(model, request, &context))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    error = %error,
                    "auto-review approval request failed"
                );
                return AutoReviewOutcome::AskUser;
            }
        };
        match parse_reviewer_decision(&response.content) {
            Some(ReviewerDecision::Approve { rationale }) => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review approved tool request"
                );
                self.emit_auto_review_decision(
                    session_id,
                    turn_id,
                    request,
                    "approve",
                    rationale.as_str(),
                )
                .await;
                AutoReviewOutcome::Approve
            }
            Some(ReviewerDecision::Deny { rationale }) => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review denied tool request"
                );
                self.emit_auto_review_decision(
                    session_id,
                    turn_id,
                    request,
                    "deny",
                    rationale.as_str(),
                )
                .await;
                AutoReviewOutcome::Deny(rationale)
            }
            Some(ReviewerDecision::Uncertain { rationale }) => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review deferred tool request to user"
                );
                AutoReviewOutcome::AskUser
            }
            None => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    "auto-review returned an invalid decision"
                );
                AutoReviewOutcome::AskUser
            }
        }
    }

    async fn build_approval_review_context(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
        permission_profile: &devo_safety::RuntimePermissionProfile,
        runtime_context: &Arc<crate::session_context::SessionRuntimeContext>,
    ) -> crate::approval_reviewer::ApprovalReviewContext {
        use crate::approval_reviewer::ApprovalReviewContext;

        let profile_summary = Some(format!(
            "preset: {:?}; writable_roots: [{}]; readable_roots: [{}]; allow_shell_commands: {}; allow_network: {}",
            permission_profile.preset,
            permission_profile
                .writable_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            permission_profile
                .readable_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            permission_profile.allow_shell_commands,
            permission_profile.allow_network,
        ));

        let agents_rules = devo_core::AgentsMdManager::new(runtime_context.agents_md.clone())
            .load(&request.cwd)
            .map(|snapshot| snapshot.rendered_instructions);

        let mut transcript_tail = Vec::new();
        let mut recent_decisions = Vec::new();

        // This runs inside the session's own turn, whose actor mailbox stays
        // blocked until the turn ends; `SessionHandle` round-trips from here
        // would deadlock. Read the snapshots registered at turn start instead.
        if let Some(snapshot) = self.active_spawn_snapshot_for_session(session_id).await {
            for item in snapshot.stable_items.iter().rev().take(10).rev() {
                transcript_tail.push(format_persisted_turn_item(item));
            }
        }
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                push_recent_approval_decisions(
                    "session",
                    &inline.session_approval_cache,
                    &mut recent_decisions,
                );
                push_recent_approval_decisions(
                    "turn",
                    &inline.turn_approval_cache,
                    &mut recent_decisions,
                );
            }
        }

        ApprovalReviewContext {
            profile_summary,
            agents_rules,
            transcript_tail,
            recent_decisions,
        }
    }

    async fn emit_auto_review_decision(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
        decision: &str,
        rationale: &str,
    ) {
        let approval_id = format!("auto-review-{}", request.tool_call_id);
        self.emit_turn_item(
            session_id,
            turn_id,
            ItemKind::ApprovalDecision,
            TurnItem::ApprovalDecision(ApprovalDecisionItem {
                approval_id: approval_id.clone(),
                decision: decision.to_string(),
                scope: "auto_review".to_string(),
            }),
            serde_json::json!({
                "approval_id": approval_id,
                "decision": decision,
                "scope": "auto_review",
                "rationale": rationale,
                "tool_name": request.tool_name,
                "resource": format!("{:?}", request.resource),
                "target": request.target,
            }),
        )
        .await;
    }

    async fn approval_cache_grant(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<PermissionGrant> {
        if let Some(grant) = self.session_approval_cache_grant(session_id, request).await {
            return Some(grant);
        }
        if let Some(parent_session_id) = self.parent_session_id(session_id).await {
            return self
                .session_approval_cache_grant(parent_session_id, request)
                .await;
        }
        None
    }

    /// Prefer the live turn-inline profile when a turn is in flight so mid-turn
    /// PathPrefix/Session grants are visible to `policy_decision`.
    async fn live_permission_profile(
        &self,
        session_id: SessionId,
    ) -> Option<devo_safety::RuntimePermissionProfile> {
        if let Some(profile) = self.turn_inline_permission_profile(session_id).await {
            return Some(profile);
        }
        if let Some(parent_session_id) = self.parent_session_id(session_id).await {
            return self.turn_inline_permission_profile(parent_session_id).await;
        }
        None
    }

    async fn turn_inline_permission_profile(
        &self,
        session_id: SessionId,
    ) -> Option<devo_safety::RuntimePermissionProfile> {
        let stream = self.active_stream_state(session_id).await?;
        let stream = stream.lock().await;
        stream
            .turn_inline
            .as_ref()
            .map(|inline| inline.hook_context.config.permission_profile.clone())
    }

    async fn apply_approval_scope_to_turn_inline(
        &self,
        session_id: SessionId,
        scope: &ApprovalScopeValue,
        pending: &PendingApproval,
    ) {
        let Some(stream) = self.active_stream_state(session_id).await else {
            return;
        };
        let mut stream = stream.lock().await;
        let Some(inline) = stream.turn_inline.as_mut() else {
            return;
        };
        apply_approval_scope_to_state(
            &mut inline.session_approval_cache,
            &mut inline.turn_approval_cache,
            scope,
            pending,
        );
        apply_path_scope_to_permission_profile(
            &mut inline.hook_context.config.permission_profile,
            scope,
            pending,
        );
    }

    async fn session_approval_cache_grant(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<PermissionGrant> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return cache_grant(&inline.session_approval_cache, request)
                    .or_else(|| cache_grant(&inline.turn_approval_cache, request));
            }
        }
        let session_handle = self.session(session_id).await?;
        let cache = session_handle.approval_cache_snapshot().await?;
        cache_grant(&cache.session_approval_cache, request)
            .or_else(|| cache_grant(&cache.turn_approval_cache, request))
    }

    async fn check_escalation_unsandboxed_forbidden(
        &self,
        session_id: SessionId,
        cwd: &Path,
    ) -> Result<(), String> {
        let Some(profile_name) = self.session_sandbox_profile(session_id, cwd).await else {
            return Ok(());
        };
        if profile_name == "off" {
            return Ok(());
        }
        if !devo_sandbox::unsandboxed_execution_allowed(Some(profile_name.as_str()), cwd) {
            return Err(
                "unsandboxed execution is forbidden when the session sandbox profile has deny-read paths configured"
                    .to_string(),
            );
        }
        Ok(())
    }

    async fn session_sandbox_profile(&self, session_id: SessionId, cwd: &Path) -> Option<String> {
        // Tool authorization runs on the session actor task during an in-flight
        // turn. Prefer the turn-inline snapshot so we never wait on the actor
        // mailbox (which would deadlock).
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.hook_context.config.sandbox_profile.clone();
            }
        }
        let session_handle = self.session(session_id).await?;
        session_handle
            .shell_exec_context(cwd.to_path_buf())
            .await
            .and_then(|context| context.sandbox_profile)
    }

    async fn parent_session_id(&self, session_id: SessionId) -> Option<SessionId> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.summary.parent_session_id;
            }
        }
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        session_handle.parent_session_id().await.and_then(|p| p)
    }

    /// Sub-agent turns route interactive approvals through the parent session so
    /// the active ACP connection and approval cache stay aligned with the UI.
    async fn permission_host_session_id(&self, session_id: SessionId) -> SessionId {
        let Some(parent_session_id) = self.parent_session_id(session_id).await else {
            return session_id;
        };
        if self
            .active_turns
            .active_connection_id(parent_session_id)
            .await
            .is_some()
        {
            parent_session_id
        } else {
            session_id
        }
    }

    async fn persist_command_prefix_rule(&self, prefix: &[String]) -> Result<(), String> {
        let policy_path = crate::exec_policy_store::default_user_rules_path()
            .map_err(|error| error.to_string())?;
        let prefix = prefix.to_vec();
        tokio::task::spawn_blocking(move || {
            devo_execpolicy::blocking_append_allow_prefix_rule(&policy_path, &prefix)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        *self
            .user_exec_policy
            .lock()
            .expect("user exec policy lock poisoned") =
            crate::exec_policy_store::load_user_exec_policy();
        Ok(())
    }

    async fn request_tool_approval(
        &self,
        session_id: SessionId,
        request: ToolPermissionRequest,
    ) -> Result<PermissionGrant, String> {
        let host_session_id = self.permission_host_session_id(session_id).await;
        let available_scopes = approval_scopes_for_request(&request);
        let connection_id = self
            .active_turns
            .active_connection_id(host_session_id)
            .await
            .or(self.active_turns.active_connection_id(session_id).await);
        let Some(connection_id) = connection_id else {
            return Err("no ACP client connection is available for permission request".to_string());
        };

        if host_session_id != session_id {
            tracing::debug!(
                child_session_id = %session_id,
                parent_session_id = %host_session_id,
                tool = %request.tool_name,
                "routing sub-agent permission request through parent session"
            );
        }

        let approval_id = request.tool_call_id.clone();
        let (tx, rx) = oneshot::channel();
        let pending = PendingApproval {
            owner_session_id: session_id,
            tool_name: request.tool_name.clone(),
            resource: Some(request.resource.clone()),
            path: request.path.clone(),
            host: request.host.clone(),
            command_prefix: request.command_prefix.clone(),
            command_pattern: request.command_pattern.clone(),
            requests_escalation: request.requests_escalation,
            command: devo_core::tools::command_str_for_permission_request(&request),
            cwd: request.cwd.clone(),
            sandbox_permissions: devo_core::tools::sandbox_permissions_from_input(&request.input),
            tx,
        };
        self.session_interactive
            .register_pending_approval(host_session_id, approval_id.clone(), pending)
            .await;

        let request_params =
            acp_request_permission_params(host_session_id, &request, &available_scopes);
        let cancel_token = self
            .active_turns
            .cancel_token_for_host_or_session(host_session_id, session_id)
            .await;
        let response = match self
            .send_request_to_connection_cancellable(
                connection_id,
                devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD,
                serde_json::to_value(request_params)
                    .expect("serialize ACP permission request params"),
                cancel_token,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.session_interactive
                    .remove_pending_approval(host_session_id, &approval_id)
                    .await;
                return Err(format!("permission request failed: {error}"));
            }
        };
        let response: devo_protocol::AcpRequestPermissionResponse =
            match serde_json::from_value(response) {
                Ok(response) => response,
                Err(error) => {
                    self.session_interactive
                        .remove_pending_approval(host_session_id, &approval_id)
                        .await;
                    return Err(format!(
                        "invalid session/request_permission response: {error}"
                    ));
                }
            };
        let (decision, scope) = match approval_decision_from_acp_outcome(response.outcome) {
            Ok(decision) => decision,
            Err(error) => {
                self.session_interactive
                    .remove_pending_approval(host_session_id, &approval_id)
                    .await;
                return Err(error);
            }
        };

        if let Some(pending) = self
            .session_interactive
            .remove_pending_approval(host_session_id, &approval_id)
            .await
        {
            let _ = pending.tx.send(decision.clone());
            if matches!(decision, ApprovalDecisionValue::Approve) {
                let (scope_tx, _) = oneshot::channel();
                let pending_for_scope = PendingApproval {
                    owner_session_id: pending.owner_session_id,
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
                    tx: scope_tx,
                };
                // ExecuteTurn owns the session mailbox, so ApplyApprovalScope cannot
                // run until the turn ends. Update live TurnInlineState here so the
                // same turn's later tool calls see PathPrefix/Session grants.
                self.apply_approval_scope_to_turn_inline(
                    host_session_id,
                    &scope,
                    &pending_for_scope,
                )
                .await;
                if let Some(session_handle) = self.session(host_session_id).await {
                    let prefix_to_persist = (scope == ApprovalScopeValue::CommandPrefixPersist)
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
        }

        complete_approval_wait(rx)
            .await
            .and_then(|decision| match decision {
                ApprovalDecisionValue::Approve => Ok(approved_permission_grant(&request)),
                ApprovalDecisionValue::Deny => Err("rejected by user".to_string()),
                ApprovalDecisionValue::Cancel => Err("cancelled by user".to_string()),
            })
    }
}

fn policy_decision(
    profile: &devo_safety::RuntimePermissionProfile,
    request: &ToolPermissionRequest,
    exec_policy: Option<&devo_execpolicy::Policy>,
) -> PolicyAuthorization {
    if profile.auto_approve {
        return PolicyAuthorization::Allow;
    }
    if request_forces_approval(request) {
        return PolicyAuthorization::Ask;
    }
    match request.resource {
        devo_safety::ResourceKind::Network => {
            if profile.allow_network {
                PolicyAuthorization::Allow
            } else {
                PolicyAuthorization::Ask
            }
        }
        devo_safety::ResourceKind::ShellExec => {
            shell_exec_policy_decision(profile, request, exec_policy)
        }
        devo_safety::ResourceKind::FileRead => {
            let Some(path) = request.path.as_ref() else {
                return PolicyAuthorization::Ask;
            };
            if path_matches_any_prefix(path, &profile.readable_roots)
                || path_matches_any_prefix(path, &profile.writable_roots)
            {
                PolicyAuthorization::Allow
            } else {
                PolicyAuthorization::Ask
            }
        }
        devo_safety::ResourceKind::FileWrite => {
            let Some(path) = request.path.as_ref() else {
                return PolicyAuthorization::Ask;
            };
            if path_matches_any_prefix(path, &profile.writable_roots) {
                PolicyAuthorization::Allow
            } else {
                PolicyAuthorization::Ask
            }
        }
        devo_safety::ResourceKind::Custom(_) => PolicyAuthorization::Allow,
    }
}

fn shell_exec_policy_decision(
    profile: &devo_safety::RuntimePermissionProfile,
    request: &ToolPermissionRequest,
    exec_policy: Option<&devo_execpolicy::Policy>,
) -> PolicyAuthorization {
    use devo_execpolicy::Decision;
    use devo_util_shell_command::is_dangerous_command::command_might_be_dangerous;

    if !profile.allow_shell_commands {
        return PolicyAuthorization::Ask;
    }
    let command = shell_command_for_policy(request);
    if command.is_empty() {
        return PolicyAuthorization::Ask;
    }
    // Fail closed on multi-line / control-separated commands even when a
    // pre-parsed argv is present (shlex would otherwise collapse newlines).
    if command
        .as_bytes()
        .iter()
        .any(|b| matches!(b, b'\n' | b'\r' | 0x0b | 0x0c))
    {
        return PolicyAuthorization::Ask;
    }
    // Background `&` (not `&&`) splits jobs; argv-based checks miss the
    // trailing command, so fail closed like newlines.
    if command_contains_standalone_ampersand(&command) {
        return PolicyAuthorization::Ask;
    }
    let argv = shell_argv_for_policy(request, &command);

    if let (Some(policy), Some(argv)) = (exec_policy, argv.as_ref())
        && let Some(decision) =
            crate::exec_policy_store::exec_policy_decision_for_argv(policy, argv)
    {
        return match decision {
            Decision::Allow => PolicyAuthorization::Allow,
            Decision::Forbidden => {
                PolicyAuthorization::Deny("command blocked by user exec policy rules".to_string())
            }
            Decision::Prompt => PolicyAuthorization::Ask,
        };
    }

    if argv
        .as_ref()
        .is_some_and(|argv| command_might_be_dangerous(argv))
    {
        return PolicyAuthorization::Ask;
    }

    match devo_safety::evaluate_shell_command_for_profile(profile, &command, &request.cwd) {
        // NoMatch means the analyzer found no out-of-policy file access (or the
        // command was ambiguous without a definite file touch). Allow and run
        // under the session sandbox. Ask/Deny still require user approval.
        devo_safety::permission::PolicyDecision::NoMatch
        | devo_safety::permission::PolicyDecision::Allow => PolicyAuthorization::Allow,
        devo_safety::permission::PolicyDecision::Ask
        | devo_safety::permission::PolicyDecision::Deny { .. } => PolicyAuthorization::Ask,
    }
}

fn shell_command_for_policy(request: &ToolPermissionRequest) -> String {
    request
        .target
        .clone()
        .or_else(|| devo_core::tools::command_str_for_permission_request(request))
        .unwrap_or_default()
}

fn shell_argv_for_policy(request: &ToolPermissionRequest, command: &str) -> Option<Vec<String>> {
    request
        .command_argv
        .clone()
        .or_else(|| parse_safe_shell_argv(command))
}

fn parse_safe_shell_argv(command: &str) -> Option<Vec<String>> {
    if command
        .as_bytes()
        .iter()
        .any(|b| matches!(b, b'\n' | b'\r' | 0x0b | 0x0c))
    {
        return None;
    }
    let argv = shlex::split(command)?;
    if argv.iter().any(|token| {
        token.contains(['|', ';', '>', '<', '*', '?', '$', '(', ')'])
            || token.contains("$(")
            || command.contains("&&")
            || command.contains("||")
            || command.contains("$(")
            || command.contains('`')
            || command_contains_standalone_ampersand(command)
    }) {
        return None;
    }
    if argv.first().is_some_and(|token| {
        token.split_once('=').is_some_and(|(name, value)| {
            !name.is_empty()
                && !value.is_empty()
                && name
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                && name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        })
    }) {
        return None;
    }
    Some(argv)
}

/// True when `command` has a background `&` (not `&&`).
///
/// Uses shlex tokenization so `&` inside quoted strings (e.g. URL query
/// strings) does not count. Also treats a token that *ends* with `&`
/// (`sleep 1& rm x` → `1&`) as a background operator.
fn command_contains_standalone_ampersand(command: &str) -> bool {
    shlex::split(command).is_some_and(|argv| {
        argv.iter()
            .any(|token| token_is_background_ampersand(token))
    })
}

fn token_is_background_ampersand(token: &str) -> bool {
    token != "&&" && (token == "&" || token.ends_with('&'))
}

fn approval_scopes_for_request(request: &ToolPermissionRequest) -> Vec<String> {
    let mut scopes = vec!["once".to_string(), "turn".to_string()];
    // Shell commands get a session grant when an exact command string is
    // Prefer exact command + cwd cache when available; generalized patterns are a fallback.
    let session_available = match request.tool_name.as_str() {
        "bash" | "shell_command" | "exec_command" => {
            devo_core::tools::command_str_for_permission_request(request).is_some()
                || request.command_pattern.is_some()
        }
        _ => true,
    };
    if session_available {
        scopes.push("session".to_string());
    }
    if request.path.is_some() {
        scopes.push("path_prefix".to_string());
    }
    if request.host.is_some() {
        scopes.push("host".to_string());
    }
    if let Some(prefix) = request.command_prefix.as_ref() {
        scopes.push("command_prefix".to_string());
        if !devo_core::tools::is_banned_prefix_suggestion(prefix) {
            scopes.push("command_prefix_persist".to_string());
        }
    }
    scopes.push("tool".to_string());
    scopes
}

fn acp_request_permission_params(
    session_id: SessionId,
    request: &ToolPermissionRequest,
    available_scopes: &[String],
) -> devo_protocol::AcpRequestPermissionParams {
    devo_protocol::AcpRequestPermissionParams {
        session_id,
        tool_call: devo_protocol::AcpToolCallUpdate {
            tool_call_id: request.tool_call_id.clone(),
            title: Some(request.action_summary.clone()),
            kind: Some(acp_tool_kind_for_permission_request(request)),
            status: Some(devo_protocol::AcpToolCallStatus::Pending),
            raw_input: Some(request.input.clone()),
            raw_output: None,
            content: Vec::new(),
            locations: request
                .path
                .as_ref()
                .map(|path| {
                    vec![devo_protocol::AcpToolCallLocation {
                        path: path.clone(),
                        line: None,
                    }]
                })
                .unwrap_or_default(),
            meta: None,
        },
        options: acp_permission_options_for_scopes(
            available_scopes,
            request.command_pattern.as_deref(),
            devo_core::tools::command_str_for_permission_request(request).as_deref(),
            request.command_prefix.as_deref(),
            request.path.as_ref(),
            request.host.as_deref(),
        ),
        meta: {
            let mut meta = serde_json::Map::new();
            if let Some(pattern) = request.command_pattern.as_ref() {
                meta.insert(
                    "commandPattern".to_string(),
                    serde_json::Value::from(pattern.clone()),
                );
            }
            if let Some(prefix) = request.command_prefix.as_ref() {
                meta.insert(
                    "commandPrefix".to_string(),
                    serde_json::Value::from(prefix.clone()),
                );
            }
            if let Some(target) = devo_core::tools::command_str_for_permission_request(request) {
                meta.insert("target".to_string(), serde_json::Value::String(target));
            }
            if let Some(justification) = request.justification.as_ref() {
                meta.insert(
                    "justification".to_string(),
                    serde_json::Value::String(justification.clone()),
                );
            }
            meta.insert(
                "resource".to_string(),
                serde_json::Value::String(format!("{:?}", request.resource)),
            );
            if let Some(path) = request.path.as_ref() {
                meta.insert(
                    "path".to_string(),
                    serde_json::Value::String(path.display().to_string()),
                );
            }
            if let Some(host) = request.host.as_ref() {
                meta.insert("host".to_string(), serde_json::Value::String(host.clone()));
            }
            (!meta.is_empty()).then_some(meta)
        },
    }
}

fn acp_permission_options_for_scopes(
    scopes: &[String],
    command_pattern: Option<&[String]>,
    exact_command: Option<&str>,
    command_prefix: Option<&[String]>,
    path: Option<&std::path::PathBuf>,
    host: Option<&str>,
) -> Vec<devo_protocol::AcpPermissionOption> {
    let mut options = vec![devo_protocol::AcpPermissionOption {
        option_id: "allow_once".to_string(),
        name: "Yes, proceed".to_string(),
        kind: devo_protocol::AcpPermissionOptionKind::AllowOnce,
        meta: None,
    }];
    if scopes.iter().any(|scope| scope == "session") {
        let name = exact_command
            .map(|command| format!("Yes, and don't ask again for `{command}` in this session"))
            .or_else(|| {
                command_pattern.map(|pattern| {
                    format!(
                        "Yes, and don't ask again for `{}` in this session",
                        pattern.join(" ")
                    )
                })
            })
            .unwrap_or_else(|| {
                "Yes, and don't ask again for this command in this session".to_string()
            });
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_session".to_string(),
            name,
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes.iter().any(|scope| scope == "command_prefix_persist")
        && let Some(prefix) = command_prefix
    {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_prefix_rule".to_string(),
            name: format!(
                "Yes, and don't ask again for commands that start with `{}`",
                prefix.join(" ")
            ),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes.iter().any(|scope| scope == "path_prefix")
        && let Some(path) = path
    {
        let root = if path.is_dir() {
            path.display().to_string()
        } else {
            path.parent().unwrap_or(path).display().to_string()
        };
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_path_prefix".to_string(),
            name: format!("Yes, and don't ask again for files under `{root}`"),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes.iter().any(|scope| scope == "host")
        && let Some(host) = host
    {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_host".to_string(),
            name: format!("Yes, and allow `{host}` for this session"),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    options.push(devo_protocol::AcpPermissionOption {
        option_id: "reject_once".to_string(),
        name: "No, continue without running it".to_string(),
        kind: devo_protocol::AcpPermissionOptionKind::RejectOnce,
        meta: None,
    });
    options
}

fn acp_tool_kind_for_permission_request(
    request: &ToolPermissionRequest,
) -> devo_protocol::AcpToolKind {
    match request.resource {
        devo_safety::ResourceKind::FileRead => devo_protocol::AcpToolKind::Read,
        devo_safety::ResourceKind::FileWrite => devo_protocol::AcpToolKind::Edit,
        devo_safety::ResourceKind::ShellExec => devo_protocol::AcpToolKind::Execute,
        devo_safety::ResourceKind::Network => devo_protocol::AcpToolKind::Fetch,
        devo_safety::ResourceKind::Custom(_) => devo_protocol::AcpToolKind::Other,
    }
}

fn approval_decision_from_acp_outcome(
    outcome: devo_protocol::AcpPermissionOutcome,
) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
    match outcome {
        devo_protocol::AcpPermissionOutcome::Selected { option_id } => match option_id.as_str() {
            "allow_once" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Once)),
            "allow_session" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Session)),
            "allow_prefix_rule" => Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::CommandPrefixPersist,
            )),
            "allow_path_prefix" => Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::PathPrefix,
            )),
            "allow_host" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Host)),
            "reject_once" => Ok((ApprovalDecisionValue::Deny, ApprovalScopeValue::Once)),
            _ => Err(format!("unknown permission option selected: {option_id}")),
        },
        devo_protocol::AcpPermissionOutcome::Cancelled => {
            Ok((ApprovalDecisionValue::Cancel, ApprovalScopeValue::Once))
        }
    }
}

fn cache_grant(
    cache: &crate::execution::ApprovalGrantCache,
    request: &ToolPermissionRequest,
) -> Option<PermissionGrant> {
    if let Some(key) = sandbox_bypass_key_from_request(request)
        && cache.sandbox_bypass_commands.contains(&key)
    {
        return Some(PermissionGrant::from_approval(/*bypass_sandbox*/ true));
    }
    permission_cache_matches(cache, request)
        .then(|| PermissionGrant::from_approval(/*bypass_sandbox*/ false))
}

fn permission_cache_matches(
    cache: &crate::execution::ApprovalGrantCache,
    request: &ToolPermissionRequest,
) -> bool {
    if cache.tools.contains(&request.tool_name) {
        return true;
    }
    if request
        .host
        .as_ref()
        .is_some_and(|host| cache.hosts.contains(host))
    {
        return true;
    }
    if let Some(command) = devo_core::tools::command_str_for_permission_request(request)
        && cache
            .exact_commands
            .contains(&(command, request.cwd.clone()))
    {
        return true;
    }
    request.path.as_ref().is_some_and(|path| {
        let prefixes = match request.resource {
            devo_safety::ResourceKind::FileWrite => &cache.write_path_prefixes,
            _ => &cache.read_path_prefixes,
        };
        path_matches_any_prefix(path, prefixes)
    }) || request.command_prefix.as_ref().is_some_and(|command| {
        cache
            .command_prefixes
            .iter()
            .any(|prefix| command.starts_with(prefix))
    }) || request.command_argv.as_ref().is_some_and(|argv| {
        cache
            .command_patterns
            .iter()
            .any(|pattern| devo_core::tools::command_pattern_matches(pattern, argv))
    })
}

fn request_forces_approval(request: &ToolPermissionRequest) -> bool {
    request.requests_escalation
}

fn escalation_permission_grant(request: &ToolPermissionRequest) -> PermissionGrant {
    PermissionGrant {
        bypass_sandbox: request.requests_escalation,
        already_approved: false,
    }
}

fn approved_permission_grant(request: &ToolPermissionRequest) -> PermissionGrant {
    PermissionGrant::from_approval(request.requests_escalation)
}

fn sandbox_bypass_key_from_request(
    request: &ToolPermissionRequest,
) -> Option<crate::execution::SandboxBypassKey> {
    let command = devo_core::tools::command_str_for_permission_request(request)?;
    Some(crate::execution::SandboxBypassKey {
        command,
        cwd: request.cwd.clone(),
        sandbox_permissions: devo_core::tools::sandbox_permissions_from_input(&request.input),
    })
}

fn path_matches_any_prefix<'a, I>(path: &Path, prefixes: I) -> bool
where
    I: IntoIterator<Item = &'a PathBuf>,
{
    let path = normalize_permission_path(path);
    prefixes
        .into_iter()
        .any(|prefix| path.starts_with(normalize_permission_path(prefix)))
}

fn normalize_permission_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn permission_tool_extra(
    request: &ToolPermissionRequest,
) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([
        (
            "tool_name".to_string(),
            serde_json::Value::String(request.tool_name.clone()),
        ),
        ("tool_input".to_string(), request.input.clone()),
        (
            "tool_use_id".to_string(),
            serde_json::Value::String(request.tool_call_id.clone()),
        ),
    ])
}

fn permission_mode_authorization(
    mode: PermissionMode,
    request: &ToolPermissionRequest,
) -> Option<Result<PermissionGrant, String>> {
    match mode {
        PermissionMode::AutoApprove => Some(Ok(escalation_permission_grant(request))),
        PermissionMode::Deny => Some(Err("approval policy is deny".to_string())),
        PermissionMode::Interactive => None,
    }
}

fn push_recent_approval_decisions(
    scope: &str,
    cache: &crate::execution::ApprovalGrantCache,
    recent_decisions: &mut Vec<String>,
) {
    for tool in &cache.tools {
        recent_decisions.push(format!("{scope} allow tool: {tool}"));
    }
    for host in &cache.hosts {
        recent_decisions.push(format!("{scope} allow host: {host}"));
    }
    for path in &cache.read_path_prefixes {
        recent_decisions.push(format!("{scope} allow read path: {}", path.display()));
    }
    for path in &cache.write_path_prefixes {
        recent_decisions.push(format!("{scope} allow write path: {}", path.display()));
    }
    for prefix in &cache.command_prefixes {
        recent_decisions.push(format!("{scope} allow command: {}", prefix.join(" ")));
    }
    for pattern in &cache.command_patterns {
        recent_decisions.push(format!(
            "{scope} allow command pattern: {}",
            pattern.join(" ")
        ));
    }
    for key in &cache.sandbox_bypass_commands {
        recent_decisions.push(format!(
            "{scope} allow unsandboxed command: {} ({})",
            key.command, key.sandbox_permissions
        ));
    }
}

fn format_persisted_turn_item(item: &crate::execution::PersistedTurnItem) -> String {
    match &item.turn_item {
        devo_core::TurnItem::UserMessage(text) => format!("user: {}", text.text),
        devo_core::TurnItem::SteerInput(text) => format!("steer: {}", text.text),
        devo_core::TurnItem::AgentMessage(text) => format!("assistant: {}", text.text),
        devo_core::TurnItem::Plan(text) => format!("plan: {}", text.text),
        devo_core::TurnItem::Reasoning(text) => format!("reasoning: {}", text.text),
        devo_core::TurnItem::ToolCall(tool_call) => {
            format!("tool_call {}: {}", tool_call.tool_name, tool_call.input)
        }
        devo_core::TurnItem::ToolResult(result) => {
            let name = result.tool_name.as_deref().unwrap_or("unknown");
            format!("tool_result {}: {}", name, result.output)
        }
        devo_core::TurnItem::CommandExecution(exec) => {
            format!("command: {}", exec.command)
        }
        devo_core::TurnItem::ApprovalRequest(request) => {
            format!("approval_request: {}", request.action_summary)
        }
        devo_core::TurnItem::ApprovalDecision(decision) => {
            format!(
                "approval_decision: {} ({})",
                decision.decision, decision.scope
            )
        }
        devo_core::TurnItem::HookPrompt(text) => format!("hook: {}", text.text),
        devo_core::TurnItem::WebSearch(text) => format!("web_search: {}", text.text),
        devo_core::TurnItem::ImageGeneration(text) => format!("image: {}", text.text),
        devo_core::TurnItem::ContextCompaction(text) => format!("compaction: {}", text.text),
        devo_core::TurnItem::TurnSummary(text) => format!("turn_summary: {}", text.text),
        devo_core::TurnItem::ToolProgress(progress) => {
            format!("tool_progress: {}", progress.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn approval_policy_strings_map_to_permission_modes() {
        assert_eq!(
            permission_mode_from_approval_policy("on-request"),
            Some(PermissionMode::Interactive)
        );
        assert_eq!(
            permission_mode_from_approval_policy("never"),
            Some(PermissionMode::AutoApprove)
        );
        assert_eq!(
            permission_mode_from_approval_policy("deny"),
            Some(PermissionMode::Deny)
        );
        assert_eq!(permission_mode_from_approval_policy("unknown"), None);
    }

    #[test]
    fn command_prefix_cache_allows_matching_command_prefix() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        cache
            .command_prefixes
            .insert(vec!["git".to_string(), "add".to_string()]);
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(permission_cache_matches(&cache, &request));
    }

    #[test]
    fn approval_scopes_include_command_prefix_for_shell_commands() {
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(
            approval_scopes_for_request(&request)
                .iter()
                .any(|scope| scope == "command_prefix")
        );
    }

    #[test]
    fn approval_scopes_gate_session_scope_on_command_pattern_for_shell_tools() {
        let mut shell_request = test_permission_request("shell_command");
        assert!(
            !approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session"),
            "shell command without a safe pattern must not offer session scope"
        );

        shell_request.target = Some("git status".to_string());
        assert!(
            approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session"),
            "shell command with an exact command string should offer session scope"
        );

        shell_request.command_pattern =
            Some(vec!["git".to_string(), "add".to_string(), "*".to_string()]);
        assert!(
            approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session")
        );

        let exec_request = test_permission_request("exec_command");
        assert!(
            !approval_scopes_for_request(&exec_request)
                .iter()
                .any(|scope| scope == "session")
        );

        let file_request = test_permission_request("write");
        assert!(
            approval_scopes_for_request(&file_request)
                .iter()
                .any(|scope| scope == "session"),
            "non-shell tools keep the session scope"
        );
    }

    #[test]
    fn acp_permission_options_label_session_scope_with_command_pattern() {
        let scopes = vec!["once".to_string(), "session".to_string()];
        let pattern = vec!["git".to_string(), "add".to_string(), "*".to_string()];

        let options =
            acp_permission_options_for_scopes(&scopes, Some(&pattern), None, None, None, None);
        let session_option = options
            .iter()
            .find(|option| option.option_id == "allow_session")
            .expect("session option");
        assert_eq!(
            session_option.name,
            "Yes, and don't ask again for `git add *` in this session"
        );

        let options = acp_permission_options_for_scopes(
            &scopes,
            Some(&pattern),
            Some("git add file.txt"),
            None,
            None,
            None,
        );
        let session_option = options
            .iter()
            .find(|option| option.option_id == "allow_session")
            .expect("session option");
        assert_eq!(
            session_option.name,
            "Yes, and don't ask again for `git add file.txt` in this session"
        );

        let options = acp_permission_options_for_scopes(
            &scopes, /*command_pattern*/ None, None, None, None, None,
        );
        let session_option = options
            .iter()
            .find(|option| option.option_id == "allow_session")
            .expect("session option");
        assert_eq!(
            session_option.name,
            "Yes, and don't ask again for this command in this session"
        );
    }

    #[test]
    fn command_pattern_cache_allows_matching_argv() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        cache
            .command_patterns
            .insert(vec!["git".to_string(), "add".to_string(), "*".to_string()]);

        let mut request = test_permission_request("shell_command");
        request.command_argv = Some(vec![
            "git".to_string(),
            "add".to_string(),
            "src/main.rs".to_string(),
        ]);
        assert!(permission_cache_matches(&cache, &request));

        request.command_argv = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(
            !permission_cache_matches(&cache, &request),
            "trailing wildcard requires at least one argument"
        );

        request.command_argv = Some(vec![
            "git".to_string(),
            "commit".to_string(),
            "src/main.rs".to_string(),
        ]);
        assert!(!permission_cache_matches(&cache, &request));

        request.command_argv = None;
        assert!(
            !permission_cache_matches(&cache, &request),
            "unsafe commands without screened argv never match patterns"
        );
    }

    #[test]
    fn explicit_escalation_forces_approval() {
        let mut request = test_permission_request("exec_command");
        request.requests_escalation = true;

        assert!(request_forces_approval(&request));
    }

    #[test]
    fn permission_mode_overrides_authorization_policy() {
        let request = test_permission_request("shell_command");
        assert_eq!(
            permission_mode_authorization(PermissionMode::AutoApprove, &request),
            Some(Ok(PermissionGrant::default()))
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Deny, &request),
            Some(Err("approval policy is deny".to_string()))
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Interactive, &request),
            None
        );
    }

    #[test]
    fn auto_approve_grants_sandbox_bypass_for_escalation() {
        let mut request = test_permission_request("shell_command");
        request.requests_escalation = true;
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });

        assert_eq!(
            permission_mode_authorization(PermissionMode::AutoApprove, &request),
            Some(Ok(PermissionGrant {
                bypass_sandbox: true,
                already_approved: false,
            }))
        );
    }

    #[test]
    fn auto_approve_profile_grants_sandbox_bypass_for_escalation() {
        let root = abs_path(&["workspace"]);
        let mut profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        profile.auto_approve = true;
        let mut request = test_permission_request("shell_command");
        request.requests_escalation = true;
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });
        request.target = Some("npm install".to_string());
        request.cwd = root;

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
        assert_eq!(
            escalation_permission_grant(&request),
            PermissionGrant {
                bypass_sandbox: true,
                already_approved: false,
            }
        );
    }

    #[test]
    fn approval_scopes_include_command_prefix_persist_when_not_banned() {
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "pull".to_string()]);
        let scopes = approval_scopes_for_request(&request);
        assert!(scopes.iter().any(|scope| scope == "command_prefix"));
        assert!(scopes.iter().any(|scope| scope == "command_prefix_persist"));

        request.command_prefix = Some(vec!["git".to_string()]);
        let scopes = approval_scopes_for_request(&request);
        assert!(scopes.iter().any(|scope| scope == "command_prefix"));
        assert!(
            !scopes.iter().any(|scope| scope == "command_prefix_persist"),
            "banned bare git prefix must not offer persist scope"
        );
    }

    #[test]
    fn acp_permission_options_include_allow_prefix_rule() {
        let scopes = vec!["once".to_string(), "command_prefix_persist".to_string()];
        let prefix = vec!["git".to_string(), "pull".to_string()];
        let options = acp_permission_options_for_scopes(
            &scopes,
            /*command_pattern*/ None,
            /*exact_command*/ None,
            Some(&prefix),
            None,
            None,
        );
        let persist_option = options
            .iter()
            .find(|option| option.option_id == "allow_prefix_rule")
            .expect("persist option");
        assert_eq!(
            persist_option.name,
            "Yes, and don't ask again for commands that start with `git pull`"
        );
        assert_eq!(
            persist_option.kind,
            devo_protocol::AcpPermissionOptionKind::AllowAlways
        );
    }

    #[test]
    fn acp_request_permission_params_include_target_and_resource_meta() {
        let mut request = test_permission_request("shell_command");
        request.target = Some(r#"echo "Hello from Devo!" > ~/Desktop/hello-devo.txt"#.to_string());
        request.justification = Some("Write a greeting file on the desktop.".to_string());
        request.action_summary = format!("shell_command: {}", request.target.as_ref().unwrap());

        let params = acp_request_permission_params(
            SessionId::new(),
            &request,
            &["once".to_string(), "session".to_string()],
        );
        let meta = params.meta.expect("permission meta");
        assert_eq!(
            meta.get("target").and_then(serde_json::Value::as_str),
            Some(r#"echo "Hello from Devo!" > ~/Desktop/hello-devo.txt"#)
        );
        assert_eq!(
            meta.get("resource").and_then(serde_json::Value::as_str),
            Some("ShellExec")
        );
        assert_eq!(
            meta.get("justification")
                .and_then(serde_json::Value::as_str),
            Some("Write a greeting file on the desktop.")
        );
    }

    #[test]
    fn allow_prefix_rule_maps_to_command_prefix_persist_scope() {
        let outcome = devo_protocol::AcpPermissionOutcome::Selected {
            option_id: "allow_prefix_rule".to_string(),
        };
        assert_eq!(
            approval_decision_from_acp_outcome(outcome),
            Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::CommandPrefixPersist
            ))
        );
    }

    #[test]
    fn blocking_append_allow_prefix_rule_writes_rule_line() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let policy_path = tmp.path().join("rules").join("default.rules");
        let prefix = vec!["git".to_string(), "pull".to_string()];

        devo_execpolicy::blocking_append_allow_prefix_rule(&policy_path, &prefix)
            .expect("append rule");

        let contents = std::fs::read_to_string(&policy_path).expect("read rules");
        assert_eq!(
            contents,
            r#"prefix_rule(pattern=["git", "pull"], decision="allow")
"#
        );
    }

    #[test]
    fn path_prefix_match_normalizes_parent_components() {
        let root = abs_path(&["workspace"]);
        let inside = root.join("src").join("..").join("Cargo.toml");
        let outside = root.join("src").join("..").join("..").join("outside.txt");

        assert!(path_matches_any_prefix(&inside, [&root]));
        assert!(!path_matches_any_prefix(&outside, [&root]));
    }

    #[test]
    fn approval_path_cache_does_not_allow_parent_escape() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let root = abs_path(&["workspace", "generated"]);
        cache.write_path_prefixes.insert(root.clone());

        let mut escaped = test_permission_request("write");
        escaped.resource = devo_safety::ResourceKind::FileWrite;
        escaped.path = Some(root.join("..").join("outside.txt"));

        let mut allowed = test_permission_request("write");
        allowed.resource = devo_safety::ResourceKind::FileWrite;
        allowed.path = Some(root.join("..").join("generated").join("file.txt"));

        assert!(!permission_cache_matches(&cache, &escaped));
        assert!(permission_cache_matches(&cache, &allowed));

        let mut read_only = test_permission_request("read");
        read_only.resource = devo_safety::ResourceKind::FileRead;
        read_only.path = Some(root.join("file.txt"));
        assert!(
            !permission_cache_matches(&cache, &read_only),
            "write path grants must not auto-allow reads via write cache"
        );
    }

    #[test]
    fn policy_allows_file_read_inside_readable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("read");
        request.resource = devo_safety::ResourceKind::FileRead;
        request.path = Some(root.join("Cargo.toml"));

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
    }

    #[test]
    fn policy_asks_for_file_read_outside_readable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root,
        );
        let mut request = test_permission_request("read");
        request.resource = devo_safety::ResourceKind::FileRead;
        request.path = Some(abs_path(&["outside", "secret.txt"]));

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Ask
        ));
    }

    #[test]
    fn policy_asks_for_shell_redirect_outside_writable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("shell_command");
        request.target = Some(format!(
            "cat > {}/outside.txt",
            abs_path(&["etc"]).display()
        ));
        request.input = serde_json::json!({ "command": request.target });

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Ask
        ));
    }

    #[test]
    fn policy_allows_shell_redirect_inside_writable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("shell_command");
        request.target = Some(format!("cat > {}/file.txt", root.display()));
        request.input = serde_json::json!({ "command": request.target });
        request.cwd = root;

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
    }

    #[test]
    fn policy_allows_shell_command_without_file_access() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("shell_command");
        request.target = Some("git status".to_string());
        request.input = serde_json::json!({ "command": "git status" });
        request.cwd = root;

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
    }

    #[test]
    fn policy_asks_for_dangerous_shell_command() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("shell_command");
        request.target = Some("rm -f important.txt".to_string());
        request.input = serde_json::json!({ "command": "rm -f important.txt" });
        request.cwd = root;

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Ask
        ));
    }

    #[test]
    fn policy_asks_for_shell_background_ampersand() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        for command in ["sleep 1 & touch evil", "sleep 1& rm x"] {
            let mut request = test_permission_request("shell_command");
            // Background `&` (spaced or attached) must not pass safe-prefix Allow.
            request.target = Some(command.to_string());
            request.input = serde_json::json!({ "command": command });
            request.cwd = root.clone();

            assert!(
                matches!(
                    test_policy_decision(&profile, &request),
                    PolicyAuthorization::Ask
                ),
                "expected Ask for {command}"
            );
        }
    }

    #[test]
    fn policy_allows_quoted_ampersand_in_url_query() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        let mut request = test_permission_request("shell_command");
        // `&` inside quotes is part of the URL, not a background job.
        request.target = Some(r#"echo "http://x/?a=1&b=2""#.to_string());
        request.input = serde_json::json!({ "command": r#"echo "http://x/?a=1&b=2""# });
        request.cwd = root;

        assert!(matches!(
            test_policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
    }

    #[test]
    fn sandbox_bypass_cache_grants_unsandboxed_execution() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let mut request = test_permission_request("shell_command");
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });
        request.target = Some("npm install".to_string());
        request.requests_escalation = true;
        let key = sandbox_bypass_key_from_request(&request).expect("bypass key");
        cache.sandbox_bypass_commands.insert(key);

        assert_eq!(
            cache_grant(&cache, &request),
            Some(PermissionGrant {
                bypass_sandbox: true,
                already_approved: true,
            })
        );
    }

    #[test]
    fn sandbox_bypass_cache_requires_exact_command_and_permissions() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let mut request = test_permission_request("shell_command");
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });
        request.target = Some("npm install".to_string());
        request.requests_escalation = true;
        let key = sandbox_bypass_key_from_request(&request).expect("bypass key");
        cache.sandbox_bypass_commands.insert(key);

        request.target = Some("npm ci".to_string());
        assert_eq!(cache_grant(&cache, &request), None);
    }

    fn test_policy_decision(
        profile: &devo_safety::RuntimePermissionProfile,
        request: &ToolPermissionRequest,
    ) -> PolicyAuthorization {
        policy_decision(profile, request, /*exec_policy*/ None)
    }

    fn test_permission_request(tool_name: &str) -> ToolPermissionRequest {
        ToolPermissionRequest {
            tool_call_id: "call".into(),
            tool_name: tool_name.into(),
            input: serde_json::json!({}),
            cwd: std::path::PathBuf::new(),
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            resource: devo_safety::ResourceKind::ShellExec,
            action_summary: tool_name.into(),
            justification: None,
            path: None,
            host: None,
            target: None,
            command_prefix: None,
            command_argv: None,
            command_pattern: None,
            requests_escalation: false,
        }
    }

    fn abs_path(parts: &[&str]) -> PathBuf {
        #[cfg(windows)]
        let mut path = PathBuf::from(r"C:\");
        #[cfg(unix)]
        let mut path = PathBuf::from("/");

        for part in parts {
            path.push(part);
        }
        path
    }
}
