//! Provider model-directory discovery.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use devo_core::{ModelCatalog, PresetModelCatalog, read_user_auth_config};
use devo_protocol::{InputModality, ProviderModelInfo, ReasoningCapability};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::{ProtocolErrorCode, SuccessResponse};

use super::ServerRuntime;

const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
type DiscoveryCache = Mutex<BTreeMap<String, (Instant, BTreeMap<String, ProviderModelInfo>)>>;
static DISCOVERY_CACHE: OnceLock<DiscoveryCache> = OnceLock::new();

impl ServerRuntime {
    /// Discovers models from a connected provider standard /models endpoint
    /// and persists the result into that Connection directory.
    pub(super) async fn handle_native_provider_discover(
        &self,
        request_id: Value,
        params: Value,
    ) -> Value {
        let params: devo_protocol::native::rpc_admin::ProviderDiscoverParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid provider/discover params: {error}"),
                    );
                }
            };
        let Some(provider_id) = non_empty(&params.provider_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider_id cannot be empty",
            );
        };

        let (mut provider, connected, user_config_dir) = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            let connected = store
                .provider_connection_ids()
                .map(|ids| ids.contains(&provider_id))
                .unwrap_or(false);
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
            let provider = configured_providers
                .into_iter()
                .find(|provider| provider.id == provider_id)
                .or_else(|| {
                    self.deps
                        .model_catalog
                        .list_providers()
                        .into_iter()
                        .find(|provider| provider.id == provider_id)
                });
            let Some(provider) = provider else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("provider {provider_id} does not exist"),
                );
            };
            let mut provider = canonical_provider(provider, catalog);
            if let Some(config) = store
                .effective_config()
                .provider_catalog
                .providers
                .get(&provider_id)
            {
                provider.options = config.options.clone();
                provider.request = config.request.clone();
                if let Some(headers) = &config.headers {
                    provider.headers = headers.clone();
                }
            }
            (provider, connected, store.user_config_dir().to_path_buf())
        };
        if !connected {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("provider {provider_id} is a template; connect it before discovery"),
            );
        }

        let api_key = match provider_api_key(&provider, &user_config_dir) {
            Ok(api_key) => api_key,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    error.to_string(),
                );
            }
        };
        let discovered =
            match discover_models(&provider, api_key.as_deref(), params.force_refresh).await {
                Ok(models) => models,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        error.to_string(),
                    );
                }
            };
        // Discovery is the live directory: keep prior overlays only for models
        // that still exist remotely. Stale template / renamed ids must not
        // remain selectable after a refresh (e.g. Ollama placeholder catalog
        // entries that 404 on chat completions).
        let mut next_models = BTreeMap::new();
        for (model_id, discovered_model) in discovered {
            let model = match provider.models.remove(&model_id) {
                Some(existing) => merge_discovered_model(existing, discovered_model),
                None => discovered_model,
            };
            next_models.insert(model_id, model);
        }
        provider.models = next_models;

        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(devo_core::PROVIDER_CONFIG_FILE_NAME)
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
        let provider = match store.upsert_provider_connection(provider, None, None, None) {
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
            result: devo_protocol::native::rpc_admin::ProviderDiscoverResult {
                provider_id,
                models: provider.models,
            },
        })
        .expect("serialize provider/discover response")
    }
}

fn canonical_provider(
    mut provider: devo_protocol::ProviderInfo,
    catalog: &dyn ModelCatalog,
) -> devo_protocol::ProviderInfo {
    if provider.models.is_empty() {
        provider.models = catalog.list_provider_models(&provider.id);
    }
    provider
}

fn provider_api_key(
    provider: &devo_protocol::ProviderInfo,
    config_dir: &std::path::Path,
) -> anyhow::Result<Option<String>> {
    let auth = read_user_auth_config(&config_dir.join(devo_core::AUTH_CONFIG_FILE_NAME))?;
    let Some(credential_id) = provider.credential.as_deref() else {
        return Ok(None);
    };
    Ok(Some(
        auth.credentials
            .get(credential_id)
            .with_context(|| format!("missing credential {credential_id} in auth.json"))?
            .value
            .clone(),
    ))
}

async fn discover_models(
    provider: &devo_protocol::ProviderInfo,
    api_key: Option<&str>,
    force_refresh: bool,
) -> anyhow::Result<BTreeMap<String, ProviderModelInfo>> {
    let cache_key = format!(
        "{}|{}|{}",
        provider.id,
        provider.base_url.as_deref().unwrap_or_default(),
        provider.credential.as_deref().unwrap_or_default(),
    );
    if !force_refresh
        && let Some((cached_at, models)) = DISCOVERY_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .expect("provider discovery cache mutex should not be poisoned")
            .get(&cache_key)
        && cached_at.elapsed() < DISCOVERY_CACHE_TTL
    {
        return Ok(models.clone());
    }

    let models = fetch_discovered_models(provider, api_key).await?;
    DISCOVERY_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .expect("provider discovery cache mutex should not be poisoned")
        .insert(cache_key, (Instant::now(), models.clone()));
    Ok(models)
}

async fn fetch_discovered_models(
    provider: &devo_protocol::ProviderInfo,
    api_key: Option<&str>,
) -> anyhow::Result<BTreeMap<String, ProviderModelInfo>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("failed to build model discovery client")?;
    let mut headers = HeaderMap::new();
    for (name, value) in &provider.headers {
        headers.insert(
            HeaderName::try_from(name).with_context(|| format!("invalid header name {name}"))?,
            HeaderValue::try_from(value)
                .with_context(|| format!("invalid value for header {name}"))?,
        );
    }
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        if provider
            .wire_apis
            .first()
            .is_some_and(|wire_api| *wire_api == devo_core::ProviderWireApi::AnthropicMessages)
        {
            headers.insert(
                HeaderName::from_static("x-api-key"),
                HeaderValue::try_from(api_key).context("invalid provider API key")?,
            );
        } else {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::try_from(format!("Bearer {api_key}"))
                    .context("invalid provider API key")?,
            );
        }
    }

    let mut last_status = None;
    for url in discovery_urls(provider) {
        let response = client.get(&url).headers(headers.clone()).send().await?;
        last_status = Some(response.status());
        if response.status().is_success() {
            return parse_models(response.json().await?);
        }
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("model discovery failed with {status}: {body}");
        }
    }
    anyhow::bail!("model discovery endpoint not found (last status: {last_status:?})")
}

fn discovery_urls(provider: &devo_protocol::ProviderInfo) -> Vec<String> {
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
        .trim_end_matches('/')
        .to_string();
    let is_ollama = provider.id.eq_ignore_ascii_case("ollama");
    let mut urls = Vec::new();
    // Prefer Ollama's native tag list first — older OpenAI-compat /v1/models
    // responses could expose ids that chat completions reject with 404.
    if is_ollama && let Some(root) = base.strip_suffix("/v1") {
        urls.push(format!("{root}/api/tags"));
    }
    urls.push(format!("{base}/models"));
    if !base.ends_with("/v1") {
        urls.push(format!("{base}/v1/models"));
    }
    if base.ends_with("/anthropic") {
        urls.push(format!("{}/models", base.trim_end_matches("/anthropic")));
    }
    if !is_ollama && let Some(root) = base.strip_suffix("/v1") {
        urls.push(format!("{root}/api/tags"));
    }
    urls.dedup();
    urls
}

fn parse_models(value: Value) -> anyhow::Result<BTreeMap<String, ProviderModelInfo>> {
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .context("provider model discovery response has no data array")?;
    let mut models = BTreeMap::new();
    for entry in entries {
        // Prefer Ollama's canonical `model` field over display-oriented `name`
        // when both are present (see ollama /api/tags).
        let Some(id) = entry
            .get("id")
            .or_else(|| entry.get("model"))
            .or_else(|| entry.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let name = entry
            .get("display_name")
            .or_else(|| entry.get("displayName"))
            .or_else(|| entry.get("name"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let release_date = entry
            .get("created_at")
            .or_else(|| entry.get("createdAt"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let family = entry
            .get("family")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let context_window = number_field(
            entry,
            &[
                "context_window",
                "contextWindow",
                "context_length",
                "contextLength",
            ],
        )
        .or_else(|| nested_number_field(entry, "limit", &["context", "context_window"]));
        let max_tokens = number_field(
            entry,
            &[
                "max_tokens",
                "maxTokens",
                "max_output_tokens",
                "maxOutputTokens",
            ],
        )
        .or_else(|| nested_number_field(entry, "limit", &["output", "max_tokens"]));
        let cost = entry.get("cost").or_else(|| entry.get("pricing")).cloned();
        let status = entry
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let capabilities = entry.get("capabilities").cloned();
        let reasoning_capability = entry
            .get("reasoning")
            .and_then(Value::as_bool)
            .filter(|reasoning| *reasoning)
            .map(|_| ReasoningCapability::Toggle);
        let input_modalities = entry
            .get("input_modalities")
            .or_else(|| entry.get("inputModalities"))
            .or_else(|| entry.get("modalities"))
            .and_then(Value::as_array)
            .map(|modalities| {
                modalities
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|modality| match modality.to_ascii_lowercase().as_str() {
                        "text" => Some(InputModality::Text),
                        "image" => Some(InputModality::Image),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|modalities| !modalities.is_empty());
        models.insert(
            id.to_string(),
            ProviderModelInfo {
                name,
                family,
                release_date,
                status,
                capabilities,
                context_window,
                max_tokens,
                cost,
                reasoning_capability,
                input_modalities,
                metadata: Some(entry.clone()),
                enabled: Some(true),
                ..ProviderModelInfo::default()
            },
        );
    }
    Ok(models)
}

fn merge_discovered_model(
    existing: ProviderModelInfo,
    mut discovered: ProviderModelInfo,
) -> ProviderModelInfo {
    macro_rules! fill_if_missing {
        ($field:ident) => {
            if discovered.$field.is_none() {
                discovered.$field = existing.$field;
            }
        };
    }

    fill_if_missing!(name);
    fill_if_missing!(family);
    fill_if_missing!(release_date);
    fill_if_missing!(status);
    fill_if_missing!(capabilities);
    fill_if_missing!(wire_api);
    fill_if_missing!(context_window);
    fill_if_missing!(effective_context_window_percent);
    fill_if_missing!(max_tokens);
    fill_if_missing!(temperature);
    fill_if_missing!(top_p);
    fill_if_missing!(top_k);
    fill_if_missing!(reasoning_capability);
    fill_if_missing!(reasoning_implementation);
    fill_if_missing!(default_reasoning_effort);
    fill_if_missing!(default_reasoning_selection);
    fill_if_missing!(base_instructions);
    fill_if_missing!(input_modalities);
    fill_if_missing!(channel);
    fill_if_missing!(supports_image_detail_original);
    fill_if_missing!(truncation_policy);
    fill_if_missing!(web_search);
    fill_if_missing!(web_fetch);
    fill_if_missing!(cost);
    fill_if_missing!(metadata);
    fill_if_missing!(request);
    fill_if_missing!(options);
    if discovered.headers.is_empty() {
        discovered.headers = existing.headers;
    }
    for (variant_id, variant) in existing.variants {
        discovered.variants.entry(variant_id).or_insert(variant);
    }
    fill_if_missing!(default_variant);
    if existing.enabled == Some(false) {
        discovered.enabled = existing.enabled;
    } else {
        fill_if_missing!(enabled);
    }
    fill_if_missing!(priority);
    discovered
}

fn number_field(value: &Value, names: &[&str]) -> Option<u32> {
    names
        .iter()
        .find_map(|name| value.get(name).and_then(Value::as_u64))
        .and_then(|value| u32::try_from(value).ok())
}

fn nested_number_field(value: &Value, object: &str, names: &[&str]) -> Option<u32> {
    value
        .get(object)
        .and_then(|value| number_field(value, names))
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use devo_protocol::{InputModality, ProviderModelInfo, ReasoningCapability};
    use pretty_assertions::assert_eq;

    use super::{discovery_urls, merge_discovered_model, parse_models};

    #[test]
    fn parses_openai_and_anthropic_style_directory_entries() {
        let models = parse_models(serde_json::json!({
            "data": [
                {"id": "model-a", "owned_by": "team-a"},
                {"id": "model-b", "display_name": "Model B", "created_at": "2026-09-01"}
            ]
        }))
        .expect("parse model directory");

        assert_eq!(models["model-a"].name, None);
        assert_eq!(models["model-b"].name.as_deref(), Some("Model B"));
        assert_eq!(
            models["model-b"].release_date.as_deref(),
            Some("2026-09-01")
        );
        assert_eq!(
            models["model-b"].metadata,
            Some(serde_json::json!({
                "id": "model-b",
                "display_name": "Model B",
                "created_at": "2026-09-01"
            }))
        );
    }

    #[test]
    fn discovery_urls_cover_v1_and_deepseek_anthropic_bases() {
        let provider = devo_protocol::ProviderInfo {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: Some("https://api.deepseek.com/anthropic".to_string()),
            wire_apis: vec![devo_core::ProviderWireApi::AnthropicMessages],
            ..devo_protocol::ProviderInfo::default()
        };
        let urls = discovery_urls(&provider);

        assert!(urls.contains(&"https://api.deepseek.com/anthropic/models".to_string()));
        assert!(urls.contains(&"https://api.deepseek.com/anthropic/v1/models".to_string()));
        assert!(urls.contains(&"https://api.deepseek.com/models".to_string()));
    }

    #[test]
    fn discovery_urls_include_ollama_native_tags() {
        let provider = devo_protocol::ProviderInfo {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
            wire_apis: vec![devo_core::ProviderWireApi::OpenAIChatCompletions],
            ..devo_protocol::ProviderInfo::default()
        };
        let urls = discovery_urls(&provider);
        assert_eq!(
            urls,
            vec![
                "http://localhost:11434/api/tags".to_string(),
                "http://localhost:11434/v1/models".to_string(),
            ]
        );
    }

    #[test]
    fn parse_models_prefers_ollama_model_field_over_name() {
        let models = parse_models(serde_json::json!({
            "models": [{
                "name": "display-only",
                "model": "qwen3:8b",
                "size": 1
            }]
        }))
        .expect("parse ollama tags");
        assert!(models.contains_key("qwen3:8b"));
        assert!(!models.contains_key("display-only"));
    }

    #[test]
    fn parses_common_model_metadata_without_losing_the_raw_entry() {
        let models = parse_models(serde_json::json!({
            "models": [{
                "id": "reasoning-model",
                "family": "reasoning",
                "contextWindow": 128000,
                "limit": {"output": 8192},
                "cost": {"input": 1.0, "output": 4.0},
                "reasoning": true,
                "inputModalities": ["text", "image", "audio"],
                "status": "active"
            }]
        }))
        .expect("parse model directory");

        let model = &models["reasoning-model"];
        assert_eq!(model.family.as_deref(), Some("reasoning"));
        assert_eq!(model.context_window, Some(128_000));
        assert_eq!(model.max_tokens, Some(8_192));
        assert_eq!(
            model.cost,
            Some(serde_json::json!({"input": 1.0, "output": 4.0}))
        );
        assert_eq!(
            model.reasoning_capability,
            Some(ReasoningCapability::Toggle)
        );
        assert_eq!(
            model.input_modalities,
            Some(vec![InputModality::Text, InputModality::Image])
        );
        assert_eq!(
            model.metadata,
            Some(serde_json::json!({
                "id": "reasoning-model",
                "family": "reasoning",
                "contextWindow": 128000,
                "limit": {"output": 8192},
                "cost": {"input": 1.0, "output": 4.0},
                "reasoning": true,
                "inputModalities": ["text", "image", "audio"],
                "status": "active"
            }))
        );
    }

    #[test]
    fn discovery_preserves_connection_overrides() {
        let merged = merge_discovered_model(
            ProviderModelInfo {
                options: Some(serde_json::json!({"timeout": 30})),
                variants: [(
                    "fast".to_string(),
                    devo_protocol::ProviderModelVariant {
                        label: Some("Fast".to_string()),
                        ..devo_protocol::ProviderModelVariant::default()
                    },
                )]
                .into_iter()
                .collect(),
                enabled: Some(false),
                ..ProviderModelInfo::default()
            },
            ProviderModelInfo {
                name: Some("Discovered model".to_string()),
                context_window: Some(128_000),
                enabled: Some(true),
                metadata: Some(serde_json::json!({"id": "model"})),
                ..ProviderModelInfo::default()
            },
        );

        assert_eq!(merged.name.as_deref(), Some("Discovered model"));
        assert_eq!(merged.context_window, Some(128_000));
        assert_eq!(merged.options, Some(serde_json::json!({"timeout": 30})));
        assert_eq!(merged.enabled, Some(false));
        assert_eq!(merged.variants["fast"].label.as_deref(), Some("Fast"));
    }
}
