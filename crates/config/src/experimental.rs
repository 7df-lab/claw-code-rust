use serde::Deserialize;
use serde::Serialize;

/// Experimental feature gates.
///
/// The former `code-search` gate has been removed. Semantic code search is now
/// provided by the bundled `code_search` MCP server (`devo-code-search-mcp`),
/// which is disabled by default until the user enables it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExperimentalConfig {}
