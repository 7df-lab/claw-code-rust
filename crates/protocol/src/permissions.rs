use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum PermissionPreset {
    /// Read and edit workspace files and run shell commands; network and
    /// outside-workspace writes ask.
    #[default]
    Default,
    /// Same base policy as default, but eligible approvals may be routed
    /// through an automatic reviewer before the user is interrupted.
    AutoReview,
    /// Allow all tool requests without approval.
    FullAccess,
}

impl<'de> Deserialize<'de> for PermissionPreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            // The "read-only" preset was removed; existing configs and
            // persisted sessions that still reference it are migrated to Default
            // (more permissive: shell is allowed). Prefer sandbox_profile =
            // "read-only" for OS-level restriction.
            "default" => Ok(PermissionPreset::Default),
            "read-only" => {
                tracing::warn!(
                    "permission_preset \"read-only\" is deprecated and maps to Default \
                     (shell allowed); set sandbox_profile = \"read-only\" for a \
                     read-only OS sandbox instead"
                );
                Ok(PermissionPreset::Default)
            }
            "auto-review" => Ok(PermissionPreset::AutoReview),
            "full-access" => Ok(PermissionPreset::FullAccess),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["default", "auto-review", "full-access"],
            )),
        }
    }
}
