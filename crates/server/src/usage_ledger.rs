use chrono::Utc;
use devo_core::SessionId;
use devo_core::TurnId;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::ids;
use devo_protocol::native::model::ModelBinding;
use devo_protocol::native::usage::{
    CallContext, TokenUsage, UsageCallOutcome, UsagePurpose, UsageRecord,
};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use tracing::warn;
use uuid::Uuid;

use anyhow::Result;
use async_trait::async_trait;
use devo_provider::ModelProviderSDK;
use futures::Stream;

use crate::db::Database;
use crate::persistence::RolloutStore;

#[derive(Clone)]
pub(crate) struct UsageLedger {
    rollout_store: RolloutStore,
    db: Arc<Database>,
}

impl UsageLedger {
    pub(crate) fn new(rollout_store: RolloutStore, db: Arc<Database>) -> Self {
        Self { rollout_store, db }
    }

    pub(crate) fn instrumented_provider(
        &self,
        provider: Arc<dyn ModelProviderSDK>,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        purpose: UsagePurpose,
    ) -> Arc<dyn ModelProviderSDK> {
        Arc::new(InstrumentedProvider {
            provider,
            ledger: self.clone(),
            context: RuntimeCallContext::new(session_id, turn_id, purpose),
        })
    }

    fn record(
        &self,
        context: &RuntimeCallContext,
        provider: &str,
        request: &ModelRequest,
        outcome: UsageCallOutcome,
        usage: Option<&Usage>,
    ) {
        let Some(rollout_path) = self
            .db
            .get_session_index(&context.session_id)
            .ok()
            .flatten()
            .and_then(|record| record.rollout_path)
        else {
            warn!(
                session_id = %context.session_id,
                purpose = ?context.call.purpose,
                "cannot persist model-call usage because the session index is unavailable"
            );
            return;
        };
        let record = UsageRecord {
            call_id: Uuid::now_v7().to_string(),
            session_id: context.call.session_id.clone(),
            turn_id: context.call.turn_id.clone(),
            purpose: context.call.purpose,
            model: ModelBinding {
                provider: provider.to_owned(),
                model: request.model.clone(),
                variant: None,
                reasoning_effort: request.reasoning_effort,
            },
            outcome,
            usage: usage.map(token_usage),
            estimated_cost: None,
            recorded_at: Utc::now(),
        };
        if let Err(error) =
            self.rollout_store
                .append_usage_record(&rollout_path, context.session_id, record)
        {
            warn!(
                session_id = %context.session_id,
                purpose = ?context.call.purpose,
                %error,
                "failed to persist model-call usage"
            );
        }
    }
}

#[derive(Clone)]
struct RuntimeCallContext {
    session_id: SessionId,
    call: CallContext,
}

impl RuntimeCallContext {
    fn new(session_id: SessionId, turn_id: Option<TurnId>, purpose: UsagePurpose) -> Self {
        Self {
            session_id,
            call: CallContext {
                session_id: ids::SessionId::from_legacy_uuid(Uuid::from(session_id)),
                turn_id: turn_id.map(Uuid::from).map(ids::TurnId::from_legacy_uuid),
                purpose,
            },
        }
    }
}

fn token_usage(usage: &Usage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens as u64,
        output_tokens: usage.output_tokens as u64,
        reasoning_tokens: usage.reasoning_output_tokens.unwrap_or(0) as u64,
        cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0) as u64,
        cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as u64,
    }
}

struct InstrumentedProvider {
    provider: Arc<dyn ModelProviderSDK>,
    ledger: UsageLedger,
    context: RuntimeCallContext,
}

#[async_trait]
impl ModelProviderSDK for InstrumentedProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut attempt = CallAttemptGuard::new(
            self.ledger.clone(),
            self.context.clone(),
            self.provider.name().to_owned(),
            request.clone(),
        );
        match self.provider.completion(request.clone()).await {
            Ok(response) => {
                attempt.finish(UsageCallOutcome::Succeeded, Some(&response.usage));
                Ok(response)
            }
            Err(error) => {
                attempt.finish(UsageCallOutcome::Failed, None);
                Err(error)
            }
        }
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let mut attempt = CallAttemptGuard::new(
            self.ledger.clone(),
            self.context.clone(),
            self.provider.name().to_owned(),
            request.clone(),
        );
        match self.provider.completion_stream(request.clone()).await {
            Ok(inner) => Ok(Box::pin(MeteredStream { inner, attempt })),
            Err(error) => {
                attempt.finish(UsageCallOutcome::Failed, None);
                Err(error)
            }
        }
    }

    fn name(&self) -> &str {
        self.provider.name()
    }
}

struct CallAttemptGuard {
    ledger: UsageLedger,
    context: RuntimeCallContext,
    provider_name: String,
    request: ModelRequest,
    terminal: bool,
}

impl CallAttemptGuard {
    fn new(
        ledger: UsageLedger,
        context: RuntimeCallContext,
        provider_name: String,
        request: ModelRequest,
    ) -> Self {
        Self {
            ledger,
            context,
            provider_name,
            request,
            terminal: false,
        }
    }

    fn finish(&mut self, outcome: UsageCallOutcome, usage: Option<&Usage>) {
        if self.terminal {
            return;
        }
        self.ledger.record(
            &self.context,
            &self.provider_name,
            &self.request,
            outcome,
            usage,
        );
        self.terminal = true;
    }
}

impl Drop for CallAttemptGuard {
    fn drop(&mut self) {
        self.finish(UsageCallOutcome::Cancelled, None);
    }
}

struct MeteredStream {
    inner: Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>,
    attempt: CallAttemptGuard,
}

impl Stream for MeteredStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = self.inner.as_mut().poll_next(context);
        match &item {
            Poll::Ready(Some(Ok(StreamEvent::MessageDone { response }))) => {
                self.attempt
                    .finish(UsageCallOutcome::Succeeded, Some(&response.usage));
            }
            Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                self.attempt.finish(UsageCallOutcome::Failed, None);
            }
            Poll::Pending | Poll::Ready(Some(Ok(_))) => {}
        }
        item
    }
}

#[cfg(test)]
mod tests {
    use devo_core::ParsedRolloutLine;
    use devo_core::RolloutLineV2;
    use devo_core::parse_rollout_line;
    use devo_protocol::ModelProfileKey;
    use devo_protocol::SamplingControls;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn usage_record_is_appended_to_the_session_rollout() {
        let temp = TempDir::new().expect("temp dir");
        let db = Arc::new(Database::open(temp.path().join("devo.db")).expect("open database"));
        let rollout_store = RolloutStore::new(temp.path().to_path_buf(), Some(Arc::clone(&db)));
        let session_id = SessionId::new();
        let session_record = rollout_store.create_session_record(
            session_id,
            Utc::now(),
            temp.path().to_path_buf(),
            Vec::new(),
            None,
            Some("catalog-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&session_record)
            .expect("append session metadata");
        let metadata = crate::persistence::session_metadata_from_record(
            &session_record,
            session_record.created_at,
        );
        db.upsert_session(&metadata, Some(session_record.rollout_path.as_path()))
            .expect("index session");
        let ledger = UsageLedger::new(rollout_store, db);
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("catalog-model".into()),
            model: "wire-model".into(),
            system: None,
            messages: Vec::new(),
            max_tokens: 128,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: Some(2),
            cache_read_input_tokens: Some(3),
            reasoning_output_tokens: Some(1),
            total_tokens: Some(14),
        };

        ledger.record(
            &RuntimeCallContext::new(session_id, None, UsagePurpose::TitleGeneration),
            "test-provider",
            &request,
            UsageCallOutcome::Succeeded,
            Some(&usage),
        );

        let raw = std::fs::read_to_string(&session_record.rollout_path).expect("read rollout");
        let last = raw.lines().last().expect("usage line");
        let ParsedRolloutLine::V2(line) = parse_rollout_line(last).expect("parse usage line")
        else {
            panic!("usage must use v2");
        };
        let RolloutLineV2::Internal {
            entry: devo_core::InternalRecordV2::UsageRecord { record },
            ..
        } = *line
        else {
            panic!("usage must use the internal usage record");
        };
        assert_eq!(
            record,
            UsageRecord {
                call_id: record.call_id.clone(),
                session_id: ids::SessionId::from_legacy_uuid(Uuid::from(session_id)),
                turn_id: None,
                purpose: UsagePurpose::TitleGeneration,
                model: ModelBinding {
                    provider: "test-provider".into(),
                    model: "wire-model".into(),
                    variant: None,
                    reasoning_effort: None,
                },
                outcome: UsageCallOutcome::Succeeded,
                usage: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    reasoning_tokens: 1,
                    cache_read_input_tokens: 3,
                    cache_creation_input_tokens: 2,
                }),
                estimated_cost: None,
                recorded_at: record.recorded_at,
            }
        );
    }
}
