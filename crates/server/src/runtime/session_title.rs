use std::sync::Arc;

use crate::titles::build_title_generation_request;
use crate::titles::normalize_generated_title;

use super::*;

impl ServerRuntime {
    /// Records first user input and marks title generation in flight without
    /// assigning a display title string.
    pub(super) async fn prepare_title_from_user_input(
        self: &Arc<Self>,
        session_id: SessionId,
        user_input: &str,
    ) {
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        let _ = session_handle
            .set_first_user_input_if_unset(user_input.to_string())
            .await;
        drop(state_change_guard);

        self.mark_title_generating(session_id).await;
    }

    /// Completes LLM title generation before the first turn starts working.
    ///
    /// On untitled sessions this awaits the title model (success → Final,
    /// retries exhausted → Unset). Already-final titles are a no-op. Title
    /// failure does not abort the caller — turn work should still proceed.
    pub(super) async fn await_title_before_first_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        user_input: &str,
    ) {
        self.prepare_title_from_user_input(session_id, user_input)
            .await;

        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        if matches!(title_context.title_state, SessionTitleState::Final(_)) {
            return;
        }
        if user_input.is_empty() {
            return;
        }
        {
            let mut in_flight = self.title_generation_in_flight.lock().await;
            if !in_flight.insert(session_id) {
                return;
            }
        }
        self.clone()
            .generate_final_title(session_id, user_input.to_string())
            .await;
        self.title_generation_in_flight
            .lock()
            .await
            .remove(&session_id);
    }

    /// Spawns LLM title generation as a post-turn fallback when the title is
    /// still Unset/Generating (e.g. first-turn await failed or was skipped).
    pub(super) async fn schedule_final_title_generation(
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
            SessionTitleState::Unset | SessionTitleState::Generating
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
                .generate_final_title(session_id, first_input)
                .await;
            runtime
                .title_generation_in_flight
                .lock()
                .await
                .remove(&session_id);
        });
    }

    /// Clears in-flight auto title work after an explicit user rename.
    pub(super) async fn cancel_auto_title_generation(&self, session_id: SessionId) {
        self.title_generation_in_flight
            .lock()
            .await
            .remove(&session_id);
    }

    async fn mark_title_generating(&self, session_id: SessionId) {
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
        updated_summary.title_state = SessionTitleState::Generating;
        updated_summary.updated_at = updated_at;
        session_handle.update_summary(updated_summary.clone()).await;

        if let Some(record) = session_handle.record().await.flatten()
            && let Err(error) = self.rollout_store.append_title_update(
                &record,
                String::new(),
                SessionTitleState::Generating,
                previous_title,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist generating title state");
        }

        self.persist_session_summary_if_persistent(session_id, &updated_summary)
            .await;
        drop(state_change_guard);

        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: updated_summary,
        }))
        .await;
    }

    const MAX_TITLE_RETRIES: usize = 5;
    const TITLE_RETRY_BASE_DELAY_SECS: u64 = 1;

    fn title_retry_backoff_secs(attempt: usize) -> u64 {
        let base = Self::TITLE_RETRY_BASE_DELAY_SECS * (1u64 << attempt.saturating_sub(1));
        base + (attempt as u64 % 3)
    }

    async fn generate_final_title(
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

            let provider = self.usage_ledger.instrumented_provider(
                runtime_context.provider_for_route(turn_config.provider_route.clone()),
                session_id,
                None,
                devo_protocol::native::usage::UsagePurpose::TitleGeneration,
            );
            let model_request = build_title_generation_request(
                catalog_request_model,
                request_model.clone(),
                &first_user_input,
            );
            let response = match provider.completion(model_request).await {
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
                        let delay = Self::title_retry_backoff_secs(attempt);
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
                        let delay = Self::title_retry_backoff_secs(attempt);
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            };

            let Some(session_handle) = self.session(session_id).await else {
                return;
            };
            let previous_title = session_handle
                .summary()
                .await
                .and_then(|summary| summary.title);
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
                    previous_title,
                )
            {
                tracing::warn!(session_id = %session_id, error = %error, "failed to persist title");
            }

            self.persist_session_summary_if_persistent(session_id, &updated_summary)
                .await;

            self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
                session: updated_summary,
            }))
            .await;
            return;
        }
        tracing::warn!(session_id = %session_id, "title generation exhausted all retries");
        self.reset_title_after_generation_failure(session_id).await;
    }

    async fn reset_title_after_generation_failure(&self, session_id: SessionId) {
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(mut summary) = session_handle.summary().await else {
            return;
        };
        if matches!(summary.title_state, SessionTitleState::Final(_)) {
            return;
        }
        summary.title = None;
        summary.title_state = SessionTitleState::Unset;
        summary.updated_at = Utc::now();
        session_handle.update_summary(summary.clone()).await;
        self.persist_session_summary_if_persistent(session_id, &summary)
            .await;
        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: summary,
        }))
        .await;
    }
}
