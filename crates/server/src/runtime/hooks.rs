use serde_json::Map;
use serde_json::Value;

use super::*;

use crate::runtime::session_actor::SessionActorState;
use crate::runtime::session_actor::snapshots::HookContextSnapshot;

impl ServerRuntime {
    pub(crate) fn hook_runner(&self) -> Option<devo_core::HookRunner> {
        let config = {
            let config_store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            config_store.effective_config().hooks.clone()
        };
        (!config.is_empty()).then(|| devo_core::HookRunner::new(config))
    }

    pub(crate) fn hook_context_from_actor_state(
        state: &SessionActorState,
        session_id: SessionId,
    ) -> Option<devo_core::HookRuntimeContext> {
        let runner = state.runtime_context.hook_runner()?;
        let transcript_path = state
            .record
            .as_ref()
            .map(|record| record.rollout_path.display().to_string())
            .unwrap_or_default();
        let permission_mode = Some(permission_mode_label(state.config.permission_mode));
        let agent_id = state
            .summary
            .parent_session_id
            .is_some()
            .then(|| session_id.to_string());
        let agent_type = state
            .summary
            .agent_role
            .clone()
            .or_else(|| state.summary.agent_nickname.clone());
        Some(devo_core::HookRuntimeContext {
            runner,
            base: devo_core::HookBaseInput {
                session_id: session_id.to_string(),
                transcript_path,
                cwd: state.summary.cwd.clone(),
                permission_mode,
                agent_id,
                agent_type,
            },
        })
    }

    pub(crate) async fn hook_context_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<devo_core::HookRuntimeContext> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Self::hook_runtime_context_from_snapshot(session_id, &inline.hook_context);
            }
        }
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let snapshot = session_handle.hook_context_snapshot().await?;
        Self::hook_runtime_context_from_snapshot(session_id, &snapshot)
    }

    fn hook_runtime_context_from_snapshot(
        session_id: SessionId,
        snapshot: &HookContextSnapshot,
    ) -> Option<devo_core::HookRuntimeContext> {
        let runner = snapshot.runtime_context.hook_runner()?;
        let transcript_path = snapshot
            .record
            .as_ref()
            .map(|record| record.rollout_path.display().to_string())
            .unwrap_or_default();
        let permission_mode = Some(permission_mode_label(snapshot.config.permission_mode));
        let agent_id = snapshot
            .summary
            .parent_session_id
            .is_some()
            .then(|| session_id.to_string());
        let agent_type = snapshot
            .summary
            .agent_role
            .clone()
            .or_else(|| snapshot.summary.agent_nickname.clone());
        Some(devo_core::HookRuntimeContext {
            runner,
            base: devo_core::HookBaseInput {
                session_id: session_id.to_string(),
                transcript_path,
                cwd: snapshot.summary.cwd.clone(),
                permission_mode,
                agent_id,
                agent_type,
            },
        })
    }

    pub(crate) async fn run_session_hook(
        &self,
        session_id: SessionId,
        event: devo_core::HookEvent,
        extra: Map<String, Value>,
    ) -> devo_core::HookRunReport {
        let Some(context) = self.hook_context_for_session(session_id).await else {
            return devo_core::HookRunReport::default();
        };
        let input = devo_core::HookInput::new(&context.base, event, extra);
        context.runner.run(input).await
    }

    pub(crate) async fn run_session_hook_for_actor_state(
        &self,
        state: &SessionActorState,
        session_id: SessionId,
        event: devo_core::HookEvent,
        extra: Map<String, Value>,
    ) -> devo_core::HookRunReport {
        let Some(context) = Self::hook_context_from_actor_state(state, session_id) else {
            return devo_core::HookRunReport::default();
        };
        let input = devo_core::HookInput::new(&context.base, event, extra);
        context.runner.run(input).await
    }

    pub(crate) async fn run_global_hook(
        &self,
        event: devo_core::HookEvent,
        extra: Map<String, Value>,
    ) -> devo_core::HookRunReport {
        let Some(runner) = self.hook_runner() else {
            return devo_core::HookRunReport::default();
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let base = devo_core::HookBaseInput {
            session_id: String::new(),
            transcript_path: String::new(),
            cwd,
            permission_mode: None,
            agent_id: None,
            agent_type: None,
        };
        runner
            .run(devo_core::HookInput::new(&base, event, extra))
            .await
    }

    pub(crate) async fn config_change_hook_block_reason(
        &self,
        source: &str,
        file_path: Option<String>,
    ) -> Option<String> {
        let mut extra = Map::from_iter([("source".to_string(), Value::String(source.to_string()))]);
        if let Some(file_path) = file_path {
            extra.insert("file_path".to_string(), Value::String(file_path));
        }
        self.run_global_hook(devo_core::HookEvent::ConfigChange, extra)
            .await
            .first_blocking_reason()
            .map(str::to_string)
    }
}

pub(crate) fn permission_mode_label(mode: devo_safety::PermissionMode) -> String {
    match mode {
        devo_safety::PermissionMode::Yolo => "yolo",
        devo_safety::PermissionMode::Interactive => "interactive",
        devo_safety::PermissionMode::Deny => "deny",
    }
    .to_string()
}
