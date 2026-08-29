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

/// One transcript-affecting lifecycle transition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ItemLifecycleEvent {
    TextStarted {
        item_id: ItemId,
        kind: TextItemKind,
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
    /// A tool row finished and should commit to history.
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
    /// Clears live tool rows when a turn ends without individual completions.
    TurnLiveToolsCleared,
}
