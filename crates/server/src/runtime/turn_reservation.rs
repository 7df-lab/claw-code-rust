use super::*;

impl ServerRuntime {
    pub(super) async fn subscribe_terminal_turn_status(
        &self,
        turn_id: TurnId,
    ) -> oneshot::Receiver<TerminalTurnSnapshot> {
        let (sender, receiver) = oneshot::channel();
        self.acp_prompt_waiters
            .lock()
            .await
            .entry(turn_id)
            .or_default()
            .push(sender);
        receiver
    }

    pub(super) async fn recent_terminal_turn_status(
        &self,
        turn_id: TurnId,
    ) -> Option<TerminalTurnSnapshot> {
        self.terminal_turn_statuses
            .lock()
            .await
            .iter()
            .rev()
            .find_map(|(completed_turn_id, status)| {
                (*completed_turn_id == turn_id).then(|| status.clone())
            })
    }

    pub(super) async fn record_terminal_turn_status(
        &self,
        turn_id: TurnId,
        snapshot: TerminalTurnSnapshot,
    ) {
        {
            let mut statuses = self.terminal_turn_statuses.lock().await;
            statuses.retain(|(completed_turn_id, _)| *completed_turn_id != turn_id);
            statuses.push_back((turn_id, snapshot.clone()));
            while statuses.len() > TERMINAL_TURN_STATUS_LIMIT {
                statuses.pop_front();
            }
        }

        let waiters = self.acp_prompt_waiters.lock().await.remove(&turn_id);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                let _ = waiter.send(snapshot.clone());
            }
        }
    }

    pub(super) async fn runtime_active_turn_id(&self, session_id: SessionId) -> Option<TurnId> {
        self.active_turns.active_turn_id(session_id).await
    }

    pub(super) async fn register_runtime_active_turn(
        &self,
        session_id: SessionId,
        turn: TurnMetadata,
    ) {
        self.active_turns
            .register_turn_metadata(session_id, turn)
            .await;
    }

    pub(super) async fn clear_active_turn_interrupt_handles(&self, session_id: SessionId) {
        self.active_turns.clear_interrupt_handles(session_id).await;
    }

    pub(super) async fn clear_active_turn_runtime_handles(&self, session_id: SessionId) {
        self.active_turns.clear_runtime_handles(session_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::PresetModelCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::ErrorResponse;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::StreamEvent;
    use devo_protocol::SuccessResponse;
    use devo_provider::ModelProviderSDK;
    use devo_provider::SingleProviderRouter;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("noop provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            anyhow::bail!("noop provider does not support streaming")
        }

        fn name(&self) -> &str {
            "noop-provider"
        }
    }

    fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(
            crate::db::Database::open(data_root.join("turn_reservation.db"))
                .expect("open test database"),
        );
        ServerRuntime::new(
            data_root.to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
                crate::empty_mcp_manager(),
                "test-model".to_string(),
                Arc::new(PresetModelCatalog::default()),
                Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                    bundled: Some(BundledSkillsConfig { enabled: false }),
                    ..SkillsConfig::default()
                })),
                devo_core::AgentsMdConfig::default(),
                db,
                Arc::new(std::sync::Mutex::new(
                    AppConfigStore::load(data_root.to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
        )
    }

    async fn start_session(runtime: &Arc<ServerRuntime>, cwd: std::path::PathBuf) -> SessionId {
        let value = runtime
            .start_session_with_registry(
                /*connection_id*/ 1,
                serde_json::json!(1),
                SessionStartParams {
                    cwd,
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        let response: SuccessResponse<SessionStartResult> =
            serde_json::from_value(value).expect("session start response");
        response.result.session.session_id
    }

    #[tokio::test]
    async fn reject_active_turn_policy_does_not_enqueue_input() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = start_session(&runtime, data_root.path().to_path_buf()).await;
        let session_handle = runtime.session(session_id).await.expect("session");
        let reservation = session_handle
            .turn_reservation_snapshot()
            .await
            .expect("turn reservation snapshot");
        let turn_config = reservation.runtime_context.resolve_turn_config(None, None);
        let active_turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence: 1,
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: Utc::now(),
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        session_handle
            .begin_active_turn(active_turn, turn_config)
            .await;

        let value = runtime
            .handle_turn_start_with_queue_policy(
                None,
                serde_json::json!(2),
                TurnStartParams {
                    session_id,
                    input: vec![devo_protocol::InputItem::Text {
                        text: "must not queue".to_string(),
                    }],
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    execution_mode: devo_protocol::TurnExecutionMode::Regular,
                },
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let response: ErrorResponse = serde_json::from_value(value).expect("error response");
        let queued_len = session_handle
            .pending_queue_snapshot()
            .await
            .map(|snapshot| snapshot.pending_count)
            .unwrap_or(0);

        assert_eq!(response.error.code, ProtocolErrorCode::TurnAlreadyRunning);
        assert_eq!(queued_len, 0);

        Ok(())
    }
}
