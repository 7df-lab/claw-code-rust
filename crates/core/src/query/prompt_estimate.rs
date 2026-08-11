//! Prompt token estimation for assembled model requests.

use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::approx_tokens_from_byte_count;

const MCP_TOOL_PREFIX: &str = "mcp__";

/// Byte-heuristic category split for an assembled request (before provider
/// usage scaling).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawContextBreakdown {
    pub base: u64,
    pub skills: u64,
    pub tools_builtin: u64,
    pub tools_mcp: u64,
    pub conversation: u64,
}

impl RawContextBreakdown {
    pub fn total(self) -> u64 {
        self.base
            .saturating_add(self.skills)
            .saturating_add(self.tools_builtin)
            .saturating_add(self.tools_mcp)
            .saturating_add(self.conversation)
    }
}

/// Classify assembled request bytes into occupancy categories.
pub(crate) fn estimate_request_context_breakdown(request: &ModelRequest) -> RawContextBreakdown {
    let mut breakdown = RawContextBreakdown::default();

    if let Some(system) = request.system.as_ref() {
        breakdown.base = breakdown
            .base
            .saturating_add(approx_tokens_from_byte_count(system.len()));
    }

    for message in &request.messages {
        let bytes = serde_json::to_string(message).map_or(0, |json| json.len());
        let tokens = approx_tokens_from_byte_count(bytes);
        match classify_message(message) {
            MessageCategory::Base => breakdown.base = breakdown.base.saturating_add(tokens),
            MessageCategory::Skills => breakdown.skills = breakdown.skills.saturating_add(tokens),
            MessageCategory::Conversation => {
                breakdown.conversation = breakdown.conversation.saturating_add(tokens);
            }
        }
    }

    if let Some(tools) = request.tools.as_ref() {
        for tool in tools {
            let bytes = serde_json::to_string(tool).map_or(0, |json| json.len());
            let tokens = approx_tokens_from_byte_count(bytes);
            if tool.name.starts_with(MCP_TOOL_PREFIX) {
                breakdown.tools_mcp = breakdown.tools_mcp.saturating_add(tokens);
            } else {
                breakdown.tools_builtin = breakdown.tools_builtin.saturating_add(tokens);
            }
        }
    }

    let hosted_bytes = serde_json::to_string(&request.hosted_tools).map_or(0, |json| json.len());
    breakdown.tools_builtin = breakdown
        .tools_builtin
        .saturating_add(approx_tokens_from_byte_count(hosted_bytes));

    breakdown
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageCategory {
    Base,
    Skills,
    Conversation,
}

fn classify_message(message: &RequestMessage) -> MessageCategory {
    for content in &message.content {
        match content {
            RequestContent::Text { text } => {
                if let Some(category) = classify_text(text) {
                    return category;
                }
            }
            RequestContent::ToolResult { content, .. } => {
                if let Some(category) = classify_text(content) {
                    return category;
                }
            }
            RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolUse { .. } => {}
        }
    }
    MessageCategory::Conversation
}

fn classify_text(text: &str) -> Option<MessageCategory> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("<available_skills>")
        || trimmed.starts_with("<skill ")
        || trimmed.starts_with("<skill>")
        || trimmed.starts_with("<skill_content")
    {
        return Some(MessageCategory::Skills);
    }
    if trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<language_preference>")
        || trimmed.starts_with("<context_changes>")
        || trimmed.starts_with("<user_instructions_updates>")
        || trimmed.starts_with("<user_instructions>")
        || trimmed.starts_with("# AGENTS.md instructions for ")
    {
        return Some(MessageCategory::Base);
    }
    None
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::HostedToolDefinition;
    use devo_protocol::ModelProfileKey;
    use devo_protocol::SamplingControls;
    use devo_protocol::ToolDefinition;

    fn request_with(
        system: Option<&str>,
        messages: Vec<RequestMessage>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> ModelRequest {
        ModelRequest {
            model_slug: ModelProfileKey::Generic,
            model: "test".into(),
            system: system.map(str::to_string),
            messages,
            max_tokens: 128,
            tools,
            hosted_tools: Vec::<HostedToolDefinition>::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        }
    }

    fn text_message(role: &str, text: &str) -> RequestMessage {
        RequestMessage {
            role: role.into(),
            content: vec![RequestContent::Text { text: text.into() }],
        }
    }

    #[test]
    fn breakdown_classifies_system_skills_tools_and_conversation() {
        let request = request_with(
            Some("you are a coding agent"),
            vec![
                text_message("user", "<available_skills>\nskills\n</available_skills>"),
                text_message(
                    "user",
                    "<environment_context>\n  <cwd>/</cwd>\n</environment_context>",
                ),
                text_message("user", "please fix the bug"),
                text_message("assistant", "looking into it"),
            ],
            Some(vec![
                ToolDefinition {
                    name: "bash".into(),
                    description: "run shell".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    output_schema: None,
                },
                ToolDefinition {
                    name: "mcp__docs__search".into(),
                    description: "search docs".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    output_schema: None,
                },
            ]),
        );

        let breakdown = estimate_request_context_breakdown(&request);
        assert!(breakdown.base > 0);
        assert!(breakdown.skills > 0);
        assert!(breakdown.tools_builtin > 0);
        assert!(breakdown.tools_mcp > 0);
        assert!(breakdown.conversation > 0);
        assert!(breakdown.total() > 0);
    }

    #[test]
    fn agents_md_instructions_count_as_base() {
        let request = request_with(
            None,
            vec![text_message(
                "user",
                "# AGENTS.md instructions for /repo\n<INSTRUCTIONS>\nrules\n</INSTRUCTIONS>",
            )],
            None,
        );
        let breakdown = estimate_request_context_breakdown(&request);
        assert!(breakdown.base > 0);
        assert_eq!(breakdown.skills, 0);
        assert_eq!(breakdown.conversation, 0);
    }
}
