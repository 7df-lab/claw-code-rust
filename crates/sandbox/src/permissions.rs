use std::path::PathBuf;

/// Network access requested by a single sandboxed tool invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SandboxNetworkPermission {
    /// Preserve the active profile's network policy.
    #[default]
    Unchanged,
    /// Allow network access while preserving filesystem sandboxing.
    Enabled,
}

/// Additional sandbox capabilities granted to one tool invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SandboxPermissionOverlay {
    pub network: SandboxNetworkPermission,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
}
