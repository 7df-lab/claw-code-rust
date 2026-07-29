use std::borrow::Cow;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use crate::titles::build_title_generation_request;
use crate::titles::derive_provisional_title;
use crate::titles::normalize_generated_title;

use super::*;

/// Used only when a turn event stream is active but inline state is missing.
/// Avoids mailbox round-trips that deadlock the session actor.
fn next_fallback_item_seq() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 32);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl ServerRuntime {
    pub(super) async fn maybe_start_title_generation_from_user_input(
        self: &Arc<Self>,
        session_id: SessionId,
        user_input: &str,
    ) {
        self.maybe_prepare_title_generation_from_user_input(session_id, user_input)
            .await;
        self.maybe_schedule_final_title_generation(session_id, Some(user_input.to_string()))
            .await;
    }

    /// Assigns a provisional title and records the first user input without
    /// calling the title model.
    pub(super) async fn maybe_prepare_title_generation_from_user_input(
        self: &Arc<Self>,
        session_id: SessionId,
        user_input: &str,
    ) {
        self.maybe_assign_provisional_title(session_id, user_input)
            .await;

        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let _state_change_guard = session_handle.lock_state_change().await;
        let _ = session_handle
            .set_first_user_input_if_unset(user_input.to_string())
            .await;
    }

    /// Spawns final (LLM) title generation in the background.
    ///
    /// Safe to call at turn start: actor mailbox round-trips happen here, then
    /// the model call runs on a detached task so it does not block `ExecuteTurn`.
    /// Duplicate schedules for the same session are ignored while a generation
    /// task is already in flight.
    pub(super) async fn maybe_schedule_final_title_generation(
        self: &Arc<Self>,
        session_id: SessionId,
        first_input_override: Option<String>,
    ) {
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        let needs_title = matches!(
            title_context.title_state,
            SessionTitleState::Unset | SessionTitleState::Provisional
        );
        if !needs_title {
            return;
        }
        let first_input = if let Some(first_input) = first_input_override {
            first_input
        } else {
            session_handle
                .export_runtime_session()
                .await
                .and_then(|session| session.first_user_input)
                .unwrap_or_default()
        };
        if first_input.is_empty() {
            return;
        }
        {
            let mut in_flight = self.title_generation_in_flight.lock().await;
            if !in_flight.insert(session_id) {
                return;
            }
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .clone()
                .maybe_generate_final_title(session_id, first_input)
                .await;
            runtime
                .title_generation_in_flight
                .lock()
                .await
                .remove(&session_id);
        });
    }

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

    pub(super) async fn maybe_assign_provisional_title(
        &self,
        session_id: SessionId,
        first_user_input: &str,
    ) {
        let Some(candidate) = derive_provisional_title(first_user_input) else {
            return;
        };
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        if title_context.title_state != SessionTitleState::Unset {
            return;
        }
        let Some(summary) = session_handle.summary().await else {
            return;
        };
        if summary.title.is_some() {
            return;
        }

        let previous_title = summary.title.clone();
        let updated_at = Utc::now();
        let mut updated_summary = summary;
        updated_summary.title = Some(candidate.clone());
        updated_summary.title_state = SessionTitleState::Provisional;
        updated_summary.updated_at = updated_at;
        session_handle.update_summary(updated_summary.clone()).await;

        if let Some(record) = session_handle.record().await.flatten()
            && let Err(error) = self.rollout_store.append_title_update(
                &record,
                candidate.clone(),
                SessionTitleState::Provisional,
                previous_title,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist provisional title");
        }

        self.persist_session_summary_if_persistent(session_id, &updated_summary)
            .await;
        drop(state_change_guard);

        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: updated_summary,
        }))
        .await;
    }

    /// Attempts to generate a final session title by calling the LLM.
    /// Retries up to MAX_TITLE_RETRIES times with exponential backoff.
    /// Exhausting retries leaves the title at `Provisional`; the caller
    /// should re-trigger on the next user message.
    const MAX_TITLE_RETRIES: usize = 5;
    const TITLE_RETRY_BASE_DELAY_SECS: u64 = 1;

    pub(super) async fn maybe_generate_final_title(
        self: Arc<Self>,
        session_id: SessionId,
        first_user_input: String,
    ) {
        for attempt in 1..=Self::MAX_TITLE_RETRIES {
            let Some(session_handle) = self.session(session_id).await else {
                return;
            };
            let Some(title_context) = session_handle.title_generation_context().await else {
                return;
            };
            if matches!(title_context.title_state, SessionTitleState::Final(_)) {
                return;
            }
            let model_selection = title_context
                .model_selection
                .clone()
                .unwrap_or_else(|| title_context.runtime_context.default_model.clone());
            let reasoning_effort_selection = title_context.reasoning_effort_selection.clone();
            let runtime_context = title_context.runtime_context;

            let turn_config = runtime_context
                .resolve_turn_config(Some(model_selection.as_str()), reasoning_effort_selection);
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let catalog_request_model = resolved_request.request_model.clone();
            let request_model = turn_config.provider_request_model(&catalog_request_model);

            let response = match runtime_context
                .provider_router
                .complete(
                    turn_config.provider_route.clone(),
                    build_title_generation_request(
                        catalog_request_model,
                        request_model.clone(),
                        &first_user_input,
                    ),
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        attempt,
                        model = %turn_config.model.slug,
                        request_model = %request_model,
                        error = %error,
                        "title gen failed"
                    );
                    if attempt < Self::MAX_TITLE_RETRIES {
                        let delay = Self::TITLE_RETRY_BASE_DELAY_SECS * (1u64 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            };

            let generated_title = match normalize_generated_title(&response.content) {
                Ok(title) => title,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        attempt,
                        model = %turn_config.model.slug,
                        request_model = %request_model,
                        response_id = %response.id,
                        content_blocks = response.content.len(),
                        title_error = error.as_str(),
                        "title gen returned no valid title"
                    );
                    if attempt < Self::MAX_TITLE_RETRIES {
                        let delay = Self::TITLE_RETRY_BASE_DELAY_SECS * (1u64 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            };

            let Some(session_handle) = self.session(session_id).await else {
                return;
            };
            let state_change_guard = session_handle.lock_state_change().await;
            let Some(updated_summary) = session_handle
                .update_title(
                    generated_title.clone(),
                    SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
                )
                .await
                .flatten()
            else {
                return;
            };
            if let Some(record) = session_handle.record().await.flatten()
                && let Err(error) = self.rollout_store.append_title_update(
                    &record,
                    generated_title.clone(),
                    SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
                    updated_summary.title.clone(),
                )
            {
                tracing::warn!(session_id = %session_id, error = %error, "failed to persist title");
            }

            self.persist_session_summary_if_persistent(session_id, &updated_summary)
                .await;
            drop(state_change_guard);

            self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
                session: updated_summary,
            }))
            .await;
            return;
        }
        tracing::warn!(session_id = %session_id, "title generation exhausted all retries");
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

    pub(super) async fn start_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) -> (ItemId, u64) {
        let item_id = ItemId::new();
        let item_seq = self.allocate_item_sequence(session_id).await;
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
        self.persist_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            turn_item,
            Some(TurnStatus::Running),
            None,
        )
        .await;
        self.emit_item_completed(
            session_id,
            turn_id,
            item_id,
            Some(item_seq),
            item_kind,
            payload,
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
