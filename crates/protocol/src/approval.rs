use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use ts_rs::TS;

use crate::{SessionId, TurnId};

/// Describes a UI/client response to a pending approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ApprovalResponseParams {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    #[schemars(with = "String")]
    pub approval_id: SmolStr,
    pub decision: ApprovalDecisionValue,
    pub scope: ApprovalScopeValue,
}

/// Enumerates client decisions for approval requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionValue {
    Approve,
    Deny,
    Cancel,
}

/// Enumerates the scopes supported by approval responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScopeValue {
    Once,
    Turn,
    Session,
    PathPrefix,
    Host,
    Tool,
    CommandPrefix,
    CommandPrefixPersist,
}
