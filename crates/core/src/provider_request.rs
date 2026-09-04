//! Runtime composition of arbitrary model request defaults.

use std::collections::BTreeMap;

use serde_json::Value;

const REQUEST_HEADERS_KEY: &str = "__devo_request_headers";

/// Merges configured provider/model defaults with per-turn reasoning fields.
///
/// Both values are JSON objects in normal use. Object members are merged
/// recursively and the turn-specific value wins on scalar conflicts.
pub fn merge_model_request_body(
    defaults: Option<&Value>,
    turn_extra: Option<Value>,
) -> Option<Value> {
    let mut result = defaults.cloned();
    let Some(turn_extra) = turn_extra else {
        return result;
    };
    let Some(result) = result.as_mut() else {
        return Some(turn_extra);
    };
    merge_json_value(result, turn_extra);
    Some(result.clone())
}

/// Adds resolved model/variant headers to the internal request envelope.
/// Provider adapters strip this envelope before serializing the JSON body.
pub fn add_model_request_headers(
    body: Option<Value>,
    headers: &BTreeMap<String, String>,
) -> Option<Value> {
    if headers.is_empty() {
        return body;
    }
    let mut body = body.unwrap_or_else(|| Value::Object(Default::default()));
    if let Value::Object(object) = &mut body {
        object.insert(
            REQUEST_HEADERS_KEY.to_string(),
            serde_json::to_value(headers).expect("serialize model request headers"),
        );
    }
    Some(body)
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
