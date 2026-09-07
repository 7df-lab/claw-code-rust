use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolPreparationFeedback, ToolSpec};

pub struct GoalUpdateHandler {
    spec: ToolSpec,
}

impl Default for GoalUpdateHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalUpdateHandler {
    pub fn new() -> Self {
        Self {
            spec: goal_update_spec(),
        }
    }
}

pub fn goal_update_spec() -> ToolSpec {
    ToolSpec {
        name: "update_goal".to_string(),
        description:
            "Mark the active persistent goal complete after verifying that the full objective is achieved."
                .to_string(),
        input_schema: goal_update_schema(),
        output_mode: ToolOutputMode::StructuredJson,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn goal_update_schema() -> JsonSchema {
    let status_schema = JsonSchema {
        enum_values: Some(vec![json!("complete")]),
        ..JsonSchema::string(Some("Goal status update. Only 'complete' is accepted."))
    };
    JsonSchema::object(
        BTreeMap::from([("status".to_string(), status_schema)]),
        Some(vec!["status".to_string()]),
        Some(false),
    )
}

#[async_trait]
impl ToolHandler for GoalUpdateHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let status = input
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'status' field".to_string()))?;
        if status != "complete" {
            return Err(ToolCallError::InvalidInput(
                "update_goal only accepts status='complete'".to_string(),
            ));
        }

        let coordinator = ctx.agent_coordinator.ok_or_else(|| {
            ToolCallError::NeedsConfiguration(
                "update_goal requires a server runtime coordinator".to_string(),
            )
        })?;
        let result = Arc::clone(&coordinator)
            .update_goal(ctx.session_id.clone(), status.to_string())
            .await?;

        let tokens_used = result
            .get("tokens_used")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();
        let time_used_seconds = result
            .get("time_used_seconds")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();

        Ok(ToolResult::success(
            ToolResultContent::Mixed {
                text: Some(format!(
                    "Goal marked complete. Final usage: {tokens_used} tokens, {time_used_seconds} seconds."
                )),
                json: Some(result),
            },
            "Goal updated",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn schema_only_allows_complete_status() {
        // Trace: L2-DES-GOAL-001
        let schema = goal_update_schema();
        let status_schema = schema
            .properties
            .as_ref()
            .expect("properties")
            .get("status")
            .expect("status schema");

        assert_eq!(status_schema.enum_values, Some(vec![json!("complete")]));
    }

    #[tokio::test]
    async fn rejects_non_complete_status() {
        // Trace: L2-DES-GOAL-001
        let handler = GoalUpdateHandler::new();
        let ctx = ToolContext {
            output_store: None,
            tool_call_id: crate::invocation::ToolCallId("call-1".into()),
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            workspace_root: std::env::temp_dir(),
            budgets: crate::contracts::ToolBudgets {
                output_limit_bytes: 1024,
                wall_time_limit_ms: None,
            },
            cancel_token: tokio_util::sync::CancellationToken::new(),
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_permission_overlay: None,
            sandbox_profile: None,
        };

        let error = handler
            .handle(ctx, json!({ "status": "paused" }), None)
            .await
            .expect_err("non-complete status should fail");

        assert_eq!(
            error.to_string(),
            "invalid input: update_goal only accepts status='complete'"
        );
    }
}
