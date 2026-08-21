use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::AcpAuthMethodId;
use crate::AcpErrorCode;
use crate::AcpLogoutCapabilities;
use crate::AcpMeta;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAuthenticateParams {
    #[serde(rename = "methodId")]
    pub method_id: AcpAuthMethodId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpEmptyAuthResult {
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

pub type AcpAuthenticateResult = AcpEmptyAuthResult;
pub type AcpLogoutResult = AcpEmptyAuthResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAuthMethod {
    pub id: AcpAuthMethodId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

impl AcpAuthMethod {
    pub fn agent(
        id: impl Into<String>,
        name: impl Into<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description,
            meta: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAuthCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout: Option<AcpLogoutCapabilities>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

impl AcpAuthCapabilities {
    pub fn with_logout() -> Self {
        Self {
            logout: Some(AcpLogoutCapabilities::default()),
            meta: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.logout.is_none() && self.meta.is_none()
    }
}

pub fn acp_auth_required_response(request_id: serde_json::Value) -> serde_json::Value {
    crate::acp_error_response_with_data(
        request_id,
        AcpErrorCode::AuthenticationRequired,
        "Authentication required",
        serde_json::json!({ "reason": "auth_required" }),
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::AcpAgentCapabilities;
    use crate::AcpInitializeResult;

    use super::*;

    #[test]
    fn auth_method_defaults_to_agent_type() {
        let method: AcpAuthMethod = serde_json::from_value(serde_json::json!({
            "id": "agent-login",
            "name": "Agent login",
            "description": "Sign in using the agent"
        }))
        .expect("deserialize auth method");

        assert_eq!(
            method,
            AcpAuthMethod::agent(
                "agent-login",
                "Agent login",
                Some("Sign in using the agent".to_string())
            )
        );
        assert_eq!(
            serde_json::to_value(method).expect("serialize auth method"),
            serde_json::json!({
                "id": "agent-login",
                "name": "Agent login",
                "description": "Sign in using the agent"
            })
        );
    }

    #[test]
    fn auth_method_ignores_unknown_extension_fields() {
        let method: AcpAuthMethod = serde_json::from_value(serde_json::json!({
            "id": "agent-login",
            "name": "Agent login",
            "type": "agent"
        }))
        .expect("deserialize auth method");

        assert_eq!(
            method,
            AcpAuthMethod::agent("agent-login", "Agent login", /*description*/ None)
        );
    }

    #[test]
    fn auth_required_error_uses_acp_shape() {
        assert_eq!(
            acp_auth_required_response(serde_json::json!(7)),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "error": {
                    "code": -32000,
                    "message": "Authentication required",
                    "data": {
                        "reason": "auth_required"
                    }
                }
            })
        );
    }

    #[test]
    fn initialize_result_advertises_auth_when_configured() {
        let result = AcpInitializeResult {
            protocol_version: 1,
            agent_capabilities: AcpAgentCapabilities {
                auth: AcpAuthCapabilities::with_logout(),
                ..AcpAgentCapabilities::default()
            },
            auth_methods: vec![AcpAuthMethod::agent(
                "agent-login",
                "Agent login",
                Some("Sign in using the agent".to_string()),
            )],
            agent_info: None,
            meta: None,
        };

        let json = serde_json::to_value(result).expect("serialize initialize result");

        assert_eq!(
            json["agentCapabilities"]["auth"],
            serde_json::json!({ "logout": {} })
        );
        assert_eq!(
            json["authMethods"],
            serde_json::json!([
                {
                    "id": "agent-login",
                    "name": "Agent login",
                    "description": "Sign in using the agent"
                }
            ])
        );
    }
}
