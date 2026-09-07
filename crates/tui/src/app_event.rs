//! Application-level events for the Claw v2 TUI.
//!
use std::path::PathBuf;

use devo_core::ItemId;
use devo_core::SessionId;
use devo_protocol::ReferenceSearchSnapshot;

use crate::app_command::AppCommand;
use crate::events::PlanStep;
use crate::events::TextItemKind;

/// Direction for Settings Hub value cycling (`←` / `→`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCycleDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectorsSnapshot {
    pub(crate) connectors: Vec<ConnectorInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectorInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubagentDebugStep {
    Discover {
        session_id: SessionId,
        parent_session_id: SessionId,
        nickname: String,
        status: String,
        last_task_message: Option<String>,
    },
    TextDelta {
        session_id: SessionId,
        item_id: ItemId,
        kind: TextItemKind,
        delta: String,
    },
    ToolCall {
        session_id: SessionId,
        tool_use_id: String,
        summary: String,
    },
    ToolOutputDelta {
        session_id: SessionId,
        tool_use_id: String,
        delta: String,
    },
    ToolResult {
        session_id: SessionId,
        tool_use_id: String,
        title: String,
        preview: String,
        is_error: bool,
    },
    PlanUpdated {
        session_id: SessionId,
        explanation: Option<String>,
        steps: Vec<PlanStep>,
    },
    Finish {
        session_id: SessionId,
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum AppEvent {
    ContinueTurnRecovery {
        recovery: devo_protocol::native::rpc_turn::TurnRecovery,
    },
    CancelTurnRecovery,
    /// Request a redraw on the next frame.
    Redraw,

    /// Request to exit the TUI.
    Exit(ExitMode),

    /// Provider onboarding completed successfully.
    OnboardingCompleted,

    /// Submit the current composer text.
    SubmitUserInput {
        text: String,
    },

    /// Send a command request to the host/worker adapter.
    Command(AppCommand),

    /// Open a read-only transcript overlay for a live direct child agent.
    OpenSubagentOverlay {
        session_id: SessionId,
    },

    /// Inject one deterministic sub-agent monitor step for TUI debugging.
    DebugSubagentStep {
        step: SubagentDebugStep,
    },

    #[allow(dead_code)]
    /// Interrupt the current turn or cancel the active UI surface.
    Interrupt,

    #[allow(dead_code)]
    /// Clear the visible transcript.
    ClearTranscript,

    #[allow(dead_code)]
    /// Open the slash command popup.
    OpenSlashCommandPopup,

    #[allow(dead_code)]
    /// Close the currently active popup or transient view.
    ClosePopup,

    #[allow(dead_code)]
    /// Execute a slash command selected or typed by the user.
    RunSlashCommand {
        command: String,
    },

    #[allow(dead_code)]
    /// Open the model picker.
    OpenModelPicker,

    #[allow(dead_code)]
    /// Apply a selected model.
    ModelSelected {
        model: String,
    },

    #[allow(dead_code)]
    /// Open the reasoning-effort picker.
    OpenReasoningEffortPicker,

    #[allow(dead_code)]
    /// Apply a selected reasoning effort.
    ReasoningEffortSelected {
        value: Option<String>,
    },

    /// Async update of the current git branch for status-line rendering.
    StatusLineBranchUpdated {
        cwd: PathBuf,
        branch: Option<String>,
    },

    /// Request a server-backed reference-search refresh for composer popups.
    ReferenceSearchRequested {
        query: String,
    },

    /// Cancel the active composer reference-search session, if any.
    ReferenceSearchCancelled,

    /// Async reference-search results for a composer popup query.
    ReferenceSearchResults {
        snapshot: ReferenceSearchSnapshot,
    },

    /// Request a persistent composer-history entry by absolute log offset.
    HistoryEntryRequested {
        log_id: u64,
        offset: usize,
    },

    /// Replace the current status message.
    StatusMessageChanged {
        message: String,
    },

    /// Insert text into the composer at the cursor (used by `/mcps` / `/skills` pickers).
    ///
    /// When `binding` is set, `text` is inserted as a highlighted atomic chip and
    /// expanded to `binding` in the model-facing payload on submit (same path as
    /// `@` file mentions).
    InsertComposerText {
        text: String,
        binding: Option<String>,
    },

    #[allow(dead_code)]
    /// Apply a user-confirmed status-line item ordering/selection.
    StatusLineSetup {
        items: Vec<StatusLineItem>,
    },

    #[allow(dead_code)]
    /// Dismiss the status-line setup UI without changing config.
    StatusLineSetupCancelled,

    #[allow(dead_code)]
    /// Apply a user-confirmed terminal-title item ordering/selection.
    TerminalTitleSetup {
        items: Vec<TerminalTitleItem>,
    },

    #[allow(dead_code)]
    /// Apply a temporary terminal-title preview while the setup UI is open.
    TerminalTitleSetupPreview {
        items: Vec<TerminalTitleItem>,
    },

    #[allow(dead_code)]
    /// Dismiss the terminal-title setup UI without changing config.
    TerminalTitleSetupCancelled,

    #[allow(dead_code)]
    /// Open the theme picker.
    OpenThemePicker,
    #[allow(dead_code)]
    /// Apply a selected theme.
    ThemeSelected {
        name: String,
    },
    /// Apply show-reasoning preference (collapsed vs full).
    CollapseReasoningSelected {
        collapsed: bool,
    },
    /// Settings Hub deep-links and local actions.
    SettingsOpenModel,
    SettingsOpenPermissions,
    SettingsOpenReasoning,
    SettingsOpenCompaction,
    SettingsCycleMode,
    /// Cycle the UI theme from Settings Hub (`←` / `→` on the Theme row).
    SettingsCycleTheme {
        direction: SettingsCycleDirection,
    },
    /// Coalesced transcript reload after rapid theme changes.
    FlushDebouncedThemeReload {
        epoch: u64,
    },
    /// Open the `/mcps` server list from the cached snapshot.
    McpOpenServerList,
    /// Open the `/mcps` detail panel for one server.
    McpOpenServerDetail {
        name: String,
    },
    /// Select one MCP server from the picker (opens detail).
    McpServerSelected {
        name: String,
    },
    /// Open the `/skills` list from the cached snapshot.
    SkillOpenList,
    /// Select one skill from the picker (opens detail).
    SkillSelected {
        name: String,
    },
    /// Clear the managed inline UI and re-emit committed transcript lines.
    ///
    /// Used after theme changes so already-flushed header/logo cells can be
    /// redrawn with the new accent colors.
    ReloadInlineTranscript,
    /// Result of computing a `/diff` command (ANSI-colored diff text).
    DiffResult(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitMode {
    /// Let the host perform orderly shutdown before exiting.
    ShutdownFirst,
    #[allow(dead_code)]
    /// Exit the UI loop immediately.
    Immediate,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusLineItem {
    Model,
    Tokens,
    CurrentDir,
    Custom(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalTitleItem {
    Project,
    Model,
    Spinner,
    Custom(String),
}
