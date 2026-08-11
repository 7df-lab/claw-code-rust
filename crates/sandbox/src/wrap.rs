//! Command-wrapping sandbox API: decide whether a child process is launched
//! directly or through an OS sandbox launcher (Linux `bwrap`; macOS
//! `sandbox-exec`).
//!
//! Two composition modes:
//!
//! - [`WrapMode::PipeComposed`]: Linux pipe spawns apply the profile via
//!   `pre_exec` Landlock, so the wrapper only adds what Landlock cannot express.
//!   macOS pipe spawns use `sandbox-exec`, because applying Seatbelt after
//!   `fork()` is not safe in a multi-threaded process.
//! - [`WrapMode::PtyOnly`]: PTY spawns have no `pre_exec` hook, so the
//!   wrapper carries the full profile policy.
//!
//! The API never fails closed (user decision): a missing launcher or a wrap
//! construction failure logs a warning and yields [`SandboxWrap::None`]. Only
//! profile resolution errors are returned as `Err`.
//!
//! Every decision that resolves a non-`off` profile is also recorded as a
//! [`SandboxEvent`] on the caller-provided [`SandboxLogger`] (spawn-time
//! logging — a `pre_exec` child cannot log, so the parent side is the correct
//! layer): a successful wrap logs `profile_applied`, a warn-and-release logs
//! `apply_failed` (construction/validation failed) or `not_enforced` (the
//! environment cannot provide enforcement). [`wrap_command_for_profile`]
//! flushes the events to disk before returning.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::logging::SandboxLogger;
use crate::profiles::{ProfileName, SandboxConfig, SandboxProfile, load_sandbox_config};
use crate::types::SandboxEvent;

/// Name prefix of the per-launch bwrap placeholder directory under
/// `devo_home` (see [`crate::bwrap_placeholder`]).
pub(crate) const PLACEHOLDER_DIR_PREFIX: &str = "bwrap-placeholder.";

/// How long after a successful spawn a bwrap placeholder directory must
/// survive: mounts are not yet up when `spawn` returns, but are long up
/// after this delay.
pub const PLACEHOLDER_CLEANUP_DELAY: Duration = Duration::from_secs(60);

/// Placeholder directories older than this are removed by the janitor.
const PLACEHOLDER_STALE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Environment override for launcher detection, for tests and diagnostics:
/// `auto` (default), `none` (pretend no launcher exists), `bwrap`, or
/// `sandbox-exec`.
const LAUNCHER_OVERRIDE_ENV: &str = "DEVO_SANDBOX_LAUNCHER";

/// How a wrapped command composes with the other sandbox layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// PTY spawns have no `pre_exec` hook: the wrapper carries the full policy.
    PtyOnly,
    /// Pipe spawns already apply Landlock/Seatbelt via `pre_exec`; the wrapper
    /// only adds Linux deny bind-overs and network restriction.
    PipeComposed,
}

/// Sandbox decision for a command about to be spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxWrap {
    /// Run the command as-is (no wrapper needed, or none available — see the
    /// warning logged in that case).
    None,
    /// Replace program/args with a sandbox launcher invocation.
    Wrapped(WrappedCommand),
}

impl SandboxWrap {
    /// Whether the spawner must still apply a resolved profile in `pre_exec`.
    ///
    /// macOS never applies Seatbelt after `fork()`: active profiles use the
    /// `sandbox-exec` wrapper, while an unavailable wrapper preserves the
    /// existing warn-and-run-unwrapped behavior.
    pub fn requires_child_apply(&self) -> bool {
        if cfg!(target_os = "macos") {
            return false;
        }
        !matches!(self, Self::Wrapped(wrapped) if wrapped.helper_enforces)
    }
}

/// A launcher invocation that sandboxes the original command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommand {
    /// Launcher executable (`bwrap`, `devo-linux-sandbox`, or
    /// `/usr/bin/sandbox-exec` on macOS).
    pub program: String,
    /// Arguments up to and including the `--` separator; the original program
    /// and its arguments are appended after them.
    pub prefix_args: Vec<String>,
    /// bwrap read-deny placeholder directory. The spawner removes it with
    /// [`remove_placeholder_dir`] [`PLACEHOLDER_CLEANUP_DELAY`] after a
    /// successful spawn (mounts are not up when `spawn` returns).
    pub placeholder_dir: Option<PathBuf>,
    /// When true, the launcher applies the complete sandbox policy; the parent
    /// must not also apply a `pre_exec` plan onto it.
    pub helper_enforces: bool,
}

/// Decide whether `profile` requires the spawned command to be wrapped in a
/// sandbox launcher, and build that invocation.
///
/// `None`/`"off"`/Windows → [`SandboxWrap::None`]. A missing launcher or a
/// construction failure logs a warning and returns `Ok(SandboxWrap::None)`
/// (never fails closed); only profile resolution errors are `Err`.
///
/// `logger` receives one event per resolved non-`off` decision (see the
/// module docs) and is flushed to disk before this function returns. There is
/// no global logger: callers either hold a [`SandboxLogger`] or pass a fresh
/// `&SandboxLogger::new()` per spawn.
pub fn wrap_command_for_profile(
    profile: Option<&str>,
    workspace: &Path,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    static JANITOR: std::sync::Once = std::sync::Once::new();
    JANITOR.call_once(cleanup_stale_placeholder_dirs);

    if cfg!(windows) {
        let active = profile.is_some_and(|p| {
            let p = p.trim();
            !p.is_empty() && p != "off" && p != "none"
        });
        if active {
            tracing::info!(
                profile = profile.expect("checked above"),
                mode = ?mode,
                "Windows sandbox profile selected; wrap_command returns None because \
                 enforcement is applied by devo-windows-sandbox in shell_exec"
            );
            if let Err(error) = logger.flush_to_disk() {
                tracing::warn!(error = %error, "failed to flush sandbox events to disk");
            }
        }
        return Ok(SandboxWrap::None);
    }
    let Some(profile) = profile else {
        return Ok(SandboxWrap::None);
    };
    let profile_name = profile
        .parse::<ProfileName>()
        .map_err(|error| anyhow::anyhow!("invalid sandbox profile '{profile}': {error}"))?;
    if profile_name == ProfileName::Off {
        return Ok(SandboxWrap::None);
    }
    let config = load_sandbox_config(workspace)?;
    let resolved = profile_name.resolve_profile(workspace, &config)?;
    let wrap = wrap_for_platform(
        &profile_name,
        &config,
        &resolved,
        workspace,
        mode,
        launcher_availability(),
        logger,
    );
    // Events were recorded at the decision sites; persist them (best-effort).
    if let Err(error) = logger.flush_to_disk() {
        tracing::warn!(error = %error, "failed to flush sandbox events to disk");
    }
    wrap
}

/// Tag a wrap-path sandbox event with the wrap mode and launcher, then log it.
#[allow(unused)]
fn log_wrap_event(logger: &SandboxLogger, mut event: SandboxEvent, mode: WrapMode, launcher: &str) {
    event.mode = Some(format!("{mode:?}"));
    event.launcher = Some(launcher.to_string());
    logger.log(event);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherOverride {
    Auto,
    None,
    Bwrap,
    SandboxExec,
}

fn launcher_override(value: Option<&str>) -> LauncherOverride {
    match value {
        Some("none") => LauncherOverride::None,
        Some("bwrap") => LauncherOverride::Bwrap,
        Some("sandbox-exec") => LauncherOverride::SandboxExec,
        _ => LauncherOverride::Auto,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LauncherAvailability {
    sandbox_exec: bool,
    bwrap: bool,
}

fn launcher_availability() -> LauncherAvailability {
    match launcher_override(std::env::var(LAUNCHER_OVERRIDE_ENV).ok().as_deref()) {
        LauncherOverride::None => LauncherAvailability {
            sandbox_exec: false,
            bwrap: false,
        },
        LauncherOverride::Bwrap => LauncherAvailability {
            sandbox_exec: false,
            bwrap: true,
        },
        LauncherOverride::SandboxExec => LauncherAvailability {
            sandbox_exec: true,
            bwrap: false,
        },
        LauncherOverride::Auto => LauncherAvailability {
            sandbox_exec: sandbox_exec_available(),
            bwrap: bwrap_available(),
        },
    }
}

fn sandbox_exec_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| Path::new("/usr/bin/sandbox-exec").is_file())
}

fn bwrap_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let Ok(output) = std::process::Command::new("bwrap").arg("--help").output() else {
            return false;
        };
        if !output.status.success() {
            // Older bwrap may exit non-zero on --help; fall back to --version.
            return std::process::Command::new("bwrap")
                .arg("--version")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let help = format!("{stdout}{stderr}");
        // Deny-read masks require `--perms` when the host bwrap supports it. Devo still uses
        // host placeholders today, but warn when the binary is too old so we
        // know advanced deny mounts are unavailable.
        if !(help.contains("--perms")) {
            tracing::warn!(
                "system bwrap does not advertise --perms; in-namespace \
                 deny-read masks are unavailable (placeholder binds still work)"
            );
        }
        true
    })
}

fn wrap_for_platform(
    profile_name: &ProfileName,
    config: &SandboxConfig,
    resolved: &SandboxProfile,
    workspace: &Path,
    mode: WrapMode,
    launchers: LauncherAvailability,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    #[cfg(target_os = "linux")]
    {
        linux_wrap(
            profile_name,
            config,
            resolved,
            workspace,
            mode,
            launchers,
            logger,
        )
    }
    #[cfg(target_os = "macos")]
    {
        // `config` and bwrap availability are Linux-only inputs.
        let _ = (config, launchers.bwrap);
        macos_wrap(
            profile_name,
            resolved,
            workspace,
            launchers.sandbox_exec,
            mode,
            logger,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (
            profile_name,
            config,
            resolved,
            workspace,
            mode,
            launchers.sandbox_exec,
            launchers.bwrap,
            logger,
        );
        Ok(SandboxWrap::None)
    }
}

/// macOS wrap: `sandbox-exec -p <sbpl>` carrying the full profile for both pipe
/// and PTY children. Never fails closed — a missing launcher, a build failure,
/// or a failed precheck warns, records an event, and runs unwrapped.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn macos_wrap(
    profile_name: &ProfileName,
    resolved: &SandboxProfile,
    workspace: &Path,
    sandbox_exec_available: bool,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    if !sandbox_exec_available {
        tracing::warn!(
            profile = %profile_name,
            "sandbox-exec is not available; child runs WITHOUT sandbox enforcement \
             (deny paths, filesystem policy, and network restriction are NOT enforced)"
        );
        log_wrap_event(
            logger,
            SandboxEvent::not_enforced(
                &profile_name.to_string(),
                workspace,
                "sandbox-exec is not available",
            ),
            mode,
            "sandbox-exec",
        );
        return Ok(SandboxWrap::None);
    }
    let sbpl = match crate::seatbelt::seatbelt_profile_for(workspace, resolved) {
        Ok(sbpl) => sbpl,
        Err(error) => {
            tracing::warn!(
                profile = %profile_name,
                error = %error,
                "could not build the Seatbelt profile; child runs WITHOUT \
                 sandbox enforcement"
            );
            log_wrap_event(
                logger,
                SandboxEvent::apply_failed(&profile_name.to_string(), workspace, &error),
                mode,
                "sandbox-exec",
            );
            return Ok(SandboxWrap::None);
        }
    };
    if !crate::seatbelt::sandbox_exec_accepts_profile(&sbpl) {
        tracing::warn!(
            profile = %profile_name,
            "sandbox-exec rejected the generated Seatbelt profile; child runs \
             WITHOUT sandbox enforcement"
        );
        log_wrap_event(
            logger,
            SandboxEvent::apply_failed(
                &profile_name.to_string(),
                workspace,
                &"sandbox-exec rejected the generated Seatbelt profile",
            ),
            mode,
            "sandbox-exec",
        );
        return Ok(SandboxWrap::None);
    }
    tracing::info!(
        profile = %profile_name,
        mode = ?mode,
        "spawning command inside sandbox-exec (Seatbelt)"
    );
    log_wrap_event(
        logger,
        SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
        mode,
        "/usr/bin/sandbox-exec",
    );
    Ok(SandboxWrap::Wrapped(WrappedCommand {
        program: "/usr/bin/sandbox-exec".to_string(),
        prefix_args: vec!["-p".to_string(), sbpl],
        placeholder_dir: None,
        helper_enforces: true,
    }))
}

/// Without the `enforce` feature there is no sbpl emitter; warn, record, and
/// run unwrapped (never fail closed).
#[cfg(all(not(feature = "enforce"), target_os = "macos"))]
fn macos_wrap(
    profile_name: &ProfileName,
    _resolved: &SandboxProfile,
    workspace: &Path,
    _sandbox_exec_available: bool,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    tracing::warn!(
        profile = %profile_name,
        "built without the 'enforce' feature; child runs WITHOUT sandbox enforcement"
    );
    log_wrap_event(
        logger,
        SandboxEvent::not_enforced(
            &profile_name.to_string(),
            workspace,
            "built without the 'enforce' feature",
        ),
        mode,
        "sandbox-exec",
    );
    Ok(SandboxWrap::None)
}

#[cfg(target_os = "linux")]
fn linux_wrap(
    profile_name: &ProfileName,
    config: &SandboxConfig,
    resolved: &SandboxProfile,
    workspace: &Path,
    mode: WrapMode,
    launchers: LauncherAvailability,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    // Prefer the Linux helper path (bwrap → apply-seccomp-then-exec) when
    // available: parent only serializes the profile; the helper enforces.
    // Skip when DEVO_SANDBOX_LAUNCHER forces a direct launcher (helper outer
    // sets `bwrap` to avoid recursive helper wraps).
    if matches!(
        launcher_override(std::env::var(LAUNCHER_OVERRIDE_ENV).ok().as_deref()),
        LauncherOverride::Auto
    ) && linux_wrap_adds_enforcement(resolved, mode)
        && let Some(helper) = crate::linux_helper::find_linux_sandbox_helper()
    {
        let mut permission_profile =
            crate::LinuxSandboxPermissionProfile::new(profile_name.to_string(), workspace);
        if resolved.restrict_network {
            // Prefer the in-process managed-proxy port store; fall back to a
            // parent-process HTTP(S)_PROXY when the user already has one.
            permission_profile =
                permission_profile.with_proxy_network(crate::sandbox_proxy_available());
        }
        match crate::linux_helper::create_linux_sandbox_command_args(
            &[],
            workspace,
            &permission_profile,
            workspace,
        ) {
            Ok(prefix_args) => {
                tracing::info!(
                    profile = %profile_name,
                    mode = ?mode,
                    helper = %helper.display(),
                    "spawning command via devo-linux-sandbox helper"
                );
                log_wrap_event(
                    logger,
                    SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
                    mode,
                    crate::DEVO_LINUX_SANDBOX_ARG0,
                );
                return Ok(SandboxWrap::Wrapped(WrappedCommand {
                    program: helper.to_string_lossy().into_owned(),
                    prefix_args,
                    placeholder_dir: None,
                    helper_enforces: true,
                }));
            }
            Err(error) => {
                tracing::warn!(
                    profile = %profile_name,
                    error = %error,
                    "could not build linux-sandbox helper args; falling back to direct bwrap"
                );
            }
        }
    }

    if !linux_wrap_adds_enforcement(resolved, mode) {
        return Ok(SandboxWrap::None);
    }
    if !launchers.bwrap {
        // Pipe spawns keep their pre_exec Landlock enforcement (including the
        // nono network block); PTY spawns get nothing at all. Either way the
        // deny bind-overs are lost, so name the paths that go unenforced.
        tracing::warn!(
            profile = %profile_name,
            mode = ?mode,
            deny = ?resolved.deny,
            restrict_network = resolved.restrict_network,
            "bwrap is not available; spawning WITHOUT the sandbox wrapper — \
             the listed deny paths are NOT enforced (PTY spawns also lose \
             network restriction and all filesystem policy)"
        );
        log_wrap_event(
            logger,
            SandboxEvent::not_enforced(
                &profile_name.to_string(),
                workspace,
                "bwrap is not available; deny paths are not enforced",
            ),
            mode,
            "bwrap",
        );
        return Ok(SandboxWrap::None);
    }
    let devbox_based = crate::bwrap::is_devbox_based(profile_name, config);
    match crate::bwrap::bwrap_wrap_argv(workspace, resolved, devbox_based, mode) {
        Ok((prefix_args, placeholder_dir)) => {
            tracing::info!(
                profile = %profile_name,
                mode = ?mode,
                "spawning command inside bwrap sandbox"
            );
            log_wrap_event(
                logger,
                SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
                mode,
                "bwrap",
            );
            Ok(SandboxWrap::Wrapped(WrappedCommand {
                program: "bwrap".to_string(),
                prefix_args,
                placeholder_dir,
                helper_enforces: false,
            }))
        }
        Err(error) => {
            tracing::warn!(
                profile = %profile_name,
                mode = ?mode,
                error = %error,
                "could not build the bwrap sandbox wrapper; spawning unwrapped \
                 (deny paths are NOT enforced)"
            );
            log_wrap_event(
                logger,
                SandboxEvent::apply_failed(&profile_name.to_string(), workspace, &error),
                mode,
                "bwrap",
            );
            Ok(SandboxWrap::None)
        }
    }
}

/// Whether a Linux bwrap wrap enforces anything beyond the pre_exec sandbox.
#[cfg(any(target_os = "linux", test))]
fn linux_wrap_adds_enforcement(resolved: &SandboxProfile, mode: WrapMode) -> bool {
    match mode {
        // PTY has no pre_exec: the wrapper carries the entire policy.
        WrapMode::PtyOnly => true,
        // Pipe children already get Landlock via pre_exec; bwrap only adds
        // read-deny bind-overs and network restriction.
        WrapMode::PipeComposed => !resolved.deny.is_empty() || resolved.restrict_network,
    }
}

/// Best-effort removal of a per-launch bwrap placeholder directory. Spawners
/// call this [`PLACEHOLDER_CLEANUP_DELAY`] after a successful spawn; it is
/// also safe to call once the wrapped process has exited. Refuses to touch
/// anything not named like a placeholder directory.
pub fn remove_placeholder_dir(directory: &Path) {
    if !is_placeholder_dir_name(directory) {
        tracing::warn!(
            path = %directory.display(),
            "refusing to remove a directory that is not a bwrap placeholder directory"
        );
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %directory.display(),
            error = %error,
            "could not remove bwrap placeholder directory"
        );
    }
}

fn is_placeholder_dir_name(directory: &Path) -> bool {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(PLACEHOLDER_DIR_PREFIX))
}

/// Startup janitor: remove per-launch placeholder directories older than 24h
/// that crashed processes left behind under `devo_home`. Best-effort.
pub fn cleanup_stale_placeholder_dirs() {
    let Ok(devo_home) = crate::paths::devo_home() else {
        return;
    };
    cleanup_stale_placeholder_dirs_in(&devo_home, SystemTime::now());
}

fn cleanup_stale_placeholder_dirs_in(root: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_placeholder_dir_name(&path) || !path.is_dir() {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age >= PLACEHOLDER_STALE_AGE);
        if !stale {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "removed stale bwrap placeholder directory")
            }
            Err(error) => tracing::warn!(
                path = %path.display(),
                error = %error,
                "could not remove stale bwrap placeholder directory"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
