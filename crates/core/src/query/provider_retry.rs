//! Provider error classification and retry policy for the query loop.

use std::io::ErrorKind;
use std::time::Duration;

use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::AgentError;
use devo_provider::error::ProviderError;

use super::event::EventCallback;
use super::event::ProviderRetryStatus;
use super::event::QueryEvent;
use super::event::QueryProviderRetryPhase;
use super::event::emit_query_event;

const MAX_RETRIES: usize = 5;
const INITIAL_RETRY_BACKOFF_MS: u64 = 250;
const RATE_LIMIT_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    ContextTooLong,
    ParameterError,
    FileContentAnomaly,
    AuthenticationFailure,
    FeatureUnavailable,
    TaskNotFound,
    RateLimit,
    NoApiPermission,
    FileTooLarge,
    ServerError,
    NetworkError,
    Unretryable,
}

pub(crate) enum ProviderRetryDecision {
    RetryAfter(Duration),
    CompactAndRetry,
    Fail,
}

pub(crate) fn classify_error(e: &anyhow::Error) -> ErrorClass {
    for cause in e.chain() {
        let Some(provider_error) = cause.downcast_ref::<ProviderError>() else {
            continue;
        };
        match provider_error {
            ProviderError::AuthenticationError { .. } => return ErrorClass::AuthenticationFailure,
            ProviderError::RateLimitError { .. } => return ErrorClass::RateLimit,
            ProviderError::ProviderServerError {
                status_code: Some(429),
                ..
            } => return ErrorClass::RateLimit,
            ProviderError::ProviderServerError {
                status_code: Some(408),
                ..
            }
            | ProviderError::ProviderTimeoutError { .. }
            | ProviderError::StreamError { .. } => return ErrorClass::NetworkError,
            ProviderError::ProviderServerError { .. } => return ErrorClass::ServerError,
            ProviderError::ContextLimitError { .. } => return ErrorClass::ContextTooLong,
            ProviderError::ModelNotFoundError { .. } => return ErrorClass::TaskNotFound,
            ProviderError::InvalidRequestError { .. } => return ErrorClass::ParameterError,
            ProviderError::QuotaExceededError { .. }
            | ProviderError::ContentFilteredError { .. } => {
                return ErrorClass::Unretryable;
            }
            ProviderError::UnknownError {
                status_code: Some(429),
                ..
            } => return ErrorClass::RateLimit,
            ProviderError::UnknownError {
                status_code: Some(408),
                ..
            } => return ErrorClass::NetworkError,
            ProviderError::UnknownError {
                status_code: Some(500..=599),
                ..
            } => return ErrorClass::ServerError,
            ProviderError::UnknownError { .. } => {}
        }
    }

    if e.chain().any(|cause| {
        cause
            .downcast_ref::<devo_provider::timeout::StreamIdleTimeoutError>()
            .is_some()
    }) {
        return ErrorClass::NetworkError;
    }

    if e.chain().any(|cause| {
        cause.downcast_ref::<reqwest::Error>().is_some_and(|error| {
            error.is_timeout()
                || error.is_connect()
                || error.status() == Some(reqwest::StatusCode::REQUEST_TIMEOUT)
        })
    }) {
        return ErrorClass::NetworkError;
    }

    if e.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
            )
        })
    }) {
        return ErrorClass::NetworkError;
    }

    let msg = e.to_string().to_lowercase();
    // TODO: Expand the error of ContextTooLong
    if msg.contains("context_too_long") {
        ErrorClass::ContextTooLong
    } else if msg.contains("401")
        || msg.contains("authentication failure")
        || msg.contains("token timeout")
        || msg.contains("unauthorized")
        || msg.contains("api key")
    {
        ErrorClass::AuthenticationFailure
    } else if msg.contains("404")
        && (msg.contains("feature not available")
            || msg.contains("fine-tuning feature not available"))
    {
        ErrorClass::FeatureUnavailable
    } else if msg.contains("404")
        && (msg.contains("task does not exist")
            || msg.contains("does not exist")
            || msg.contains("not found"))
    {
        ErrorClass::TaskNotFound
    } else if msg.contains("429") || msg.contains("rate limit") {
        ErrorClass::RateLimit
    } else if msg.contains("434") || msg.contains("no api permission") || msg.contains("beta phase")
    {
        ErrorClass::NoApiPermission
    } else if msg.contains("435")
        || msg.contains("file size exceeds 100mb")
        || msg.contains("smaller than 100mb")
    {
        ErrorClass::FileTooLarge
    } else if msg.contains("400")
        && (msg.contains("file content anomaly")
            || msg.contains("jsonl file content")
            || msg.contains("jsonl"))
    {
        ErrorClass::FileContentAnomaly
    } else if msg.contains("408")
        || msg.contains("request timeout")
        || msg.contains("request timed out")
        || msg.contains("operation timed out")
        || msg.contains("timed out")
        || msg.contains("deadline has elapsed")
        || msg.contains("deadline exceeded")
        || msg.contains("provider timeout")
        || msg.contains("stream idle timeout")
        || msg.contains("network error")
        || msg.contains("network is unreachable")
        || msg.contains("network unreachable")
        || msg.contains("host unreachable")
        || msg.contains("destination unreachable")
        || msg.contains("unreachable host")
        || msg.contains("no route to host")
        || msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection closed")
        || msg.contains("connection aborted")
        || msg.contains("connection timed out")
        || msg.contains("connection failure")
        || msg.contains("connection failed")
        || msg.contains("failed to connect")
        || msg.contains("connect error")
        || msg.contains("error trying to connect")
        || msg.contains("error sending request")
        || msg.contains("dns error")
        || msg.contains("failed to lookup address information")
        || msg.contains("temporary failure in name resolution")
        || msg.contains("name or service not known")
        || msg.contains("nodename nor servname")
        || msg.contains("could not resolve host")
        || msg.contains("unexpected eof")
        || msg.contains("invalidcontenttype")
        || msg.contains("invalid content-type")
        || msg.contains("invalid header value")
        || msg.contains("text/event-stream")
        // Stream-level decode/decrypt errors (e.g. TLS decrypt failure, chunk
        // deserialization).  These are typically transient — the proxy or
        // TLS-terminator that sits between us and the provider may have had a
        // hiccup; retrying usually succeeds.
        || msg.contains("error decoding")
        || msg.contains("decoding response")
        || msg.contains("cannot decrypt")
        || msg.contains("decrypt error")
        || msg.contains("decrypterror")
        || msg.contains("stream error")
        || msg.contains("failed to decode")
    {
        ErrorClass::NetworkError
    } else if msg.contains("400")
        || msg.contains("parameter error")
        || msg.contains("invalid parameter")
        || msg.contains("bad request")
    {
        ErrorClass::ParameterError
    } else if msg.starts_with('5')
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("internal server error")
        || msg.contains("server error occurred while processing the request")
    {
        ErrorClass::ServerError
    } else {
        ErrorClass::Unretryable
    }
}

pub(crate) fn provider_retry_decision(
    error: &anyhow::Error,
    retry_count: &mut usize,
    context_compacted: &mut bool,
) -> ProviderRetryDecision {
    match classify_error(error) {
        ErrorClass::ContextTooLong => {
            if *context_compacted {
                ProviderRetryDecision::Fail
            } else {
                *context_compacted = true;
                ProviderRetryDecision::CompactAndRetry
            }
        }
        ErrorClass::RateLimit => {
            if *retry_count >= MAX_RETRIES {
                ProviderRetryDecision::Fail
            } else {
                *retry_count += 1;
                ProviderRetryDecision::RetryAfter(RATE_LIMIT_RETRY_DELAY)
            }
        }
        ErrorClass::ServerError | ErrorClass::NetworkError => {
            if *retry_count >= MAX_RETRIES {
                ProviderRetryDecision::Fail
            } else {
                *retry_count += 1;
                ProviderRetryDecision::RetryAfter(retry_backoff_duration(*retry_count))
            }
        }
        ErrorClass::ParameterError
        | ErrorClass::FileContentAnomaly
        | ErrorClass::AuthenticationFailure
        | ErrorClass::FeatureUnavailable
        | ErrorClass::TaskNotFound
        | ErrorClass::NoApiPermission
        | ErrorClass::FileTooLarge
        | ErrorClass::Unretryable => ProviderRetryDecision::Fail,
    }
}

pub(crate) async fn wait_for_provider_retry(
    on_event: &Option<EventCallback>,
    cancel_token: Option<&CancellationToken>,
    provider: &str,
    model: &str,
    attempt: usize,
    backoff: Duration,
) -> Result<(), AgentError> {
    let backoff_ms = backoff.as_millis().min(u128::from(u64::MAX)) as u64;
    emit_query_event(
        on_event,
        QueryEvent::ProviderRetryStatus(ProviderRetryStatus {
            provider: provider.to_string(),
            model: model.to_string(),
            attempt,
            backoff_ms,
            phase: QueryProviderRetryPhase::Scheduled,
            message: format!("Retrying provider request in {:.1}s", backoff.as_secs_f64()),
        }),
    )
    .await;

    if let Some(cancel_token) = cancel_token {
        tokio::select! {
            biased;
            () = cancel_token.cancelled() => return Err(AgentError::Aborted),
            () = sleep(backoff) => {}
        }
    } else {
        sleep(backoff).await;
    }

    emit_query_event(
        on_event,
        QueryEvent::ProviderRetryStatus(ProviderRetryStatus {
            provider: provider.to_string(),
            model: model.to_string(),
            attempt,
            backoff_ms: 0,
            phase: QueryProviderRetryPhase::Resumed,
            message: "Retrying provider request now".to_string(),
        }),
    )
    .await;

    Ok(())
}

fn retry_backoff_duration(attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10) as u32;
    let multiplier = 2u64.pow(exponent);
    Duration::from_millis(INITIAL_RETRY_BACKOFF_MS.saturating_mul(multiplier))
}
