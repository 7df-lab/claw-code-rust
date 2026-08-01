//! Shared pre-spawn sandbox preparation for shell_exec pipe and PTY paths.
//!
//! This is intentionally **not** the `devo_util_process` pipe backend: that
//! module owns process handles, channels, and lifecycle. Here we only decide
//! wrap / child-apply / placeholder cleanup / proxy env so both executors
//! stay consistent without sharing spawn semantics.
//!
//! # Platform sandbox implementation
//!
//! - **macOS** — Always through `/usr/bin/sandbox-exec` with a generated
//!   Seatbelt profile. Pipe and PTY both use this wrapper; Seatbelt is never
//!   applied in `pre_exec` after `fork` (unsafe in a multithreaded parent). If
//!   `sandbox-exec` is missing or rejects the profile, the child runs
//!   unwrapped (warn-and-release).
//! - **Linux** — Two layers compose by mode:
//!   - *Pipe* (`PipeComposed`): Landlock + seccomp are resolved in the parent
//!     and applied in the child via `pre_exec`. A `bwrap` /
//!     `devo-linux-sandbox` wrapper is added only when the profile needs what
//!     Landlock cannot express (deny-read bind-overs, network restriction).
//!   - *PTY* (`PtyOnly`): no `pre_exec`, so the wrapper carries the full
//!     policy whenever a profile is active. Temporary bwrap placeholder dirs
//!     are cleaned up after spawn.
//! - **Windows** — `wrap_command_for_profile` is a no-op. Enforcement goes
//!   through `try_windows_sandbox_launch` / `devo_windows_sandbox`, which
//!   builds a launcher embedding the full command line plus read/write/deny
//!   roots and optional network restriction. If launch prep is not wired yet,
//!   the child runs unwrapped (one-time warning).
//!
//! Active profiles may also inject proxy-related env vars on Unix so outbound
//! traffic can be steered through the sandbox proxy when configured.

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;
use tokio::process::Command;

use super::resolve::ShellSpec;

/// Resolved sandbox launch configuration for one shell spawn.
///
/// See the module docs for how this plan maps onto macOS Seatbelt, Linux
/// Landlock/`bwrap`, and Windows sandbox launch.
pub(crate) struct SandboxLaunchPlan {
    wrap: devo_sandbox::SandboxWrap,
    #[cfg(not(unix))]
    windows_launch: Option<devo_windows_sandbox::WindowsSandboxLaunch>,
    #[cfg(unix)]
    child_apply_plan: Option<devo_sandbox::ResolvedEnforcementPlan>,
    sandbox_profile: Option<String>,
    workdir: PathBuf,
}

impl SandboxLaunchPlan {
    /// Prepare a pipe-mode launch (`WrapMode::PipeComposed` + optional `pre_exec`).
    pub(crate) fn prepare_pipe(
        sandbox_profile: Option<&str>,
        workdir: &Path,
        shell: &ShellSpec,
        command: &str,
    ) -> Result<Self, String> {
        Self::prepare(
            sandbox_profile,
            workdir,
            shell,
            command,
            devo_sandbox::WrapMode::PipeComposed,
            /*attach_child_apply*/ true,
        )
    }

    /// Prepare a PTY-mode launch (`WrapMode::PtyOnly`; no `pre_exec`).
    pub(crate) fn prepare_pty(
        sandbox_profile: Option<&str>,
        workdir: &Path,
        shell: &ShellSpec,
        command: &str,
    ) -> Result<Self, String> {
        Self::prepare(
            sandbox_profile,
            workdir,
            shell,
            command,
            devo_sandbox::WrapMode::PtyOnly,
            /*attach_child_apply*/ false,
        )
    }

    fn prepare(
        sandbox_profile: Option<&str>,
        workdir: &Path,
        shell: &ShellSpec,
        command: &str,
        mode: devo_sandbox::WrapMode,
        attach_child_apply: bool,
    ) -> Result<Self, String> {
        // Platform wrap decision (details in module docs):
        // - macOS → sandbox-exec / Seatbelt
        // - Linux → optional bwrap / linux-sandbox helper (composes with pre_exec)
        // - Windows → SandboxWrap::None; see try_windows_sandbox_launch below
        #[cfg(unix)]
        let wrap = match devo_sandbox::wrap_command_for_profile(
            sandbox_profile,
            workdir,
            mode,
            &devo_sandbox::SandboxLogger::new(),
        ) {
            Ok(wrap) => wrap,
            Err(error) => return Err(format!("failed to set up sandbox: {error}")),
        };
        #[cfg(not(unix))]
        let wrap = {
            let _ = mode;
            devo_sandbox::SandboxWrap::None
        };

        #[cfg(not(unix))]
        let windows_launch = try_windows_sandbox_launch(sandbox_profile, workdir, shell, command)?;
        #[cfg(unix)]
        let _ = (shell, command);

        #[cfg(unix)]
        let child_apply_plan = if attach_child_apply && wrap.requires_child_apply() {
            // `requires_child_apply` is false on macOS (Seatbelt is only via
            // `sandbox-exec`) and when a Linux wrapper already enforces the full
            // policy. Otherwise resolve Landlock/seccomp for `pre_exec`.
            match devo_util_process::sandbox::resolve_profile_for_spawn(sandbox_profile, workdir) {
                Ok(plan) => plan,
                Err(error) => {
                    return Err(format!("failed to resolve sandbox profile: {error}"));
                }
            }
        } else {
            None
        };

        Ok(Self {
            wrap,
            #[cfg(not(unix))]
            windows_launch,
            #[cfg(unix)]
            child_apply_plan,
            sandbox_profile: sandbox_profile.map(str::to_string),
            workdir: workdir.to_path_buf(),
        })
    }

    #[cfg(all(test, any(target_os = "macos", windows)))]
    pub(crate) fn wrap(&self) -> &devo_sandbox::SandboxWrap {
        &self.wrap
    }

    pub(crate) fn placeholder_dir(&self) -> Option<&Path> {
        match &self.wrap {
            devo_sandbox::SandboxWrap::Wrapped(wrapped) => wrapped.placeholder_dir.as_deref(),
            devo_sandbox::SandboxWrap::None => None,
        }
    }

    /// Schedule delayed removal of a bwrap placeholder directory after spawn.
    ///
    /// bwrap mounts are not up when spawn returns, so the directory must
    /// outlive the launch.
    pub(crate) fn schedule_placeholder_cleanup(&self) {
        let Some(directory) = self.placeholder_dir().map(Path::to_path_buf) else {
            return;
        };

        tokio::spawn(async move {
            tokio::time::sleep(devo_sandbox::PLACEHOLDER_CLEANUP_DELAY).await;
            devo_sandbox::remove_placeholder_dir(&directory);
        });
    }

    /// Build a tokio pipe [`Command`] from this plan (shell + command args).
    pub(crate) fn build_tokio_command(&self, shell: &ShellSpec, command: &str) -> Command {
        // Prefer OS wrapper (`sandbox-exec` / `bwrap` / Windows launcher); else bare shell.
        let mut child = match &self.wrap {
            devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
                let mut child = Command::new(&wrapped.program);
                child
                    .args(&wrapped.prefix_args)
                    .arg(shell.program)
                    .args(shell.args)
                    .arg(command);
                child
            }
            devo_sandbox::SandboxWrap::None => {
                #[cfg(not(unix))]
                if let Some(launch) = &self.windows_launch {
                    let mut child = Command::new(&launch.program);
                    child.args(&launch.args);
                    for (key, value) in &launch.env {
                        child.env(key, value);
                    }
                    child
                } else {
                    let mut child = Command::new(shell.program);
                    child.args(shell.args).arg(command);
                    child
                }
                #[cfg(unix)]
                {
                    let mut child = Command::new(shell.program);
                    child.args(shell.args).arg(command);
                    child
                }
            }
        };

        #[cfg(unix)]
        {
            let sandbox_plan = self.child_apply_plan.clone();
            unsafe {
                // `pre_exec` runs in the child after `fork`, before `exec`. Apply the
                // parent-resolved Landlock/seccomp plan here so only the spawned
                // command is sandboxed (parent stays unrestricted). Config must not
                // be loaded in this hook — resolve above in the parent. Skipped when
                // `sandbox_plan` is `None` (macOS / fully wrapped Linux).
                child.pre_exec(move || {
                    devo_util_process::sandbox::apply_resolved_in_child(sandbox_plan.as_ref())
                });
            }
        }

        #[cfg(unix)]
        for (key, value) in devo_sandbox::proxy_env_for_sandbox_profile(
            self.sandbox_profile.as_deref(),
            &self.workdir,
        ) {
            child.env(key, value);
        }

        child
    }

    /// Build a portable-pty [`CommandBuilder`] from this plan.
    pub(crate) fn build_pty_command_builder(
        &self,
        shell: &ShellSpec,
        command: &str,
    ) -> CommandBuilder {
        let mut builder = match &self.wrap {
            devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
                let mut builder = CommandBuilder::new(&wrapped.program);
                builder.args(&wrapped.prefix_args);
                builder.arg(shell.program);
                builder
            }
            devo_sandbox::SandboxWrap::None => {
                #[cfg(not(unix))]
                if let Some(launch) = &self.windows_launch {
                    let mut builder = CommandBuilder::new(&launch.program);
                    builder.args(
                        launch
                            .args
                            .iter()
                            .map(|arg| arg.as_str())
                            .collect::<Vec<_>>(),
                    );
                    for (key, value) in &launch.env {
                        builder.env(key, value);
                    }
                    builder
                } else {
                    CommandBuilder::new(shell.program)
                }
                #[cfg(unix)]
                CommandBuilder::new(shell.program)
            }
        };

        // Windows sandbox launch already embeds the full command line.
        #[cfg(not(unix))]
        if self.windows_launch.is_none() {
            builder.args(shell.args);
            builder.arg(command);
        }
        #[cfg(unix)]
        {
            builder.args(shell.args);
            builder.arg(command);
        }

        #[cfg(unix)]
        for (key, value) in devo_sandbox::proxy_env_for_sandbox_profile(
            self.sandbox_profile.as_deref(),
            &self.workdir,
        ) {
            builder.env(key, value);
        }

        builder
    }
}

#[cfg(not(unix))]
/// Build a Windows sandbox launcher when the profile requires wrapping.
///
/// Unlike Unix, Windows does not use `SandboxWrap` / `pre_exec`. The
/// `devo_windows_sandbox` crate prepares a process launch with the resolved
/// read-only, read-write, and deny roots (plus optional network restriction)
/// and embeds the shell command line in that launch.
fn try_windows_sandbox_launch(
    sandbox_profile: Option<&str>,
    workdir: &Path,
    shell: &ShellSpec,
    command: &str,
) -> Result<Option<devo_windows_sandbox::WindowsSandboxLaunch>, String> {
    use std::sync::Once;
    if !devo_windows_sandbox::should_wrap_profile(sandbox_profile) {
        return Ok(None);
    }
    let profile = sandbox_profile.expect("checked by should_wrap_profile");
    let profile_name = profile
        .parse::<devo_sandbox::ProfileName>()
        .map_err(|error| format!("invalid sandbox profile '{profile}': {error}"))?;
    let config = devo_sandbox::load_sandbox_config(workdir)
        .map_err(|error| format!("failed to set up Windows sandbox: {error}"))?;
    let resolved = profile_name
        .resolve_profile(workdir, &config)
        .map_err(|error| format!("failed to set up Windows sandbox: {error}"))?;
    let request = devo_windows_sandbox::WindowsSandboxRequest {
        command: command.to_string(),
        shell_program: shell.program.to_string(),
        shell_args: shell.args.iter().map(|arg| arg.to_string()).collect(),
        cwd: workdir.to_path_buf(),
        readable_roots: resolved.read_only,
        writable_roots: resolved.read_write,
        deny_read: resolved.deny,
        restrict_network: resolved.restrict_network,
    };
    match devo_windows_sandbox::prepare_windows_sandbox_launch(&request) {
        Ok(Some(launch)) => Ok(Some(launch)),
        Ok(None) => {
            static WARNED: Once = Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "Windows sandbox profile is active but launch preparation is not wired yet; \
                     commands run unwrapped"
                );
            });
            Ok(None)
        }
        Err(error) => Err(format!("failed to set up Windows sandbox: {error}")),
    }
}
