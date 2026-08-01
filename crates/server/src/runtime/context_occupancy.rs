//! Helpers for context-window occupancy snapshots.

use devo_core::RawContextBreakdown;
use devo_protocol::canonical::item::ContextCategoryId;
use devo_protocol::canonical::item::ContextOccupancy;

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
