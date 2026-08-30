use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::SamplingControls;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratedTitleError {
    NoTextContent,
    EmptyTextContent,
    InvalidLength,
}

impl GeneratedTitleError {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            GeneratedTitleError::NoTextContent => "no_text_content",
            GeneratedTitleError::EmptyTextContent => "empty_text_content",
            GeneratedTitleError::InvalidLength => "invalid_length",
        }
    }
}

/// Builds a non-tool model request used to generate one final session title.
pub(crate) fn build_title_generation_request(
    model_slug: String,
    model: String,
    user_input: &str,
) -> ModelRequest {
    ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::CatalogSlug(model_slug),
        model,
        system: Some(
            "Generate a short session title. Respond with only the title. Match the language of the first user message exactly — do not translate. Prefer 3 to 8 words (or a similarly short phrase in that language). Use sentence case when the language has case. No markdown, no quotes, no trailing punctuation unless required by a proper noun.".to_string(),
        ),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: format!(
                    "First user message:\n{user_input}\n\nReturn only the best concise title in the same language as the message above."
                ),
            }],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls { temperature: None, top_p: None, top_k: None },
        request_thinking: Some("disabled".to_string()),
        reasoning_effort: None,
        extra_body: None,
    }
}

/// Extracts and normalizes one title candidate from a complete provider response.
pub(crate) fn normalize_generated_title(
    content: &[ResponseContent],
) -> Result<String, GeneratedTitleError> {
    let mut saw_text = false;
    for block in content {
        let ResponseContent::Text(text) = block else {
            continue;
        };
        saw_text = true;
        for line in text.lines() {
            let candidate = normalize_generated_title_line(line);
            if candidate.is_empty() {
                continue;
            }
            let visible = candidate.chars().count();
            if !(3..=80).contains(&visible) {
                return Err(GeneratedTitleError::InvalidLength);
            }
            return Ok(candidate);
        }
    }

    if saw_text {
        Err(GeneratedTitleError::EmptyTextContent)
    } else {
        Err(GeneratedTitleError::NoTextContent)
    }
}

fn normalize_generated_title_line(line: &str) -> String {
    let line = trim_title_wrappers(line.trim());
    let line = strip_generated_title_prefix(line);
    let line = trim_title_wrappers(line);
    if line.is_empty() {
        return String::new();
    }
    let collapsed = collapse_whitespace(line);
    let without_trailing = collapsed
        .trim_end_matches(['.', '!', '?', ':', ';'])
        .to_string();
    let without_wrappers = trim_title_wrappers(without_trailing.trim());
    sentence_case(without_wrappers)
}

fn trim_title_wrappers(input: &str) -> &str {
    input.trim_matches(|ch| matches!(ch, '"' | '\'' | '#' | '`' | '*' | '_' | ' '))
}

fn strip_generated_title_prefix(input: &str) -> &str {
    let trimmed = input.trim();
    for prefix in [
        "session title:",
        "session title -",
        "generated title:",
        "generated title -",
        "short title:",
        "short title -",
        "title:",
        "title -",
    ] {
        if trimmed
            .as_bytes()
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix.as_bytes()))
        {
            return trimmed[prefix.len()..].trim();
        }
    }
    trimmed
}

fn collapse_whitespace(input: &str) -> String {
    let mut words = input.split_whitespace();
    let Some(first) = words.next() else {
        return String::new();
    };

    let mut output = String::from(first);
    for word in words {
        output.push(' ');
        output.push_str(word);
    }
    output
}

fn sentence_case(input: &str) -> String {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), chars.as_str())
}

#[cfg(test)]
mod tests {
    use devo_protocol::ResponseContent;
    use pretty_assertions::assert_eq;

    use super::GeneratedTitleError;
    use super::normalize_generated_title;

    #[test]
    fn normalizes_generated_title_text() {
        assert_eq!(
            normalize_generated_title(&[ResponseContent::Text(
                "\"rollout persistence follow up.\"\nextra".to_string()
            )]),
            Ok("Rollout persistence follow up".to_string())
        );
    }

    #[test]
    fn skips_blank_generated_title_lines() {
        assert_eq!(
            normalize_generated_title(&[ResponseContent::Text(
                "\n\nTitle: restore token stats".to_string()
            )]),
            Ok("Restore token stats".to_string())
        );
    }

    #[test]
    fn strips_common_generated_title_wrappers() {
        assert_eq!(
            normalize_generated_title(&[ResponseContent::Text(
                "**Session title:** `quiet CLI logs`;".to_string()
            )]),
            Ok("Quiet CLI logs".to_string())
        );
    }

    #[test]
    fn rejects_tool_only_generated_title_response() {
        assert_eq!(
            normalize_generated_title(&[ResponseContent::ToolUse {
                id: "call_1".to_string(),
                name: "noop".to_string(),
                input: serde_json::json!({})
            }]),
            Err(GeneratedTitleError::NoTextContent)
        );
    }
}
