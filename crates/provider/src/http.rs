use anyhow::Context;
use anyhow::Result;
use devo_network_proxy::NetworkProxyConfig;
use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use tracing::warn;

use crate::error::context_limit_error;
use crate::timeout::connect_timeout;

#[derive(Clone, Copy)]
enum HttpClientKind {
    Request,
    Streaming,
}

#[derive(Default)]
struct HttpClientCache {
    request_clients: Vec<(NetworkProxyConfig, Client)>,
    streaming_clients: Vec<(NetworkProxyConfig, Client)>,
}

impl HttpClientCache {
    fn get_or_build(
        &mut self,
        kind: HttpClientKind,
        network_proxy: &NetworkProxyConfig,
        build: impl FnOnce() -> Result<Client>,
    ) -> Result<Client> {
        let clients = match kind {
            HttpClientKind::Request => &mut self.request_clients,
            HttpClientKind::Streaming => &mut self.streaming_clients,
        };
        if let Some((_, client)) = clients
            .iter()
            .find(|(cached_proxy, _)| cached_proxy == network_proxy)
        {
            return Ok(client.clone());
        }

        let client = build()?;
        clients.push((network_proxy.clone(), client.clone()));
        Ok(client)
    }
}

fn cached_http_client(
    kind: HttpClientKind,
    network_proxy: &NetworkProxyConfig,
    build: impl FnOnce() -> Result<Client>,
) -> Result<Client> {
    // An empty config resolves proxy environment variables during the first
    // build. The server fixes its environment before provider initialization,
    // so equivalent empty configs can safely share that client for its lifetime.
    static CACHE: OnceLock<Mutex<HttpClientCache>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(HttpClientCache::default()))
        .lock()
        .expect("provider HTTP client cache mutex should not be poisoned")
        .get_or_build(kind, network_proxy, build)
}

/// HTTP options shared by model-provider adapters.
#[derive(Clone, Debug, Default)]
pub struct ProviderHttpOptions {
    network_proxy: NetworkProxyConfig,
    custom_headers: HeaderMap,
}

impl ProviderHttpOptions {
    /// Builds provider HTTP options from raw config fields.
    pub fn from_raw(proxy_url: Option<String>, headers: Option<String>) -> Result<Self> {
        Self::from_raw_with_no_proxy(proxy_url, None, headers)
    }

    /// Builds provider HTTP options from raw proxy, bypass, and header fields.
    pub fn from_raw_with_no_proxy(
        proxy_url: Option<String>,
        no_proxy: Option<String>,
        headers: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            network_proxy: NetworkProxyConfig {
                proxy_url: proxy_url.and_then(non_empty_owned_string),
                no_proxy: no_proxy.and_then(non_empty_owned_string),
            },
            custom_headers: parse_custom_headers(headers)?,
        })
    }

    /// Returns the configured proxy URL, when present.
    pub fn proxy_url(&self) -> Option<&str> {
        self.network_proxy.proxy_url.as_deref()
    }

    /// HTTP client for non-streaming requests with a connection timeout.
    pub(crate) fn build_request_client(&self) -> Result<Client> {
        cached_http_client(HttpClientKind::Request, &self.network_proxy, || {
            let builder = Client::builder().connect_timeout(connect_timeout());
            devo_network_proxy::apply_proxy_config(builder, &self.network_proxy)?
                .build()
                .context("failed to build provider HTTP client")
        })
    }

    /// HTTP client for SSE streaming with a connection timeout.
    pub(crate) fn build_streaming_client(&self) -> Result<Client> {
        cached_http_client(HttpClientKind::Streaming, &self.network_proxy, || {
            let builder = Client::builder().connect_timeout(connect_timeout());
            devo_network_proxy::apply_proxy_config(builder, &self.network_proxy)?
                .build()
                .context("failed to build provider streaming HTTP client")
        })
    }

    pub(crate) fn apply_custom_headers(&self, builder: RequestBuilder) -> RequestBuilder {
        if self.custom_headers.is_empty() {
            builder
        } else {
            builder.headers(self.custom_headers.clone())
        }
    }

    /// Applies model/variant headers after provider defaults.
    pub(crate) fn apply_request_headers(
        &self,
        builder: RequestBuilder,
        headers: &BTreeMap<String, String>,
    ) -> RequestBuilder {
        let mut request_headers = HeaderMap::new();
        for (name, value) in headers {
            let Ok(name) = HeaderName::try_from(name) else {
                warn!(header = %name, "ignoring invalid model request header name");
                continue;
            };
            let Ok(value) = HeaderValue::try_from(value) else {
                warn!(header = %name, "ignoring invalid model request header value");
                continue;
            };
            request_headers.insert(name, value);
        }
        if request_headers.is_empty() {
            builder
        } else {
            builder.headers(request_headers)
        }
    }
}

pub(crate) async fn invalid_status_error(
    provider: &'static str,
    model: &str,
    operation: &str,
    status: StatusCode,
    response: Response,
    request_body: &Value,
) -> anyhow::Error {
    let response_body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
    warn!(
        provider,
        model,
        operation,
        status = %status,
        http_body = %request_body,
        response_body = %response_body,
        "provider request failed"
    );
    let response_value = serde_json::from_str::<Value>(&response_body).ok();
    let message = response_value
        .as_ref()
        .and_then(|value| value.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or(&response_body)
        .to_string();
    let error_kind = response_value
        .as_ref()
        .and_then(|value| value.pointer("/error/type"))
        .and_then(Value::as_str);
    let error_code = response_value
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    if let Some(error) = context_limit_error(message, error_kind, error_code) {
        return anyhow::Error::new(error);
    }
    anyhow::anyhow!(
        "{provider} {operation} error for model {model}: Invalid status code: {status}; response body: {response_body}"
    )
}

fn parse_custom_headers(headers: Option<String>) -> Result<HeaderMap> {
    let Some(headers) = headers else {
        return Ok(HeaderMap::new());
    };
    let headers = headers.trim();
    if headers.is_empty() {
        return Ok(HeaderMap::new());
    }
    let value: Value =
        serde_json::from_str(headers).context("provider custom headers must be valid JSON")?;
    let object = value
        .as_object()
        .context("provider custom headers must be a JSON object string")?;
    let mut parsed = HeaderMap::with_capacity(object.len());
    for (name, value) in object {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid provider custom header name `{name}`"))?;
        let value = value
            .as_str()
            .with_context(|| format!("provider custom header `{name}` value must be a string"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid provider custom header `{name}` value"))?;
        parsed.insert(header_name, header_value);
    }
    Ok(parsed)
}

fn non_empty_owned_string(mut value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        // Trim in place: these values originate as owned environment/config
        // strings, so avoid allocating another `String` just to drop whitespace.
        let end = value.trim_end().len();
        value.truncate(end);
        let start = value.len() - value.trim_start().len();
        if start > 0 {
            value.drain(..start);
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn http_client_cache_reuses_equivalent_clients() {
        let mut cache = HttpClientCache::default();
        let config = NetworkProxyConfig::default();
        let build_count = AtomicUsize::new(0);
        let client = Client::new();

        for _ in 0..2 {
            cache
                .get_or_build(HttpClientKind::Request, &config, || {
                    build_count.fetch_add(1, Ordering::SeqCst);
                    Ok(client.clone())
                })
                .expect("cached HTTP client");
        }

        assert_eq!(build_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn http_client_cache_separates_kinds_and_proxy_configs() {
        let mut cache = HttpClientCache::default();
        let default_proxy = NetworkProxyConfig::default();
        let explicit_proxy = NetworkProxyConfig {
            proxy_url: Some("http://proxy.example:8080".to_string()),
            no_proxy: Some("localhost".to_string()),
        };
        let build_count = AtomicUsize::new(0);
        let client = Client::new();

        for (kind, config) in [
            (HttpClientKind::Request, &default_proxy),
            (HttpClientKind::Streaming, &default_proxy),
            (HttpClientKind::Request, &explicit_proxy),
        ] {
            cache
                .get_or_build(kind, config, || {
                    build_count.fetch_add(1, Ordering::SeqCst);
                    Ok(client.clone())
                })
                .expect("cached HTTP client");
        }

        assert_eq!(build_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn http_client_cache_builds_once_for_concurrent_callers() {
        let cache = Arc::new(Mutex::new(HttpClientCache::default()));
        let barrier = Arc::new(Barrier::new(4));
        let build_count = Arc::new(AtomicUsize::new(0));
        let client = Client::new();
        let mut threads = Vec::new();

        for _ in 0..4 {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            let build_count = Arc::clone(&build_count);
            let client = client.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                cache
                    .lock()
                    .expect("cache mutex")
                    .get_or_build(
                        HttpClientKind::Request,
                        &NetworkProxyConfig::default(),
                        || {
                            build_count.fetch_add(1, Ordering::SeqCst);
                            Ok(client)
                        },
                    )
                    .expect("cached HTTP client");
            }));
        }

        for thread in threads {
            thread.join().expect("cache caller joins");
        }

        assert_eq!(build_count.load(Ordering::SeqCst), 1);
    }

    /// Trace: L2-DES-APP-005
    /// Verifies: provider custom headers parse from a JSON object string.
    #[test]
    fn custom_headers_parse_json_object_string() {
        let options = ProviderHttpOptions::from_raw(
            None,
            Some(r#"{"X-Devo":"yes","Authorization":"custom"}"#.to_string()),
        )
        .expect("parse options");
        let request = options
            .apply_custom_headers(Client::new().get("http://example.com"))
            .build()
            .expect("build request");

        assert_eq!(
            request
                .headers()
                .get("x-devo")
                .expect("x-devo header")
                .to_str()
                .expect("header value"),
            "yes"
        );
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .expect("authorization header")
                .to_str()
                .expect("header value"),
            "custom"
        );
    }

    /// Trace: L2-DES-APP-005
    /// Verifies: invalid provider custom header value errors do not print the value.
    #[test]
    fn custom_header_value_errors_do_not_print_value() {
        let error = ProviderHttpOptions::from_raw(
            None,
            Some("{\"X-Secret\":\"secret\\nvalue\"}".to_string()),
        )
        .expect_err("invalid header value");
        let message = error.to_string();

        assert_eq!(message, "invalid provider custom header `X-Secret` value");
        assert!(!message.contains("secret"));
    }
}
