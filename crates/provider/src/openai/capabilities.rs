use devo_protocol::RequestRole;

/// Transport variants used to resolve OpenAI-family capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAITransport {
    ChatCompletions,
    Responses,
}

/// How a model expects reasoning controls to be encoded on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAIReasoningMode {
    /// `reasoning_effort` / `reasoning.effort`.
    Effort,
    /// OpenAI-compatible `thinking` object with `enabled` / `disabled`.
    Thinking,
    /// Models that require both a thinking toggle and an effort field.
    ThinkingWithEffort,
}

/// Capability profile for an OpenAI-family model on a specific transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenAIRequestProfile {
    pub reasoning_mode: OpenAIReasoningMode,
    pub supported_roles: &'static [RequestRole],
    pub supports_temperature: bool,
    pub supports_top_p: bool,
    pub supports_top_k: bool,
    pub supports_reasoning_content: bool,
}

impl OpenAIRequestProfile {
    const fn new(
        reasoning_mode: OpenAIReasoningMode,
        supported_roles: &'static [RequestRole],
        supports_temperature: bool,
        supports_top_p: bool,
        supports_top_k: bool,
        supports_reasoning_content: bool,
    ) -> Self {
        Self {
            reasoning_mode,
            supported_roles,
            supports_temperature,
            supports_top_p,
            supports_top_k,
            supports_reasoning_content,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelMatcher {
    Prefix(&'static str),
    #[allow(dead_code)]
    Contains(&'static str),
}

impl ModelMatcher {
    fn matches(self, model: &str) -> bool {
        let lowered = model.to_ascii_lowercase();
        match self {
            ModelMatcher::Prefix(value) => lowered.starts_with(value),
            ModelMatcher::Contains(value) => lowered.contains(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProfileRule {
    matcher: ModelMatcher,
    transport: OpenAITransport,
    profile: OpenAIRequestProfile,
}

const DEFAULT_ROLES: &[RequestRole] = &[
    RequestRole::System,
    RequestRole::Developer,
    RequestRole::User,
    RequestRole::Assistant,
    RequestRole::Tool,
    RequestRole::Function,
];

const RESPONSES_ROLES: &[RequestRole] = &[
    RequestRole::System,
    RequestRole::Developer,
    RequestRole::User,
    RequestRole::Assistant,
    RequestRole::Tool,
    RequestRole::Function,
];

const DEFAULT_CHAT_COMPLETIONS: OpenAIRequestProfile = OpenAIRequestProfile::new(
    OpenAIReasoningMode::Effort,
    DEFAULT_ROLES,
    true,
    true,
    false,
    false,
);

const DEFAULT_RESPONSES: OpenAIRequestProfile = OpenAIRequestProfile::new(
    OpenAIReasoningMode::Effort,
    RESPONSES_ROLES,
    true,
    true,
    false,
    false,
);

const ZAI_CHAT_COMPLETIONS: OpenAIRequestProfile = OpenAIRequestProfile::new(
    OpenAIReasoningMode::Thinking,
    DEFAULT_ROLES,
    true,
    true,
    true,
    true,
);

const OPENAI_PROFILE_RULES: &[ProfileRule] = &[
    ProfileRule {
        matcher: ModelMatcher::Prefix("glm-"),
        transport: OpenAITransport::ChatCompletions,
        profile: ZAI_CHAT_COMPLETIONS,
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("deepseek-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::ThinkingWithEffort,
            DEFAULT_ROLES,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("minimax-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            DEFAULT_ROLES,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("qwen-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            DEFAULT_ROLES,
            true,
            true,
            true,
            true,
        ),
    },
];

/// Resolves the wire profile for an OpenAI-family model on the given transport.
pub(crate) fn resolve_request_profile(
    model: &str,
    transport: OpenAITransport,
) -> OpenAIRequestProfile {
    for rule in OPENAI_PROFILE_RULES {
        if rule.transport == transport && rule.matcher.matches(model) {
            return rule.profile;
        }
    }

    match transport {
        OpenAITransport::ChatCompletions => DEFAULT_CHAT_COMPLETIONS,
        OpenAITransport::Responses => DEFAULT_RESPONSES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn resolve_request_profile_uses_zai_thinking_for_chat_completions() {
        let profile = resolve_request_profile("glm-4.5", OpenAITransport::ChatCompletions);
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
        assert!(profile.supports_top_k);
        assert!(profile.supports_reasoning_content);
    }

    #[test]
    fn resolve_request_profile_defaults_to_effort_for_responses() {
        let profile = resolve_request_profile("glm-4.5", OpenAITransport::Responses);
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }
}
