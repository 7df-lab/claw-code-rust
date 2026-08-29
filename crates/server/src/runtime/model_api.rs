use devo_core::ModelCatalogEntry;
use devo_protocol::native::rpc_admin::ModelPreferences;
use devo_protocol::native::rpc_admin::PreferencesOption;

use crate::runtime::handlers::acp_config_options::{
    ACP_MODEL_CONFIG_ID, ACP_REASONING_EFFORT_CONFIG_ID,
};
use crate::session_context::SessionRuntimeContext;
use crate::{ProtocolErrorCode, SuccessResponse};

use super::ServerRuntime;

/// Projects the ACP config-option selects into canonical model preferences
/// (ratified #12): the model select becomes `model` + `available_models`,
/// the reasoning-effort select becomes `reasoning_effort` +
/// `available_efforts`. Each `available_models` entry also carries that
/// model's own `available_efforts` from the catalog.
fn model_preferences_from_config_options(
    options: &[devo_core::AcpSessionConfigOption],
    runtime_context: &SessionRuntimeContext,
) -> ModelPreferences {
    let mut preferences = ModelPreferences {
        model: None,
        reasoning_effort: None,
        available_models: Vec::new(),
        available_efforts: Vec::new(),
    };
    for option in options {
        let devo_core::AcpSessionConfigOption::Select {
            id,
            current_value,
            options: select_options,
            ..
        } = option
        else {
            continue;
        };
        // Preferences are flat lists; grouped selects are flattened in order.
        let entries: Vec<PreferencesOption> = match select_options {
            devo_core::AcpSessionConfigSelectOptions::Ungrouped(entries) => entries.clone(),
            devo_core::AcpSessionConfigSelectOptions::Grouped(groups) => groups
                .iter()
                .flat_map(|group| group.options.clone())
                .collect(),
        }
        .into_iter()
        .map(|entry| PreferencesOption {
            value: entry.value.to_string(),
            label: entry.name,
            description: entry.description,
            available_efforts: Vec::new(),
        })
        .collect();
        match id.as_str() {
            ACP_MODEL_CONFIG_ID => {
                preferences.model = Some(current_value.to_string());
                preferences.available_models = entries;
            }
            ACP_REASONING_EFFORT_CONFIG_ID => {
                preferences.reasoning_effort = Some(current_value.to_string());
                preferences.available_efforts = entries;
            }
            _ => {}
        }
    }
    enrich_available_models_with_efforts(&mut preferences, runtime_context);
    preferences
}

fn enrich_available_models_with_efforts(
    preferences: &mut ModelPreferences,
    runtime_context: &SessionRuntimeContext,
) {
    for model_option in &mut preferences.available_models {
        let turn_config =
            runtime_context.resolve_turn_config(Some(model_option.value.as_str()), None);
        model_option.available_efforts = turn_config
            .model
            .effective_reasoning_capability()
            .options()
            .into_iter()
            .map(|option| PreferencesOption {
                value: option.value,
                label: option.label,
                description: Some(option.description),
                available_efforts: Vec::new(),
            })
            .collect();
    }
}

impl ServerRuntime {
    async fn model_config_runtime_context(
        &self,
        cwd: Option<&std::path::Path>,
        method: &str,
    ) -> Result<
        std::sync::Arc<crate::session_context::SessionRuntimeContext>,
        (ProtocolErrorCode, String),
    > {
        match cwd {
            Some(cwd) if !cwd.is_absolute() => Err((
                ProtocolErrorCode::InvalidParams,
                format!("{method} cwd must be an absolute path"),
            )),
            Some(cwd) => self.deps.context_for_workspace(cwd).await.map_err(|error| {
                (
                    ProtocolErrorCode::InternalError,
                    format!(
                        "failed to load model config for cwd {}: {error}",
                        cwd.display()
                    ),
                )
            }),
            None => Ok(self.deps.process_context.clone()),
        }
    }

    /// Native `model/preferences/read` (ratified #12): the workspace's
    /// effective model defaults plus selectable values, projected from the
    /// same context source as the ACP model configuration options.
    pub(super) async fn handle_native_model_preferences_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ModelPreferencesReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical model/preferences/read params: {error}"),
                    );
                }
            };
        let runtime_context = match self
            .model_config_runtime_context(params.cwd.as_deref(), "model/preferences/read")
            .await
        {
            Ok(runtime_context) => runtime_context,
            Err((code, message)) => return self.error_response(request_id, code, message),
        };
        let options = self.acp_model_config_options_for_context(&runtime_context);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ModelPreferencesReadResult {
                preferences: model_preferences_from_config_options(&options, &runtime_context),
            },
        })
        .expect("serialize canonical model/preferences/read response")
    }

    /// Native `model/preferences/write` (ratified #12): patch semantics,
    /// naturally idempotent absolute writes into the user config, validated
    /// against the selectable values.
    pub(super) async fn handle_native_model_preferences_write(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ModelPreferencesWriteParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical model/preferences/write params: {error}"),
                    );
                }
            };
        let runtime_context = match self
            .model_config_runtime_context(params.cwd.as_deref(), "model/preferences/write")
            .await
        {
            Ok(runtime_context) => runtime_context,
            Err((code, message)) => return self.error_response(request_id, code, message),
        };
        let preferences = model_preferences_from_config_options(
            &self.acp_model_config_options_for_context(&runtime_context),
            &runtime_context,
        );
        for (config_id, value) in [
            (ACP_MODEL_CONFIG_ID, params.patch.model.as_ref()),
            (
                ACP_REASONING_EFFORT_CONFIG_ID,
                params.patch.reasoning_effort.as_ref(),
            ),
        ] {
            let Some(value) = value else {
                continue;
            };
            let allowed = match config_id {
                ACP_MODEL_CONFIG_ID => preferences
                    .available_models
                    .iter()
                    .any(|option| option.value == *value),
                ACP_REASONING_EFFORT_CONFIG_ID => preferences
                    .available_efforts
                    .iter()
                    .any(|option| option.value == *value),
                _ => false,
            };
            if !allowed {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid value '{value}' for model preference '{config_id}'"),
                );
            }
            let config_file = {
                let store = runtime_context
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
                .config_change_hook_block_reason("user_settings", Some(config_file))
                .await
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::PolicyDenied,
                    format!("config change blocked by hook: {reason}"),
                );
            }
            {
                let mut store = runtime_context
                    .config_store
                    .lock()
                    .expect("app config store mutex should not be poisoned");
                if let Err(error) = store.set_model_config_option(config_id, value) {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        error.to_string(),
                    );
                }
            }
        }
        self.deps.invalidate_workspace_contexts();

        let options = self.acp_model_config_options_for_context(&runtime_context);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ModelPreferencesWriteResult {
                preferences: model_preferences_from_config_options(&options, &runtime_context),
            },
        })
        .expect("serialize canonical model/preferences/write response")
    }

    /// Native `model/list` (L2-DES-APP-008): the same catalog source as
    /// `model/catalog`, projected to the parity canonical `ModelInfo` shape
    /// (ratified Open Decision #7).
    pub(super) async fn handle_native_model_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if let Err(error) =
            serde_json::from_value::<devo_protocol::native::rpc_admin::ModelListParams>(params)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid canonical model/list params: {error}"),
            );
        }
        let models = self
            .deps
            .model_catalog
            .list_visible()
            .into_iter()
            .map(|model| {
                devo_protocol::native::rpc_admin::ModelInfo::from(ModelCatalogEntry::from(model))
            })
            .collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ModelListResult { models },
        })
        .expect("serialize canonical model/list response")
    }
}
