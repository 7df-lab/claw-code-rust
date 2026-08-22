//! The single Native API method registry. Truth source for the wire contract:
//! OpenRPC, JSON Schema, the TS SDK and method docs are all generated from
//! this registry, and CI asserts that the registry, the OpenRPC document and
//! client constants enumerate the same method set.
//!
//! Gate (01 §10): a method may not ship with only a prose description — every
//! method registers its params/result types, error codes, capability and
//! idempotency behavior exactly once, here.

use schemars::JsonSchema;
use schemars::schema::RootSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

use super::error::codes;
use super::event::*;
use super::item::ApprovalDecision;
use super::item::Item;
use super::item::ItemEnvelope;
use super::page::Page;
use super::rpc_admin::*;
use super::rpc_search::*;
use super::rpc_session::*;
use super::rpc_turn::*;
use super::rpc_workspace::*;
use super::turn::Turn;

/// How a write method protects against retries and lost updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idempotency {
    /// Reads and naturally idempotent operations (`session/interrupt`,
    /// `subscription/ack`, `session/queue/remove`).
    None,
    /// Carries `idempotencyKey`, scoped to `(clientIdentity, method, key)`.
    Key,
    /// Carries `expectedVersion` (guards against lost update).
    ExpectedVersion,
    /// The single-use `restorePlanId` is the idempotency identity.
    RestorePlan,
}

pub struct MethodSpec {
    pub name: &'static str,
    pub params_schema: fn() -> RootSchema,
    pub result_schema: fn() -> RootSchema,
    pub error_codes: &'static [&'static str],
    /// Experimental methods name the initialize capability that gates them.
    pub required_capability: Option<&'static str>,
    pub idempotency: Idempotency,
}

fn schema_of<T: JsonSchema>() -> RootSchema {
    schemars::schema_for!(T)
}

// ── Server -> Client reverse requests (01 §6) ──

/// Interactions needing one unique verifiable answer use JSON-RPC requests,
/// not notifications. Each corresponds to a `waiting`-state item; the first
/// valid response wins. Late JSON-RPC responses are ignored because a response
/// cannot itself receive another JSON-RPC error response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRespondParams {
    pub request_id: String,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRespondParams {
    pub request_id: String,
    pub answers: JsonValue,
}

const SESSION_ERRORS: &[&str] = &[codes::SESSION_NOT_FOUND];
const TURN_ERRORS: &[&str] = &[codes::SESSION_NOT_FOUND, codes::TURN_ALREADY_ACTIVE];
const GOAL_ERRORS: &[&str] = &[
    codes::SESSION_NOT_FOUND,
    codes::GOAL_NOT_FOUND,
    codes::GOAL_TRANSITION_INVALID,
];

pub static NATIVE_METHODS: &[MethodSpec] = &[
    // ── Connection & subscription ──
    MethodSpec {
        name: "initialize",
        params_schema: schema_of::<InitializeParams>,
        result_schema: schema_of::<InitializeResult>,
        error_codes: &[codes::UNSUPPORTED_PROTOCOL_VERSION],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "runtime/ping",
        params_schema: schema_of::<RuntimePingParams>,
        result_schema: schema_of::<RuntimePingResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "subscription/create",
        params_schema: schema_of::<SubscriptionCreateParams>,
        result_schema: schema_of::<SubscriptionCreateResult>,
        error_codes: &[codes::NOT_INITIALIZED, codes::CURSOR_EXPIRED],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "subscription/update",
        params_schema: schema_of::<SubscriptionUpdateParams>,
        result_schema: schema_of::<SubscriptionCreateResult>,
        error_codes: &[codes::NOT_INITIALIZED],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "subscription/ack",
        params_schema: schema_of::<SubscriptionAckParams>,
        result_schema: schema_of::<RuntimePingResult>,
        error_codes: &[codes::CURSOR_EXPIRED],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "subscription/unsubscribe",
        params_schema: schema_of::<SubscriptionUnsubscribeParams>,
        result_schema: schema_of::<RuntimePingResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Session ──
    MethodSpec {
        name: "session/new",
        params_schema: schema_of::<SessionNewParams>,
        result_schema: schema_of::<SessionNewResult>,
        error_codes: &[
            codes::INVALID_CWD,
            codes::CWD_ACCESS_DENIED,
            codes::IDEMPOTENCY_CONFLICT,
        ],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/list",
        params_schema: schema_of::<SessionListParams>,
        result_schema: schema_of::<SessionListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/read",
        params_schema: schema_of::<SessionReadParams>,
        result_schema: schema_of::<SessionReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/resume",
        params_schema: schema_of::<SessionResumeParams>,
        result_schema: schema_of::<SessionResumeResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/fork",
        params_schema: schema_of::<SessionForkParams>,
        result_schema: schema_of::<SessionForkResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/rollback/preview",
        params_schema: schema_of::<SessionRollbackPreviewParams>,
        result_schema: schema_of::<SessionRollbackPreviewResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/rollback/commit",
        params_schema: schema_of::<SessionRollbackCommitParams>,
        result_schema: schema_of::<SessionRollbackCommitResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::RESTORE_PLAN_NOT_FOUND,
            codes::RESTORE_PLAN_EXPIRED,
            codes::WORKSPACE_VERSION_CONFLICT,
        ],
        required_capability: None,
        idempotency: Idempotency::RestorePlan,
    },
    MethodSpec {
        name: "session/metadata/update",
        params_schema: schema_of::<SessionMetadataUpdateParams>,
        result_schema: schema_of::<SessionMetadataUpdateResult>,
        error_codes: &[codes::SESSION_NOT_FOUND, codes::VERSION_CONFLICT],
        required_capability: None,
        idempotency: Idempotency::ExpectedVersion,
    },
    MethodSpec {
        name: "session/cwd/change",
        params_schema: schema_of::<SessionCwdChangeParams>,
        result_schema: schema_of::<SessionCwdChangeResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::INVALID_CWD,
            codes::CWD_ACCESS_DENIED,
        ],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/archive",
        params_schema: schema_of::<SessionArchiveParams>,
        result_schema: schema_of::<SessionArchiveResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/delete",
        params_schema: schema_of::<SessionDeleteParams>,
        result_schema: schema_of::<SessionDeleteResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/turns/list",
        params_schema: schema_of::<SessionTurnsListParams>,
        result_schema: schema_of::<Page<Turn>>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/items/list",
        params_schema: schema_of::<SessionItemsListParams>,
        result_schema: schema_of::<Page<ItemEnvelope>>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Turn & queue ──
    MethodSpec {
        name: "turn/start",
        params_schema: schema_of::<TurnStartParams>,
        result_schema: schema_of::<TurnStartResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::TURN_ALREADY_ACTIVE,
            codes::IDEMPOTENCY_CONFLICT,
            codes::UNSUPPORTED_MODALITY,
        ],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "turn/steer",
        params_schema: schema_of::<TurnSteerParams>,
        result_schema: schema_of::<TurnSteerResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::TURN_NOT_STEERABLE,
            codes::IDEMPOTENCY_CONFLICT,
            codes::UNSUPPORTED_MODALITY,
        ],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/interrupt",
        params_schema: schema_of::<SessionInterruptParams>,
        result_schema: schema_of::<SessionInterruptResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "turn/read",
        params_schema: schema_of::<TurnReadParams>,
        result_schema: schema_of::<TurnReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/queue/push",
        params_schema: schema_of::<SessionQueuePushParams>,
        result_schema: schema_of::<SessionQueuePushResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::IDEMPOTENCY_CONFLICT,
            codes::UNSUPPORTED_MODALITY,
        ],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/queue/list",
        params_schema: schema_of::<SessionQueueListParams>,
        result_schema: schema_of::<SessionQueueListResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/queue/update",
        params_schema: schema_of::<SessionQueueUpdateParams>,
        result_schema: schema_of::<SessionQueueUpdateResult>,
        error_codes: &[codes::SESSION_NOT_FOUND, codes::QUEUE_ITEM_NOT_FOUND],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/queue/remove",
        params_schema: schema_of::<SessionQueueRemoveParams>,
        result_schema: schema_of::<SessionQueueRemoveResult>,
        error_codes: &[codes::SESSION_NOT_FOUND, codes::QUEUE_ITEM_NOT_FOUND],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/queue/steer",
        params_schema: schema_of::<SessionQueueSteerParams>,
        result_schema: schema_of::<SessionQueueSteerResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::QUEUE_ITEM_NOT_FOUND,
            codes::TURN_NOT_STEERABLE,
        ],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Catalog / context ──
    MethodSpec {
        name: "model/list",
        params_schema: schema_of::<ModelListParams>,
        result_schema: schema_of::<ModelListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "model/preferences/read",
        params_schema: schema_of::<ModelPreferencesReadParams>,
        result_schema: schema_of::<ModelPreferencesReadResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "model/preferences/write",
        params_schema: schema_of::<ModelPreferencesWriteParams>,
        result_schema: schema_of::<ModelPreferencesWriteResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Provider ──
    MethodSpec {
        name: "provider/list",
        params_schema: schema_of::<ProviderListParams>,
        result_schema: schema_of::<ProviderListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "provider/upsert",
        params_schema: schema_of::<ProviderUpsertParams>,
        result_schema: schema_of::<ProviderUpsertResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "provider/validate",
        params_schema: schema_of::<ProviderValidateParams>,
        result_schema: schema_of::<ProviderValidateResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "tool/list",
        params_schema: schema_of::<ToolListParams>,
        result_schema: schema_of::<ToolListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "skill/list",
        params_schema: schema_of::<SkillListParams>,
        result_schema: schema_of::<SkillListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "skill/set_enabled",
        params_schema: schema_of::<SkillSetEnabledParams>,
        result_schema: schema_of::<SkillSetEnabledResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "mcp/list",
        params_schema: schema_of::<McpListParams>,
        result_schema: schema_of::<McpListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "mcp/tools",
        params_schema: schema_of::<McpToolsParams>,
        result_schema: schema_of::<McpToolsResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "mcp/set_enabled",
        params_schema: schema_of::<McpSetEnabledParams>,
        result_schema: schema_of::<McpSetEnabledResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "context/usage/read",
        params_schema: schema_of::<ContextUsageReadParams>,
        result_schema: schema_of::<ContextUsageReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/compact/start",
        params_schema: schema_of::<SessionCompactStartParams>,
        result_schema: schema_of::<TurnStartResult>,
        error_codes: TURN_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Connection-local search ──
    MethodSpec {
        name: "search/start",
        params_schema: schema_of::<SearchStartParams>,
        result_schema: schema_of::<SearchStartResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "search/update",
        params_schema: schema_of::<SearchUpdateParams>,
        result_schema: schema_of::<SearchUpdateResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "search/cancel",
        params_schema: schema_of::<SearchCancelParams>,
        result_schema: schema_of::<SearchCancelResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Workspace ──
    MethodSpec {
        name: "workspace/changes/read",
        params_schema: schema_of::<WorkspaceChangesReadParams>,
        result_schema: schema_of::<WorkspaceChangesReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Goal ──
    MethodSpec {
        name: "session/goal/set",
        params_schema: schema_of::<SessionGoalSetParams>,
        result_schema: schema_of::<SessionGoalSetResult>,
        error_codes: &[codes::SESSION_NOT_FOUND, codes::IDEMPOTENCY_CONFLICT],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/goal/read",
        params_schema: schema_of::<SessionGoalReadParams>,
        result_schema: schema_of::<SessionGoalReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/update",
        params_schema: schema_of::<SessionGoalUpdateParams>,
        result_schema: schema_of::<SessionGoalUpdateResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/message/edit",
        params_schema: schema_of::<SessionMessageEditParams>,
        result_schema: schema_of::<SessionMessageEditResult>,
        error_codes: &[
            codes::SESSION_NOT_FOUND,
            codes::INVALID_ITEM_SHAPE,
            codes::VERSION_CONFLICT,
        ],
        required_capability: None,
        idempotency: Idempotency::Key,
    },
    MethodSpec {
        name: "session/goal/pause",
        params_schema: schema_of::<SessionGoalTransitionParams>,
        result_schema: schema_of::<SessionGoalTransitionResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/resume",
        params_schema: schema_of::<SessionGoalTransitionParams>,
        result_schema: schema_of::<SessionGoalTransitionResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/complete",
        params_schema: schema_of::<SessionGoalTransitionParams>,
        result_schema: schema_of::<SessionGoalTransitionResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/cancel",
        params_schema: schema_of::<SessionGoalTransitionParams>,
        result_schema: schema_of::<SessionGoalTransitionResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/clear",
        params_schema: schema_of::<SessionGoalTransitionParams>,
        result_schema: schema_of::<SessionGoalClearResult>,
        error_codes: GOAL_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Task & agent ──
    MethodSpec {
        name: "task/list",
        params_schema: schema_of::<TaskListParams>,
        result_schema: schema_of::<TaskListResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "task/read",
        params_schema: schema_of::<TaskReadParams>,
        result_schema: schema_of::<TaskReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "task/write_stdin",
        params_schema: schema_of::<TaskWriteStdinParams>,
        result_schema: schema_of::<TaskWriteStdinResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "task/interrupt",
        params_schema: schema_of::<TaskInterruptParams>,
        result_schema: schema_of::<TaskInterruptResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "agent/list",
        params_schema: schema_of::<AgentListParams>,
        result_schema: schema_of::<AgentListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "agent/read",
        params_schema: schema_of::<AgentReadParams>,
        result_schema: schema_of::<AgentReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "agent/message",
        params_schema: schema_of::<AgentMessageParams>,
        result_schema: schema_of::<AgentMessageResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "agent/cancel",
        params_schema: schema_of::<AgentCancelParams>,
        result_schema: schema_of::<AgentCancelResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    // ── Security ──
    MethodSpec {
        name: "permission/profile/read",
        params_schema: schema_of::<PermissionProfileReadParams>,
        result_schema: schema_of::<PermissionProfileReadResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "permission/profile/update",
        params_schema: schema_of::<PermissionProfileUpdateParams>,
        result_schema: schema_of::<PermissionProfileUpdateResult>,
        error_codes: SESSION_ERRORS,
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "credential/list",
        params_schema: schema_of::<CredentialListParams>,
        result_schema: schema_of::<CredentialListResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "credential/set",
        params_schema: schema_of::<CredentialSetParams>,
        result_schema: schema_of::<CredentialSetResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "credential/delete",
        params_schema: schema_of::<CredentialDeleteParams>,
        result_schema: schema_of::<CredentialDeleteResult>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
];

/// Server -> Client reverse requests (01 §6). Params of the request itself
/// are the waiting-state item payload (`PendingControlRequest`); the schemas
/// here describe the client's answer.
pub static REVERSE_METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "approval/command/request",
        params_schema: schema_of::<Item>,
        result_schema: schema_of::<ApprovalRespondParams>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "approval/fileChange/request",
        params_schema: schema_of::<Item>,
        result_schema: schema_of::<ApprovalRespondParams>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "approval/permission/request",
        params_schema: schema_of::<Item>,
        result_schema: schema_of::<ApprovalRespondParams>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "userInput/request",
        params_schema: schema_of::<Item>,
        result_schema: schema_of::<UserInputRespondParams>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
    MethodSpec {
        name: "session/goal/completionApproval/request",
        params_schema: schema_of::<Item>,
        result_schema: schema_of::<ApprovalRespondParams>,
        error_codes: &[],
        required_capability: None,
        idempotency: Idempotency::None,
    },
];

pub fn method_names() -> Vec<&'static str> {
    NATIVE_METHODS.iter().map(|spec| spec.name).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn method_names_are_unique() {
        let names: HashSet<_> = NATIVE_METHODS.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), NATIVE_METHODS.len());
    }

    #[test]
    fn write_methods_declare_idempotency_explicitly() {
        for spec in NATIVE_METHODS {
            if spec.name == "session/interrupt"
                || spec.name == "subscription/ack"
                || spec.name == "session/queue/remove"
            {
                assert_eq!(spec.idempotency, Idempotency::None, "{}", spec.name);
            }
        }
        for name in [
            "session/new",
            "turn/start",
            "turn/steer",
            "session/queue/push",
            "session/goal/set",
        ] {
            let spec = NATIVE_METHODS
                .iter()
                .find(|spec| spec.name == name)
                .expect("registered");
            assert_eq!(spec.idempotency, Idempotency::Key, "{name}");
        }
    }

    #[test]
    fn every_method_produces_schemas() {
        for spec in NATIVE_METHODS.iter().chain(REVERSE_METHODS) {
            let _ = (spec.params_schema)();
            let _ = (spec.result_schema)();
        }
    }
}
