use devo_core::ModelCatalogEntry;
use devo_core::ModelCatalogParams;
use devo_core::ModelCatalogResult;
use devo_core::ModelConfigParams;
use devo_core::ModelConfigResult;
use devo_core::ModelConfigSetParams;
use devo_core::ModelSavedEntry;
use devo_core::ModelSavedParams;
use devo_core::ModelSavedResult;
use devo_core::ProviderWireApi;

use crate::runtime::handlers::acp_config_options::{
    ACP_MODEL_CONFIG_ID, ACP_REASONING_EFFORT_CONFIG_ID, select_options_contain_value,
};
use crate::{ProtocolErrorCode, SuccessResponse};

use super::ServerRuntime;

impl ServerRuntime {
    pub(super) async fn handle_model_config(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<ModelConfigParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid model/config params: {error}"),
                );
            }
        };

        let runtime_context = match params.cwd.as_deref() {
            Some(cwd) if !cwd.is_absolute() => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "model/config cwd must be an absolute path".to_string(),
                );
            }
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(context) => context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!(
                            "failed to load model config for cwd {}: {error}",
                            cwd.display()
                        ),
                    );
                }
            },
            None => self.deps.process_context.clone(),
        };

        let config_options = self.acp_model_config_options_for_context(&runtime_context);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ModelConfigResult { config_options },
        })
        .expect("serialize model/config response")
    }

    pub(super) async fn handle_model_config_set(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<ModelConfigSetParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid model/config/set params: {error}"),
                );
            }
        };

        let runtime_context = match params.cwd.as_deref() {
            Some(cwd) if !cwd.is_absolute() => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "model/config/set cwd must be an absolute path".to_string(),
                );
            }
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(context) => context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!(
                            "failed to load model config for cwd {}: {error}",
                            cwd.display()
                        ),
                    );
                }
            },
            None => self.deps.process_context.clone(),
        };

        match params.config_id.as_str() {
            ACP_MODEL_CONFIG_ID | ACP_REASONING_EFFORT_CONFIG_ID => {}
            _ => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("unknown model config option '{}'", params.config_id),
                );
            }
        }

        let config_options = self.acp_model_config_options_for_context(&runtime_context);
        let Some(config_option) = config_options.iter().find(|option| match option {
            devo_core::AcpSessionConfigOption::Select { id, .. } => id == &params.config_id,
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("unknown model config option '{}'", params.config_id),
            );
        };
        let value_is_allowed = match config_option {
            devo_core::AcpSessionConfigOption::Select { options, .. } => {
                select_options_contain_value(options, &params.value)
            }
        };
        if !value_is_allowed {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!(
                    "invalid value '{}' for model config option '{}'",
                    params.value, params.config_id
                ),
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
            if let Err(error) = store.set_model_config_option(&params.config_id, &params.value) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    error.to_string(),
                );
            }
        }
        self.deps.invalidate_workspace_contexts();

        let config_options = self.acp_model_config_options_for_context(&runtime_context);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ModelConfigResult { config_options },
        })
        .expect("serialize model/config/set response")
    }

    pub(super) async fn handle_model_catalog(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if let Err(error) = serde_json::from_value::<ModelCatalogParams>(params) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid model/catalog params: {error}"),
            );
        }

        let catalog = &self.deps.model_catalog;
        let models: Vec<ModelCatalogEntry> = catalog
            .list_visible()
            .into_iter()
            .map(ModelCatalogEntry::from)
            .collect();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ModelCatalogResult { models },
        })
        .expect("serialize model/catalog response")
    }

    pub(super) async fn handle_model_saved(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if let Err(error) = serde_json::from_value::<ModelSavedParams>(params) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid model/saved params: {error}"),
            );
        }

        let config = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider
            .clone();

        let catalog = &self.deps.model_catalog;
        let mut models = Vec::new();

        for binding in config
            .model_bindings
            .values()
            .filter(|binding| binding.enabled)
        {
            let slug = binding.model_slug.clone();
            let catalog_model = catalog.get(&slug);
            models.push(ModelSavedEntry {
                slug: slug.clone(),
                display_name: binding
                    .display_name
                    .clone()
                    .or_else(|| catalog_model.map(|m| m.display_name.clone()))
                    .unwrap_or_else(|| slug.clone()),
                channel: catalog_model.and_then(|m| m.channel.clone()),
                description: catalog_model.and_then(|m| m.description.clone()),
                provider_id: binding.provider.clone(),
                wire_api: binding.invocation_method,
                context_window: catalog_model.map(|m| m.context_window).unwrap_or(200_000),
            });
        }

        for (provider_id, provider_config) in &config.model_providers {
            let wire_api = provider_config
                .wire_api
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
            models.extend(provider_config.models.iter().map(|configured| {
                let slug = configured.model.clone();
                let catalog_model = catalog.get(&slug);
                ModelSavedEntry {
                    slug: slug.clone(),
                    display_name: catalog_model
                        .map(|m| m.display_name.clone())
                        .unwrap_or_else(|| slug.clone()),
                    channel: catalog_model.and_then(|m| m.channel.clone()),
                    description: catalog_model.and_then(|m| m.description.clone()),
                    provider_id: provider_id.clone(),
                    wire_api,
                    context_window: catalog_model.map(|m| m.context_window).unwrap_or(200_000),
                }
            }));
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ModelSavedResult { models },
        })
        .expect("serialize model/saved response")
    }
}
