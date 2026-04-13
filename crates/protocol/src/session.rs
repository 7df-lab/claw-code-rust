use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::turn::TurnSummary;
use crate::{SessionId, SessionTitleState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeStatus {
    Idle,
    ActiveTurn,
    WaitingClient,
    Archived,
    Unloaded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: Option<String>,
    pub title_state: SessionTitleState,
    pub ephemeral: bool,
    pub resolved_model: Option<String>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub status: SessionRuntimeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStartParams {
    pub cwd: PathBuf,
    pub ephemeral: bool,
    pub title: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStartResult {
    pub session_id: SessionId,
    pub created_at: DateTime<Utc>,
    pub cwd: PathBuf,
    pub ephemeral: bool,
    pub resolved_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeResult {
    pub session: SessionSummary,
    pub latest_turn: Option<TurnSummary>,
    pub loaded_item_count: u64,
    pub history_items: Vec<SessionHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHistoryItemKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHistoryItem {
    pub kind: SessionHistoryItemKind,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListResult {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleUpdateParams {
    pub session_id: SessionId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleUpdateResult {
    pub session: SessionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionForkParams {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionForkResult {
    pub session: SessionSummary,
    pub forked_from_session_id: SessionId,
}
