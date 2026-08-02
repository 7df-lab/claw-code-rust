use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::AcpMcpServer;
use crate::AcpMeta;
use crate::AcpSessionConfigId;
use crate::AcpSessionConfigOption;
use crate::AcpSessionConfigValueId;
use crate::AcpSessionModeId;
use crate::AcpSessionModeState;
use crate::DEVO_SESSION_META;
use crate::SessionId;
use crate::SessionMetadata;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpListSessionsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpListSessionsResult {
    pub sessions: Vec<AcpSessionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionInfo {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpLoadSessionParams {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default)]
    pub mcp_servers: Vec<AcpMcpServer>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpResumeSessionParams {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default)]
    pub mcp_servers: Vec<AcpMcpServer>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpLoadSessionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<AcpSessionModeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<AcpSessionConfigOption>>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

pub type AcpResumeSessionResult = AcpLoadSessionResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionActionParams {
    pub session_id: SessionId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

pub type AcpCloseSessionParams = AcpSessionActionParams;
pub type AcpDeleteSessionParams = AcpSessionActionParams;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpEmptyResult {
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

pub type AcpCloseSessionResult = AcpEmptyResult;
pub type AcpDeleteSessionResult = AcpEmptyResult;
pub type AcpSetModeResult = AcpEmptyResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetModeParams {
    pub session_id: SessionId,
    pub mode_id: AcpSessionModeId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetConfigOptionParams {
    pub session_id: SessionId,
    pub config_id: AcpSessionConfigId,
    pub value: AcpSessionConfigValueId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetConfigOptionResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_options: Vec<AcpSessionConfigOption>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

pub fn acp_session_info_from_metadata(session: &SessionMetadata) -> AcpSessionInfo {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_SESSION_META.to_string(),
        serde_json::to_value(session).expect("serialize session metadata"),
    );
    AcpSessionInfo {
        session_id: session.session_id,
        cwd: session.cwd.clone(),
        title: session.title.clone(),
        updated_at: Some(session.last_activity_at.to_rfc3339()),
        additional_directories: session.additional_directories.clone(),
        meta: Some(meta),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::SessionRuntimeStatus;
    use crate::SessionTitleState;

    #[test]
    fn session_info_uses_acp_field_names_and_preserves_devo_metadata() {
        let created_at = Utc::now();
        let updated_at = created_at + chrono::TimeDelta::minutes(2);
        let last_activity_at = created_at + chrono::TimeDelta::minutes(1);
        let session = SessionMetadata {
            session_id: SessionId::new(),
            cwd: ".".into(),
            additional_directories: vec!["/workspace/shared".into()],
            created_at,
            updated_at,
            last_activity_at,
            title: Some("Work".to_string()),
            title_state: SessionTitleState::Unset,
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: None,
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            last_query_usage: None,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
            effective_context_window: None,
            permission_preset: None,
        };

        let info = acp_session_info_from_metadata(&session);
        let json = serde_json::to_value(&info).expect("serialize session info");

        assert_eq!(json["sessionId"], serde_json::json!(session.session_id));
        assert_eq!(json["title"], serde_json::json!("Work"));
        assert_eq!(
            json["updatedAt"],
            serde_json::json!(last_activity_at.to_rfc3339())
        );
        assert_eq!(
            json["additionalDirectories"],
            serde_json::json!(["/workspace/shared"])
        );
        assert_eq!(
            serde_json::from_value::<SessionMetadata>(json["_meta"][DEVO_SESSION_META].clone())
                .expect("decode Devo session metadata"),
            session
        );
    }

    #[test]
    fn additional_session_methods_use_acp_field_names() {
        let session_id = SessionId::new();
        let load = AcpLoadSessionParams {
            session_id,
            cwd: "repo".into(),
            additional_directories: vec!["docs".into()],
            mcp_servers: Vec::new(),
            meta: None,
        };
        let set_mode = AcpSetModeParams {
            session_id,
            mode_id: "build".to_string(),
            meta: None,
        };
        let set_config = AcpSetConfigOptionParams {
            session_id,
            config_id: "permission".to_string(),
            value: "default".to_string(),
            meta: None,
        };

        assert_eq!(
            serde_json::to_value(load).expect("serialize load params"),
            serde_json::json!({
                "sessionId": session_id,
                "cwd": "repo",
                "additionalDirectories": ["docs"],
                "mcpServers": []
            })
        );
        assert_eq!(
            serde_json::to_value(set_mode).expect("serialize set mode params"),
            serde_json::json!({
                "sessionId": session_id,
                "modeId": "build"
            })
        );
        assert_eq!(
            serde_json::to_value(set_config).expect("serialize set config params"),
            serde_json::json!({
                "sessionId": session_id,
                "configId": "permission",
                "value": "default"
            })
        );
        assert_eq!(
            serde_json::to_value(AcpDeleteSessionResult::default())
                .expect("serialize delete result"),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(AcpLoadSessionResult::default()).expect("serialize load result"),
            serde_json::json!({})
        );
    }
}
