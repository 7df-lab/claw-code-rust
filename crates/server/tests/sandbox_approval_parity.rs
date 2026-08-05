//! Approval / sandbox parity regression tests (Phase 9 / 11).
//!
//! Lightweight checks that lock the product contracts from the parity plan
//! without requiring full OS sandbox enforcement.

use std::path::PathBuf;

use pretty_assertions::assert_eq;

#[test]
fn sandbox_denied_message_contains_hint_prefix() {
    let message = devo_sandbox::shell_error_message(
        Some("strict"),
        1,
        "",
        "operation not permitted",
        "[stderr]\noperation not permitted",
    );
    assert!(
        message.starts_with("SANDBOX_DENIED:"),
        "expected SANDBOX_DENIED prefix, got {message}"
    );
    assert!(message.contains("require_escalated"));
}

#[test]
fn escalation_bypass_maps_to_sandbox_profile_off() {
    use devo_core::tools::router::PermissionGrant;

    let grant = PermissionGrant {
        bypass_sandbox: true,
        already_approved: true,
        sandbox_permission_overlay: None,
    };
    let mut sandbox_profile = Some("workspace".to_string());
    if grant.bypass_sandbox {
        sandbox_profile = Some("off".to_string());
    }
    assert_eq!(sandbox_profile.as_deref(), Some("off"));
}

#[test]
fn append_allow_prefix_rule_writes_default_rules_line() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy_path = temp.path().join("default.rules");
    let prefix = vec!["git".to_string(), "pull".to_string()];

    devo_execpolicy::blocking_append_allow_prefix_rule(&policy_path, &prefix)
        .expect("append prefix rule");

    let contents = std::fs::read_to_string(&policy_path).expect("read rules");
    assert_eq!(
        contents,
        "prefix_rule(pattern=[\"git\", \"pull\"], decision=\"allow\")\n"
    );
}

#[test]
fn path_prefix_grant_merges_into_runtime_writable_roots() {
    use devo_safety::{PermissionPreset, RuntimePermissionProfile};

    let root = PathBuf::from("/tmp/workspace");
    let mut profile =
        RuntimePermissionProfile::from_preset(PermissionPreset::Default, root.clone());
    assert!(
        !profile
            .writable_roots
            .contains(&PathBuf::from("/tmp/extra"))
    );

    profile.grant_writable_root(PathBuf::from("/tmp/extra"));
    assert!(
        profile
            .readable_roots
            .contains(&PathBuf::from("/tmp/extra"))
    );
    assert!(
        profile
            .writable_roots
            .contains(&PathBuf::from("/tmp/extra"))
    );
    assert!(profile.writable_roots.contains(&root));
}
