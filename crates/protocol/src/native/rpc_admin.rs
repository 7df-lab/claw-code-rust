//! Params/result types for connection, catalog, context, permission and
//! credential methods. Truth source: `devo-api-design/01-native-api.md`
//! §4.1/§4.4/§4.7.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use std::path::PathBuf;

use super::ids::SessionId;
use super::item::ContextOccupancy;
use super::item::ToolSource;
use super::model::PermissionProfile;

// ── initialize ──

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Modalities the client can render, e.g. `text`, `image`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<String>,
    /// Delta encodings the client accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delta_encodings: Vec<String>,
    /// Experimental extensions the client opts into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experimental: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Date version requested by the client, e.g. `2026-08-01`.
    pub protocol_version: String,
    /// Stable identity of this client installation; scopes idempotency keys.
    pub client_identity: String,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub connection_id: String,
    /// The negotiated (possibly downgraded) protocol date version.
    pub protocol_version: String,
    pub server_instance_id: String,
    pub capabilities: ServerCapabilities,
    pub limits: ServerLimits,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delta_encodings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experimental: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_depth: Option<u32>,
}

// ── runtime/ping ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePingParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePingResult {
    pub server_time_ms: i64,
}

// ── model/preferences/read|write (ratified #12) ──

/// One selectable value in a preferences option list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Effective model preferences for a workspace — the defaults new sessions
/// start with — plus the selectable values. Mode is deliberately absent:
/// permission mode is session-scoped (`session/metadata/update`), not a
/// model preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferences {
    /// Current default: the provider model binding id when configured,
    /// otherwise the model slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Current default reasoning effort selection
    /// (`disabled`/`low`/`high`/`max`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub available_models: Vec<PreferencesOption>,
    pub available_efforts: Vec<PreferencesOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesReadParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesReadResult {
    pub preferences: ModelPreferences,
}

/// Patch semantics: only present fields change. Naturally idempotent (the
/// write sets absolute values), so no idempotency key is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesWriteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub patch: ModelPreferencesPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesWriteResult {
    pub preferences: ModelPreferences,
}

// ── catalog ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub slug: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Wire API the model is invoked through (legacy enum reused; see
    /// `crate::ProviderWireApi`).
    pub provider: crate::ProviderWireApi,
    pub context_window: u32,
    pub reasoning_capability: crate::ReasoningCapability,
    pub input_modalities: Vec<crate::InputModality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl From<crate::ModelCatalogEntry> for ModelInfo {
    fn from(entry: crate::ModelCatalogEntry) -> Self {
        Self {
            slug: entry.slug,
            display_name: entry.display_name,
            channel: entry.channel,
            description: entry.description,
            provider: entry.provider,
            context_window: entry.context_window,
            reasoning_capability: entry.reasoning_capability,
            input_modalities: entry.input_modalities,
            max_tokens: entry.max_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResult {
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub source: ToolSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolListResult {
    pub tools: Vec<ToolInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillListParams {
    /// Workspace root to scope the listing to (ratified #4); absent lists
    /// user-global skills only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub force_reload: bool,
}

/// Native skill record (ratified #4): keyed by `path` (the TUI's keying),
/// carrying everything the picker and metadata surfaces render. Legacy
/// `SkillSource`/`SkillScope`/`SkillInterface` are reused unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<crate::SkillInterface>,
    pub path: PathBuf,
    pub enabled: bool,
    pub source: crate::SkillSource,
    pub scope: crate::SkillScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

impl From<crate::SkillRecord> for SkillInfo {
    fn from(record: crate::SkillRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            description: record.description,
            short_description: record.short_description,
            interface: record.interface,
            path: record.path,
            enabled: record.enabled,
            source: record.source,
            scope: record.scope,
            plugin_id: record.plugin_id,
        }
    }
}

impl From<SkillInfo> for crate::SkillRecord {
    fn from(info: SkillInfo) -> Self {
        Self {
            id: info.id,
            name: info.name,
            description: info.description,
            short_description: info.short_description,
            interface: info.interface,
            dependencies: None,
            path: info.path,
            enabled: info.enabled,
            source: info.source,
            scope: info.scope,
            plugin_id: info.plugin_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillListResult {
    pub skills: Vec<SkillInfo>,
}

/// Keyed by `path` (ratified #4): paths are unambiguous across user and
/// workspace scopes where names collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillSetEnabledParams {
    pub path: PathBuf,
    pub enabled: bool,
    /// Optional workspace root for the returned catalog snapshot. When absent,
    /// the response is scoped to user-global skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillSetEnabledResult {
    pub skills: Vec<SkillInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub status: String,
    pub tool_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpListResult {
    pub servers: Vec<McpServerInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsParams {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpToolEntry {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsResult {
    pub tools: Vec<McpToolEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpSetEnabledParams {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpSetEnabledResult {
    pub servers: Vec<McpServerInfo>,
}

// ── context/usage/read ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageReadParams {
    pub session_id: SessionId,
}

/// Current context-window occupancy with category breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageReadResult {
    pub occupancy: ContextOccupancy,
}

// ── permission/profile/* ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProfileReadParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProfileReadResult {
    pub profile: PermissionProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProfileUpdateParams {
    pub session_id: SessionId,
    pub profile: PermissionProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProfileUpdateResult {
    pub profile: PermissionProfile,
}

// ── provider/* (ratified #11) ──

/// Native mirror of the legacy provider vendor. `credential` is a
/// credential *id* into `auth.json`, never the secret; `api_key` on
/// upsert/validate is write-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderVendorInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<String>,
    pub wire_apis: Vec<crate::ProviderWireApi>,
    pub enabled: bool,
}

impl From<crate::ProviderVendor> for ProviderVendorInfo {
    fn from(vendor: crate::ProviderVendor) -> Self {
        Self {
            name: vendor.name,
            base_url: vendor.base_url,
            credential: vendor.credential,
            headers: vendor.headers,
            wire_apis: vendor.wire_apis,
            enabled: vendor.enabled,
        }
    }
}

impl From<ProviderVendorInfo> for crate::ProviderVendor {
    fn from(vendor: ProviderVendorInfo) -> Self {
        Self {
            name: vendor.name,
            base_url: vendor.base_url,
            credential: vendor.credential,
            headers: vendor.headers,
            wire_apis: vendor.wire_apis,
            enabled: vendor.enabled,
        }
    }
}

/// Native mirror of the legacy provider model binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelBindingInfo {
    pub binding_id: String,
    pub model_slug: String,
    pub provider: String,
    pub request_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub invocation_method: crate::ProviderWireApi,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    pub enabled: bool,
}

impl From<crate::ProviderModelBinding> for ProviderModelBindingInfo {
    fn from(binding: crate::ProviderModelBinding) -> Self {
        Self {
            binding_id: binding.binding_id,
            model_slug: binding.model_slug,
            provider: binding.provider,
            request_model: binding.request_model,
            display_name: binding.display_name,
            invocation_method: binding.invocation_method,
            default_reasoning_effort: binding.default_reasoning_effort,
            enabled: binding.enabled,
        }
    }
}

impl From<ProviderModelBindingInfo> for crate::ProviderModelBinding {
    fn from(binding: ProviderModelBindingInfo) -> Self {
        Self {
            binding_id: binding.binding_id,
            model_slug: binding.model_slug,
            provider: binding.provider,
            request_model: binding.request_model,
            display_name: binding.display_name,
            invocation_method: binding.invocation_method,
            default_reasoning_effort: binding.default_reasoning_effort,
            enabled: binding.enabled,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListResult {
    pub providers: Vec<ProviderVendorInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpsertParams {
    pub provider_vendor: ProviderVendorInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ProviderModelBindingInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model_binding: Option<String>,
    /// Write-only secret, stored in the keyring; never echoed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpsertResult {
    pub provider_vendor: ProviderVendorInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ProviderModelBindingInfo>,
}

/// Live network probe of a vendor + binding configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderValidateParams {
    pub provider_vendor: ProviderVendorInfo,
    pub model_binding: ProviderModelBindingInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderValidateResult {
    pub reply_preview: String,
}

// ── credential/* ──

/// Secrets are never echoed back; only id/provider/mask are returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialInfo {
    pub id: String,
    pub provider: String,
    pub masked: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialListResult {
    pub credentials: Vec<CredentialInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSetParams {
    pub provider: String,
    /// The secret itself; write-only, never appears in any response.
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSetResult {
    pub credential: CredentialInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDeleteParams {
    pub credential_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDeleteResult {}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn mcp_tools_params_and_result_round_trip_camel_case() {
        let params = McpToolsParams {
            name: "time".to_string(),
        };
        let params_json = serde_json::to_value(&params).expect("serialize params");
        assert_eq!(params_json, serde_json::json!({ "name": "time" }));
        assert_eq!(
            serde_json::from_value::<McpToolsParams>(params_json).expect("parse params"),
            params
        );

        let result = McpToolsResult {
            tools: vec![McpToolEntry {
                name: "get_time".to_string(),
                description: "Current time".to_string(),
            }],
        };
        let result_json = serde_json::to_value(&result).expect("serialize result");
        assert_eq!(
            result_json,
            serde_json::json!({
                "tools": [{ "name": "get_time", "description": "Current time" }]
            })
        );
        assert_eq!(
            serde_json::from_value::<McpToolsResult>(result_json).expect("parse result"),
            result
        );
    }
}
