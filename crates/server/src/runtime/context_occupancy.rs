//! Helpers for context-window occupancy snapshots.

use devo_core::RawContextBreakdown;
use devo_protocol::Model;
use devo_protocol::native::item::ContextCategoryId;
use devo_protocol::native::item::ContextOccupancy;

/// Resolve the applied compaction / occupancy limit for a model.
///
/// Global `compaction_token_limit` and session overrides are ignored: the only
/// user-facing limit is the model's usable window
/// (`context_window × effective_context_window_percent / 100`).
pub(crate) fn resolved_compaction_limit(_global: Option<u64>, model: &Model) -> u64 {
    let model_window = u64::from(model.context_window.max(1));
    u64::from(model.effective_context_window())
        .min(model_window)
        .max(1)
}

/// Window used for occupancy percent (bar denominator).
///
/// Always the model effective window. Session overrides and global compaction
/// preferences are not applied (product: one Context window, stored as ratio).
pub(crate) fn occupancy_window_tokens(
    _override_limit: Option<usize>,
    model: Option<&Model>,
    global_compaction_token_limit: Option<u64>,
) -> u64 {
    let Some(model) = model else {
        return 1;
    };
    resolved_compaction_limit(global_compaction_token_limit, model)
}

/// Apply an absolute compaction limit onto session token-budget fields.
pub(crate) fn apply_resolved_compaction_limit(config: &mut devo_core::SessionConfig, limit: usize) {
    // Keep override cleared so mid-turn / resume paths do not revive a stale
    // global threshold; the budget itself carries the applied model window.
    config.effective_context_window_override = None;
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

    fn sample_model(context_window: u32, percent: f64) -> Model {
        Model {
            context_window,
            effective_context_window_percent: Some(percent),
            ..Model::default()
        }
    }

    #[test]
    fn resolved_compaction_limit_ignores_global() {
        let model = sample_model(/*context_window*/ 200_000, /*percent*/ 95.0);
        assert_eq!(resolved_compaction_limit(Some(100_000), &model), 190_000);
    }

    #[test]
    fn resolved_compaction_limit_uses_model_effective() {
        let model = sample_model(/*context_window*/ 200_000, /*percent*/ 95.0);
        assert_eq!(resolved_compaction_limit(None, &model), 190_000);
    }

    #[test]
    fn occupancy_window_tokens_uses_model_effective_only() {
        let model = sample_model(/*context_window*/ 200_000, /*percent*/ 95.0);
        assert_eq!(
            occupancy_window_tokens(Some(100_000), Some(&model), Some(50_000)),
            190_000
        );
        assert_eq!(occupancy_window_tokens(None, Some(&model), None), 190_000);
        assert_ne!(occupancy_window_tokens(None, Some(&model), None), 200_000);
    }

    #[test]
    fn apply_resolved_compaction_limit_updates_budget_clears_override() {
        let mut config = devo_core::SessionConfig {
            effective_context_window_override: Some(50_000),
            ..Default::default()
        };
        apply_resolved_compaction_limit(&mut config, 250_000);
        assert_eq!(config.effective_context_window_override, None);
        assert_eq!(config.token_budget.context_window, 250_000);
        assert_eq!(config.token_budget.auto_compact_token_limit, Some(250_000));
    }
}
