use std::path::PathBuf;
use std::sync::Arc;

use devo_safety::legacy_permissions::PermissionPolicy;

/// The execution context provided to every tool call.
///
/// Instead of a monolithic context object, tools receive only the
/// dependencies they actually need. This makes tool implementations
/// easier to test and reason about.
pub struct ToolContext {
    /// Current working directory for the session.
    pub cwd: PathBuf,
    /// The permission policy in effect.
    pub permissions: Arc<dyn PermissionPolicy>,
    /// Session-level metadata tools can use for state.
    pub session_id: String,
}
