use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use devo_core::tools::ClientFilesystem;
use devo_core::tools::ClientTextFileRead;
use devo_core::tools::ClientTextFileWrite;
use tokio_util::sync::CancellationToken;

use super::*;

const ACP_FS_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpFsCapability {
    ReadTextFile,
    WriteTextFile,
}

impl ServerRuntime {
    async fn read_acp_client_text_file_with_cancel(
        &self,
        session_id: SessionId,
        path: PathBuf,
        line: Option<u64>,
        limit: Option<u64>,
        cancel_token: CancellationToken,
    ) -> Result<ClientTextFileRead, ToolCallError> {
        if !path.is_absolute() {
            return Err(ToolCallError::InvalidInput(
                "fs/read_text_file path must be absolute".to_string(),
            ));
        }
        let Some(connection_id) = self
            .active_acp_connection_with_fs_capability(session_id, AcpFsCapability::ReadTextFile)
            .await
        else {
            return Ok(ClientTextFileRead::Unsupported);
        };
        let params = crate::AcpFsReadTextFileParams {
            session_id,
            path,
            line: line.and_then(|line| u32::try_from(line).ok()),
            limit: limit.and_then(|limit| u32::try_from(limit).ok()),
            meta: None,
        };
        let response = self
            .send_request_to_connection_with_timeout(
                connection_id,
                crate::ACP_FS_READ_TEXT_FILE_METHOD,
                serde_json::to_value(params).expect("serialize ACP fs/read_text_file params"),
                ACP_FS_CLIENT_REQUEST_TIMEOUT,
                cancel_token,
            )
            .await
            .map_err(|error| {
                ToolCallError::ExecutionFailed(format!("client fs/read_text_file failed: {error}"))
            })?;
        let response = serde_json::from_value::<crate::AcpFsReadTextFileResult>(response).map_err(
            |error| {
                ToolCallError::ExecutionFailed(format!(
                    "invalid fs/read_text_file response: {error}"
                ))
            },
        )?;
        Ok(ClientTextFileRead::Content(response.content))
    }

    async fn write_acp_client_text_file_with_cancel(
        &self,
        session_id: SessionId,
        path: PathBuf,
        content: String,
        cancel_token: CancellationToken,
    ) -> Result<ClientTextFileWrite, ToolCallError> {
        if !path.is_absolute() {
            return Err(ToolCallError::InvalidInput(
                "fs/write_text_file path must be absolute".to_string(),
            ));
        }
        let Some(connection_id) = self
            .active_acp_connection_with_fs_capability(session_id, AcpFsCapability::WriteTextFile)
            .await
        else {
            return Ok(ClientTextFileWrite::Unsupported);
        };
        let params = crate::AcpFsWriteTextFileParams {
            session_id,
            path,
            content,
            meta: None,
        };
        self.send_request_to_connection_with_timeout(
            connection_id,
            crate::ACP_FS_WRITE_TEXT_FILE_METHOD,
            serde_json::to_value(params).expect("serialize ACP fs/write_text_file params"),
            ACP_FS_CLIENT_REQUEST_TIMEOUT,
            cancel_token,
        )
        .await
        .map_err(|error| {
            ToolCallError::ExecutionFailed(format!("client fs/write_text_file failed: {error}"))
        })?;
        Ok(ClientTextFileWrite::Written)
    }

    async fn active_acp_connection_with_fs_capability(
        &self,
        session_id: SessionId,
        capability: AcpFsCapability,
    ) -> Option<u64> {
        let connection_id = self.active_turns.active_connection_id(session_id).await?;
        let connections = self.connections.lock().await;
        let connection = connections.get(&connection_id)?;
        if connection.protocol != Some(super::connection::ConnectionProtocol::Acp) {
            return None;
        }
        client_capabilities_support_fs(&connection.acp_client_capabilities, capability)
            .then_some(connection_id)
    }
}

#[async_trait::async_trait]
impl ClientFilesystem for ServerRuntime {
    async fn read_text_file(
        self: Arc<Self>,
        session_id: String,
        path: PathBuf,
        line: Option<u64>,
        limit: Option<u64>,
        cancel_token: CancellationToken,
    ) -> Result<ClientTextFileRead, ToolCallError> {
        let session_id = SessionId::try_from(session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.read_acp_client_text_file_with_cancel(session_id, path, line, limit, cancel_token)
            .await
    }

    async fn write_text_file(
        self: Arc<Self>,
        session_id: String,
        path: PathBuf,
        content: String,
        cancel_token: CancellationToken,
    ) -> Result<ClientTextFileWrite, ToolCallError> {
        let session_id = SessionId::try_from(session_id.as_str())
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        self.write_acp_client_text_file_with_cancel(session_id, path, content, cancel_token)
            .await
    }
}

fn client_capabilities_support_fs(
    client_capabilities: &crate::AcpClientCapabilities,
    capability: AcpFsCapability,
) -> bool {
    match capability {
        AcpFsCapability::ReadTextFile => client_capabilities.fs.read_text_file,
        AcpFsCapability::WriteTextFile => client_capabilities.fs.write_text_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_capabilities_gate_fs_methods() {
        let capabilities = crate::AcpClientCapabilities {
            fs: crate::AcpFileSystemCapabilities {
                read_text_file: true,
                write_text_file: false,
                meta: None,
            },
            terminal: false,
            session: None,
            meta: None,
        };

        assert!(client_capabilities_support_fs(
            &capabilities,
            AcpFsCapability::ReadTextFile
        ));
        assert!(!client_capabilities_support_fs(
            &capabilities,
            AcpFsCapability::WriteTextFile
        ));
        assert!(!client_capabilities_support_fs(
            &crate::AcpClientCapabilities::default(),
            AcpFsCapability::ReadTextFile
        ));
    }
}
