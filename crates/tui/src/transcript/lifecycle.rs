//! Transcript lifecycle events for [`super::TranscriptProjector`].
//!
//! Tool rows use fact-only events (`ToolOpened`, chunks, `ToolClosed`). Verbs and
//! titles are derived at render time in [`super::presentation`].

use std::collections::HashMap;
use std::path::PathBuf;

use devo_core::ItemId;
use devo_protocol::protocol::ExecCommandSource;
use devo_protocol::protocol::FileChange;

use crate::events::PlanStep;
use crate::events::TextItemKind;

/// Authoritative outcome used when committing the current turn's live tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnToolOutcome {
    Completed,
    Failed,
    Interrupted,
}

/// One transcript-affecting lifecycle transition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ItemLifecycleEvent {
    TextStarted {
        item_id: ItemId,
        kind: TextItemKind,
        item_seq: Option<u64>,
    },
    TextDelta {
        item_id: ItemId,
        kind: TextItemKind,
        delta: String,
    },
    TextCompleted {
        item_id: ItemId,
        kind: TextItemKind,
        final_text: String,
    },
    ProposedPlanStarted {
        item_id: ItemId,
    },
    ProposedPlanDelta {
        item_id: ItemId,
        delta: String,
    },
    ProposedPlanCompleted {
        item_id: ItemId,
        final_text: String,
    },
    /// A tool row opened (model call, file change, or command execution).
    ToolOpened {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        item_seq: Option<u64>,
        command: Option<String>,
        command_source: Option<ExecCommandSource>,
        parsed_commands: Vec<devo_protocol::parse_command::ParsedCommand>,
    },
    /// Partial tool-call input JSON while parameters are still streaming.
    ToolInputChunk {
        tool_use_id: String,
        chunk: String,
    },
    /// Streaming tool output (command stdout, etc.).
    ToolOutputChunk {
        tool_use_id: String,
        chunk: String,
    },
    /// A tool row finished. It remains live until the turn commit boundary.
    ToolClosed {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        display_content: Option<String>,
        file_changes: Option<HashMap<PathBuf, FileChange>>,
        is_error: bool,
        truncated: bool,
    },
    PlanUpdated {
        explanation: Option<String>,
        steps: Vec<PlanStep>,
    },
    /// Commits every tool owned by the current turn in sequence order.
    TurnLiveToolsCleared {
        outcome: TurnToolOutcome,
    },
}
