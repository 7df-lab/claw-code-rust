//! Durable checkpoints for turns blocked on interactive tool approval.

use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use devo_core::TurnApprovalCheckpointRecordedRecord;
use devo_core::TurnId;
use devo_core::tools::{
    AdditionalSandboxPermissions, NetworkPermission, SandboxPermissionRequest,
    ToolPermissionRequest,
};
use devo_core::{ContentBlock, Message, Role, read_canonical_history};
use devo_protocol::CollaborationMode;
use devo_protocol::SessionId;
use devo_safety::ResourceKind;
use serde::{Deserialize, Serialize};

use super::ServerRuntime;

/// Serializable mirror of [`ToolPermissionRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct StoredToolPermissionRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub cwd: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub resource: ResourceKind,
    pub action_summary: String,
    pub justification: Option<String>,
    pub path: Option<String>,
    pub host: Option<String>,
    pub target: Option<String>,
    pub command_prefix: Option<Vec<String>>,
    pub command_argv: Option<Vec<String>>,
    pub command_pattern: Option<Vec<String>>,
    pub sandbox_permissions: StoredSandboxPermissionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum StoredSandboxPermissionRequest {
    Default,
    FullEscalation,
    AdditionalPermissions {
        network: StoredNetworkPermission,
        read_paths: Vec<String>,
        write_paths: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredNetworkPermission {
    Unchanged,
    Enabled,
}

impl From<&SandboxPermissionRequest> for StoredSandboxPermissionRequest {
    fn from(request: &SandboxPermissionRequest) -> Self {
        match request {
            SandboxPermissionRequest::Default => Self::Default,
            SandboxPermissionRequest::FullEscalation => Self::FullEscalation,
            SandboxPermissionRequest::AdditionalPermissions(permissions) => {
                Self::AdditionalPermissions {
                    network: match permissions.network {
                        NetworkPermission::Unchanged => StoredNetworkPermission::Unchanged,
                        NetworkPermission::Enabled => StoredNetworkPermission::Enabled,
                    },
                    read_paths: permissions
                        .read_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                    write_paths: permissions
                        .write_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                }
            }
        }
    }
}

impl StoredSandboxPermissionRequest {
    fn into_sandbox_permission_request(self) -> SandboxPermissionRequest {
        match self {
            Self::Default => SandboxPermissionRequest::Default,
            Self::FullEscalation => SandboxPermissionRequest::FullEscalation,
            Self::AdditionalPermissions {
                network,
                read_paths,
                write_paths,
            } => SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
                network: match network {
                    StoredNetworkPermission::Unchanged => NetworkPermission::Unchanged,
                    StoredNetworkPermission::Enabled => NetworkPermission::Enabled,
                },
                read_paths: read_paths.into_iter().map(PathBuf::from).collect(),
                write_paths: write_paths.into_iter().map(PathBuf::from).collect(),
            }),
        }
    }
}

impl From<&ToolPermissionRequest> for StoredToolPermissionRequest {
    fn from(request: &ToolPermissionRequest) -> Self {
        Self {
            tool_call_id: request.tool_call_id.clone(),
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            cwd: request.cwd.display().to_string(),
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            resource: request.resource.clone(),
            action_summary: request.action_summary.clone(),
            justification: request.justification.clone(),
            path: request.path.as_ref().map(|path| path.display().to_string()),
            host: request.host.clone(),
            target: request.target.clone(),
            command_prefix: request.command_prefix.clone(),
            command_argv: request.command_argv.clone(),
            command_pattern: request.command_pattern.clone(),
            sandbox_permissions: StoredSandboxPermissionRequest::from(&request.sandbox_permissions),
        }
    }
}

impl StoredToolPermissionRequest {
    pub(crate) fn into_tool_permission_request(self) -> ToolPermissionRequest {
        ToolPermissionRequest {
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            input: self.input,
            cwd: PathBuf::from(self.cwd),
            session_id: self.session_id,
            turn_id: self.turn_id,
            resource: self.resource,
            action_summary: self.action_summary,
            justification: self.justification,
            path: self.path.map(PathBuf::from),
            host: self.host,
            target: self.target,
            command_prefix: self.command_prefix,
            command_argv: self.command_argv,
            command_pattern: self.command_pattern,
            sandbox_permissions: self.sandbox_permissions.into_sandbox_permission_request(),
        }
    }
}

pub(crate) fn tool_permission_request_from_checkpoint(
    checkpoint: &TurnApprovalCheckpointRecordedRecord,
) -> Option<ToolPermissionRequest> {
    serde_json::from_value::<StoredToolPermissionRequest>(checkpoint.permission_request.clone())
        .ok()
        .map(StoredToolPermissionRequest::into_tool_permission_request)
}

pub(crate) fn host_session_id_from_checkpoint(
    checkpoint: &TurnApprovalCheckpointRecordedRecord,
) -> SessionId {
    checkpoint
        .host_session_id
        .unwrap_or(checkpoint.owner_session_id)
}

pub(crate) fn collaboration_mode_from_checkpoint(
    checkpoint: &TurnApprovalCheckpointRecordedRecord,
) -> CollaborationMode {
    checkpoint
        .turn_config
        .get("collaborationMode")
        .and_then(|value| value.as_str())
        .and_then(|value| match value {
            "Plan" => Some(CollaborationMode::Plan),
            "Build" => Some(CollaborationMode::Build),
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn pending_tool_index_for_request(messages: &[Message], tool_call_id: &str) -> u32 {
    let mut index = 0u32;
    for message in messages {
        for block in &message.content {
            if let ContentBlock::ToolUse { id, .. } = block {
                if id == tool_call_id {
                    return index;
                }
                index += 1;
            }
        }
    }
    index
}

pub(crate) fn permission_request_from_messages(
    messages: &[Message],
    approval_id: &str,
    owner_session_id: SessionId,
    turn_id: TurnId,
    cwd: &Path,
) -> Option<ToolPermissionRequest> {
    for message in messages {
        if message.role != Role::Assistant {
            continue;
        }
        for block in &message.content {
            let ContentBlock::ToolUse { id, name, input } = block else {
                continue;
            };
            if id != approval_id {
                continue;
            }
            let sandbox_permissions =
                devo_core::tools::sandbox_permission_request_from_input(input).ok()?;
            return Some(ToolPermissionRequest {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                input: input.clone(),
                cwd: cwd.to_path_buf(),
                session_id: owner_session_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                resource: devo_safety::ResourceKind::Custom(name.clone()),
                action_summary: format!("Resume tool call {name}"),
                justification: None,
                path: None,
                host: None,
                target: None,
                command_prefix: None,
                command_argv: None,
                command_pattern: None,
                sandbox_permissions,
            });
        }
    }
    None
}

pub(crate) fn latest_approval_checkpoints_for_rollout(
    path: &Path,
) -> std::collections::HashMap<String, TurnApprovalCheckpointRecordedRecord> {
    read_canonical_history(path)
        .map(|history| history.approval_checkpoints)
        .unwrap_or_default()
}

impl ServerRuntime {
    pub(crate) async fn persist_approval_checkpoint(
        &self,
        host_session_id: SessionId,
        owner_session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
    ) -> Result<TurnApprovalCheckpointRecordedRecord, String> {
        let handle = self
            .session(owner_session_id)
            .await
            .ok_or_else(|| "session not found".to_string())?;
        let snapshot = handle
            .approval_checkpoint_snapshot()
            .await
            .ok_or_else(|| "approval checkpoint snapshot unavailable".to_string())?;
        let turn_config_value = serde_json::json!({
            "modelSlug": snapshot.turn_config.model.slug,
            "requestModel": snapshot.turn_config.request_model,
            "modelBindingId": snapshot.turn_config.model_binding_id,
            "reasoningEffortSelection": snapshot.turn_config.reasoning_effort_selection,
            "collaborationMode": format!("{:?}", snapshot.collaboration_mode),
        });
        let pending_tool_index =
            pending_tool_index_for_request(&snapshot.messages, &request.tool_call_id);
        let checkpoint = TurnApprovalCheckpointRecordedRecord {
            schema_version: 1,
            session_id: owner_session_id,
            owner_session_id,
            host_session_id: Some(host_session_id),
            turn_id,
            approval_id: request.tool_call_id.clone(),
            permission_request: serde_json::to_value(StoredToolPermissionRequest::from(request))
                .map_err(|error| format!("serialize permission request: {error}"))?,
            messages: snapshot.messages,
            turn_config: turn_config_value,
            pending_tool_index,
            created_at: Utc::now(),
        };
        let record = self
            .rollout_session_record(owner_session_id)
            .await
            .ok_or_else(|| "session rollout record unavailable".to_string())?;
        self.rollout_store
            .append_approval_checkpoint(&record.rollout_path, &checkpoint)
            .map_err(|error| format!("append approval checkpoint: {error}"))?;
        let _ = handle.mark_active_turn_waiting_approval(turn_id).await;
        Ok(checkpoint)
    }

    async fn rollout_session_record(
        &self,
        session_id: SessionId,
    ) -> Option<devo_core::SessionRecord> {
        let handle = self.session(session_id).await?;
        match handle.record().await {
            Some(Some(record)) => Some(record),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devo_core::tools::{
        AdditionalSandboxPermissions, NetworkPermission, SandboxPermissionRequest,
    };
    use pretty_assertions::assert_eq;

    use super::StoredSandboxPermissionRequest;
    use super::StoredToolPermissionRequest;

    #[test]
    fn sandbox_permissions_round_trip_additional_permissions() {
        let request =
            SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
                network: NetworkPermission::Enabled,
                read_paths: vec![PathBuf::from("/tmp/read")],
                write_paths: vec![PathBuf::from("/tmp/write")],
            });
        let stored = StoredSandboxPermissionRequest::from(&request);
        let restored = stored.into_sandbox_permission_request();
        assert_eq!(restored, request);
    }

    #[test]
    fn stored_tool_permission_request_round_trip() {
        let request = devo_core::tools::ToolPermissionRequest {
            tool_call_id: "tool-1".to_string(),
            tool_name: "shell".to_string(),
            input: serde_json::json!({"command": "echo hi"}),
            cwd: PathBuf::from("/repo"),
            session_id: "ses_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            resource: devo_safety::ResourceKind::ShellExec,
            action_summary: "run echo".to_string(),
            justification: None,
            path: None,
            host: None,
            target: None,
            command_prefix: None,
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: SandboxPermissionRequest::FullEscalation,
        };
        let stored = StoredToolPermissionRequest::from(&request);
        let restored = stored.into_tool_permission_request();
        assert_eq!(restored.tool_call_id, request.tool_call_id);
        assert_eq!(restored.tool_name, request.tool_name);
        assert_eq!(restored.sandbox_permissions, request.sandbox_permissions);
    }
}
