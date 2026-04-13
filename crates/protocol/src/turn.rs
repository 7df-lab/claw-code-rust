use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ItemId, SessionId, TurnId, TurnStatus, TurnUsage};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub model_slug: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub usage: Option<TurnUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    Text { text: String },
    Skill { id: String },
    LocalImage { path: PathBuf },
    Mention { path: String, name: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub session_id: SessionId,
    pub input: Vec<InputItem>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub accepted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInterruptParams {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInterruptResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSteerParams {
    pub session_id: SessionId,
    pub expected_turn_id: TurnId,
    pub input: Vec<InputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSteerResult {
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    Regular,
    Review,
    ManualCompaction,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerInputRecord {
    pub item_id: ItemId,
    pub received_at: DateTime<Utc>,
    pub input: Vec<InputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTurnSteeringState {
    pub turn_id: TurnId,
    pub turn_kind: TurnKind,
    pub pending_inputs: VecDeque<SteerInputRecord>,
}
