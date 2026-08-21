use anyhow::{Context, Result};
use devo_protocol::{
    ACP_SESSION_UPDATE_METHOD, AcpSessionNotification, AcpSessionUpdate, DEVO_TURN_USAGE_META,
    ServerEvent, SessionCompactionCompletedPayload, SessionCompactionFailedPayload,
    SessionCompactionStartedPayload, TurnEventPayload, TurnFailedPayload, TurnUsageUpdatedPayload,
};

use crate::client_core::ServerNotificationMessage;

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    TurnStarted(TurnEventPayload),
    TurnCompleted(TurnEventPayload),
    TurnFailed(TurnFailedPayload),
    TurnUsageUpdated(TurnUsageUpdatedPayload),
    SessionCompactionStarted(SessionCompactionStartedPayload),
    SessionCompactionCompleted(SessionCompactionCompletedPayload),
    SessionCompactionFailed(SessionCompactionFailedPayload),
    Other(ServerNotificationMessage),
}

pub fn client_event_from_notification(
    notification: &ServerNotificationMessage,
) -> Result<Option<ClientEvent>> {
    if notification.method == ACP_SESSION_UPDATE_METHOD {
        let acp_notification: AcpSessionNotification =
            match serde_json::from_value(notification.params.clone()) {
                Ok(notification) => notification,
                Err(_) => return Ok(Some(ClientEvent::Other(notification.clone()))),
            };
        if let AcpSessionUpdate::UsageUpdate { meta, .. } = acp_notification.update
            && let Some(payload) = meta
                .as_ref()
                .and_then(|meta| meta.get(DEVO_TURN_USAGE_META))
        {
            return Ok(Some(ClientEvent::TurnUsageUpdated(
                serde_json::from_value(payload.clone()).context("decode Devo usage metadata")?,
            )));
        }
        return Ok(None);
    }

    let event: ServerEvent = match serde_json::from_value(notification.params.clone()) {
        Ok(event) => event,
        Err(_) => return Ok(Some(ClientEvent::Other(notification.clone()))),
    };
    Ok(Some(match event {
        ServerEvent::TurnStarted(payload) => ClientEvent::TurnStarted(payload),
        ServerEvent::TurnCompleted(payload) => ClientEvent::TurnCompleted(payload),
        ServerEvent::TurnFailed(payload) => ClientEvent::TurnFailed(payload),
        ServerEvent::TurnUsageUpdated(payload) => ClientEvent::TurnUsageUpdated(payload),
        ServerEvent::SessionCompactionStarted(payload) => {
            ClientEvent::SessionCompactionStarted(payload)
        }
        ServerEvent::SessionCompactionCompleted(payload) => {
            ClientEvent::SessionCompactionCompleted(payload)
        }
        ServerEvent::SessionCompactionFailed(payload) => {
            ClientEvent::SessionCompactionFailed(payload)
        }
        _ => ClientEvent::Other(notification.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use devo_protocol::{SessionId, TurnId, TurnUsage, TurnUsageUpdatedPayload};

    use super::ClientEvent;
    use super::client_event_from_notification;
    use crate::ServerNotificationMessage;

    #[test]
    fn client_event_usage_normalizes_devo_notification() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let payload = TurnUsageUpdatedPayload {
            session_id,
            turn_id,
            usage: TurnUsage {
                input_tokens: 700,
                output_tokens: 30,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(730),
            },
            total_input_tokens: 1_300,
            total_output_tokens: 80,
            total_tokens: 1_380,
            total_cache_read_tokens: 0,
            last_query_input_tokens: 700,
            context_window: Some(200_000),
        };
        let notification = ServerNotificationMessage {
            method: "turn/usage/updated".to_string(),
            params: serde_json::to_value(devo_protocol::ServerEvent::TurnUsageUpdated(
                payload.clone(),
            ))
            .expect("serialize usage event"),
        };

        assert_eq!(
            client_event_from_notification(&notification).expect("decode client event"),
            Some(ClientEvent::TurnUsageUpdated(payload))
        );
    }

    #[test]
    fn client_event_usage_normalizes_acp_metadata() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let payload = TurnUsageUpdatedPayload {
            session_id,
            turn_id,
            usage: TurnUsage {
                input_tokens: 700,
                output_tokens: 30,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(730),
            },
            total_input_tokens: 1_300,
            total_output_tokens: 80,
            total_tokens: 1_380,
            total_cache_read_tokens: 0,
            last_query_input_tokens: 700,
            context_window: Some(200_000),
        };
        let notification = ServerNotificationMessage {
            method: devo_protocol::ACP_SESSION_UPDATE_METHOD.to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "usage_update",
                    "used": 1_380,
                    "size": 200_000,
                    "_meta": {
                        "devo/turnUsage": payload,
                    },
                },
            }),
        };

        assert_eq!(
            client_event_from_notification(&notification).expect("decode client event"),
            Some(ClientEvent::TurnUsageUpdated(payload))
        );
    }
}
