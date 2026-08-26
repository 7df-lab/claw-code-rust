//! Provider HTTP connection timeout configuration.
//!
//! Provider response duration is intentionally not bounded at this layer. A
//! model may need an arbitrarily long time to load or generate output, while
//! transport failures are reported by the HTTP/SSE stream and requests can be
//! cancelled by the owning turn.

use std::time::Duration;

/// TCP/TLS connection timeout for provider HTTP clients.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// TCP/TLS connection timeout for provider HTTP clients.
#[inline]
pub fn connect_timeout() -> Duration {
    Duration::from_secs(CONNECT_TIMEOUT_SECS)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn connect_timeout_is_thirty_seconds() {
        assert_eq!(connect_timeout(), Duration::from_secs(30));
    }
}
