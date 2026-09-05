//! Propagate dropped tool futures (interrupts and timeouts) to the MCP server.

use rmcp::model::ServerResult;
use rmcp::service::{RequestHandle, RoleClient, ServiceError};

struct CancelOnDrop(Option<RequestHandle<RoleClient>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(request) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                if let Err(error) = request
                    .cancel(Some("tool call interrupted or timed out".into()))
                    .await
                {
                    tracing::debug!("failed to cancel MCP tool request: {error}");
                }
            });
        }
    }
}

pub(crate) async fn await_response(
    request: RequestHandle<RoleClient>,
) -> Result<ServerResult, ServiceError> {
    let mut guard = CancelOnDrop(Some(request));
    let result = (&mut guard.0.as_mut().expect("request is armed").rx)
        .await
        .map_err(|_| ServiceError::TransportClosed);
    guard.0.take();
    result?
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rmcp::model::*;
    use rmcp::service::{NotificationContext, RequestContext, RoleServer};
    use rmcp::{ServerHandler, ServiceExt};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Notify, mpsc};

    struct Server {
        started: Arc<Notify>,
        cancelled: mpsc::UnboundedSender<CancelledNotificationParam>,
    }

    impl ServerHandler for Server {
        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            self.started.notify_one();
            std::future::pending().await
        }

        async fn on_cancelled(
            &self,
            notification: CancelledNotificationParam,
            _context: NotificationContext<RoleServer>,
        ) {
            self.cancelled.send(notification).unwrap();
        }
    }

    #[tokio::test]
    async fn dropping_response_future_cancels_matching_request() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let started = Arc::new(Notify::new());
        let (cancelled, mut notifications) = mpsc::unbounded_channel();
        let server = Server {
            started: Arc::clone(&started),
            cancelled,
        };
        let server_task = tokio::spawn(async move { server.serve(server_io).await.unwrap() });
        let client = ().serve(client_io).await.unwrap();
        let server = server_task.await.unwrap();
        let request = client
            .peer()
            .send_request_with_option(
                ClientRequest::CallToolRequest(CallToolRequest {
                    method: Default::default(),
                    params: CallToolRequestParams {
                        meta: None,
                        name: "blocking".into(),
                        arguments: None,
                        task: None,
                    },
                    extensions: Default::default(),
                }),
                Default::default(),
            )
            .await
            .unwrap();
        let id = request.id.clone();
        let mut response = Box::pin(await_response(request));
        tokio::select! {
            biased;
            _ = &mut response => panic!("blocking call completed"),
            _ = started.notified() => {}
        }
        drop(response);
        let notification = tokio::time::timeout(Duration::from_secs(5), notifications.recv())
            .await
            .unwrap();
        assert_eq!(
            notification,
            Some(CancelledNotificationParam {
                request_id: id,
                reason: Some("tool call interrupted or timed out".into()),
            })
        );
        client.cancel().await.unwrap();
        server.cancel().await.unwrap();
    }
}
