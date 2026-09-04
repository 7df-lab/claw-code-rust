use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use devo_config::ResolvedWebFetchConfig;
use devo_config::ResolvedWebSearchConfig;
use devo_provider::ProviderRoute;
use devo_safety::PermissionMode;
use devo_safety::PermissionPreset;
use devo_safety::RuntimePermissionProfile;

use devo_protocol::CollaborationMode;
use devo_protocol::PendingInputItem;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::TurnKind;
use serde_json::Value;

use crate::AgentsMdConfig;
use crate::Message;
use crate::Model;
use crate::SessionContext;
use crate::TokenBudget;
use crate::TurnContext;
use crate::state::turn::TurnState;

/// Configuration for a session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub token_budget: TokenBudget,
    /// Session-scoped absolute effective context window override.
    /// When set, turn starts merge this into the model-derived budget so hot
    /// updates are not wiped by `TurnConfig::token_budget()`. Clamped to the
    /// active model `context_window` at resolve time.
    pub effective_context_window_override: Option<usize>,
    pub permission_mode: PermissionMode,
    pub permission_profile: RuntimePermissionProfile,
    pub agents_md: AgentsMdConfig,
    pub available_skills_instructions: Option<String>,
    /// Active sandbox profile name for child processes spawned by tools.
    /// `None` means no sandboxing; otherwise the value is a profile name such
    /// as `"workspace"`, `"strict"`, or `"off"`.
    pub sandbox_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGoalState {
    pub goal: ThreadGoal,
}

impl SessionGoalState {
    pub fn new(goal: ThreadGoal) -> Self {
        Self { goal }
    }

    pub fn context_prompt(&self) -> Option<String> {
        crate::render_goal_continuation_prompt(&self.goal)
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let permission_profile =
            RuntimePermissionProfile::from_preset(PermissionPreset::Default, cwd);
        Self {
            token_budget: TokenBudget::default(),
            effective_context_window_override: None,
            permission_mode: permission_profile.permission_mode(),
            permission_profile,
            agents_md: AgentsMdConfig::default(),
            available_skills_instructions: None,
            sandbox_profile: Some("workspace".to_string()),
        }
    }
}

/// Per-turn execution settings resolved before the query loop starts.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Catalog model keyed by `model_slug`; used for prompts, capabilities,
    /// reasoning metadata, context limits, session metadata, and UI state.
    pub model: Model,
    /// Provider wire model identifier from the selected binding's `request_model`.
    /// This is the string sent as `ModelRequest.model` for the base model.
    pub request_model: String,
    /// Provider model binding id selected for this turn, when the request was
    /// resolved through configured provider bindings.
    pub model_binding_id: Option<String>,
    /// Provider-scoped variant lookup used when reasoning resolves to another
    /// catalog slug before the request is built.
    pub provider_request_models: ProviderRequestModelMap,
    /// Provider route selected by the model-provider binding for this turn.
    pub provider_route: ProviderRoute,
    /// Named provider/model variant selected for this turn, if any.
    pub variant: Option<String>,
    /// Effective web search behavior for this turn.
    pub web_search: ResolvedWebSearchConfig,
    /// Effective web fetch behavior for this turn.
    pub web_fetch: ResolvedWebFetchConfig,
    pub reasoning_effort_selection: Option<String>,
}

/// Provider request model names keyed by catalog model slug for one selected provider.
///
/// Example: the catalog slug `kimi-k2.5-thinking` can map to the provider wire
/// name `moonshotai/kimi-k2.5-thinking`. The map is provider-scoped so a
/// duplicate slug configured under another provider is ignored for this turn.
#[derive(Debug, Clone, Default)]
pub struct ProviderRequestModelMap {
    by_model_slug: HashMap<String, String>,
    request_defaults: Option<Value>,
    request_headers: BTreeMap<String, String>,
}

impl ProviderRequestModelMap {
    pub fn new(by_model_slug: HashMap<String, String>) -> Self {
        Self {
            by_model_slug,
            request_defaults: None,
            request_headers: BTreeMap::new(),
        }
    }

    /// Attaches provider/model request defaults resolved from providers.json.
    pub fn with_request_config(
        mut self,
        request_defaults: Option<Value>,
        request_headers: BTreeMap<String, String>,
    ) -> Self {
        self.request_defaults = request_defaults;
        self.request_headers = request_headers;
        self
    }

    pub fn request_defaults(&self) -> Option<&Value> {
        self.request_defaults.as_ref()
    }

    pub fn request_headers(&self) -> &BTreeMap<String, String> {
        &self.request_headers
    }

    pub fn get(&self, model_slug: &str) -> Option<&str> {
        self.by_model_slug.get(model_slug).map(String::as_str)
    }
}

impl From<HashMap<String, String>> for ProviderRequestModelMap {
    fn from(by_model_slug: HashMap<String, String>) -> Self {
        Self::new(by_model_slug)
    }
}

impl TurnConfig {
    pub fn token_budget(&self) -> TokenBudget {
        TokenBudget::for_model(&self.model)
    }

    /// Builds the turn token budget from the model effective window.
    ///
    /// Session effective-context overrides are ignored (product: one Context
    /// window stored as a ratio on the model). The parameter is retained so
    /// call sites can keep passing the config field without churn.
    pub fn token_budget_for_session(
        &self,
        _effective_context_window_override: Option<usize>,
    ) -> TokenBudget {
        self.token_budget()
    }

    pub fn new(model: Model, reasoning_effort_selection: Option<String>) -> Self {
        let request_model = model.slug.clone();
        let reasoning_effort_selection =
            model.normalize_reasoning_effort_selection(reasoning_effort_selection.as_deref());
        Self {
            model,
            request_model,
            model_binding_id: None,
            provider_request_models: ProviderRequestModelMap::default(),
            provider_route: ProviderRoute::Default,
            variant: None,
            web_search: ResolvedWebSearchConfig::Disabled,
            web_fetch: ResolvedWebFetchConfig::Local,
            reasoning_effort_selection,
        }
    }

    pub fn with_request_model(
        model: Model,
        request_model: String,
        provider_request_models: ProviderRequestModelMap,
        reasoning_effort_selection: Option<String>,
    ) -> Self {
        Self::with_provider_route(
            model,
            request_model,
            provider_request_models,
            ProviderRoute::Default,
            reasoning_effort_selection,
        )
    }

    pub fn with_provider_route(
        model: Model,
        request_model: String,
        provider_request_models: ProviderRequestModelMap,
        provider_route: ProviderRoute,
        reasoning_effort_selection: Option<String>,
    ) -> Self {
        Self::with_provider_route_and_web_search(
            model,
            request_model,
            provider_request_models,
            provider_route,
            ResolvedWebSearchConfig::Disabled,
            reasoning_effort_selection,
        )
    }

    pub fn with_provider_route_and_web_search(
        model: Model,
        request_model: String,
        provider_request_models: ProviderRequestModelMap,
        provider_route: ProviderRoute,
        web_search: ResolvedWebSearchConfig,
        reasoning_effort_selection: Option<String>,
    ) -> Self {
        Self::with_provider_route_and_web_tools(
            model,
            request_model,
            provider_request_models,
            provider_route,
            web_search,
            ResolvedWebFetchConfig::Local,
            reasoning_effort_selection,
        )
    }

    pub fn with_provider_route_and_web_tools(
        model: Model,
        request_model: String,
        provider_request_models: ProviderRequestModelMap,
        provider_route: ProviderRoute,
        web_search: ResolvedWebSearchConfig,
        web_fetch: ResolvedWebFetchConfig,
        reasoning_effort_selection: Option<String>,
    ) -> Self {
        let reasoning_effort_selection =
            model.normalize_reasoning_effort_selection(reasoning_effort_selection.as_deref());
        Self {
            model,
            request_model,
            model_binding_id: None,
            provider_request_models,
            provider_route,
            variant: None,
            web_search,
            web_fetch,
            reasoning_effort_selection,
        }
    }

    pub fn provider_request_model(&self, resolved_catalog_model: &str) -> String {
        if resolved_catalog_model == self.model.slug {
            return self.request_model.clone();
        }
        // Thinking may resolve the catalog model to a variant slug. Keep catalog
        // metadata from the variant, but translate the final request back to the
        // selected provider's `request_model` when a matching binding exists.
        self.provider_request_models
            .get(resolved_catalog_model)
            .map(str::to_string)
            .unwrap_or_else(|| resolved_catalog_model.to_string())
    }
}

/// Mutable state for one conversation session.
///
/// This corresponds to the session-level state in Claude Code's
/// `AppStateStore` and `QueryEngine`, but stripped of UI concerns.
pub struct SessionState {
    pub id: String,
    pub config: SessionConfig,
    pub messages: Vec<Message>,
    pub prompt_messages: Option<Vec<Message>>,
    pub session_context: Option<SessionContext>,
    pub latest_turn_context: Option<TurnContext>,
    pub active_goal: Option<SessionGoalState>,
    pub collaboration_mode: CollaborationMode,
    pub cwd: PathBuf,
    pub turn_count: usize,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize, // TODO: from Anthropic Messages API, indicate how many tokens utlized to create cache.
    pub total_cache_read_tokens: usize,     // TODO: same with `total_input_cached_tokens`.
    pub prompt_token_estimate: usize,
    /// Latest assembled-request category estimate (before provider scaling).
    pub raw_context_breakdown: Option<crate::RawContextBreakdown>,
    /// Input tokens reported by the model for the most recent turn.
    pub last_input_tokens: usize,
    /// Total context tokens reported by the model for the most recent turn.
    /// This includes input plus output and drives automatic compaction.
    pub last_turn_tokens: usize,
    /// True when the most recently finished turn was user-interrupted.
    /// Consumed when the next query inserts the `<turn_aborted>` notice.
    pub last_turn_interrupted: bool,
    /// Thread-safe queue for pending turn inputs.
    /// - Source: user sends `turn/start` while a turn is active.
    /// - Lifecycle: preserved across turns; unconsumed items are pushed back
    ///   when the current turn ends and consumed when the next turn starts.
    pub pending_turn_queue: Arc<Mutex<VecDeque<PendingInputItem>>>,
    /// Thread-safe queue for inputs steering the active turn.
    /// - Source: user sends `turn/steer` while a turn is active.
    /// - Lifecycle: scoped to current turn only; cleared when the turn ends.
    pub steer_input_queue: Arc<Mutex<VecDeque<PendingInputItem>>>,
    /// Turn-scoped state (Some while a turn is active).
    pub(crate) turn_state: Option<TurnState>,
}

impl SessionState {
    pub fn new(config: SessionConfig, cwd: PathBuf) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            config,
            messages: Vec::new(),
            prompt_messages: None,
            session_context: None,
            latest_turn_context: None,
            active_goal: None,
            collaboration_mode: CollaborationMode::Build,
            cwd,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            raw_context_breakdown: None,
            last_input_tokens: 0,
            last_turn_tokens: 0,
            last_turn_interrupted: false,
            pending_turn_queue: Arc::new(Mutex::new(VecDeque::new())),
            steer_input_queue: Arc::new(Mutex::new(VecDeque::new())),
            turn_state: None,
        }
    }

    /// Clones session state for read-only export without active turn bookkeeping.
    pub fn snapshot_for_export(&self) -> Self {
        Self {
            id: self.id.clone(),
            config: self.config.clone(),
            messages: self.messages.clone(),
            prompt_messages: self.prompt_messages.clone(),
            session_context: self.session_context.clone(),
            latest_turn_context: self.latest_turn_context.clone(),
            active_goal: self.active_goal.clone(),
            collaboration_mode: self.collaboration_mode,
            cwd: self.cwd.clone(),
            turn_count: self.turn_count,
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            total_tokens: self.total_tokens,
            total_cache_creation_tokens: self.total_cache_creation_tokens,
            total_cache_read_tokens: self.total_cache_read_tokens,
            prompt_token_estimate: self.prompt_token_estimate,
            raw_context_breakdown: self.raw_context_breakdown,
            last_input_tokens: self.last_input_tokens,
            last_turn_tokens: self.last_turn_tokens,
            last_turn_interrupted: self.last_turn_interrupted,
            pending_turn_queue: Arc::clone(&self.pending_turn_queue),
            steer_input_queue: Arc::clone(&self.steer_input_queue),
            turn_state: None,
        }
    }

    /// Marks that the previous turn ended as interrupted so the next query can
    /// insert an explicit aborted-turn notice.
    pub fn mark_last_turn_interrupted(&mut self) {
        self.last_turn_interrupted = true;
    }

    /// Takes and clears the interrupted-turn flag for the next query preamble.
    pub fn take_last_turn_interrupted(&mut self) -> bool {
        std::mem::take(&mut self.last_turn_interrupted)
    }

    pub fn push_message(&mut self, msg: Message) {
        if let Some(prompt_messages) = self.prompt_messages.as_mut() {
            self.messages.push(msg.clone());
            prompt_messages.push(msg);
        } else {
            self.messages.push(msg);
        }
    }

    pub fn to_request_messages(&self) -> Vec<devo_protocol::RequestMessage> {
        self.prompt_source_messages()
            .iter()
            .map(|m| m.to_request_message())
            .collect()
    }

    pub fn prompt_source_messages(&self) -> &[Message] {
        self.prompt_messages
            .as_deref()
            .unwrap_or(self.messages.as_slice())
    }

    pub fn set_prompt_messages(&mut self, messages: Vec<Message>) {
        self.prompt_messages = Some(messages);
    }

    pub fn clear_prompt_messages(&mut self) {
        self.prompt_messages = None;
    }

    pub fn set_active_goal(&mut self, goal: ThreadGoal) {
        self.active_goal =
            (goal.status == ThreadGoalStatus::Active).then(|| SessionGoalState::new(goal));
    }

    pub fn clear_active_goal(&mut self) {
        self.active_goal = None;
    }

    pub fn goal_context_prompt(&self) -> Option<String> {
        if self.collaboration_mode == CollaborationMode::Plan {
            return None;
        }
        self.active_goal
            .as_ref()
            .and_then(SessionGoalState::context_prompt)
    }

    pub fn insert_context_message(&mut self, msg: Message) {
        crate::history::insert_context_diff_message(&mut self.messages, msg.clone());
        if let Some(prompt_messages) = self.prompt_messages.as_mut() {
            crate::history::insert_context_diff_message(prompt_messages, msg);
        }
    }

    /// Drains all pending inputs from the active-turn steer queue.
    pub fn drain_steer_input_queue(&self) -> Vec<PendingInputItem> {
        let mut guard = self
            .steer_input_queue
            .lock()
            .expect("steer input queue mutex should not be poisoned");
        guard.drain(..).collect()
    }

    pub fn start_turn(&mut self, kind: TurnKind) {
        // The session turn queue is owned by the server-side drain (canonical
        // `session/queue/*` semantics): queued entries become follow-up turns
        // with their own drain notification and persistence bookkeeping, so
        // they must never be absorbed into a running or starting turn here.
        // Turn-scoped pending input starts empty and only collects transient
        // steering fragments produced during this turn (e.g. budget notices).
        self.turn_state = Some(TurnState::new(kind));
    }

    pub fn end_turn(&mut self) {
        // Unconsumed turn-scoped pending input expires with the turn: it is
        // transient steering produced for this turn only. It must not be
        // re-queued into the canonical turn queue, whose entries are owned by
        // the server-side drain (a transient item would surface as a blank
        // queue row and bypass the drain's persistence bookkeeping).
        self.turn_state = None;
        // Steer inputs that arrived before the injection boundary but
        // were not consumed before turn end degrade back into the session turn
        // queue. This preserves the message; a later follow-up drain may start
        // it as its own turn. They append behind already-queued inputs, while
        // preserving arrival order among themselves.
        let late_steer: Vec<PendingInputItem> = {
            let mut steer = self
                .steer_input_queue
                .lock()
                .expect("steer input queue mutex should not be poisoned");
            steer.drain(..).collect()
        };
        if !late_steer.is_empty() {
            let mut queue = self
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            for item in late_steer {
                queue.push_back(item);
            }
        }
    }

    /// Merge turn-scoped pending input with the steer inbox.
    /// Order: steer inbox → turn-state pending. The session turn queue is
    /// deliberately NOT drained here: its entries become follow-up turns via
    /// the server-side drain, never silent mid-turn injections.
    pub fn take_turn_pending_input(&mut self) -> Vec<PendingInputItem> {
        let mut result = self.drain_steer_input_queue();
        if let Some(turn) = self.turn_state.as_mut() {
            result.extend(turn.take_pending_input());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use devo_protocol::PendingInputKind;
    use devo_protocol::ReasoningCapability;
    use devo_protocol::ReasoningEffort;
    use devo_protocol::SessionId;
    use pretty_assertions::assert_eq;

    use super::*;

    fn active_thread_goal(objective: &str, token_budget: Option<i64>) -> ThreadGoal {
        ThreadGoal {
            thread_id: SessionId::new(),
            objective: objective.to_string(),
            status: ThreadGoalStatus::Active,
            token_budget,
            tokens_used: 17,
            time_used_seconds: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn goal_context_prompt_escapes_untrusted_objective_xml() {
        // Trace: L2-DES-GOAL-001
        let state = SessionGoalState::new(active_thread_goal(
            "finish <goal> & report \"done\"",
            Some(100),
        ));

        let prompt = state.context_prompt().expect("active goal prompt");

        assert!(prompt.contains("finish &lt;goal&gt; &amp; report &quot;done&quot;"));
        assert!(!prompt.contains("finish <goal> & report \"done\""));
        assert!(prompt.contains("Completion audit:"));
    }

    #[test]
    fn goal_context_prompt_does_not_fabricate_default_budget() {
        // Trace: L2-DES-GOAL-001
        let state = SessionGoalState::new(active_thread_goal("finish goal", None));

        let prompt = state.context_prompt().expect("active goal prompt");

        assert!(prompt.contains("- Token budget: none"));
        assert!(prompt.contains("- Tokens remaining: unlimited"));
    }

    #[test]
    fn plan_mode_session_suppresses_goal_context_prompt() {
        // Trace: L2-DES-GOAL-001
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_thread_goal("plan should not pursue goal", None));
        session.collaboration_mode = CollaborationMode::Plan;

        assert_eq!(session.goal_context_prompt(), None);
    }

    #[test]
    fn turn_config_normalizes_default_reasoning_effort_selection() {
        let model = Model {
            slug: "deepseek-v4-flash".to_string(),
            display_name: "deepseek-v4-flash".to_string(),
            reasoning_capability: ReasoningCapability::Levels(
                devo_protocol::levels_with_leading_off([
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ]),
            ),
            default_reasoning_effort: Some(ReasoningEffort::High),
            ..Model::default()
        };

        let direct = TurnConfig::new(model.clone(), Some("default".to_string()));
        let provider_bound = TurnConfig::with_request_model(
            model,
            "vendor/deepseek-v4-flash".to_string(),
            ProviderRequestModelMap::default(),
            Some(String::new()),
        );

        assert_eq!(direct.reasoning_effort_selection, Some("high".to_string()));
        assert_eq!(
            provider_bound.reasoning_effort_selection,
            Some("high".to_string())
        );
    }

    #[test]
    fn end_turn_degrades_unconsumed_steer_inputs_into_the_turn_queue() {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.start_turn(TurnKind::Regular);
        let steer = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "late steer".to_string(),
            },
            None,
            chrono::Utc::now(),
        );
        session
            .steer_input_queue
            .lock()
            .expect("steer lock")
            .push_back(steer.clone());

        session.end_turn();

        let queue = session.pending_turn_queue.lock().expect("queue lock");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, steer.id);
        assert!(
            matches!(&queue[0].kind, PendingInputKind::UserText { text } if text == "late steer")
        );
        assert!(
            session
                .steer_input_queue
                .lock()
                .expect("steer lock")
                .is_empty()
        );
    }

    #[test]
    fn turn_config_token_budget_uses_model_effective_context() {
        let model = Model {
            slug: "deepseek-v4-pro".to_string(),
            display_name: "deepseek-v4-pro".to_string(),
            context_window: 1_000_000,
            effective_context_window_percent: Some(95.0),
            max_tokens: Some(384_000),
            ..Model::default()
        };
        let turn_config = TurnConfig::new(model, None);

        assert_eq!(
            turn_config.token_budget(),
            TokenBudget {
                context_window: 950_000,
                max_output_tokens: 384_000,
                compact_threshold: 1.0,
                auto_compact_token_limit: Some(950_000),
            }
        );
    }

    #[test]
    fn session_config_default_values() {
        let config = SessionConfig::default();
        assert_eq!(config.permission_profile.preset, PermissionPreset::Default);
        assert_eq!(
            config.permission_mode,
            config.permission_profile.permission_mode()
        );
        assert_eq!(config.permission_mode, PermissionMode::Interactive);
    }

    #[test]
    fn session_state_new_initializes_correctly() {
        let config = SessionConfig::default();
        let cwd = PathBuf::from("/tmp");
        let state = SessionState::new(config, cwd.clone());

        assert!(!state.id.is_empty());
        assert!(state.messages.is_empty());
        assert!(state.session_context.is_none());
        assert!(state.latest_turn_context.is_none());
        assert_eq!(state.cwd, cwd);
        assert_eq!(state.turn_count, 0);
        assert_eq!(state.total_input_tokens, 0);
        assert_eq!(state.total_output_tokens, 0);
        assert_eq!(state.last_turn_tokens, 0);
    }

    #[test]
    fn session_state_push_message() {
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state.push_message(Message::user("hello"));
        state.push_message(Message::assistant_text("hi"));
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn session_state_to_request_messages() {
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state.push_message(Message::user("hello"));
        state.push_message(Message::assistant_text("hi"));

        let req_msgs = state.to_request_messages();
        assert_eq!(req_msgs.len(), 2);
        assert_eq!(req_msgs[0].role, "user");
        assert_eq!(req_msgs[1].role, "assistant");
    }

    #[test]
    fn session_state_unique_ids() {
        let s1 = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        let s2 = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        assert_ne!(s1.id, s2.id);
    }

    #[test]
    fn session_state_start_turn_creates_turn_state() {
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        assert!(state.turn_state.is_none());
        state.start_turn(TurnKind::Regular);
        assert!(state.turn_state.is_some());
        assert_eq!(state.turn_state.as_ref().unwrap().kind, TurnKind::Regular);
    }

    #[test]
    fn session_state_start_turn_leaves_pending_queue_untouched() {
        use chrono::Utc;
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state
            .pending_turn_queue
            .lock()
            .expect("queue lock")
            .push_back(PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: "queued".to_string(),
                },
                None,
                Utc::now(),
            ));
        state.start_turn(TurnKind::Regular);
        // Queued entries become follow-up turns via the server-side drain;
        // they are never absorbed into the starting turn.
        let pending = state.take_turn_pending_input();
        assert!(pending.is_empty());
        assert_eq!(state.pending_turn_queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn session_state_end_turn_expires_unconsumed_turn_pending() {
        use chrono::Utc;
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state.start_turn(TurnKind::Regular);
        // Push a transient turn-scoped fragment (e.g. a budget notice).
        if let Some(turn) = state.turn_state.as_mut() {
            turn.push_pending_input(PendingInputItem::new(
                devo_protocol::PendingInputKind::BudgetLimitSteering,
                None,
                Utc::now(),
            ));
        }
        state.end_turn();
        assert!(state.turn_state.is_none());
        // Transient fragments expire with the turn instead of leaking into
        // the canonical turn queue owned by the server-side drain.
        assert!(state.pending_turn_queue.lock().unwrap().is_empty());
    }

    #[test]
    fn session_state_take_turn_pending_merges_steer_and_turn_pending() {
        use chrono::Utc;
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state.start_turn(TurnKind::Regular);
        // Push to turn-scoped pending.
        if let Some(turn) = state.turn_state.as_mut() {
            turn.push_pending_input(PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: "turn-item".to_string(),
                },
                None,
                Utc::now(),
            ));
        }
        // Push to the steer inbox.
        state
            .steer_input_queue
            .lock()
            .expect("steer lock")
            .push_back(PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: "steer-item".to_string(),
                },
                None,
                Utc::now(),
            ));
        let merged = state.take_turn_pending_input();
        assert_eq!(merged.len(), 2);
        assert!(
            matches!(&merged[0].kind, devo_protocol::PendingInputKind::UserText { text } if text == "steer-item")
        );
        assert!(
            matches!(&merged[1].kind, devo_protocol::PendingInputKind::UserText { text } if text == "turn-item")
        );
    }

    #[test]
    fn session_state_take_turn_pending_leaves_turn_queue_untouched() {
        use chrono::Utc;
        let mut state = SessionState::new(SessionConfig::default(), PathBuf::from("/tmp"));
        state
            .pending_turn_queue
            .lock()
            .expect("queue lock")
            .push_back(PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: "queued".to_string(),
                },
                None,
                Utc::now(),
            ));
        // The session turn queue is drained only by the server-side follow-up
        // scheduling, never by the running turn.
        let items = state.take_turn_pending_input();
        assert!(items.is_empty());
        assert_eq!(state.pending_turn_queue.lock().unwrap().len(), 1);
    }
}
