//! Helpers for context-window occupancy snapshots.

use devo_core::RawContextBreakdown;
use devo_protocol::Model;
use devo_protocol::canonical::item::ContextCategoryId;
use devo_protocol::canonical::item::ContextOccupancy;

/// Resolve the applied compaction / effective-context limit for a model.
///
/// When a global `compaction_token_limit` is set, clamp it to the model's hard
/// `context_window`. Otherwise use the model's effective context window.
pub(crate) fn resolved_compaction_limit(global: Option<u64>, model: &Model) -> u64 {
    let model_window = u64::from(model.context_window.max(1));
    let model_effective = u64::from(model.effective_context_window())
        .min(model_window)
        .max(1);
    match global.filter(|limit| *limit > 0) {
        Some(limit) => limit.min(model_window).max(1),
        None => model_effective,
    }
}

/// Apply an absolute compaction limit onto session token-budget fields.
pub(crate) fn apply_resolved_compaction_limit(config: &mut devo_core::SessionConfig, limit: usize) {
    config.effective_context_window_override = Some(limit);
    config.token_budget.context_window = limit;
    config.token_budget.auto_compact_token_limit = Some(limit);
}

pub(crate) fn occupancy_from_raw(
    context_window_tokens: u64,
    raw: RawContextBreakdown,
    anchor_total: u64,
) -> ContextOccupancy {
    ContextOccupancy::scale_raw_to_total(
        context_window_tokens,
        anchor_total,
        raw.base,
        raw.skills,
        raw.tools_builtin,
        raw.tools_mcp,
        raw.conversation,
    )
}

pub(crate) fn occupancy_after_compaction(
    context_window_tokens: u64,
    previous: Option<&ContextOccupancy>,
    conversation_tokens: u64,
    fallback_raw: Option<RawContextBreakdown>,
) -> ContextOccupancy {
    let (base, skills, tools_builtin, tools_mcp) = if let Some(previous) = previous {
        (
            previous.tokens_for(ContextCategoryId::Base),
            previous.tokens_for(ContextCategoryId::Skills),
            previous.tokens_for(ContextCategoryId::ToolsBuiltin),
            previous.tokens_for(ContextCategoryId::ToolsMcp),
        )
    } else if let Some(raw) = fallback_raw {
        (raw.base, raw.skills, raw.tools_builtin, raw.tools_mcp)
    } else {
        (0, 0, 0, 0)
    };
    ContextOccupancy::from_category_tokens(
        context_window_tokens,
        base,
        skills,
        tools_builtin,
        tools_mcp,
        conversation_tokens,
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn sample_model(context_window: u32, percent: u8) -> Model {
        Model {
            slug: "test-model".to_string(),
            context_window,
            effective_context_window_percent: Some(percent),
            ..Model::default()
        }
    }

    #[test]
    fn resolved_compaction_limit_uses_model_effective_when_global_unset() {
        let model = sample_model(/*context_window*/ 200_000, /*percent*/ 95);
        assert_eq!(resolved_compaction_limit(/*global*/ None, &model), 190_000);
    }

    #[test]
    fn resolved_compaction_limit_clamps_global_to_model_window() {
        let model = sample_model(/*context_window*/ 200_000, /*percent*/ 95);
        assert_eq!(resolved_compaction_limit(Some(250_000), &model), 200_000);
        assert_eq!(resolved_compaction_limit(Some(100_000), &model), 100_000);
    }

    #[test]
    fn apply_resolved_compaction_limit_updates_token_budget() {
        let mut config = devo_core::SessionConfig::default();
        apply_resolved_compaction_limit(&mut config, /*limit*/ 250_000);
        assert_eq!(config.effective_context_window_override, Some(250_000));
        assert_eq!(config.token_budget.context_window, 250_000);
        assert_eq!(config.token_budget.auto_compact_token_limit, Some(250_000));
    }
}
