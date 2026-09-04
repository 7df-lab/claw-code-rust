use anyhow::Context;
use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::Model;
use devo_core::ModelCatalog;
use devo_core::PROVIDER_CONFIG_FILE_NAME;
use devo_core::PresetModelCatalog;
use devo_core::ProviderHttpConfig;
use devo_core::UserAuthConfigFile;
use devo_core::read_user_auth_config;
use devo_core::test_model_connection;
use devo_protocol::ModelProfileKey;
use devo_provider::ProviderHttpOptions;
use devo_util_paths::current_user_config_file;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;

use super::ServerRuntime;

impl ServerRuntime {
    /// Native `provider/list` (ratified #11), backed by the bundled provider
    /// directory plus the effective config store and projected into the
    /// canonical camelCase result.
    pub(super) async fn handle_native_provider_list(
        &self,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let mut providers = self.deps.model_catalog.list_providers();
        let template_provider_ids = self.deps.model_catalog.list_template_provider_ids();
        let store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let config = store.effective_config();
        let live_catalog = PresetModelCatalog::load_from_provider_config_with_overrides(
            &config.provider_catalog,
            &config.provider.model_overrides,
        )
        .ok();
        let catalog: &dyn ModelCatalog = live_catalog
            .as_ref()
            .map(|catalog| catalog as &dyn ModelCatalog)
            .unwrap_or(self.deps.model_catalog.as_ref());
        let connected_provider_ids = match store.provider_connection_ids() {
            Ok(provider_ids) => provider_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider connections: {error}"),
                );
            }
        };
        let connection_models = match store.provider_connection_models() {
            Ok(models) => models,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider Connection models: {error}"),
                );
            }
        };
        let configured_providers = match store.provider_connections() {
            Ok(providers) => providers,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider Connections: {error}"),
                );
            }
        };
        for configured in configured_providers {
            if let Some(directory_entry) =
                providers.iter_mut().find(|entry| entry.id == configured.id)
            {
                *directory_entry = configured;
            } else {
                providers.push(configured);
            }
        }
        let providers = providers
            .into_iter()
            .map(|provider| {
                let mut info = canonical_provider_info(provider, catalog);
                if let Some(config) = store
                    .effective_config()
                    .provider_catalog
                    .providers
                    .get(&info.id)
                {
                    info.options = config.options.clone();
                    info.request = config.request.clone();
                    if let Some(headers) = &config.headers {
                        info.headers = headers.clone();
                    }
                }
                info
            })
            .collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderListResult {
                providers,
                template_provider_ids,
                connected_provider_ids,
                connection_models,
            },
        })
        .expect("serialize canonical provider/list response")
    }

    /// Native `provider/upsert` (ratified #11).
    pub(super) async fn handle_native_provider_upsert(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderUpsertParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/upsert params: {error}"),
                    );
                }
            };
        let Some(_provider_id) = normalized_provider_id(&params.provider.id)
            .or_else(|| normalized_provider_id(&params.provider.name))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider name cannot be empty",
            );
        };
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
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

        let mut store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let default_model = params.default_model.clone();
        let small_model = params.small_model.clone();
        let provider = match store.upsert_provider_connection(
            params.provider,
            params.default_model,
            params.small_model,
            params.api_key,
        ) {
            Ok(provider) => provider,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    error.to_string(),
                );
            }
        };
        drop(store);
        self.deps.invalidate_workspace_contexts();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderUpsertResult {
                provider,
                default_model,
                small_model,
            },
        })
        .expect("serialize canonical provider/upsert response")
    }

    /// Native provider/disconnect removes one user Connection while leaving
    /// the built-in provider directory untouched.
    pub(super) async fn handle_native_provider_disconnect(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderDisconnectParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/disconnect params: {error}"),
                    );
                }
            };
        let provider_id = params.provider_id.trim();
        if provider_id.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider_id cannot be empty",
            );
        }
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
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

        let mut store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        if let Err(error) = store.disconnect_provider(provider_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            );
        }
        drop(store);
        self.deps.invalidate_workspace_contexts();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderDisconnectResult {
                provider_id: provider_id.to_string(),
            },
        })
        .expect("serialize canonical provider/disconnect response")
    }

    /// Native provider/model/remove removes one model from a user Connection
    /// while leaving the provider template and its built-in models untouched.
    pub(super) async fn handle_native_provider_model_remove(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderModelRemoveParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/model/remove params: {error}"),
                    );
                }
            };
        let provider_id = params.provider_id.trim();
        let model_id = params.model_id.trim();
        if provider_id.is_empty() || model_id.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider_id and model_id cannot be empty",
            );
        }
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
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

        let mut store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let connected_provider_ids = match store.provider_connection_ids() {
            Ok(provider_ids) => provider_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider connections: {error}"),
                );
            }
        };
        if !connected_provider_ids.iter().any(|id| id == provider_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("provider {provider_id} is not a user Connection"),
            );
        }
        if let Err(error) = store.remove_provider_model(provider_id, model_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            );
        }
        drop(store);
        self.deps.invalidate_workspace_contexts();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderModelRemoveResult {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            },
        })
        .expect("serialize canonical provider/model/remove response")
    }

    /// Native `provider/validate` (ratified #11).
    pub(super) async fn handle_native_provider_validate(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderValidateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/validate params: {error}"),
                    );
                }
            };
        let Some(provider_id) = normalized_provider_id(&params.provider.id)
            .or_else(|| normalized_provider_id(&params.provider.name))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider name cannot be empty",
            );
        };
        if params.model.trim().is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "model cannot be empty",
            );
        }
        let _ = provider_id;
        let provider_http = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store.effective_config().provider_http.clone()
        };

        match validate_provider_candidate(params, self.deps.model_catalog.as_ref(), provider_http)
            .await
        {
            Ok(reply_preview) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_admin::ProviderValidateResult { reply_preview },
            })
            .expect("serialize canonical provider/validate response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            ),
        }
    }
}

/// Completes a provider entry with the model metadata from the bundled
/// directory when the caller supplied only a connection and model id.
///
/// Connection overlays win on key conflicts. Disabled models remain present
/// so settings UIs can show them with `enabled=false`.
fn canonical_provider_info(
    mut provider: devo_protocol::ProviderInfo,
    catalog: &dyn ModelCatalog,
) -> devo_protocol::ProviderInfo {
    let catalog_models = catalog.list_provider_models(&provider.id);
    if provider.models.is_empty() {
        provider.models = catalog_models;
    } else if !catalog_models.is_empty() {
        let mut merged = catalog_models;
        for (model_id, model) in provider.models {
            merged.insert(model_id, model);
        }
        provider.models = merged;
    }
    provider
}
fn normalized_provider_id(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

async fn validate_provider_candidate(
    params: devo_protocol::native::rpc_admin::ProviderValidateParams,
    catalog: &dyn ModelCatalog,
    provider_http: ProviderHttpConfig,
) -> anyhow::Result<String> {
    let provider_id = normalized_provider_id(&params.provider.id)
        .or_else(|| normalized_provider_id(&params.provider.name))
        .context("provider name cannot be empty")?;
    if params.provider.wire_apis.is_empty() {
        anyhow::bail!("wire_apis must contain at least one wire API");
    }

    let requested_model_id = params
        .model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(&params.model);
    let (model_id, model_info) = if let Some(model) = params.provider.models.get(requested_model_id)
    {
        (requested_model_id.to_string(), model)
    } else if let Some((base_model_id, variant_id)) = requested_model_id.rsplit_once('/') {
        let model = params
            .provider
            .models
            .get(base_model_id)
            .filter(|model| model.variants.contains_key(variant_id))
            .context("model variant is not present in provider directory")?;
        (base_model_id.to_string(), model)
    } else {
        anyhow::bail!(
            "model {} is not present in provider directory",
            params.model
        );
    };
    let wire_api = model_info
        .wire_api
        .or_else(|| params.provider.wire_apis.first().copied())
        .context("provider has no usable wire API")?;
    if !params.provider.wire_apis.contains(&wire_api) {
        anyhow::bail!("model wire API must be supported by provider");
    }

    let model_ref = format!("{provider_id}/{model_id}");
    let (validation_model, model_profile) = resolve_validation_model(catalog, wire_api, &model_ref);
    let api_key = resolve_validation_api_key(&provider_id, &params)?;
    let headers = (!params.provider.headers.is_empty())
        .then(|| serde_json::to_string(&params.provider.headers))
        .transpose()?;
    let provider = crate::provider_config::build_provider_adapter(
        wire_api,
        params.provider.base_url.clone(),
        api_key,
        ProviderHttpOptions::from_raw_with_no_proxy(
            provider_http.proxy_url,
            provider_http.no_proxy,
            headers,
        )?,
    )?;

    test_model_connection(
        provider.as_ref(),
        &validation_model,
        model_profile,
        requested_model_id,
        "Reply with OK only.",
    )
    .await
    .map_err(Into::into)
}
fn resolve_validation_model(
    catalog: &dyn ModelCatalog,
    wire_api: devo_core::ProviderWireApi,
    model_slug: &str,
) -> (Model, ModelProfileKey) {
    if let Some(entry) = catalog.get(model_slug) {
        let mut model = entry.clone();
        model.provider = wire_api;
        return (model, ModelProfileKey::CatalogSlug(model_slug.to_string()));
    }
    (
        Model {
            slug: model_slug.to_string(),
            display_name: model_slug.to_string(),
            provider: wire_api,
            ..Model::default()
        },
        ModelProfileKey::Generic,
    )
}

fn resolve_validation_api_key(
    provider_id: &str,
    params: &devo_protocol::native::rpc_admin::ProviderValidateParams,
) -> anyhow::Result<Option<String>> {
    if let Some(api_key) = params.api_key.as_deref() {
        let trimmed = api_key.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }

    let Some(credential_id) = params.provider.credential.as_deref() else {
        return Ok(None);
    };
    let auth = current_server_user_auth_config()?;
    let credential = auth.credentials.get(credential_id).with_context(|| {
        format!(
            "provider {provider_id} references missing credential {credential_id} in user auth.json"
        )
    })?;
    Ok(Some(credential.value.clone()))
}

fn current_server_user_auth_config() -> anyhow::Result<UserAuthConfigFile> {
    let config_file = current_user_config_file().context("could not determine user config path")?;
    let config_dir = config_file
        .parent()
        .context("user config path has no parent directory")?;
    read_user_auth_config(&config_dir.join(AUTH_CONFIG_FILE_NAME)).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_core::PresetModelCatalog;
    use devo_core::ProviderWireApi;
    use devo_protocol::ProviderInfo;
    use devo_protocol::ProviderModelInfo;
    use devo_protocol::native::rpc_admin::ProviderValidateParams;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn normalized_provider_id_trims_and_rejects_empty_names() {
        assert_eq!(
            normalized_provider_id(" openai "),
            Some("openai".to_string())
        );
        assert_eq!(normalized_provider_id("   "), None);
    }

    #[test]
    fn resolve_validation_model_preserves_runtime_catalog_profile() {
        let catalog = PresetModelCatalog::new(vec![Model {
            slug: "catalog-slug".to_string(),
            display_name: "Catalog Model".to_string(),
            context_window: 123_456,
            effective_context_window_percent: Some(70.0),
            max_tokens: Some(7_654),
            provider: ProviderWireApi::AnthropicMessages,
            ..Model::default()
        }]);

        let resolved = resolve_validation_model(
            &catalog,
            ProviderWireApi::OpenAIChatCompletions,
            "catalog-slug",
        );

        assert_eq!(
            resolved,
            (
                Model {
                    slug: "catalog-slug".to_string(),
                    display_name: "Catalog Model".to_string(),
                    context_window: 123_456,
                    effective_context_window_percent: Some(70.0),
                    max_tokens: Some(7_654),
                    provider: ProviderWireApi::OpenAIChatCompletions,
                    ..Model::default()
                },
                ModelProfileKey::CatalogSlug("catalog-slug".to_string()),
            )
        );
    }

    #[test]
    fn resolve_validation_model_uses_generic_profile_for_unknown_slug() {
        let resolved = resolve_validation_model(
            &PresetModelCatalog::default(),
            ProviderWireApi::OpenAIChatCompletions,
            "custom-catalog-slug",
        );

        assert_eq!(
            resolved,
            (
                Model {
                    slug: "custom-catalog-slug".to_string(),
                    display_name: "custom-catalog-slug".to_string(),
                    provider: ProviderWireApi::OpenAIChatCompletions,
                    ..Model::default()
                },
                ModelProfileKey::Generic,
            )
        );
    }

    /// Trace: L2-DES-APP-005, L2-DES-MODEL-001
    /// Verifies: provider validation applies provider custom header parsing before sending a validation request.
    #[tokio::test]
    async fn validate_provider_candidate_rejects_invalid_custom_headers() {
        let params = ProviderValidateParams {
            provider: ProviderInfo {
                id: "openai".to_string(),
                name: "openai".to_string(),
                description: None,
                base_url: Some("http://provider.example/v1".to_string()),
                credential: None,
                headers: BTreeMap::from([("bad header".to_string(), "value".to_string())]),
                options: None,
                request: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::from([(
                    "test-model".to_string(),
                    ProviderModelInfo {
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
            },
            model: "test-model".to_string(),
            api_key: None,
        };
        let catalog = PresetModelCatalog::new(Vec::new());

        let error = validate_provider_candidate(params, &catalog, ProviderHttpConfig::default())
            .await
            .expect_err("invalid headers should reject validation");

        assert_eq!(
            error.to_string(),
            "invalid provider custom header name `bad header`"
        );
    }
}
