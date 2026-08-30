//! Canonical session restore (`session/resume` + items/turns list).

use anyhow::Result;

use devo_core::PermissionPreset;
use devo_core::SessionId;
use devo_server::StdioServerClient;

use crate::events::WorkerEvent;

use super::history;

/// Result of restoring a session through canonical APIs (resume + items
/// list + queue list), replacing the legacy `session/resume` aggregate
/// result (L2-DES-APP-008 Phase C).
pub(crate) struct NativeSessionRestore {
    pub(crate) session: devo_protocol::native::session::Session,
    pub(crate) history_items: Vec<devo_protocol::SessionHistoryItem>,
    pub(crate) pending_texts: Vec<String>,
    pub(crate) last_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    pub(crate) last_query_total_tokens: usize,
    pub(crate) last_query_input_tokens: usize,
}

pub(crate) fn native_session_id(session_id: SessionId) -> devo_protocol::native::ids::SessionId {
    devo_protocol::native::ids::SessionId::from_string(session_id.to_string())
}

pub(crate) async fn restore_session_native(
    client: &mut StdioServerClient,
    session_id: SessionId,
) -> Result<NativeSessionRestore> {
    let resumed = client.session_resume_native(session_id).await?;
    let fallback_mode = resumed
        .session
        .settings
        .mode
        .as_deref()
        .and_then(|mode| serde_json::from_value(serde_json::Value::String(mode.to_string())).ok())
        .unwrap_or_default();

    let mut turns = Vec::new();
    let mut cursor = None;
    loop {
        let page = client
            .session_turns_list_native(session_id, cursor.clone(), Some(200))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        turns.extend(page.data);
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }

    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = client
            .session_items_list_native(session_id, cursor.clone(), Some(500))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        items.extend(page.data);
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    let (turn_query_total, turn_query_input) = resume_query_tokens_from_turns(&turns);
    let history_items = history::restored_history_items(turns, items, fallback_mode);

    let queue = client
        .session_queue_list(devo_protocol::native::rpc_turn::SessionQueueListParams {
            session_id: native_session_id(session_id),
        })
        .await?;
    let pending_texts = queue
        .entries
        .iter()
        .map(|entry| entry.preview.clone())
        .collect();

    Ok(NativeSessionRestore {
        session: resumed.session,
        history_items,
        pending_texts,
        last_context_occupancy: resumed.last_context_occupancy,
        last_query_total_tokens: resumed
            .last_query_total_tokens
            .map(|tokens| tokens as usize)
            .filter(|tokens| *tokens > 0)
            .unwrap_or(turn_query_total),
        last_query_input_tokens: turn_query_input,
    })
}

fn resume_query_tokens_from_turns(turns: &[devo_protocol::native::turn::Turn]) -> (usize, usize) {
    for turn in turns.iter().rev() {
        let Some(usage) = turn.usage.as_ref() else {
            continue;
        };
        let query = &usage.query;
        let total = if query.total_tokens > 0 {
            query.total_tokens as usize
        } else {
            (query.input_tokens + query.output_tokens) as usize
        };
        if total > 0 || query.input_tokens > 0 {
            return (total, query.input_tokens as usize);
        }
    }
    (0, 0)
}

/// Builds the `SessionSwitched` event from a canonical restore. Mapping
/// notes: `last_query_total_tokens` and `prompt_token_estimate` both seed
/// from session cumulative input usage until a live query usage event arrives.
pub(crate) fn session_switched_event_from_restore(
    session_id: SessionId,
    restore: &NativeSessionRestore,
) -> WorkerEvent {
    let session = &restore.session;
    let active_agent_label = session.parent.as_ref().map(|parent| {
        let label = match parent {
            devo_protocol::native::session::SessionParent::Agent { role, .. } => {
                role.clone().unwrap_or_else(|| "subagent".to_string())
            }
        };
        format!("Agent: {label}")
    });
    let total_usage = &session.usage.total;
    let last_query_total_tokens = restore.last_query_total_tokens;
    let last_query_input_tokens = restore.last_query_input_tokens;
    let prompt_token_estimate = last_query_input_tokens.max(total_usage.input_tokens as usize);
    let legacy_session_id = session_id;
    WorkerEvent::SessionSwitched {
        session_id: legacy_session_id.to_string(),
        cwd: session.cwd.clone(),
        title: session.title.clone(),
        model: Some(session.model.model.clone()),
        model_binding_id: (session.model.provider != "unknown")
            .then(|| session.model.provider.clone()),
        reasoning_effort_selection: session
            .settings
            .reasoning_effort
            .map(|effort| effort.to_string()),
        reasoning_effort: session.settings.reasoning_effort,
        active_agent_label,
        total_input_tokens: total_usage.input_tokens as usize,
        total_output_tokens: total_usage.output_tokens as usize,
        total_tokens: total_usage.total_tokens as usize,
        total_cache_read_tokens: total_usage.cache_read_input_tokens as usize,
        last_query_total_tokens,
        last_query_input_tokens,
        prompt_token_estimate,
        history_items: history::project_history_items(&restore.history_items),
        rich_history_items: restore.history_items.clone(),
        loaded_item_count: restore.history_items.len() as u64,
        pending_texts: restore.pending_texts.clone(),
        collaboration_mode: session
            .settings
            .mode
            .as_deref()
            .and_then(|mode| {
                serde_json::from_value(serde_json::Value::String(mode.to_string())).ok()
            })
            .unwrap_or_default(),
        permission_preset: Some(match session.settings.permission_profile {
            devo_protocol::native::model::PermissionProfile::Default => PermissionPreset::Default,
            devo_protocol::native::model::PermissionProfile::AutoReview => {
                PermissionPreset::AutoReview
            }
            devo_protocol::native::model::PermissionProfile::FullAccess => {
                PermissionPreset::FullAccess
            }
        }),
        effective_context_window: session.settings.effective_context_window,
        last_context_occupancy: restore.last_context_occupancy.clone(),
    }
}
