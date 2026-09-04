use std::sync::Arc;
use std::time::Duration;

use devo_core::resolve_small_model;

use crate::titles::build_title_generation_request;
use crate::titles::heuristic_title_from_user_input;
use crate::titles::normalize_generated_title;

use super::*;

/// Sync spawn helper so retry futures do not form a Send cycle with
/// `notify_title_polish` / `run_title_polish` opaque async types.
fn spawn_title_polish_retry(runtime: Arc<ServerRuntime>, session_id: SessionId, delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        runtime.notify_title_polish(session_id).await;
    });
}

fn spawn_title_polish_run(
    runtime: Arc<ServerRuntime>,
    session_id: SessionId,
    first_user_input: String,
) {
    tokio::spawn(async move {
        runtime
            .clone()
            .run_title_polish(session_id, first_user_input)
            .await;
        runtime
            .title_generation_in_flight
            .lock()
            .await
            .remove(&session_id);
    });
}

impl ServerRuntime {
    /// Records first user input, applies an immediate heuristic title when still
    /// Unset, and marks optional LLM polish as pending.
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

        self.apply_heuristic_title_if_unset(session_id, user_input)
            .await;
        self.mark_title_polish_pending(session_id).await;
    }

    /// Wakes optional LLM title polish when the session is idle.
    ///
    /// Does not call the model inline — the idle-session readiness gate controls when work runs.
    pub(super) async fn notify_title_polish(self: &Arc<Self>, session_id: SessionId) {
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return;
        }
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        if !matches!(
            title_context.title_state,
            SessionTitleState::Final(SessionTitleFinalSource::Heuristic)
        ) {
            self.clear_title_polish_pending(session_id).await;
            return;
        }
        let first_input = {
            let exported = session_handle.export_runtime_session().await;
            let from_session = exported
                .as_ref()
                .and_then(|session| session.first_user_input.clone())
                .filter(|text| !text.is_empty());
            if let Some(text) = from_session {
                text
            } else if let Some(text) = Self::first_user_text_from_history(
                exported
                    .as_ref()
                    .map(|session| session.history_items.as_slice())
                    .unwrap_or(&[]),
            ) {
                let _ = session_handle
                    .set_first_user_input_if_unset(text.clone())
                    .await;
                text
            } else {
                return;
            }
        };
        {
            let pending = self.title_polish_pending.lock().await;
            if !pending.contains_key(&session_id) {
                return;
            }
        }
        {
            let mut in_flight = self.title_generation_in_flight.lock().await;
            if !in_flight.insert(session_id) {
                return;
            }
        }
        spawn_title_polish_run(Arc::clone(self), session_id, first_input);
    }

    /// Re-arms polish after resume when the session still has a heuristic title.
    pub(super) async fn rearm_title_polish_if_needed(self: &Arc<Self>, session_id: SessionId) {
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        if !matches!(
            title_context.title_state,
            SessionTitleState::Final(SessionTitleFinalSource::Heuristic)
        ) {
            return;
        }
        self.mark_title_polish_pending(session_id).await;
        self.notify_title_polish(session_id).await;
    }

    fn first_user_text_from_history(
        history_items: &[devo_protocol::SessionHistoryItem],
    ) -> Option<String> {
        history_items.iter().find_map(|item| {
            if item.kind != devo_protocol::SessionHistoryItemKind::User {
                return None;
            }
            let body = item.body.trim();
            if body.is_empty() {
                None
            } else {
                Some(body.to_string())
            }
        })
    }

    /// Clears pending/in-flight auto title polish after an explicit user rename.
    pub(super) async fn cancel_auto_title_generation(&self, session_id: SessionId) {
        self.clear_title_polish_pending(session_id).await;
        self.title_generation_in_flight
            .lock()
            .await
            .remove(&session_id);
    }

    async fn mark_title_polish_pending(&self, session_id: SessionId) {
        let mut pending = self.title_polish_pending.lock().await;
        pending
            .entry(session_id)
            .or_insert_with(TitlePolishPending::default);
    }

    async fn clear_title_polish_pending(&self, session_id: SessionId) {
        self.title_polish_pending.lock().await.remove(&session_id);
    }

    async fn apply_heuristic_title_if_unset(&self, session_id: SessionId, user_input: &str) {
        let Some(heuristic) = heuristic_title_from_user_input(user_input) else {
            return;
        };
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return;
        };
        if title_context.title_state != SessionTitleState::Unset {
            return;
        }
        let previous_title = session_handle
            .summary()
            .await
            .and_then(|summary| summary.title);
        let Some(updated_summary) = session_handle
            .update_title(
                heuristic.clone(),
                SessionTitleState::Final(SessionTitleFinalSource::Heuristic),
            )
            .await
            .flatten()
        else {
            return;
        };
        if let Some(record) = session_handle.record().await.flatten()
            && let Err(error) = self.rollout_store.append_title_update(
                &record,
                heuristic,
                SessionTitleState::Final(SessionTitleFinalSource::Heuristic),
                previous_title,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist heuristic title");
        }
        self.persist_session_summary_if_persistent(session_id, &updated_summary)
            .await;
        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: updated_summary,
        }))
        .await;
    }

    const MAX_TITLE_POLISH_ATTEMPTS: usize = 5;

    fn title_polish_backoff(attempt: usize) -> Duration {
        match attempt {
            1 => Duration::from_secs(30),
            2 => Duration::from_secs(2 * 60),
            3 => Duration::from_secs(5 * 60),
            4 => Duration::from_secs(10 * 60),
            _ => Duration::from_secs(20 * 60),
        }
    }

    async fn run_title_polish(self: Arc<Self>, session_id: SessionId, first_user_input: String) {
        let attempt = {
            let mut pending = self.title_polish_pending.lock().await;
            let Some(entry) = pending.get_mut(&session_id) else {
                return;
            };
            entry.attempts = entry.attempts.saturating_add(1);
            entry.attempts
        };
        if attempt > Self::MAX_TITLE_POLISH_ATTEMPTS {
            self.clear_title_polish_pending(session_id).await;
            return;
        }

        let polish_ok = self
            .try_polish_title_once(session_id, &first_user_input)
            .await;
        if polish_ok {
            self.clear_title_polish_pending(session_id).await;
            return;
        }

        if attempt >= Self::MAX_TITLE_POLISH_ATTEMPTS {
            tracing::warn!(session_id = %session_id, "title polish exhausted retries; keeping heuristic");
            self.clear_title_polish_pending(session_id).await;
            return;
        }

        spawn_title_polish_retry(
            Arc::clone(&self),
            session_id,
            Self::title_polish_backoff(attempt),
        );
    }

    async fn try_polish_title_once(
        self: &Arc<Self>,
        session_id: SessionId,
        first_user_input: &str,
    ) -> bool {
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return false;
        }
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let Some(title_context) = session_handle.title_generation_context().await else {
            return false;
        };
        if !matches!(
            title_context.title_state,
            SessionTitleState::Final(SessionTitleFinalSource::Heuristic)
        ) {
            return true;
        }

        let configured_small_model = title_context
            .runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_catalog_config()
            .small_model;
        let primary_selection = title_context
            .model_selection
            .clone()
            .unwrap_or_else(|| title_context.runtime_context.default_model.clone());
        let reasoning_effort_selection = title_context.reasoning_effort_selection.clone();
        let runtime_context = title_context.runtime_context;
        let primary_turn_config = runtime_context.resolve_turn_config(
            Some(primary_selection.as_str()),
            reasoning_effort_selection.clone(),
        );
        let configured_small_model = configured_small_model.filter(|model_ref| {
            let Some((provider_id, model_id)) = model_ref.split_once('/') else {
                return false;
            };
            let provider_models = runtime_context
                .model_catalog
                .list_provider_models(provider_id);
            provider_models.contains_key(model_id)
                || model_id
                    .rsplit_once('/')
                    .is_some_and(|(base_model_id, variant_id)| {
                        provider_models
                            .get(base_model_id)
                            .is_some_and(|model| model.variants.contains_key(variant_id))
                    })
        });
        let small_model_selection = configured_small_model.or_else(|| {
            resolve_small_model(
                runtime_context.model_catalog.as_ref(),
                primary_turn_config.model.slug.as_str(),
            )
        });
        let turn_config = if let Some(model_selection) = small_model_selection {
            runtime_context
                .resolve_turn_config(Some(model_selection.as_str()), reasoning_effort_selection)
        } else {
            primary_turn_config
        };
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
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
            first_user_input,
        );
        let response = match provider.completion(model_request).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    model = %turn_config.model.slug,
                    request_model = %request_model,
                    error = %error,
                    "title polish failed"
                );
                return false;
            }
        };

        let generated_title = match normalize_generated_title(&response.content) {
            Ok(title) => title,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    model = %turn_config.model.slug,
                    request_model = %request_model,
                    response_id = %response.id,
                    content_blocks = response.content.len(),
                    title_error = error.as_str(),
                    "title polish returned no valid title"
                );
                return false;
            }
        };

        let Some(session_handle) = self.session(session_id).await else {
            return false;
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
            // Rename / higher Final won the race — stop polishing.
            return true;
        };
        if let Some(record) = session_handle.record().await.flatten()
            && let Err(error) = self.rollout_store.append_title_update(
                &record,
                generated_title,
                SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
                previous_title,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist polished title");
        }

        self.persist_session_summary_if_persistent(session_id, &updated_summary)
            .await;
        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: updated_summary,
        }))
        .await;
        true
    }
}
