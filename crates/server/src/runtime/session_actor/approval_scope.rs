use std::path::{Path, PathBuf};

use devo_protocol::ApprovalScopeValue;
use devo_safety::RuntimePermissionProfile;

use crate::execution::ApprovalGrantCache;
use crate::execution::PendingApproval;

/// Applies an approval scope into session/turn grant caches.
pub(crate) fn apply_approval_scope_to_state(
    session_cache: &mut ApprovalGrantCache,
    turn_cache: &mut ApprovalGrantCache,
    scope: &ApprovalScopeValue,
    pending: &PendingApproval,
) {
    match scope {
        ApprovalScopeValue::Once => {}
        ApprovalScopeValue::Turn => {
            turn_cache.tools.insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::Session => {
            // Prefer exact command + cwd. Fall back to a
            // generalized pattern only when the exact command is unavailable,
            // then to a whole-tool grant for non-shell tools.
            if let Some(command) = pending.command.as_ref() {
                session_cache
                    .exact_commands
                    .insert((command.clone(), pending.cwd.clone()));
            } else if let Some(pattern) = pending.command_pattern.clone() {
                session_cache.command_patterns.insert(pattern);
            } else {
                session_cache.tools.insert(pending.tool_name.clone());
            }
            if let Some(path) = pending.path.as_ref() {
                insert_path_prefix_grant(session_cache, pending.resource.as_ref(), path);
            }
        }
        ApprovalScopeValue::PathPrefix => {
            if let Some(path) = pending.path.as_ref() {
                // Session-scoped so "don't ask again for these files" lasts for
                // the rest of the conversation (session-scoped file approval).
                insert_path_prefix_grant(session_cache, pending.resource.as_ref(), path);
            }
        }
        ApprovalScopeValue::Host => {
            if let Some(host) = pending.host.clone() {
                session_cache.hosts.insert(host);
            }
        }
        ApprovalScopeValue::Tool => {
            turn_cache.tools.insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::CommandPrefix => {
            if let Some(command_prefix) = pending.command_prefix.clone() {
                session_cache.command_prefixes.insert(command_prefix);
            }
        }
        ApprovalScopeValue::CommandPrefixPersist => {
            if let Some(command_prefix) = pending.command_prefix.clone() {
                session_cache.command_prefixes.insert(command_prefix);
            }
        }
    }
    if pending.requests_escalation
        && matches!(scope, ApprovalScopeValue::Session)
        && let Some(key) = crate::execution::sandbox_bypass_key_from_pending(pending)
    {
        session_cache.sandbox_bypass_commands.insert(key);
    }
}

/// Grants PathPrefix/Session path roots onto a runtime permission profile.
pub(crate) fn apply_path_scope_to_permission_profile(
    profile: &mut RuntimePermissionProfile,
    scope: &ApprovalScopeValue,
    pending: &PendingApproval,
) {
    if !matches!(
        scope,
        ApprovalScopeValue::PathPrefix | ApprovalScopeValue::Session
    ) {
        return;
    }
    let Some(path) = pending.path.as_ref() else {
        return;
    };
    let grant = path_prefix_grant_root(path);
    match pending.resource.as_ref() {
        Some(devo_safety::ResourceKind::FileWrite) => {
            profile.grant_writable_root(grant);
        }
        Some(devo_safety::ResourceKind::FileRead) | Some(_) | None => {
            // Read (and unknown) approvals must not elevate write roots.
            profile.grant_readable_root(grant);
        }
    }
}

fn insert_path_prefix_grant(
    cache: &mut ApprovalGrantCache,
    resource: Option<&devo_safety::ResourceKind>,
    path: &Path,
) {
    let grant = path_prefix_grant_root(path);
    match resource {
        Some(devo_safety::ResourceKind::FileWrite) => {
            cache.write_path_prefixes.insert(grant);
        }
        Some(devo_safety::ResourceKind::FileRead) => {
            cache.read_path_prefixes.insert(grant);
        }
        // Unknown / non-file resources: do not elevate write rights.
        Some(_) | None => {
            cache.read_path_prefixes.insert(grant);
        }
    }
}

pub(crate) fn path_prefix_grant_root(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devo_protocol::ApprovalScopeValue;
    use pretty_assertions::assert_eq;

    use super::apply_approval_scope_to_state;
    use super::apply_path_scope_to_permission_profile;
    use super::path_prefix_grant_root;
    use devo_safety::PermissionPreset;
    use devo_safety::RuntimePermissionProfile;

    #[test]
    fn command_prefix_persist_scope_stores_prefix_in_session_cache() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let mut pending = pending_approval(/*command_pattern*/ None);
        pending.command_prefix = Some(vec!["git".to_string(), "pull".to_string()]);

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::CommandPrefixPersist,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .command_prefixes
            .insert(vec!["git".to_string(), "pull".to_string()]);
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn host_scope_stores_host_in_session_cache() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let pending = crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: "fetch".to_string(),
            resource: Some(devo_safety::ResourceKind::Network),
            path: None,
            host: Some("api.example.com".to_string()),
            command_prefix: None,
            command_pattern: None,
            requests_escalation: false,
            command: None,
            cwd: PathBuf::from("/workspace"),
            sandbox_permissions: String::new(),
            tx,
        };

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Host,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .hosts
            .insert("api.example.com".to_string());
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    fn pending_approval(command_pattern: Option<Vec<String>>) -> crate::execution::PendingApproval {
        pending_approval_with_escalation(command_pattern, false, None)
    }

    fn pending_approval_with_escalation(
        command_pattern: Option<Vec<String>>,
        requests_escalation: bool,
        command: Option<String>,
    ) -> crate::execution::PendingApproval {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: "shell_command".to_string(),
            resource: Some(devo_safety::ResourceKind::ShellExec),
            path: None,
            host: None,
            command_prefix: None,
            command_pattern,
            requests_escalation,
            command,
            cwd: PathBuf::from("/workspace"),
            sandbox_permissions: if requests_escalation {
                "require_escalated".to_string()
            } else {
                String::new()
            },
            tx,
        }
    }

    #[test]
    fn path_prefix_scope_stores_parent_directory_for_files() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let file_path = PathBuf::from("/workspace/src/main.rs");
        let pending = crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: "write".to_string(),
            resource: Some(devo_safety::ResourceKind::FileWrite),
            path: Some(file_path.clone()),
            host: None,
            command_prefix: None,
            command_pattern: None,
            requests_escalation: false,
            command: None,
            cwd: PathBuf::from("/workspace"),
            sandbox_permissions: String::new(),
            tx,
        };

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::PathPrefix,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .write_path_prefixes
            .insert(PathBuf::from("/workspace/src"));
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn path_prefix_scope_stores_read_grants_separately() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let file_path = PathBuf::from("/workspace/src/main.rs");
        let pending = crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: "read".to_string(),
            resource: Some(devo_safety::ResourceKind::FileRead),
            path: Some(file_path),
            host: None,
            command_prefix: None,
            command_pattern: None,
            requests_escalation: false,
            command: None,
            cwd: PathBuf::from("/workspace"),
            sandbox_permissions: String::new(),
            tx,
        };

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::PathPrefix,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .read_path_prefixes
            .insert(PathBuf::from("/workspace/src"));
        assert_eq!(session_cache, expected_session_cache);
        assert!(session_cache.write_path_prefixes.is_empty());
    }

    #[test]
    fn session_scope_stores_sandbox_bypass_for_escalation() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let pending = pending_approval_with_escalation(None, true, Some("npm install".to_string()));

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .exact_commands
            .insert(("npm install".to_string(), PathBuf::from("/workspace")));
        expected_session_cache
            .sandbox_bypass_commands
            .insert(crate::execution::SandboxBypassKey {
                command: "npm install".to_string(),
                cwd: PathBuf::from("/workspace"),
                sandbox_permissions: "require_escalated".to_string(),
            });
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn session_scope_with_exact_command_prefers_exact_over_pattern() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let mut pending = pending_approval(Some(vec![
            "git".to_string(),
            "add".to_string(),
            "*".to_string(),
        ]));
        pending.command = Some("git add file.txt".to_string());

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .exact_commands
            .insert(("git add file.txt".to_string(), PathBuf::from("/workspace")));
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn session_scope_with_pattern_stores_pattern_not_tool_name() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let pending = pending_approval(Some(vec![
            "git".to_string(),
            "add".to_string(),
            "*".to_string(),
        ]));

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache.command_patterns.insert(vec![
            "git".to_string(),
            "add".to_string(),
            "*".to_string(),
        ]);
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn session_scope_without_pattern_keeps_tool_grant() {
        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let pending = pending_approval(/*command_pattern*/ None);

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &pending,
        );

        let mut expected_session_cache = crate::execution::ApprovalGrantCache::default();
        expected_session_cache
            .tools
            .insert("shell_command".to_string());
        assert_eq!(session_cache, expected_session_cache);
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn path_prefix_scope_updates_inline_cache_and_profile_for_child_path() {
        // Mid-turn authorize reads TurnInlineState; applying scope there must
        // make a sibling/child path grant-visible without waiting on the actor.
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("src");
        std::fs::create_dir_all(&dir).expect("create src dir");
        let child = dir.join("helper.rs");

        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let mut profile = RuntimePermissionProfile::from_preset(
            PermissionPreset::Default,
            temp.path().to_path_buf(),
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let pending = crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: "read".to_string(),
            resource: Some(devo_safety::ResourceKind::FileRead),
            path: Some(dir.clone()),
            host: None,
            command_prefix: None,
            command_pattern: None,
            requests_escalation: false,
            command: None,
            cwd: temp.path().to_path_buf(),
            sandbox_permissions: String::new(),
            tx,
        };

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::PathPrefix,
            &pending,
        );
        apply_path_scope_to_permission_profile(
            &mut profile,
            &ApprovalScopeValue::PathPrefix,
            &pending,
        );

        let grant_root = path_prefix_grant_root(&dir);
        assert_eq!(grant_root, dir);
        assert!(session_cache.read_path_prefixes.contains(&grant_root));
        assert!(profile.readable_roots.contains(&grant_root));
        assert!(
            session_cache
                .read_path_prefixes
                .iter()
                .any(|prefix| child.starts_with(prefix))
        );
        assert!(
            profile
                .readable_roots
                .iter()
                .any(|prefix| child.starts_with(prefix))
        );
    }
}
