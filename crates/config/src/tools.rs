//! Web-tool configuration and resolution.
//!
//! The serialized config keeps user intent separate from runtime credentials:
//! provider-hosted modes stay as capability flags, while local search resolves a
//! named provider plus user-scoped auth entry into the per-turn settings used by
//! tool/runtime code.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::AuthCredentialKind;
use crate::ProviderConfigError;
use crate::UserAuthConfigFile;

/// Tool-specific configuration stored under `[tools]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default, skip_serializing_if = "WebSearchConfig::is_default")]
    pub web_search: WebSearchConfig,
    #[serde(default, skip_serializing_if = "WebFetchConfig::is_default")]
    pub web_fetch: WebFetchConfig,
}

impl ToolsConfig {
    pub fn is_empty(&self) -> bool {
        self.web_search.is_default() && self.web_fetch.is_default()
    }
}

/// Selects how Devo exposes web search for a provider/model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchMode {
    Disabled,
    #[default]
    Provider,
    Local,
}

/// Configures the effective `web_search` capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebSearchConfig {
    #[serde(default, skip_serializing_if = "is_default_web_search_mode")]
    pub mode: WebSearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_provider: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub local_providers: BTreeMap<String, LocalWebSearchProviderConfig>,
}

impl WebSearchConfig {
    pub fn is_default(&self) -> bool {
        self.mode == WebSearchMode::Provider
            && self.local_provider.is_none()
            && self.local_providers.is_empty()
    }
}

/// Selects how Devo exposes web fetch for a provider/model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchMode {
    Disabled,
    Provider,
    #[default]
    Local,
}

/// Configures the effective `web_fetch` capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebFetchConfig {
    #[serde(default, skip_serializing_if = "is_default_web_fetch_mode")]
    pub mode: WebFetchMode,
}

impl WebFetchConfig {
    pub fn is_default(&self) -> bool {
        self.mode == WebFetchMode::Local
    }
}

/// Local third-party search provider configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalWebSearchProviderConfig {
    pub kind: LocalWebSearchProviderKind,
    pub credential: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

/// Supported local web search services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalWebSearchProviderKind {
    Exa,
    Tavily,
}

/// Fully resolved web search behavior for one model invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ResolvedWebSearchConfig {
    #[default]
    Disabled,
    Provider,
    Local(ResolvedLocalWebSearchConfig),
}

impl ResolvedWebSearchConfig {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    pub fn is_provider(&self) -> bool {
        matches!(self, Self::Provider)
    }
}

/// Fully resolved web fetch behavior for one model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ResolvedWebFetchConfig {
    Disabled,
    Provider,
    #[default]
    Local,
}

impl ResolvedWebFetchConfig {
    pub fn is_local(self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn is_provider(self) -> bool {
        matches!(self, Self::Provider)
    }
}

/// Local search service plus user-scoped API key for one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLocalWebSearchConfig {
    pub provider_id: String,
    pub kind: LocalWebSearchProviderKind,
    pub api_key: String,
    pub base_url: Option<String>,
    pub max_results: Option<u32>,
}

pub fn resolve_web_search_config(
    global: &WebSearchConfig,
    provider_override: Option<&WebSearchConfig>,
    model_override: Option<&WebSearchConfig>,
    auth: &UserAuthConfigFile,
) -> Result<ResolvedWebSearchConfig, ProviderConfigError> {
    let effective = model_override.or(provider_override).unwrap_or(global);
    match effective.mode {
        WebSearchMode::Disabled => Ok(ResolvedWebSearchConfig::Disabled),
        WebSearchMode::Provider => Ok(ResolvedWebSearchConfig::Provider),
        WebSearchMode::Local => {
            resolve_local_web_search(global, effective, auth).map(ResolvedWebSearchConfig::Local)
        }
    }
}

pub fn resolve_web_fetch_config(
    global: &WebFetchConfig,
    provider_override: Option<&WebFetchConfig>,
    model_override: Option<&WebFetchConfig>,
) -> ResolvedWebFetchConfig {
    let effective = model_override.or(provider_override).unwrap_or(global);
    match effective.mode {
        WebFetchMode::Disabled => ResolvedWebFetchConfig::Disabled,
        WebFetchMode::Provider => ResolvedWebFetchConfig::Provider,
        WebFetchMode::Local => ResolvedWebFetchConfig::Local,
    }
}

fn resolve_local_web_search(
    global: &WebSearchConfig,
    effective: &WebSearchConfig,
    auth: &UserAuthConfigFile,
) -> Result<ResolvedLocalWebSearchConfig, ProviderConfigError> {
    let provider_id = match effective.local_provider.as_deref() {
        Some(provider_id) if !provider_id.trim().is_empty() => provider_id,
        _ if global.local_providers.len() == 1 => global
            .local_providers
            .keys()
            .next()
            .expect("single local provider key should exist")
            .as_str(),
        _ => {
            return Err(ProviderConfigError::Validation {
                message: "tools.web_search mode `local` requires local_provider when zero or multiple local providers are configured".to_string(),
            });
        }
    };
    let provider =
        global
            .local_providers
            .get(provider_id)
            .ok_or_else(|| ProviderConfigError::Validation {
                message: format!(
                    "tools.web_search references missing local provider `{provider_id}`"
                ),
            })?;
    if provider.credential.trim().is_empty() {
        return Err(ProviderConfigError::Validation {
            message: format!("web search local provider `{provider_id}` has an empty credential"),
        });
    }
    let credential =
        auth.credentials
            .get(&provider.credential)
            .ok_or_else(|| ProviderConfigError::Validation {
                message: format!(
                    "web search local provider `{provider_id}` references missing credential `{}` in user auth.json",
                    provider.credential
                ),
            })?;
    match credential.kind {
        AuthCredentialKind::ApiKey => Ok(ResolvedLocalWebSearchConfig {
            provider_id: provider_id.to_string(),
            kind: provider.kind,
            api_key: credential.value.clone(),
            base_url: provider.base_url.clone(),
            max_results: provider.max_results,
        }),
    }
}

fn is_default_web_search_mode(mode: &WebSearchMode) -> bool {
    *mode == WebSearchMode::Provider
}

fn is_default_web_fetch_mode(mode: &WebFetchMode) -> bool {
    *mode == WebFetchMode::Local
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::AuthCredentialConfig;

    fn auth() -> UserAuthConfigFile {
        UserAuthConfigFile {
            credentials: BTreeMap::from([(
                "exa_api_key".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::ApiKey,
                    value: "secret".to_string(),
                },
            )]),
            ..UserAuthConfigFile::default()
        }
    }

    fn local_global_config() -> WebSearchConfig {
        WebSearchConfig {
            mode: WebSearchMode::Local,
            local_provider: Some("exa".to_string()),
            local_providers: BTreeMap::from([(
                "exa".to_string(),
                LocalWebSearchProviderConfig {
                    kind: LocalWebSearchProviderKind::Exa,
                    credential: "exa_api_key".to_string(),
                    base_url: None,
                    max_results: Some(5),
                },
            )]),
        }
    }

    #[test]
    fn default_mode_resolves_provider() {
        let resolved = resolve_web_search_config(
            &WebSearchConfig::default(),
            None,
            None,
            &UserAuthConfigFile::default(),
        )
        .expect("resolve web search");

        assert_eq!(resolved, ResolvedWebSearchConfig::Provider);
    }

    #[test]
    fn default_web_fetch_mode_resolves_local() {
        // Trace: L2-DES-APP-005
        // Verifies: missing web_fetch config preserves the local webfetch default.
        assert_eq!(WebFetchConfig::default().mode, WebFetchMode::Local);
        assert_eq!(
            resolve_web_fetch_config(&WebFetchConfig::default(), None, None),
            ResolvedWebFetchConfig::Local
        );
    }

    #[test]
    fn provider_and_model_overrides_global() {
        let global = WebSearchConfig::default();
        let provider = WebSearchConfig {
            mode: WebSearchMode::Provider,
            ..WebSearchConfig::default()
        };
        let model = WebSearchConfig {
            mode: WebSearchMode::Disabled,
            ..WebSearchConfig::default()
        };

        assert_eq!(
            resolve_web_search_config(&global, Some(&provider), None, &auth())
                .expect("provider override"),
            ResolvedWebSearchConfig::Provider
        );
        assert_eq!(
            resolve_web_search_config(&global, Some(&provider), Some(&model), &auth())
                .expect("model override"),
            ResolvedWebSearchConfig::Disabled
        );
    }

    #[test]
    fn local_provider_requires_credential_in_auth_json() {
        let error = resolve_web_search_config(
            &local_global_config(),
            None,
            None,
            &UserAuthConfigFile::default(),
        )
        .expect_err("missing credential should fail");

        assert!(error.to_string().contains("exa_api_key"));
    }

    #[test]
    fn local_provider_resolves_api_key_reference() {
        let resolved = resolve_web_search_config(&local_global_config(), None, None, &auth())
            .expect("resolve local web search");

        assert_eq!(
            resolved,
            ResolvedWebSearchConfig::Local(ResolvedLocalWebSearchConfig {
                provider_id: "exa".to_string(),
                kind: LocalWebSearchProviderKind::Exa,
                api_key: "secret".to_string(),
                base_url: None,
                max_results: Some(5),
            })
        );
    }
}
