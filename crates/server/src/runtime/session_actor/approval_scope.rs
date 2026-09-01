use std::path::{Component, Path, PathBuf};

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
            } else if let Some(path) = pending.path.as_ref() {
                insert_exact_file_path_grant(session_cache, pending.resource.as_ref(), path);
            } else {
                session_cache.tools.insert(pending.tool_name.clone());
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

/// Grants PathPrefix folder roots onto a runtime permission profile.
pub(crate) fn apply_path_scope_to_permission_profile(
    profile: &mut RuntimePermissionProfile,
    scope: &ApprovalScopeValue,
    pending: &PendingApproval,
) {
    if !matches!(scope, ApprovalScopeValue::PathPrefix) {
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

pub(crate) fn normalize_permission_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn insert_exact_file_path_grant(
    cache: &mut ApprovalGrantCache,
    resource: Option<&devo_safety::ResourceKind>,
    path: &Path,
) {
    let grant = normalize_permission_path(path);
    match resource {
        Some(devo_safety::ResourceKind::FileWrite) => {
            cache.write_exact_paths.insert(grant);
        }
        Some(devo_safety::ResourceKind::FileRead) => {
            cache.read_exact_paths.insert(grant);
        }
        Some(_) | None => {
            cache.read_exact_paths.insert(grant);
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
    use super::normalize_permission_path;
    use super::path_prefix_grant_root;
    use devo_safety::PermissionPreset;
    use devo_safety::ResourceKind;
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
            persisted: None,
            checkpoint: None,
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
            persisted: None,
            checkpoint: None,
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
            persisted: None,
            checkpoint: None,
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
            persisted: None,
            checkpoint: None,
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
            persisted: None,
            checkpoint: None,
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

    fn abs_path(parts: &[&str]) -> PathBuf {
        #[cfg(windows)]
        let mut path = PathBuf::from(r"C:\");
        #[cfg(unix)]
        let mut path = PathBuf::from("/");

        for part in parts {
            path.push(part);
        }
        path
    }

    fn file_pending_approval(
        tool_name: &str,
        resource: ResourceKind,
        path: PathBuf,
    ) -> crate::execution::PendingApproval {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        crate::execution::PendingApproval {
            owner_session_id: devo_protocol::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            tool_name: tool_name.to_string(),
            resource: Some(resource),
            path: Some(path),
            host: None,
            command_prefix: None,
            command_pattern: None,
            requests_escalation: false,
            command: None,
            cwd: abs_path(&["workspace"]),
            sandbox_permissions: String::new(),
            persisted: None,
            checkpoint: None,
            tx,
        }
    }

    #[test]
    fn once_scope_does_not_store_file_grants() {
        let file_path = abs_path(&["workspace", "src", "main.rs"]);
        let pending = file_pending_approval("write", ResourceKind::FileWrite, file_path);

        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Once,
            &pending,
        );

        assert_eq!(
            session_cache,
            crate::execution::ApprovalGrantCache::default()
        );
        assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
    }

    #[test]
    fn session_scope_stores_exact_file_path_for_read_write_and_edit() {
        let cases = [
            ("read", ResourceKind::FileRead),
            ("write", ResourceKind::FileWrite),
            ("edit", ResourceKind::FileWrite),
        ];
        let file_path = abs_path(&["workspace", "src", "main.rs"]);
        let normalized = normalize_permission_path(&file_path);

        for (tool_name, resource) in cases {
            let pending = file_pending_approval(tool_name, resource.clone(), file_path.clone());
            let mut session_cache = crate::execution::ApprovalGrantCache::default();
            let mut turn_cache = crate::execution::ApprovalGrantCache::default();

            apply_approval_scope_to_state(
                &mut session_cache,
                &mut turn_cache,
                &ApprovalScopeValue::Session,
                &pending,
            );

            assert!(
                session_cache.tools.is_empty(),
                "{tool_name} session scope must not grant the whole tool"
            );
            assert!(session_cache.read_path_prefixes.is_empty());
            assert!(session_cache.write_path_prefixes.is_empty());

            match resource {
                ResourceKind::FileRead => {
                    assert_eq!(session_cache.read_exact_paths, [normalized.clone()].into());
                    assert!(session_cache.write_exact_paths.is_empty());
                }
                ResourceKind::FileWrite => {
                    assert_eq!(session_cache.write_exact_paths, [normalized.clone()].into());
                    assert!(session_cache.read_exact_paths.is_empty());
                }
                _ => unreachable!("file tool test cases only use read/write resources"),
            }
            assert_eq!(turn_cache, crate::execution::ApprovalGrantCache::default());
        }
    }

    #[test]
    fn session_scope_does_not_widen_permission_profile_roots() {
        let file_path = abs_path(&["workspace", "src", "main.rs"]);
        let pending = file_pending_approval("read", ResourceKind::FileRead, file_path);
        let mut profile = RuntimePermissionProfile::from_preset(
            PermissionPreset::Default,
            abs_path(&["workspace"]),
        );
        let before_readable = profile.readable_roots.clone();
        let before_writable = profile.writable_roots.clone();

        apply_path_scope_to_permission_profile(
            &mut profile,
            &ApprovalScopeValue::Session,
            &pending,
        );

        assert_eq!(profile.readable_roots, before_readable);
        assert_eq!(profile.writable_roots, before_writable);
    }

    #[test]
    fn path_prefix_scope_allows_sibling_files_in_same_directory() {
        let dir = abs_path(&["workspace", "src"]);
        let granted_file = dir.join("main.rs");
        let sibling_file = dir.join("helper.rs");
        let outside_file = abs_path(&["workspace", "other.rs"]);

        let cases = [
            ("read", ResourceKind::FileRead, "read_path_prefixes"),
            ("write", ResourceKind::FileWrite, "write_path_prefixes"),
            ("edit", ResourceKind::FileWrite, "write_path_prefixes"),
        ];

        for (tool_name, resource, prefix_field) in cases {
            let pending = file_pending_approval(tool_name, resource, granted_file.clone());
            let mut session_cache = crate::execution::ApprovalGrantCache::default();
            let mut turn_cache = crate::execution::ApprovalGrantCache::default();

            apply_approval_scope_to_state(
                &mut session_cache,
                &mut turn_cache,
                &ApprovalScopeValue::PathPrefix,
                &pending,
            );

            let prefix_root = path_prefix_grant_root(&granted_file);
            assert_eq!(prefix_root, dir);
            let prefixes = match prefix_field {
                "read_path_prefixes" => &session_cache.read_path_prefixes,
                "write_path_prefixes" => &session_cache.write_path_prefixes,
                _ => unreachable!(),
            };
            assert!(
                prefixes.contains(&prefix_root),
                "{tool_name} path prefix grant"
            );
            assert!(session_cache.read_exact_paths.is_empty());
            assert!(session_cache.write_exact_paths.is_empty());
            assert!(sibling_file.starts_with(prefix_root.as_path()));
            assert!(!outside_file.starts_with(prefix_root.as_path()));
        }
    }

    #[test]
    fn session_scope_does_not_allow_sibling_files() {
        let dir = abs_path(&["workspace", "src"]);
        let granted_file = dir.join("main.rs");
        let sibling_file = dir.join("helper.rs");

        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let pending = file_pending_approval("write", ResourceKind::FileWrite, granted_file);

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &pending,
        );

        let normalized_sibling = normalize_permission_path(&sibling_file);
        assert!(
            !session_cache
                .write_exact_paths
                .contains(&normalized_sibling)
        );
        assert!(session_cache.write_path_prefixes.is_empty());
    }

    #[test]
    fn read_and_write_session_grants_stay_separate() {
        let file_path = abs_path(&["workspace", "src", "main.rs"]);
        let normalized = normalize_permission_path(&file_path);

        let mut session_cache = crate::execution::ApprovalGrantCache::default();
        let mut turn_cache = crate::execution::ApprovalGrantCache::default();
        let read_pending = file_pending_approval("read", ResourceKind::FileRead, file_path.clone());

        apply_approval_scope_to_state(
            &mut session_cache,
            &mut turn_cache,
            &ApprovalScopeValue::Session,
            &read_pending,
        );

        assert_eq!(session_cache.read_exact_paths, [normalized.clone()].into());
        assert!(session_cache.write_exact_paths.is_empty());
    }
}
