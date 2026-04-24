use devo_protocol::RequestRole;
use devo_protocol::ToolDefinition;
use serde_json::Value;
use serde_json::json;
use tracing::warn;

use super::OpenAIReasoningEffort;
use super::OpenAIRole;
use super::capabilities::OpenAIReasoningMode;
use super::capabilities::OpenAIRequestProfile;

pub(crate) fn request_role(role: &str) -> OpenAIRole {
    match role.parse::<RequestRole>() {
        Ok(RequestRole::System) => OpenAIRole::System,
        Ok(RequestRole::Developer) => OpenAIRole::Developer,
        Ok(RequestRole::User) => OpenAIRole::User,
        Ok(RequestRole::Assistant) => OpenAIRole::Assistant,
        Ok(RequestRole::Tool) => OpenAIRole::Tool,
        Ok(RequestRole::Function) => OpenAIRole::Function,
        Err(_) => {
            warn!(
                role = role,
                fallback = "user",
                "unknown OpenAI request role; defaulting to user"
            );
            OpenAIRole::User
        }
    }
}

pub(crate) fn reasoning_effort(thinking: Option<&str>) -> Option<OpenAIReasoningEffort> {
    let thinking = thinking?.trim().to_ascii_lowercase();
    match thinking.as_str() {
        "none" | "disabled" => Some(OpenAIReasoningEffort::None),
        "minimal" => Some(OpenAIReasoningEffort::Minimal),
        "low" => Some(OpenAIReasoningEffort::Low),
        "medium" | "enabled" | "" => Some(OpenAIReasoningEffort::Medium),
        "high" => Some(OpenAIReasoningEffort::High),
        "xhigh" => Some(OpenAIReasoningEffort::XHigh),
        "max" => Some(OpenAIReasoningEffort::Max),
        _ => None,
    }
}

pub(crate) enum OpenAIReasoningValue {
    Effort(OpenAIReasoningEffort),
    Thinking { enabled: bool },
}

pub(crate) fn reasoning_value(
    profile: OpenAIRequestProfile,
    thinking: Option<&str>,
) -> Option<OpenAIReasoningValue> {
    match profile.reasoning_mode {
        OpenAIReasoningMode::Effort => reasoning_effort(thinking).map(OpenAIReasoningValue::Effort),
        OpenAIReasoningMode::Thinking => {
            let enabled = !matches!(
                thinking
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "disabled" | "none"
            );
            Some(OpenAIReasoningValue::Thinking { enabled })
        }
    }
}

pub(crate) fn tool_definitions(tools: &[ToolDefinition]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect(),
    )
}
