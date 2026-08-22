use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::Utc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use devo_core::ItemId;
use devo_core::Message;
use devo_core::ResponseItem;
use devo_core::SessionId;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TextItem;
use devo_core::TokenInfo;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnStatus;
use devo_core::TurnUsage;
use devo_core::Worklog;
use devo_core::history::compaction::CompactAction;
use devo_core::history::compaction::CompactionConfig;
use devo_core::history::compaction::CompactionKind;
use devo_core::history::compaction::compact_history;
use devo_core::history::summarizer::DefaultHistorySummarizer;
use devo_core::message_to_response_items;
use devo_core::tools::AgentToolCoordinator;
use devo_core::tools::PermissionChecker;
use devo_core::tools::PermissionGrant;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolPermissionRequest;
use devo_protocol::{
    SessionDeletedPayload, WorkspaceChangeAttribution, WorkspaceChangeScope, WorkspaceChangeView,
    WorkspaceChangesReadParams, WorkspaceChangesReadResult, WorkspaceChangesUpdatedPayload,
    WorkspaceDiffDetail,
};
use devo_safety::PermissionMode;

use crate::ApprovalDecisionValue;
use crate::ApprovalScopeValue;
use crate::ClientTransportKind;
use crate::ConnectionState;
use crate::ErrorResponse;
use crate::EventContext;
use crate::InitializeResult;
use crate::ItemDeltaKind;
use crate::ItemEnvelope;
use crate::ItemEventPayload;
use crate::ItemKind;
use crate::ProtocolError;
use crate::ProtocolErrorCode;
use crate::ProtocolExposurePolicy;
use crate::ProtocolSet;
use crate::RequestUserInputArgs;
use crate::RequestUserInputPayload;
use crate::RequestUserInputResponse;
use crate::ServerEvent;
use crate::ServerProtocol;
use crate::ServerRequestResolvedPayload;
use crate::SessionCompactionFailedPayload;
use crate::SessionEffectiveContextWindowUpdatedPayload;
use crate::SessionEventPayload;
use crate::SessionForkParams;
use crate::SessionForkResult;
use crate::SessionMetadata;
use crate::SessionResumeParams;
use crate::SessionResumeResult;
use crate::SessionRuntimeStatus;
use crate::SessionStartParams;
use crate::SessionStartResult;
use crate::SessionStatusChangedPayload;
use crate::SuccessResponse;
use crate::TurnEventPayload;
use crate::TurnInterruptParams;
use crate::TurnInterruptResult;
use crate::TurnMetadata;
use crate::TurnStartParams;
use crate::TurnStartResult;
use crate::TurnUsageUpdatedPayload;
use crate::approval_reviewer::build_approval_review_request;
use crate::approval_reviewer::extend_approval_review_request;
use crate::approval_reviewer::parse_reviewer_decision;
use crate::db::QueueType;
use crate::execution::PendingUserInput;
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::goal::Goal;
use crate::goal::GoalAction;
use crate::goal::GoalId;
use crate::goal::GoalMutation;
use crate::goal_durable::GoalDurableStore;
use crate::persistence::RolloutStore;
use crate::persistence::build_item_record;
use crate::projection::history_item_from_turn_item;
pub(crate) use crate::runtime::handlers::goal::GoalStore;
use crate::subagent::AgentPath;
use crate::subagent::AgentRegistry;
use crate::subagent::SubagentMailbox;
use crate::subagent::SubagentMetadata;
use crate::subagent::SubagentOutputBuffer;
use crate::subagent::SubagentStatus;
use crate::usage_ledger::UsageLedger;
use crate::workspace_changes::ActiveWorkspaceBaseline;

mod acp_fs;
mod active_turn;
mod agents;
mod approval;
mod command_exec;
mod compaction_persist;
pub(crate) use compaction_persist::CompactionSummaryPersist;
pub(crate) use compaction_persist::append_compaction_summary_and_snapshot;
pub(crate) use compaction_persist::build_compaction_snapshot_line;
pub(crate) use compaction_persist::compaction_persisted_turn_item;
pub(crate) use compaction_persist::preserved_item_ids_from_compacted;
pub(crate) use compaction_persist::summary_turn_item_from_compacted;
mod connection;
pub(crate) mod context_occupancy;
mod context_usage;
mod control_requests;
mod goal_accounting;
mod goal_continuation;
mod goal_handlers;
mod handlers;
mod hooks;
mod interaction_items;
mod items;
mod lifecycle;
mod mcp;
mod model_api;
mod outbound;
mod permission_decision;
mod proposed_plan;
mod provider_vendor_api;
mod reference_search;
mod session_actor;
mod session_cache;
mod session_interactive;
mod skills;
mod subagent_usage;
mod turn_exec;
mod turn_lifecycle;
mod turn_reservation;
mod user_input;
mod workspace_baseline;

pub(crate) use connection::ConnectionRuntime;
pub(crate) use connection::INBOUND_CONCURRENCY_LIMIT;
pub use connection::IncomingResponse;
pub use connection::PostResponseActions;
pub(crate) use items::render_input_items;
pub(crate) use outbound::OUTBOUND_CHANNEL_CAPACITY;
pub use outbound::OutboundFrame;
pub(crate) use outbound::enqueue_outbound;
pub(crate) use outbound::log_outbound_frame;
pub(crate) use outbound::outbound_frame_to_value;
pub use outbound::test_outbound_channel;
use session_actor::SessionHandle;
use session_interactive::SessionInteractiveLanes;
use turn_exec::ExecuteTurnRequest;

pub(crate) use session_actor::SessionActorState;

pub struct ServerRuntime {
    metadata: InitializeResult,
    protocol_exposure: ProtocolExposurePolicy,
    deps: ServerRuntimeDependencies,
    rollout_store: RolloutStore,
    goal_durable_store: GoalDurableStore,
    usage_ledger: UsageLedger,
    /// Per-session actor handles; map lock must not be held across await.
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    /// Interactive approval and user-input waits outside session actors.
    session_interactive: SessionInteractiveLanes,
    /// New-style (`subscription/*`) subscriptions keyed by subscription id
    /// (08 §4). Lock order: `connections` → `event_subscriptions`.
    event_subscriptions: Mutex<HashMap<String, handlers::subscription::EventSubscription>>,
    /// Count of active `SessionsByCwd` selectors; gates per-event cwd
    /// resolution during broadcast (0 = skip the lookup entirely).
    sessions_by_cwd_subscriptions: std::sync::atomic::AtomicUsize,
    /// In-flight turn execution handles keyed by session id.
    active_turns: active_turn::ActiveTurnRegistry,
    connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
    terminal_turn_statuses: Mutex<VecDeque<(TurnId, TerminalTurnSnapshot)>>,
    acp_prompt_waiters: Mutex<HashMap<TurnId, Vec<oneshot::Sender<TerminalTurnSnapshot>>>>,
    active_goal_continuation_turns: Mutex<HashMap<SessionId, TurnId>>,
    goal_continuation_turn_goals: Mutex<HashMap<TurnId, GoalId>>,
    next_connection_id: AtomicU64,
    /// Per-session goal stores for goal lifecycle management.
    goal_stores: Mutex<HashMap<SessionId, GoalStore>>,
    /// Per-root-session agent registries for subagent coordination.
    agent_registries: Mutex<HashMap<SessionId, AgentRegistry>>,
    /// Per-session inboxes used by agent tools to exchange ordered messages.
    agent_mailboxes: Mutex<HashMap<SessionId, SubagentMailbox>>,
    /// Per-parent child-output buffers used by wait_agent polling.
    agent_output_buffers: Mutex<HashMap<SessionId, SubagentOutputBuffer>>,
    /// Per-parent `wait_agent` sequence cursors keyed by optional target string.
    agent_wait_cursors: Mutex<HashMap<SessionId, HashMap<String, u64>>>,
    /// Latest subagent turn usage grouped under the parent turn that requested the work.
    subagent_usage: Mutex<subagent_usage::SubagentUsageState>,
    /// Live client-owned reference search sessions.
    reference_searches:
        Mutex<HashMap<devo_protocol::ReferenceSearchId, reference_search::ReferenceSearchState>>,
    /// Live client-owned shell/process sessions.
    command_exec_manager: command_exec::CommandExecManager,
    /// Turn-scoped workspace baselines captured at actual execution start.
    active_workspace_baselines: Mutex<HashMap<TurnId, ActiveWorkspaceBaseline>>,
    /// In-process idempotency for canonical `turn/start`
    /// (`(session, idempotencyKey) -> turn`). Retry-safe within the process;
    /// cross-restart dedup is a documented follow-up (L2-DES-APP-008 Phase B).
    turn_start_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::turn::Turn>>,
    /// In-process idempotency for canonical `session/new`
    /// (`idempotencyKey -> session`). Same process-scope caveat as
    /// `turn_start_idempotency`.
    session_new_idempotency: Mutex<HashMap<String, SessionId>>,
    /// In-process idempotency for canonical `session/goal/set`
    /// (`(session, idempotencyKey) -> goal`).
    goal_set_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::goal::Goal>>,
    /// In-process idempotency for canonical `session/goal/update` (ratified
    /// #3 in-place edit; same keying as goal/set).
    goal_update_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::goal::Goal>>,
    /// In-process idempotency for canonical `task/start` (`(session,
    /// idempotencyKey) -> item id as process id`).
    task_start_idempotency: Mutex<HashMap<(SessionId, String), String>>,
    /// Short-lived, connection-bound P4d rollback plans.
    restore_plans: Mutex<handlers::rollback_plan::RestorePlanStore>,
    /// Sessions with an in-flight model title-generation task.
    title_generation_in_flight: Mutex<HashSet<SessionId>>,
    /// Weak back-reference used when session actors need the owning runtime `Arc`.
    self_weak: std::sync::Weak<ServerRuntime>,
    /// LRU order for loaded root session actors.
    session_lru: Mutex<session_cache::ParentSessionLru>,
    /// Per-session gate that serializes lazy parent session hydration.
    parent_session_load_gate: Arc<session_cache::SessionLoadGate>,
    /// User exec-policy rules loaded from `$DEVO_HOME/rules/*.rules`.
    user_exec_policy: std::sync::Mutex<Option<devo_execpolicy::Policy>>,
    /// Localhost HTTP CONNECT proxy for restricted sandbox profiles.
    sandbox_network_proxy: std::sync::Arc<
        std::sync::Mutex<Option<devo_sandbox_network_proxy::SharedSandboxNetworkProxyHandle>>,
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnInputMode {
    VisibleUserMessage,
    HiddenGoalContinuation { goal: devo_protocol::ThreadGoal },
}

const TERMINAL_TURN_STATUS_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalTurnSnapshot {
    pub(crate) status: TurnStatus,
    pub(crate) stop_reason: Option<devo_core::StopReason>,
    pub(crate) failure_reason: Option<devo_protocol::TurnFailureReason>,
}

impl TerminalTurnSnapshot {
    pub(crate) fn from_turn(turn: &TurnMetadata) -> Self {
        Self {
            status: turn.status.clone(),
            stop_reason: turn.stop_reason.clone(),
            failure_reason: turn.failure_reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnStartQueuePolicy {
    Queue,
    RejectActive,
}

impl TurnInputMode {
    fn emits_user_message(&self) -> bool {
        matches!(self, Self::VisibleUserMessage)
    }
}

fn session_model_selection(session: &SessionMetadata) -> Option<&str> {
    session
        .model_binding_id
        .as_deref()
        .or(session.model.as_deref())
}

fn requested_model_selection<'a>(
    model_binding_id: Option<&'a str>,
    model: Option<&'a str>,
    session: &'a SessionMetadata,
) -> Option<&'a str> {
    model_binding_id
        .or(model)
        .or_else(|| session_model_selection(session))
}

const SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: &str = "devo_subagent_usage_parent_session_id";
const SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: &str = "devo_subagent_usage_parent_turn_id";

pub(super) fn subagent_usage_owner_pending_metadata(
    parent_session_id: SessionId,
    parent_turn_id: Option<TurnId>,
) -> serde_json::Value {
    serde_json::json!({
        SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: parent_session_id.to_string(),
        SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: parent_turn_id.map(|turn_id| turn_id.to_string()),
    })
}

impl ServerRuntime {
    pub fn new(server_home: PathBuf, deps: ServerRuntimeDependencies) -> Arc<Self> {
        // Embedded callers historically exposed both surfaces. The server
        // process always calls `with_protocols` with its explicit startup
        // policy, whose CLI default is Native-only.
        Self::with_protocols(server_home, deps, ProtocolSet::all())
    }

    pub fn with_protocols(
        server_home: PathBuf,
        deps: ServerRuntimeDependencies,
        protocols: ProtocolSet,
    ) -> Arc<Self> {
        let rollout_store = RolloutStore::new(server_home.clone(), Some(Arc::clone(&deps.db)));
        let goal_durable_store = GoalDurableStore::with_primary(
            server_home.clone(),
            rollout_store.clone(),
            Arc::clone(&deps.db),
        );
        let usage_ledger = UsageLedger::new(rollout_store.clone(), Arc::clone(&deps.db));
        let sandbox_network_proxy = std::sync::Arc::new(std::sync::Mutex::new(None));
        // Proxy startup is async; ports are published via the thread-safe
        // `set_sandbox_proxy_ports` store (not process-wide `env::set_var`).
        if let Ok(runtime_handle) = tokio::runtime::Handle::try_current() {
            let proxy_slot = sandbox_network_proxy.clone();
            runtime_handle.spawn(async move {
                match devo_sandbox_network_proxy::start_sandbox_network_proxy().await {
                    Ok(handle) => {
                        tracing::info!(
                            http_port = handle.http_port(),
                            "started sandbox network proxy"
                        );
                        *proxy_slot.lock().expect("proxy slot mutex") =
                            Some(std::sync::Arc::new(handle));
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "failed to start sandbox network proxy; restricted profiles keep network isolation without managed proxy egress"
                        );
                    }
                }
            });
        } else {
            tracing::warn!(
                "no tokio runtime available at ServerRuntime::new; sandbox network proxy not started"
            );
        }
        Arc::new_cyclic(|self_weak| Self {
            metadata: InitializeResult {
                server_name: "devo-server".into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                platform_family: std::env::consts::FAMILY.into(),
                platform_os: std::env::consts::OS.into(),
                server_home,
            },
            protocol_exposure: ProtocolExposurePolicy::new(protocols),
            deps,
            rollout_store,
            goal_durable_store,
            usage_ledger,
            sessions: Mutex::new(HashMap::new()),
            session_interactive: SessionInteractiveLanes::default(),
            event_subscriptions: Mutex::new(HashMap::new()),
            sessions_by_cwd_subscriptions: std::sync::atomic::AtomicUsize::new(0),
            active_turns: active_turn::ActiveTurnRegistry::default(),
            connections: Arc::new(Mutex::new(HashMap::new())),
            terminal_turn_statuses: Mutex::new(VecDeque::new()),
            acp_prompt_waiters: Mutex::new(HashMap::new()),
            active_goal_continuation_turns: Mutex::new(HashMap::new()),
            goal_continuation_turn_goals: Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            goal_stores: Mutex::new(HashMap::new()),
            agent_registries: Mutex::new(HashMap::new()),
            agent_mailboxes: Mutex::new(HashMap::new()),
            agent_output_buffers: Mutex::new(HashMap::new()),
            agent_wait_cursors: Mutex::new(HashMap::new()),
            subagent_usage: Mutex::new(subagent_usage::SubagentUsageState::default()),
            reference_searches: Mutex::new(HashMap::new()),
            command_exec_manager: command_exec::CommandExecManager::new(),
            active_workspace_baselines: Mutex::new(HashMap::new()),
            turn_start_idempotency: Mutex::new(HashMap::new()),
            session_new_idempotency: Mutex::new(HashMap::new()),
            goal_set_idempotency: Mutex::new(HashMap::new()),
            goal_update_idempotency: Mutex::new(HashMap::new()),
            task_start_idempotency: Mutex::new(HashMap::new()),
            restore_plans: Mutex::new(HashMap::new()),
            title_generation_in_flight: Mutex::new(HashSet::new()),
            self_weak: self_weak.clone(),
            session_lru: Mutex::new(session_cache::ParentSessionLru::new(
                session_cache::PARENT_SESSION_LRU_CAPACITY,
            )),
            parent_session_load_gate: Arc::new(session_cache::SessionLoadGate::default()),
            user_exec_policy: std::sync::Mutex::new(
                crate::exec_policy_store::load_user_exec_policy(),
            ),
            sandbox_network_proxy,
        })
    }

    pub async fn enabled_protocols(&self) -> ProtocolSet {
        self.protocol_exposure.enabled().await
    }

    pub async fn protocol_enabled(&self, protocol: ServerProtocol) -> bool {
        self.protocol_exposure.allows(protocol).await
    }

    pub async fn enable_protocols(&self, protocols: &ProtocolSet) -> ProtocolSet {
        self.protocol_exposure.enable(protocols).await
    }

    pub fn sandbox_network_proxy(
        &self,
    ) -> Option<devo_sandbox_network_proxy::SharedSandboxNetworkProxyHandle> {
        self.sandbox_network_proxy
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// The rollout store, for the startup event-log reconciler (08 §7).
    pub(crate) fn rollout_store(&self) -> RolloutStore {
        self.rollout_store.clone()
    }

    /// The shared SQLite handle (session index, queues, event log).
    pub(crate) fn deps_db(&self) -> Arc<crate::db::Database> {
        Arc::clone(&self.deps.db)
    }
}

fn permission_mode_from_approval_policy(policy: &str) -> Option<PermissionMode> {
    match policy {
        "on-request" | "interactive" | "ask" => Some(PermissionMode::Interactive),
        "never" | "auto" | "auto-approve" | "yolo" => Some(PermissionMode::Yolo),
        "deny" => Some(PermissionMode::Deny),
        _ => None,
    }
}

fn safety_profile_from_protocol(
    preset: devo_protocol::PermissionPreset,
    cwd: std::path::PathBuf,
    additional_directories: Vec<std::path::PathBuf>,
) -> devo_safety::RuntimePermissionProfile {
    let preset = match preset {
        devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
        devo_protocol::PermissionPreset::AutoReview => devo_safety::PermissionPreset::AutoReview,
        devo_protocol::PermissionPreset::FullAccess => devo_safety::PermissionPreset::FullAccess,
    };
    devo_safety::RuntimePermissionProfile::from_preset(preset, cwd)
        .with_additional_roots(additional_directories)
}

pub(crate) fn protocol_preset_from_safety(
    preset: devo_safety::PermissionPreset,
) -> devo_protocol::PermissionPreset {
    match preset {
        devo_safety::PermissionPreset::Default => devo_protocol::PermissionPreset::Default,
        devo_safety::PermissionPreset::AutoReview => devo_protocol::PermissionPreset::AutoReview,
        devo_safety::PermissionPreset::FullAccess => devo_protocol::PermissionPreset::FullAccess,
    }
}
