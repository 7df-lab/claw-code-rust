use anyhow::Context;
use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::Model;
use devo_core::ModelCatalog;
use devo_core::ProviderHttpConfig;
use devo_core::ProviderValidateParams;
use devo_core::ProviderValidateResult;
use devo_core::ProviderVendorConfig;
use devo_core::ProviderVendorListParams;
use devo_core::ProviderVendorListResult;
use devo_core::ProviderVendorUpsertParams;
use devo_core::ProviderVendorUpsertResult;
use devo_core::UserAuthConfigFile;
use devo_core::read_user_auth_config;
use devo_core::test_model_connection;
use devo_protocol::ModelProfileKey;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderHttpOptions;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::openai::OpenAIProvider;
use devo_provider::openai::OpenAIResponsesProvider;
use devo_util_paths::current_user_config_file;
use std::time::Duration;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;
use crate::provider_config::normalize_openai_base_url;

use super::ServerRuntime;

impl ServerRuntime {
    pub(super) async fn handle_provider_vendor_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if !params.is_null()
            && let Err(error) = serde_json::from_value::<ProviderVendorListParams>(params)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid provider/list params: {error}"),
            );
        }

        let store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let provider_vendors = store.provider_vendors();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ProviderVendorListResult { provider_vendors },
        })
        .expect("serialize provider/list response")
    }

    pub(super) async fn handle_provider_vendor_upsert(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ProviderVendorUpsertParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid provider/upsert params: {error}"),
                );
            }
        };

        let Some(provider_id) = normalized_provider_id(&params.provider_vendor.name) else {
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

        let mut store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let model_binding = params.model_binding;
        let default_model_binding = params.default_model_binding;
        let api_key = params.api_key;
        let provider_vendor = match store.upsert_provider_vendor(
            provider_id,
            params.provider_vendor,
            model_binding.clone(),
            default_model_binding,
            api_key,
        ) {
            Ok(provider_vendor) => provider_vendor,
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
            result: ProviderVendorUpsertResult {
                provider_vendor,
                model_binding,
            },
        })
        .expect("serialize provider/upsert response")
    }

    pub(super) async fn handle_provider_validate(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ProviderValidateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid provider/validate params: {error}"),
                );
            }
        };

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
                result: ProviderValidateResult { reply_preview },
            })
            .expect("serialize provider/validate response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            ),
        }
    }
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
    params: ProviderValidateParams,
    catalog: &dyn ModelCatalog,
    provider_http: ProviderHttpConfig,
) -> anyhow::Result<String> {
    let provider_id = normalized_provider_id(&params.provider_vendor.name)
        .context("provider name cannot be empty")?;
    if params.model_binding.provider.trim() != provider_id {
        anyhow::bail!("model binding provider must match provider vendor");
    }
    if params.model_binding.request_model.trim().is_empty() {
        anyhow::bail!("model binding request_model cannot be empty");
    }
    if params.provider_vendor.wire_apis.is_empty() {
        anyhow::bail!("wire_apis must contain at least one wire API");
    }
    if !params
        .provider_vendor
        .wire_apis
        .contains(&params.model_binding.invocation_method)
    {
        anyhow::bail!("model binding invocation_method must be supported by provider vendor");
    }

    let (validation_model, model_profile) = resolve_validation_model(
        catalog,
        params.model_binding.invocation_method,
        &params.model_binding.model_slug,
    );
    let api_key = resolve_validation_api_key(&provider_id, &params)?;
    let provider = build_validation_provider(
        params.model_binding.invocation_method,
        params.provider_vendor.base_url,
        api_key,
        ProviderHttpOptions::from_raw_with_no_proxy(
            provider_http.proxy_url,
            provider_http.no_proxy,
            params.provider_vendor.headers.clone(),
        )?,
    )?;

    tokio::time::timeout(
        Duration::from_secs(20),
        test_model_connection(
            provider.as_ref(),
            &validation_model,
            model_profile,
            &params.model_binding.request_model,
            "Reply with OK only.",
        ),
    )
    .await
    .context("provider validation timed out after 20s")?
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
    params: &ProviderValidateParams,
) -> anyhow::Result<Option<String>> {
    if let Some(api_key) = params.api_key.as_deref() {
        let trimmed = api_key.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }

    let provider_config = ProviderVendorConfig {
        name: params.provider_vendor.name.clone(),
        base_url: params.provider_vendor.base_url.clone(),
        credential: params.provider_vendor.credential.clone(),
        headers: params.provider_vendor.headers.clone(),
        wire_apis: params.provider_vendor.wire_apis.clone(),
        web_search: None,
        web_fetch: None,
        enabled: params.provider_vendor.enabled,
    };
    resolve_provider_api_key(
        provider_id,
        &provider_config,
        &current_server_user_auth_config()?,
    )
}

fn build_validation_provider(
    wire_api: devo_core::ProviderWireApi,
    base_url: Option<String>,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
) -> anyhow::Result<Box<dyn ModelProviderSDK>> {
    match wire_api {
        devo_core::ProviderWireApi::AnthropicMessages => {
            let api_key = api_key.context("anthropic provider requires an API key")?;
            let base_url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
            Ok(Box::new(
                AnthropicProvider::new(base_url)
                    .with_http_options(http_options)?
                    .with_api_key(api_key),
            ))
        }
        devo_core::ProviderWireApi::OpenAIChatCompletions => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider = OpenAIProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Ok(Box::new(provider))
        }
        devo_core::ProviderWireApi::OpenAIResponses => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider =
                OpenAIResponsesProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Ok(Box::new(provider))
        }
    }
}

fn current_server_user_auth_config() -> anyhow::Result<UserAuthConfigFile> {
    let config_file = current_user_config_file().context("could not determine user config path")?;
    let config_dir = config_file
        .parent()
        .context("user config path has no parent directory")?;
    read_user_auth_config(&config_dir.join(AUTH_CONFIG_FILE_NAME)).map_err(Into::into)
}

fn resolve_provider_api_key(
    provider_id: &str,
    provider: &ProviderVendorConfig,
    auth: &UserAuthConfigFile,
) -> anyhow::Result<Option<String>> {
    let Some(credential_id) = provider.credential.as_deref() else {
        return Ok(None);
    };
    let credential = auth.credentials.get(credential_id).with_context(|| {
        format!(
            "provider `{provider_id}` references missing credential `{credential_id}` in user auth.json"
        )
    })?;
    Ok(Some(credential.value.clone()))
}

#[cfg(test)]
mod tests {
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderWireApi;
    use devo_protocol::ProviderModelBinding;
    use devo_protocol::ProviderValidateParams;
    use devo_protocol::ProviderVendor;
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
            effective_context_window_percent: Some(70),
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
                    effective_context_window_percent: Some(70),
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
            provider_vendor: ProviderVendor {
                name: "openai".to_string(),
                base_url: Some("http://provider.example/v1".to_string()),
                credential: None,
                headers: Some(r#"{"bad header":"value"}"#.to_string()),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            },
            model_binding: ProviderModelBinding {
                binding_id: "main".to_string(),
                model_slug: "test-model".to_string(),
                provider: "openai".to_string(),
                request_model: "test-model".to_string(),
                display_name: None,
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: None,
                enabled: true,
            },
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
