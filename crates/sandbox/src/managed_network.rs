//! Managed network sandbox scaffolding.
//!
//! Restricted sandbox profiles block outbound network by default. The
//! [`devo-sandbox-network-proxy`](../../sandbox-network-proxy) crate runs a
//! lean localhost HTTP CONNECT proxy; this module exposes the capability
//! context and env hook so those loopback ports stay reachable inside the
//! sandbox.
//!
//! Prefer [`set_sandbox_proxy_ports`] (process-local, thread-safe) over the
//! legacy env helper. Use [`managed_network_context_from_ports`] when the
//! caller holds explicit port numbers.

use std::sync::Mutex;

#[cfg(unix)]
use nono::CapabilitySet;

/// Extra network allowances applied when a profile restricts outbound network.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedNetworkSandboxContext {
    /// Localhost TCP ports that remain reachable under network restriction.
    pub loopback_ports: Vec<u16>,
    /// Whether the sandboxed process may bind local ports (for proxy listeners).
    pub allow_local_binding: bool,
}

const PROXY_PORTS_ENV: &str = "DEVO_SANDBOX_PROXY_PORTS";

static PUBLISHED_PROXY_PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// Publishes localhost proxy ports for in-process lookups (thread-safe).
///
/// Child-process HTTP(S)_PROXY injection reads these ports via
/// [`managed_network_context_from_env`]; callers no longer need to mutate the
/// process environment from a multi-threaded Tokio worker.
pub fn set_sandbox_proxy_ports(ports: &[u16]) {
    let normalized = parse_proxy_ports_iter(ports.iter().copied());
    *PUBLISHED_PROXY_PORTS.lock().expect("proxy ports mutex") = normalized;
}

/// Legacy alias for [`set_sandbox_proxy_ports`].
///
/// Does not mutate the process environment; ports live in the in-process
/// store and are passed to children via `proxy_env_for_restricted_network`
/// (Unix only).
pub fn set_sandbox_proxy_ports_env(ports: &[u16]) {
    set_sandbox_proxy_ports(ports);
}

/// Whether a managed or process-level HTTP(S) proxy is available for
/// restricted-network sandbox launches (`--allow-network-for-proxy`).
pub fn sandbox_proxy_available() -> bool {
    !managed_network_context_from_env().loopback_ports.is_empty()
        || std::env::var_os("HTTP_PROXY").is_some()
        || std::env::var_os("HTTPS_PROXY").is_some()
        || std::env::var_os("ALL_PROXY").is_some()
}

/// Builds managed-network settings from explicit loopback ports.
pub fn managed_network_context_from_ports(
    ports: impl IntoIterator<Item = u16>,
) -> ManagedNetworkSandboxContext {
    let loopback_ports = parse_proxy_ports_iter(ports);
    ManagedNetworkSandboxContext {
        allow_local_binding: !loopback_ports.is_empty(),
        loopback_ports,
    }
}

/// Reads managed-network settings from the in-process port store, falling back
/// to `DEVO_SANDBOX_PROXY_PORTS` for tests / inherited environments.
pub fn managed_network_context_from_env() -> ManagedNetworkSandboxContext {
    let published = PUBLISHED_PROXY_PORTS
        .lock()
        .expect("proxy ports mutex")
        .clone();
    if !published.is_empty() {
        return managed_network_context_from_ports(published);
    }
    managed_network_context_from_ports(
        std::env::var(PROXY_PORTS_ENV)
            .ok()
            .map(|raw| parse_proxy_ports(&raw))
            .unwrap_or_default(),
    )
}

/// Proxy env vars for a resolved sandbox profile name and workspace.
#[cfg(unix)]
pub fn proxy_env_for_sandbox_profile(
    sandbox_profile: Option<&str>,
    workdir: &std::path::Path,
) -> Vec<(String, String)> {
    let Some(profile_name) = sandbox_profile else {
        return Vec::new();
    };
    let Ok(name) = profile_name.parse::<crate::ProfileName>() else {
        return Vec::new();
    };
    let Ok(config) = crate::load_sandbox_config(workdir) else {
        return Vec::new();
    };
    let Ok(resolved) = name.resolve_profile(workdir, &config) else {
        return Vec::new();
    };
    proxy_env_for_restricted_network(resolved.restrict_network)
}

/// Proxy env vars for sandboxed shell children when network is restricted.
#[cfg(unix)]
pub fn proxy_env_for_restricted_network(restrict_network: bool) -> Vec<(String, String)> {
    if !restrict_network {
        return Vec::new();
    }
    let Some(port) = managed_network_context_from_env()
        .loopback_ports
        .first()
        .copied()
    else {
        return Vec::new();
    };
    let proxy_url = format!("http://127.0.0.1:{port}");
    vec![
        ("HTTP_PROXY".to_string(), proxy_url.clone()),
        ("HTTPS_PROXY".to_string(), proxy_url.clone()),
        ("ALL_PROXY".to_string(), proxy_url),
        (PROXY_PORTS_ENV.to_string(), port.to_string()),
    ]
}

fn parse_proxy_ports(raw: &str) -> Vec<u16> {
    parse_proxy_ports_iter(
        raw.split(',')
            .filter_map(|segment| segment.trim().parse::<u16>().ok()),
    )
}

fn parse_proxy_ports_iter(ports: impl IntoIterator<Item = u16>) -> Vec<u16> {
    let mut ports = ports.into_iter().collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Applies managed-network allowances on top of an already-built capability set.
///
/// When loopback proxy ports are configured, switches to
/// [`nono::NetworkMode::ProxyOnly`] for the first port (restricted + loopback
/// proxy semantics) and keeps any additional ports on the localhost allow-list.
#[cfg(unix)]
pub fn apply_managed_network_context(
    mut caps: CapabilitySet,
    ctx: &ManagedNetworkSandboxContext,
) -> CapabilitySet {
    let Some((first, rest)) = ctx.loopback_ports.split_first() else {
        return caps;
    };
    caps = caps.proxy_only(*first);
    for port in rest {
        caps = caps.allow_localhost_port(*port);
    }
    if ctx.allow_local_binding {
        // Seatbelt ProxyOnly already allows bind/inbound when localhost ports
        // are non-empty; this flag documents intent for Linux/landlock paths.
    }
    caps
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_proxy_ports_dedupes_and_sorts() {
        assert_eq!(parse_proxy_ports("5678,1234,5678"), vec![1234, 5678]);
    }

    #[test]
    #[serial_test::serial(sandbox_proxy_ports)]
    fn set_sandbox_proxy_ports_is_visible_to_context_lookup() {
        set_sandbox_proxy_ports(&[9, 1, 9]);
        assert_eq!(
            managed_network_context_from_env().loopback_ports,
            vec![1, 9]
        );
        assert!(sandbox_proxy_available());
        set_sandbox_proxy_ports(&[]);
        // Do not assert via `sandbox_proxy_available()`: that also inspects
        // HTTP(S)_PROXY / ALL_PROXY, which may be set in the ambient process.
        assert!(
            PUBLISHED_PROXY_PORTS
                .lock()
                .expect("proxy ports mutex")
                .is_empty()
        );
    }

    #[test]
    #[cfg(unix)]
    fn apply_managed_network_context_adds_localhost_ports() {
        let ctx = ManagedNetworkSandboxContext {
            loopback_ports: vec![8080, 3000],
            allow_local_binding: true,
        };
        let caps = apply_managed_network_context(CapabilitySet::new().block_network(), &ctx);
        assert!(matches!(
            caps.network_mode(),
            nono::NetworkMode::ProxyOnly { port: 8080, .. }
        ));
        let mut ports = caps.localhost_ports().to_vec();
        ports.sort_unstable();
        assert_eq!(ports, vec![3000]);
    }
}
