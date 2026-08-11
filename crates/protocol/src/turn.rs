use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use crate::{
    ItemId, PendingInputId, ReasoningEffort, SessionId, StopReason, TurnId, TurnStatus, TurnUsage,
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnMetadata {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub kind: TurnKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub request_model: String,
    pub request_thinking: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub usage: Option<TurnUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<TurnFailureReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnFailureReason {
    MaxTurnRequests,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    Text { text: String },
    Skill { name: String, path: PathBuf },
    LocalImage { path: PathBuf },
    Mention { path: String, name: Option<String> },
}

impl<'de> Deserialize<'de> for InputItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum WireInputItem {
            Text {
                text: String,
            },
            Skill {
                name: Option<String>,
                path: Option<PathBuf>,
                id: Option<String>,
            },
            LocalImage {
                path: PathBuf,
            },
            Mention {
                path: String,
                name: Option<String>,
            },
        }

        match WireInputItem::deserialize(deserializer)? {
            WireInputItem::Text { text } => Ok(Self::Text { text }),
            WireInputItem::Skill { name, path, id } => {
                let name = name
                    .or(id)
                    .ok_or_else(|| serde::de::Error::missing_field("name"))?;
                Ok(Self::Skill {
                    name,
                    path: path.unwrap_or_default(),
                })
            }
            WireInputItem::LocalImage { path } => Ok(Self::LocalImage { path }),
            WireInputItem::Mention { path, name } => Ok(Self::Mention { path, name }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    #[default]
    Build,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnExecutionMode {
    #[default]
    Regular,
}

fn is_default_turn_execution_mode(mode: &TurnExecutionMode) -> bool {
    *mode == TurnExecutionMode::Regular
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnStartParams {
    pub session_id: SessionId,
    pub input: Vec<InputItem>,
    /// Legacy model selector retained for compatibility with older clients.
    /// New clients should send [`Self::model_binding_id`] instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider model binding selected for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default, alias = "interaction_mode")]
    pub collaboration_mode: CollaborationMode,
    #[serde(default, skip_serializing_if = "is_default_turn_execution_mode")]
    pub execution_mode: TurnExecutionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputDisposition {
    #[default]
    Started,
    Queued,
    Steered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum TurnStartResult {
    Started {
        turn_id: TurnId,
        status: TurnStatus,
        accepted_at: DateTime<Utc>,
    },
    Queued {
        active_turn_id: TurnId,
        queued_input_id: PendingInputId,
        status: TurnStatus,
        accepted_at: DateTime<Utc>,
    },
}

impl TurnStartResult {
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            Self::Started { turn_id, .. } => Some(*turn_id),
            Self::Queued { .. } => None,
        }
    }

    pub fn active_turn_id(&self) -> TurnId {
        match self {
            Self::Started { turn_id, .. } => *turn_id,
            Self::Queued { active_turn_id, .. } => *active_turn_id,
        }
    }

    pub fn disposition(&self) -> TurnInputDisposition {
        match self {
            Self::Started { .. } => TurnInputDisposition::Started,
            Self::Queued { .. } => TurnInputDisposition::Queued,
        }
    }

    pub fn status(&self) -> TurnStatus {
        match self {
            Self::Started { status, .. } | Self::Queued { status, .. } => status.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ShellCommandParams {
    pub session_id: SessionId,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ShellCommandResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub accepted_at: DateTime<Utc>,
}

impl ShellCommandResult {
    pub fn status(&self) -> TurnStatus {
        self.status.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnInterruptParams {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnInterruptResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    #[default]
    Regular,
    Review,
    ManualCompaction,
    Other(String),
}

impl<'de> Deserialize<'de> for TurnKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(text) => Ok(match text.as_str() {
                "regular" => Self::Regular,
                "review" => Self::Review,
                "manual_compaction" => Self::ManualCompaction,
                other => Self::Other(other.to_string()),
            }),
            serde_json::Value::Object(object) if object.len() == 1 => {
                let Some((kind, payload)) = object.into_iter().next() else {
                    return Err(D::Error::custom("expected a turn kind object"));
                };
                match kind.as_str() {
                    "other" => match payload {
                        serde_json::Value::String(text) => Ok(Self::Other(text)),
                        _ => Err(D::Error::custom(
                            "expected string payload for other turn kind",
                        )),
                    },
                    "regular" => Ok(Self::Regular),
                    "review" => Ok(Self::Review),
                    "manual_compaction" => Ok(Self::ManualCompaction),
                    other => Err(D::Error::unknown_variant(
                        other,
                        &["regular", "review", "manual_compaction", "other"],
                    )),
                }
            }
            serde_json::Value::Object(_) => {
                Err(D::Error::custom("expected a single-field turn kind object"))
            }
            _ => Err(D::Error::custom("expected a turn kind string")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SteerInputRecord {
    pub item_id: ItemId,
    pub received_at: DateTime<Utc>,
    pub input: Vec<InputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActiveTurnSteeringState {
    pub turn_id: TurnId,
    pub turn_kind: TurnKind,
    pub pending_inputs: VecDeque<SteerInputRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PendingInputItem {
    #[serde(default)]
    pub id: PendingInputId,
    pub kind: PendingInputKind,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl PendingInputItem {
    pub fn new(
        kind: PendingInputKind,
        metadata: Option<serde_json::Value>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: PendingInputId::new(),
            kind,
            metadata,
            created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PendingInputKind {
    UserText {
        text: String,
    },
    UserInput {
        input: Vec<InputItem>,
        display_text: String,
        prompt_text: String,
        #[serde(default)]
        prompt_messages: Vec<String>,
    },
    ToolCallBlockedByHook {
        tool_use_id: String,
        reason: String,
    },
    BudgetLimitSteering,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn turn_metadata_roundtrips_with_logical_and_request_fields() {
        let metadata = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status: TurnStatus::Completed,
            kind: TurnKind::Regular,
            model: "logical-model".to_string(),
            model_binding_id: Some("provider-binding".to_string()),
            reasoning_effort_selection: Some("high".to_string()),
            reasoning_effort: Some(ReasoningEffort::High),
            request_model: "provider-model".to_string(),
            request_thinking: Some("medium".to_string()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: Some(TurnUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }),
            stop_reason: Some(StopReason::EndTurn),
            failure_reason: None,
        };

        let json = serde_json::to_string(&metadata).expect("serialize");
        let restored: TurnMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, metadata);
    }

    #[test]
    fn turn_start_params_default_to_build_collaboration_mode() {
        let json = serde_json::json!({
            "session_id": SessionId::new(),
            "input": [{ "type": "text", "text": "hello" }],
            "model": null,
            "thinking": null,
            "sandbox": null,
            "approval_policy": null,
            "cwd": null
        });

        let restored: TurnStartParams = serde_json::from_value(json).expect("deserialize");

        assert_eq!(restored.collaboration_mode, CollaborationMode::Build);
        assert_eq!(restored.execution_mode, TurnExecutionMode::Regular);
    }

    #[test]
    fn turn_start_params_serialize_binding_without_legacy_model() {
        let session_id = SessionId::new();
        let params = TurnStartParams {
            session_id,
            input: vec![InputItem::Text {
                text: "hello".to_string(),
            }],
            model: None,
            model_binding_id: Some("glm-zai".to_string()),
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode: CollaborationMode::Build,
            execution_mode: TurnExecutionMode::Regular,
        };

        assert_eq!(
            serde_json::to_value(params).expect("serialize"),
            serde_json::json!({
                "session_id": session_id,
                "input": [{ "type": "text", "text": "hello" }],
                "model_binding_id": "glm-zai",
                "sandbox": null,
                "approval_policy": null,
                "cwd": null,
                "collaboration_mode": "build"
            })
        );
    }

    #[test]
    fn turn_start_params_read_legacy_model_without_binding() {
        let session_id = SessionId::new();
        let restored: TurnStartParams = serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "input": [{ "type": "text", "text": "hello" }],
            "model": "glm-4.5",
            "sandbox": null,
            "approval_policy": null,
            "cwd": null,
            "collaboration_mode": "build"
        }))
        .expect("deserialize legacy turn request");

        assert_eq!(
            restored,
            TurnStartParams {
                session_id,
                input: vec![InputItem::Text {
                    text: "hello".to_string(),
                }],
                model: Some("glm-4.5".to_string()),
                model_binding_id: None,
                reasoning_effort_selection: None,
                sandbox: None,
                approval_policy: None,
                cwd: None,
                collaboration_mode: CollaborationMode::Build,
                execution_mode: TurnExecutionMode::Regular,
            }
        );
    }

    #[test]
    fn turn_execution_mode_serializes_default_regular_omitted() {
        // Verifies: regular remains the default turn/start execution mode.
        let params = TurnStartParams {
            session_id: SessionId::new(),
            input: vec![InputItem::Text {
                text: "hello".into(),
            }],
            model: None,
            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode: CollaborationMode::Build,
            execution_mode: TurnExecutionMode::Regular,
        };

        let value = serde_json::to_value(params).expect("serialize");

        assert_eq!(value.get("execution_mode"), None);
    }

    #[test]
    fn turn_start_params_reject_removed_research_execution_mode() {
        let json = serde_json::json!({
            "session_id": SessionId::new(),
            "input": [{ "type": "text", "text": "research this" }],
            "model": null,
            "thinking": null,
            "sandbox": null,
            "approval_policy": null,
            "cwd": null,
            "execution_mode": "research"
        });

        let error = serde_json::from_value::<TurnStartParams>(json)
            .expect_err("removed research mode must be rejected");

        assert!(error.to_string().contains("unknown variant `research`"));
    }

    #[test]
    fn turn_start_params_accept_legacy_interaction_mode_alias() {
        let json = serde_json::json!({
            "session_id": SessionId::new(),
            "input": [{ "type": "text", "text": "hello" }],
            "model": null,
            "thinking": null,
            "sandbox": null,
            "approval_policy": null,
            "cwd": null,
            "interaction_mode": "plan"
        });

        let restored: TurnStartParams = serde_json::from_value(json).expect("deserialize");

        assert_eq!(restored.collaboration_mode, CollaborationMode::Plan);
    }

    #[test]
    fn shell_command_result_keeps_plain_started_shape() {
        let result = ShellCommandResult {
            turn_id: TurnId::new(),
            status: TurnStatus::Running,
            accepted_at: Utc::now(),
        };

        let json = serde_json::to_value(&result).expect("serialize");
        let turn_id = result.turn_id.to_string();

        assert_eq!(json["turn_id"].as_str(), Some(turn_id.as_str()));
        assert_eq!(json["status"].as_str(), Some("Running"));
        assert!(json.get("disposition").is_none());
    }

    #[test]
    fn pending_input_item_user_text_roundtrips() {
        let item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "hello".into(),
            },
            Some(serde_json::json!({"source": "tui"})),
            Utc::now(),
        );
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item.created_at, restored.created_at);
        assert_eq!(item.metadata, restored.metadata);
        assert_eq!(format!("{:?}", item.kind), format!("{:?}", restored.kind));
    }

    #[test]
    fn pending_input_item_tool_call_blocked_roundtrips() {
        let item = PendingInputItem::new(
            PendingInputKind::ToolCallBlockedByHook {
                tool_use_id: "tool-1".into(),
                reason: "blocked by safety".into(),
            },
            None,
            Utc::now(),
        );
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item.created_at, restored.created_at);
    }

    #[test]
    fn pending_input_item_budget_limit_steering_roundtrips() {
        let item = PendingInputItem::new(PendingInputKind::BudgetLimitSteering, None, Utc::now());
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            restored.kind,
            PendingInputKind::BudgetLimitSteering
        ));
    }

    #[test]
    fn pending_input_kind_serializes_tagged_shape() {
        let json = serde_json::json!({"type": "user_text", "text": "hello"});
        let kind: PendingInputKind = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(kind, PendingInputKind::UserText { .. }));
    }

    #[test]
    fn turn_kind_default_is_regular() {
        assert_eq!(TurnKind::default(), TurnKind::Regular);
    }

    #[test]
    fn turn_kind_unknown_string_deserializes_as_other() {
        let value: TurnKind = serde_json::from_value(serde_json::json!("shell_command"))
            .expect("deserialize unknown turn kind");

        assert_eq!(value, TurnKind::Other("shell_command".to_string()));
    }

    #[test]
    fn turn_kind_legacy_other_object_deserializes_as_other() {
        let value: TurnKind = serde_json::from_value(serde_json::json!({
            "other": "shell_command"
        }))
        .expect("deserialize legacy other turn kind");

        assert_eq!(value, TurnKind::Other("shell_command".to_string()));
    }
}
