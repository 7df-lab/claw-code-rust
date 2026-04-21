use std::path::PathBuf;

use crate::events::SavedModelEntry;
use devo_core::PresetModelCatalog;
use devo_core::ProviderWireApi;

/// Summary returned when the interactive TUI exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExit {
    /// Total turns completed in the session.
    pub turn_count: usize,
    /// Total input tokens accumulated in the session.
    pub total_input_tokens: usize,
    /// Total output tokens accumulated in the session.
    pub total_output_tokens: usize,
}

/// Initial session identity used to seed the interactive terminal UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialTuiSession {
    /// Model identifier used for the first requests and initial UI projection.
    pub model: String,
    /// Provider family used for the initial runtime connection and picker fallback.
    pub provider: ProviderWireApi,
    /// Working directory used for the initial session.
    pub cwd: PathBuf,
}

/// Runtime wiring used to launch the interactive terminal UI.
pub struct InteractiveTuiConfig {
    /// Initial session identity projected into the UI and passed to the worker.
    pub initial_session: InitialTuiSession,
    /// Optional CLI log-level override to forward to the spawned server process.
    pub server_log_level: Option<String>,
    /// Built-in model catalog used for onboarding and model selection.
    pub model_catalog: PresetModelCatalog,
    /// Persisted model entries available for switching in the composer popup.
    pub saved_models: Vec<SavedModelEntry>,
    /// Initial thinking selection restored from persisted config.
    pub thinking_selection: Option<String>,
    /// Whether to open the model picker on startup.
    pub show_model_onboarding: bool,
}
