//! Opaque identifier newtypes for the canonical protocol surface.
//!
//! IDs are opaque strings on the wire. Newly created resources use a prefixed
//! form (`ses_` / `turn_` / `item_` / ...); legacy bare UUIDs from pre-v2
//! rollouts remain valid, must round-trip unchanged, and are accepted anywhere
//! an ID is expected. Clients must not parse IDs.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

macro_rules! define_opaque_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl JsonSchema for $name {
            fn schema_name() -> String {
                String::from(stringify!($name))
            }

            fn json_schema(
                generator: &mut schemars::r#gen::SchemaGenerator,
            ) -> schemars::schema::Schema {
                String::json_schema(generator)
            }
        }

        impl TS for $name {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;

            fn name(_: &ts_rs::Config) -> String {
                String::from(stringify!($name))
            }

            fn inline(cfg: &ts_rs::Config) -> String {
                Self::name(cfg)
            }

            fn decl(_: &ts_rs::Config) -> String {
                String::from(concat!("type ", stringify!($name), " = string;"))
            }
        }

        impl $name {
            /// Generates a new prefixed ID (`<prefix><uuid-v7>`).
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, Uuid::now_v7().simple()))
            }

            /// Wraps a legacy bare UUID from pre-v2 rollout files, preserving
            /// the original textual form so it round-trips unchanged.
            pub fn from_legacy_uuid(value: Uuid) -> Self {
                Self(value.to_string())
            }

            /// Wraps an ID string received over the wire without interpreting it.
            pub fn from_string(value: String) -> Self {
                Self(value)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }
    };
}

define_opaque_id!(SessionId, "ses_");
define_opaque_id!(TurnId, "turn_");
define_opaque_id!(ItemId, "item_");
define_opaque_id!(GoalId, "goal_");
define_opaque_id!(EventId, "evt_");
define_opaque_id!(RunId, "run_");
define_opaque_id!(SubscriptionId, "sub_");
define_opaque_id!(QueueItemId, "qit_");
define_opaque_id!(RestorePlanId, "rpl_");

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn new_ids_carry_their_prefix() {
        assert!(SessionId::new().as_str().starts_with("ses_"));
        assert!(TurnId::new().as_str().starts_with("turn_"));
        assert!(ItemId::new().as_str().starts_with("item_"));
        assert!(GoalId::new().as_str().starts_with("goal_"));
    }

    #[test]
    fn legacy_bare_uuid_round_trips_unchanged() {
        let uuid = Uuid::now_v7();
        let id = SessionId::from_legacy_uuid(uuid);
        assert_eq!(id.as_str(), uuid.to_string());
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{uuid}\""));
        let back: SessionId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }
}
