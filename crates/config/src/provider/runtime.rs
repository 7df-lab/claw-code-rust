//! Provider fields that require rebuilding the live HTTP adapter.

use super::{ProviderConfigEntry, ProviderConfigFile};

/// Returns whether two provider catalogs require a new provider router.
///
/// Model metadata, default selections, and request/options overlays are
/// resolved per turn and therefore do not require rebuilding the HTTP
/// adapters. Only fields used to construct adapters or their route table are
/// compared here.
pub fn provider_runtime_config_changed(
    current: &ProviderConfigFile,
    inherited: &ProviderConfigFile,
) -> bool {
    if current.providers.len() != inherited.providers.len() {
        return true;
    }

    current.providers.iter().any(|(provider_id, provider)| {
        inherited
            .providers
            .get(provider_id)
            .is_none_or(|inherited_provider| {
                provider_runtime_fields_changed(provider, inherited_provider)
            })
    })
}

fn provider_runtime_fields_changed(
    current: &ProviderConfigEntry,
    inherited: &ProviderConfigEntry,
) -> bool {
    current.base_url != inherited.base_url
        || current.credential != inherited.credential
        || current.headers != inherited.headers
        || current.wire_api != inherited.wire_api
        || current.enabled != inherited.enabled
        || model_wire_apis(current) != model_wire_apis(inherited)
}

fn model_wire_apis(provider: &ProviderConfigEntry) -> Vec<String> {
    let mut wire_apis = provider
        .models
        .values()
        .filter_map(|model| model.wire_api)
        .map(|wire_api| wire_api.as_str().to_string())
        .collect::<Vec<_>>();
    wire_apis.sort();
    wire_apis.dedup();
    wire_apis
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::provider_runtime_config_changed;
    use crate::{ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig};

    #[test]
    fn ignores_model_metadata_and_default_selection_changes() {
        let current = ProviderConfigFile {
            model: Some("custom/model".to_string()),
            providers: [(
                "custom".to_string(),
                ProviderConfigEntry {
                    models: [("model".to_string(), ProviderModelConfig::default())]
                        .into_iter()
                        .collect(),
                    ..ProviderConfigEntry::default()
                },
            )]
            .into_iter()
            .collect(),
            ..ProviderConfigFile::default()
        };
        let inherited = ProviderConfigFile {
            model: Some("custom/other-model".to_string()),
            providers: [(
                "custom".to_string(),
                ProviderConfigEntry {
                    models: [(
                        "model".to_string(),
                        ProviderModelConfig {
                            name: Some("Old display name".to_string()),
                            context_window: Some(32_000),
                            ..ProviderModelConfig::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                    ..ProviderConfigEntry::default()
                },
            )]
            .into_iter()
            .collect(),
            ..ProviderConfigFile::default()
        };

        assert_eq!(provider_runtime_config_changed(&current, &inherited), false);
    }

    #[test]
    fn detects_endpoint_credential_and_route_changes() {
        let base = ProviderConfigFile {
            providers: [("custom".to_string(), ProviderConfigEntry::default())]
                .into_iter()
                .collect(),
            ..ProviderConfigFile::default()
        };
        let mut changed = base.clone();
        changed.providers.get_mut("custom").unwrap().base_url =
            Some("https://example.com".to_string());
        assert_eq!(provider_runtime_config_changed(&changed, &base), true);

        changed = base.clone();
        changed.providers.get_mut("custom").unwrap().credential =
            Some("custom_api_key".to_string());
        assert_eq!(provider_runtime_config_changed(&changed, &base), true);
    }
}
