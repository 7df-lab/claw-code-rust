//! OS-level sandboxing for Devo via [nono](https://crates.io/crates/nono).
//!
//! Applied per child process before `exec` (see
//! [`apply_profile_to_current_process`]), or — where the spawn path has no
//! `pre_exec` hook or needs enforcement Landlock/Seatbelt cannot express —
//! by wrapping the command in an OS launcher (see [`wrap_command_for_profile`]). Children of
//! network-restricted profiles are blocked via nono's `block_network`
//! (Landlock `AccessNet` with a seccomp fallback on Linux, Seatbelt
//! `(deny network*)` on macOS); Linux bwrap wraps add `--unshare-net` on top.
//!
//! The `enforce` feature (on by default) pulls in `nono` for kernel-enforced
//! sandboxing (Landlock/Seatbelt). When disabled, the crate still provides
//! lightweight helpers that compile on all targets including musl.
//!
//! ```rust,no_run
//! use devo_sandbox::{ProfileName, SandboxManager};
//! use std::path::Path;
//!
//! let workspace = Path::new("/home/user/project");
//! let mut sandbox = SandboxManager::new(ProfileName::Workspace);
//! sandbox
//!     .apply_required(workspace)
//!     .expect("required sandbox enforcement failed");
//! ```
mod bwrap;
#[cfg(target_os = "linux")]
mod bwrap_placeholder;
mod denial;
mod deny;
mod linux_helper;
mod logging;
mod managed_network;
mod network_policy;
mod paths;
mod permissions;
mod profiles;
#[cfg(all(feature = "enforce", target_os = "macos"))]
mod seatbelt;
mod types;
mod wrap;

#[cfg(target_os = "linux")]
pub use bwrap::bwrap_reexec_for_profile;
pub use bwrap::{
    bwrap_reexec_command, is_inside_bwrap, requires_read_deny, trust_bwrap_marker_for_devbox,
};
pub use denial::{
    is_likely_sandbox_denied, is_likely_sandbox_denied_after_signal,
    output_text_suggests_sandbox_denial, shell_error_message, shell_error_message_with_signal,
};
pub use linux_helper::{
    DEVO_LINUX_SANDBOX_ARG0, LinuxSandboxPermissionProfile, create_linux_sandbox_command_args,
    find_linux_sandbox_helper,
};
pub use logging::SandboxLogger;
pub use managed_network::{
    ManagedNetworkSandboxContext, managed_network_context_from_env,
    managed_network_context_from_ports, sandbox_proxy_available, set_sandbox_proxy_ports,
    set_sandbox_proxy_ports_env,
};
#[cfg(unix)]
pub use managed_network::{
    apply_managed_network_context, proxy_env_for_restricted_network, proxy_env_for_sandbox_profile,
};
pub use network_policy::{
    ChildNetworkPolicy, NETWORK_POLICY_SNAPSHOT_VERSION, NetworkPolicySnapshot,
    NetworkPolicySnapshotError, WebsiteAction, WebsiteOrigin, WebsiteOriginError, WebsitePolicy,
};
pub use permissions::{SandboxNetworkPermission, SandboxPermissionOverlay};
pub use profiles::{
    ProfileName, SandboxConfig, SandboxProfile, load_sandbox_config, sandbox_profile_conflicts,
    unsandboxed_execution_allowed,
};
pub use types::{SandboxEvent, SandboxEventType, SandboxMetrics};
pub use wrap::{
    PLACEHOLDER_CLEANUP_DELAY, SandboxWrap, WrapMode, WrappedCommand,
    cleanup_stale_placeholder_dirs, remove_placeholder_dir, wrap_command_for_profile,
    wrap_command_for_profile_with_overlay,
};

#[cfg(windows)]
pub use devo_windows_sandbox::{should_wrap_profile, windows_sandbox_available};

#[cfg(not(windows))]
/// Returns whether the Windows sandbox backend is available on this host.
pub fn windows_sandbox_available() -> bool {
    false
}

#[cfg(not(windows))]
/// Returns true when a non-off profile should use the Windows sandbox backend.
pub fn should_wrap_profile(_profile: Option<&str>) -> bool {
    false
}

#[cfg(all(feature = "enforce", unix))]
use nono::{CapabilitySet, Sandbox};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyRequirement {
    Graceful,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApplyOutcome {
    Disabled,
    #[cfg_attr(not(all(feature = "enforce", unix)), allow(dead_code))]
    Applied,
    Unavailable(String),
}

fn validate_apply_outcome(
    profile: &ProfileName,
    requirement: ApplyRequirement,
    outcome: ApplyOutcome,
) -> anyhow::Result<()> {
    match (requirement, outcome) {
        (_, ApplyOutcome::Disabled | ApplyOutcome::Applied)
        | (ApplyRequirement::Graceful, ApplyOutcome::Unavailable(_)) => Ok(()),
        (ApplyRequirement::Required, ApplyOutcome::Unavailable(details)) => anyhow::bail!(
            "sandbox profile '{profile}' requires enforcement, but it was not applied: {details}"
        ),
    }
}

/// Parent-resolved sandbox enforcement. Built before `fork`/`spawn` so the
/// child `pre_exec` path never loads config (async-signal-unsafe IO).
#[derive(Debug, Clone)]
pub struct ResolvedEnforcementPlan {
    profile_name: String,
    #[cfg(all(feature = "enforce", unix))]
    caps: CapabilitySet,
}

impl ResolvedEnforcementPlan {
    /// Profile name that produced this plan (for logging / diagnostics).
    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }
}

/// Resolve a named profile into an enforcement plan in the **parent** process.
///
/// Returns `Ok(None)` for `None` / `"off"` / inactive profiles. Errors if the
/// profile name is invalid or config/profile resolution fails.
pub fn resolve_enforcement_plan(
    profile: Option<&str>,
    workspace: &Path,
) -> anyhow::Result<Option<ResolvedEnforcementPlan>> {
    resolve_enforcement_plan_with_overlay(profile, workspace, None)
}

/// Resolve a named profile and merge a per-invocation sandbox overlay before
/// the child enforcement plan is built.
pub fn resolve_enforcement_plan_with_overlay(
    profile: Option<&str>,
    workspace: &Path,
    overlay: Option<&SandboxPermissionOverlay>,
) -> anyhow::Result<Option<ResolvedEnforcementPlan>> {
    let Some(profile_name) = profile else {
        return Ok(None);
    };
    if matches!(profile_name.trim(), "" | "off" | "none") {
        return Ok(None);
    }
    let parsed = profile_name
        .parse::<ProfileName>()
        .map_err(|error| anyhow::anyhow!("invalid sandbox profile '{profile_name}': {error}"))?;
    if parsed == ProfileName::Off {
        return Ok(None);
    }

    #[cfg(all(feature = "enforce", unix))]
    {
        let config = profiles::load_sandbox_config(workspace)?;
        let resolved = parsed.resolve_profile(workspace, &config)?;
        let caps =
            ProfileName::capability_set_from_profile_with_overlay(workspace, &resolved, overlay)?;
        Ok(Some(ResolvedEnforcementPlan {
            profile_name: profile_name.to_string(),
            caps,
        }))
    }
    #[cfg(not(all(feature = "enforce", unix)))]
    {
        let _ = workspace;
        Ok(Some(ResolvedEnforcementPlan {
            profile_name: profile_name.to_string(),
        }))
    }
}

/// Apply a previously resolved plan in the child (`pre_exec`). Does **not**
/// load sandbox config or resolve profiles.
#[cfg(all(feature = "enforce", unix))]
pub fn apply_resolved_enforcement_in_child(
    plan: Option<&ResolvedEnforcementPlan>,
) -> anyhow::Result<()> {
    let Some(plan) = plan else {
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    {
        let fallback = Sandbox::apply(&plan.caps)?;
        if !matches!(fallback, nono::sandbox::SeccompNetFallback::None) {
            // Logging from pre_exec is not async-signal-safe; keep quiet.
            let _ = fallback;
        }
    }
    #[cfg(not(target_os = "linux"))]
    Sandbox::apply(&plan.caps)?;
    let _ = &plan.profile_name;
    Ok(())
}

/// Apply a previously resolved plan in the child (`pre_exec`).
#[cfg(not(all(feature = "enforce", unix)))]
pub fn apply_resolved_enforcement_in_child(
    plan: Option<&ResolvedEnforcementPlan>,
) -> anyhow::Result<()> {
    let _ = plan;
    Ok(())
}

/// Applies a sandbox profile to the current process. This is intended for
/// child processes before `exec`, where the sandbox must be irreversible.
/// Event logging happens on the parent side (see [`wrap_command_for_profile`]);
/// a `pre_exec` child cannot log (async-signal-safety).
///
/// Prefer [`resolve_enforcement_plan`] in the parent and
/// [`apply_resolved_enforcement_in_child`] in `pre_exec` so config is never
/// loaded after `fork`.
///
/// This is a no-op when `profile` is `None`, `"off"`, or when the process was
/// built without the `enforce` feature.
pub fn apply_profile_to_current_process(
    profile: Option<&str>,
    workspace: &Path,
) -> anyhow::Result<()> {
    let plan = resolve_enforcement_plan(profile, workspace)?;
    apply_resolved_enforcement_in_child(plan.as_ref())
}

/// Manages the OS-level sandbox for the current process. Call
/// `apply_required()` when continuing without enforcement would be unsafe.
///
/// This is the process-level entry point (used by tests and child runners);
/// the per-spawn command-wrapping path lives in [`wrap_command_for_profile`].
pub struct SandboxManager {
    profile: ProfileName,
    applied: bool,
}

impl SandboxManager {
    /// Create a sandbox manager. Does not apply until `apply()` is called.
    pub fn new(profile: ProfileName) -> Self {
        Self {
            profile,
            applied: false,
        }
    }

    /// Attempt to apply the sandbox to the current process. **Irreversible.**
    ///
    /// This compatibility API degrades gracefully: `Ok(())` does not mean
    /// enforcement is active. Call [`Self::is_applied`] to inspect the outcome,
    /// or use [`Self::apply_required`] when enforcement is mandatory.
    pub fn apply(&mut self, workspace: &Path) -> anyhow::Result<()> {
        self.apply_with_requirement(workspace, ApplyRequirement::Graceful)
    }

    /// Apply the sandbox and fail unless the selected non-`off` profile is
    /// kernel-enforced. **Irreversible.**
    ///
    /// This is the fail-closed entry point for child runners and other callers
    /// that must not continue when enforcement is unsupported or application
    /// fails. Selecting `off` remains an explicit successful opt-out.
    pub fn apply_required(&mut self, workspace: &Path) -> anyhow::Result<()> {
        self.apply_with_requirement(workspace, ApplyRequirement::Required)
    }

    #[cfg(all(feature = "enforce", unix))]
    fn apply_with_requirement(
        &mut self,
        workspace: &Path,
        requirement: ApplyRequirement,
    ) -> anyhow::Result<()> {
        if self.profile == ProfileName::Off {
            tracing::info!("Sandbox disabled (profile: off)");
            return validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Disabled);
        }
        let config = profiles::load_sandbox_config(workspace)?;
        let mut resolved = self.profile.resolve_profile(workspace, &config)?;
        let support = Sandbox::support_info();
        if !support.is_supported {
            tracing::warn!(
                details = %support.details,
                "Sandbox not supported on this platform, continuing without sandbox"
            );
            return validate_apply_outcome(
                &self.profile,
                requirement,
                ApplyOutcome::Unavailable(support.details),
            );
        }
        let caps = ProfileName::capability_set_from_profile(workspace, &resolved)?;
        resolved.deny = deny::effective_deny_paths(workspace, &resolved.deny);
        match Sandbox::apply(&caps) {
            Ok(_) => {
                self.applied = true;
                tracing::info!(
                    profile = %self.profile,
                    workspace = %workspace.display(),
                    restrict_network = resolved.restrict_network,
                    "Sandbox applied (kernel-enforced, irreversible)"
                );
                validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Applied)
            }
            Err(error) => {
                tracing::warn!(
                    profile = %self.profile,
                    error = %error,
                    "Sandbox could not be applied, continuing without sandbox"
                );
                validate_apply_outcome(
                    &self.profile,
                    requirement,
                    ApplyOutcome::Unavailable(error.to_string()),
                )
            }
        }
    }

    #[cfg(not(all(feature = "enforce", unix)))]
    fn apply_with_requirement(
        &mut self,
        _workspace: &Path,
        requirement: ApplyRequirement,
    ) -> anyhow::Result<()> {
        if self.profile == ProfileName::Off {
            tracing::info!("Sandbox disabled (profile: off)");
            return validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Disabled);
        }
        tracing::info!(
            profile = %self.profile,
            "Sandbox enforcement unavailable (built without 'enforce' feature)"
        );
        validate_apply_outcome(
            &self.profile,
            requirement,
            ApplyOutcome::Unavailable("built without the 'enforce' feature".to_string()),
        )
    }

    /// Whether the sandbox was successfully applied.
    pub fn is_applied(&self) -> bool {
        self.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn resolve_enforcement_plan_is_none_for_off_profiles() {
        let workspace = std::env::temp_dir();
        assert!(
            resolve_enforcement_plan(None, &workspace)
                .expect("resolve")
                .is_none()
        );
        assert!(
            resolve_enforcement_plan(Some("off"), &workspace)
                .expect("resolve")
                .is_none()
        );
        assert!(
            resolve_enforcement_plan(Some("none"), &workspace)
                .expect("resolve")
                .is_none()
        );
    }

    #[test]
    fn resolve_enforcement_plan_rejects_unknown_profile_names() {
        let workspace = std::env::temp_dir();
        let error = resolve_enforcement_plan(Some("not-a-real-profile"), &workspace)
            .expect_err("unknown profile");
        assert!(
            error
                .to_string()
                .contains("Custom sandbox profile 'not-a-real-profile' not found"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn required_apply_rejects_unavailable_enforcement() {
        let error = validate_apply_outcome(
            &ProfileName::Strict,
            ApplyRequirement::Required,
            ApplyOutcome::Unavailable("unsupported in test".to_string()),
        )
        .expect_err("required enforcement must fail closed");

        assert_eq!(
            error.to_string(),
            "sandbox profile 'strict' requires enforcement, but it was not applied: unsupported in test"
        );
    }

    #[test]
    fn graceful_apply_accepts_unavailable_enforcement() {
        assert!(
            validate_apply_outcome(
                &ProfileName::Strict,
                ApplyRequirement::Graceful,
                ApplyOutcome::Unavailable("unsupported in test".to_string()),
            )
            .is_ok()
        );
    }

    #[test]
    fn required_apply_accepts_applied_or_explicitly_disabled_outcomes() {
        for (profile, outcome) in [
            (ProfileName::Strict, ApplyOutcome::Applied),
            (ProfileName::Off, ApplyOutcome::Disabled),
        ] {
            assert!(
                validate_apply_outcome(&profile, ApplyRequirement::Required, outcome).is_ok(),
                "unexpected rejection for {profile}"
            );
        }
    }
}
