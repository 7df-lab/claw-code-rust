//! Handlers for the `session/queue/*` API (devo-api-design/01 §4.3).
//!
//! Queue entries are pre-items: editable, not yet in history, addressed by a
//! stable `queueItemId`. The in-memory `pending_turn_queue` is the source of
//! truth (mirrored in SQLite `pending_messages`); ops serialize per session
//! on the queue mutex, last-write-wins.

use std::collections::VecDeque;

use devo_core::{
    CollaborationMode, InputItem, PendingInputId, PendingInputItem, PendingInputKind,
    TurnExecutionMode,
};
use devo_protocol::canonical::event::ServerNotification;
use devo_protocol::canonical::ids::{
    ItemId as CanonicalItemId, QueueItemId, SessionId as CanonicalSessionId,
    TurnId as CanonicalTurnId,
};
use devo_protocol::canonical::item::UserInput;
use devo_protocol::canonical::model::ModelBinding;
use devo_protocol::canonical::queue::{QueueChange, QueueEntry};
use devo_protocol::canonical::rpc_turn::{
    SessionQueueListResult, SessionQueuePushParams, SessionQueuePushResult,
    SessionQueueRemoveResult, SessionQueueSteerParams, SessionQueueSteerResult,
    SessionQueueUpdateParams, SessionQueueUpdateResult,
};
use devo_protocol::canonical::turn::{
    Turn as CanonicalTurn, TurnKind as CanonicalTurnKind, TurnStatus as CanonicalTurnStatus,
};
use uuid::Uuid;

use super::super::*;

impl ServerRuntime {
    pub(crate) async fn handle_session_queue_push(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionQueuePushParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/push params: {error}"),
                );
            }
        };
        let input_items = match legacy_input_items(&params.input) {
            Ok(items) => items,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        if input_items.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "queue push input is empty",
            );
        }
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };

        // Idle vs busy is decided by the exact turn/start path: it starts a
        // new turn when the session is idle and queues otherwise (the same
        // operation turn/start uses today, so the two entry points can
        // never disagree).
        let response = self
            .handle_turn_start_for_connection(
                Some(connection_id),
                request_id.clone(),
                serde_json::to_value(TurnStartParams {
                    session_id: legacy_session_id,
                    input: input_items,
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: CollaborationMode::default(),
                    execution_mode: TurnExecutionMode::default(),
                })
                .expect("serialize turn/start params"),
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        match response["result"]["disposition"].as_str() {
            Some("started") => {
                let turn = self
                    .active_canonical_turn(legacy_session_id)
                    .await
                    .expect("a turn just started");
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: SessionQueuePushResult::Started {
                        turn: Box::new(turn),
                    },
                })
                .expect("serialize session/queue/push response")
            }
            Some("queued") => {
                let queued_id = response["result"]["queued_input_id"]
                    .as_str()
                    .expect("queued result carries queued_input_id")
                    .to_owned();
                // The dedup key rides on the queued pre-item so a later
                // materialization can collapse retries.
                if let Some(client_user_message_id) = &params.client_user_message_id {
                    self.attach_queue_metadata(
                        legacy_session_id,
                        &queued_id,
                        client_user_message_id,
                    )
                    .await;
                }
                let position = self
                    .session_turn_reservation_snapshot(legacy_session_id)
                    .await
                    .map(|reservation| {
                        // turn/start enqueues into the shared queue
                        // synchronously (appending to the back), so the
                        // entry's position is the current length.
                        reservation
                            .pending_turn_queue
                            .lock()
                            .expect("pending turn queue mutex should not be poisoned")
                            .len() as u32
                    })
                    .unwrap_or(1);
                // turn/start enqueues into the shared queue synchronously
                // now, but the response entry is still built from the
                // accepted input directly (session/queue/list reflects the
                // queue truth immediately after). `enqueued_at` is
                // approximate.
                let entry = QueueEntry {
                    queue_item_id: QueueItemId::from_legacy_uuid(
                        Uuid::parse_str(&queued_id).expect("queued_input_id is a uuid"),
                    ),
                    position,
                    input: params.input.clone(),
                    preview: params
                        .input
                        .iter()
                        .find_map(|part| match part {
                            UserInput::Text { text } => Some(
                                text.lines()
                                    .next()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(80)
                                    .collect(),
                            ),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    enqueued_at: chrono::Utc::now(),
                };
                self.broadcast_queue_updated(
                    legacy_session_id,
                    QueueChange::Added,
                    entry.queue_item_id.clone(),
                    None,
                )
                .await;
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: SessionQueuePushResult::Queued {
                        entry: Box::new(entry),
                    },
                })
                .expect("serialize session/queue/push response")
            }
            _ => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "unexpected turn/start outcome for queue push",
            ),
        }
    }

    pub(crate) async fn handle_session_queue_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::canonical::rpc_turn::SessionQueueListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/queue/list params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let entries = canonical_queue_entries(
            &reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned"),
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueListResult { entries },
        })
        .expect("serialize session/queue/list response")
    }

    pub(crate) async fn handle_session_queue_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionQueueUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/update params: {error}"),
                );
            }
        };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let queue_item_uuid = match Uuid::parse_str(params.queue_item_id.as_str()) {
            Ok(uuid) => uuid,
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "invalid queueItemId",
                );
            }
        };
        let pending_id = PendingInputId::from(queue_item_uuid);

        // Resolve the replacement input up front (skill resolution can
        // fail before any state changes).
        let new_kind = match &params.input {
            Some(input) => {
                let input_items = match legacy_input_items(input) {
                    Ok(items) => items,
                    Err(message) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            message,
                        );
                    }
                };
                if input_items.is_empty() {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::EmptyInput,
                        "queue update input is empty",
                    );
                }
                let workspace_root = reservation.summary.cwd.clone();
                let resolved = match reservation
                    .runtime_context
                    .resolve_input_items(&input_items, Some(workspace_root.as_path()))
                {
                    Ok(Some(resolved)) => resolved,
                    Ok(None) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::EmptyInput,
                            "queue update input is empty",
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("failed to resolve queue update input: {error}"),
                        );
                    }
                };
                let display_text =
                    super::super::items::render_input_items(&input_items).unwrap_or_default();
                Some(PendingInputKind::UserInput {
                    input: input_items,
                    display_text,
                    prompt_text: resolved.prompt_text,
                    prompt_messages: resolved.prompt_messages,
                })
            }
            None => None,
        };

        let entry = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue.iter().position(|item| item.id == pending_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            if let Some(kind) = new_kind {
                queue[index].kind = kind;
            }
            if let Some(position) = params.position {
                let item = queue.remove(index).expect("index just validated");
                let target = (position.saturating_sub(1) as usize).min(queue.len());
                queue.insert(target, item);
            }
            canonical_queue_entries(&queue)
                .into_iter()
                .find(|entry| entry.queue_item_id == params.queue_item_id)
                .expect("entry just updated")
        };

        if !reservation.ephemeral {
            let ordered: Vec<PendingInputItem> = {
                reservation
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .iter()
                    .cloned()
                    .collect()
            };
            let updated = ordered
                .iter()
                .find(|item| item.id == pending_id)
                .expect("entry just updated");
            if let Err(error) =
                self.deps
                    .db
                    .update_pending_content(&legacy_session_id, QueueType::Turn, updated)
            {
                tracing::warn!(
                    session_id = %legacy_session_id,
                    error = %error,
                    "failed to persist queue entry update"
                );
            }
            if params.position.is_some() {
                let ordered_ids: Vec<PendingInputId> = ordered.iter().map(|item| item.id).collect();
                if let Err(error) = self.deps.db.set_pending_positions(
                    &legacy_session_id,
                    QueueType::Turn,
                    &ordered_ids,
                ) {
                    tracing::warn!(
                        session_id = %legacy_session_id,
                        error = %error,
                        "failed to persist queue reorder"
                    );
                }
            }
        }

        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Updated,
            entry.queue_item_id.clone(),
            None,
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueUpdateResult { entry },
        })
        .expect("serialize session/queue/update response")
    }

    pub(crate) async fn handle_session_queue_remove(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::canonical::rpc_turn::SessionQueueRemoveParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/queue/remove params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };
        let queue_item_uuid = match Uuid::parse_str(params.queue_item_id.as_str()) {
            Ok(uuid) => uuid,
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "invalid queueItemId",
                );
            }
        };
        let pending_id = PendingInputId::from(queue_item_uuid);
        // Remove directly through the shared queue, not the actor mailbox:
        // the actor loop is busy for the whole duration of a running turn
        // (`ExecuteTurn` is inline in the actor), so a mailbox round-trip
        // would block the RPC until the turn ends. The queue mutex is the
        // per-session serialization point for queue ops (01 §4.3).
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let removed = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue.iter().position(|item| item.id == pending_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            queue.remove(index).is_some()
        };
        if !removed {
            return self.error_response(
                request_id,
                ProtocolErrorCode::QueueItemNotFound,
                "queue item is no longer queued",
            );
        }
        if !reservation.ephemeral
            && let Err(error) =
                self.deps
                    .db
                    .remove_pending_by_id(&legacy_session_id, QueueType::Turn, &pending_id)
        {
            tracing::warn!(
                session_id = %legacy_session_id,
                error = %error,
                "failed to remove queue entry from database"
            );
        }
        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Removed,
            params.queue_item_id.clone(),
            None,
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueRemoveResult {},
        })
        .expect("serialize session/queue/remove response")
    }

    pub(crate) async fn handle_session_queue_steer(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionQueueSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/steer params: {error}"),
                );
            }
        };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };
        let queue_item_uuid = match Uuid::parse_str(params.queue_item_id.as_str()) {
            Ok(uuid) => uuid,
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "invalid queueItemId",
                );
            }
        };
        let pending_id = PendingInputId::from(queue_item_uuid);
        let Some(reservation) = self
            .session_turn_reservation_snapshot(legacy_session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(active_turn) = reservation.active_turn.as_ref() else {
            // The race-safe outcome: the turn is over, the entry simply
            // stays queued — the message is never lost.
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "turn already ended; the entry remains queued",
            );
        };
        if active_turn.turn_id.to_string() != params.expected_turn_id.as_str() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.kind != devo_core::TurnKind::Regular {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }
        let turn_id = active_turn.turn_id;

        let (display_input, item) = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue.iter().position(|item| item.id == pending_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            let display_input = match &queue[index].kind {
                PendingInputKind::UserText { text } => text.clone(),
                PendingInputKind::UserInput { display_text, .. } => display_text.clone(),
                _ => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "queued input cannot be steered",
                    );
                }
            };
            let item = queue.remove(index).expect("index just validated");
            (display_input, item)
        };

        reservation
            .steer_input_queue
            .lock()
            .expect("steer input queue mutex should not be poisoned")
            .push_back(item.clone());
        if !reservation.ephemeral {
            if let Err(error) =
                self.deps
                    .db
                    .remove_pending_by_id(&legacy_session_id, QueueType::Turn, &pending_id)
            {
                tracing::warn!(
                    session_id = %legacy_session_id,
                    error = %error,
                    "failed to remove promoted entry from database"
                );
            }
            if let Err(error) =
                self.deps
                    .db
                    .push_pending(&legacy_session_id, QueueType::Steer, &item)
            {
                tracing::warn!(
                    session_id = %legacy_session_id,
                    error = %error,
                    "failed to persist promoted entry to database"
                );
            }
        }

        // Materialize the user message with entry=steer (the legacy
        // SteerInput payload carries that through the projectors).
        let (item_id, item_seq) = self
            .start_item(
                legacy_session_id,
                turn_id,
                ItemKind::UserMessage,
                serde_json::json!({ "title": "You", "text": display_input.clone() }),
            )
            .await;
        self.complete_item(
            legacy_session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::UserMessage,
            TurnItem::SteerInput(TextItem {
                text: display_input.clone(),
            }),
            serde_json::json!({ "title": "You", "text": display_input }),
        )
        .await;

        self.broadcast_queue_updated(
            legacy_session_id,
            QueueChange::Promoted,
            params.queue_item_id.clone(),
            None,
        )
        .await;
        self.emit_to_connection(
            connection_id,
            "serverRequest/resolved",
            ServerEvent::ServerRequestResolved(ServerRequestResolvedPayload {
                session_id: legacy_session_id,
                request_id: "queued-steer-accepted".into(),
                turn_id: Some(turn_id),
            }),
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionQueueSteerResult {
                item_id: CanonicalItemId::from_legacy_uuid(Uuid::from(item_id)),
            },
        })
        .expect("serialize session/queue/steer response")
    }

    /// Broadcasts one canonical `queue/updated` notification to connections
    /// subscribed to this session via the new subscription API.
    pub(crate) async fn broadcast_queue_updated(
        &self,
        session_id: SessionId,
        change: QueueChange,
        queue_item_id: QueueItemId,
        started_turn_id: Option<CanonicalTurnId>,
    ) {
        let session_id_string = session_id.to_string();
        let entries = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .map(|reservation| {
                canonical_queue_entries(
                    &reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned"),
                )
            })
            .unwrap_or_default();
        let notification = ServerNotification::QueueUpdated {
            session_id: CanonicalSessionId::from_string(session_id_string.clone()),
            change,
            queue_item_id,
            started_turn_id,
            queue: entries,
        };
        let params = serde_json::to_value(&notification)
            .expect("serialize queue/updated notification")
            .get("params")
            .cloned()
            .unwrap_or_default();
        let mut connections = self.connections.lock().await;
        for (connection_id, connection) in connections.iter_mut() {
            let subscribed = connection.event_selectors.iter().any(|selector| {
                matches!(
                    selector,
                    devo_protocol::canonical::event::StreamSelector::Session { session_id }
                        if session_id.as_str() == session_id_string
                )
            });
            if !subscribed {
                continue;
            }
            let event_seq = connection.next_seq();
            let frame = super::super::outbound::OutboundFrame::notification(
                *connection_id,
                "queue/updated".to_string(),
                event_seq,
                params.clone(),
            );
            let _ = super::super::outbound::enqueue_outbound_notification(
                &connection.outbound_tx,
                frame,
                super::super::outbound::OutboundDeliveryPolicy::Reliable,
                "connection_notifications",
            )
            .await;
        }
    }

    /// Stores the domain-level dedup key on a freshly queued entry
    /// (`clientUserMessageId`, 01 §4.3). Db-only for now: the in-memory
    /// pre-item is handed to the actor asynchronously, so its metadata can
    /// only be merged once materialization needs it (a later phase reads
    /// the key back from the index on drain/resume).
    async fn attach_queue_metadata(
        &self,
        session_id: SessionId,
        queued_id: &str,
        client_user_message_id: &str,
    ) {
        let Ok(uuid) = Uuid::parse_str(queued_id) else {
            return;
        };
        let pending_id = PendingInputId::from(uuid);
        if let Err(error) = self.deps.db.set_pending_metadata_field(
            &session_id,
            QueueType::Turn,
            &pending_id,
            "clientUserMessageId",
            client_user_message_id,
        ) {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to persist queue entry dedup key"
            );
        }
    }

    /// The running turn as a canonical `Turn` (for queue/push Started).
    async fn active_canonical_turn(&self, session_id: SessionId) -> Option<CanonicalTurn> {
        let reservation = self.session_turn_reservation_snapshot(session_id).await?;
        let turn = reservation.active_turn.as_ref()?;
        Some(canonical_turn_from_metadata(turn))
    }
}

/// Converts one canonical `UserInput` into the legacy `InputItem` the turn
/// machinery consumes. Image/audio modalities have no legacy counterpart
/// and are rejected (the design's `UNSUPPORTED_MODALITY` case).
pub(crate) fn legacy_input_items(input: &[UserInput]) -> Result<Vec<InputItem>, String> {
    let mut items = Vec::new();
    for part in input {
        let item = match part {
            UserInput::Text { text } => InputItem::Text { text: text.clone() },
            UserInput::Skill { name } => InputItem::Skill {
                name: name.clone(),
                // Canonical skill input carries no path; an empty path keeps
                // resolution name-based (a non-empty path would switch
                // `find_skill` to exact path matching and never match).
                path: std::path::PathBuf::new(),
            },
            UserInput::LocalImage { path, .. } => InputItem::LocalImage { path: path.clone() },
            UserInput::Mention { uri } => InputItem::Mention {
                path: uri.clone(),
                name: None,
            },
            UserInput::Image { uri, .. } | UserInput::Audio { uri, .. } => {
                return Err(format!("unsupported input modality for queue: {uri}"));
            }
        };
        items.push(item);
    }
    Ok(items)
}

/// Maps one legacy `InputItem` back to the canonical `UserInput` part
/// (queue/list; fixes the text-only placeholder from the P4b snapshot).
pub(crate) fn canonical_user_input_from_input_item(item: &InputItem) -> UserInput {
    match item {
        InputItem::Text { text } => UserInput::Text { text: text.clone() },
        InputItem::Skill { name, .. } => UserInput::Skill { name: name.clone() },
        InputItem::LocalImage { path } => UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        },
        InputItem::Mention { path, .. } => UserInput::Mention { uri: path.clone() },
    }
}

/// Builds the canonical queue view from the session's in-memory turn queue.
/// `queueItemId` is the stable pending-input id; `position` is 1-based in
/// current queue order; `preview` is the first 80 chars of the display text.
pub(crate) fn canonical_queue_entries(queue: &VecDeque<PendingInputItem>) -> Vec<QueueEntry> {
    queue
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let input: Vec<UserInput> = match &item.kind {
                PendingInputKind::UserText { text } => vec![UserInput::Text { text: text.clone() }],
                PendingInputKind::UserInput { input, .. } => input
                    .iter()
                    .map(canonical_user_input_from_input_item)
                    .collect(),
                _ => Vec::new(),
            };
            let display_text = match &item.kind {
                PendingInputKind::UserText { text } => text.as_str(),
                PendingInputKind::UserInput { display_text, .. } => display_text.as_str(),
                _ => "",
            };
            QueueEntry {
                queue_item_id: QueueItemId::from_legacy_uuid(Uuid::from(item.id)),
                position: (index + 1) as u32,
                input,
                preview: display_text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(80)
                    .collect(),
                enqueued_at: item.created_at,
            }
        })
        .collect()
}

/// Converts runtime turn metadata into the canonical `Turn` snapshot used
/// by `session/queue/push`'s `Started` outcome.
pub(crate) fn canonical_turn_from_metadata(turn: &crate::turn::TurnMetadata) -> CanonicalTurn {
    let kind = match &turn.kind {
        devo_core::TurnKind::Regular
        | devo_core::TurnKind::Review
        | devo_core::TurnKind::Other(_) => CanonicalTurnKind::Regular,
        devo_core::TurnKind::ManualCompaction => CanonicalTurnKind::Compaction,
    };
    let status = match turn.status {
        TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => {
            CanonicalTurnStatus::InProgress
        }
        TurnStatus::Completed => CanonicalTurnStatus::Completed,
        TurnStatus::Interrupted => CanonicalTurnStatus::Interrupted,
        TurnStatus::Failed => CanonicalTurnStatus::Failed,
    };
    CanonicalTurn {
        id: CanonicalTurnId::from_legacy_uuid(Uuid::from(turn.turn_id)),
        session_id: CanonicalSessionId::from_legacy_uuid(Uuid::from(turn.session_id)),
        sequence: turn.sequence,
        kind,
        status,
        model: ModelBinding {
            provider: turn
                .model_binding_id
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            model: if turn.request_model.is_empty() {
                turn.model.clone()
            } else {
                turn.request_model.clone()
            },
            reasoning_effort: turn
                .reasoning_effort_selection
                .as_deref()
                .and_then(|selection| selection.parse().ok()),
        },
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        error: None,
        usage: None,
    }
}
