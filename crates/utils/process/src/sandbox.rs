//! Per-child-process sandbox application.
//!
//! Resolve the enforcement plan in the **parent** before spawn
//! ([`resolve_enforcement_plan`]), then apply it from `pre_exec` via
//! [`apply_resolved_in_child`]. Never load sandbox config after `fork`.

use std::path::Path;

pub use devo_sandbox::{
    ResolvedEnforcementPlan, SandboxPermissionOverlay, resolve_enforcement_plan,
    resolve_enforcement_plan_with_overlay,
};

/// Resolve a named profile in the parent process. See
/// [`devo_sandbox::resolve_enforcement_plan`].
pub fn resolve_profile_for_spawn(
    profile: Option<&str>,
    workspace: &Path,
) -> std::io::Result<Option<ResolvedEnforcementPlan>> {
    resolve_enforcement_plan(profile, workspace)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))
}

/// Resolve a named profile with a per-invocation sandbox overlay.
pub fn resolve_profile_for_spawn_with_overlay(
    profile: Option<&str>,
    workspace: &Path,
    overlay: Option<&SandboxPermissionOverlay>,
) -> std::io::Result<Option<ResolvedEnforcementPlan>> {
    resolve_enforcement_plan_with_overlay(profile, workspace, overlay)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))
}

/// Apply a parent-resolved plan in the child (`pre_exec`).
///
/// # Safety
///
/// Intended for `pre_exec` hooks: only applies already-resolved capabilities;
/// does not read config or allocate beyond what `Sandbox::apply` requires.
pub fn apply_resolved_in_child(plan: Option<&ResolvedEnforcementPlan>) -> std::io::Result<()> {
    devo_sandbox::apply_resolved_enforcement_in_child(plan)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))
}

/// Convenience: resolve then apply in the current process.
///
/// Prefer [`resolve_profile_for_spawn`] + [`apply_resolved_in_child`] for
/// spawn paths so config load stays in the parent.
pub fn apply_profile_in_child(profile: Option<&str>, workspace: &Path) -> std::io::Result<()> {
    let plan = resolve_profile_for_spawn(profile, workspace)?;
    apply_resolved_in_child(plan.as_ref())
}
