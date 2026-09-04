use std::fmt::Write as _;

use devo_core::tools::ToolPermissionRequest;
use devo_protocol::{
    ModelRequest, RequestContent, RequestMessage, ResponseContent, SamplingControls,
};

const REVIEWER_MAX_TOKENS: usize = 512;
const REVIEWER_JSON_SHAPE: &str = "{\"risk\":\"low|medium|high\",\"rationale\":\"short reason\"}";
const REVIEWER_SYSTEM_PROMPT: &str = "Rate the risk of the pending tool call. Reply with JSON only. Writing outside the workspace or running a command is not automatically high; judge from the conversation. Mark high only for clearly destructive or irreversible actions such as `rm -rf *`.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewerRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewerAssessment {
    pub risk: ReviewerRisk,
    pub rationale: String,
}

impl ReviewerRisk {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(crate) fn allows_without_user(self) -> bool {
        matches!(self, Self::Low | Self::Medium)
    }
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
        system: Some(format!(
            "{REVIEWER_SYSTEM_PROMPT} JSON shape: {REVIEWER_JSON_SHAPE}."
        )),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: review_prompt_for_request(request, context),
            }],
        }],
        max_tokens: REVIEWER_MAX_TOKENS,
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

/// Appends a review-only tail onto the in-flight turn request so the reviewer
/// shares that call's prompt prefix (system, tools, and leading messages).
pub(crate) fn extend_approval_review_request(
    mut prefix: ModelRequest,
    request: &ToolPermissionRequest,
    context: &ApprovalReviewContext,
) -> ModelRequest {
    prefix.messages.push(RequestMessage {
        role: "user".to_string(),
        content: vec![RequestContent::Text {
            text: review_suffix_for_cached_prefix(request, context),
        }],
    });
    prefix.max_tokens = REVIEWER_MAX_TOKENS;
    prefix.request_thinking = Some("disabled".to_string());
    prefix.reasoning_effort = None;
    prefix
}

pub(crate) fn parse_reviewer_decision(content: &[ResponseContent]) -> Option<ReviewerAssessment> {
    let mut combined = String::new();
    for block in content {
        if let ResponseContent::Text(text) = block {
            combined.push_str(text);
            combined.push('\n');
        }
    }
    parse_reviewer_text(&combined)
}

fn parse_reviewer_text(raw: &str) -> Option<ReviewerAssessment> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    assessment_from_json_text(trimmed)
        .or_else(|| extract_json_object(trimmed).and_then(assessment_from_json_text))
}

fn assessment_from_json_text(raw: &str) -> Option<ReviewerAssessment> {
    let value = serde_json::from_str(raw)
        .or_else(|_| jsonrepair::loads(raw, &jsonrepair::Options::default()))
        .ok()?;
    assessment_from_value(&value)
}

fn assessment_from_value(value: &serde_json::Value) -> Option<ReviewerAssessment> {
    let rationale = value
        .get("rationale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let risk = match value
        .get("risk")
        .and_then(serde_json::Value::as_str)?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "low" => ReviewerRisk::Low,
        "medium" => ReviewerRisk::Medium,
        "high" => ReviewerRisk::High,
        _ => return None,
    };
    Some(ReviewerAssessment { risk, rationale })
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        let index = start + offset;
        let ch = *byte as char;
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn review_prompt_for_request(
    request: &ToolPermissionRequest,
    context: &ApprovalReviewContext,
) -> String {
    let mut prompt = String::with_capacity(1024);
    append_section(
        &mut prompt,
        "Permission profile",
        context.profile_summary.as_deref(),
    );
    append_section(
        &mut prompt,
        "Project rules (AGENTS.md)",
        context.agents_rules.as_deref(),
    );
    append_list_section(&mut prompt, "Recent transcript", &context.transcript_tail);
    append_list_section(
        &mut prompt,
        "Recent approval decisions",
        &context.recent_decisions,
    );
    append_tool_approval_request(&mut prompt, request);
    prompt
}

fn review_suffix_for_cached_prefix(
    request: &ToolPermissionRequest,
    context: &ApprovalReviewContext,
) -> String {
    let mut prompt = String::with_capacity(1024);
    prompt.push_str(devo_core::APPROVAL_REVIEW_PROMPT.trim_end());
    prompt.push_str("\n\n");
    append_section(
        &mut prompt,
        "Permission profile",
        context.profile_summary.as_deref(),
    );
    append_tool_approval_request(&mut prompt, request);
    prompt
}

fn append_section(prompt: &mut String, heading: &str, body: Option<&str>) {
    let Some(body) = body else {
        return;
    };
    write!(prompt, "## {heading}\n{body}\n\n").expect("writing to a String cannot fail");
}

fn append_list_section(prompt: &mut String, heading: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    writeln!(prompt, "## {heading}").expect("writing to a String cannot fail");
    for line in items {
        prompt.push_str(line);
        prompt.push('\n');
    }
    prompt.push('\n');
}

fn append_tool_approval_request(prompt: &mut String, request: &ToolPermissionRequest) {
    prompt.push_str("## Tool approval request\n");
    write!(prompt, "tool_name: {}", request.tool_name).expect("writing to a String cannot fail");
    write!(prompt, "\nresource: {:?}", request.resource).expect("writing to a String cannot fail");
    write!(
        prompt,
        "\nsandbox_permissions: {:?}",
        request.sandbox_permissions
    )
    .expect("writing to a String cannot fail");
    write!(prompt, "\ncwd: {}", request.cwd.display()).expect("writing to a String cannot fail");
    write!(prompt, "\naction_summary: {}", request.action_summary)
        .expect("writing to a String cannot fail");
    if let Some(justification) = &request.justification {
        write!(prompt, "\njustification: {justification}")
            .expect("writing to a String cannot fail");
    }
    if let Some(path) = &request.path {
        write!(prompt, "\npath: {}", path.display()).expect("writing to a String cannot fail");
    }
    if let Some(host) = &request.host {
        write!(prompt, "\nhost: {host}").expect("writing to a String cannot fail");
    }
    if let Some(target) = &request.target {
        write!(prompt, "\ntarget: {target}").expect("writing to a String cannot fail");
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
    write!(prompt, "\ninput_json: {}", request.input).expect("writing to a String cannot fail");
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_approval_reviewer_json_risk() {
        assert_eq!(
            parse_reviewer_text(r#"{"risk":"low","rationale":"scoped command"}"#),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Low,
                rationale: "scoped command".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text(r#"{"risk":"MEDIUM","rationale":"network fetch"}"#),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Medium,
                rationale: "network fetch".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text(r#"{"risk":"high","rationale":"writes outside workspace"}"#),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::High,
                rationale: "writes outside workspace".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text(r#"{"decision":"approve","rationale":"legacy"}"#),
            None
        );
        assert_eq!(
            parse_reviewer_text(r#"{"risk":"approve","rationale":"wrong label"}"#),
            None
        );
        assert_eq!(
            parse_reviewer_text(
                "Risk assessment:\n{\"risk\":\"low\",\"rationale\":\"user asked to write on the desktop\"}\n"
            ),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Low,
                rationale: "user asked to write on the desktop".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text("```json\n{\"risk\": \"low\", \"rationale\": \"fenced\",}\n```"),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Low,
                rationale: "fenced".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text("{risk: 'medium', rationale: 'unquoted keys'}"),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Medium,
                rationale: "unquoted keys".to_string(),
            })
        );
        assert_eq!(
            parse_reviewer_text("{\"risk\":\"low\",\"rationale\":\"truncated\""),
            Some(ReviewerAssessment {
                risk: ReviewerRisk::Low,
                rationale: "truncated".to_string(),
            })
        );
        assert_eq!(parse_reviewer_text("this is low risk but not json"), None);
    }

    #[test]
    fn low_and_medium_risk_allow_without_user() {
        assert!(ReviewerRisk::Low.allows_without_user());
        assert!(ReviewerRisk::Medium.allows_without_user());
        assert!(!ReviewerRisk::High.allows_without_user());
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

    #[test]
    fn extend_review_request_keeps_turn_prefix() {
        use devo_protocol::ModelProfileKey;
        use devo_protocol::ToolDefinition;

        let prefix = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("session-model".to_string()),
            model: "provider-model".to_string(),
            system: Some("locked session system".to_string()),
            messages: vec![
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "environment prefix".to_string(),
                    }],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "hello".to_string(),
                    }],
                },
            ],
            max_tokens: 4096,
            tools: Some(vec![ToolDefinition {
                name: "mutating_tool".to_string(),
                description: "Mutates test state.".to_string(),
                input_schema: json!({}),
                output_schema: None,
            }]),
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(0.7),
                ..SamplingControls::default()
            },
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: Some(json!({"keep": true})),
        };
        let request = ToolPermissionRequest {
            tool_call_id: "call".to_string(),
            tool_name: "mutating_tool".to_string(),
            input: json!({}),
            cwd: std::path::PathBuf::from("repo"),
            session_id: "session".to_string(),
            turn_id: Some("turn".to_string()),
            resource: devo_safety::ResourceKind::FileWrite,
            action_summary: "Write a file".to_string(),
            justification: None,
            path: None,
            host: None,
            target: None,
            command_prefix: None,
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: devo_core::tools::SandboxPermissionRequest::Default,
        };
        let context = ApprovalReviewContext {
            profile_summary: Some("preset: auto_review".to_string()),
            agents_rules: Some("- duplicated in prefix".to_string()),
            transcript_tail: vec!["user: hello".to_string()],
            recent_decisions: vec!["allow_once mutating_tool".to_string()],
        };

        let extended = extend_approval_review_request(prefix.clone(), &request, &context);
        assert_eq!(
            extended.model_slug,
            ModelProfileKey::CatalogSlug("session-model".to_string())
        );
        assert_eq!(extended.model, "provider-model");
        assert_eq!(extended.system.as_deref(), Some("locked session system"));
        assert_eq!(extended.sampling.temperature, Some(0.7));
        assert_eq!(extended.request_thinking.as_deref(), Some("disabled"));
        assert_eq!(extended.reasoning_effort, None);
        assert_eq!(extended.extra_body, Some(json!({"keep": true})));
        assert_eq!(extended.max_tokens, 512);
        assert_eq!(
            extended.tools.as_ref().map(|tools| tools[0].name.as_str()),
            Some("mutating_tool")
        );
        assert_eq!(extended.messages.len(), 3);
        let RequestContent::Text { text: prefix_text } = &extended.messages[0].content[0] else {
            panic!("prefix message should remain text");
        };
        assert_eq!(prefix_text, "environment prefix");
        let RequestContent::Text { text: tail } = &extended.messages[2].content[0] else {
            panic!("review tail should be text");
        };
        assert!(tail.contains("Do not call tools"));
        assert!(tail.contains("\"risk\":\"low|medium|high\""));
        assert!(tail.contains("Do not mark **high** only because"));
        assert!(tail.contains("`rm -rf *`"));
        assert!(!tail.contains("You are Devo"));
        assert!(tail.contains("## Permission profile"));
        assert!(tail.contains("preset: auto_review"));
        assert!(
            !tail.contains("## Recent approval decisions"),
            "prior approvals already live in the turn prefix"
        );
        assert!(!tail.contains("allow_once mutating_tool"));
        assert!(tail.contains("## Tool approval request"));
        assert!(
            !tail.contains("## Project rules"),
            "AGENTS.md already lives in the turn prefix"
        );
        assert!(
            !tail.contains("## Recent transcript"),
            "transcript already lives in the turn prefix"
        );
    }
}
