use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($name:ident) => {
        /// Strongly typed UUID v7 identifier used by the conversation model.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(
            #[doc = "The underlying UUID value carried by this typed identifier."] Uuid,
        );

        impl $name {
            /// Creates a new time-ordered UUID v7 identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = uuid::Error;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }

        impl TryFrom<String> for $name {
            type Error = uuid::Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::try_from(value.as_str())
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::try_from(s)
            }
        }
    };
}

define_id!(SessionId);
define_id!(TurnId);
define_id!(ItemId);

#[cfg(test)]
mod tests {
    use super::{ItemId, SessionId, TurnId};

    #[test]
    fn ids_roundtrip_via_string() {
        let id = SessionId::new();
        let parsed = SessionId::try_from(id.to_string()).expect("id should parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn generated_ids_are_unique() {
        let first = TurnId::new();
        let second = TurnId::new();
        assert_ne!(first, second);
    }

    #[test]
    fn ids_serialize_as_strings() {
        let id = ItemId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let restored: ItemId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, restored);
    }
}
