//! Tri-state patch field: `Missing | Null | Value(T)`.
//!
//! Public wire types never expose `Option<Option<T>>` for patches. A field
//! that is omitted from the JSON object means "leave unchanged", an explicit
//! `null` means "clear", and any other value means "set". Struct fields using
//! this type must be annotated with:
//!
//! ```rust,ignore
//! #[serde(default, skip_serializing_if = "PatchField::is_missing")]
//! pub title: PatchField<String>,
//! ```

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

#[derive(Debug, Clone, PartialEq, Eq, Default, JsonSchema)]
#[serde(untagged)]
pub enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<T> PatchField<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl<T: Serialize> Serialize for PatchField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Callers are expected to skip `Missing` via `skip_serializing_if`;
            // serializing it directly degrades to `null` rather than failing.
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A present field is either `null` or a value; absence is handled by
        // `#[serde(default)]` on the containing struct field.
        Ok(Option::<T>::deserialize(deserializer)?
            .map_or(Self::Null, Self::Value))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde::Deserialize;
    use serde::Serialize;

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Patch {
        #[serde(default, skip_serializing_if = "PatchField::is_missing")]
        title: PatchField<String>,
    }

    #[test]
    fn missing_null_and_value_are_distinct() {
        let missing: Patch = serde_json::from_str("{}").expect("missing");
        assert_eq!(missing.title, PatchField::Missing);

        let null: Patch = serde_json::from_str("{\"title\":null}").expect("null");
        assert_eq!(null.title, PatchField::Null);

        let value: Patch = serde_json::from_str("{\"title\":\"hi\"}").expect("value");
        assert_eq!(value.title, PatchField::Value("hi".to_owned()));
    }

    #[test]
    fn missing_is_omitted_when_serializing() {
        let patch = Patch {
            title: PatchField::Missing,
        };
        assert_eq!(serde_json::to_string(&patch).expect("serialize"), "{}");
    }
}
