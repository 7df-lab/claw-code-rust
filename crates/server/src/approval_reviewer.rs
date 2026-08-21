use std::fmt::Write as _;

use devo_core::tools::ToolPermissionRequest;
use devo_protocol::{
    ModelRequest, RequestContent, RequestMessage, ResponseContent, SamplingControls,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewerDecision {
    Approve { rationale: String },
    Deny { rationale: String },
    Uncertain { rationale: String },
}

/// Additional context provided to the auto-approval reviewer so it can make
/// stateful decisions instead of judging each request in isolation.
#[derive(Debug, Clone, Default)]
pub(crate) struct ApprovalReviewContext {
    /// Human-readable summary of the active permission profile.
    pub profile_summary: Option<String>,
    /// Rendered AGENTS.md / project rules relevant to the request.
    pub agents_rules: Option<String>,
    /// Recent transcript items (oldest to newest).
    pub transcript_tail: Vec<String>,
    /// Recent approval decisions already granted in this session.
    pub recent_decisions: Vec<String>,
}

pub(crate) fn build_approval_review_request(
    model: String,
    request: &ToolPermissionRequest,
    context: &ApprovalReviewContext,
) -> ModelRequest {
    ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model,
        system: Some(
            "You are Devo's automatic approval reviewer. Decide whether a tool approval request is safe given the user's active permission profile, project rules, conversation transcript, and prior approval decisions. Respond with exactly one compact JSON object and no markdown: {\"decision\":\"approve|deny|uncertain\",\"rationale\":\"short reason\"}. Approve only when the action is clearly low risk, consistent with prior approvals, and scoped to the stated target. Deny destructive, credential, privilege escalation, or ambiguous high-impact actions. Use uncertain when more context or user intent is needed."
                .to_string(),
        ),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: review_prompt_for_request(request, context),
            }],
        }],
        max_tokens: 128,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: Some(0.0),
            ..SamplingControls::default()
        },
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

pub(crate) fn parse_reviewer_decision(content: &[ResponseContent]) -> Option<ReviewerDecision> {
    let raw = content.iter().find_map(|block| match block {
        ResponseContent::Text(text) => Some(text.as_str()),
        ResponseContent::ToolUse { .. }
        | ResponseContent::HostedToolUse { .. }
        | ResponseContent::ProviderReasoning { .. } => None,
    })?;
    parse_reviewer_text(raw)
}

fn parse_reviewer_text(raw: &str) -> Option<ReviewerDecision> {
    let trimmed = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let rationale = value
        .get("rationale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    match value.get("decision").and_then(serde_json::Value::as_str)? {
        "approve" => Some(ReviewerDecision::Approve { rationale }),
        "deny" => Some(ReviewerDecision::Deny { rationale }),
        "uncertain" => Some(ReviewerDecision::Uncertain { rationale }),
        _ => None,
    }
}

fn review_prompt_for_request(
    request: &ToolPermissionRequest,
    context: &ApprovalReviewContext,
) -> String {
    let mut prompt = String::with_capacity(1024);

    if let Some(profile) = &context.profile_summary {
        prompt.push_str("## Permission profile\n");
        prompt.push_str(profile);
        prompt.push_str("\n\n");
    }

    if let Some(rules) = &context.agents_rules {
        prompt.push_str("## Project rules (AGENTS.md)\n");
        prompt.push_str(rules);
        prompt.push_str("\n\n");
    }

    if !context.transcript_tail.is_empty() {
        prompt.push_str("## Recent transcript\n");
        for line in &context.transcript_tail {
            prompt.push_str(line);
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    if !context.recent_decisions.is_empty() {
        prompt.push_str("## Recent approval decisions\n");
        for line in &context.recent_decisions {
            prompt.push_str(line);
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    prompt.push_str("## Tool approval request\n");
    write!(&mut prompt, "tool_name: {}", request.tool_name)
        .expect("writing to a String cannot fail");
    write!(&mut prompt, "\nresource: {:?}", request.resource)
        .expect("writing to a String cannot fail");
    write!(
        &mut prompt,
        "\nsandbox_permissions: {:?}",
        request.sandbox_permissions
    )
    .expect("writing to a String cannot fail");
    write!(&mut prompt, "\ncwd: {}", request.cwd.display())
        .expect("writing to a String cannot fail");
    write!(&mut prompt, "\naction_summary: {}", request.action_summary)
        .expect("writing to a String cannot fail");
    if let Some(justification) = &request.justification {
        write!(&mut prompt, "\njustification: {justification}")
            .expect("writing to a String cannot fail");
    }
    if let Some(path) = &request.path {
        write!(&mut prompt, "\npath: {}", path.display()).expect("writing to a String cannot fail");
    }
    if let Some(host) = &request.host {
        write!(&mut prompt, "\nhost: {host}").expect("writing to a String cannot fail");
    }
    if let Some(target) = &request.target {
        write!(&mut prompt, "\ntarget: {target}").expect("writing to a String cannot fail");
    }
    if let Some(command_prefix) = &request.command_prefix {
        prompt.push_str("\ncommand_prefix: ");
        let mut tokens = command_prefix.iter();
        if let Some(first) = tokens.next() {
            prompt.push_str(first);
            for token in tokens {
                prompt.push(' ');
                prompt.push_str(token);
            }
        }
    }
    write!(&mut prompt, "\ninput_json: {}", request.input)
        .expect("writing to a String cannot fail");
    prompt
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_approval_reviewer_json_decision() {
        assert_eq!(
            parse_reviewer_text(r#"{"decision":"approve","rationale":"scoped command"}"#),
            Some(ReviewerDecision::Approve {
                rationale: "scoped command".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text(r#"{"decision":"deny","rationale":"destructive"}"#),
            Some(ReviewerDecision::Deny {
                rationale: "destructive".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text(r#"{"decision":"uncertain","rationale":"needs user"}"#),
            Some(ReviewerDecision::Uncertain {
                rationale: "needs user".to_string(),
            })
        );
    }

    #[test]
    fn builds_review_prompt_with_command_prefix() {
        let request = ToolPermissionRequest {
            tool_call_id: "call".to_string(),
            tool_name: "shell_command".to_string(),
            input: json!({ "command": "git add -A" }),
            cwd: std::path::PathBuf::from("repo"),
            session_id: "session".to_string(),
            turn_id: Some("turn".to_string()),
            resource: devo_safety::ResourceKind::ShellExec,
            action_summary: "Run git add -A".to_string(),
            justification: Some("stage files".to_string()),
            path: None,
            host: None,
            target: Some("git add -A".to_string()),
            command_prefix: Some(vec!["git".to_string(), "add".to_string()]),
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: devo_core::tools::SandboxPermissionRequest::Default,
        };

        let model_request = build_approval_review_request(
            "model".to_string(),
            &request,
            &ApprovalReviewContext::default(),
        );
        let RequestContent::Text { text } = &model_request.messages[0].content[0] else {
            panic!("review request should contain text content");
        };
        assert_eq!(
            text,
            "## Tool approval request\ntool_name: shell_command\nresource: ShellExec\nsandbox_permissions: Default\ncwd: repo\naction_summary: Run git add -A\njustification: stage files\ntarget: git add -A\ncommand_prefix: git add\ninput_json: {\"command\":\"git add -A\"}"
        );
    }

    #[test]
    fn builds_review_prompt_with_context() {
        let request = ToolPermissionRequest {
            tool_call_id: "call".to_string(),
            tool_name: "shell_command".to_string(),
            input: json!({ "command": "rm -rf build/" }),
            cwd: std::path::PathBuf::from("repo"),
            session_id: "session".to_string(),
            turn_id: Some("turn".to_string()),
            resource: devo_safety::ResourceKind::ShellExec,
            action_summary: "Remove build directory".to_string(),
            justification: None,
            path: None,
            host: None,
            target: Some("rm -rf build/".to_string()),
            command_prefix: None,
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: devo_core::tools::SandboxPermissionRequest::Default,
        };
        let context = ApprovalReviewContext {
            profile_summary: Some("preset: default; writable: /workspace".to_string()),
            agents_rules: Some("- Never delete build artifacts".to_string()),
            transcript_tail: vec!["user: clean the build".to_string()],
            recent_decisions: vec!["allow_once shell_command: ls".to_string()],
        };

        let model_request = build_approval_review_request("model".to_string(), &request, &context);
        let RequestContent::Text { text } = &model_request.messages[0].content[0] else {
            panic!("review request should contain text content");
        };
        assert!(text.contains("## Permission profile"));
        assert!(text.contains("preset: default; writable: /workspace"));
        assert!(text.contains("## Project rules (AGENTS.md)"));
        assert!(text.contains("- Never delete build artifacts"));
        assert!(text.contains("## Recent transcript"));
        assert!(text.contains("user: clean the build"));
        assert!(text.contains("## Recent approval decisions"));
        assert!(text.contains("allow_once shell_command: ls"));
        assert!(text.contains("## Tool approval request"));
    }
}
