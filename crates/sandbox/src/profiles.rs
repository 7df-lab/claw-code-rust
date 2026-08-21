//! Sandbox profiles. Built-in: `workspace`, `devbox`, `read-only`, `strict`,
//! `off`. Custom profiles via `~/.devo/sandbox.toml` or `.devo/sandbox.toml`.
//! A custom profile's `deny` list is kernel-enforced (read + write/rename) on
//! both platforms.

use anyhow::Context;
#[cfg(all(feature = "enforce", unix))]
use nono::{AccessMode, CapabilitySet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(all(feature = "enforce", unix))]
use crate::deny::{
    apply_deny_globs_to_capability_set, apply_deny_paths_to_capability_set, effective_deny_paths,
    partition_deny_entries,
};
use crate::paths::devo_home;
#[cfg(all(feature = "enforce", unix))]
use crate::paths::{DEVICE_DIRS, DEVICE_FILES};
use crate::paths::{essential_writable_paths, essential_writable_paths_minimal};
#[cfg(all(feature = "enforce", unix))]
use crate::permissions::SandboxPermissionOverlay;

/// A resolved sandbox profile ready to be converted to a `CapabilitySet`.
#[derive(Debug, Clone)]
pub struct SandboxProfile {
    /// Display name
    pub name: String,
    /// Paths the agent can read (but not write)
    pub read_only: Vec<PathBuf>,
    /// Paths the agent can read and write
    pub read_write: Vec<PathBuf>,
    /// Paths denied entirely (overrides read_only/read_write)
    pub deny: Vec<PathBuf>,
    /// Whether to grant read access to the entire filesystem by default
    pub default_read: bool,
    /// Whether child processes should have network blocked
    pub restrict_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProfileConfig {
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub restrict_network: Option<bool>,
    #[serde(default)]
    pub read_only: Vec<String>,
    #[serde(default)]
    pub read_write: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct SandboxConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProfileName {
    #[default]
    Workspace,
    Devbox,
    ReadOnly,
    Strict,
    Off,
    Custom(String),
}

impl std::fmt::Display for ProfileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace => write!(f, "workspace"),
            Self::Devbox => write!(f, "devbox"),
            Self::ReadOnly => write!(f, "read-only"),
            Self::Strict => write!(f, "strict"),
            Self::Off => write!(f, "off"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

impl std::str::FromStr for ProfileName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "workspace" => Ok(Self::Workspace),
            "devbox" => Ok(Self::Devbox),
            "read-only" | "readonly" => Ok(Self::ReadOnly),
            "strict" => Ok(Self::Strict),
            "off" | "none" => Ok(Self::Off),
            // Anything else is treated as a custom profile name.
            // Validation happens when we try to load it from config.
            other => Ok(Self::Custom(other.to_string())),
        }
    }
}

/// Load sandbox config from `~/.devo/sandbox.toml` and `.devo/sandbox.toml`.
///
/// Project config may **add** new profile names only. It cannot redefine a
/// name already present in the global config — last-write-wins would let a
/// malicious workspace hollow out a user/enterprise custom profile (e.g.
/// empty `deny` / broad `read_write`) while keeping the trusted name.
///
/// Missing files are treated as absent; all other I/O and TOML errors are
/// returned without merging project profiles.
pub fn load_sandbox_config(workspace: &Path) -> anyhow::Result<SandboxConfig> {
    let global_path = devo_home()?.join("sandbox.toml");
    let project_path = workspace.join(".devo").join("sandbox.toml");
    load_sandbox_config_from_paths(&global_path, &project_path)
}

pub fn sandbox_profile_conflicts(workspace: &Path) -> anyhow::Result<Vec<String>> {
    let global_path = devo_home()?.join("sandbox.toml");
    let project_path = workspace.join(".devo").join("sandbox.toml");
    let global = load_config_file(&global_path)
        .with_context(|| {
            format!(
                "failed to load global sandbox config at {}",
                global_path.display()
            )
        })?
        .unwrap_or_default();
    let project = load_config_file(&project_path)
        .with_context(|| {
            format!(
                "failed to load project sandbox config at {}",
                project_path.display()
            )
        })?
        .unwrap_or_default();

    Ok(mismatched_profile_names(&global, &project))
}

/// Whether running without a filesystem sandbox would silently grant reads that
/// the active profile denies (`unsandboxed_execution_allowed`).
///
/// Denied reads only exist inside the sandbox. If the resolved profile has any
/// `deny` paths, bypassing the sandbox would drop that restriction, so silent
/// unsandbox / `bypass_sandbox` must keep the command sandboxed.
pub fn unsandboxed_execution_allowed(profile: Option<&str>, workspace: &Path) -> bool {
    match profile_has_denied_reads(profile, workspace) {
        Ok(has_denied_reads) => !has_denied_reads,
        // Fail closed: if we cannot resolve the profile, do not drop the sandbox.
        Err(error) => {
            tracing::warn!(
                error = %error,
                profile = ?profile,
                "could not resolve sandbox profile for unsandbox check; keeping sandbox"
            );
            false
        }
    }
}

fn profile_has_denied_reads(profile: Option<&str>, workspace: &Path) -> anyhow::Result<bool> {
    let Some(profile_name) = profile else {
        return Ok(false);
    };
    if matches!(profile_name.trim(), "" | "off" | "none") {
        return Ok(false);
    }
    let parsed = profile_name
        .parse::<ProfileName>()
        .map_err(|error| anyhow::anyhow!("invalid sandbox profile '{profile_name}': {error}"))?;
    if parsed == ProfileName::Off {
        return Ok(false);
    }
    let config = load_sandbox_config(workspace)?;
    let resolved = parsed.resolve_profile(workspace, &config)?;
    Ok(!resolved.deny.is_empty())
}

fn load_sandbox_config_from_paths(
    global_path: &Path,
    project_path: &Path,
) -> anyhow::Result<SandboxConfig> {
    // Read global first so a malformed or unreadable trusted config cannot be
    // silently replaced by a project profile with the same name.
    let mut config = load_config_file(global_path)
        .with_context(|| {
            format!(
                "failed to load global sandbox config at {}",
                global_path.display()
            )
        })?
        .unwrap_or_default();

    // Project config is additive only and is read only after global succeeds.
    if let Some(project) = load_config_file(project_path).with_context(|| {
        format!(
            "failed to load project sandbox config at {}",
            project_path.display()
        )
    })? {
        merge_project_profiles(&mut config, project);
    }

    Ok(config)
}

fn mismatched_profile_names(global: &SandboxConfig, project: &SandboxConfig) -> Vec<String> {
    let mut names: Vec<String> = project
        .profiles
        .iter()
        .filter(|(name, _)| matches!(name.parse(), Ok(ProfileName::Custom(_))))
        .filter_map(|(name, project_profile)| {
            global
                .profiles
                .get(name)
                .filter(|global_profile| *global_profile != project_profile)
                .map(|_| name.to_owned())
        })
        .collect();
    names.sort_unstable();
    names
}

/// Merge project profiles into `config`. Names already defined globally are
/// ignored so a workspace cannot replace a global custom profile's policy.
fn merge_project_profiles(config: &mut SandboxConfig, project: SandboxConfig) {
    for (name, profile) in project.profiles {
        config.profiles.entry(name).or_insert(profile);
    }
}

fn load_config_file(path: &Path) -> anyhow::Result<Option<SandboxConfig>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("could not read sandbox config"),
    };
    toml::from_str(&content)
        .map(Some)
        .context("could not parse sandbox config")
}

impl ProfileName {
    /// Convert this profile into a nono `CapabilitySet` for the given workspace.
    #[cfg(all(feature = "enforce", unix))]
    pub fn to_capability_set(&self, workspace: &Path) -> anyhow::Result<CapabilitySet> {
        let config = load_sandbox_config(workspace)?;
        self.to_capability_set_with_config(workspace, &config)
    }

    /// Convert using an already-loaded config (avoids re-reading disk).
    ///
    /// A custom profile's own `deny` list is kernel-enforced (read + write/rename)
    /// on top of the base profile.
    #[cfg(all(feature = "enforce", unix))]
    pub fn to_capability_set_with_config(
        &self,
        workspace: &Path,
        config: &SandboxConfig,
    ) -> anyhow::Result<CapabilitySet> {
        if *self == Self::Off {
            return Ok(CapabilitySet::new());
        }

        let profile = self.resolve_profile(workspace, config)?;
        Self::capability_set_from_profile(workspace, &profile)
    }

    #[cfg(all(feature = "enforce", unix))]
    pub fn capability_set_from_profile(
        workspace: &Path,
        profile: &SandboxProfile,
    ) -> anyhow::Result<CapabilitySet> {
        Self::capability_set_from_profile_with_overlay(workspace, profile, None)
    }

    #[cfg(all(feature = "enforce", unix))]
    pub fn capability_set_from_profile_with_overlay(
        workspace: &Path,
        profile: &SandboxProfile,
        overlay: Option<&SandboxPermissionOverlay>,
    ) -> anyhow::Result<CapabilitySet> {
        let mut caps = CapabilitySet::new();

        // Default read access
        if profile.default_read {
            caps = caps.allow_path("/", AccessMode::Read)?;
        }

        // Explicit read-only paths — skip non-existent (nothing to read)
        for path in &profile.read_only {
            if !path.exists() {
                continue;
            }
            let Some(path_str) = path.to_str() else {
                tracing::warn!(path = ?path, "Skipping non-UTF8 read_only path");
                continue;
            };
            caps = caps.allow_path(path_str, AccessMode::Read)?;
        }

        // Read-write paths. nono/Landlock need the directory to exist at
        // apply time (it opens an O_PATH fd), but new files within it can
        // be created freely after the sandbox is applied. Pre-create
        // directories like ~/.devo/ that may not exist on first run.
        for path in &profile.read_write {
            if !path.exists() && std::fs::create_dir_all(path).is_err() {
                tracing::warn!(path = ?path, "read_write path does not exist and could not be created, skipping");
                continue;
            }
            let Some(path_str) = path.to_str() else {
                tracing::warn!(path = ?path, "Skipping non-UTF8 read_write path");
                continue;
            };
            caps = caps.allow_path(path_str, AccessMode::ReadWrite)?;
        }

        // Device special files (character devices like /dev/null, /dev/tty, etc.).
        for dev in DEVICE_FILES {
            let p = Path::new(dev);
            if !p.exists() {
                continue;
            }
            if p.is_dir() {
                // Some systems (e.g. Fedora) expose `/dev/fd` as a directory
                // symlink to `/proc/self/fd`. Treat it like other device
                // directories so bwrap gets a well-formed sandbox command.
                match caps.clone().allow_path(dev, AccessMode::ReadWrite) {
                    Ok(updated_caps) => caps = updated_caps,
                    Err(e) => {
                        tracing::warn!(path = dev, error = %e, "Could not allow device directory");
                    }
                }
            } else {
                if let Err(e) = caps.allow_file_mut(p, AccessMode::ReadWrite) {
                    tracing::warn!(path = dev, error = %e, "Could not allow device file");
                }
            }
        }
        // Device directories (e.g. /dev/pts for PTY slaves on Linux).
        for dev in DEVICE_DIRS {
            let p = Path::new(dev);
            if p.exists() && p.is_dir() {
                caps = caps.allow_path(dev, AccessMode::ReadWrite)?;
            }
        }

        if let Some(overlay) = overlay {
            for path in &overlay.read_paths {
                if !path.exists() {
                    continue;
                }
                caps = caps.allow_path(path, AccessMode::Read)?;
            }
            for path in &overlay.write_paths {
                if !path.exists() && std::fs::create_dir_all(path).is_err() {
                    anyhow::bail!(
                        "additional sandbox write path does not exist and could not be created: {}",
                        path.display()
                    );
                }
                caps = caps.allow_path(path, AccessMode::ReadWrite)?;
            }
        }

        // Kernel deny (read+write): macOS Seatbelt rules; Linux via bwrap bind-over.
        // The effective deny set is the profile's own `deny` (custom profiles only;
        // built-ins carry an empty `deny`). An empty set means there is nothing to
        // enforce. Keying on emptiness rather than profile type avoids enforcing
        // unintentional denies.
        //
        // Split exact paths from globs: exact paths keep the literal/subpath flow;
        // globs become anchored Seatbelt regexes on macOS (a no-op here on Linux,
        // where they are expanded and bound over at bwrap re-exec).
        let (exact_deny, glob_deny) = partition_deny_entries(&profile.deny);
        let all_denied = effective_deny_paths(workspace, &exact_deny);
        if !all_denied.is_empty() {
            apply_deny_paths_to_capability_set(&mut caps, &all_denied)?;
        }
        if !glob_deny.is_empty() {
            apply_deny_globs_to_capability_set(&mut caps, workspace, &glob_deny)?;
        }

        // Network restriction, enforced by nono: Landlock `AccessNet` with a
        // seccomp fallback on Linux, Seatbelt `(deny network*)` on macOS.
        // bwrap-wrapped launches add `--unshare-net` on top (see `wrap`).
        if profile.restrict_network {
            caps = caps.block_network();
            let managed_network = crate::managed_network::managed_network_context_from_env();
            caps = crate::managed_network::apply_managed_network_context(caps, &managed_network);
        }
        if overlay.is_some_and(|overlay| {
            matches!(overlay.network, crate::SandboxNetworkPermission::Enabled)
        }) {
            caps = caps.set_network_mode(nono::NetworkMode::AllowAll);
        }

        Ok(caps)
    }

    /// Resolve this profile into a fully-specified `SandboxProfile` for logging.
    pub fn resolve_profile(
        &self,
        workspace: &Path,
        config: &SandboxConfig,
    ) -> anyhow::Result<SandboxProfile> {
        self.resolve(workspace, config)
    }

    fn resolve(&self, workspace: &Path, config: &SandboxConfig) -> anyhow::Result<SandboxProfile> {
        match self {
            // Selected `off` is handled before resolve (empty CapabilitySet /
            // early return in apply). Reaching here is almost always a custom
            // profile with `extends = "off"` / `"none"` — return Err, never panic.
            Self::Off => anyhow::bail!(
                "sandbox profile 'off' cannot be resolved as a base profile; \
                 choose a built-in base (workspace, devbox, read-only, strict)"
            ),

            Self::Workspace => Ok(SandboxProfile {
                name: "workspace".to_string(),
                read_only: vec![],
                read_write: essential_writable_paths(workspace)?,
                deny: vec![],
                default_read: true,
                restrict_network: false,
            }),

            Self::Devbox => {
                // Everything writable except /data. Enumerate top-level
                // dirs and skip the exclusion list. Can't grant "/" because
                // Landlock has no deny_path — sub-path exceptions are
                // only possible by not granting the parent.
                //
                // /data is excluded from read_write here (so it is not writable)
                // but is deliberately NOT a kernel-deny: it stays readable via
                // default_read, and its Linux write-deny comes from the
                // bwrap_reexec_command(&["/data"]) re-exec, not from profile.deny.
                // Keeping deny empty stops a custom profile that extends devbox
                // from inheriting /data into the enforced kernel-deny set.
                let exclude = [PathBuf::from("/data")];
                let mut read_write = vec![workspace.to_path_buf()];
                if let Ok(entries) = std::fs::read_dir("/") {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if exclude.contains(&path) {
                            continue;
                        }
                        // Skip virtual filesystems (handled separately)
                        if matches!(path.to_str(), Some("/proc" | "/sys" | "/dev")) {
                            continue;
                        }
                        if path.is_dir() {
                            read_write.push(path);
                        }
                    }
                }
                Ok(SandboxProfile {
                    name: "devbox".to_string(),
                    read_only: vec![],
                    read_write,
                    deny: vec![],
                    default_read: true,
                    restrict_network: false,
                })
            }

            Self::ReadOnly => Ok(SandboxProfile {
                name: "read-only".to_string(),
                read_only: vec![],
                read_write: essential_writable_paths_minimal()?,
                deny: vec![],
                default_read: true,
                restrict_network: true,
            }),

            Self::Strict => {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"));
                let system_read: Vec<PathBuf> = [
                    "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/dev", "/proc", "/sys",
                    "/tmp",
                    // Landlock realpath: /etc/resolv.conf often → /run/systemd/resolve/…
                    "/run",
                    // NSS/SSSD (and similar) under /var — needed beyond resolv.conf alone
                    "/var",
                    // macOS-specific paths (filtered by exists() below)
                    "/System",  // Security framework, dylibs, TLS certificates
                    "/Library", // System-wide frameworks
                    "/private", // Real path behind /etc, /tmp, /var symlinks
                ]
                .iter()
                .map(PathBuf::from)
                .filter(|p| p.exists())
                // ~/Library is needed for macOS keychain access (TLS cert validation)
                .chain(std::iter::once(home.join("Library")))
                .filter(|p| p.exists())
                .chain(std::iter::once(workspace.to_path_buf()))
                .collect();

                Ok(SandboxProfile {
                    name: "strict".to_string(),
                    read_only: system_read,
                    read_write: essential_writable_paths(workspace)?,
                    deny: vec![],
                    default_read: false,
                    restrict_network: true,
                })
            }

            Self::Custom(name) => {
                let profile_config = config.profiles.get(name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Custom sandbox profile '{name}' not found. \
                         Define it in ~/.devo/sandbox.toml or .devo/sandbox.toml:\n\n\
                         [profiles.{name}]\n\
                         extends = \"workspace\"\n\
                         read_only = [\"/data\"]\n"
                    )
                })?;

                // Start from the base profile if `extends` is set
                let mut profile = if let Some(base_name) = &profile_config.extends {
                    let base: ProfileName = base_name.parse().map_err(|e: String| {
                        anyhow::anyhow!("Profile '{name}' extends invalid base: {e}")
                    })?;
                    if matches!(base, Self::Off) {
                        anyhow::bail!(
                            "Profile '{name}' extends '{base_name}', but 'off'/'none' \
                             is not a valid base profile"
                        );
                    }
                    if matches!(base, Self::Custom(_)) {
                        anyhow::bail!(
                            "Profile '{name}' extends '{base_name}', but custom profiles \
                             cannot extend other custom profiles (only built-ins)"
                        );
                    }
                    base.resolve(workspace, config)?
                } else {
                    // Default: start from workspace
                    Self::Workspace.resolve(workspace, config)?
                };

                profile.name = name.clone();

                // Apply overrides from the custom config
                if let Some(restrict_net) = profile_config.restrict_network {
                    profile.restrict_network = restrict_net;
                }

                // Add custom read-only paths
                for path_str in &profile_config.read_only {
                    profile.read_only.push(PathBuf::from(path_str));
                }

                // Add custom read-write paths
                for path_str in &profile_config.read_write {
                    profile.read_write.push(PathBuf::from(path_str));
                }

                // Add custom deny paths
                for path_str in &profile_config.deny {
                    profile.deny.push(PathBuf::from(path_str));
                }

                Ok(profile)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEST_CONFIG_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestConfigDir {
        path: PathBuf,
    }

    impl TestConfigDir {
        fn new() -> Self {
            let unique = NEXT_TEST_CONFIG_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "devo-sandbox-profiles-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temporary config directory");
            Self { path }
        }

        fn global_path(&self) -> PathBuf {
            self.path.join("global-sandbox.toml")
        }

        fn project_path(&self) -> PathBuf {
            self.path.join("project-sandbox.toml")
        }
    }

    impl Drop for TestConfigDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_config(path: &Path, contents: &str) {
        std::fs::write(path, contents).expect("write sandbox config");
    }

    #[test]
    fn parse_profile_names() {
        assert_eq!(
            "workspace".parse::<ProfileName>().unwrap(),
            ProfileName::Workspace
        );
        assert_eq!(
            "devbox".parse::<ProfileName>().unwrap(),
            ProfileName::Devbox
        );
        assert_eq!(
            "read-only".parse::<ProfileName>().unwrap(),
            ProfileName::ReadOnly
        );
        assert_eq!(
            "readonly".parse::<ProfileName>().unwrap(),
            ProfileName::ReadOnly
        );
        assert_eq!(
            "strict".parse::<ProfileName>().unwrap(),
            ProfileName::Strict
        );
        assert_eq!("off".parse::<ProfileName>().unwrap(), ProfileName::Off);
        assert_eq!("none".parse::<ProfileName>().unwrap(), ProfileName::Off);
        // Unknown names become Custom profiles
        assert_eq!(
            "my-custom-profile".parse::<ProfileName>().unwrap(),
            ProfileName::Custom("my-custom-profile".to_string())
        );
    }

    #[test]
    fn display_roundtrip() {
        for profile in [
            ProfileName::Workspace,
            ProfileName::Devbox,
            ProfileName::ReadOnly,
            ProfileName::Strict,
            ProfileName::Off,
        ] {
            let s = profile.to_string();
            let parsed: ProfileName = s.parse().unwrap();
            assert_eq!(parsed, profile);
        }
    }

    #[test]
    fn display_custom() {
        let p = ProfileName::Custom("my-custom".to_string());
        assert_eq!(p.to_string(), "my-custom");
    }

    #[test]
    fn missing_config_files_are_absent() {
        let test_dir = TestConfigDir::new();

        let config =
            load_sandbox_config_from_paths(&test_dir.global_path(), &test_dir.project_path())
                .expect("missing configuration files are optional");

        assert_eq!(config, SandboxConfig::default());
    }

    #[test]
    fn malformed_global_config_fails_before_project_merge() {
        let test_dir = TestConfigDir::new();
        write_config(&test_dir.global_path(), "[profiles.secure");
        write_config(
            &test_dir.project_path(),
            "[profiles.project-only]\nextends = \"workspace\"\n",
        );

        let error =
            load_sandbox_config_from_paths(&test_dir.global_path(), &test_dir.project_path())
                .expect_err("malformed global configuration must fail closed");

        assert!(
            error.to_string().contains("global sandbox config"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn malformed_project_config_fails_explicitly() {
        let test_dir = TestConfigDir::new();
        write_config(
            &test_dir.global_path(),
            "[profiles.secure]\nextends = \"workspace\"\n",
        );
        write_config(&test_dir.project_path(), "[profiles.project");

        let error =
            load_sandbox_config_from_paths(&test_dir.global_path(), &test_dir.project_path())
                .expect_err("malformed project configuration must fail closed");

        assert!(
            error.to_string().contains("project sandbox config"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn malformed_global_config_cannot_be_replaced_by_project_trusted_name() {
        let test_dir = TestConfigDir::new();
        write_config(&test_dir.global_path(), "[profiles.secure");
        write_config(
            &test_dir.project_path(),
            "[profiles.secure]\nextends = \"workspace\"\nread_write = [\"/\"]\n",
        );

        let error =
            load_sandbox_config_from_paths(&test_dir.global_path(), &test_dir.project_path())
                .expect_err(
                    "project must not replace a trusted profile after global loading fails",
                );

        assert!(
            error.to_string().contains("global sandbox config"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn unreadable_global_config_is_not_treated_as_absent() {
        let test_dir = TestConfigDir::new();

        let error = load_sandbox_config_from_paths(&test_dir.path, &test_dir.project_path())
            .expect_err("a directory cannot be read as a sandbox configuration file");

        assert!(
            error.to_string().contains("global sandbox config"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn built_in_network_restriction_values() {
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig::default();

        for (name, expected) in [
            (ProfileName::Workspace, false),
            (ProfileName::Devbox, false),
            (ProfileName::ReadOnly, true),
            (ProfileName::Strict, true),
        ] {
            let resolved = name.resolve_profile(&workspace, &config).unwrap();
            assert_eq!(resolved.restrict_network, expected, "{name}");
        }
    }

    fn network_inheritance_config() -> SandboxConfig {
        SandboxConfig {
            profiles: HashMap::from([
                (
                    "strict-inherited".to_string(),
                    ProfileConfig {
                        extends: Some("strict".to_string()),
                        restrict_network: None,
                        read_only: vec![],
                        read_write: vec![],
                        deny: vec![],
                    },
                ),
                (
                    "read-only-inherited".to_string(),
                    ProfileConfig {
                        extends: Some("read-only".to_string()),
                        restrict_network: None,
                        read_only: vec![],
                        read_write: vec![],
                        deny: vec![],
                    },
                ),
                (
                    "strict-unrestricted".to_string(),
                    ProfileConfig {
                        extends: Some("strict".to_string()),
                        restrict_network: Some(false),
                        read_only: vec![],
                        read_write: vec![],
                        deny: vec![],
                    },
                ),
                (
                    "workspace-restricted".to_string(),
                    ProfileConfig {
                        extends: Some("workspace".to_string()),
                        restrict_network: Some(true),
                        read_only: vec![],
                        read_write: vec![],
                        deny: vec![],
                    },
                ),
            ]),
        }
    }

    #[test]
    fn custom_network_restriction_inherits_and_overrides_base() {
        let workspace = std::env::current_dir().unwrap();
        let config = network_inheritance_config();

        for (name, expected) in [
            ("strict-inherited", true),
            ("read-only-inherited", true),
            ("strict-unrestricted", false),
            ("workspace-restricted", true),
        ] {
            let profile_name = ProfileName::Custom(name.to_string());
            let resolved = profile_name.resolve_profile(&workspace, &config).unwrap();
            assert_eq!(resolved.restrict_network, expected, "{name}");
        }
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn restrict_network_blocks_network_in_capability_set() {
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig::default();
        for (name, expected_blocked) in [
            (ProfileName::ReadOnly, true),
            (ProfileName::Strict, true),
            (ProfileName::Workspace, false),
            (ProfileName::Devbox, false),
        ] {
            let caps = name
                .to_capability_set_with_config(&workspace, &config)
                .expect("capability set builds");
            assert_eq!(
                matches!(caps.network_mode(), nono::NetworkMode::Blocked),
                expected_blocked,
                "{name}"
            );
        }
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: additional permissions expand a profile without disabling its sandbox policy.
    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn sandbox_overlay_adds_paths_and_network_access() {
        let workspace = std::env::current_dir().unwrap();
        let temp_root = std::env::temp_dir();
        let suffix = std::process::id();
        let read_path = temp_root.join(format!("devo-sandbox-overlay-input-{suffix}"));
        let write_path = temp_root.join(format!("devo-sandbox-overlay-test-{suffix}"));
        let _ = std::fs::remove_file(&read_path);
        let _ = std::fs::remove_dir_all(&read_path);
        let _ = std::fs::remove_dir_all(&write_path);
        std::fs::create_dir_all(&read_path).expect("create input");
        std::fs::create_dir_all(&write_path).expect("create output");
        let read_path = std::fs::canonicalize(&read_path).expect("canonicalize input");
        let write_path = std::fs::canonicalize(&write_path).expect("canonicalize output");
        let overlay = SandboxPermissionOverlay {
            network: crate::SandboxNetworkPermission::Enabled,
            read_paths: vec![read_path.clone()],
            write_paths: vec![write_path.clone()],
        };

        let profile = ProfileName::Strict
            .resolve_profile(&workspace, &SandboxConfig::default())
            .expect("resolve strict profile");
        let caps = ProfileName::capability_set_from_profile_with_overlay(
            &workspace,
            &profile,
            Some(&overlay),
        )
        .expect("build overlay capability set");

        assert!(caps.path_covered_with_access(&read_path, AccessMode::Read));
        assert!(caps.path_covered_with_access(&write_path, AccessMode::ReadWrite));
        assert_eq!(*caps.network_mode(), nono::NetworkMode::AllowAll);
        std::fs::remove_dir_all(&write_path).expect("remove output");
        std::fs::remove_dir_all(&read_path).expect("remove input");
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn strict_allowlist_includes_run_and_var_when_present() {
        // Regression: /run (resolv realpath) + /var (NSS/SSSD) when present.
        let workspace = std::env::temp_dir();
        let profile = ProfileName::Strict
            .resolve_profile(&workspace, &SandboxConfig::default())
            .expect("strict resolves");
        assert!(!profile.default_read);
        if PathBuf::from("/run").exists() {
            assert!(
                profile.read_only.iter().any(|p| p == Path::new("/run")),
                "strict read_only must include exact /run for systemd-resolved DNS; got {:?}",
                profile.read_only
            );
        }
        if PathBuf::from("/var").exists() {
            assert!(
                profile.read_only.iter().any(|p| p == Path::new("/var")),
                "strict read_only must include exact /var for NSS/SSSD; got {:?}",
                profile.read_only
            );
        }
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn base_profile_capability_set_builds() {
        // A base profile with no `deny` builds a CapabilitySet without erroring.
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig::default();
        let result = ProfileName::Workspace.to_capability_set_with_config(&workspace, &config);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn custom_profile_from_config() {
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig {
            profiles: HashMap::from([(
                "project".to_string(),
                ProfileConfig {
                    extends: Some("workspace".to_string()),
                    restrict_network: Some(true),
                    read_only: vec!["/data".to_string()],
                    read_write: vec![],
                    deny: vec!["/data/private".to_string()],
                },
            )]),
        };

        let profile = ProfileName::Custom("project".to_string());
        let result = profile.to_capability_set_with_config(&workspace, &config);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn custom_extends_devbox_has_no_data_in_deny() {
        // Regression: devbox excludes /data via a local list, not profile.deny, so
        // a custom profile extending devbox must not inherit /data into the kernel
        // deny set (which would wrongly read-deny /data and force fail-closed).
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig {
            profiles: HashMap::from([(
                "mydev".to_string(),
                ProfileConfig {
                    extends: Some("devbox".to_string()),
                    restrict_network: None,
                    read_only: vec![],
                    read_write: vec![],
                    deny: vec![],
                },
            )]),
        };
        let profile = ProfileName::Custom("mydev".to_string());
        let resolved = profile.resolve_profile(&workspace, &config).unwrap();
        assert!(
            !resolved.deny.contains(&PathBuf::from("/data")),
            "custom profile extending devbox must not inherit /data into deny: {:?}",
            resolved.deny
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn custom_profile_not_found() {
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig::default();

        let profile = ProfileName::Custom("nonexistent".to_string());
        let result = profile.to_capability_set_with_config(&workspace, &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "Unexpected error: {err}");
    }

    #[test]
    fn mismatched_profile_names_reports_only_changed_custom_profiles() {
        let profile = |restrict_network| ProfileConfig {
            extends: Some("workspace".to_string()),
            restrict_network: Some(restrict_network),
            read_only: vec![],
            read_write: vec![],
            deny: vec![],
        };
        let global = SandboxConfig {
            profiles: HashMap::from([
                ("dev".to_string(), profile(false)),
                ("same".to_string(), profile(false)),
            ]),
        };
        let project = SandboxConfig {
            profiles: HashMap::from([
                ("dev".to_string(), profile(true)),
                ("same".to_string(), profile(false)),
                ("project-only".to_string(), profile(true)),
                ("devbox".to_string(), profile(true)),
            ]),
        };

        assert_eq!(mismatched_profile_names(&global, &project), vec!["dev"]);
    }

    #[test]
    fn parse_toml_config() {
        let toml_str = r#"
[profiles.devbox]
extends = "workspace"
restrict_network = true
read_only = ["/data"]
deny = ["/data/private"]

[profiles.ci]
extends = "strict"
read_write = ["/tmp/ci-artifacts"]
"#;
        let config: SandboxConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert!(config.profiles.contains_key("devbox"));
        assert!(config.profiles.contains_key("ci"));
        assert_eq!(config.profiles["devbox"].read_only, vec!["/data"]);
        assert_eq!(config.profiles["devbox"].deny, vec!["/data/private"]);
    }

    #[test]
    fn project_cannot_redefine_global_profile() {
        // Global "secure" with a real deny list must win over a project hollow-out.
        let mut config = SandboxConfig {
            profiles: HashMap::from([(
                "secure".to_string(),
                ProfileConfig {
                    extends: Some("workspace".to_string()),
                    restrict_network: Some(true),
                    read_only: vec![],
                    read_write: vec![],
                    deny: vec!["/home/user/.ssh".to_string()],
                },
            )]),
        };
        let project = SandboxConfig {
            profiles: HashMap::from([
                (
                    "secure".to_string(),
                    ProfileConfig {
                        extends: Some("workspace".to_string()),
                        restrict_network: Some(false),
                        read_only: vec![],
                        read_write: vec!["/".to_string()],
                        deny: vec![],
                    },
                ),
                (
                    "project-only".to_string(),
                    ProfileConfig {
                        extends: Some("workspace".to_string()),
                        restrict_network: None,
                        read_only: vec![],
                        read_write: vec![],
                        deny: vec!["./secrets".to_string()],
                    },
                ),
            ]),
        };

        merge_project_profiles(&mut config, project);

        assert_eq!(
            config.profiles["secure"].deny,
            vec!["/home/user/.ssh".to_string()],
            "global deny must be preserved"
        );
        assert_eq!(config.profiles["secure"].restrict_network, Some(true));
        assert!(
            config.profiles["secure"].read_write.is_empty(),
            "project must not widen global read_write"
        );
        assert!(
            config.profiles.contains_key("project-only"),
            "new project-only profile names are still allowed"
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn extends_off_returns_err_not_panic() {
        let workspace = std::env::current_dir().unwrap();
        let config = SandboxConfig {
            profiles: HashMap::from([(
                "broken".to_string(),
                ProfileConfig {
                    extends: Some("off".to_string()),
                    restrict_network: None,
                    read_only: vec![],
                    read_write: vec![],
                    deny: vec![],
                },
            )]),
        };
        let err = ProfileName::Custom("broken".to_string())
            .resolve_profile(&workspace, &config)
            .expect_err("extends=off must Err");
        let msg = err.to_string();
        assert!(
            msg.contains("off") || msg.contains("none"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn resolve_off_returns_err_not_panic() {
        let workspace = std::env::current_dir().unwrap();
        let err = ProfileName::Off
            .resolve_profile(&workspace, &SandboxConfig::default())
            .expect_err("Off.resolve must Err");
        assert!(err.to_string().contains("off"), "unexpected error: {err}");
    }

    #[test]
    fn unsandboxed_execution_allowed_for_inactive_and_builtin_profiles() {
        let workspace = std::env::temp_dir();
        assert!(unsandboxed_execution_allowed(None, &workspace));
        assert!(unsandboxed_execution_allowed(Some("off"), &workspace));
        assert!(unsandboxed_execution_allowed(Some("workspace"), &workspace));
    }

    #[test]
    fn unsandboxed_execution_blocked_when_custom_profile_has_deny() {
        let dir = TestConfigDir::new();
        let workspace = dir.path.clone();
        let config_dir = workspace.join(".devo");
        std::fs::create_dir_all(&config_dir).expect("mkdir .devo");
        write_config(
            &config_dir.join("sandbox.toml"),
            r#"
[profiles.locked]
extends = "workspace"
deny = ["/etc/passwd"]
"#,
        );
        assert!(!unsandboxed_execution_allowed(Some("locked"), &workspace));
    }
}
