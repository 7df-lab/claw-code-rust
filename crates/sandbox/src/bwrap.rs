//! Linux bwrap helpers: the process re-exec with fail-closed read-deny
//! placeholders, and the shared argv builder for wrapped command launches
//! (see [`crate::wrap`]). Placeholder machinery lives in
//! [`crate::bwrap_placeholder`].

#[cfg(target_os = "linux")]
use crate::bwrap_placeholder;
#[cfg(all(feature = "enforce", target_os = "linux"))]
use crate::deny;
#[cfg(any(target_os = "linux", all(feature = "enforce", unix)))]
use crate::profiles;
use crate::profiles::ProfileName;
#[cfg(target_os = "linux")]
use crate::profiles::{SandboxConfig, SandboxProfile};
#[cfg(target_os = "linux")]
use crate::wrap::WrapMode;
#[cfg(target_os = "linux")]
use anyhow::Context;
use std::path::Path;
use std::path::PathBuf;

const BWRAP_ENV_VAR: &str = "__DEVO_INSIDE_BWRAP";
const BWRAP_MARKER_VALUE: &str = "devo-bwrap-v1";

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountNamespaceIsolation {
    Isolated,
    SharedWithPidOne,
    Unknown,
}

/// Whether this process is the bwrap re-exec created by this crate.
pub fn is_inside_bwrap() -> bool {
    is_inside_bwrap_with_marker(std::env::var_os(BWRAP_ENV_VAR).as_deref())
}

fn is_inside_bwrap_with_marker(marker: Option<&std::ffi::OsStr>) -> bool {
    #[cfg(target_os = "linux")]
    {
        // A bwrap re-exec always creates a new mount namespace. Reject the
        // marker unless procfs affirmatively proves that isolation boundary.
        return has_bwrap_marker_and_mount_namespace(marker, bwrap_mount_namespace_isolation());
    }

    #[cfg(not(target_os = "linux"))]
    {
        has_exact_bwrap_marker(marker)
    }
}

fn has_exact_bwrap_marker(marker: Option<&std::ffi::OsStr>) -> bool {
    marker.is_some_and(|value| value == std::ffi::OsStr::new(BWRAP_MARKER_VALUE))
}

#[cfg(any(target_os = "linux", test))]
fn has_bwrap_marker_and_mount_namespace(
    marker: Option<&std::ffi::OsStr>,
    namespace_isolation: MountNamespaceIsolation,
) -> bool {
    has_exact_bwrap_marker(marker)
        && matches!(namespace_isolation, MountNamespaceIsolation::Isolated)
}

#[cfg(target_os = "linux")]
fn bwrap_mount_namespace_isolation() -> MountNamespaceIsolation {
    let Ok(current) = std::fs::read_link("/proc/self/ns/mnt") else {
        return MountNamespaceIsolation::Unknown;
    };
    let Ok(pid_one) = std::fs::read_link("/proc/1/ns/mnt") else {
        return MountNamespaceIsolation::Unknown;
    };
    if current == pid_one {
        MountNamespaceIsolation::SharedWithPidOne
    } else {
        MountNamespaceIsolation::Isolated
    }
}

/// Kept closed until a separate devbox-specific trust policy is introduced.
pub fn trust_bwrap_marker_for_devbox() -> bool {
    false
}

/// Options for a bwrap launch built by [`bwrap_base_argv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BwrapOpts {
    /// Block all network access by unsharing into an empty network namespace.
    pub unshare_net: bool,
    /// Kill the sandboxed process when the parent (devo) process dies.
    pub die_with_parent: bool,
    /// Unshare the user namespace (always set for container uid0).
    pub unshare_user: bool,
    /// Unshare the PID namespace (always set).
    pub unshare_pid: bool,
    /// Start a new session (always passes `--new-session`).
    pub new_session: bool,
    /// Mount `/proc` via `--proc /proc`. Set false after a failed proc mount
    /// preflight (`--no-proc` retry path).
    pub mount_proc: bool,
    /// Full filesystem policy carried by bwrap (PTY wraps). `None` selects the
    /// identity view `--bind / /`: the write policy is then left to the
    /// Landlock/Seatbelt applied via `pre_exec` inside the child (pipe wraps).
    pub full_fs_plan: Option<FsPlan>,
}

impl Default for BwrapOpts {
    fn default() -> Self {
        Self {
            unshare_net: false,
            // Default process hygiene flags for every bwrap launch.
            die_with_parent: true,
            unshare_user: true,
            unshare_pid: true,
            new_session: true,
            mount_proc: true,
            full_fs_plan: None,
        }
    }
}

/// Filesystem layout for a bwrap launch, derived from a resolved profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsPlan {
    /// `true` (workspace/devbox/read-only): `--ro-bind / /` plus a writable
    /// `--bind` for each `read_write` root. `false` (strict): `--tmpfs /`
    /// plus explicit `--ro-bind` / `--bind` roots.
    pub default_read: bool,
    pub read_only: Vec<String>,
    pub read_write: Vec<String>,
}

/// Deny binds mount after every fs-plan bind so they shadow any broader
/// grant. Default namespace hygiene flags (`--new-session`,
/// `--unshare-user` / `--unshare-pid`, `--die-with-parent`) are emitted by
/// default. `/dev` uses `--dev` (minimal device nodes) rather than
/// `--dev-bind`. Returns the argv plus the per-launch placeholder directory
/// (created lazily, at most one per launch) so the caller can clean it up once
/// the mounts are up.
fn bwrap_base_argv(
    deny_write: &[String],
    deny_read: &[String],
    opts: BwrapOpts,
) -> anyhow::Result<(Vec<String>, Option<PathBuf>)> {
    let mut argv: Vec<String> = Vec::new();
    // Session/namespace hygiene precedes filesystem binds.
    if opts.new_session {
        argv.push("--new-session".to_string());
    }
    if opts.die_with_parent {
        argv.push("--die-with-parent".to_string());
    }
    if opts.unshare_pid {
        argv.push("--unshare-pid".to_string());
    }
    if opts.unshare_user {
        argv.push("--unshare-user".to_string());
    }
    if opts.unshare_net {
        argv.push("--unshare-net".to_string());
    }

    match &opts.full_fs_plan {
        None => argv.extend(["--bind".to_string(), "/".to_string(), "/".to_string()]),
        Some(plan) if plan.default_read => {
            argv.extend(["--ro-bind".to_string(), "/".to_string(), "/".to_string()]);
            for path in &plan.read_write {
                argv.extend(["--bind".to_string(), path.clone(), path.clone()]);
            }
        }
        Some(plan) => {
            argv.extend(["--tmpfs".to_string(), "/".to_string()]);
            for path in &plan.read_only {
                argv.extend(["--ro-bind".to_string(), path.clone(), path.clone()]);
            }
            for path in &plan.read_write {
                argv.extend(["--bind".to_string(), path.clone(), path.clone()]);
            }
        }
    }
    for path in deny_write {
        if Path::new(path).exists() {
            argv.extend(["--ro-bind".to_string(), path.clone(), path.clone()]);
        }
    }

    #[cfg(target_os = "linux")]
    let placeholder_dir = {
        let mut directory: Option<PathBuf> = None;
        for (index, path) in deny_read.iter().enumerate() {
            if directory.is_none() {
                directory = Some(
                    bwrap_placeholder::create_private_bwrap_placeholder_directory().with_context(
                        || {
                            format!(
                                "could not create bwrap placeholder for read-deny path {path}; \
                                 refusing to start with a partial sandbox"
                            )
                        },
                    )?,
                );
            }
            let parent = directory
                .as_ref()
                .expect("placeholder directory created above");
            let blocked =
                bwrap_placeholder::bwrap_blocked_source_in(parent, Path::new(path), index)
                    .with_context(|| {
                        format!(
                            "could not create bwrap placeholder for read-deny path {path}; \
                         refusing to start with a partial sandbox"
                        )
                    })?;
            argv.push("--ro-bind".to_string());
            argv.push(blocked.display().to_string());
            argv.push(path.clone());
        }
        directory
    };
    #[cfg(not(target_os = "linux"))]
    let placeholder_dir: Option<PathBuf> = None;
    #[cfg(not(target_os = "linux"))]
    let _ = deny_read;

    // `--dev /dev` mounts a minimal writable /dev with standard nodes
    // (not the host's full `--dev-bind` tree).
    argv.extend(["--dev".to_string(), "/dev".to_string()]);
    if opts.mount_proc {
        argv.extend(["--proc".to_string(), "/proc".to_string()]);
    }
    Ok((argv, placeholder_dir))
}

/// Build a bwrap command that re-execs the current process with `deny_write`
/// paths mounted read-only and `deny_read` paths bound over with an unreadable
/// placeholder (EPERM on read).
///
/// Returns `Ok(None)` only if already inside bwrap. Caller should `cmd.exec()`
/// the returned command; construction failures are returned as errors.
pub fn bwrap_reexec_command(
    deny_write: &[&str],
    deny_read: &[&str],
) -> anyhow::Result<Option<std::process::Command>> {
    bwrap_reexec_command_with_state(deny_write, deny_read, is_inside_bwrap())
}

fn bwrap_reexec_command_with_state(
    deny_write: &[&str],
    deny_read: &[&str],
    inside_bwrap: bool,
) -> anyhow::Result<Option<std::process::Command>> {
    if inside_bwrap {
        return Ok(None);
    }
    let self_exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let deny_write: Vec<String> = deny_write.iter().map(|path| path.to_string()).collect();
    let deny_read: Vec<String> = deny_read.iter().map(|path| path.to_string()).collect();
    // The placeholder dir (if any) survives the exec: the process image is
    // replaced, so no in-process cleanup can run — the startup janitor in
    // `wrap` removes it once it is stale.
    let (argv, _placeholder_dir) = bwrap_base_argv(&deny_write, &deny_read, BwrapOpts::default())?;
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(argv);
    cmd.env(BWRAP_ENV_VAR, BWRAP_MARKER_VALUE);
    cmd.arg("--").arg(self_exe).args(args);
    Ok(Some(cmd))
}

/// Whether kernel read-deny enforcement is required. Configuration errors are
/// returned rather than being converted into an unsandboxed fallback.
#[cfg(all(feature = "enforce", unix))]
pub fn requires_read_deny(profile: &ProfileName, workspace: &Path) -> anyhow::Result<bool> {
    match profile {
        ProfileName::Custom(name) => {
            let config = profiles::load_sandbox_config(workspace)?;
            Ok(config
                .profiles
                .get(name)
                .is_some_and(|profile| !profile.deny.is_empty()))
        }
        _ => Ok(false),
    }
}

/// Stub when `enforce` is unavailable — nothing is kernel-enforced.
#[cfg(not(all(feature = "enforce", unix)))]
pub fn requires_read_deny(_profile: &ProfileName, _workspace: &Path) -> anyhow::Result<bool> {
    Ok(false)
}

/// A profile's resolved bwrap deny plan.
#[cfg(target_os = "linux")]
struct BwrapDenyPlan {
    deny_write: Vec<String>,
    deny_read: Vec<String>,
    has_globs: bool,
}

/// Resolve a profile's `deny` entries into bwrap read-deny bind targets:
/// exact paths resolved/sorted/deduped, globs expanded at launch. Also
/// returns whether any glob entries were seen — globs are best-effort (their
/// expansion cannot cover files created after launch), so a glob-only deny
/// still requires the bwrap launch even when it expands to nothing.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn deny_read_strings(workspace: &Path, entries: &[PathBuf]) -> anyhow::Result<(Vec<String>, bool)> {
    let (exact, globs) = deny::partition_deny_entries(entries);
    let mut deny_read = deny::exact_deny_path_strings(workspace, &exact);
    let has_globs = !globs.is_empty();
    if has_globs {
        tracing::warn!(
            count = globs.len(),
            "sandbox deny globs are enforced best-effort on Linux (expanded at launch); \
             files matching them that are created later are NOT covered"
        );
        let expanded = deny::expand_deny_globs(
            workspace,
            &globs,
            deny::DENY_GLOB_MAX_DEPTH,
            deny::DENY_GLOB_MAX_MATCHES,
            deny::DENY_GLOB_MAX_ENTRIES,
        )
        .ok_or_else(|| anyhow::anyhow!("sandbox deny glob expansion exceeded safety limits"))?;
        deny_read.extend(expanded);
    }
    Ok((deny_read, has_globs))
}

/// Without kernel enforcement there is nothing to bind over.
#[cfg(all(not(feature = "enforce"), target_os = "linux"))]
fn deny_read_strings(
    _workspace: &Path,
    _entries: &[PathBuf],
) -> anyhow::Result<(Vec<String>, bool)> {
    Ok((Vec::new(), false))
}

/// Resolve the bwrap deny plan in one checked config read.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn bwrap_deny_plan(profile: &ProfileName, workspace: &Path) -> anyhow::Result<BwrapDenyPlan> {
    let config = profiles::load_sandbox_config(workspace)?;
    let deny_write: Vec<String> = if is_devbox_based(profile, &config) {
        vec!["/data".to_string()]
    } else {
        Vec::new()
    };
    let entries = if *profile == ProfileName::Off {
        Vec::new()
    } else {
        profile.resolve_profile(workspace, &config)?.deny
    };
    let (deny_read, has_globs) = deny_read_strings(workspace, &entries)?;
    Ok(BwrapDenyPlan {
        deny_write,
        deny_read,
        has_globs,
    })
}

/// Without kernel enforcement, preserve the devbox `/data` write-deny mount.
#[cfg(all(not(feature = "enforce"), target_os = "linux"))]
fn bwrap_deny_plan(profile: &ProfileName, workspace: &Path) -> anyhow::Result<BwrapDenyPlan> {
    let config = profiles::load_sandbox_config(workspace)?;
    let deny_write = if is_devbox_based(profile, &config) {
        vec!["/data".to_string()]
    } else {
        Vec::new()
    };
    Ok(BwrapDenyPlan {
        deny_write,
        deny_read: Vec::new(),
        has_globs: false,
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn is_devbox_based(profile: &ProfileName, config: &SandboxConfig) -> bool {
    match profile {
        ProfileName::Devbox => true,
        ProfileName::Custom(name) => {
            config.profiles.get(name).and_then(|p| p.extends.as_deref()) == Some("devbox")
        }
        _ => false,
    }
}

/// Build the bwrap re-exec command needed on Linux. It returns `Ok(None)` only
/// if no mount-namespace enforcement is needed or we are already inside bwrap;
/// configuration, glob-expansion, and placeholder failures are errors.
#[cfg(target_os = "linux")]
pub fn bwrap_reexec_for_profile(
    profile: &ProfileName,
    workspace: &Path,
) -> anyhow::Result<Option<std::process::Command>> {
    bwrap_reexec_for_profile_with_state(profile, workspace, is_inside_bwrap())
}

#[cfg(target_os = "linux")]
fn bwrap_reexec_for_profile_with_state(
    profile: &ProfileName,
    workspace: &Path,
    inside_bwrap: bool,
) -> anyhow::Result<Option<std::process::Command>> {
    let BwrapDenyPlan {
        deny_write,
        deny_read,
        has_globs,
    } = bwrap_deny_plan(profile, workspace)?;
    if deny_write.is_empty() && deny_read.is_empty() && !has_globs {
        return Ok(None);
    }
    let write_refs: Vec<&str> = deny_write.iter().map(String::as_str).collect();
    let read_refs: Vec<&str> = deny_read.iter().map(String::as_str).collect();
    bwrap_reexec_command_with_state(&write_refs, &read_refs, inside_bwrap)
}

/// How to treat a bind source that does not exist at launch time.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingSource {
    /// Read-only roots: nothing to read, so skip (matches the nono capability path).
    Skip,
    /// Writable roots: pre-create the directory (matches the nono capability
    /// path, which creates missing `read_write` dirs before granting them).
    CreateDirAll,
}

/// Stringify bind sources for a bwrap fs plan; bwrap fails the whole launch on
/// a missing source, so non-existent paths are skipped or pre-created first.
#[cfg(target_os = "linux")]
fn bind_sources(paths: &[PathBuf], missing: MissingSource) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| {
            if !path.exists() {
                match missing {
                    MissingSource::Skip => return None,
                    MissingSource::CreateDirAll => {
                        if std::fs::create_dir_all(path).is_err() {
                            tracing::warn!(path = ?path, "read_write path does not exist and could not be created, skipping");
                            return None;
                        }
                    }
                }
            }
            match path.to_str() {
                Some(path) => Some(path.to_string()),
                None => {
                    tracing::warn!(path = ?path, "Skipping non-UTF8 path in bwrap fs plan");
                    None
                }
            }
        })
        .collect()
}

/// Build the bwrap argv prefix (up to and including `--`) that enforces
/// `resolved` for a wrapped command launch, plus the placeholder directory
/// created for read-deny bind-overs (if any).
///
/// Unlike the re-exec path, no `__DEVO_INSIDE_BWRAP` trust marker is set:
/// wrapped launches are ordinary command spawns, not the process re-exec the
/// marker authenticates.
#[cfg(target_os = "linux")]
pub(crate) fn bwrap_wrap_argv(
    workspace: &Path,
    resolved: &SandboxProfile,
    devbox_based: bool,
    mode: WrapMode,
) -> anyhow::Result<(Vec<String>, Option<PathBuf>)> {
    // devbox keeps `/data` write-denied via a read-only bind over itself, as
    // in the re-exec path (it is excluded from `read_write`, never a profile
    // kernel-deny, so it stays readable).
    let deny_write = if devbox_based {
        vec!["/data".to_string()]
    } else {
        Vec::new()
    };
    let (deny_read, _has_globs) = deny_read_strings(workspace, &resolved.deny)?;
    let full_fs_plan = match mode {
        // Pipe children get their fs policy from the pre_exec Landlock inside
        // the bwrap; the wrapper only adds deny bind-overs and network
        // restriction, so the identity view keeps the mount layer permissive.
        WrapMode::PipeComposed => None,
        // PTY children have no pre_exec hook: bwrap carries the full policy.
        WrapMode::PtyOnly => Some(FsPlan {
            default_read: resolved.default_read,
            read_only: bind_sources(&resolved.read_only, MissingSource::Skip),
            read_write: bind_sources(&resolved.read_write, MissingSource::CreateDirAll),
        }),
    };
    let (mut argv, placeholder_dir) = bwrap_base_argv(
        &deny_write,
        &deny_read,
        BwrapOpts {
            // PipeComposed children get Landlock/seccomp ProxyOnly via pre_exec,
            // so skipping --unshare-net keeps the host loopback proxy reachable.
            // PtyOnly has no pre_exec Landlock path: always unshare when the
            // profile restricts network, even if managed proxy ports are set.
            unshare_net: resolved.restrict_network
                && match mode {
                    WrapMode::PtyOnly => true,
                    WrapMode::PipeComposed => {
                        crate::managed_network::managed_network_context_from_env()
                            .loopback_ports
                            .is_empty()
                    }
                },
            full_fs_plan,
            ..BwrapOpts::default()
        },
    )?;
    argv.push("--".to_string());
    Ok((argv, placeholder_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn argv_strings(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn bwrap_reexec_returns_none_inside_bwrap() {
        let result = bwrap_reexec_command_with_state(&["/data"], &[], /*inside_bwrap*/ true)
            .expect("build bwrap command");
        assert!(result.is_none());
    }

    #[test]
    fn bwrap_reexec_returns_some_outside_bwrap() {
        let cmd = bwrap_reexec_command_with_state(&["/tmp"], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        assert_eq!(cmd.get_program(), "bwrap");
    }

    #[test]
    fn bwrap_reexec_writes_the_exact_marker_value() {
        let cmd = bwrap_reexec_command_with_state(&[], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let marker = cmd.get_envs().find_map(|(name, value)| {
            if name == std::ffi::OsStr::new(BWRAP_ENV_VAR) {
                value
            } else {
                None
            }
        });

        assert_eq!(marker, Some(std::ffi::OsStr::new(BWRAP_MARKER_VALUE)));
    }

    #[test]
    fn bwrap_marker_requires_the_exact_value_without_mutating_environment() {
        assert!(!has_exact_bwrap_marker(/*marker*/ None));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new(""))));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new("0"))));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new("wrong"))));
        assert!(has_exact_bwrap_marker(Some(std::ffi::OsStr::new(
            BWRAP_MARKER_VALUE
        ))));
    }

    #[test]
    fn bwrap_marker_requires_affirmative_mount_namespace_isolation() {
        let marker = Some(std::ffi::OsStr::new(BWRAP_MARKER_VALUE));
        assert!(!has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::Unknown
        ));
        assert!(!has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::SharedWithPidOne
        ));
        assert!(has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::Isolated
        ));
        assert!(!has_bwrap_marker_and_mount_namespace(
            Some(std::ffi::OsStr::new("wrong")),
            MountNamespaceIsolation::Isolated
        ));
    }

    #[test]
    fn bwrap_reexec_skips_nonexistent_paths() {
        let cmd = bwrap_reexec_command_with_state(
            &["/nonexistent-test-path-xyz-12345"],
            &[],
            /*inside_bwrap*/ false,
        )
        .expect("build bwrap command")
        .expect("bwrap command outside bwrap");
        let args = argv_strings(&cmd);
        assert!(
            !args
                .iter()
                .any(|arg| arg == "/nonexistent-test-path-xyz-12345")
        );
    }

    #[test]
    fn bwrap_reexec_mounts_existing_paths_read_only() {
        let tmp = std::env::temp_dir();
        let tmp_s = tmp.to_string_lossy();
        let cmd =
            bwrap_reexec_command_with_state(&[tmp_s.as_ref()], &[], /*inside_bwrap*/ false)
                .expect("build bwrap command")
                .expect("bwrap command outside bwrap");
        let args = argv_strings(&cmd);
        assert!(
            args.windows(3)
                .any(|window| window == ["--ro-bind", tmp_s.as_ref(), tmp_s.as_ref()])
        );
    }

    #[test]
    fn bwrap_reexec_uses_minimal_dev() {
        let cmd = bwrap_reexec_command_with_state(&[], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let args = argv_strings(&cmd);
        assert!(
            args.windows(2).any(|window| window == ["--dev", "/dev"]),
            "expected --dev /dev, got {args:?}"
        );
        assert!(
            !args
                .windows(3)
                .any(|window| window == ["--dev-bind", "/dev", "/dev"]),
            "must not use host --dev-bind: {args:?}"
        );
    }

    #[test]
    fn bwrap_base_argv_identity_plan_binds_root_read_write() {
        let (argv, placeholder_dir) =
            bwrap_base_argv(&[], &[], BwrapOpts::default()).expect("build identity argv");
        assert_eq!(placeholder_dir, None);
        assert!(argv.windows(3).any(|window| window == ["--bind", "/", "/"]));
        assert!(argv.windows(2).any(|window| window == ["--dev", "/dev"]));
        assert!(argv.windows(2).any(|window| window == ["--proc", "/proc"]));
        assert!(argv.iter().any(|arg| arg == "--new-session"));
        assert!(argv.iter().any(|arg| arg == "--unshare-user"));
        assert!(argv.iter().any(|arg| arg == "--unshare-pid"));
        assert!(argv.iter().any(|arg| arg == "--die-with-parent"));
        assert!(!argv.iter().any(|arg| arg == "--unshare-net"));
    }

    #[test]
    fn bwrap_base_argv_default_read_plan_ro_binds_root_then_upgrades_writable_roots() {
        let plan = FsPlan {
            default_read: true,
            read_only: vec![],
            read_write: vec!["/ws".to_string(), "/home/u/.devo".to_string()],
        };
        let (argv, _) = bwrap_base_argv(
            &[],
            &[],
            BwrapOpts {
                full_fs_plan: Some(plan),
                ..BwrapOpts::default()
            },
        )
        .expect("build default-read argv");
        let root = argv
            .windows(3)
            .position(|window| window == ["--ro-bind", "/", "/"])
            .expect("root ro-bind");
        let ws = argv
            .windows(3)
            .position(|window| window == ["--bind", "/ws", "/ws"])
            .expect("workspace bind");
        let home = argv
            .windows(3)
            .position(|window| window == ["--bind", "/home/u/.devo", "/home/u/.devo"])
            .expect("devo home bind");
        assert!(root < ws && ws < home, "argv: {argv:?}");
    }

    #[test]
    fn bwrap_base_argv_strict_plan_tmpfs_then_read_only_then_read_write() {
        let plan = FsPlan {
            default_read: false,
            read_only: vec!["/usr".to_string()],
            read_write: vec!["/ws".to_string()],
        };
        let (argv, _) = bwrap_base_argv(
            &[],
            &[],
            BwrapOpts {
                full_fs_plan: Some(plan),
                ..BwrapOpts::default()
            },
        )
        .expect("build strict argv");
        let tmpfs = argv
            .windows(2)
            .position(|window| window == ["--tmpfs", "/"])
            .expect("tmpfs root");
        let ro = argv
            .windows(3)
            .position(|window| window == ["--ro-bind", "/usr", "/usr"])
            .expect("read-only bind");
        let rw = argv
            .windows(3)
            .position(|window| window == ["--bind", "/ws", "/ws"])
            .expect("read-write bind");
        assert!(tmpfs < ro && ro < rw, "argv: {argv:?}");
        assert!(
            !argv.windows(3).any(|window| window == ["--bind", "/", "/"]),
            "strict plan must not expose the host root: {argv:?}"
        );
    }

    #[test]
    fn bwrap_base_argv_emits_network_and_hygiene_flags() {
        let (argv, _) = bwrap_base_argv(
            &[],
            &[],
            BwrapOpts {
                unshare_net: true,
                ..BwrapOpts::default()
            },
        )
        .expect("build flagged argv");
        assert!(argv.iter().any(|arg| arg == "--unshare-net"));
        assert!(argv.iter().any(|arg| arg == "--die-with-parent"));
        assert!(argv.iter().any(|arg| arg == "--new-session"));
        assert!(argv.iter().any(|arg| arg == "--unshare-user"));
        assert!(argv.iter().any(|arg| arg == "--unshare-pid"));
        // Hygiene flags precede filesystem binds (before filesystem binds).
        let new_session = argv.iter().position(|arg| arg == "--new-session").unwrap();
        let root_bind = argv
            .windows(3)
            .position(|window| window == ["--bind", "/", "/"])
            .unwrap();
        assert!(new_session < root_bind, "argv: {argv:?}");
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_base_argv_mounts_deny_placeholders_after_fs_plan() {
        let workspace = temp_workspace_with_sandbox_toml("placeholder-order", "");
        let denied = workspace.join("secret.txt");
        std::fs::write(&denied, "secret").expect("write denied file");
        let plan = FsPlan {
            default_read: true,
            read_only: vec![],
            read_write: vec![workspace.display().to_string()],
        };
        let (argv, placeholder_dir) = bwrap_base_argv(
            &[],
            &[denied.display().to_string()],
            BwrapOpts {
                full_fs_plan: Some(plan),
                ..BwrapOpts::default()
            },
        )
        .expect("build deny argv");
        let workspace_bind = [
            "--bind".to_string(),
            workspace.display().to_string(),
            workspace.display().to_string(),
        ];
        let rw = argv
            .windows(3)
            .position(|window| window == workspace_bind)
            .expect("workspace bind");
        let deny = argv
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind" && window[2] == denied.display().to_string()
            })
            .expect("deny placeholder bind");
        assert!(rw < deny, "placeholder must mount last: {argv:?}");
        let placeholder_dir = placeholder_dir.expect("placeholder dir created");
        assert!(placeholder_dir.is_dir());
        let _ = std::fs::remove_dir_all(&placeholder_dir);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[cfg(all(feature = "enforce", unix))]
    fn temp_workspace_with_sandbox_toml(tag: &str, toml_body: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("devo-{tag}-{}-{nanos}", std::process::id()));
        let devo = workspace.join(".devo");
        std::fs::create_dir_all(&devo).expect("create sandbox config directory");
        std::fs::write(devo.join("sandbox.toml"), toml_body).expect("write sandbox config");
        workspace
    }

    #[cfg(all(feature = "enforce", unix))]
    fn temp_workspace_with_deny(tag: &str, deny_toml: &str) -> PathBuf {
        temp_workspace_with_sandbox_toml(
            tag,
            &format!("[profiles.denytest]\nextends = \"workspace\"\ndeny = [{deny_toml}]\n"),
        )
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn requires_read_deny_only_for_custom_profile_with_deny() {
        let workspace = temp_workspace_with_deny("requires-deny", "\".env\"");
        assert!(
            requires_read_deny(&ProfileName::Custom("denytest".to_string()), &workspace)
                .expect("load sandbox config")
        );
        assert!(
            !requires_read_deny(&ProfileName::Custom("undefined".to_string()), &workspace)
                .expect("load sandbox config")
        );
        assert!(!requires_read_deny(&ProfileName::Workspace, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Strict, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Devbox, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Off, &workspace).expect("load config"));
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_reexec_binds_nonexistent_deny_read_paths() {
        let missing = "/nonexistent-deny-read-path-xyz-12345";
        let cmd = bwrap_reexec_command_with_state(&[], &[missing], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let args = argv_strings(&cmd);
        assert!(
            args.windows(3)
                .any(|window| window[0] == "--ro-bind" && window[2] == missing)
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_reexec_for_profile_devbox_extends_composes_data_and_read_deny() {
        let workspace = temp_workspace_with_sandbox_toml(
            "devbox-compose",
            "[profiles.devcustom]\nextends = \"devbox\"\ndeny = [\"secret.pem\"]\n",
        );
        let cmd = bwrap_reexec_for_profile_with_state(
            &ProfileName::Custom("devcustom".to_string()),
            &workspace,
            /*inside_bwrap*/ false,
        )
        .expect("load sandbox config")
        .expect("build bwrap re-exec command");
        let args = argv_strings(&cmd);
        let deny_path = workspace.join("secret.pem").to_string_lossy().to_string();
        assert!(
            args.windows(3)
                .any(|window| window[0] == "--ro-bind" && window[2] == deny_path)
        );
        if Path::new("/data").exists() {
            assert!(
                args.windows(3)
                    .any(|window| window == ["--ro-bind", "/data", "/data"])
            );
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_wrap_argv_builds_prefix_without_reexec_marker() {
        let workspace = temp_workspace_with_deny("wrap-argv", "\"secret.txt\"");
        let config = profiles::load_sandbox_config(&workspace).expect("load sandbox config");
        let resolved = ProfileName::Custom("denytest".to_string())
            .resolve_profile(&workspace, &config)
            .expect("resolve profile");
        let (argv, placeholder_dir) = bwrap_wrap_argv(
            &workspace,
            &resolved,
            /*devbox_based*/ false,
            WrapMode::PtyOnly,
        )
        .expect("build wrap argv");
        assert_eq!(argv.last().map(String::as_str), Some("--"));
        assert!(!argv.iter().any(|arg| arg.contains(BWRAP_MARKER_VALUE)));
        let deny_path = workspace.join("secret.txt").display().to_string();
        assert!(
            argv.windows(3)
                .any(|window| window[0] == "--ro-bind" && window[2] == deny_path),
            "deny placeholder bind missing: {argv:?}"
        );
        assert!(
            argv.windows(3)
                .any(|window| window == ["--ro-bind", "/", "/"]),
            "PTY wrap carries the full fs plan: {argv:?}"
        );
        if let Some(directory) = placeholder_dir {
            let _ = std::fs::remove_dir_all(&directory);
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_wrap_argv_unshares_net_only_for_restricted_profiles() {
        let workspace = std::env::current_dir().expect("cwd");
        let config = SandboxConfig::default();
        let restricted = ProfileName::ReadOnly
            .resolve_profile(&workspace, &config)
            .expect("resolve read-only");
        let (argv, _) = bwrap_wrap_argv(
            &workspace,
            &restricted,
            /*devbox_based*/ false,
            WrapMode::PtyOnly,
        )
        .expect("build read-only wrap argv");
        assert!(argv.iter().any(|arg| arg == "--unshare-net"));

        let open = ProfileName::Workspace
            .resolve_profile(&workspace, &config)
            .expect("resolve workspace");
        let (argv, _) = bwrap_wrap_argv(
            &workspace,
            &open,
            /*devbox_based*/ false,
            WrapMode::PtyOnly,
        )
        .expect("build workspace wrap argv");
        assert!(!argv.iter().any(|arg| arg == "--unshare-net"));
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    #[serial_test::serial(sandbox_proxy_ports)]
    fn bwrap_wrap_argv_pipe_mode_uses_identity_fs_plan() {
        // PipeComposed skips --unshare-net when managed proxy ports are published;
        // clear the process-global store so this assertion is order-independent.
        crate::managed_network::set_sandbox_proxy_ports(&[]);
        let workspace = std::env::current_dir().expect("cwd");
        let config = SandboxConfig::default();
        let resolved = ProfileName::ReadOnly
            .resolve_profile(&workspace, &config)
            .expect("resolve read-only");
        let (argv, _) = bwrap_wrap_argv(
            &workspace,
            &resolved,
            /*devbox_based*/ false,
            WrapMode::PipeComposed,
        )
        .expect("build pipe wrap argv");
        assert!(
            argv.windows(3).any(|window| window == ["--bind", "/", "/"]),
            "pipe wrap keeps the permissive identity view (pre_exec Landlock enforces): {argv:?}"
        );
        assert!(argv.iter().any(|arg| arg == "--unshare-net"));
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    #[serial_test::serial(sandbox_proxy_ports)]
    fn bwrap_wrap_argv_pty_unshares_net_even_with_proxy_ports() {
        let workspace = std::env::current_dir().expect("cwd");
        let config = SandboxConfig::default();
        let restricted = ProfileName::ReadOnly
            .resolve_profile(&workspace, &config)
            .expect("resolve read-only");
        crate::managed_network::set_sandbox_proxy_ports(&[18080]);
        let (argv, _) = bwrap_wrap_argv(
            &workspace,
            &restricted,
            /*devbox_based*/ false,
            WrapMode::PtyOnly,
        )
        .expect("build pty wrap argv");
        assert!(
            argv.iter().any(|arg| arg == "--unshare-net"),
            "PTY must unshare net even when proxy ports are published: {argv:?}"
        );

        crate::managed_network::set_sandbox_proxy_ports(&[18080]);
        let (pipe_argv, _) = bwrap_wrap_argv(
            &workspace,
            &restricted,
            /*devbox_based*/ false,
            WrapMode::PipeComposed,
        )
        .expect("build pipe wrap argv");
        assert!(
            !pipe_argv.iter().any(|arg| arg == "--unshare-net"),
            "PipeComposed may skip unshare when proxy ports are set: {pipe_argv:?}"
        );
        crate::managed_network::set_sandbox_proxy_ports(&[]);
    }
}
