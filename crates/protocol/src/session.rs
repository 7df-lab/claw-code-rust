use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::SessionId;
use crate::SessionTitleState;
use crate::turn::TurnMetadata;

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
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: Option<String>,
    pub title_state: SessionTitleState,
    pub ephemeral: bool,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub prompt_token_estimate: usize,
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
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeResult {
    pub session: SessionMetadata,
    pub latest_turn: Option<TurnMetadata>,
    pub loaded_item_count: u64,
    pub history_items: Vec<SessionHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHistoryItemKind {
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHistoryItem {
    pub tool_call_id: Option<String>,
    pub kind: SessionHistoryItemKind,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListResult {
    pub sessions: Vec<SessionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleUpdateParams {
    pub session_id: SessionId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleUpdateResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadataUpdateParams {
    pub session_id: SessionId,
    pub model: Option<String>,
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadataUpdateResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCompactParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCompactResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionForkParams {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionForkResult {
    pub session: SessionMetadata,
    pub forked_from_session_id: SessionId,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::SessionTitleState;

    #[test]
    fn session_metadata_roundtrips_with_model_and_thinking() {
        let metadata = SessionMetadata {
            session_id: SessionId::new(),
            cwd: "/tmp".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            title: Some("Test".to_string()),
            title_state: SessionTitleState::Unset,
            ephemeral: false,
            model: Some("test-model".to_string()),
            thinking: Some("medium".to_string()),
            total_input_tokens: 12,
            total_output_tokens: 34,
            prompt_token_estimate: 21,
            status: SessionRuntimeStatus::Idle,
        };

        let json = serde_json::to_string(&metadata).expect("serialize");
        let restored: SessionMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, metadata);
    }
}
