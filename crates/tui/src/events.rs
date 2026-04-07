use std::time::{Duration, Instant};

use clawcr_core::SessionId;
use ratatui::style::Color;
/// One persisted session entry shown in the interactive session picker panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionListEntry {
    /// Stable session identifier used when switching the active session.
    pub session_id: SessionId,
    /// Human-readable session title shown to the user.
    pub title: String,
    /// Timestamp summary rendered beside the title for quick scanning.
    pub updated_at: String,
    /// Whether this entry is the currently active session.
    pub is_active: bool,
}

/// One built-in or custom model entry shown in the interactive model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelListEntry {
    /// Stable model slug used when switching the active model.
    pub slug: String,
    /// Human-readable display name shown to the user.
    pub display_name: String,
    /// Optional descriptive text rendered beneath the model name.
    pub description: Option<String>,
    /// Whether this entry is the currently active model.
    pub is_current: bool,
    /// Whether this model comes from the built-in catalog.
    pub is_builtin: bool,
}

/// One event emitted by the background query worker into the interactive UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerEvent {
    /// A new assistant turn has started.
    TurnStarted,
    /// Incremental assistant text.
    TextDelta(String),
    /// A tool call started.
    ToolCall {
        /// Human-readable summary line for the tool execution.
        summary: String,
        /// Optional structured input preview for the tool call.
        detail: Option<String>,
    },
    /// A tool call finished.
    ToolResult {
        /// Human-readable output preview shown in the transcript.
        preview: String,
        /// Whether the tool returned an error.
        is_error: bool,
        /// Whether the preview was truncated for display.
        truncated: bool,
    },
    /// The current turn completed successfully.
    TurnFinished {
        /// Human-readable stop reason.
        stop_reason: String,
        /// Total turns completed in the session.
        turn_count: usize,
        /// Total input tokens accumulated in the session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the session.
        total_output_tokens: usize,
    },
    /// The current turn failed.
    TurnFailed {
        /// Human-readable error text to surface in the transcript and status bar.
        message: String,
        /// Total turns completed in the session so far.
        turn_count: usize,
        /// Total input tokens accumulated in the session.
        total_input_tokens: usize,
        /// Total output tokens accumulated in the session.
        total_output_tokens: usize,
    },
    /// Current known sessions were listed from the server.
    SessionsListed {
        /// Structured sessions rendered into the bottom picker panel.
        sessions: Vec<SessionListEntry>,
    },
    /// The interactive client cleared its active session and is waiting for the next prompt.
    NewSessionPrepared,
    /// The active session changed.
    SessionSwitched {
        /// The new active session identifier.
        session_id: String,
        /// Optional human-readable session title.
        title: Option<String>,
        /// The model restored from the resumed session, when one exists.
        model: Option<String>,
        /// Replay-friendly transcript items loaded from the resumed session.
        history_items: Vec<TranscriptItem>,
        /// Number of persisted items loaded for the resumed session.
        loaded_item_count: u64,
    },
    /// The current session title changed.
    SessionRenamed {
        /// The renamed session identifier.
        session_id: String,
        /// The new session title.
        title: String,
    },
    /// The current session title changed due to automatic or explicit server-side updates.
    SessionTitleUpdated {
        /// The updated session identifier.
        session_id: String,
        /// The new best-known title.
        title: String,
    },
}

/// One rendered transcript item shown in the history pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptItem {
    /// Stable kind used for styling and incremental updates.
    pub kind: TranscriptItemKind,
    /// Short title rendered above or before the body.
    pub title: String,
    /// Main text body for the transcript item.
    pub body: String,
    /// Time when the tool output should start folding away.
    pub fold_next_at: Option<Instant>,
    /// Current fold stage for tool outputs.
    pub fold_stage: u8,
}

impl TranscriptItem {
    /// Creates a new transcript item with the supplied title and body.
    pub(crate) fn new(
        kind: TranscriptItemKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            body: body.into(),
            fold_next_at: None,
            fold_stage: 0,
        }
    }

    /// Marks a tool-output item for the compacting fold animation.
    pub(crate) fn with_tool_fold(mut self) -> Self {
        self.fold_next_at = Some(Instant::now() + Duration::from_millis(700));
        self.fold_stage = 0;
        self
    }

    /// Advances the fold animation when its next deadline has passed.
    pub(crate) fn advance_fold(&mut self, now: Instant) -> bool {
        if self.kind != TranscriptItemKind::ToolResult {
            return false;
        }

        let Some(next_at) = self.fold_next_at else {
            return false;
        };
        if now < next_at {
            return false;
        }

        if self.fold_stage >= 2 {
            self.fold_next_at = None;
            return false;
        }

        self.fold_stage += 1;
        self.fold_next_at = if self.fold_stage >= 2 {
            None
        } else {
            Some(now + Duration::from_millis(120))
        };
        true
    }
}

/// Visual category for one transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptItemKind {
    /// User-authored prompt text.
    User,
    /// Assistant-authored text.
    Assistant,
    /// Tool execution start marker.
    ToolCall,
    /// Successful tool result.
    ToolResult,
    /// Failed tool result or runtime error.
    Error,
    /// Local UI/system note that is not model-authored content.
    System,
}

impl TranscriptItemKind {
    /// Returns the accent color used for the item title.
    pub(crate) fn accent(self) -> Color {
        match self {
            TranscriptItemKind::User => Color::Cyan,
            TranscriptItemKind::Assistant => Color::Rgb(232, 232, 224),
            TranscriptItemKind::ToolCall => Color::DarkGray,
            TranscriptItemKind::ToolResult => Color::DarkGray,
            TranscriptItemKind::Error => Color::Red,
            TranscriptItemKind::System => Color::DarkGray,
        }
    }
}
