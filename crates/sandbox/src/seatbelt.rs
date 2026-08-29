//! macOS Seatbelt (SBPL) profile emitter for the `sandbox-exec` command wrapper.
//!
//! PTY spawns have no `pre_exec` hook, so they cannot receive the profile the
//! pipe path applies via [`crate::apply_profile_to_current_process`]. Instead
//! the command is wrapped in `sandbox-exec -p <sbpl>`, and this module renders
//! that `<sbpl>`. It mirrors nono's (private) `generate_profile` for the exact
//! `CapabilitySet` devo builds — same sections, same order — so the PTY wrapper
//! enforces the same policy as the pre_exec path. In particular the deny rules
//! (produced by [`crate::deny::seatbelt_deny_rules`] and carried as nono
//! platform rules) land between the read-allows and the write-allows; see the
//! ordering contract in `crates/sandbox/Cargo.toml`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use nono::{AccessMode, CapabilitySet, FsCapability, IpcMode, NetworkMode};
use nono::{ProcessInfoMode, SignalMode};
use sha2::{Digest, Sha256};

use crate::deny::escape_seatbelt_path;
use crate::profiles::{ProfileName, SandboxProfile};

/// Render the Seatbelt profile enforcing `profile` rooted at `workspace`,
/// ready to pass to `sandbox-exec -p` verbatim (no temp file).
///
/// Fail-closed: a path that cannot be expressed as a Seatbelt filter is an
/// error (mirroring nono); the caller warns and runs unwrapped rather than
/// enforcing a silently different policy.
pub(crate) fn seatbelt_profile_for(
    workspace: &Path,
    profile: &SandboxProfile,
) -> anyhow::Result<String> {
    let caps = ProfileName::capability_set_from_profile(workspace, profile)?;
    emit_seatbelt_profile(&caps)
}

/// One-shot validation of a freshly built profile: run a no-op under it
/// (`sandbox-exec -p <sbpl> /usr/bin/true`) once per distinct profile text and
/// cache the verdict by profile hash. `false` means the profile was rejected
/// (or the check could not run), so the caller warns and runs unwrapped —
/// never fail closed.
pub(crate) fn sandbox_exec_accepts_profile(sbpl: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<[u8; 32], bool>>> = OnceLock::new();
    let hash: [u8; 32] = Sha256::digest(sbpl.as_bytes()).into();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&accepted) = cache
        .lock()
        .expect("sandbox-exec precheck cache poisoned")
        .get(&hash)
    {
        return accepted;
    }
    let accepted = match std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", sbpl, "/usr/bin/true"])
        .output()
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            tracing::debug!(
                stderr = %String::from_utf8_lossy(&output.stderr),
                "sandbox-exec rejected the generated Seatbelt profile"
            );
            false
        }
        Err(error) => {
            tracing::debug!(error = %error, "sandbox-exec profile precheck could not run");
            false
        }
    };
    cache
        .lock()
        .expect("sandbox-exec precheck cache poisoned")
        .insert(hash, accepted);
    accepted
}

/// Mach services through which Keychain items could be read despite file-level
/// denies; denied unless the profile explicitly grants a keychain DB (mirror of
/// nono's default keychain-deny list).
const KEYCHAIN_MACH_SERVICES: &[&str] = &[
    "com.apple.SecurityServer",
    "com.apple.securityd",
    "com.apple.security.keychaind",
    "com.apple.secd",
    "com.apple.security.agent",
];

/// DNS stays usable under `(deny network*)`: macOS resolves all DNS through
/// the mDNSResponder Unix socket, which Seatbelt classifies as network-outbound
/// (mirrors nono's MDNS_RULES, both firmlink alias forms of the socket path).
const MDNS_RULES: &str = "\
(allow system-socket (socket-domain AF_UNIX) (socket-type SOCK_STREAM))\n\
(allow network-outbound (path \"/private/var/run/mDNSResponder\"))\n\
(allow network-outbound (path \"/var/run/mDNSResponder\"))\n";

/// Render a `CapabilitySet` as an SBPL profile, mirroring nono's
/// `generate_profile` section by section.
fn emit_seatbelt_profile(caps: &CapabilitySet) -> anyhow::Result<String> {
    let mut sbpl = String::new();
    sbpl.push_str("(version 1)\n");
    sbpl.push_str("(deny default)\n");
    if caps.seatbelt_debug_deny() {
        sbpl.push_str("(debug deny)\n");
    }
    sbpl.push_str("(allow process-exec*)\n");
    sbpl.push_str("(allow process-fork)\n");
    match caps.process_info_mode() {
        ProcessInfoMode::Isolated | ProcessInfoMode::AllowSameSandbox => {
            sbpl.push_str("(allow process-info* (target self))\n");
            sbpl.push_str("(allow process-info* (target same-sandbox))\n");
        }
        ProcessInfoMode::AllowAll => sbpl.push_str("(allow process-info*)\n"),
    }
    sbpl.push_str("(allow sysctl-read)\n");

    // Mach IPC: allow service resolution, but deny the Keychain/security
    // daemons unless a keychain DB was explicitly granted — blanket
    // mach-lookup would otherwise bypass file-level credential denies.
    sbpl.push_str("(allow mach-lookup)\n");
    if !grants_keychain_db_access(caps) {
        for service in KEYCHAIN_MACH_SERVICES {
            sbpl.push_str(&format!("(deny mach-lookup (global-name \"{service}\"))\n"));
        }
    }
    sbpl.push_str("(allow mach-per-user-lookup)\n");
    sbpl.push_str("(allow mach-task-name)\n");
    sbpl.push_str("(deny mach-priv*)\n");

    sbpl.push_str("(allow ipc-posix-shm-read-data)\n");
    sbpl.push_str("(allow ipc-posix-shm-write-data)\n");
    sbpl.push_str("(allow ipc-posix-shm-write-create)\n");
    if caps.ipc_mode() == IpcMode::Full {
        sbpl.push_str("(allow ipc-posix-sem*)\n");
    }
    match caps.signal_mode() {
        SignalMode::Isolated | SignalMode::AllowSameSandbox => {
            sbpl.push_str("(allow signal (target self))\n");
            sbpl.push_str("(allow signal (target same-sandbox))\n");
        }
        SignalMode::AllowAll => sbpl.push_str("(allow signal)\n"),
    }
    sbpl.push_str("(allow system-fsctl)\n");
    sbpl.push_str("(allow system-info)\n");

    // The root directory entry itself (required for exec path resolution).
    sbpl.push_str("(allow file-read* (literal \"/\"))\n");
    // Metadata (not data) on parent directories of granted paths, so programs
    // can lstat() each path component during resolution.
    for parent in parent_dirs(caps) {
        let escaped = escape_filter_path(Path::new(&parent), "parent directory")?;
        sbpl.push_str(&format!(
            "(allow file-read-metadata (literal \"{escaped}\"))\n"
        ));
    }

    // Executables may only be mapped from readable paths.
    for cap in caps.fs_capabilities() {
        if matches!(cap.access, AccessMode::Read | AccessMode::ReadWrite) {
            for filter in path_filters(cap)? {
                sbpl.push_str(&format!("(allow file-map-executable ({filter}))\n"));
            }
        }
    }

    // ioctl on TTY/PTY devices and on explicitly granted paths.
    sbpl.push_str("(allow file-ioctl (literal \"/dev/tty\"))\n");
    sbpl.push_str("(allow file-ioctl (regex #\"^/dev/ttys[0-9]+$\"))\n");
    sbpl.push_str("(allow file-ioctl (regex #\"^/dev/pty[a-z][0-9a-f]+$\"))\n");
    for cap in caps.fs_capabilities() {
        for filter in path_filters(cap)? {
            sbpl.push_str(&format!("(allow file-ioctl ({filter}))\n"));
        }
    }

    // Pseudo-terminal operations (PTY children need these).
    sbpl.push_str("(allow pseudo-tty)\n");

    for cap in caps.fs_capabilities() {
        if matches!(cap.access, AccessMode::Read | AccessMode::ReadWrite) {
            for filter in path_filters(cap)? {
                sbpl.push_str(&format!("(allow file-read* ({filter}))\n"));
            }
        }
    }

    if caps.extensions_enabled() {
        sbpl.push_str("(allow file-read* (extension \"com.apple.app-sandbox.read\"))\n");
        sbpl.push_str("(allow file-read* (extension \"com.apple.app-sandbox.read-write\"))\n");
        sbpl.push_str("(allow file-write* (extension \"com.apple.app-sandbox.read-write\"))\n");
    }

    // Platform deny rules BETWEEN read and write allows: for equal specificity
    // Seatbelt is last-match-wins, so the denies override the read allows and
    // the per-action write denies below still beat the workspace write grant
    // (see `SEATBELT_WRITE_DENY_ACTIONS` in deny/mod.rs).
    for rule in caps.platform_rules() {
        sbpl.push_str(rule);
        sbpl.push('\n');
    }

    for cap in caps.fs_capabilities() {
        if matches!(cap.access, AccessMode::Write | AccessMode::ReadWrite) {
            for filter in path_filters(cap)? {
                sbpl.push_str(&format!("(allow file-write* ({filter}))\n"));
            }
        }
    }

    emit_network_rules(&mut sbpl, caps)?;
    Ok(sbpl)
}

/// Seatbelt path filters for one capability: `(subpath ...)` for directories,
/// `(literal ...)` for files, one per alias form (resolved first, then the
/// original path when canonicalization changed it, e.g. `/tmp` vs
/// `/private/tmp`). Mirror of nono's `path_filters_for_cap`.
fn path_filters(cap: &FsCapability) -> anyhow::Result<Vec<String>> {
    let kind = if cap.is_file { "literal" } else { "subpath" };
    let mut filters = Vec::with_capacity(2);
    let resolved = escape_filter_path(&cap.resolved, "granted path")?;
    filters.push(format!("{kind} \"{resolved}\""));
    if cap.original != cap.resolved {
        // A non-UTF-8 original is skipped (nono behavior); a control character
        // is fail-closed.
        if let Some(original) = cap.original.to_str() {
            let escaped = escape_filter_path(Path::new(original), "granted path")?;
            filters.push(format!("{kind} \"{escaped}\""));
        }
    }
    Ok(filters)
}

/// Escape `path` for a quoted SBPL string, failing closed on characters that
/// would make the rule target a different path than intended.
fn escape_filter_path(path: &Path, what: &str) -> anyhow::Result<String> {
    escape_seatbelt_path(path)
        .ok_or_else(|| anyhow::anyhow!("cannot express {what} {path:?} as a Seatbelt filter"))
}

/// Parent directories needing metadata access for path resolution (mirror of
/// nono's `collect_parent_dirs`). Sorted for deterministic output; nono emits
/// its set in random order, which is safe to deviate from because these are
/// all same-action allows whose relative order carries no semantics.
fn parent_dirs(caps: &CapabilitySet) -> Vec<String> {
    let mut parents = HashSet::new();
    for cap in caps.fs_capabilities() {
        let paths: Vec<&Path> = if cap.original != cap.resolved {
            vec![cap.resolved.as_path(), cap.original.as_path()]
        } else {
            vec![cap.resolved.as_path()]
        };
        for path in paths {
            let mut current = path.parent();
            while let Some(parent) = current {
                let parent_str = parent.to_string_lossy().into_owned();
                if parent_str == "/" || parent_str.is_empty() {
                    break;
                }
                // Already present: its ancestors were processed too — stop early.
                if !parents.insert(parent_str) {
                    break;
                }
                current = parent.parent();
            }
        }
    }
    let mut parents: Vec<String> = parents.into_iter().collect();
    parents.sort_unstable();
    parents
}

/// Whether the capability set explicitly grants a macOS keychain DB (the
/// narrow opt-in that suppresses the default Keychain Mach denies). Mirror of
/// nono's `has_explicit_keychain_db_access`.
fn grants_keychain_db_access(caps: &CapabilitySet) -> bool {
    let user_keychain_dbs = std::env::var("HOME").ok().map(|home| {
        [
            Path::new(&home).join("Library/Keychains/login.keychain-db"),
            Path::new(&home).join("Library/Keychains/metadata.keychain-db"),
        ]
    });
    let system_keychain_dbs = [
        PathBuf::from("/Library/Keychains/login.keychain-db"),
        PathBuf::from("/Library/Keychains/metadata.keychain-db"),
    ];
    let is_keychain_db = |path: &Path| {
        system_keychain_dbs
            .iter()
            .any(|candidate| path == candidate)
            || user_keychain_dbs
                .as_ref()
                .is_some_and(|dbs| dbs.iter().any(|candidate| path == candidate))
    };
    caps.fs_capabilities()
        .iter()
        .any(|cap| is_keychain_db(&cap.original) || is_keychain_db(&cap.resolved))
}

/// Network section (mirror of nono's per-`NetworkMode` emission). Devo's
/// profile construction only produces `Blocked`/`AllowAll` without port or
/// unix-socket grants; the remaining capability surface is rejected explicitly
/// rather than silently under-enforced.
fn emit_network_rules(sbpl: &mut String, caps: &CapabilitySet) -> anyhow::Result<()> {
    if !caps.unix_socket_capabilities().is_empty() {
        anyhow::bail!("unix-socket capabilities are not supported by the Seatbelt emitter");
    }
    if !caps.tcp_connect_ports().is_empty() || !caps.tcp_bind_ports().is_empty() {
        anyhow::bail!("Seatbelt cannot filter by TCP port");
    }
    let localhost_ports = caps.localhost_ports();
    match caps.network_mode() {
        NetworkMode::Blocked => {
            sbpl.push_str("(deny network*)\n");
            sbpl.push_str(MDNS_RULES);
            if !localhost_ports.is_empty() {
                sbpl.push_str(
                    "(allow system-socket (socket-domain AF_INET) (socket-type SOCK_STREAM))\n",
                );
                sbpl.push_str(
                    "(allow system-socket (socket-domain AF_INET6) (socket-type SOCK_STREAM))\n",
                );
                for port in localhost_ports {
                    sbpl.push_str(&format!(
                        "(allow network-outbound (remote tcp \"localhost:{port}\"))\n"
                    ));
                }
                // Seatbelt cannot filter bind/inbound by port.
                sbpl.push_str("(allow network-bind)\n");
                sbpl.push_str("(allow network-inbound)\n");
            }
        }
        NetworkMode::ProxyOnly { port, bind_ports } => {
            sbpl.push_str("(deny network*)\n");
            sbpl.push_str(MDNS_RULES);
            sbpl.push_str(&format!(
                "(allow network-outbound (remote tcp \"localhost:{port}\"))\n"
            ));
            for localhost_port in localhost_ports {
                sbpl.push_str(&format!(
                    "(allow network-outbound (remote tcp \"localhost:{localhost_port}\"))\n"
                ));
            }
            sbpl.push_str(
                "(allow system-socket (socket-domain AF_INET) (socket-type SOCK_STREAM))\n",
            );
            sbpl.push_str(
                "(allow system-socket (socket-domain AF_INET6) (socket-type SOCK_STREAM))\n",
            );
            if !bind_ports.is_empty() || !localhost_ports.is_empty() {
                sbpl.push_str("(allow network-bind)\n");
                sbpl.push_str("(allow network-inbound)\n");
            }
        }
        NetworkMode::AllowAll => {
            sbpl.push_str("(allow system-socket)\n");
            sbpl.push_str("(allow network-outbound)\n");
            sbpl.push_str("(allow network-inbound)\n");
            sbpl.push_str("(allow network-bind)\n");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A canonicalized workspace directly under /tmp (firmlinked to
    /// /private/tmp) so the firmlink-alias behavior is deterministic.
    fn temp_workspace(tag: &str) -> (PathBuf, TempDirGuard) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let workspace = PathBuf::from(format!(
            "/tmp/devo-seatbelt-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&workspace).expect("create temp workspace");
        let workspace = dunce::canonicalize(&workspace).expect("canonicalize temp workspace");
        let guard = TempDirGuard(workspace.clone());
        (workspace, guard)
    }

    fn synthetic_profile(
        read_only: &[&str],
        read_write: &[&str],
        deny: &[&str],
        default_read: bool,
        restrict_network: bool,
    ) -> SandboxProfile {
        SandboxProfile {
            name: "golden".to_string(),
            read_only: read_only.iter().map(PathBuf::from).collect(),
            read_write: read_write.iter().map(PathBuf::from).collect(),
            deny: deny.iter().map(PathBuf::from).collect(),
            default_read,
            restrict_network,
        }
    }

    /// The 10 deny rules emitted per filter, hardcoded (not derived from
    /// production constants) so a change in the deny set breaks this golden.
    fn expected_deny_block(rules: &mut String, filter: &str) {
        rules.push_str(&format!("(deny file-read* {filter})\n"));
        rules.push_str(&format!("(deny file-write* {filter})\n"));
        for action in [
            "file-write-data",
            "file-write-create",
            "file-write-unlink",
            "file-write-mode",
            "file-write-owner",
            "file-write-flags",
            "file-write-times",
            "file-write-setugid",
        ] {
            rules.push_str(&format!("(deny {action} {filter})\n"));
        }
    }

    #[test]
    fn golden_profile_for_synthetic_profile() {
        let (workspace, _guard) = temp_workspace("golden");
        let ws = workspace.display().to_string();
        let secret = workspace.join("secret.txt");
        std::fs::write(&secret, "SECRET").expect("write denied file");
        let ext = PathBuf::from(format!(
            "/tmp/devo-seatbelt-golden-ext-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos()
        ));
        std::fs::write(&ext, "EXT").expect("write external denied file");
        let ext_alias = PathBuf::from(format!("/private{}", ext.display()));
        let _ext_guard = TempDirGuard(ext.clone());
        let ws_alias = ws.replace("/private/tmp/", "/tmp/");

        let profile = synthetic_profile(
            &["/System"],
            &[&ws],
            &["secret.txt", &ext.display().to_string()],
            /*default_read*/ true,
            /*restrict_network*/ true,
        );
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");

        let mut expected = String::new();
        expected.push_str(
            r##"(version 1)
(deny default)
(allow process-exec*)
(allow process-fork)
(allow process-info* (target self))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow mach-lookup)
(deny mach-lookup (global-name "com.apple.SecurityServer"))
(deny mach-lookup (global-name "com.apple.securityd"))
(deny mach-lookup (global-name "com.apple.security.keychaind"))
(deny mach-lookup (global-name "com.apple.secd"))
(deny mach-lookup (global-name "com.apple.security.agent"))
(allow mach-per-user-lookup)
(allow mach-task-name)
(deny mach-priv*)
(allow ipc-posix-shm-read-data)
(allow ipc-posix-shm-write-data)
(allow ipc-posix-shm-write-create)
(allow signal (target self))
(allow signal (target same-sandbox))
(allow system-fsctl)
(allow system-info)
(allow file-read* (literal "/"))
(allow file-read-metadata (literal "/dev"))
(allow file-read-metadata (literal "/private"))
(allow file-read-metadata (literal "/private/tmp"))
"##,
        );
        for filter in [
            "subpath \"/\"".to_string(),
            "subpath \"/System\"".to_string(),
            format!("subpath \"{ws}\""),
            "literal \"/dev/null\"".to_string(),
            "literal \"/dev/zero\"".to_string(),
            "literal \"/dev/random\"".to_string(),
            "literal \"/dev/urandom\"".to_string(),
            "literal \"/dev/tty\"".to_string(),
            "literal \"/dev/ptmx\"".to_string(),
            // `/dev/fd` is a directory (symlink to `/proc/self/fd` on Linux;
            // directory-like on macOS), so Seatbelt emits `subpath`.
            "subpath \"/dev/fd\"".to_string(),
        ] {
            expected.push_str(&format!("(allow file-map-executable ({filter}))\n"));
        }
        expected.push_str(
            r##"(allow file-ioctl (literal "/dev/tty"))
(allow file-ioctl (regex #"^/dev/ttys[0-9]+$"))
(allow file-ioctl (regex #"^/dev/pty[a-z][0-9a-f]+$"))
"##,
        );
        for filter in [
            "subpath \"/\"".to_string(),
            "subpath \"/System\"".to_string(),
            format!("subpath \"{ws}\""),
            "literal \"/dev/null\"".to_string(),
            "literal \"/dev/zero\"".to_string(),
            "literal \"/dev/random\"".to_string(),
            "literal \"/dev/urandom\"".to_string(),
            "literal \"/dev/tty\"".to_string(),
            "literal \"/dev/ptmx\"".to_string(),
            "subpath \"/dev/fd\"".to_string(),
        ] {
            expected.push_str(&format!("(allow file-ioctl ({filter}))\n"));
        }
        expected.push_str("(allow pseudo-tty)\n");
        for filter in [
            "subpath \"/\"".to_string(),
            "subpath \"/System\"".to_string(),
            format!("subpath \"{ws}\""),
            "literal \"/dev/null\"".to_string(),
            "literal \"/dev/zero\"".to_string(),
            "literal \"/dev/random\"".to_string(),
            "literal \"/dev/urandom\"".to_string(),
            "literal \"/dev/tty\"".to_string(),
            "literal \"/dev/ptmx\"".to_string(),
            "subpath \"/dev/fd\"".to_string(),
        ] {
            expected.push_str(&format!("(allow file-read* ({filter}))\n"));
        }
        // Deny platform rules: sorted paths, each with its firmlink alias.
        for filter in [
            format!("(literal \"{ws}/secret.txt\")"),
            format!("(literal \"{ws_alias}/secret.txt\")"),
            format!("(literal \"{}\")", ext.display()),
            format!("(literal \"{}\")", ext_alias.display()),
        ] {
            expected_deny_block(&mut expected, &filter);
        }
        for filter in [
            format!("subpath \"{ws}\""),
            "literal \"/dev/null\"".to_string(),
            "literal \"/dev/zero\"".to_string(),
            "literal \"/dev/random\"".to_string(),
            "literal \"/dev/urandom\"".to_string(),
            "literal \"/dev/tty\"".to_string(),
            "literal \"/dev/ptmx\"".to_string(),
            "subpath \"/dev/fd\"".to_string(),
        ] {
            expected.push_str(&format!("(allow file-write* ({filter}))\n"));
        }
        expected.push_str(
            r##"(deny network*)
(allow system-socket (socket-domain AF_UNIX) (socket-type SOCK_STREAM))
(allow network-outbound (path "/private/var/run/mDNSResponder"))
(allow network-outbound (path "/var/run/mDNSResponder"))
"##,
        );

        assert_eq!(sbpl, expected);
    }

    #[test]
    fn deny_rules_sit_between_read_and_write_allows() {
        let (workspace, _guard) = temp_workspace("order");
        std::fs::write(workspace.join("secret.txt"), "SECRET").expect("write denied file");
        let profile = synthetic_profile(
            &[],
            &[&workspace.display().to_string()],
            &["secret.txt"],
            true,
            false,
        );
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");

        let last_read_allow = sbpl.rfind("(allow file-read*").expect("read allows exist");
        let first_deny = sbpl.find("(deny file-read*").expect("deny rules exist");
        let first_write_allow = sbpl
            .find("(allow file-write* (subpath")
            .expect("write allows exist");
        assert!(
            last_read_allow < first_deny && first_deny < first_write_allow,
            "deny rules must land between read allows and write allows:\n{sbpl}"
        );
    }

    #[test]
    fn deny_glob_becomes_anchored_regex_with_firmlink_alias() {
        let (workspace, _guard) = temp_workspace("glob");
        let ws = workspace.display().to_string();
        let ws_alias = ws.replace("/private/tmp/", "/tmp/");
        let profile = synthetic_profile(&[], &[&ws], &["**/*.pem"], true, false);
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");

        for root in [&ws, &ws_alias] {
            let filter = format!("(regex #\"^{root}/(.*/)?[^/]*\\.pem$\")");
            assert!(
                sbpl.contains(&format!("(deny file-read* {filter})")),
                "missing read-deny regex for root {root}:\n{sbpl}"
            );
            assert!(
                sbpl.contains(&format!("(deny file-write* {filter})")),
                "missing write-deny regex for root {root}:\n{sbpl}"
            );
        }
    }

    #[test]
    fn network_restriction_toggles_network_section() {
        let (workspace, _guard) = temp_workspace("net");
        let ws = workspace.display().to_string();

        let restricted = synthetic_profile(&[], &[&ws], &[], true, true);
        let sbpl = seatbelt_profile_for(&workspace, &restricted).expect("profile renders");
        assert!(sbpl.contains("(deny network*)\n"), "{sbpl}");
        assert!(
            sbpl.contains(
                "(allow system-socket (socket-domain AF_UNIX) (socket-type SOCK_STREAM))\n"
            ),
            "mDNS socket rule missing:\n{sbpl}"
        );
        assert!(
            sbpl.contains("(allow network-outbound (path \"/private/var/run/mDNSResponder\"))\n")
        );
        assert!(sbpl.contains("(allow network-outbound (path \"/var/run/mDNSResponder\"))\n"));
        assert!(!sbpl.contains("(allow network-bind)"), "{sbpl}");

        let open = synthetic_profile(&[], &[&ws], &[], true, false);
        let sbpl = seatbelt_profile_for(&workspace, &open).expect("profile renders");
        assert!(!sbpl.contains("(deny network*)"), "{sbpl}");
        assert!(sbpl.contains("(allow system-socket)\n"), "{sbpl}");
        assert!(sbpl.contains("(allow network-outbound)\n"), "{sbpl}");
        assert!(sbpl.contains("(allow network-inbound)\n"), "{sbpl}");
        assert!(sbpl.contains("(allow network-bind)\n"), "{sbpl}");
    }

    #[test]
    fn built_in_profiles_render_enforceable_profiles() {
        let (workspace, _guard) = temp_workspace("builtin");
        let config = crate::profiles::SandboxConfig::default();

        for name in [
            crate::profiles::ProfileName::Workspace,
            crate::profiles::ProfileName::ReadOnly,
            crate::profiles::ProfileName::Strict,
            crate::profiles::ProfileName::Devbox,
        ] {
            let resolved = name
                .resolve_profile(&workspace, &config)
                .expect("built-in resolves");
            let sbpl = seatbelt_profile_for(&workspace, &resolved).expect("profile renders");
            for required in [
                "(version 1)\n(deny default)\n",
                "(allow process-exec*)\n",
                "(allow pseudo-tty)\n",
                "(deny mach-lookup (global-name \"com.apple.secd\"))\n",
            ] {
                assert!(sbpl.contains(required), "{name} missing {required:?}");
            }
            let restricts = matches!(
                name,
                crate::profiles::ProfileName::ReadOnly | crate::profiles::ProfileName::Strict
            );
            assert_eq!(sbpl.contains("(deny network*)"), restricts, "{name}");
        }

        let strict = crate::profiles::ProfileName::Strict
            .resolve_profile(&workspace, &config)
            .expect("strict resolves");
        let sbpl = seatbelt_profile_for(&workspace, &strict).expect("profile renders");
        assert!(
            !sbpl.contains("(allow file-read* (subpath \"/\"))"),
            "strict must not grant default read:\n{sbpl}"
        );
        assert!(
            sbpl.contains("(allow file-read* (subpath \"/usr\"))"),
            "{sbpl}"
        );
        assert!(
            sbpl.contains("(allow file-read* (subpath \"/System\"))"),
            "{sbpl}"
        );
    }

    #[test]
    fn denied_device_file_removes_its_capability() {
        let (workspace, _guard) = temp_workspace("devdeny");
        let profile = synthetic_profile(&[], &[], &["/dev/null"], true, false);
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");
        assert!(
            !sbpl.contains("(allow file-read* (literal \"/dev/null\"))"),
            "the /dev/null capability must be removed once denied:\n{sbpl}"
        );
        assert!(
            sbpl.contains("(deny file-read* (literal \"/dev/null\"))"),
            "{sbpl}"
        );
        assert!(
            sbpl.contains("(deny file-write* (literal \"/dev/null\"))"),
            "{sbpl}"
        );
    }

    #[test]
    fn deny_paths_are_escaped_and_control_chars_fail_closed() {
        let (workspace, _guard) = temp_workspace("escape");
        let profile = synthetic_profile(&[], &[], &["/tmp/quo\"te.txt"], true, false);
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");
        assert!(
            sbpl.contains("(deny file-read* (literal \"/tmp/quo\\\"te.txt\"))"),
            "quote must be escaped:\n{sbpl}"
        );

        let profile = synthetic_profile(&[], &[], &["/tmp/a\u{07}b"], true, false);
        let error = seatbelt_profile_for(&workspace, &profile)
            .expect_err("a control character in a deny path must fail closed");
        assert!(
            error.to_string().contains("escape deny path"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn sandbox_exec_accepts_valid_profiles_and_rejects_garbage() {
        if !Path::new("/usr/bin/sandbox-exec").is_file() {
            eprintln!("skipping: sandbox-exec unavailable");
            return;
        }
        assert!(sandbox_exec_accepts_profile(
            "(version 1)\n(allow default)\n"
        ));
        assert!(!sandbox_exec_accepts_profile("(version 1)\n(deny default"));

        let (workspace, _guard) = temp_workspace("precheck");
        std::fs::write(workspace.join("secret.txt"), "SECRET").expect("write denied file");
        let profile = synthetic_profile(
            &["/System"],
            &[&workspace.display().to_string()],
            &["secret.txt"],
            true,
            true,
        );
        let sbpl = seatbelt_profile_for(&workspace, &profile).expect("profile renders");
        assert!(
            sandbox_exec_accepts_profile(&sbpl),
            "the emitted golden-class profile must pass sandbox-exec validation:\n{sbpl}"
        );
    }
}
