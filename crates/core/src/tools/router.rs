use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use devo_config::ResolvedLocalWebSearchConfig;
use devo_safety::ResourceKind;
use devo_tools::contracts::ToolBudgets;
use devo_tools::contracts::ToolProgress;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::info;
use tracing::warn;

use crate::invocation::ToolContent;
use crate::registry::ToolRegistry;
use crate::tool_spec::ToolCapabilityTag;
use crate::tools::deferred_loading::is_subagent_agent_coordination_tool;
use devo_tools::AgentToolCoordinator;
use devo_tools::ClientFilesystem;
use devo_tools::FileReadLedger;
use devo_tools::ToolAgentScope;
use tokio_util::sync::CancellationToken;

type ProgressCallback = dyn Fn(String, ToolProgress) -> BoxFuture<'static, ()> + Send + Sync;
type ProgressCallbackArc = Arc<ProgressCallback>;
type CompletionCallback = dyn Fn(ToolCallResult) -> BoxFuture<'static, ()> + Send + Sync;
type CompletionCallbackArc = Arc<CompletionCallback>;
type ExecutionStartCallback = dyn Fn(ToolCall) -> BoxFuture<'static, ()> + Send + Sync;
type ExecutionStartCallbackArc = Arc<ExecutionStartCallback>;
type PermissionFuture = futures::future::BoxFuture<'static, Result<PermissionGrant, String>>;
type PermissionCheckFn = dyn Fn(ToolPermissionRequest) -> PermissionFuture + Send + Sync;
const PROGRESS_DRAIN_GRACE_MS: u64 = 50;
/// Content written into a tool-result item when a tool is cancelled mid-execution.
pub const INTERRUPTED_TOOL_RESULT_MESSAGE: &str = "tool execution was interrupted";

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub content: ToolContent,
    pub is_error: bool,
    pub display_content: Option<String>,
}

impl ToolCallResult {
    pub fn success(tool_use_id: &str, content: ToolContent) -> Self {
        ToolCallResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: false,
            display_content: None,
        }
    }

    pub fn error(tool_use_id: &str, message: &str) -> Self {
        ToolCallResult {
            tool_use_id: tool_use_id.to_string(),
            content: ToolContent::Text(message.to_string()),
            is_error: true,
            display_content: None,
        }
    }
}

pub struct ToolRuntime {
    registry: Arc<ToolRegistry>,
    permission: PermissionChecker,
    gate: RwLock<()>,
    context: ToolRuntimeContext,
    execution_options: ToolExecutionOptions,
}

impl ToolRuntime {
    pub fn new(registry: Arc<ToolRegistry>, permission: PermissionChecker) -> Self {
        ToolRuntime {
            registry,
            permission,
            gate: RwLock::new(()),
            context: ToolRuntimeContext::default(),
            execution_options: ToolExecutionOptions::default(),
        }
    }

    pub fn new_with_context(
        registry: Arc<ToolRegistry>,
        permission: PermissionChecker,
        context: ToolRuntimeContext,
    ) -> Self {
        ToolRuntime {
            registry,
            permission,
            gate: RwLock::new(()),
            context,
            execution_options: ToolExecutionOptions::default(),
        }
    }

    pub fn new_with_context_and_options(
        registry: Arc<ToolRegistry>,
        permission: PermissionChecker,
        context: ToolRuntimeContext,
        execution_options: ToolExecutionOptions,
    ) -> Self {
        ToolRuntime {
            registry,
            permission,
            gate: RwLock::new(()),
            context,
            execution_options,
        }
    }

    pub fn new_without_permissions(registry: Arc<ToolRegistry>) -> Self {
        ToolRuntime {
            registry,
            permission: PermissionChecker::always_allow(),
            gate: RwLock::new(()),
            context: ToolRuntimeContext::default(),
            execution_options: ToolExecutionOptions::default(),
        }
    }

    pub fn cancel_token(&self) -> &CancellationToken {
        &self.execution_options.cancel_token
    }

    pub async fn execute_batch(&self, calls: &[ToolCall]) -> Vec<ToolCallResult> {
        self.execute_batch_inner(
            calls, /*on_progress*/ None, /*on_completion*/ None,
        )
        .await
    }

    pub async fn execute_batch_streaming(
        &self,
        calls: &[ToolCall],
        on_progress: impl Fn(String, ToolProgress) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Vec<ToolCallResult> {
        self.execute_batch_inner(
            calls,
            Some(Box::new(on_progress)),
            /*on_completion*/ None,
        )
        .await
    }

    pub async fn execute_batch_streaming_with_completion(
        &self,
        calls: &[ToolCall],
        on_progress: impl Fn(String, ToolProgress) -> BoxFuture<'static, ()> + Send + Sync + 'static,
        on_completion: impl Fn(ToolCallResult) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Vec<ToolCallResult> {
        self.execute_batch_inner(
            calls,
            Some(Box::new(on_progress)),
            Some(Box::new(on_completion)),
        )
        .await
    }

    async fn execute_batch_inner(
        &self,
        calls: &[ToolCall],
        on_progress: Option<Box<ProgressCallback>>,
        on_completion: Option<Box<CompletionCallback>>,
    ) -> Vec<ToolCallResult> {
        // Wrap the Box in an Arc so it can be shared across spawned tasks
        let on_progress: Option<ProgressCallbackArc> = on_progress.map(Arc::from);
        let on_completion: Option<CompletionCallbackArc> = on_completion.map(Arc::from);

        let mut indexed_results = Vec::with_capacity(calls.len());

        let (parallel, exclusive): (Vec<_>, Vec<_>) =
            calls.iter().enumerate().partition(|(_, call)| {
                let tool_name = canonical_tool_name(&self.registry, &call.name);
                self.registry.supports_parallel(tool_name)
            });

        if !parallel.is_empty() {
            let _guard = self.gate.read().await;
            let mut futures: FuturesUnordered<_> = parallel
                .iter()
                .map(|(index, call)| {
                    let on_progress = on_progress.clone();
                    async move { (*index, self.execute_single(call, &on_progress).await) }
                })
                .collect();
            while let Some((index, result)) = futures.next().await {
                if let Some(callback) = &on_completion {
                    callback(result.clone()).await;
                }
                indexed_results.push((index, result));
            }
        }

        for (index, call) in exclusive {
            let _guard = self.gate.write().await;
            let result = self.execute_single(call, &on_progress).await;
            if let Some(callback) = &on_completion {
                callback(result.clone()).await;
            }
            indexed_results.push((index, result));
        }

        indexed_results.sort_by_key(|(index, _)| *index);
        indexed_results
            .into_iter()
            .map(|(_, result)| result)
            .collect()
    }

    pub fn agent_scope(&self) -> ToolAgentScope {
        self.context.agent_scope
    }

    pub(crate) async fn execute_single(
        &self,
        call: &ToolCall,
        on_progress: &Option<ProgressCallbackArc>,
    ) -> ToolCallResult {
        let tool_name = canonical_tool_name(&self.registry, &call.name);
        if self.context.agent_scope == ToolAgentScope::Subagent
            && (is_subagent_agent_coordination_tool(&call.name)
                || is_subagent_agent_coordination_tool(tool_name))
        {
            return ToolCallResult::error(
                &call.id,
                "sub-agents cannot use parent-agent coordination tools",
            );
        }
        if let Some(reason) = super::hook_events::pre_tool_use_block_reason(
            self.context.hooks.as_ref(),
            call,
            tool_name,
        )
        .await
        {
            super::hook_events::post_tool_use_failure(
                self.context.hooks.as_ref(),
                call,
                tool_name,
                &reason,
            )
            .await;
            return ToolCallResult::error(&call.id, &format!("blocked by hook: {reason}"));
        }
        let tool = match self
            .registry
            .get(tool_name)
            .or_else(|| self.registry.get(&call.name))
        {
            Some(t) => t.clone(),
            None => {
                warn!(tool = %call.name, "tool not found");
                let message = format!("unknown tool: {}", call.name);
                super::hook_events::post_tool_use_failure(
                    self.context.hooks.as_ref(),
                    call,
                    tool_name,
                    &message,
                )
                .await;
                return ToolCallResult::error(&call.id, &message);
            }
        };

        let mut sandbox_profile = self.context.sandbox_profile.clone();
        let mut already_bypassed_sandbox = sandbox_profile_is_inactive(sandbox_profile.as_deref());
        // `already_approved`: only user/session/auto-review approvals unlock
        // a silent unsandbox retry after SANDBOX_DENIED (UnlessTrusted). Policy
        // Allow without asking stays false so denial surfaces for require_escalated.
        let mut already_approved = false;
        let permission_request = match self.permission_request_for_call(call, tool_name) {
            Ok(request) => request,
            Err(reason) => {
                let message = format!("invalid sandbox permission request: {reason}");
                super::hook_events::post_tool_use_failure(
                    self.context.hooks.as_ref(),
                    call,
                    tool_name,
                    &message,
                )
                .await;
                return ToolCallResult::error(&call.id, &message);
            }
        };
        let mut sandbox_permission_overlay = None;
        if let Some(request) = permission_request {
            match self.permission.check(request).await {
                Ok(grant) => {
                    already_approved = grant.already_approved;
                    sandbox_permission_overlay = grant.sandbox_permission_overlay.clone();
                    if grant.bypass_sandbox {
                        // `unsandboxed_execution_allowed`: deny-read policies
                        // must stay sandboxed even after escalation approval.
                        if devo_sandbox::unsandboxed_execution_allowed(
                            sandbox_profile.as_deref(),
                            &self.context.cwd,
                        ) {
                            sandbox_profile = Some("off".to_string());
                            already_bypassed_sandbox = true;
                        } else {
                            info!(
                                tool = %tool_name,
                                id = %call.id,
                                "keeping sandbox after escalation because profile has deny-read paths"
                            );
                        }
                    }
                }
                Err(reason) => {
                    let message = format!("permission denied: {reason}");
                    super::hook_events::post_tool_use_failure(
                        self.context.hooks.as_ref(),
                        call,
                        tool_name,
                        &message,
                    )
                    .await;
                    return ToolCallResult::error(&call.id, &message);
                }
            }
        }

        if let Some(callback) = &self.execution_options.on_tool_execution_start {
            callback(call.clone()).await;
        }
        info!(tool = %tool_name, id = %call.id, "executing tool");

        let build_ctx = |sandbox_profile: Option<String>| crate::contracts::ToolContext {
            tool_call_id: crate::invocation::ToolCallId(call.id.clone()),
            session_id: self.context.session_id.clone(),
            turn_id: self.context.turn_id.clone(),
            workspace_root: self.context.cwd.clone(),
            budgets: self.execution_options.budgets,
            cancel_token: self.execution_options.cancel_token.clone(),
            agent_scope: self.context.agent_scope,
            collaboration_mode: self.context.collaboration_mode,
            agent_coordinator: self.context.agent_coordinator.clone(),
            client_filesystem: self.context.client_filesystem.clone(),
            file_read_ledger: Some(Arc::clone(&self.context.file_read_ledger)),
            network_proxy: self.context.network_proxy.clone(),
            network_no_proxy: self.context.network_no_proxy.clone(),
            sandbox_profile,
            sandbox_permission_overlay: sandbox_permission_overlay.clone(),
        };

        let (progress_sender, progress_task) = match on_progress {
            Some(callback) => {
                let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ToolProgress>();
                let callback = Arc::clone(callback);
                let tool_use_id = call.id.clone();
                let task = tokio::spawn(async move {
                    while let Some(progress) = progress_rx.recv().await {
                        callback(tool_use_id.clone(), progress).await;
                    }
                });
                (Some(progress_tx), Some(task))
            }
            None => (None, None),
        };

        let input = self.input_for_tool_call(tool_name, &call.input);
        let cancel_token = self.execution_options.cancel_token.clone();
        let result = tokio::select! {
            biased;
            () = cancel_token.cancelled() => Err(crate::contracts::ToolCallError::Cancelled),
            result = tool.handle(build_ctx(sandbox_profile.clone()), input.clone(), progress_sender) => result,
        };

        // UnlessTrusted-style failure upgrade (tool orchestrator):
        // only silently retry sandbox-off once when this call was already
        // user-/session-/auto-review-approved. Otherwise return the denial so
        // the model can re-call with `sandbox_permissions: require_escalated`.
        // Deny-read profiles never silent-unsandbox (`unsandboxed_execution_allowed`).
        let result = match result {
            Ok(output)
                if !already_bypassed_sandbox
                    && already_approved
                    && is_shell_family_tool(tool_name)
                    && tool_result_is_sandbox_denied(&output) =>
            {
                if !devo_sandbox::unsandboxed_execution_allowed(
                    sandbox_profile.as_deref(),
                    &self.context.cwd,
                ) || sandbox_permission_overlay.is_some()
                {
                    info!(
                        tool = %tool_name,
                        id = %call.id,
                        "skipping unsandbox retry after SANDBOX_DENIED because the request must remain sandboxed"
                    );
                    Ok(output)
                } else {
                    info!(
                        tool = %tool_name,
                        id = %call.id,
                        "retrying tool without sandbox after SANDBOX_DENIED (already approved)"
                    );
                    tokio::select! {
                        biased;
                        () = cancel_token.cancelled() => {
                            Err(crate::contracts::ToolCallError::Cancelled)
                        }
                        result = tool.handle(
                            build_ctx(Some("off".to_string())),
                            input,
                            None,
                        ) => result,
                    }
                }
            }
            other => other,
        };

        if let Some(progress_task) = progress_task
            && tokio::time::timeout(
                Duration::from_millis(PROGRESS_DRAIN_GRACE_MS),
                progress_task,
            )
            .await
            .is_err()
        {
            warn!(tool = %tool_name, id = %call.id, "timed out draining tool progress");
        }

        match result {
            Ok(output) => {
                let content = match output.content {
                    crate::contracts::ToolResultContent::Text(text) => {
                        crate::invocation::ToolContent::Text(text)
                    }
                    crate::contracts::ToolResultContent::Json(json) => {
                        crate::invocation::ToolContent::Json(json)
                    }
                    crate::contracts::ToolResultContent::Mixed { text, json } => {
                        crate::invocation::ToolContent::Mixed { text, json }
                    }
                };
                let is_error = matches!(
                    output.structured_status,
                    crate::contracts::ToolTerminalStatus::Failed(_)
                        | crate::contracts::ToolTerminalStatus::Denied { .. }
                        | crate::contracts::ToolTerminalStatus::BlockedByMode { .. }
                );
                let result = ToolCallResult {
                    tool_use_id: call.id.clone(),
                    content,
                    is_error,
                    display_content: output.display_content,
                };
                if result.is_error {
                    super::hook_events::post_tool_use_failure(
                        self.context.hooks.as_ref(),
                        call,
                        tool_name,
                        &result.content.clone().into_string(),
                    )
                    .await;
                } else {
                    super::hook_events::post_tool_use(
                        self.context.hooks.as_ref(),
                        call,
                        tool_name,
                        &result,
                    )
                    .await;
                }
                result
            }
            Err(crate::contracts::ToolCallError::Cancelled) => {
                let message = INTERRUPTED_TOOL_RESULT_MESSAGE;
                super::hook_events::post_tool_use_failure(
                    self.context.hooks.as_ref(),
                    call,
                    tool_name,
                    message,
                )
                .await;
                ToolCallResult::error(&call.id, message)
            }
            Err(e) => {
                let message = e.to_string();
                super::hook_events::post_tool_use_failure(
                    self.context.hooks.as_ref(),
                    call,
                    tool_name,
                    &message,
                )
                .await;
                ToolCallResult::error(&call.id, &message)
            }
        }
    }

    fn input_for_tool_call(&self, tool_name: &str, input: &serde_json::Value) -> serde_json::Value {
        if tool_name != "web_search" {
            return input.clone();
        }
        let mut input = input.clone();
        if let Some(config) = &self.context.local_web_search
            && let Some(object) = input.as_object_mut()
            && let Ok(value) = serde_json::to_value(config)
        {
            object.insert("__devo_local_web_search".to_string(), value);
        }
        input
    }

    fn permission_request_for_call(
        &self,
        call: &ToolCall,
        tool_name: &str,
    ) -> Result<Option<ToolPermissionRequest>, String> {
        let Some(spec) = self.registry.spec(tool_name) else {
            return Ok(None);
        };
        let resource = resource_kind_for_tool(tool_name, &spec.capability_tags);
        let needs_permission = spec.execution_mode == crate::tool_spec::ToolExecutionMode::Mutating
            || resource_requires_permission(&resource);
        if !needs_permission {
            return Ok(None);
        }

        let path = path_for_tool_input(tool_name, &call.input, &self.context.cwd);
        let host = host_for_tool_input(tool_name, &call.input);
        let target = target_for_tool_input(tool_name, &call.input);
        let command_prefix = command_prefix_for_tool_input(tool_name, &call.input);
        let command_argv =
            command_str_for_tool_input(tool_name, &call.input).and_then(safe_shell_argv);
        let command_pattern = command_str_for_tool_input(tool_name, &call.input)
            .and_then(|command| generalize_command_pattern(command, &self.context.cwd));
        Ok(Some(ToolPermissionRequest {
            tool_call_id: call.id.clone(),
            tool_name: tool_name.to_string(),
            input: call.input.clone(),
            cwd: self.context.cwd.clone(),
            session_id: self.context.session_id.clone(),
            turn_id: self.context.turn_id.clone(),
            resource,
            action_summary: crate::tool_summary::tool_summary(
                tool_name,
                &call.input,
                &self.context.cwd,
            ),
            justification: justification_for_tool_input(&call.input),
            path,
            host,
            target,
            command_prefix,
            command_argv,
            command_pattern,
            sandbox_permissions: sandbox_permission_request_from_input(&call.input)?,
        }))
    }
}

fn canonical_tool_name<'a>(registry: &ToolRegistry, tool_name: &'a str) -> &'a str {
    match tool_name {
        "bash" if registry.spec("shell_command").is_some() => "shell_command",
        "glob" if registry.spec("find").is_some() => "find",
        "websearch" | "web-search" if registry.spec("web_search").is_some() => "web_search",
        "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url"
            if registry.spec("webfetch").is_some() =>
        {
            "webfetch"
        }
        _ => tool_name,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxPermissionRequest {
    Default,
    AdditionalPermissions(AdditionalSandboxPermissions),
    FullEscalation,
}

impl SandboxPermissionRequest {
    pub fn requests_escalation(&self) -> bool {
        !matches!(self, Self::Default)
    }

    pub fn bypasses_sandbox(&self) -> bool {
        matches!(self, Self::FullEscalation)
    }

    pub fn overlay(&self) -> Option<AdditionalSandboxPermissions> {
        match self {
            Self::AdditionalPermissions(permissions) => Some(permissions.clone()),
            Self::Default | Self::FullEscalation => None,
        }
    }
}

pub type AdditionalSandboxPermissions = devo_tools::SandboxPermissionOverlay;
pub type NetworkPermission = devo_tools::SandboxNetworkPermission;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionGrant {
    /// When true, this invocation must run with sandbox profile off/None.
    pub bypass_sandbox: bool,
    /// When true, a user (or session-cache / auto-review) approval already
    /// covered this call — `already_approved`. Policy Allow without
    /// asking leaves this false.
    pub already_approved: bool,
    /// Additional capabilities to merge into the active sandbox profile.
    pub sandbox_permission_overlay: Option<AdditionalSandboxPermissions>,
}

impl PermissionGrant {
    /// Grant from an interactive (or session-cache / auto-review) approval.
    pub fn from_approval(request: &SandboxPermissionRequest) -> Self {
        Self {
            bypass_sandbox: request.bypasses_sandbox(),
            already_approved: true,
            sandbox_permission_overlay: request.overlay(),
        }
    }
}

#[derive(Clone)]
pub struct PermissionChecker {
    inner: Arc<PermissionCheckFn>,
}

impl PermissionChecker {
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(ToolPermissionRequest) -> PermissionFuture + Send + Sync + 'static,
    {
        PermissionChecker {
            inner: Arc::new(check),
        }
    }

    pub fn always_allow() -> Self {
        PermissionChecker::new(|_| Box::pin(async { Ok(PermissionGrant::default()) }))
    }

    pub async fn check(&self, request: ToolPermissionRequest) -> Result<PermissionGrant, String> {
        (self.inner)(request).await
    }
}

#[derive(Clone)]
pub struct ToolRuntimeContext {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub cwd: PathBuf,
    pub agent_scope: ToolAgentScope,
    pub collaboration_mode: devo_protocol::CollaborationMode,
    pub agent_coordinator: Option<Arc<dyn AgentToolCoordinator>>,
    pub client_filesystem: Option<Arc<dyn ClientFilesystem>>,
    pub file_read_ledger: Arc<FileReadLedger>,
    pub local_web_search: Option<ResolvedLocalWebSearchConfig>,
    pub hooks: Option<crate::hooks::HookRuntimeContext>,
    pub network_proxy: Option<String>,
    pub network_no_proxy: Option<String>,
    /// Active sandbox profile name for child processes spawned by tools.
    pub sandbox_profile: Option<String>,
}

impl Default for ToolRuntimeContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            turn_id: None,
            cwd: PathBuf::new(),
            agent_scope: ToolAgentScope::default(),
            collaboration_mode: devo_protocol::CollaborationMode::default(),
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: Arc::new(FileReadLedger::new()),
            local_web_search: None,
            hooks: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_profile: None,
        }
    }
}

impl std::fmt::Debug for ToolRuntimeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRuntimeContext")
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("cwd", &self.cwd)
            .field("agent_scope", &self.agent_scope)
            .field("collaboration_mode", &self.collaboration_mode)
            .field(
                "agent_coordinator",
                &self.agent_coordinator.as_ref().map(|_| "<configured>"),
            )
            .field(
                "client_filesystem",
                &self.client_filesystem.as_ref().map(|_| "<configured>"),
            )
            .field("file_read_ledger", &"<configured>")
            .field(
                "local_web_search",
                &self
                    .local_web_search
                    .as_ref()
                    .map(|config| &config.provider_id),
            )
            .field("hooks", &self.hooks.as_ref().map(|_| "<configured>"))
            .field(
                "network_proxy",
                &self.network_proxy.as_ref().map(|_| "<configured>"),
            )
            .field(
                "network_no_proxy",
                &self.network_no_proxy.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct ToolExecutionOptions {
    pub budgets: ToolBudgets,
    pub cancel_token: CancellationToken,
    pub on_tool_execution_start: Option<ExecutionStartCallbackArc>,
}

impl std::fmt::Debug for ToolExecutionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionOptions")
            .field("budgets", &self.budgets)
            .field("cancel_token", &self.cancel_token)
            .field(
                "on_tool_execution_start",
                &self
                    .on_tool_execution_start
                    .as_ref()
                    .map(|_| "<configured>"),
            )
            .finish()
    }
}

impl Default for ToolExecutionOptions {
    fn default() -> Self {
        Self {
            budgets: ToolBudgets {
                output_limit_bytes: 32 * 1024,
                wall_time_limit_ms: Some(6_000),
            },
            cancel_token: CancellationToken::new(),
            on_tool_execution_start: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolPermissionRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub cwd: PathBuf,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub resource: ResourceKind,
    pub action_summary: String,
    pub justification: Option<String>,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    pub target: Option<String>,
    pub command_prefix: Option<Vec<String>>,
    /// Shell argv for the command after safety screening (no metacharacters,
    /// no leading env assignment); used to match session command patterns.
    pub command_argv: Option<Vec<String>>,
    /// Generalized command pattern (e.g. `git add *`) offered as the
    /// session-scoped approval grant for shell tools.
    pub command_pattern: Option<Vec<String>>,
    pub sandbox_permissions: SandboxPermissionRequest,
}

fn resource_kind_for_tool(tool_name: &str, tags: &[ToolCapabilityTag]) -> ResourceKind {
    if tags
        .iter()
        .any(|tag| matches!(tag, ToolCapabilityTag::NetworkAccess))
    {
        return ResourceKind::Network;
    }
    if tags
        .iter()
        .any(|tag| matches!(tag, ToolCapabilityTag::ExecuteProcess))
    {
        return ResourceKind::ShellExec;
    }
    if tags
        .iter()
        .any(|tag| matches!(tag, ToolCapabilityTag::WriteFiles))
    {
        return ResourceKind::FileWrite;
    }
    if tags.iter().any(|tag| {
        matches!(
            tag,
            ToolCapabilityTag::ReadFiles | ToolCapabilityTag::SearchWorkspace
        )
    }) {
        return ResourceKind::FileRead;
    }
    ResourceKind::Custom(tool_name.to_string())
}

fn resource_requires_permission(resource: &ResourceKind) -> bool {
    matches!(
        resource,
        ResourceKind::FileRead
            | ResourceKind::FileWrite
            | ResourceKind::ShellExec
            | ResourceKind::Network
    )
}

fn path_for_tool_input(tool_name: &str, input: &serde_json::Value, cwd: &Path) -> Option<PathBuf> {
    let raw = match tool_name {
        "read" | "write" | "edit" => input
            .get("filePath")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("path").and_then(serde_json::Value::as_str)),
        "lsp" => input
            .get("filePath")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("path").and_then(serde_json::Value::as_str))
            .or_else(|| input.get("file_path").and_then(serde_json::Value::as_str))
            .or(Some(".")),
        "find" | "grep" | "glob" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or(Some(".")),
        "code_search" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or(Some(".")),
        _ => None,
    }?;
    let path = PathBuf::from(raw);
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn host_for_tool_input(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url" => input
            .get("url")
            .and_then(serde_json::Value::as_str)
            .and_then(host_from_url),
        "web_search" | "websearch" | "web-search" => input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(|_| "web_search".to_string()),
        _ => None,
    }
}

fn host_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme
        .split('/')
        .next()
        .and_then(|host| (!host.is_empty()).then(|| host.to_string()))
}

fn target_for_tool_input(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "bash" | "shell_command" => input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        "exec_command" => input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url" => input
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        "web_search" | "websearch" | "web-search" => input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn command_prefix_for_tool_input(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<Vec<String>> {
    if tool_name == "exec_command"
        && let Some(prefix_rule) = input.get("prefix_rule").and_then(prefix_rule_from_value)
    {
        if crate::tools::exec_policy_amend::is_banned_prefix_suggestion(&prefix_rule) {
            return None;
        }
        return Some(prefix_rule);
    }

    let command = command_str_for_tool_input(tool_name, input)?;
    command_prefix(command)
}

fn command_str_for_tool_input<'a>(
    tool_name: &str,
    input: &'a serde_json::Value,
) -> Option<&'a str> {
    match tool_name {
        "bash" | "shell_command" => input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(serde_json::Value::as_str),
        "exec_command" => input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(serde_json::Value::as_str),
        _ => None,
    }
}

fn prefix_rule_from_value(value: &serde_json::Value) -> Option<Vec<String>> {
    let prefix = value
        .as_array()?
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    (!prefix.is_empty()).then(|| prefix.into_iter().map(str::to_string).collect())
}

pub fn sandbox_permissions_from_input(input: &serde_json::Value) -> String {
    input
        .get("sandbox_permissions")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub fn sandbox_permission_cache_key_from_input(input: &serde_json::Value) -> String {
    match sandbox_permission_request_from_input(input) {
        Ok(SandboxPermissionRequest::Default) => "default".to_string(),
        Ok(SandboxPermissionRequest::FullEscalation) => "full_escalation".to_string(),
        Ok(SandboxPermissionRequest::AdditionalPermissions(permissions)) => serde_json::json!({
            "tier": "additional_permissions",
            "network": matches!(permissions.network, NetworkPermission::Enabled),
            "read_paths": permissions.read_paths,
            "write_paths": permissions.write_paths,
        })
        .to_string(),
        Err(_) => serde_json::json!({
            "invalid": input.get("sandbox_permissions"),
            "additional_permissions": input.get("additional_permissions"),
        })
        .to_string(),
    }
}

pub fn sandbox_permission_request_from_input(
    input: &serde_json::Value,
) -> Result<SandboxPermissionRequest, String> {
    let mode = sandbox_permissions_from_input(input);
    let additional = input.get("additional_permissions");

    if mode == "require_escalated" {
        if additional.is_some() {
            return Err(
                "require_escalated cannot be combined with additional_permissions".to_string(),
            );
        }
        return Ok(SandboxPermissionRequest::FullEscalation);
    }

    let has_additional = additional.is_some();
    if !matches!(
        mode.as_str(),
        "" | "use_default" | "with_additional_permissions"
    ) {
        return Err(format!("unsupported sandbox_permissions value: {mode}"));
    }
    if mode == "with_additional_permissions" && !has_additional {
        return Err(
            "with_additional_permissions requires a non-empty additional_permissions object"
                .to_string(),
        );
    }
    let Some(additional) = additional else {
        return Ok(SandboxPermissionRequest::Default);
    };
    let permissions = parse_additional_sandbox_permissions(additional)?;
    if permissions.network == NetworkPermission::Unchanged
        && permissions.read_paths.is_empty()
        && permissions.write_paths.is_empty()
    {
        return Err("additional_permissions must request at least one capability".to_string());
    }
    Ok(SandboxPermissionRequest::AdditionalPermissions(permissions))
}

fn parse_additional_sandbox_permissions(
    value: &serde_json::Value,
) -> Result<AdditionalSandboxPermissions, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "additional_permissions must be an object".to_string())?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "network" | "file_system"))
    {
        return Err("additional_permissions contains an unknown field".to_string());
    }
    let network = match object.get("network") {
        None => NetworkPermission::Unchanged,
        Some(network) => {
            let network = network
                .as_object()
                .ok_or_else(|| "additional_permissions.network must be an object".to_string())?;
            if network.keys().any(|key| key != "enabled") {
                return Err("additional_permissions.network contains an unknown field".to_string());
            }
            match network.get("enabled") {
                Some(value) if value.as_bool() == Some(true) => NetworkPermission::Enabled,
                Some(value) if value.as_bool() == Some(false) => NetworkPermission::Unchanged,
                Some(_) => {
                    return Err(
                        "additional_permissions.network.enabled must be a boolean".to_string()
                    );
                }
                None => NetworkPermission::Unchanged,
            }
        }
    };
    let file_system =
        match object.get("file_system") {
            None => None,
            Some(file_system) => Some(file_system.as_object().ok_or_else(|| {
                "additional_permissions.file_system must be an object".to_string()
            })?),
        };
    if let Some(file_system) = file_system
        && file_system
            .keys()
            .any(|key| !matches!(key.as_str(), "read" | "write"))
    {
        return Err("additional_permissions.file_system contains an unknown field".to_string());
    }
    let read_paths = parse_absolute_permission_paths(
        file_system.and_then(|fs| fs.get("read")),
        "additional_permissions.file_system.read",
    )?;
    let write_paths = parse_absolute_permission_paths(
        file_system.and_then(|fs| fs.get("write")),
        "additional_permissions.file_system.write",
    )?;
    Ok(AdditionalSandboxPermissions {
        network,
        read_paths,
        write_paths,
    })
}

fn parse_absolute_permission_paths(
    value: Option<&serde_json::Value>,
    field: &str,
) -> Result<Vec<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let paths = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    let mut paths = paths
        .iter()
        .map(|value| {
            let path = value
                .as_str()
                .ok_or_else(|| format!("{field} entries must be strings"))?;
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(format!("{field} entries must be absolute paths"));
            }
            Ok(path)
        })
        .collect::<Result<Vec<_>, String>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn sandbox_profile_is_inactive(profile: Option<&str>) -> bool {
    matches!(
        profile.map(str::trim),
        None | Some("") | Some("off") | Some("none")
    )
}

fn is_shell_family_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "bash" | "shell_command" | "exec_command" | "write_stdin"
    )
}

fn tool_result_is_sandbox_denied(output: &crate::contracts::ToolResult) -> bool {
    let text = match &output.content {
        crate::contracts::ToolResultContent::Text(text) => text.as_str(),
        crate::contracts::ToolResultContent::Mixed {
            text: Some(text), ..
        } => text.as_str(),
        crate::contracts::ToolResultContent::Json(_)
        | crate::contracts::ToolResultContent::Mixed { text: None, .. } => "",
    };
    // Only the structured prefix triggers router unsandbox retry. Free-form
    // keyword heuristics (`is_likely_sandbox_denied`) stay in
    // `devo_sandbox` where exit codes can exclude 2/126/127 false positives.
    text.starts_with("SANDBOX_DENIED:") || output.result_summary.starts_with("SANDBOX_DENIED:")
}

pub fn command_str_for_permission_request(request: &ToolPermissionRequest) -> Option<String> {
    request
        .target
        .clone()
        .or_else(|| {
            command_str_for_tool_input(&request.tool_name, &request.input).map(str::to_string)
        })
        .filter(|command| !command.is_empty())
}

fn command_prefix(command: &str) -> Option<Vec<String>> {
    let argv = safe_shell_argv(command)?;
    prefix_from_argv(&argv)
}

/// Splits a shell command string into argv, rejecting anything that makes
/// token-based matching unsafe: compound commands, expansions, redirects,
/// and leading env assignments. Returns `None` unless the command is a
/// single plain invocation.
fn safe_shell_argv(command: &str) -> Option<Vec<String>> {
    if command_contains_line_separators(command) {
        return None;
    }
    let argv = shlex::split(command)?;
    if argv
        .iter()
        .any(|token| shell_token_requires_user_scope(command, token))
        || argv
            .first()
            .is_some_and(|token| looks_like_env_assignment(token))
    {
        return None;
    }
    Some(argv)
}

fn command_contains_line_separators(command: &str) -> bool {
    command
        .as_bytes()
        .iter()
        .any(|b| matches!(b, b'\n' | b'\r' | 0x0b | 0x0c))
}

fn shell_token_requires_user_scope(command: &str, token: &str) -> bool {
    token.contains(['|', ';', '>', '<', '*', '?', '$', '(', ')'])
        || token.contains("$(")
        || command.contains("&&")
        || command.contains("||")
        || command.contains("$(")
        || command.contains('`')
        || command_contains_standalone_ampersand(command)
}

/// True when `command` has a background `&` (not `&&`).
///
/// Uses shlex tokenization so `&` inside quoted strings (e.g. URL query
/// strings) does not count. Also treats a token that *ends* with `&`
/// (`sleep 1& rm x` → `1&`) as a background operator — shells parse that
/// the same way as a spaced `&`.
fn command_contains_standalone_ampersand(command: &str) -> bool {
    shlex::split(command).is_some_and(|argv| {
        argv.iter()
            .any(|token| token_is_background_ampersand(token))
    })
}

fn token_is_background_ampersand(token: &str) -> bool {
    // `&&` is AND-list, not background. A lone `&`, or any other token that
    // ends with `&` (`1&`, `cmd&`), is background.
    token != "&&" && (token == "&" || token.ends_with('&'))
}

fn looks_like_env_assignment(token: &str) -> bool {
    let Some((name, value)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !value.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
}

fn prefix_from_argv(argv: &[String]) -> Option<Vec<String>> {
    let executable = argv.first()?.clone();
    let second = argv
        .iter()
        .skip(1)
        .find(|token| !token.starts_with('-'))
        .cloned();
    Some(
        second
            .map(|token| vec![executable.clone(), token])
            .unwrap_or_else(|| vec![executable]),
    )
}

/// Upper bounds for generalized command patterns; commands beyond them fall
/// back to per-call approval instead of a session grant.
const COMMAND_PATTERN_MAX_TOKENS: usize = 16;
const COMMAND_PATTERN_MAX_WILDCARDS: usize = 8;

/// Programs whose arguments must never be generalized into a session approval
/// pattern: privilege escalation, shell re-entry, and argument-driven command
/// execution can each turn a broad pattern into an arbitrary command grant.
const COMMAND_PATTERN_PROGRAM_BLOCKLIST: &[&str] = &[
    "sudo",
    "doas",
    "run0",
    "sh",
    "bash",
    "zsh",
    "fish",
    "env",
    "xargs",
    "find",
    "eval",
    // Destructive / irreversible programs must never generalize to wildcards
    // (e.g. `rm foo` → `rm *` would auto-approve any rm for the session).
    "rm",
    "rmdir",
    "dd",
    "mkfs",
    "shred",
    "wipe",
    "chmod",
    "chown",
    "chgrp",
    "mv",
    "python",
    "python3",
    "perl",
    "ruby",
    "node",
    "php",
    "lua",
    "osascript",
];

/// Derives a generalized command pattern (e.g. `git add file.txt` ->
/// `["git", "add", "*"]`) used as a session-scoped approval grant. The
/// program, flags, and leading subcommand words stay verbatim; the first
/// value-like token and every later non-flag token become `*`. Returns
/// `None` when the command cannot be generalized safely (compound commands,
/// expansions, env assignments, blocklisted programs such as `sudo` or
/// `sh -c`, or patterns beyond the token/wildcard limits).
pub fn generalize_command_pattern(command: &str, cwd: &Path) -> Option<Vec<String>> {
    let argv = safe_shell_argv(command)?;
    command_pattern_from_argv(&argv, cwd)
}

fn command_pattern_from_argv(argv: &[String], _cwd: &Path) -> Option<Vec<String>> {
    if argv.is_empty() || argv.len() > COMMAND_PATTERN_MAX_TOKENS {
        return None;
    }
    let program = argv.first()?;
    if COMMAND_PATTERN_PROGRAM_BLOCKLIST.contains(&program_basename(program)) {
        return None;
    }
    let mut pattern = Vec::with_capacity(argv.len());
    pattern.push(program.clone());
    let mut wildcards = 0;
    let mut values_started = false;
    for token in &argv[1..] {
        // Flags stay verbatim in any position; subcommand words stay
        // verbatim only until the first value-like token appears.
        if token.starts_with('-') || (!values_started && is_subcommand_word(token)) {
            pattern.push(token.clone());
            continue;
        }
        values_started = true;
        wildcards += 1;
        if wildcards > COMMAND_PATTERN_MAX_WILDCARDS {
            return None;
        }
        pattern.push("*".to_string());
    }
    Some(pattern)
}

/// Matches a generalized command pattern against a concrete argv. An inner
/// `*` matches exactly one token; a trailing `*` matches one or more tokens.
/// Patterns without `*` require a full verbatim match.
pub fn command_pattern_matches(pattern: &[String], argv: &[String]) -> bool {
    let mut args = argv.iter();
    let mut tokens = pattern.iter().peekable();
    while let Some(token) = tokens.next() {
        if token.as_str() == "*" {
            if tokens.peek().is_none() {
                return args.next().is_some();
            }
            if args.next().is_none() {
                return false;
            }
            continue;
        }
        if args.next() != Some(token) {
            return false;
        }
    }
    args.next().is_none()
}

fn program_basename(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn is_subcommand_word(token: &str) -> bool {
    // Subcommand words are short lowercase identifiers (`^[a-z][a-z0-9_-]{0,31}$`,
    // e.g. `add` in `git add`); the charset already excludes path and
    // assignment punctuation such as `.`, `/`, `=`, `:`, `~`.
    // Do not probe the filesystem: cwd existence would make patterns drift.
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && token.len() <= 32
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

fn justification_for_tool_input(input: &serde_json::Value) -> Option<String> {
    input
        .get("justification")
        .or_else(|| input.get("description"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ToolCallError;
    use crate::contracts::ToolContext;
    use crate::contracts::ToolProgressSender;
    use crate::contracts::ToolResult;
    use crate::contracts::ToolResultContent;
    use crate::json_schema::JsonSchema;
    use crate::registry::ToolRegistryBuilder;
    use crate::tool_handler::ToolHandler;
    use crate::tool_spec::ToolExecutionMode;
    use crate::tool_spec::ToolOutputMode;
    use crate::tool_spec::ToolPreparationFeedback;
    use crate::tool_spec::ToolSpec;
    use async_trait::async_trait;
    use pretty_assertions::assert_eq;

    struct ReadOnlyTool {
        spec: ToolSpec,
    }

    impl ReadOnlyTool {
        fn new() -> Self {
            Self {
                spec: ToolSpec::new(
                    "read_tool",
                    "read",
                    JsonSchema::object(Default::default(), None, None),
                ),
            }
        }
    }

    #[async_trait]
    impl ToolHandler for ReadOnlyTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }
        async fn handle(
            &self,
            _ctx: ToolContext,
            _input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            Ok(ToolResult::success(
                ToolResultContent::Text("read ok".into()),
                "read ok",
            ))
        }
    }

    struct WriteTool {
        spec: ToolSpec,
    }

    impl WriteTool {
        fn new() -> Self {
            Self {
                spec: ToolSpec::new(
                    "write_tool",
                    "write",
                    JsonSchema::object(Default::default(), None, None),
                ),
            }
        }
    }

    #[async_trait]
    impl ToolHandler for WriteTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }
        async fn handle(
            &self,
            _ctx: ToolContext,
            _input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            Ok(ToolResult::success(
                ToolResultContent::Text("write ok".into()),
                "write ok",
            ))
        }
    }

    struct DelayedReadTool {
        spec: ToolSpec,
    }

    impl DelayedReadTool {
        fn new() -> Self {
            Self {
                spec: ToolSpec::new(
                    "delayed_read_tool",
                    "delayed read",
                    JsonSchema::object(Default::default(), None, None),
                ),
            }
        }
    }

    #[async_trait]
    impl ToolHandler for DelayedReadTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }
        async fn handle(
            &self,
            _ctx: ToolContext,
            input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            let delay_ms = input
                .get("delay_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            let output = input
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Ok(ToolResult::success(
                ToolResultContent::Text(output.to_string()),
                "done",
            ))
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("read_tool", Arc::new(ReadOnlyTool::new()));
        builder.push_spec(ToolSpec {
            name: "read_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        builder.register_handler("read", Arc::new(ReadOnlyTool::new()));
        builder.push_spec(ToolSpec {
            name: "read".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::ReadFiles],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        builder.register_handler("write_tool", Arc::new(WriteTool::new()));
        builder.push_spec(ToolSpec {
            name: "write_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::WriteFiles],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        builder.register_handler("delayed_read_tool", Arc::new(DelayedReadTool::new()));
        builder.push_spec(ToolSpec {
            name: "delayed_read_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        Arc::new(builder.build())
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "nonexistent".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(result.is_error);
        assert!(result.content.into_string().contains("unknown tool"));
    }

    #[tokio::test]
    async fn subagent_runtime_blocks_parent_agent_coordination_tools() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_with_context(
            registry,
            PermissionChecker::always_allow(),
            ToolRuntimeContext {
                agent_scope: ToolAgentScope::Subagent,
                ..ToolRuntimeContext::default()
            },
        );

        for name in [
            "spawn_agent",
            "spawn-agent",
            "spawnagent",
            "spawn_subagent",
            "spawn-subagent",
            "subagent",
            "sub_agent",
            "delegate",
            "send_message",
            "send-message",
            "sendmessage",
            "await_task",
            "await-task",
            "awaittask",
            "list_tasks",
            "list-tasks",
            "listtasks",
            "cancel_task",
            "cancel-task",
            "canceltask",
            "wait_agent",
            "wait-agent",
            "waitagent",
            "subagent_result",
            "subagent-result",
            "list_agents",
            "list-agents",
            "listagents",
            "subagent_status",
            "subagent-status",
            "close_agent",
            "close-agent",
            "closeagent",
        ] {
            let call = ToolCall {
                id: format!("call-{name}"),
                name: name.to_string(),
                input: serde_json::json!({}),
            };
            let result = runtime.execute_single(&call, &None).await;

            assert!(result.is_error);
            assert_eq!(
                result.content.into_string(),
                "sub-agents cannot use parent-agent coordination tools"
            );
        }
    }

    #[tokio::test]
    async fn read_only_tool_succeeds() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn execute_batch_runs_all_tools() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let calls = vec![
            ToolCall {
                id: "c1".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".into(),
                name: "write_tool".into(),
                input: serde_json::json!({}),
            },
        ];
        let results = runtime.execute_batch(&calls).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.is_error));
    }

    #[tokio::test]
    async fn permission_checker_allow() {
        let checker = PermissionChecker::always_allow();
        assert!(
            checker
                .check(test_permission_request("any_tool"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn permission_checker_deny() {
        let checker = PermissionChecker::new(|request| {
            let n = request.tool_name;
            Box::pin(async move {
                if n == "blocked" {
                    Err("blocked".into())
                } else {
                    Ok(PermissionGrant::default())
                }
            })
        });
        assert!(
            checker
                .check(test_permission_request("allowed"))
                .await
                .is_ok()
        );
        assert!(
            checker
                .check(test_permission_request("blocked"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn runtime_denies_mutating_with_deny_checker() {
        let registry = make_registry();
        let checker = PermissionChecker::new(|request| {
            let n = request.tool_name;
            Box::pin(async move { Err(format!("{n} denied")) })
        });
        let runtime = ToolRuntime::new(registry, checker);
        // Read-only tools that do not access guarded resources should still run
        // without a permission request.
        let read_call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let read_result = runtime.execute_single(&read_call, &None).await;
        assert!(
            !read_result.is_error,
            "read-only tool should bypass permission check"
        );

        // Mutating tool should be denied
        let write_call = ToolCall {
            id: "c2".into(),
            name: "write_tool".into(),
            input: serde_json::json!({}),
        };
        let write_result = runtime.execute_single(&write_call, &None).await;
        assert!(write_result.is_error, "mutating tool should be denied");
        assert!(
            write_result
                .content
                .into_string()
                .contains("permission denied")
        );
    }

    #[tokio::test]
    async fn runtime_checks_file_read_tools() {
        let registry = make_registry();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let checker = PermissionChecker::new(move |request| {
            tx.lock()
                .expect("lock sender")
                .take()
                .expect("send once")
                .send(request)
                .expect("receiver still alive");
            Box::pin(async { Err("read denied".into()) })
        });
        let runtime = ToolRuntime::new_with_context(
            registry,
            checker,
            ToolRuntimeContext {
                cwd: PathBuf::from("C:/workspace"),
                ..ToolRuntimeContext::default()
            },
        );
        let call = ToolCall {
            id: "call-read".into(),
            name: "read".into(),
            input: serde_json::json!({ "filePath": "src/lib.rs" }),
        };

        let result = runtime.execute_single(&call, &None).await;
        let request = rx.await.expect("permission request");

        assert!(result.is_error);
        assert_eq!(request.tool_name, "read");
        assert_eq!(request.resource, devo_safety::ResourceKind::FileRead);
        assert_eq!(
            request.path,
            Some(PathBuf::from("C:/workspace").join("src/lib.rs"))
        );
        assert!(result.content.into_string().contains("permission denied"));
    }

    #[tokio::test]
    async fn mutating_tool_permission_request_carries_context_and_summary() {
        let registry = make_registry();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let checker = PermissionChecker::new(move |request| {
            tx.lock()
                .expect("lock sender")
                .take()
                .expect("send once")
                .send(request)
                .expect("receiver still alive");
            Box::pin(async { Ok(PermissionGrant::default()) })
        });
        let runtime = ToolRuntime::new_with_context(
            registry,
            checker,
            ToolRuntimeContext {
                session_id: "session-1".into(),
                turn_id: Some("turn-1".into()),
                cwd: PathBuf::from("C:/workspace"),
                agent_scope: ToolAgentScope::Parent,
                collaboration_mode: devo_protocol::CollaborationMode::Build,
                agent_coordinator: None,
                client_filesystem: None,
                file_read_ledger: std::sync::Arc::new(devo_tools::FileReadLedger::new()),
                local_web_search: None,
                hooks: None,
                network_proxy: None,
                network_no_proxy: None,
                sandbox_profile: None,
            },
        );
        let call = ToolCall {
            id: "call-1".into(),
            name: "write_tool".into(),
            input: serde_json::json!({ "filePath": "src/main.rs" }),
        };

        let result = runtime.execute_single(&call, &None).await;
        let request = rx.await.expect("permission request");

        assert!(!result.is_error);
        assert_eq!(request.tool_call_id, "call-1");
        assert_eq!(request.tool_name, "write_tool");
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.turn_id, Some("turn-1".into()));
        assert_eq!(request.resource, devo_safety::ResourceKind::FileWrite);
    }

    #[tokio::test]
    async fn bash_alias_uses_shell_command_permission_metadata() {
        let mut builder = ToolRegistryBuilder::new();
        let handler: Arc<dyn ToolHandler> = Arc::new(WriteTool::new());
        builder.register_handler("shell_command", Arc::clone(&handler));
        builder.register_handler("bash", handler);
        builder.push_spec(ToolSpec {
            name: "shell_command".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let checker = PermissionChecker::new(move |request| {
            tx.lock()
                .expect("lock sender")
                .take()
                .expect("send once")
                .send(request)
                .expect("receiver still alive");
            Box::pin(async { Err("blocked".into()) })
        });
        let runtime = ToolRuntime::new(registry, checker);
        let call = ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            input: serde_json::json!({ "command": "git status" }),
        };

        let result = runtime.execute_single(&call, &None).await;
        let request = rx.await.expect("permission request");

        assert!(result.is_error);
        assert_eq!(request.tool_name, "shell_command");
        assert_eq!(request.resource, devo_safety::ResourceKind::ShellExec);
        assert_eq!(request.target.as_deref(), Some("git status"));
        assert_eq!(
            request.command_prefix,
            Some(vec!["git".to_string(), "status".to_string()])
        );
    }

    #[test]
    fn path_for_tool_input_resolves_relative_paths_against_cwd() {
        let path = path_for_tool_input(
            "write",
            &serde_json::json!({ "filePath": "src/lib.rs" }),
            Path::new("C:/workspace"),
        );

        assert_eq!(path, Some(PathBuf::from("C:/workspace").join("src/lib.rs")));
    }

    #[test]
    fn path_for_tool_input_defaults_workspace_searches_to_cwd() {
        let path = path_for_tool_input(
            "grep",
            &serde_json::json!({ "pattern": "needle" }),
            Path::new("C:/workspace"),
        );

        assert_eq!(path, Some(PathBuf::from("C:/workspace").join(".")));
    }

    #[test]
    fn path_for_tool_input_code_search_uses_search_root() {
        let default_root = path_for_tool_input(
            "code_search",
            &serde_json::json!({
                "operation": "find_related",
                "file_path": "src/main.rs",
                "line": 1
            }),
            Path::new("C:/workspace"),
        );
        let explicit_root = path_for_tool_input(
            "code_search",
            &serde_json::json!({
                "operation": "find_related",
                "path": "crates/core",
                "file_path": "src/main.rs",
                "line": 1
            }),
            Path::new("C:/workspace"),
        );

        assert_eq!(
            (default_root, explicit_root),
            (
                Some(PathBuf::from("C:/workspace").join(".")),
                Some(PathBuf::from("C:/workspace").join("crates/core"))
            )
        );
    }

    #[tokio::test]
    async fn runtime_code_search_permission_uses_search_root() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("code_search", Arc::new(ReadOnlyTool::new()));
        builder.push_spec(ToolSpec {
            name: "code_search".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::StructuredJson,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::SearchWorkspace],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let checker = PermissionChecker::new(move |request| {
            tx.lock()
                .expect("lock sender")
                .take()
                .expect("send once")
                .send(request)
                .expect("receiver still alive");
            Box::pin(async { Err("read denied".into()) })
        });
        let runtime = ToolRuntime::new_with_context(
            registry,
            checker,
            ToolRuntimeContext {
                cwd: PathBuf::from("C:/workspace"),
                ..ToolRuntimeContext::default()
            },
        );
        let call = ToolCall {
            id: "call-code-search".into(),
            name: "code_search".into(),
            input: serde_json::json!({
                "operation": "find_related",
                "file_path": "src/main.rs",
                "line": 1
            }),
        };

        let result = runtime.execute_single(&call, &None).await;
        let request = rx.await.expect("permission request");

        assert!(result.is_error);
        assert_eq!(request.tool_name, "code_search");
        assert_eq!(request.resource, devo_safety::ResourceKind::FileRead);
        assert_eq!(request.path, Some(PathBuf::from("C:/workspace").join(".")));
        assert!(result.content.into_string().contains("permission denied"));
    }

    #[test]
    fn host_from_url_ignores_scheme_and_path() {
        assert_eq!(
            host_from_url("https://example.com/docs/index.html"),
            Some("example.com".into())
        );
    }

    #[test]
    fn command_prefix_uses_first_command_tokens() {
        assert_eq!(
            command_prefix("git add -A"),
            Some(vec!["git".to_string(), "add".to_string()])
        );
        assert_eq!(
            command_prefix("'cargo' test --all"),
            Some(vec!["cargo".to_string(), "test".to_string()])
        );
    }

    #[test]
    fn command_prefix_rejects_complex_shell_features() {
        assert_eq!(command_prefix("git add -A | tee out.txt"), None);
        assert_eq!(command_prefix("npm test > output.txt"), None);
        assert_eq!(command_prefix("echo $(pwd)"), None);
        assert_eq!(command_prefix("echo $HOME"), None);
        assert_eq!(command_prefix("FOO=bar cargo test"), None);
        assert_eq!(command_prefix("(pwd)"), None);
        assert_eq!(command_prefix("rg *.rs"), None);
        assert_eq!(command_prefix("cargo fmt && cargo test"), None);
    }

    #[test]
    fn exec_command_prefix_rule_overrides_derived_prefix() {
        assert_eq!(
            command_prefix_for_tool_input(
                "exec_command",
                &serde_json::json!({
                    "cmd": "git add -A",
                    "prefix_rule": ["cargo", "test"]
                })
            ),
            Some(vec!["cargo".to_string(), "test".to_string()])
        );
    }

    #[test]
    fn exec_command_banned_prefix_rule_is_not_offered() {
        assert_eq!(
            command_prefix_for_tool_input(
                "exec_command",
                &serde_json::json!({
                    "cmd": "git status",
                    "prefix_rule": ["git"]
                })
            ),
            None
        );
    }

    fn strs(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| token.to_string()).collect()
    }

    #[test]
    fn command_pattern_generalizes_value_arguments() {
        let cwd = std::path::Path::new("/nonexistent-cwd-for-pattern-tests");
        assert_eq!(
            generalize_command_pattern("git add file.txt", cwd),
            Some(strs(&["git", "add", "*"]))
        );
        // Interpreters are blocklisted and must not generalize.
        assert_eq!(generalize_command_pattern("node -e 'foo.bar'", cwd), None);
        assert_eq!(
            generalize_command_pattern("git commit -m 'initial commit'", cwd),
            Some(strs(&["git", "commit", "-m", "*"]))
        );
        assert_eq!(
            generalize_command_pattern("git add a.txt b.txt", cwd),
            Some(strs(&["git", "add", "*", "*"]))
        );
        // Words after the first value are values too, even if they look like subcommands.
        assert_eq!(
            generalize_command_pattern("git add file.txt docs", cwd),
            Some(strs(&["git", "add", "*", "*"]))
        );
    }

    #[test]
    fn command_pattern_allows_verbatim_patterns_without_wildcards() {
        let cwd = std::path::Path::new("/nonexistent-cwd-for-pattern-tests");
        assert_eq!(
            generalize_command_pattern("cargo build --release", cwd),
            Some(strs(&["cargo", "build", "--release"]))
        );
        assert_eq!(
            generalize_command_pattern("git status", cwd),
            Some(strs(&["git", "status"]))
        );
    }

    #[test]
    fn command_pattern_rejects_unsafe_commands() {
        let cwd = std::path::Path::new("/nonexistent-cwd-for-pattern-tests");
        // Blocklisted programs, including via path basename.
        assert_eq!(generalize_command_pattern("sudo rm -rf /tmp/x", cwd), None);
        assert_eq!(generalize_command_pattern("rm /tmp/x", cwd), None);
        assert_eq!(
            generalize_command_pattern("dd if=/dev/zero of=x", cwd),
            None
        );
        assert_eq!(generalize_command_pattern("/usr/bin/sudo ls", cwd), None);
        assert_eq!(generalize_command_pattern("sh -c 'ls'", cwd), None);
        assert_eq!(generalize_command_pattern("bash -c 'ls'", cwd), None);
        assert_eq!(generalize_command_pattern("find . -name foo", cwd), None);
        assert_eq!(generalize_command_pattern("xargs rm", cwd), None);
        assert_eq!(generalize_command_pattern("env ls", cwd), None);
        assert_eq!(generalize_command_pattern("eval ls", cwd), None);
        // Compound commands, expansions, redirects, env assignments.
        assert_eq!(
            generalize_command_pattern("git add a && git commit", cwd),
            None
        );
        assert_eq!(generalize_command_pattern("cat x | grep y", cwd), None);
        assert_eq!(generalize_command_pattern("echo hi > out.txt", cwd), None);
        assert_eq!(generalize_command_pattern("echo $(pwd)", cwd), None);
        assert_eq!(generalize_command_pattern("FOO=bar cargo test", cwd), None);
        // Background `&` (not `&&`) must not be treated as a safe prefix.
        assert_eq!(
            generalize_command_pattern("sleep 1 & touch /tmp/x", cwd),
            None
        );
        // Quoted URL query `&` is not a background operator.
        assert!(!command_contains_standalone_ampersand(
            r#"curl "http://example.com/?a=1&b=2""#
        ));
        assert!(command_contains_standalone_ampersand("sleep 1 & touch x"));
        assert!(command_contains_standalone_ampersand("sleep 1& rm x"));
        assert!(!command_contains_standalone_ampersand("true && false"));
        assert!(!token_is_background_ampersand("&&"));
        assert!(token_is_background_ampersand("1&"));
        assert!(token_is_background_ampersand("&"));
        // Unparseable quoting.
        assert_eq!(
            generalize_command_pattern("git add 'unterminated", cwd),
            None
        );
    }

    #[test]
    fn command_pattern_enforces_token_and_wildcard_limits() {
        let cwd = std::path::Path::new("/nonexistent-cwd-for-pattern-tests");
        // 17 tokens exceeds the 16-token cap.
        assert_eq!(
            generalize_command_pattern("git a b c d e f g h i j k l m n o p", cwd),
            None
        );
        // 9 wildcards exceeds the 8-wildcard cap.
        assert_eq!(
            generalize_command_pattern("git add A B C D E F G H I", cwd),
            None
        );
        // 8 wildcards is still allowed.
        assert_eq!(
            generalize_command_pattern("git add A B C D E F G H", cwd),
            Some(strs(&[
                "git", "add", "*", "*", "*", "*", "*", "*", "*", "*"
            ]))
        );
    }

    #[test]
    fn command_pattern_keeps_subcommand_words_without_cwd_stat() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let cwd = tempdir.path();
        std::fs::write(cwd.join("add"), "placeholder").expect("write placeholder");

        // Existing cwd path must not turn a subcommand word into a wildcard.
        assert_eq!(
            generalize_command_pattern("git add", cwd),
            Some(strs(&["git", "add"]))
        );
    }

    #[test]
    fn command_pattern_matches_trailing_wildcard_one_or_more() {
        let pattern = strs(&["git", "add", "*"]);
        assert!(command_pattern_matches(
            &pattern,
            &strs(&["git", "add", "file.txt"])
        ));
        assert!(command_pattern_matches(
            &pattern,
            &strs(&["git", "add", "a", "b"])
        ));
        assert!(!command_pattern_matches(&pattern, &strs(&["git", "add"])));
        assert!(!command_pattern_matches(
            &pattern,
            &strs(&["git", "commit", "x"])
        ));
        assert!(!command_pattern_matches(
            &pattern,
            &strs(&["sudo", "git", "add", "x"])
        ));
    }

    #[test]
    fn command_pattern_matches_inner_wildcard_exactly_one() {
        let pattern = strs(&["git", "commit", "-m", "*", "--amend"]);
        assert!(command_pattern_matches(
            &pattern,
            &strs(&["git", "commit", "-m", "msg", "--amend"])
        ));
        assert!(!command_pattern_matches(
            &pattern,
            &strs(&["git", "commit", "-m", "a", "b", "--amend"])
        ));
        assert!(!command_pattern_matches(
            &pattern,
            &strs(&["git", "commit", "-m", "--amend"])
        ));
    }

    #[test]
    fn command_pattern_matches_verbatim_requires_exact_argv() {
        let pattern = strs(&["git", "status"]);
        assert!(command_pattern_matches(&pattern, &strs(&["git", "status"])));
        assert!(!command_pattern_matches(
            &pattern,
            &strs(&["git", "status", "-s"])
        ));
        assert!(!command_pattern_matches(&pattern, &strs(&["git"])));
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: sandbox permission inputs are classified into explicit tiers.
    #[test]
    fn explicit_sandbox_permissions_are_classified() {
        assert_eq!(
            sandbox_permission_request_from_input(&serde_json::json!({
            "sandbox_permissions": "require_escalated"
            }))
            .expect("full escalation request"),
            SandboxPermissionRequest::FullEscalation
        );
        assert_eq!(
            sandbox_permission_request_from_input(&serde_json::json!({
                "sandbox_permissions": "with_additional_permissions",
                "additional_permissions": {
                    "network": {"enabled": true},
                    "file_system": {
                        "read": ["/tmp/input"],
                        "write": ["/tmp/output"]
                    }
                }
            }))
            .expect("additional permissions request"),
            SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
                network: NetworkPermission::Enabled,
                read_paths: vec![std::path::PathBuf::from("/tmp/input")],
                write_paths: vec![std::path::PathBuf::from("/tmp/output")],
            })
        );
        assert_eq!(
            sandbox_permission_request_from_input(&serde_json::json!({
                "additional_permissions": {
                    "file_system": {"read": ["/tmp/legacy"]}
                }
            }))
            .expect("legacy additional permissions request"),
            SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
                network: NetworkPermission::Unchanged,
                read_paths: vec![std::path::PathBuf::from("/tmp/legacy")],
                write_paths: vec![],
            })
        );
        assert_eq!(
            sandbox_permission_request_from_input(&serde_json::json!({
            "sandbox_permissions": "use_default"
            }))
            .expect("default sandbox request"),
            SandboxPermissionRequest::Default
        );
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: malformed or ambiguous sandbox requests fail closed.
    #[test]
    fn sandbox_permission_request_rejects_invalid_inputs() {
        for input in [
            serde_json::json!({
                "sandbox_permissions": "with_additional_permissions",
                "additional_permissions": {}
            }),
            serde_json::json!({
                "additional_permissions": {
                    "file_system": {"read": ["relative/path"]}
                }
            }),
            serde_json::json!({
                "sandbox_permissions": "require_escalated",
                "additional_permissions": {
                    "file_system": {"read": ["/tmp/input"]}
                }
            }),
        ] {
            assert!(
                sandbox_permission_request_from_input(&input).is_err(),
                "input should be rejected: {input}"
            );
        }
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: permission-cache keys use normalized tiers and path sets.
    #[test]
    fn sandbox_permission_cache_key_normalizes_additional_permissions() {
        let first = sandbox_permission_cache_key_from_input(&serde_json::json!({
            "sandbox_permissions": "with_additional_permissions",
            "additional_permissions": {
                "file_system": {"read": ["/tmp/b", "/tmp/a"]}
            }
        }));
        let second = sandbox_permission_cache_key_from_input(&serde_json::json!({
            "additional_permissions": {
                "file_system": {"read": ["/tmp/a", "/tmp/b"]}
            }
        }));
        assert_eq!(first, second);
        assert_ne!(
            first,
            sandbox_permission_cache_key_from_input(&serde_json::json!({
                "sandbox_permissions": "require_escalated"
            }))
        );
    }

    #[test]
    fn sandbox_profile_inactive_detection() {
        assert!(sandbox_profile_is_inactive(None));
        assert!(sandbox_profile_is_inactive(Some("")));
        assert!(sandbox_profile_is_inactive(Some("off")));
        assert!(sandbox_profile_is_inactive(Some("none")));
        assert!(!sandbox_profile_is_inactive(Some("workspace")));
    }

    #[test]
    fn tool_result_detects_sandbox_denied_prefix() {
        let denied = ToolResult::error(
            ToolResultContent::Text("SANDBOX_DENIED: blocked".into()),
            "failed",
            ToolCallError::ExecutionFailed("SANDBOX_DENIED: blocked".into()),
        );
        assert!(tool_result_is_sandbox_denied(&denied));

        let other = ToolResult::error(
            ToolResultContent::Text("exit code 1".into()),
            "failed",
            ToolCallError::ExecutionFailed("exit code 1".into()),
        );
        assert!(!tool_result_is_sandbox_denied(&other));
    }

    #[tokio::test]
    async fn execute_single_retries_without_sandbox_when_already_approved() {
        struct SandboxAwareTool {
            spec: ToolSpec,
            attempts: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        }

        #[async_trait]
        impl ToolHandler for SandboxAwareTool {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            async fn handle(
                &self,
                ctx: ToolContext,
                _input: serde_json::Value,
                _progress: Option<ToolProgressSender>,
            ) -> Result<ToolResult, ToolCallError> {
                self.attempts
                    .lock()
                    .expect("attempts lock")
                    .push(ctx.sandbox_profile.clone());
                if ctx.sandbox_profile.as_deref() == Some("workspace") {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text(
                            "SANDBOX_DENIED: The command was blocked by the OS sandbox.".into(),
                        ),
                        "denied",
                        ToolCallError::ExecutionFailed("SANDBOX_DENIED".into()),
                    ));
                }
                Ok(ToolResult::success(
                    ToolResultContent::Text("ok outside sandbox".into()),
                    "ok",
                ))
            }
        }

        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "shell_command",
            Arc::new(SandboxAwareTool {
                spec: ToolSpec {
                    name: "shell_command".into(),
                    description: String::new(),
                    input_schema: JsonSchema::object(Default::default(), None, None),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::Mutating,
                    capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                    supports_parallel: false,
                    preparation_feedback: ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                },
                attempts: Arc::clone(&attempts),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "shell_command".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());

        let permission_checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let permission_checks_for_checker = Arc::clone(&permission_checks);
        let checker = PermissionChecker::new(move |_request| {
            permission_checks_for_checker.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(PermissionGrant::from_approval(
                    &SandboxPermissionRequest::Default,
                ))
            })
        });

        let mut runtime = ToolRuntime::new(registry, checker);
        runtime.context.sandbox_profile = Some("workspace".to_string());

        let call = ToolCall {
            id: "deny1".into(),
            name: "shell_command".into(),
            input: serde_json::json!({ "command": "touch /tmp/x" }),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
        assert_eq!(result.content.into_string(), "ok outside sandbox");
        assert_eq!(
            permission_checks.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "SANDBOX_DENIED retry must not request a second permission check"
        );
        assert_eq!(
            *attempts.lock().expect("attempts lock"),
            vec![Some("workspace".to_string()), Some("off".to_string())]
        );
    }

    #[tokio::test]
    async fn execute_single_does_not_silent_unsandbox_without_approval() {
        struct SandboxAwareTool {
            spec: ToolSpec,
            attempts: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        }

        #[async_trait]
        impl ToolHandler for SandboxAwareTool {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            async fn handle(
                &self,
                ctx: ToolContext,
                _input: serde_json::Value,
                _progress: Option<ToolProgressSender>,
            ) -> Result<ToolResult, ToolCallError> {
                self.attempts
                    .lock()
                    .expect("attempts lock")
                    .push(ctx.sandbox_profile.clone());
                Ok(ToolResult::error(
                    ToolResultContent::Text(
                        "SANDBOX_DENIED: The command was blocked by the OS sandbox.".into(),
                    ),
                    "denied",
                    ToolCallError::ExecutionFailed("SANDBOX_DENIED".into()),
                ))
            }
        }

        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "shell_command",
            Arc::new(SandboxAwareTool {
                spec: ToolSpec {
                    name: "shell_command".into(),
                    description: String::new(),
                    input_schema: JsonSchema::object(Default::default(), None, None),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::Mutating,
                    capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                    supports_parallel: false,
                    preparation_feedback: ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                },
                attempts: Arc::clone(&attempts),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "shell_command".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());

        // Policy Allow: already_approved stays false (PermissionGrant::default).
        let checker =
            PermissionChecker::new(|_| Box::pin(async { Ok(PermissionGrant::default()) }));

        let mut runtime = ToolRuntime::new(registry, checker);
        runtime.context.sandbox_profile = Some("workspace".to_string());

        let call = ToolCall {
            id: "deny2".into(),
            name: "shell_command".into(),
            input: serde_json::json!({ "command": "touch /tmp/x" }),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(result.is_error);
        assert!(
            result.content.into_string().starts_with("SANDBOX_DENIED:"),
            "denial must surface for require_escalated"
        );
        assert_eq!(
            *attempts.lock().expect("attempts lock"),
            vec![Some("workspace".to_string())],
            "must not silent unsandbox without already_approved"
        );
    }

    #[tokio::test]
    async fn execute_single_skips_unsandbox_retry_when_profile_has_deny_read() {
        struct SandboxAwareTool {
            spec: ToolSpec,
            attempts: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        }

        #[async_trait]
        impl ToolHandler for SandboxAwareTool {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            async fn handle(
                &self,
                ctx: ToolContext,
                _input: serde_json::Value,
                _progress: Option<ToolProgressSender>,
            ) -> Result<ToolResult, ToolCallError> {
                self.attempts
                    .lock()
                    .expect("attempts lock")
                    .push(ctx.sandbox_profile.clone());
                Ok(ToolResult::error(
                    ToolResultContent::Text(
                        "SANDBOX_DENIED: The command was blocked by the OS sandbox.".into(),
                    ),
                    "denied",
                    ToolCallError::ExecutionFailed("SANDBOX_DENIED".into()),
                ))
            }
        }

        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "shell_command",
            Arc::new(SandboxAwareTool {
                spec: ToolSpec {
                    name: "shell_command".into(),
                    description: String::new(),
                    input_schema: JsonSchema::object(Default::default(), None, None),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::Mutating,
                    capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                    supports_parallel: false,
                    preparation_feedback: ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                },
                attempts: Arc::clone(&attempts),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "shell_command".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());

        let checker = PermissionChecker::new(|_| {
            Box::pin(async {
                Ok(PermissionGrant::from_approval(
                    &SandboxPermissionRequest::Default,
                ))
            })
        });

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path();
        let config_dir = workspace.join(".devo");
        std::fs::create_dir_all(&config_dir).expect("mkdir");
        std::fs::write(
            config_dir.join("sandbox.toml"),
            r#"
[profiles.locked]
extends = "workspace"
deny = ["/etc/passwd"]
"#,
        )
        .expect("write config");

        let mut runtime = ToolRuntime::new(registry, checker);
        runtime.context.cwd = workspace.to_path_buf();
        runtime.context.sandbox_profile = Some("locked".to_string());

        let call = ToolCall {
            id: "deny3".into(),
            name: "shell_command".into(),
            input: serde_json::json!({ "command": "cat /etc/passwd" }),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(result.is_error);
        assert_eq!(
            *attempts.lock().expect("attempts lock"),
            vec![Some("locked".to_string())],
            "deny-read profiles must not silent-unsandbox after SANDBOX_DENIED"
        );
    }

    fn test_permission_request(tool_name: &str) -> ToolPermissionRequest {
        ToolPermissionRequest {
            tool_call_id: "call".into(),
            tool_name: tool_name.into(),
            input: serde_json::json!({}),
            cwd: std::path::PathBuf::new(),
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            resource: devo_safety::ResourceKind::Custom(tool_name.into()),
            action_summary: tool_name.into(),
            justification: None,
            path: None,
            host: None,
            target: None,
            command_prefix: None,
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: SandboxPermissionRequest::Default,
        }
    }

    #[tokio::test]
    async fn runtime_concurrent_then_sequential() {
        // Two parallel tools followed by a sequential tool should still work
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let calls = vec![
            ToolCall {
                id: "r1".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "r2".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "w1".into(),
                name: "write_tool".into(),
                input: serde_json::json!({}),
            },
        ];
        let results = runtime.execute_batch(&calls).await;
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| !r.is_error));
        // Order should be preserved (parallel tools first, then sequential)
        assert_eq!(results[0].tool_use_id, "r1".to_string());
        assert_eq!(results[1].tool_use_id, "r2".to_string());
    }

    #[tokio::test]
    async fn parallel_completion_callback_streams_before_batch_is_done_but_results_stay_ordered() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let calls = vec![
            ToolCall {
                id: "slow".into(),
                name: "delayed_read_tool".into(),
                input: serde_json::json!({
                    "delay_ms": 50,
                    "output": "slow output",
                }),
            },
            ToolCall {
                id: "fast".into(),
                name: "delayed_read_tool".into(),
                input: serde_json::json!({
                    "delay_ms": 5,
                    "output": "fast output",
                }),
            },
        ];
        let completions = Arc::new(std::sync::Mutex::new(Vec::new()));
        let completions_clone = Arc::clone(&completions);

        let results = runtime
            .execute_batch_streaming_with_completion(
                &calls,
                |_tool_use_id, _content| Box::pin(async {}),
                move |result| {
                    let completions_clone = Arc::clone(&completions_clone);
                    Box::pin(async move {
                        completions_clone
                            .lock()
                            .expect("lock completions")
                            .push(result.tool_use_id.clone());
                    })
                },
            )
            .await;

        assert_eq!(
            completions.lock().expect("lock completions").as_slice(),
            &["fast".to_string(), "slow".to_string()]
        );
        assert_eq!(
            results
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>(),
            vec!["slow", "fast"]
        );
    }

    #[tokio::test]
    async fn runtime_empty_batch() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let results = runtime.execute_batch(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn runtime_single_tool() {
        let registry = make_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
        assert_eq!(result.tool_use_id, "c1");
    }

    // --- Streaming tests ---

    struct StreamingHandler {
        chunks: Vec<String>,
        spec: ToolSpec,
    }

    impl StreamingHandler {
        fn new(chunks: Vec<String>) -> Self {
            Self {
                spec: ToolSpec::new(
                    "stream_tool",
                    "stream",
                    JsonSchema::object(Default::default(), None, None),
                ),
                chunks,
            }
        }
    }

    #[async_trait]
    impl ToolHandler for StreamingHandler {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }
        async fn handle(
            &self,
            _ctx: ToolContext,
            _input: serde_json::Value,
            progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            if let Some(progress) = progress {
                for chunk in &self.chunks {
                    let _ = progress.send(crate::contracts::ToolProgress::OutputDelta {
                        delta: chunk.clone(),
                    });
                }
            }
            Ok(ToolResult::success(
                ToolResultContent::Text(self.chunks.join("")),
                "done",
            ))
        }
    }

    fn make_streaming_registry() -> Arc<ToolRegistry> {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "stream_tool",
            Arc::new(StreamingHandler::new(vec!["hello ".into(), "world".into()])),
        );
        builder.push_spec(ToolSpec {
            name: "stream_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        Arc::new(builder.build())
    }

    #[tokio::test]
    async fn execute_single_receives_progress() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "s1".into(),
            name: "stream_tool".into(),
            input: serde_json::json!({}),
        };

        let result = runtime.execute_single(&call, &None).await;
        assert!(!result.is_error);
        assert_eq!(result.content.into_string(), "hello world");
    }

    #[tokio::test]
    async fn execute_batch_streaming_receives_progress() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "s1".into(),
            name: "stream_tool".into(),
            input: serde_json::json!({}),
        };
        let progress_items = Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_items_for_callback = Arc::clone(&progress_items);

        let results = runtime
            .execute_batch_streaming(&[call], move |tool_use_id, progress| {
                let progress_items_for_callback = Arc::clone(&progress_items_for_callback);
                Box::pin(async move {
                    let ToolProgress::OutputDelta { delta } = progress else {
                        return;
                    };
                    progress_items_for_callback
                        .lock()
                        .expect("progress lock")
                        .push(format!("{tool_use_id}:{delta}"));
                })
            })
            .await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert_eq!(results[0].content.clone().into_string(), "hello world");
        assert_eq!(
            *progress_items.lock().expect("progress lock"),
            vec!["s1:hello ".to_string(), "s1:world".to_string()]
        );
    }

    #[tokio::test]
    async fn execute_batch_streaming_empty() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let results = runtime
            .execute_batch_streaming(&[], |_, _| Box::pin(async {}))
            .await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn execute_batch_streaming_unknown_tool() {
        let registry = make_streaming_registry();
        let runtime = ToolRuntime::new_without_permissions(registry);
        let call = ToolCall {
            id: "x1".into(),
            name: "nonexistent".into(),
            input: serde_json::json!({}),
        };
        let results = runtime
            .execute_batch_streaming(&[call], |_, _| Box::pin(async {}))
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedContextOptions {
        output_limit_bytes: usize,
        wall_time_limit_ms: Option<u64>,
        cancel_token_cancelled: bool,
        agent_coordinator_configured: bool,
        agent_scope: ToolAgentScope,
    }

    struct ContextCaptureTool {
        spec: ToolSpec,
        seen: Arc<std::sync::Mutex<Option<CapturedContextOptions>>>,
    }

    impl ContextCaptureTool {
        fn new(seen: Arc<std::sync::Mutex<Option<CapturedContextOptions>>>) -> Self {
            Self {
                spec: ToolSpec::new(
                    "capture_context",
                    "capture context",
                    JsonSchema::object(Default::default(), None, None),
                ),
                seen,
            }
        }
    }

    #[async_trait]
    impl ToolHandler for ContextCaptureTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        async fn handle(
            &self,
            ctx: ToolContext,
            _input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            *self.seen.lock().expect("seen lock") = Some(CapturedContextOptions {
                output_limit_bytes: ctx.budgets.output_limit_bytes,
                wall_time_limit_ms: ctx.budgets.wall_time_limit_ms,
                cancel_token_cancelled: ctx.cancel_token.is_cancelled(),
                agent_coordinator_configured: ctx.agent_coordinator.is_some(),
                agent_scope: ctx.agent_scope,
            });
            Ok(ToolResult::success(
                ToolResultContent::Text("captured".into()),
                "captured",
            ))
        }
    }

    #[tokio::test]
    async fn runtime_passes_custom_execution_options_to_tool_context() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "capture_context",
            Arc::new(ContextCaptureTool::new(Arc::clone(&seen))),
        );
        builder.push_spec(ToolSpec {
            name: "capture_context".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::new(builder.build()),
            PermissionChecker::always_allow(),
            ToolRuntimeContext::default(),
            ToolExecutionOptions {
                budgets: ToolBudgets {
                    output_limit_bytes: 7,
                    wall_time_limit_ms: Some(11),
                },
                cancel_token: CancellationToken::new(),
                on_tool_execution_start: None,
            },
        );
        let call = ToolCall {
            id: "ctx".into(),
            name: "capture_context".into(),
            input: serde_json::json!({}),
        };

        let result = runtime.execute_single(&call, &None).await;

        assert!(!result.is_error);
        assert_eq!(
            *seen.lock().expect("seen lock"),
            Some(CapturedContextOptions {
                output_limit_bytes: 7,
                wall_time_limit_ms: Some(11),
                cancel_token_cancelled: false,
                agent_coordinator_configured: false,
                agent_scope: ToolAgentScope::Parent,
            })
        );
    }

    #[tokio::test]
    async fn runtime_cancels_tool_when_cancel_token_already_fired() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "capture_context",
            Arc::new(ContextCaptureTool::new(Arc::clone(&seen))),
        );
        builder.push_spec(ToolSpec {
            name: "capture_context".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::new(builder.build()),
            PermissionChecker::always_allow(),
            ToolRuntimeContext::default(),
            ToolExecutionOptions {
                budgets: ToolBudgets {
                    output_limit_bytes: 7,
                    wall_time_limit_ms: Some(11),
                },
                cancel_token,
                on_tool_execution_start: None,
            },
        );
        let call = ToolCall {
            id: "ctx".into(),
            name: "capture_context".into(),
            input: serde_json::json!({}),
        };

        let result = runtime.execute_single(&call, &None).await;

        assert!(result.is_error);
        assert_eq!(
            result.content.into_string(),
            INTERRUPTED_TOOL_RESULT_MESSAGE
        );
        assert!(seen.lock().expect("seen lock").is_none());
    }

    #[tokio::test]
    async fn runtime_interrupts_hanging_tool_via_cancel_token() {
        #[derive(Debug)]
        struct HangingTool;

        #[async_trait]
        impl ToolHandler for HangingTool {
            fn spec(&self) -> &ToolSpec {
                Box::leak(Box::new(ToolSpec {
                    name: "hanging".into(),
                    description: String::new(),
                    input_schema: JsonSchema::object(Default::default(), None, None),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::ReadOnly,
                    capability_tags: vec![],
                    supports_parallel: true,
                    preparation_feedback: ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                }))
            }

            async fn handle(
                &self,
                _ctx: ToolContext,
                _input: serde_json::Value,
                _progress: Option<ToolProgressSender>,
            ) -> Result<ToolResult, ToolCallError> {
                std::future::pending::<()>().await;
                unreachable!("hanging tool should be cancelled")
            }
        }

        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("hanging", Arc::new(HangingTool));
        builder.push_spec(ToolSpec {
            name: "hanging".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let cancel_token = CancellationToken::new();
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::new(builder.build()),
            PermissionChecker::always_allow(),
            ToolRuntimeContext::default(),
            ToolExecutionOptions {
                cancel_token: cancel_token.clone(),
                ..ToolExecutionOptions::default()
            },
        );
        let call = ToolCall {
            id: "hang-1".into(),
            name: "hanging".into(),
            input: serde_json::json!({}),
        };

        let execute = runtime.execute_single(&call, &None);
        tokio::pin!(execute);
        tokio::select! {
            _ = &mut execute => panic!("hanging tool should not complete before cancel"),
            () = tokio::time::sleep(Duration::from_millis(10)) => {
                cancel_token.cancel();
            }
        }
        let result = execute.await;
        assert!(result.is_error);
        assert_eq!(
            result.content.into_string(),
            INTERRUPTED_TOOL_RESULT_MESSAGE
        );
    }

    #[derive(Debug, Default)]
    struct FakeAgentCoordinator;

    #[async_trait]
    impl devo_tools::AgentToolCoordinator for FakeAgentCoordinator {
        async fn spawn_agent(
            self: Arc<Self>,
            _params: devo_protocol::SpawnAgentParams,
        ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn send_message(
            self: Arc<Self>,
            _params: devo_protocol::AgentMessageParams,
        ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn wait_agent(
            self: Arc<Self>,
            _params: devo_protocol::WaitAgentParams,
        ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn list_agents(
            self: Arc<Self>,
            _params: devo_protocol::AgentListParams,
        ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn close_agent(
            self: Arc<Self>,
            _params: devo_protocol::CloseAgentParams,
        ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }
    }

    #[tokio::test]
    async fn runtime_passes_agent_coordinator_to_tool_context() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "capture_context",
            Arc::new(ContextCaptureTool::new(Arc::clone(&seen))),
        );
        builder.push_spec(ToolSpec {
            name: "capture_context".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::new(builder.build()),
            PermissionChecker::always_allow(),
            ToolRuntimeContext {
                agent_coordinator: Some(
                    Arc::new(FakeAgentCoordinator) as Arc<dyn devo_tools::AgentToolCoordinator>
                ),
                ..ToolRuntimeContext::default()
            },
            ToolExecutionOptions::default(),
        );
        let call = ToolCall {
            id: "ctx".into(),
            name: "capture_context".into(),
            input: serde_json::json!({}),
        };

        let result = runtime.execute_single(&call, &None).await;

        assert!(!result.is_error);
        assert!(
            seen.lock()
                .expect("seen lock")
                .as_ref()
                .is_some_and(|context| context.agent_coordinator_configured)
        );
    }
}
