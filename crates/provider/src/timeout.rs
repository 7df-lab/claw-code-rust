//! Provider HTTP and SSE stream timeout configuration.
//!
//! Non-streaming requests use a total [`request_timeout`]. Streaming responses
//! use a per-chunk idle [`stream_idle_timeout`] that resets whenever a new SSE
//! event arrives, so long generations are not cut off by a fixed wall-clock cap.

use std::time::Duration;

use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};

use crate::error::ProviderError;

/// Total wall-clock timeout for non-streaming provider HTTP requests.
pub const REQUEST_TIMEOUT_SECS: u64 = 120;

/// TCP/TLS connect timeout for provider HTTP clients.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// Maximum idle time between consecutive SSE events during streaming.
pub const STREAM_IDLE_TIMEOUT_SECS: u64 = 60;

/// Total timeout for non-streaming provider HTTP requests.
#[inline]
pub fn request_timeout() -> Duration {
    Duration::from_secs(REQUEST_TIMEOUT_SECS)
}

/// TCP/TLS connect timeout for provider HTTP clients.
#[inline]
pub fn connect_timeout() -> Duration {
    Duration::from_secs(CONNECT_TIMEOUT_SECS)
}

/// Idle timeout between consecutive SSE events during streaming.
#[inline]
pub fn stream_idle_timeout() -> Duration {
    Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS)
}

/// Waits for the next SSE event, failing when no data arrives within
/// [`stream_idle_timeout`].
pub async fn next_eventsource_event(
    event_source: &mut EventSource,
) -> Result<Option<Result<Event, reqwest_eventsource::Error>>, StreamIdleTimeoutError> {
    match tokio::time::timeout(stream_idle_timeout(), event_source.next()).await {
        Ok(event) => Ok(event),
        Err(_) => Err(StreamIdleTimeoutError {
            idle_timeout: stream_idle_timeout(),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamIdleTimeoutError {
    pub idle_timeout: Duration,
}

impl std::fmt::Display for StreamIdleTimeoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "provider stream idle timeout after {}s without receiving data",
            self.idle_timeout.as_secs()
        )
    }
}

impl std::error::Error for StreamIdleTimeoutError {}

/// Maps a stream idle timeout into a retryable [`ProviderError::ProviderTimeoutError`].
pub fn stream_idle_timeout_provider_error(
    provider_name: &str,
    model: &str,
    idle: StreamIdleTimeoutError,
) -> ProviderError {
    ProviderError::ProviderTimeoutError {
        message: format!("{provider_name} stream idle timeout for model {model}: {idle}"),
        provider_name: Some(provider_name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn request_timeout_is_two_minutes() {
        assert_eq!(request_timeout(), Duration::from_secs(120));
    }

    #[test]
    fn connect_timeout_is_thirty_seconds() {
        assert_eq!(connect_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn stream_idle_timeout_is_one_minute() {
        assert_eq!(stream_idle_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn stream_idle_timeout_maps_to_provider_timeout_error() {
        let error = stream_idle_timeout_provider_error(
            "openai",
            "gpt-test",
            StreamIdleTimeoutError {
                idle_timeout: stream_idle_timeout(),
            },
        );
        assert!(error.is_recoverable());
        assert!(error.is_transient());
        match error {
            ProviderError::ProviderTimeoutError {
                message,
                provider_name,
            } => {
                assert_eq!(
                    message,
                    "openai stream idle timeout for model gpt-test: provider stream idle timeout after 60s without receiving data"
                );
                assert_eq!(provider_name.as_deref(), Some("openai"));
            }
            other => panic!("expected ProviderTimeoutError, got {other:?}"),
        }
    }
}
