use super::*;

const ACP_SESSION_LIST_CURSOR_PREFIX: &str = "devo-session-list-v1:";

impl ServerRuntime {
    pub(super) async fn validate_acp_existing_session_cwd(
        &self,
        method: &str,
        session_id: SessionId,
        cwd: &Path,
    ) -> Result<(), (AcpErrorCode, String)> {
        let stored_cwd = self
            .resolve_indexed_session_cwd(session_id)
            .map_err(|error| (AcpErrorCode::ServerError, error.to_string()))?;
        let Some(stored_cwd) = stored_cwd else {
            return Err((
                AcpErrorCode::ServerError,
                "session does not exist".to_string(),
            ));
        };
        if stored_cwd != cwd {
            return Err((
                AcpErrorCode::InvalidParams,
                format!("{method} cwd does not match the stored session cwd"),
            ));
        }
        Ok(())
    }

    fn resolve_indexed_session_cwd(
        &self,
        session_id: SessionId,
    ) -> anyhow::Result<Option<PathBuf>> {
        self.rollout_store
            .resolve_indexed_session_cwd(&self.deps.db, &session_id)
    }

    pub(super) async fn acp_session_tool_registry(
        &self,
        method: &str,
        mcp_servers: &[AcpMcpServer],
        cwd: &Path,
    ) -> Result<Option<Arc<devo_core::tools::ToolRegistry>>, String> {
        if mcp_servers.is_empty() {
            return Ok(None);
        }

        let mcp_config = acp_mcp_config(method, mcp_servers)?;
        let user_config_dir = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .user_config_dir()
            .to_path_buf();
        let config_store = AppConfigStore::load(user_config_dir, Some(cwd))
            .map_err(|error| format!("failed to load {method} workspace config: {error}"))?;
        let (tool_plan, oauth_store_mode) = {
            let config = config_store.effective_config();
            (
                ToolPlanConfig::from_app_config(config),
                config.mcp_oauth_credentials_store.unwrap_or_default(),
            )
        };
        let mcp_manager = Arc::new(RmcpMcpManager::new(
            mcp_config.with_code_search_workspace_cwd(cwd.to_path_buf()),
            oauth_store_mode,
        ));
        let registry =
            devo_core::tools::handlers::build_registry_from_plan_with_mcp(&tool_plan, mcp_manager)
                .await;

        Ok(Some(Arc::new(registry)))
    }
    pub(super) async fn apply_acp_session_additional_directories(
        &self,
        session_id: SessionId,
        additional_directories: Vec<PathBuf>,
    ) -> Result<SessionMetadata, String> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        let _state_change_guard = session_arc.lock_state_change().await;
        let snapshot = session_arc
            .hook_context_snapshot()
            .await
            .ok_or_else(|| "session actor unavailable".to_string())?;
        if snapshot.summary.additional_directories == additional_directories {
            return Ok(snapshot.summary);
        }

        let updated_at = Utc::now();
        let mut updated_summary = snapshot.summary.clone();
        updated_summary.additional_directories = additional_directories.clone();
        updated_summary.updated_at = updated_at;

        // Apply new permission roots so tool authorization uses the updated workspace.
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            snapshot.config.permission_profile.preset,
            updated_summary.cwd.clone(),
        )
        .with_additional_roots(additional_directories.clone());
        if !session_arc.apply_permission_profile(profile).await {
            return Err("failed to apply updated permission profile".to_string());
        }
        session_arc.update_summary(updated_summary.clone()).await;

        if let Some(mut record) = snapshot.record {
            record.additional_directories = additional_directories;
            record.updated_at = updated_at;
            if let Err(error) = self.rollout_store.append_session_meta(&record) {
                return Err(format!(
                    "failed to persist ACP session additional directories: {error}"
                ));
            }
        }

        if !updated_summary.ephemeral
            && let Err(error) = self.deps.db.upsert_session(&updated_summary, None)
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to persist ACP session additional directories"
            );
        }
        Ok(updated_summary)
    }

    pub(super) async fn send_acp_history_updates(
        &self,
        connection_id: u64,
        session_id: SessionId,
        history_items: &[SessionHistoryItem],
        history_limit: Option<usize>,
    ) {
        let replay_start = history_replay_start(history_items, history_limit);
        let mut parent_message_id: Option<String> = None;
        for (index, item) in history_items.iter().enumerate().skip(replay_start) {
            if item.kind == SessionHistoryItemKind::User {
                parent_message_id = Some(format!("history-{index}"));
            }
            let Some(update) =
                acp_update_from_history_item(index, item, parent_message_id.as_deref())
            else {
                continue;
            };
            let notification = AcpClientNotification::new(
                ACP_SESSION_UPDATE_METHOD,
                AcpSessionNotification {
                    session_id,
                    update,
                    meta: None,
                },
            );
            self.send_raw_to_connection(
                connection_id,
                serde_json::to_value(notification).expect("serialize ACP history notification"),
            )
            .await;
        }
    }
}

pub(super) fn history_limit_from_meta(meta: &Option<AcpMeta>) -> Option<usize> {
    let value = meta.as_ref()?.get("devo/historyLimit")?;
    let limit = value.as_u64()?;
    usize::try_from(limit).ok().filter(|limit| *limit > 0)
}

fn history_replay_start(
    history_items: &[SessionHistoryItem],
    history_limit: Option<usize>,
) -> usize {
    let Some(limit) = history_limit else {
        return 0;
    };
    if history_items.len() <= limit {
        return 0;
    }

    let mut start = history_items.len() - limit;
    while start > 0 && history_items[start].kind != SessionHistoryItemKind::User {
        start -= 1;
    }
    start
}

pub(super) fn validate_acp_session_roots(
    method: &str,
    cwd: &Path,
    additional_directories: &[PathBuf],
) -> Result<(), String> {
    if !cwd.is_absolute() {
        return Err(format!("{method} cwd must be an absolute path"));
    }
    if let Some((index, _)) = additional_directories
        .iter()
        .enumerate()
        .find(|(_, directory)| !directory.is_absolute())
    {
        return Err(format!(
            "{method} additionalDirectories[{index}] must be an absolute path"
        ));
    }
    Ok(())
}

pub(super) fn encode_session_list_cursor(start: usize) -> String {
    URL_SAFE_NO_PAD.encode(format!("{ACP_SESSION_LIST_CURSOR_PREFIX}{start}"))
}

pub(super) fn decode_session_list_cursor(cursor: &str) -> Result<usize, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| "session/list cursor is invalid".to_string())?;
    let value =
        std::str::from_utf8(&bytes).map_err(|_| "session/list cursor is invalid".to_string())?;
    let start = value
        .strip_prefix(ACP_SESSION_LIST_CURSOR_PREFIX)
        .ok_or_else(|| "session/list cursor is invalid".to_string())?;
    start
        .parse::<usize>()
        .map_err(|_| "session/list cursor is invalid".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn history_item(kind: SessionHistoryItemKind) -> SessionHistoryItem {
        SessionHistoryItem::new(None, kind, String::new(), String::new())
    }

    #[test]
    fn history_limit_meta_accepts_positive_integer() {
        let mut meta = AcpMeta::new();
        meta.insert("devo/historyLimit".to_string(), serde_json::json!(30));

        assert_eq!(history_limit_from_meta(&Some(meta)), Some(30));
    }

    #[test]
    fn history_replay_start_expands_to_user_turn_boundary() {
        let items = vec![
            history_item(SessionHistoryItemKind::User),
            history_item(SessionHistoryItemKind::Assistant),
            history_item(SessionHistoryItemKind::ToolCall),
            history_item(SessionHistoryItemKind::User),
            history_item(SessionHistoryItemKind::Assistant),
            history_item(SessionHistoryItemKind::ToolResult),
        ];

        assert_eq!(history_replay_start(&items, Some(2)), 3);
    }

    #[test]
    fn history_replay_start_keeps_full_history_when_limit_covers_items() {
        let items = vec![
            history_item(SessionHistoryItemKind::User),
            history_item(SessionHistoryItemKind::Assistant),
        ];

        assert_eq!(history_replay_start(&items, Some(2)), 0);
    }
}
