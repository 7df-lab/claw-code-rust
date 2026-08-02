use super::ServerRuntime;
use crate::{
    ProtocolErrorCode, SkillChangedParams, SkillChangedResult, SkillListParams, SkillListResult,
    SkillSetEnabledParams, SkillSetEnabledResult, SuccessResponse,
};

impl ServerRuntime {
    pub(super) async fn handle_skills_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<SkillListParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid skills/list params: {error}"),
                );
            }
        };

        let skills = match params.cwd.as_deref() {
            // With cwd: workspace context (LRU-cached). Skill discovery uses that
            // catalog so project skill overlays apply.
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
                result: SkillListResult { skills },
            })
            .expect("serialize skills/list response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to discover skills: {error}"),
            ),
        }
    }

    pub(super) async fn handle_skills_changed(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<SkillChangedParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid skills/changed params: {error}"),
                );
            }
        };

        let skills = match params.cwd.as_deref() {
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(runtime_context) => runtime_context.discover_skills(Some(cwd), true),
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize skills workspace: {error}"),
                    );
                }
            },
            None => self.deps.discover_skills(None, true),
        };

        match skills {
            Ok(skills) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: SkillChangedResult { skills },
            })
            .expect("serialize skills/changed response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to discover skills: {error}"),
            ),
        }
    }

    pub(super) async fn handle_skills_set_enabled(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<SkillSetEnabledParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid skills/set_enabled params: {error}"),
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

        match self
            .deps
            .set_skill_enabled(params.path, params.enabled, None)
        {
            Ok(skills) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: SkillSetEnabledResult { skills },
            })
            .expect("serialize skills/set_enabled response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to update skill config: {error}"),
            ),
        }
    }
}
