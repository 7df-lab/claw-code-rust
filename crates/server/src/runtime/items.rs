use std::borrow::Cow;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use devo_protocol::native::item::Item as NativeItem;
use devo_protocol::native::legacy_wire_from_native_item;

use super::*;

/// Used only when a turn event stream is active but inline state is missing.
/// Avoids mailbox round-trips that deadlock the session actor.
fn next_fallback_item_seq() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 32);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl ServerRuntime {
    /// Persist session summary to SQLite if the session is durable.
    /// The rollout file is the authoritative store, so failures here are
    /// logged as warnings rather than propagated.
    pub(super) async fn persist_session_summary_if_persistent(
        &self,
        session_id: SessionId,
        summary: &SessionMetadata,
    ) {
        if !summary.ephemeral
            && let Err(err) = self.deps.db.upsert_session(summary, None)
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to persist session metadata to database"
            );
        }
    }

    pub(super) async fn emit_turn_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_kind: ItemKind,
        turn_item: TurnItem,
        payload: serde_json::Value,
    ) {
        let (item_id, item_seq) = self
            .start_item(session_id, turn_id, item_kind.clone(), payload.clone())
            .await;
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            item_kind.clone(),
            turn_item,
            payload.clone(),
        )
        .await;
    }

    pub(super) async fn emit_turn_native_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        native_item: NativeItem,
        turn_item: TurnItem,
    ) {
        let (item_id, item_seq) = self
            .start_native_item(session_id, turn_id, native_item.clone())
            .await;
        self.complete_native_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            native_item,
            turn_item,
        )
        .await;
    }

    pub(super) async fn start_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) -> (ItemId, u64) {
        let item_id = ItemId::new();
        let item_seq = self.allocate_item_sequence(session_id).await;
        self.remember_item_started_at(session_id, item_id).await;
        self.emit_item_started(
            session_id,
            turn_id,
            item_id,
            Some(item_seq),
            item_kind,
            payload,
        )
        .await;
        (item_id, item_seq)
    }

    pub(super) async fn start_native_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        native_item: NativeItem,
    ) -> (ItemId, u64) {
        let item_id = ItemId::new();
        let item_seq = self.allocate_item_sequence(session_id).await;
        self.remember_item_started_at(session_id, item_id).await;
        self.emit_native_item_started(session_id, turn_id, item_id, Some(item_seq), native_item)
            .await;
        (item_id, item_seq)
    }

    async fn remember_item_started_at(&self, session_id: SessionId, item_id: ItemId) {
        let Some(stream) = self.active_stream_state(session_id).await else {
            return;
        };
        let mut stream = stream.lock().await;
        if let Some(inline) = stream.turn_inline.as_mut() {
            inline.item_started_at.insert(item_id, chrono::Utc::now());
        }
    }

    async fn take_item_started_at(
        &self,
        session_id: SessionId,
        item_id: ItemId,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let stream = self.active_stream_state(session_id).await?;
        let mut stream = stream.lock().await;
        stream
            .turn_inline
            .as_mut()
            .and_then(|inline| inline.item_started_at.remove(&item_id))
    }

    fn payload_with_started_at(
        mut payload: serde_json::Value,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> serde_json::Value {
        if let Some(started_at) = started_at
            && let Some(object) = payload.as_object_mut()
        {
            object.insert(
                "startedAt".to_string(),
                serde_json::Value::String(
                    started_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                ),
            );
        }
        payload
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn emit_item_started(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: Option<u64>,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) {
        self.broadcast_event(ServerEvent::ItemStarted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq,
            },
            item: ItemEnvelope {
                item_id,
                item_kind,
                payload,
            },
        }))
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn emit_item_completed(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: Option<u64>,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) {
        self.broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq,
            },
            item: ItemEnvelope {
                item_id,
                item_kind,
                payload,
            },
        }))
        .await;
    }

    pub(super) async fn emit_native_item_started(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: Option<u64>,
        native_item: NativeItem,
    ) {
        let (item_kind, payload) =
            legacy_wire_from_native_item(&native_item).expect("native item must reverse-project");
        self.emit_item_started(session_id, turn_id, item_id, item_seq, item_kind, payload)
            .await;
    }

    pub(super) async fn emit_native_item_completed(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: Option<u64>,
        native_item: NativeItem,
    ) {
        let (item_kind, payload) =
            legacy_wire_from_native_item(&native_item).expect("native item must reverse-project");
        self.emit_item_completed(session_id, turn_id, item_id, item_seq, item_kind, payload)
            .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: u64,
        item_kind: ItemKind,
        turn_item: TurnItem,
        payload: serde_json::Value,
    ) {
        let started_at = self.take_item_started_at(session_id, item_id).await;
        self.persist_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            turn_item,
            Some(TurnStatus::Running),
            None,
            started_at,
        )
        .await;
        self.emit_item_completed(
            session_id,
            turn_id,
            item_id,
            Some(item_seq),
            item_kind,
            Self::payload_with_started_at(payload, started_at),
        )
        .await;
    }

    pub(super) async fn complete_native_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: u64,
        native_item: NativeItem,
        turn_item: TurnItem,
    ) {
        let started_at = self.take_item_started_at(session_id, item_id).await;
        self.persist_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            turn_item,
            Some(TurnStatus::Running),
            None,
            started_at,
        )
        .await;
        let (item_kind, payload) =
            legacy_wire_from_native_item(&native_item).expect("native item must reverse-project");
        self.emit_item_completed(
            session_id,
            turn_id,
            item_id,
            Some(item_seq),
            item_kind,
            Self::payload_with_started_at(payload, started_at),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: u64,
        turn_item: TurnItem,
        turn_status: Option<TurnStatus>,
        worklog: Option<Worklog>,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        if let Some(stream) = self.active_stream_state(session_id).await {
            // Mutate inline state under the lock, then release before any
            // blocking rollout I/O so the event stream cannot pin the async
            // mutex across synchronous disk writes.
            let inline_rollout = {
                let mut stream = stream.lock().await;
                stream.turn_inline.as_mut().map(|inline| {
                    if inline.turn_id == turn_id
                        && let Some(history_item) = history_item_from_turn_item(&turn_item)
                    {
                        inline.history_items.push(history_item);
                    }
                    if inline.turn_id == turn_id {
                        inline
                            .persisted_turn_items
                            .push(crate::execution::PersistedTurnItem {
                                turn_id,
                                turn_kind: inline.turn_kind.clone(),
                                item_id,
                                turn_item: turn_item.clone(),
                            });
                    }
                    inline.record.clone().map(|record| {
                        (
                            record,
                            build_item_record(
                                session_id,
                                turn_id,
                                item_id,
                                item_seq,
                                turn_item.clone(),
                                turn_status.clone(),
                                worklog.clone(),
                                started_at,
                            ),
                        )
                    })
                })
            };
            if let Some(rollout) = inline_rollout {
                if let Some((record, item)) = rollout
                    && let Err(error) = self.rollout_store.append_item(&record, item)
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist item line");
                }
                return;
            }
            // Active stream is registered but inline state is missing. The session
            // actor is not polling its mailbox until the stream finishes, so we
            // must not fall through to blocking actor commands.
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn_id,
                "persist_item skipped: active turn stream has no inline state"
            );
            return;
        }
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        if let Some(history_item) = history_item_from_turn_item(&turn_item) {
            session_handle.append_history_item(history_item).await;
        }
        let Some(prep) = session_handle.prepare_persist_item(turn_id).await else {
            return;
        };
        session_handle
            .append_persisted_item(crate::execution::PersistedTurnItem {
                turn_id,
                turn_kind: prep.turn_kind,
                item_id,
                turn_item: turn_item.clone(),
            })
            .await;
        if let Some(record) = prep.record {
            let item = build_item_record(
                session_id,
                turn_id,
                item_id,
                item_seq,
                turn_item,
                turn_status,
                worklog,
                started_at,
            );
            if let Err(error) = self.rollout_store.append_item(&record, item) {
                tracing::warn!(session_id = %session_id, error = %error, "failed to persist item line");
            }
        }
    }

    pub(super) async fn allocate_item_sequence(&self, session_id: SessionId) -> u64 {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                return inline.allocate_item_seq();
            }
            // Same deadlock constraint as persist_item: never wait on the actor
            // mailbox while its turn event stream is registered.
            return next_fallback_item_seq();
        }
        if let Some(handle) = self.session(session_id).await
            && let Some(item_seq) = handle.allocate_item_seq().await
        {
            return item_seq;
        }
        1
    }
}

pub(crate) fn render_input_items(input: &[crate::InputItem]) -> Option<String> {
    let mut rendered = String::new();
    for item in input {
        let part = match item {
            crate::InputItem::Text { text } => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                Cow::Borrowed(text)
            }
            crate::InputItem::Skill { name, path } => {
                Cow::Owned(format!("[skill:{name} @ {}]", path.display()))
            }
            crate::InputItem::LocalImage { path } => {
                Cow::Owned(format!("[image:{}]", path.display()))
            }
            crate::InputItem::Mention { path, name } => Cow::Owned(format!(
                "[mention:{}]",
                name.as_deref().unwrap_or(path.as_str())
            )),
        };
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&part);
    }
    (!rendered.is_empty()).then_some(rendered)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::InputItem;

    #[test]
    fn render_input_items_trims_text_and_preserves_item_markers() {
        let input = vec![
            InputItem::Text {
                text: "  hello  ".to_string(),
            },
            InputItem::Text {
                text: "   ".to_string(),
            },
            InputItem::Skill {
                name: "writer".to_string(),
                path: PathBuf::from("writer.md"),
            },
            InputItem::LocalImage {
                path: PathBuf::from("photo.png"),
            },
            InputItem::Mention {
                path: "src/lib.rs".to_string(),
                name: None,
            },
            InputItem::Mention {
                path: "src/main.rs".to_string(),
                name: Some("main".to_string()),
            },
        ];

        assert_eq!(
            render_input_items(&input),
            Some(
                "hello\n[skill:writer @ writer.md]\n[image:photo.png]\n[mention:src/lib.rs]\n[mention:main]"
                    .to_string()
            )
        );
    }

    #[test]
    fn render_input_items_returns_none_for_empty_text_only_input() {
        assert_eq!(
            render_input_items(&[InputItem::Text {
                text: " \n\t ".to_string(),
            }]),
            None
        );
    }
}
