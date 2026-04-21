use devo_core::{
    ContentBlock, Message, SessionRecord, TextItem, ToolCallItem, ToolResultItem, TurnItem,
    TurnRecord,
};

use crate::session::{
    SessionHistoryItem, SessionHistoryItemKind, SessionRuntimeStatus, SessionSummary,
};
use crate::turn::TurnSummary;

/// Projects a canonical core session record into the API-visible session summary.
pub trait SessionProjector {
    /// Converts one core session record into a transport-facing session summary.
    fn project_session(
        &self,
        session: &SessionRecord,
        ephemeral: bool,
        status: SessionRuntimeStatus,
    ) -> SessionSummary;
}

/// Projects a canonical core turn record into the API-visible turn summary.
pub trait TurnProjector {
    /// Converts one core turn record into a transport-facing turn summary.
    fn project_turn(&self, turn: &TurnRecord) -> TurnSummary;
}

/// Default projector that performs field-by-field protocol projection.
#[derive(Debug, Clone, Default)]
pub struct DefaultProjection;

impl DefaultProjection {
    /// Converts replayed core conversation messages into a client-facing transcript snapshot.
    pub fn project_history(&self, messages: &[Message]) -> Vec<SessionHistoryItem> {
        let mut history = Vec::new();
        for message in messages {
            for block in &message.content {
                match block {
                    ContentBlock::Text { text } if !text.is_empty() => {
                        let kind = if message.role == devo_core::Role::User {
                            SessionHistoryItemKind::User
                        } else {
                            SessionHistoryItemKind::Assistant
                        };
                        history.push(SessionHistoryItem {
                            kind,
                            title: String::new(),
                            body: text.clone(),
                        });
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        history.push(SessionHistoryItem {
                            kind: SessionHistoryItemKind::ToolCall,
                            title: summarize_tool_call(name, input),
                            body: String::new(),
                        });
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        history.push(SessionHistoryItem {
                            kind: if *is_error {
                                SessionHistoryItemKind::Error
                            } else {
                                SessionHistoryItemKind::ToolResult
                            },
                            title: if *is_error {
                                "Tool error".to_string()
                            } else {
                                "Tool output".to_string()
                            },
                            body: content.clone(),
                        });
                    }
                    ContentBlock::Text { .. } => {}
                }
            }
        }
        history
    }
}

/// Projects one canonical persisted turn item into one replay-friendly history item when visible.
pub(crate) fn history_item_from_turn_item(item: &TurnItem) -> Option<SessionHistoryItem> {
    match item {
        TurnItem::UserMessage(TextItem { text }) | TurnItem::SteerInput(TextItem { text }) => {
            Some(SessionHistoryItem {
                kind: SessionHistoryItemKind::User,
                title: String::new(),
                body: text.clone(),
            })
        }
        TurnItem::AgentMessage(TextItem { text })
        | TurnItem::Plan(TextItem { text })
        | TurnItem::Reasoning(TextItem { text })
        | TurnItem::WebSearch(TextItem { text })
        | TurnItem::ImageGeneration(TextItem { text })
        | TurnItem::ContextCompaction(TextItem { text })
        | TurnItem::HookPrompt(TextItem { text }) => Some(SessionHistoryItem {
            kind: SessionHistoryItemKind::Assistant,
            title: String::new(),
            body: text.clone(),
        }),
        TurnItem::ToolCall(ToolCallItem {
            tool_name, input, ..
        }) => Some(SessionHistoryItem {
            kind: SessionHistoryItemKind::ToolCall,
            title: summarize_tool_call(tool_name, input),
            body: String::new(),
        }),
        TurnItem::ToolResult(ToolResultItem {
            output, is_error, ..
        }) => Some(SessionHistoryItem {
            kind: if *is_error {
                SessionHistoryItemKind::Error
            } else {
                SessionHistoryItemKind::ToolResult
            },
            title: if *is_error {
                "Tool error".to_string()
            } else {
                "Tool output".to_string()
            },
            body: match output {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            },
        }),
        TurnItem::ToolProgress(_)
        | TurnItem::ApprovalRequest(_)
        | TurnItem::ApprovalDecision(_) => None,
    }
}

impl SessionProjector for DefaultProjection {
    fn project_session(
        &self,
        session: &SessionRecord,
        ephemeral: bool,
        status: SessionRuntimeStatus,
    ) -> SessionSummary {
        SessionSummary {
            session_id: session.id,
            cwd: session.cwd.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            title: session.title.clone(),
            title_state: session.title_state.clone(),
            ephemeral,
            resolved_model: session.model.clone(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            status,
        }
    }
}

impl TurnProjector for DefaultProjection {
    fn project_turn(&self, turn: &TurnRecord) -> TurnSummary {
        TurnSummary {
            turn_id: turn.id,
            session_id: turn.session_id,
            sequence: turn.sequence,
            status: turn.status.clone(),
            model_slug: turn.model_slug.clone(),
            started_at: turn.started_at,
            completed_at: turn.completed_at,
            usage: turn.usage.clone(),
        }
    }
}

fn summarize_tool_call(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "bash" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| format!("Ran {command}"))
            .unwrap_or_else(|| "Ran shell command".to_string()),
        other => format!("Ran {other}"),
    }
}
