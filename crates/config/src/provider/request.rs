//! Resolution of arbitrary provider/model request settings.

use std::collections::BTreeMap;

use serde_json::Value;

use super::catalog::ProviderConfigFile;

/// Resolves provider/model request defaults and headers.
///
/// Provider options are intentionally merged into the request object for the
/// built-in HTTP adapters. This gives custom providers an escape hatch for
/// wire-specific fields while preserving deterministic precedence:
/// provider < model < selected variant, and options < request at each level.
pub fn provider_request_config(
    config: &ProviderConfigFile,
    provider_id: &str,
    model_id: &str,
    variant_id: Option<&str>,
) -> (Option<Value>, BTreeMap<String, String>) {
    let Some(provider) = config.providers.get(provider_id) else {
        return (None, BTreeMap::new());
    };
    let model_id = model_id
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model_id);
    let model = provider.models.get(model_id);
    let selected_variant = model.and_then(|model| {
        variant_id
            .or(model.default_variant.as_deref())
            .and_then(|id| model.variants.get(id))
            .filter(|variant| !variant.disabled)
    });
    let mut request = None;
    merge_json_option(&mut request, provider.options.clone());
    merge_json_option(&mut request, provider.request.clone());
    if let Some(model) = model {
        merge_json_option(&mut request, model.options.clone());
        merge_json_option(&mut request, model.request.clone());
        if let Some(variant) = selected_variant {
            merge_json_option(&mut request, variant.options.clone());
            merge_json_option(&mut request, variant.request.clone());
        }
    }
    let mut headers = provider.headers.clone().unwrap_or_default();
    if let Some(model) = model {
        headers.extend(model.headers.clone());
        if let Some(variant) = selected_variant {
            headers.extend(variant.headers.clone());
        }
    }
    (request, headers)
}

fn merge_json_option(base: &mut Option<Value>, overlay: Option<Value>) {
    let Some(overlay) = overlay else {
        return;
    };
    match overlay {
        Value::Object(overlay) => {
            if let Some(Value::Object(base)) = base.as_mut() {
                for (key, value) in overlay {
                    merge_json_value(base.entry(key).or_insert(Value::Null), value);
                }
            } else {
                *base = Some(Value::Object(overlay));
            }
        }
        overlay => *base = Some(overlay),
    }
}

fn merge_json_value(base: &mut Value, overlay: Value) {
    match overlay {
        Value::Object(overlay) => {
            if let Value::Object(base) = base {
                for (key, value) in overlay {
                    merge_json_value(base.entry(key).or_insert(Value::Null), value);
                }
            } else {
                *base = Value::Object(overlay);
            }
        }
        overlay => *base = overlay,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::provider_request_config;
    use crate::{
        ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig, ProviderModelVariantConfig,
    };

    #[test]
    fn request_defaults_merge_provider_model_and_variant_layers() {
        let config = ProviderConfigFile {
            providers: [(
                "custom".to_string(),
                ProviderConfigEntry {
                    options: Some(serde_json::json!({"timeout": 10, "nested": {"a": true}})),
                    request: Some(serde_json::json!({"provider_field": "p"})),
                    models: [(
                        "model".to_string(),
                        ProviderModelConfig {
                            options: Some(serde_json::json!({"nested": {"b": true}})),
                            request: Some(serde_json::json!({"model_field": "m"})),
                            variants: [(
                                "fast".to_string(),
                                ProviderModelVariantConfig {
                                    options: Some(serde_json::json!({"timeout": 5})),
                                    request: Some(serde_json::json!({"variant_field": "v"})),
                                    ..ProviderModelVariantConfig::default()
                                },
                            )]
                            .into_iter()
                            .collect(),
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

        assert_eq!(
            provider_request_config(&config, "custom", "model", Some("fast")),
            (
                Some(serde_json::json!({
                    "timeout": 5,
                    "nested": {"a": true, "b": true},
                    "provider_field": "p",
                    "model_field": "m",
                    "variant_field": "v"
                })),
                std::collections::BTreeMap::new()
            )
        );
    }
}
