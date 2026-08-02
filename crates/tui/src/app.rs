use std::path::PathBuf;

use crate::events::SavedModelEntry;
use devo_core::PermissionPreset;
use devo_core::PresetModelCatalog;
use devo_core::ProviderWireApi;
use devo_protocol::CollaborationMode;
use devo_protocol::SessionId;

/// Summary returned when the interactive TUI exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExit {
    /// Active session identifier at exit, when one exists.
    pub session_id: Option<SessionId>,
    /// Whether provider onboarding completed successfully during this TUI run.
    pub onboarding_completed: bool,
    /// Total turns completed in the session.
    pub turn_count: usize,
    /// Total input tokens accumulated in the session.
    pub total_input_tokens: usize,
    /// Total output tokens accumulated in the session.
    pub total_output_tokens: usize,
    /// Display total tokens accumulated in the session.
    pub total_tokens: usize,
    /// Total cached input tokens accumulated in the session.
    pub total_cache_read_tokens: usize,
}

/// Public startup request passed from the CLI into the TUI crate.
///
/// This type intentionally carries config-shaped values: a model slug, provider fallback,
/// reasoning effort selection, and cwd. `host` resolves the model slug against the catalog before
/// constructing the chat widget's runtime session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialTuiSession {
    /// Optional pre-existing session to resume immediately on startup.
    pub session_id: Option<SessionId>,
    /// Model identifier used for the first requests and initial UI projection.
    pub model: String,
    /// Optional provider-specific model name used for requests when it differs from `model`.
    pub request_model: Option<String>,
    /// Active model binding id when startup resolved one from `[model_bindings]`.
    pub model_binding_id: Option<String>,
    /// Provider family used for the initial runtime connection and picker fallback.
    pub provider: ProviderWireApi,
    /// Initial reasoning effort selection restored from persisted config.
    pub reasoning_effort_selection: Option<String>,
    /// Initial permission preset restored from project-level config.
    pub permission_preset: PermissionPreset,
    /// Initial sandbox profile restored from project-level config.
    pub sandbox_profile: Option<String>,
    /// Initial global compaction token limit from user `config.toml`, when set.
    pub compaction_token_limit: Option<u64>,
    /// Default collaboration mode from user `config.toml`.
    pub default_collaboration_mode: CollaborationMode,
    /// Working directory used for the initial session.
    pub cwd: PathBuf,
}

/// Runtime wiring used to launch the interactive terminal UI.
pub struct InteractiveTuiConfig {
    /// Initial session request resolved by the host before it reaches internal widgets.
    pub initial_session: InitialTuiSession,
    /// Optional CLI log-level override to forward to the spawned server process.
    pub server_log_level: Option<String>,
    /// Built-in model catalog used for onboarding and model selection.
    pub model_catalog: PresetModelCatalog,
    /// Persisted model entries available for switching in the composer popup.
    pub saved_models: Vec<SavedModelEntry>,
    /// Whether to open the model picker on startup.
    pub show_model_onboarding: bool,
    /// Whether successful onboarding should exit the TUI immediately.
    pub exit_after_onboarding: bool,
}
