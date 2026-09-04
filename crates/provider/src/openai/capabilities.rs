use devo_protocol::ModelProfileKey;
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
    pub require_reasoning_content: bool,
}

impl OpenAIRequestProfile {
    const fn new(
        reasoning_mode: OpenAIReasoningMode,
        supported_roles: &'static [RequestRole],
        supports_temperature: bool,
        supports_top_p: bool,
        supports_top_k: bool,
        require_reasoning_content: bool,
    ) -> Self {
        Self {
            reasoning_mode,
            supported_roles,
            supports_temperature,
            supports_top_p,
            supports_top_k,
            require_reasoning_content,
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
        let model = model.rsplit_once('/').map_or(model, |(_, model)| model);
        match self {
            // Capability resolution uses the catalog slug, not the provider's
            // configurable wire model name. Avoid lowercasing the whole slug
            // for every provider request.
            ModelMatcher::Prefix(value) => model
                .get(..value.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(value)),
            ModelMatcher::Contains(value) => model.to_ascii_lowercase().contains(value),
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

const ROLES_WITHOUT_DEVELOPER: &[RequestRole] = &[
    RequestRole::System,
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

const OPENAI_PROFILE_RULES: &[ProfileRule] = &[
    ProfileRule {
        matcher: ModelMatcher::Prefix("glm-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Thinking,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("deepseek-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::ThinkingWithEffort,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("kimi-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Thinking,
            ROLES_WITHOUT_DEVELOPER,
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
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("qwen"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
];

/// Resolves the wire profile for an OpenAI-family model on the given transport.
pub(crate) fn resolve_request_profile(
    model_profile: &ModelProfileKey,
    transport: OpenAITransport,
) -> OpenAIRequestProfile {
    for rule in OPENAI_PROFILE_RULES {
        if rule.transport == transport
            && matches!(model_profile, ModelProfileKey::CatalogSlug(model) if rule.matcher.matches(model))
        {
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
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
        assert!(profile.supports_top_k);
        assert!(profile.require_reasoning_content);
    }

    #[test]
    fn resolve_request_profile_uses_generic_profile_without_catalog_slug() {
        let profile =
            resolve_request_profile(&ModelProfileKey::Generic, OpenAITransport::ChatCompletions);
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }

    #[test]
    fn resolve_request_profile_matches_prefix_case_insensitively() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("GLM-4.5".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
    }

    #[test]
    fn resolve_request_profile_matches_provider_catalog_slugs() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("deepseek/deepseek-v4-flash".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(
            profile.reasoning_mode,
            OpenAIReasoningMode::ThinkingWithEffort
        );
    }

    #[test]
    fn resolve_request_profile_matches_qwen3_models() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("ollama/qwen3:8b".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }

    #[test]
    fn resolve_request_profile_matches_kimi_models() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("kimi/kimi-k2.6".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
    }

    #[test]
    fn resolve_request_profile_defaults_to_effort_for_responses() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            OpenAITransport::Responses,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }
}
