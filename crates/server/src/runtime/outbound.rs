use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::NotificationEnvelope;

pub(crate) const OUTBOUND_CHANNEL_CAPACITY: usize = 4096;
pub(crate) const OUTBOUND_RELIABLE_RESERVED_CAPACITY: usize = 64;
pub(crate) const OUTBOUND_BACKPRESSURE_LOG_THRESHOLD: Duration = Duration::from_millis(50);
/// Max time streaming notifications wait for outbound capacity before being
/// dropped. Event streams must not park forever on a slow client: parent+child
/// turns share one connection and can fill the queue quickly. Lifecycle
/// traffic (`item/completed`, `turn/completed`, …) rides this wait too —
/// dropping it wedges client render state — so the budget is sized for bursty
/// slow consumers (Windows console pipes) rather than round-trip latency.
pub(crate) const OUTBOUND_NOTIFICATION_MAX_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutboundDeliveryPolicy {
    Reliable,
    BestEffort,
}

pub(crate) enum OutboundPayload {
    Notification {
        connection_id: u64,
        method: String,
        event_seq: u64,
        params: serde_json::Value,
    },
    JsonRpcResponse {
        connection_id: u64,
        value: serde_json::Value,
    },
    ClientRequest {
        connection_id: u64,
        method: String,
        value: serde_json::Value,
    },
}

pub struct OutboundFrame {
    pub(crate) payload: OutboundPayload,
    pub(crate) delivered: Option<oneshot::Sender<bool>>,
}

impl OutboundFrame {
    pub(crate) fn notification(
        connection_id: u64,
        method: String,
        event_seq: u64,
        params: serde_json::Value,
    ) -> Self {
        Self {
            payload: OutboundPayload::Notification {
                connection_id,
                method,
                event_seq,
                params,
            },
            delivered: None,
        }
    }

    pub fn json_rpc_response(connection_id: u64, value: serde_json::Value) -> Self {
        Self {
            payload: OutboundPayload::JsonRpcResponse {
                connection_id,
                value,
            },
            delivered: None,
        }
    }

    pub(crate) fn json_rpc_response_with_delivery(
        connection_id: u64,
        value: serde_json::Value,
        delivered: oneshot::Sender<bool>,
    ) -> Self {
        Self {
            payload: OutboundPayload::JsonRpcResponse {
                connection_id,
                value,
            },
            delivered: Some(delivered),
        }
    }

    pub(crate) fn client_request(
        connection_id: u64,
        method: String,
        value: serde_json::Value,
    ) -> Self {
        Self {
            payload: OutboundPayload::ClientRequest {
                connection_id,
                method,
                value,
            },
            delivered: None,
        }
    }

    pub(crate) fn connection_id(&self) -> u64 {
        match &self.payload {
            OutboundPayload::Notification { connection_id, .. }
            | OutboundPayload::JsonRpcResponse { connection_id, .. }
            | OutboundPayload::ClientRequest { connection_id, .. } => *connection_id,
        }
    }

    pub(crate) fn log_method(&self) -> &str {
        match &self.payload {
            OutboundPayload::Notification { method, .. } => method,
            OutboundPayload::JsonRpcResponse { .. } => "<response>",
            OutboundPayload::ClientRequest { method, .. } => method,
        }
    }

    pub(crate) fn event_seq(&self) -> u64 {
        match &self.payload {
            OutboundPayload::Notification { event_seq, .. } => *event_seq,
            OutboundPayload::JsonRpcResponse { .. } | OutboundPayload::ClientRequest { .. } => 0,
        }
    }
}

pub(crate) fn outbound_frame_to_value(frame: &OutboundFrame) -> serde_json::Value {
    match &frame.payload {
        OutboundPayload::Notification { method, params, .. } => {
            serde_json::to_value(NotificationEnvelope {
                method: method.clone(),
                params: params.clone(),
            })
            .expect("serialize client notification envelope")
        }
        OutboundPayload::JsonRpcResponse { value, .. }
        | OutboundPayload::ClientRequest { value, .. } => value.clone(),
    }
}

pub(crate) fn log_outbound_frame(frame: &OutboundFrame, notification: &serde_json::Value) {
    let connection_id = frame.connection_id();
    let method = frame.log_method();
    let event_seq = frame.event_seq();
    let item_id = notification_item_id(notification);
    let assistant_delta = notification_assistant_delta(method, notification);
    let delta_len = assistant_delta.map(str::len);
    let assistant_token_text = assistant_delta.and_then(assistant_token_log_preview);
    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            connection_id,
            method = %method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            assistant_token_text,
            "sending client notification"
        );
    } else {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            connection_id,
            method = %method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            "sending client notification"
        );
    }
}

/// Enqueue an outbound frame, waiting when the channel is full.
///
/// Attempts to reserve a send permit within
/// [`OUTBOUND_BACKPRESSURE_LOG_THRESHOLD`]. If the channel is still full after
/// that window, logs a backpressure warning and blocks until a permit is
/// available (or the receiver is dropped).
///
/// Returns `true` when the frame is accepted, or `false` when the outbound
/// receiver has already been dropped. `queue` is a static label used only in
/// log fields to identify which outbound path is under pressure.
pub(crate) async fn enqueue_outbound(
    tx: &mpsc::Sender<OutboundFrame>,
    frame: OutboundFrame,
    queue: &'static str,
) -> bool {
    let connection_id = frame.connection_id();
    let reserve_started_at = Instant::now();
    // Prefer a timed reserve so slow consumers surface as backpressure logs
    // instead of silent stalls.
    let permit = match tokio::time::timeout(OUTBOUND_BACKPRESSURE_LOG_THRESHOLD, tx.reserve()).await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => {
            tracing::debug!(connection_id, queue, "outbound queue receiver dropped");
            return false;
        }
        Err(_) => {
            // Timed out waiting for capacity; keep blocking so messages are not
            // dropped, but record that the consumer is lagging.
            tracing::warn!(
                connection_id,
                queue,
                threshold_ms = OUTBOUND_BACKPRESSURE_LOG_THRESHOLD.as_millis(),
                "outbound queue applying backpressure"
            );
            match tx.reserve().await {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::debug!(
                        connection_id,
                        queue,
                        "outbound queue receiver dropped during backpressure"
                    );
                    return false;
                }
            }
        }
    };
    let waited = reserve_started_at.elapsed();
    if waited >= OUTBOUND_BACKPRESSURE_LOG_THRESHOLD {
        tracing::debug!(
            connection_id,
            queue,
            waited_ms = waited.as_millis(),
            "outbound queue accepted message after backpressure"
        );
    }
    permit.send(frame);
    true
}

/// Enqueue a notification according to its delivery policy.
///
/// Best-effort notifications are accepted only when doing so preserves
/// [`OUTBOUND_RELIABLE_RESERVED_CAPACITY`] slots for lifecycle/control traffic.
/// Reliable notifications can use that reserved capacity and wait up to
/// [`OUTBOUND_NOTIFICATION_MAX_WAIT`] if reliable traffic fills the queue. This
/// keeps high-volume deltas from starving terminal events without allowing a
/// stalled client to park a turn event stream indefinitely.
pub(crate) async fn enqueue_outbound_notification(
    tx: &mpsc::Sender<OutboundFrame>,
    frame: OutboundFrame,
    policy: OutboundDeliveryPolicy,
    queue: &'static str,
) -> bool {
    let connection_id = frame.connection_id();
    if policy == OutboundDeliveryPolicy::BestEffort {
        let reserved_capacity =
            OUTBOUND_RELIABLE_RESERVED_CAPACITY.min(tx.max_capacity().saturating_sub(1));
        let required_capacity = reserved_capacity + 1;
        return match tx.try_reserve_many(required_capacity) {
            Ok(mut permits) => {
                permits
                    .next()
                    .expect("reserving positive capacity returns a permit")
                    .send(frame);
                true
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                tracing::debug!(connection_id, queue, "outbound queue receiver dropped");
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                tracing::debug!(
                    connection_id,
                    queue,
                    reserved_capacity,
                    "shedding best-effort outbound notification under backpressure"
                );
                false
            }
        };
    }

    match tx.try_reserve() {
        Ok(permit) => {
            permit.send(frame);
            return true;
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
            tracing::debug!(connection_id, queue, "outbound queue receiver dropped");
            return false;
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {}
    }

    match tokio::time::timeout(OUTBOUND_NOTIFICATION_MAX_WAIT, tx.reserve()).await {
        Ok(Ok(permit)) => {
            permit.send(frame);
            true
        }
        Ok(Err(_)) => {
            tracing::debug!(connection_id, queue, "outbound queue receiver dropped");
            false
        }
        Err(_) => {
            tracing::warn!(
                connection_id,
                queue,
                max_wait_ms = OUTBOUND_NOTIFICATION_MAX_WAIT.as_millis(),
                "dropping outbound notification under backpressure"
            );
            false
        }
    }
}

/// Test helper: drains [`OutboundFrame`] values into serialized JSON values.
#[doc(hidden)]
pub fn test_outbound_channel(
    capacity: usize,
) -> (
    mpsc::Sender<OutboundFrame>,
    mpsc::Receiver<serde_json::Value>,
) {
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundFrame>(capacity);
    let (json_tx, json_rx) = mpsc::channel(capacity);
    tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            let value = outbound_frame_to_value(&frame);
            let delivered = frame.delivered;
            let sent = json_tx.send(value).await.is_ok();
            if let Some(delivered) = delivered {
                let _ = delivered.send(sent);
            }
        }
    });
    (outbound_tx, json_rx)
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn notification_item_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("params")
        .and_then(|params| params.get("context"))
        .and_then(|context| context.get("item_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn notification_assistant_delta<'a>(method: &str, value: &'a serde_json::Value) -> Option<&'a str> {
    (method == "item/agentMessage/delta")
        .then(|| value.get("params")?.get("delta")?.as_str())
        .flatten()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::new();
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn outbound_frame_to_value_wraps_notifications() {
        let frame = OutboundFrame::notification(
            1,
            "session/update".to_string(),
            3,
            serde_json::json!({ "sessionId": "abc" }),
        );
        assert_eq!(
            outbound_frame_to_value(&frame),
            serde_json::json!({
                "method": "session/update",
                "params": { "sessionId": "abc" },
            })
        );
    }

    #[test]
    fn outbound_frame_to_value_passthrough_responses() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "ok": true },
        });
        let frame = OutboundFrame::json_rpc_response(1, response.clone());
        assert_eq!(outbound_frame_to_value(&frame), response);
    }

    #[tokio::test]
    async fn reliable_notification_uses_capacity_reserved_from_best_effort() {
        let capacity = OUTBOUND_RELIABLE_RESERVED_CAPACITY + 1;
        let (tx, mut rx) = mpsc::channel(capacity);
        let best_effort = serde_json::json!({
            "context": { "item_id": "delta-item" },
            "delta": "token",
        });
        assert!(
            enqueue_outbound_notification(
                &tx,
                OutboundFrame::notification(
                    1,
                    "item/agentMessage/delta".to_string(),
                    1,
                    best_effort.clone(),
                ),
                OutboundDeliveryPolicy::BestEffort,
                "test_notifications",
            )
            .await
        );
        assert!(
            !enqueue_outbound_notification(
                &tx,
                OutboundFrame::notification(
                    1,
                    "item/agentMessage/delta".to_string(),
                    2,
                    best_effort,
                ),
                OutboundDeliveryPolicy::BestEffort,
                "test_notifications",
            )
            .await
        );

        let completed = serde_json::json!({
            "context": { "item_id": "compaction-item" },
            "item": {
                "item_id": "compaction-item",
                "item_kind": "context_compaction",
                "payload": { "title": "Context compacted" },
            },
        });
        assert!(
            enqueue_outbound_notification(
                &tx,
                OutboundFrame::notification(1, "item/completed".to_string(), 3, completed.clone(),),
                OutboundDeliveryPolicy::Reliable,
                "test_notifications",
            )
            .await
        );

        let delivered = vec![
            outbound_frame_to_value(&rx.recv().await.expect("best-effort frame")),
            outbound_frame_to_value(&rx.recv().await.expect("reliable frame")),
        ];
        assert_eq!(
            delivered,
            vec![
                serde_json::json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "context": { "item_id": "delta-item" },
                        "delta": "token",
                    },
                }),
                serde_json::json!({
                    "method": "item/completed",
                    "params": completed,
                }),
            ]
        );
    }
}
