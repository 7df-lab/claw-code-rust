use std::{collections::VecDeque, path::PathBuf, sync::Arc};

use tokio::{sync::Mutex, task::JoinHandle};

use clawcr_core::{
    ModelCatalog, SessionConfig, SessionId, SessionRecord, SessionState, default_base_instructions,
};
use clawcr_provider::ModelProvider;
use clawcr_tools::ToolRegistry;

use crate::{
    session::{SessionHistoryItem, SessionSummary},
    turn::{SteerInputRecord, TurnSummary},
};

/// Shared server-owned runtime dependencies used by live turn execution.
pub struct ServerRuntimeDependencies {
    /// Provider used for all model requests.
    pub(crate) provider: Arc<dyn ModelProvider>,
    /// Shared built-in tool registry used by turn execution.
    pub(crate) registry: Arc<ToolRegistry>,
    /// Default model applied when no model override is present.
    pub(crate) default_model: String,
    /// Model catalog used to resolve builtin prompt metadata.
    pub(crate) model_catalog: Arc<dyn ModelCatalog>,
}

impl ServerRuntimeDependencies {
    /// Creates a new bundle of runtime dependencies for the transport server.
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        registry: Arc<ToolRegistry>,
        default_model: String,
        model_catalog: Arc<dyn ModelCatalog>,
    ) -> Self {
        Self {
            provider,
            registry,
            default_model,
            model_catalog,
        }
    }

    /// Creates an initial core session state for a newly created server session.
    pub(crate) fn new_session_state(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        model: Option<String>,
    ) -> SessionState {
        let model = model.unwrap_or_else(|| self.default_model.clone());
        let base_instructions = self
            .model_catalog
            .get(&model)
            .map(|model| model.base_instructions.trim().to_string())
            .filter(|instructions| !instructions.is_empty())
            .unwrap_or_else(|| default_base_instructions().to_string());
        let reasoning_effort = self
            .model_catalog
            .get(&model)
            .and_then(|model| model.default_reasoning_effort)
            .unwrap_or_default();
        let thinking_selection = self
            .model_catalog
            .get(&model)
            .and_then(|model| model.default_thinking_selection());
        let mut state = SessionState::new(
            SessionConfig {
                model,
                base_instructions,
                reasoning_effort,
                thinking_selection,
                ..Default::default()
            },
            cwd,
        );
        state.id = session_id.to_string();
        state
    }

    /// Returns the builtin base instructions for a model slug, if one is known.
    pub(crate) fn base_instructions_for_model(&self, model: &str) -> String {
        self.model_catalog
            .get(model)
            .map(|model| model.base_instructions.trim().to_string())
            .filter(|instructions| !instructions.is_empty())
            .unwrap_or_else(|| default_base_instructions().to_string())
    }
}

/// Mutable per-session runtime state owned by the server.
pub(crate) struct RuntimeSession {
    /// Canonical persisted session metadata when the session is durable.
    pub(crate) record: Option<SessionRecord>,
    /// Transport-facing summary exposed over the API.
    pub(crate) summary: SessionSummary,
    /// Canonical core session state used by the query loop.
    pub(crate) core_session: Arc<Mutex<SessionState>>,
    /// Currently active turn, if any.
    pub(crate) active_turn: Option<TurnSummary>,
    /// Latest terminal turn summary for the session.
    pub(crate) latest_turn: Option<TurnSummary>,
    /// Number of items loaded or appended for the session.
    pub(crate) loaded_item_count: u64,
    /// Replay-friendly ordered history used by interactive clients during session resume.
    pub(crate) history_items: Vec<SessionHistoryItem>,
    /// Pending same-turn steering inputs.
    pub(crate) steering_queue: VecDeque<SteerInputRecord>,
    /// Live query task for the active turn.
    pub(crate) active_task: Option<JoinHandle<()>>,
    /// Monotonic session-scoped item sequence counter.
    pub(crate) next_item_seq: u64,
}

impl RuntimeSession {
    /// Wraps a new runtime session in an async mutex for storage in the session map.
    pub(crate) fn shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }
}
