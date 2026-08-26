//! User-facing recovery hints for provider failures.
//!
//! Keeps actionable next-step copy in one place so query turn failures and
//! onboarding validation can show the same guidance.

use crate::error::ProviderError;

/// Network / proxy / timeout guidance.
pub const NETWORK_PROXY_HINT: &str = "Check network connectivity and proxy settings ([provider_http].proxy_url or HTTPS_PROXY/HTTP_PROXY).";

/// API key / credential guidance.
pub const AUTH_HINT: &str = "Verify the API key or credential in auth.json / provider config.";

/// Model name / access guidance.
pub const MODEL_NOT_FOUND_HINT: &str =
    "Confirm the model name and that your provider account can access it.";

impl ProviderError {
    /// Optional user-facing next step for recovering from this error.
    pub fn recovery_hint(&self) -> Option<&'static str> {
        match self {
            Self::AuthenticationError { .. } => Some(AUTH_HINT),
            Self::ProviderTimeoutError { .. } | Self::StreamError { .. } => {
                Some(NETWORK_PROXY_HINT)
            }
            Self::ProviderServerError {
                status_code: Some(408),
                ..
            }
            | Self::UnknownError {
                status_code: Some(408),
                ..
            } => Some(NETWORK_PROXY_HINT),
            Self::ModelNotFoundError { .. } => Some(MODEL_NOT_FOUND_HINT),
            Self::RateLimitError { .. }
            | Self::ProviderServerError { .. }
            | Self::ContextLimitError { .. }
            | Self::QuotaExceededError { .. }
            | Self::ContentFilteredError { .. }
            | Self::InvalidRequestError { .. }
            | Self::UnknownError { .. } => None,
        }
    }
}

/// Derives a recovery hint from a structured or stringly-typed provider failure.
pub fn recovery_hint_for_anyhow(error: &anyhow::Error) -> Option<String> {
    for cause in error.chain() {
        if let Some(provider_error) = cause.downcast_ref::<ProviderError>() {
            return provider_error.recovery_hint().map(str::to_string);
        }
        if let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() {
            if reqwest_error.status() == Some(reqwest::StatusCode::UNAUTHORIZED)
                || reqwest_error.status() == Some(reqwest::StatusCode::FORBIDDEN)
            {
                return Some(AUTH_HINT.to_string());
            }
            if reqwest_error.is_timeout()
                || reqwest_error.is_connect()
                || reqwest_error.status() == Some(reqwest::StatusCode::REQUEST_TIMEOUT)
            {
                return Some(NETWORK_PROXY_HINT.to_string());
            }
        }
    }

    recovery_hint_for_message(&error.to_string())
}

/// Derives a recovery hint from a flattened failure message.
///
/// Used when only a string is available (for example worker-side validation
/// timeouts or RPC error text).
pub fn recovery_hint_for_message(message: &str) -> Option<String> {
    let msg = message.to_lowercase();
    if msg.contains("authentication failed")
        || msg.contains("unauthorized")
        || msg.contains("api key")
        || msg.contains("credential")
        || msg.contains("invalid api key")
        || msg.contains("missing credential")
        || (msg.contains("401") && !msg.contains("1401"))
        || msg.contains("403")
    {
        return Some(AUTH_HINT.to_string());
    }
    if msg.contains("model not found")
        || (msg.contains("404")
            && (msg.contains("does not exist")
                || msg.contains("not found")
                || msg.contains("model")))
    {
        return Some(MODEL_NOT_FOUND_HINT.to_string());
    }
    if msg.contains("stream idle timeout")
        || msg.contains("provider timeout")
        || msg.contains("request timeout")
        || msg.contains("request timed out")
        || msg.contains("operation timed out")
        || msg.contains("timed out")
        || msg.contains("deadline has elapsed")
        || msg.contains("deadline exceeded")
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
        || msg.contains("network is unreachable")
        || msg.contains("network unreachable")
        || msg.contains("host unreachable")
        || msg.contains("proxy")
        || msg.contains("408")
    {
        return Some(NETWORK_PROXY_HINT.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn provider_timeout_maps_to_network_hint() {
        let error = ProviderError::ProviderTimeoutError {
            message: "idle".into(),
            provider_name: Some("openai".into()),
        };
        assert_eq!(error.recovery_hint(), Some(NETWORK_PROXY_HINT));
    }

    #[test]
    fn authentication_maps_to_auth_hint() {
        let error = ProviderError::AuthenticationError {
            message: "bad key".into(),
            provider_name: Some("openai".into()),
            status_code: Some(401),
        };
        assert_eq!(error.recovery_hint(), Some(AUTH_HINT));
    }

    #[test]
    fn model_not_found_maps_to_model_hint() {
        let error = ProviderError::ModelNotFoundError {
            message: "missing".into(),
            model_name: Some("gpt-test".into()),
        };
        assert_eq!(error.recovery_hint(), Some(MODEL_NOT_FOUND_HINT));
    }

    #[test]
    fn rate_limit_has_no_hint() {
        let error = ProviderError::RateLimitError {
            message: "slow down".into(),
            retry_after_seconds: Some(30),
            provider_name: None,
        };
        assert_eq!(error.recovery_hint(), None);
    }

    #[test]
    fn message_heuristics_cover_validation_timeout() {
        assert_eq!(
            recovery_hint_for_message("provider connection timed out").as_deref(),
            Some(NETWORK_PROXY_HINT)
        );
        assert_eq!(
            recovery_hint_for_message("anthropic provider requires an API key").as_deref(),
            Some(AUTH_HINT)
        );
        assert_eq!(
            recovery_hint_for_message("model not found: gpt-missing").as_deref(),
            Some(MODEL_NOT_FOUND_HINT)
        );
        assert_eq!(recovery_hint_for_message("quota exceeded"), None);
    }
}
