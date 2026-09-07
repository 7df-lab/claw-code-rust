use async_trait::async_trait;
use devo_protocol::CollaborationMode;
use serde_json::json;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct PlanHandler {
    spec: ToolSpec,
}

impl Default for PlanHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "update_plan".into(),
                description: "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "explanation".to_string(),
                            JsonSchema::string(Some("Optional explanation for the plan update")),
                        ),
                        (
                            "plan".to_string(),
                            JsonSchema::array(
                                JsonSchema::object(
                                    std::collections::BTreeMap::from([
                                        (
                                            "step".to_string(),
                                            JsonSchema::string(Some(
                                                "Description of the plan step",
                                            )),
                                        ),
                                        (
                                            "status".to_string(),
                                            JsonSchema::string(Some("Status of the step")),
                                        ),
                                    ]),
                                    Some(vec!["step".to_string(), "status".to_string()]),
                                    None,
                                ),
                                Some("List of plan items"),
                            ),
                        ),
                    ]),
                    Some(vec!["plan".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for PlanHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        if ctx.collaboration_mode == CollaborationMode::Plan {
            return Err(ToolCallError::BlockedByMode("plan mode".to_string()));
        }

        let explanation = input
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let plan = input
            .get("plan")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'plan' field".into()))?;

        let in_progress_count = plan
            .iter()
            .filter(|item| {
                matches!(
                    item.get("status").and_then(|v| v.as_str()),
                    Some("in_progress" | "inProgress")
                )
            })
            .count();
        if in_progress_count > 1 {
            return Ok(ToolResult::error(
                ToolResultContent::Text("At most one step can be in_progress at a time.".into()),
                "Invalid plan",
                ToolCallError::InvalidInput("at most one step can be in_progress".into()),
            ));
        }

        let plan_text = serde_json::to_string_pretty(plan)
            .map_err(|e| ToolCallError::InternalError(e.to_string()))?;
        let content = if explanation.trim().is_empty() {
            plan_text
        } else {
            format!("{explanation}\n\n{plan_text}")
        };

        Ok(ToolResult::success(
            ToolResultContent::Mixed {
                text: Some(content),
                json: Some(json!({
                    "explanation": explanation,
                    "plan": plan,
                })),
            },
            "Plan updated",
        ))
    }
}
