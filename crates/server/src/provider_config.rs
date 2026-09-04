//! Server-side provider bootstrap and routing.
//!
//! This module keeps provider construction at the runtime boundary: config and
//! auth are resolved once into concrete provider adapters, while later turns
//! select between those adapters through a route-aware facade.

use anyhow::Context;
use anyhow::Result;

use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::AppConfig;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderConfigEntry;
use devo_core::ProviderConfigFile;
use devo_core::ProviderHttpConfig;
use devo_core::ProviderWireApi;
use devo_core::UserAuthConfigFile;
use devo_core::read_user_auth_config;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::MultiProviderRouter;
use devo_provider::ProviderHttpOptions;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use devo_provider::SingleProviderRouter;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::openai::OpenAIProvider;
use devo_provider::openai::OpenAIResponsesProvider;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

const NO_PROVIDER_CONFIGURED_MESSAGE: &str =
    "No provider configured. Run `devo onboard` to complete setup.";

/// Resolved provider bootstrap owned by the server runtime.
pub struct ResolvedServerProvider {
    /// Concrete provider used for model requests.
    pub provider: Arc<dyn ModelProviderSDK>,
    /// Route-aware provider facade used for model requests.
    pub provider_router: Arc<dyn ProviderRouter>,
    /// Default model slug used when a session or turn does not request one.
    pub default_model: String,
}

/// Loads the server-side provider from a merged app config.
pub fn load_server_provider(
    app_config: &AppConfig,
    default_model: Option<&str>,
    user_config_dir: &Path,
) -> Result<ResolvedServerProvider> {
    if !app_config.has_provider_configuration() {
        let default_model = match default_model {
            Some(default_model) => default_model.to_string(),
            None => PresetModelCatalog::load()?
                .resolve_for_turn(None)?
                .slug
                .clone(),
        };
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(MissingProvider);
        return Ok(ResolvedServerProvider {
            provider: Arc::clone(&provider),
            provider_router: Arc::new(SingleProviderRouter::new(provider)),
            default_model,
        });
    }

    let provider_config = app_config.provider_catalog_config();
    let auth = read_user_auth_config(&user_config_dir.join(AUTH_CONFIG_FILE_NAME))?;
    let selection = resolve_server_model(&provider_config, default_model)?;
    let provider_config_entry = provider_config
        .providers
        .get(&selection.provider_id)
        .with_context(|| {
            format!(
                "configured provider Connection `{}` was not found",
                selection.provider_id
            )
        })?;
    let provider = build_provider_route(
        selection.wire_api,
        &selection.provider_id,
        provider_config_entry,
        &auth,
        &app_config.provider_http,
    )?;
    let provider_router = build_multi_provider_router(
        &provider_config,
        &app_config.provider_http,
        &auth,
        Arc::clone(&provider),
    )?;
    Ok(ResolvedServerProvider {
        provider,
        provider_router,
        default_model: format!("{}/{}", selection.provider_id, selection.model_id),
    })
}

struct MissingProvider;

#[async_trait::async_trait]
impl ModelProviderSDK for MissingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    fn name(&self) -> &str {
        "missing-provider"
    }
}

struct UnavailableProvider {
    message: String,
}

impl UnavailableProvider {
    fn new(message: String) -> Self {
        Self { message }
    }
}

#[async_trait::async_trait]
impl ModelProviderSDK for UnavailableProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("{}", self.message)
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("{}", self.message)
    }

    fn name(&self) -> &str {
        "unavailable-provider"
    }
}

pub(crate) fn build_provider_adapter(
    wire_api: ProviderWireApi,
    base_url: Option<String>,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
) -> Result<Arc<dyn ModelProviderSDK>> {
    let provider: Arc<dyn ModelProviderSDK> = match wire_api {
        ProviderWireApi::AnthropicMessages => {
            let api_key = api_key.context("anthropic provider requires an API key")?;
            let base_url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
            Arc::new(
                AnthropicProvider::new(base_url)
                    .with_http_options(http_options)?
                    .with_api_key(api_key),
            )
        }
        ProviderWireApi::OpenAIChatCompletions => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider = OpenAIProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Arc::new(provider)
        }
        ProviderWireApi::OpenAIResponses => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider =
                OpenAIResponsesProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Arc::new(provider)
        }
    };

    Ok(provider)
}

fn build_multi_provider_router(
    provider_config: &ProviderConfigFile,
    provider_http: &ProviderHttpConfig,
    auth: &UserAuthConfigFile,
    default_provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<dyn ProviderRouter>> {
    let mut router = MultiProviderRouter::new(default_provider);

    for (provider_id, provider) in &provider_config.providers {
        if provider.enabled == Some(false) {
            continue;
        }
        let mut wire_apis = Vec::new();
        if let Some(wire_api) = provider.wire_api {
            wire_apis.push(wire_api);
        }
        for model in provider.models.values() {
            if let Some(wire_api) = model.wire_api
                && !wire_apis.contains(&wire_api)
            {
                wire_apis.push(wire_api);
            }
        }
        if wire_apis.is_empty() {
            wire_apis.push(ProviderWireApi::OpenAIChatCompletions);
        }
        for wire_api in wire_apis {
            let provider_instance =
                match build_provider_route(wire_api, provider_id, provider, auth, provider_http) {
                    Ok(provider) => provider,
                    Err(error) => Arc::new(UnavailableProvider::new(error.to_string())),
                };
            router.insert_route(
                ProviderRoute::connection(provider_id.clone(), wire_api),
                provider_instance,
            );
        }
    }

    Ok(Arc::new(router))
}

fn build_provider_route(
    wire_api: ProviderWireApi,
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &UserAuthConfigFile,
    provider_http: &ProviderHttpConfig,
) -> Result<Arc<dyn ModelProviderSDK>> {
    build_provider_adapter(
        wire_api,
        provider.base_url.clone(),
        resolve_provider_api_key(provider_id, provider, auth)?,
        ProviderHttpOptions::from_raw_with_no_proxy(
            provider_http.proxy_url.clone(),
            provider_http.no_proxy.clone(),
            provider
                .headers
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )?,
    )
}

fn resolve_server_model(
    provider_config: &ProviderConfigFile,
    default_model: Option<&str>,
) -> Result<devo_core::ProviderModelSelection> {
    if provider_config.model.is_some() {
        return provider_config.resolve_model(None).map_err(Into::into);
    }
    if let Some(default_model) = default_model
        && let Ok(selection) = provider_config.resolve_model(Some(default_model))
    {
        return Ok(selection);
    }
    provider_config.resolve_model(None).map_err(Into::into)
}

fn resolve_provider_api_key(
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &devo_core::UserAuthConfigFile,
) -> Result<Option<String>> {
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

pub(crate) fn normalize_openai_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let Some(scheme_sep) = trimmed.find("://") else {
        return trimmed.to_string();
    };
    let has_explicit_path = trimmed[scheme_sep + 3..].contains('/');
    if has_explicit_path {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_core::AppConfig;
    use devo_core::AuthCredentialConfig;
    use devo_core::AuthCredentialKind;
    use devo_core::ProviderConfigEntry;
    use devo_core::ProviderConfigFile;
    use devo_core::ProviderModelConfig;
    use devo_core::UserAuthConfigFile;
    use pretty_assertions::assert_eq;

    use super::load_server_provider;
    use super::normalize_openai_base_url;
    use super::resolve_provider_api_key;
    use devo_protocol::ProviderWireApi;

    #[test]
    fn preserves_explicit_openai_compatible_paths() {
        assert_eq!(
            normalize_openai_base_url("https://open.bigmodel.cn/api/paas/v4/"),
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }

    #[test]
    fn appends_v1_for_bare_openai_hosts() {
        assert_eq!(
            normalize_openai_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
    }

    #[tokio::test]
    async fn empty_provider_config_loads_missing_provider_for_onboarding() {
        let config = devo_core::AppConfig::default();
        let dir = tempfile::tempdir().expect("temp dir");

        let actual = load_server_provider(&config, Some("onboard-model"), dir.path())
            .expect("load missing provider");

        assert_eq!(actual.default_model, "onboard-model");
        assert_eq!(actual.provider.name(), "missing-provider");
        let error = actual
            .provider
            .completion(devo_protocol::ModelRequest {
                model_slug: devo_protocol::ModelProfileKey::Generic,
                model: "onboard-model".to_string(),
                system: None,
                messages: Vec::new(),
                max_tokens: 1,
                tools: None,
                hosted_tools: Vec::new(),
                sampling: devo_protocol::SamplingControls::default(),
                request_thinking: None,
                reasoning_effort: None,
                extra_body: None,
            })
            .await
            .expect_err("missing provider should reject model requests");

        assert_eq!(
            error.to_string(),
            "No provider configured. Run `devo onboard` to complete setup."
        );
    }

    /// Trace: L2-DES-APP-005, L2-DES-MODEL-001
    /// Verifies: server provider construction validates provider custom header configuration.
    #[test]
    fn load_server_provider_rejects_invalid_custom_headers() {
        let config = AppConfig {
            provider_catalog: ProviderConfigFile {
                model: Some("openai/test-model".to_string()),
                providers: BTreeMap::from([(
                    "openai".to_string(),
                    ProviderConfigEntry {
                        name: Some("OpenAI".to_string()),
                        headers: Some(BTreeMap::from([(
                            "bad header".to_string(),
                            "value".to_string(),
                        )])),
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        models: BTreeMap::from([(
                            "test-model".to_string(),
                            ProviderModelConfig::default(),
                        )]),
                        ..ProviderConfigEntry::default()
                    },
                )]),
                ..ProviderConfigFile::default()
            },
            ..AppConfig::default()
        };
        let dir = tempfile::tempdir().expect("temp dir");

        let error = match load_server_provider(&config, None, dir.path()) {
            Ok(_) => panic!("invalid headers should reject provider construction"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "invalid provider custom header name `bad header`"
        );
    }

    #[test]
    fn resolves_provider_credential_id_through_user_auth() {
        let provider = ProviderConfigEntry {
            credential: Some("openrouter_api_key".to_string()),
            ..ProviderConfigEntry::default()
        };
        let auth = UserAuthConfigFile {
            credentials: BTreeMap::from([(
                "openrouter_api_key".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::ApiKey,
                    value: "sk-or-secret".to_string(),
                },
            )]),
            ..UserAuthConfigFile::default()
        };

        assert_eq!(
            resolve_provider_api_key("openrouter", &provider, &auth)
                .expect("resolve provider credential"),
            Some("sk-or-secret".to_string())
        );
    }
}
