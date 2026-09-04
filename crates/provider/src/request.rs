//! Shared request-body helpers for provider adapters.
//!
//! Provider-specific adapters build their normal payload first, then overlay
//! `extra_body` so caller-supplied escape-hatch fields keep their documented
//! precedence without each adapter reimplementing the merge contract.

use std::collections::BTreeMap;

use serde_json::Value;

pub(crate) const REQUEST_HEADERS_KEY: &str = "__devo_request_headers";

/// Merges an extra JSON object into a provider request body.
pub fn merge_extra_body(body: &mut Value, extra_body: Option<&Value>) {
    let Some(extra_body) = extra_body else {
        return;
    };
    let Some(body_object) = body.as_object_mut() else {
        return;
    };
    let Some(extra_object) = extra_body.as_object() else {
        return;
    };

    for (key, value) in extra_object {
        if key == REQUEST_HEADERS_KEY {
            continue;
        }
        body_object.insert(key.clone(), value.clone());
    }
}

/// Reads the reserved internal header envelope from request defaults.
pub(crate) fn request_headers(extra_body: Option<&Value>) -> BTreeMap<String, String> {
    extra_body
        .and_then(|body| body.get(REQUEST_HEADERS_KEY))
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::merge_extra_body;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn merge_extra_body_overrides_existing_fields() {
        let mut body = json!({
            "model": "base-model",
            "temperature": 0.2
        });
        let extra = json!({
            "temperature": 0.8,
            "top_k": 32
        });

        merge_extra_body(&mut body, Some(&extra));

        assert_eq!(
            body,
            json!({
                "model": "base-model",
                "temperature": 0.8,
                "top_k": 32
            })
        );
    }
}
