use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonSchemaPrimitiveType {
    String,
    Number,
    Boolean,
    Integer,
    Object,
    Array,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonSchemaType {
    Single(JsonSchemaPrimitiveType),
    Multiple(Vec<JsonSchemaPrimitiveType>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AdditionalProperties {
    Bool(bool),
    Schema(Box<JsonSchema>),
}

impl From<bool> for AdditionalProperties {
    fn from(b: bool) -> Self {
        AdditionalProperties::Bool(b)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchema {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<JsonSchemaType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<AdditionalProperties>,
}

impl JsonSchema {
    pub fn string(description: Option<&str>) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::String)),
            description: description.map(|d| d.to_string()),
            ..Default::default()
        }
    }

    pub fn boolean(description: Option<&str>) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Boolean)),
            description: description.map(|d| d.to_string()),
            ..Default::default()
        }
    }

    pub fn integer(description: Option<&str>) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Integer)),
            description: description.map(|d| d.to_string()),
            ..Default::default()
        }
    }

    pub fn number(description: Option<&str>) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Number)),
            description: description.map(|d| d.to_string()),
            ..Default::default()
        }
    }

    pub fn array(items: JsonSchema, description: Option<&str>) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array)),
            description: description.map(|d| d.to_string()),
            items: Some(Box::new(items)),
            ..Default::default()
        }
    }

    pub fn object(
        properties: BTreeMap<String, JsonSchema>,
        required: Option<Vec<String>>,
        additional_properties: Option<bool>,
    ) -> Self {
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object)),
            properties: Some(properties),
            required,
            additional_properties: additional_properties.map(Into::into),
            ..Default::default()
        }
    }

    pub fn to_json_value(&self) -> JsonValue {
        serde_json::to_value(self).unwrap_or(JsonValue::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_schema() {
        let s = JsonSchema::string(Some("a name"));
        assert_eq!(
            s.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::String))
        );
        assert_eq!(s.description, Some("a name".into()));
    }

    #[test]
    fn boolean_schema() {
        let s = JsonSchema::boolean(Some("flag"));
        assert_eq!(
            s.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Boolean))
        );
        assert_eq!(s.description, Some("flag".into()));
    }

    #[test]
    fn integer_number_schema() {
        let i = JsonSchema::integer(Some("count"));
        assert_eq!(
            i.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Integer))
        );
        let n = JsonSchema::number(Some("price"));
        assert_eq!(
            n.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Number))
        );
    }

    #[test]
    fn array_schema() {
        let a = JsonSchema::array(JsonSchema::string(None), Some("list"));
        assert_eq!(
            a.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array))
        );
        assert!(a.items.is_some());
        assert_eq!(a.description, Some("list".into()));
    }

    #[test]
    fn object_schema() {
        let mut props = BTreeMap::new();
        props.insert("name".into(), JsonSchema::string(Some("name")));
        let o = JsonSchema::object(props, Some(vec!["name".into()]), Some(false));
        assert_eq!(
            o.schema_type,
            Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
        );
        assert!(o.properties.is_some());
        assert_eq!(o.required.unwrap(), vec!["name"]);
        assert_eq!(
            o.additional_properties,
            Some(AdditionalProperties::Bool(false))
        );
    }

    #[test]
    fn to_json_value_roundtrip() {
        let s = JsonSchema::object(
            BTreeMap::from([
                ("name".into(), JsonSchema::string(Some("The name"))),
                ("count".into(), JsonSchema::integer(None)),
            ]),
            Some(vec!["name".into()]),
            Some(false),
        );
        let json = s.to_json_value();
        assert!(json.is_object());
        assert_eq!(json["type"], "object");
        assert!(json["properties"].is_object());
        assert_eq!(json["required"][0], "name");
        assert_eq!(json["properties"]["name"]["type"], "string");
        assert_eq!(json["properties"]["count"]["type"], "integer");
        assert_eq!(json["additionalProperties"], false);
    }

    #[test]
    fn default_schema_is_empty() {
        let s = JsonSchema::default();
        let json = s.to_json_value();
        assert_eq!(json, serde_json::json!({}));
    }
}
