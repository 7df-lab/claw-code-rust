use super::ServerRuntime;
use crate::{ProtocolErrorCode, SuccessResponse};

impl ServerRuntime {
    /// Native `skill/list` (ratified #4): workspace-scoped via `cwd` and
    /// Native `SkillInfo` records keyed by path.
    pub(super) async fn handle_native_skill_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::SkillListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid native skill/list params: {error}"),
                    );
                }
            };
        let skills = match params.cwd.as_deref() {
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(runtime_context) => {
                    runtime_context.discover_skills(Some(cwd), params.force_reload)
                }
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize skills workspace: {error}"),
                    );
                }
            },
            None => self.deps.discover_skills(None, params.force_reload),
        };
        match skills {
            Ok(skills) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_admin::SkillListResult {
                    skills: skills
                        .into_iter()
                        .map(devo_protocol::native::rpc_admin::SkillInfo::from)
                        .collect(),
                },
            })
            .expect("serialize native skill/list response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to discover skills: {error}"),
            ),
        }
    }

    /// Native `skill/set_enabled` (ratified #4): keyed by `path`.
    pub(super) async fn handle_native_skill_set_enabled(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::SkillSetEnabledParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid native skill/set_enabled params: {error}"),
                    );
                }
            };
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join("config.toml")
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("skills", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("skill config change blocked by hook: {reason}"),
            );
        }

        let skills = match params.cwd.as_deref() {
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(runtime_context) => {
                    runtime_context.set_skill_enabled(params.path, params.enabled, Some(cwd))
                }
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize skills workspace: {error}"),
                    );
                }
            },
            None => self
                .deps
                .set_skill_enabled(params.path, params.enabled, None),
        };
        match skills {
            Ok(skills) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_admin::SkillSetEnabledResult {
                    skills: skills
                        .into_iter()
                        .map(devo_protocol::native::rpc_admin::SkillInfo::from)
                        .collect(),
                },
            })
            .expect("serialize native skill/set_enabled response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to update skill config: {error}"),
            ),
        }
    }
}
