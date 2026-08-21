use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::RwLock;

/// A client-facing protocol adapter that the server may expose.
///
/// `Native` is the first-party protocol surface. ACP and future external
/// protocols are adapters around the shared Native domain model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerProtocol {
    Native,
    Acp,
}

impl ServerProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Acp => "acp",
        }
    }
}

impl fmt::Display for ServerProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ServerProtocol {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "native" => Ok(Self::Native),
            "acp" => Ok(Self::Acp),
            other => Err(format!(
                "unknown protocol {other:?}; expected one of: native, acp"
            )),
        }
    }
}

/// Non-empty set of protocol adapters exposed by a server runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolSet {
    protocols: BTreeSet<ServerProtocol>,
}

impl ProtocolSet {
    pub fn new(protocols: impl IntoIterator<Item = ServerProtocol>) -> Result<Self, String> {
        let protocols = protocols.into_iter().collect::<BTreeSet<_>>();
        if protocols.is_empty() {
            return Err("at least one protocol must be enabled".to_string());
        }
        Ok(Self { protocols })
    }

    pub fn only(protocol: ServerProtocol) -> Self {
        Self {
            protocols: BTreeSet::from([protocol]),
        }
    }

    pub fn all() -> Self {
        Self {
            protocols: BTreeSet::from([ServerProtocol::Native, ServerProtocol::Acp]),
        }
    }

    pub fn contains(&self, protocol: ServerProtocol) -> bool {
        self.protocols.contains(&protocol)
    }

    /// Extends this set monotonically and returns whether it changed.
    pub fn enable(&mut self, requested: &Self) -> bool {
        let previous_len = self.protocols.len();
        self.protocols.extend(requested.protocols.iter().copied());
        self.protocols.len() != previous_len
    }

    pub fn iter(&self) -> impl Iterator<Item = ServerProtocol> + '_ {
        self.protocols.iter().copied()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.iter().map(ServerProtocol::as_str).collect()
    }
}

impl Default for ProtocolSet {
    fn default() -> Self {
        Self::only(ServerProtocol::Native)
    }
}

impl fmt::Display for ProtocolSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self.names();
        formatter.write_str(&names.join(","))
    }
}

impl FromStr for ProtocolSet {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let protocols = value
            .split(',')
            .map(str::trim)
            .map(ServerProtocol::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(protocols)
    }
}

/// Concurrency-safe owner of the runtime's protocol exposure policy.
pub(crate) struct ProtocolExposurePolicy {
    enabled: RwLock<ProtocolSet>,
}

impl ProtocolExposurePolicy {
    pub(crate) fn new(enabled: ProtocolSet) -> Self {
        Self {
            enabled: RwLock::new(enabled),
        }
    }

    pub(crate) async fn enabled(&self) -> ProtocolSet {
        self.enabled.read().await.clone()
    }

    pub(crate) async fn allows(&self, protocol: ServerProtocol) -> bool {
        self.enabled.read().await.contains(protocol)
    }

    pub(crate) async fn enable(&self, requested: &ProtocolSet) -> ProtocolSet {
        let mut enabled = self.enabled.write().await;
        enabled.enable(requested);
        enabled.clone()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn protocol_set_defaults_to_native() {
        assert_eq!(ProtocolSet::default().names(), vec!["native"]);
    }

    #[test]
    fn protocol_set_parses_trims_and_deduplicates() {
        let protocols = "native, acp,native".parse::<ProtocolSet>().expect("parse");

        assert_eq!(protocols.names(), vec!["native", "acp"]);
    }

    #[test]
    fn protocol_set_rejects_empty_and_unknown_values() {
        assert_eq!(
            "".parse::<ProtocolSet>().expect_err("empty must fail"),
            "unknown protocol \"\"; expected one of: native, acp"
        );
        assert_eq!(
            "a2a".parse::<ProtocolSet>().expect_err("unknown must fail"),
            "unknown protocol \"a2a\"; expected one of: native, acp"
        );
    }

    #[test]
    fn enabling_protocols_is_monotonic_and_idempotent() {
        let mut enabled = ProtocolSet::only(ServerProtocol::Native);
        let acp = ProtocolSet::only(ServerProtocol::Acp);

        assert!(enabled.enable(&acp));
        assert!(!enabled.enable(&acp));
        assert_eq!(enabled.names(), vec!["native", "acp"]);
    }
}
