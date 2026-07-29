//! Handlers for the paged history read methods of the new Native API:
//! `session/turns/list` and `session/items/list` (01 §4.2, 07).
//!
//! The in-memory runtime model does not retain turn records or item
//! envelopes, so both methods read the session's rollout file through
//! `devo_core::read_canonical_history` (dual-format, fail-closed). That also
//! makes cold sessions work without a resume: resolving the rollout path is
//! enough.

use std::path::PathBuf;

use devo_core::read_canonical_history;
use devo_protocol::canonical::item::ItemEnvelope;
use devo_protocol::canonical::page::{Page, PageParams};
use devo_protocol::canonical::rpc_session::{SessionItemsListParams, SessionTurnsListParams};

use super::super::*;

/// Default page size for the history read methods.
const DEFAULT_PAGE_LIMIT: u32 = 50;
/// Maximum page size; larger requests are clamped, not rejected (01 §4.2).
const MAX_PAGE_LIMIT: u32 = 200;

impl ServerRuntime {
    pub(crate) async fn handle_session_turns_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionTurnsListParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/turns/list params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        let page = match paginate(&history.turns, &params.page, |turn| u64::from(turn.sequence))
        {
            Ok(page) => page,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: page,
        })
        .expect("serialize session/turns/list response")
    }

    pub(crate) async fn handle_session_items_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionItemsListParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/items/list params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        let items: Vec<ItemEnvelope> = match &params.turn_id {
            Some(turn_id) => history
                .items
                .into_iter()
                .filter(|item| item.turn_id == *turn_id)
                .collect(),
            None => history.items,
        };
        let page = match paginate(&items, &params.page, |item| item.seq) {
            Ok(page) => page,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: page,
        })
        .expect("serialize session/items/list response")
    }

    /// Resolves the session's rollout path and reads its canonical history.
    /// The error variant is a ready-made JSON-RPC error response (session
    /// not found, or a damaged/unreadable file).
    async fn load_canonical_history(
        &self,
        request_id: &serde_json::Value,
        session_id: devo_protocol::canonical::ids::SessionId,
    ) -> Result<devo_core::CanonicalHistory, serde_json::Value> {
        let Some(rollout_path) = self.resolve_rollout_path(&session_id).await else {
            return Err(self.error_response(
                request_id.clone(),
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            ));
        };
        read_canonical_history(&rollout_path).map_err(|error| {
            self.error_response(
                request_id.clone(),
                ProtocolErrorCode::InternalError,
                format!("failed to read session history: {error}"),
            )
        })
    }

    /// Finds the rollout file for a session, loaded or cold: a resumed/live
    /// session knows its path; otherwise the SQLite index, then the
    /// file-name scan. Ephemeral sessions have no persisted history and
    /// resolve to `None` (reported as not found — they have no history to
    /// page).
    async fn resolve_rollout_path(
        &self,
        session_id: &devo_protocol::canonical::ids::SessionId,
    ) -> Option<PathBuf> {
        let legacy_id = SessionId::try_from(session_id.as_str()).ok()?;
        if let Some(handle) = self.session(legacy_id).await
            && let Some(record) = handle.record().await.flatten()
        {
            return Some(record.rollout_path);
        }
        if let Ok(Some(index)) = self.deps.db.get_session_index(&legacy_id)
            && let Some(path) = index.rollout_path
        {
            return Some(path);
        }
        self.rollout_store
            .find_rollout_by_session_id(&legacy_id)
            .ok()
            .flatten()
    }
}

/// Slices `items` (ascending by position) into one page.
///
/// Cursor encoding: the decimal position of the previous page's last item
/// (`sequence` for turns, `seq` for items); clients must treat it as
/// opaque. `nextCursor` is the last returned item's position and is present
/// iff more data remains. The limit defaults to 50 and clamps into
/// `1..=200` — out-of-range limits never error (01 §4.2).
fn paginate<T: Clone>(
    items: &[T],
    params: &PageParams,
    position: impl Fn(&T) -> u64,
) -> Result<Page<T>, String> {
    let after = match &params.cursor {
        Some(cursor) => cursor
            .parse::<u64>()
            .map_err(|_| "malformed cursor".to_string())?,
        None => 0,
    };
    let limit = params
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT) as usize;
    let data: Vec<T> = items
        .iter()
        .filter(|item| position(item) > after)
        .take(limit)
        .cloned()
        .collect();
    let last_position = data.last().map(&position).unwrap_or(after);
    let next_cursor = items
        .iter()
        .any(|item| position(item) > last_position)
        .then(|| last_position.to_string());
    Ok(Page { data, next_cursor })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn params(cursor: Option<&str>, limit: Option<u32>) -> PageParams {
        PageParams {
            cursor: cursor.map(str::to_owned),
            limit,
        }
    }

    #[test]
    fn paginate_walks_all_pages_without_gaps_or_duplicates() {
        let items: Vec<u64> = (1..=5).collect();
        let first = paginate(&items, &params(None, Some(2)), |item| *item).expect("page 1");
        assert_eq!(first.data, vec![1, 2]);
        assert_eq!(first.next_cursor.as_deref(), Some("2"));
        let second = paginate(&items, &params(Some("2"), Some(2)), |item| *item).expect("page 2");
        assert_eq!(second.data, vec![3, 4]);
        assert_eq!(second.next_cursor.as_deref(), Some("4"));
        let third = paginate(&items, &params(Some("4"), Some(2)), |item| *item).expect("page 3");
        assert_eq!(
            third,
            Page {
                data: vec![5],
                next_cursor: None,
            }
        );
    }

    #[test]
    fn paginate_defaults_and_clamps_the_limit() {
        let items: Vec<u64> = (1..=250).collect();
        let defaulted = paginate(&items, &params(None, None), |item| *item).expect("default");
        assert_eq!(defaulted.data.len(), 50);
        let clamped = paginate(&items, &params(None, Some(1000)), |item| *item).expect("clamp");
        assert_eq!(clamped.data.len(), 200);
        assert_eq!(clamped.next_cursor.as_deref(), Some("200"));
        let zero = paginate(&items, &params(None, Some(0)), |item| *item).expect("zero");
        assert_eq!(zero.data.len(), 1);
    }

    #[test]
    fn paginate_rejects_malformed_cursor() {
        let error = paginate(&[1u64], &params(Some("not-a-cursor"), None), |item| *item)
            .expect_err("must fail");
        assert_eq!(error, "malformed cursor");
    }

    #[test]
    fn paginate_empty_input_has_no_cursor() {
        let page = paginate(&Vec::<u64>::new(), &params(None, None), |item| *item).expect("empty");
        assert_eq!(
            page,
            Page {
                data: Vec::new(),
                next_cursor: None,
            }
        );
    }
}
