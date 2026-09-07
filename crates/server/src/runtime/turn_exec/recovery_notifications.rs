//! Native recovery notifications emitted after execution ownership changes.

use devo_core::SessionId;
use devo_protocol::native::event::{ServerNotification, StreamSelector};

use super::super::ServerRuntime;
use super::super::outbound::{
    OutboundDeliveryPolicy, OutboundFrame, enqueue_outbound_notification,
};

impl ServerRuntime {
    pub(crate) async fn broadcast_recovery_state(&self, session_id: SessionId) {
        match self.turn_recovery(session_id).await {
            Ok(recovery) => {
                self.broadcast_recovery_notification(
                    session_id,
                    ServerNotification::TurnRecoveryUpdated {
                        session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(
                            session_id.into(),
                        ),
                        recovery,
                    },
                )
                .await
            }
            Err(error) => tracing::warn!(%session_id, %error, "cannot read recovery state"),
        }
    }

    pub(crate) async fn broadcast_recovery_notification(
        &self,
        session_id: SessionId,
        notification: ServerNotification,
    ) {
        let value = serde_json::to_value(notification).expect("recovery notification");
        let method = value["method"]
            .as_str()
            .expect("notification method")
            .to_owned();
        let params = value["params"].clone();
        let session_id_string = session_id.to_string();
        let recipients = {
            let mut connections = self.connections.lock().await;
            connections.iter_mut().filter_map(|(id, connection)| {
                let subscribed = connection.event_selectors.iter().any(|selector| {
                    matches!(selector, StreamSelector::Session { session_id: selected } if selected.as_str() == session_id_string)
                });
                if connection.protocol != Some(super::super::connection::ConnectionProtocol::Native)
                    || (!subscribed && !connection.should_deliver(&method, Some(session_id), &std::collections::HashMap::new()))
                { return None; }
                let seq = connection.next_seq();
                Some((connection.outbound_tx.clone(), OutboundFrame::notification(*id, method.clone(), seq, params.clone())))
            }).collect::<Vec<_>>()
        };
        for (sender, frame) in recipients {
            let _ = enqueue_outbound_notification(
                &sender,
                frame,
                OutboundDeliveryPolicy::Reliable,
                "connection_notifications",
            )
            .await;
        }
    }
}
