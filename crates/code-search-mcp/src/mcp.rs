//! Stdio MCP adapter around [`CodeSearchService`](crate::CodeSearchService).
//!
//! Devo launches this as an optional bundled MCP server. The process cwd is the
//! workspace root used for path confinement and indexing.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use devo_network_proxy::NetworkProxyConfig;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    CodeSearchError, CodeSearchOperation, CodeSearchService, ContentFilter, DEFAULT_TOP_K,
    RelatedRequest, SearchFilters, SearchOutput, SearchRequest,
};

const TOOL_NAME: &str = "code_search";
const TOOL_DESCRIPTION: &str = "Preferred codebase investigation and code retrieval tool for the current workspace. Use code_search before find or grep when you need to understand how code is implemented, locate relevant modules or symbols, answer architecture questions, find related code, or search by natural-language intent.";

/// Long-lived MCP server that owns one [`CodeSearchService`].
#[derive(Clone)]
pub struct CodeSearchMcpServer {
    service: Arc<CodeSearchService>,
    workspace_root: PathBuf,
    tools: Arc<Vec<Tool>>,
}

impl CodeSearchMcpServer {
    /// Builds a production server that indexes the process current directory.
    pub fn production() -> Result<Self, std::io::Error> {
        let workspace_root = std::env::current_dir()?;
        let proxy = NetworkProxyConfig {
            proxy_url: None,
            no_proxy: None,
        };
        let service = Arc::new(CodeSearchService::production_with_network_proxy(proxy));
        Ok(Self::with_service(service, workspace_root))
    }

    /// Builds a server around an injected service and workspace root (tests).
    pub fn with_service(service: Arc<CodeSearchService>, workspace_root: PathBuf) -> Self {
        Self {
            service,
            workspace_root,
            tools: Arc::new(vec![code_search_tool()]),
        }
    }

    /// Prefers warming the default code index for the workspace root.
    pub fn prewarm(&self) {
        let _ = self
            .service
            .prewarm(&self.workspace_root, ContentFilter::Code);
    }

    /// Runs a tool call against the workspace-scoped service.
    pub fn execute(&self, input: serde_json::Value) -> Result<SearchOutput, String> {
        let input: CodeSearchInput =
            serde_json::from_value(input).map_err(|error| error.to_string())?;
        let request = build_request(&self.workspace_root, input)?;
        match request {
            CodeSearchRequest::Search(request) => self.service.search(request),
            CodeSearchRequest::FindRelated(request) => self.service.find_related(request),
        }
        .map_err(map_code_search_error)
    }
}

fn code_search_tool() -> Tool {
    #[expect(clippy::expect_used)]
    let schema: JsonObject = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "description": "Search operation: search for query text or find chunks related to file_path:line",
                "enum": ["search", "find_related"]
            },
            "query": {
                "type": "string",
                "description": "Natural-language or code query. Required for search."
            },
            "file_path": {
                "type": "string",
                "description": "Workspace-relative or absolute source file path. Required for find_related."
            },
            "line": {
                "type": "integer",
                "description": "1-indexed source line inside file_path. Required for find_related."
            },
            "path": {
                "type": "string",
                "description": "Workspace-relative or absolute search root inside the workspace. Defaults to workspace root."
            },
            "content": {
                "type": "string",
                "description": "Content filter. Defaults to code.",
                "enum": ["code", "docs", "config", "all"]
            },
            "top_k": {
                "type": "integer",
                "description": "Maximum results to return. Defaults to 5, maximum 20."
            },
            "filter_paths": {
                "type": "array",
                "description": "Optional path prefixes to include",
                "items": {
                    "type": "string",
                    "description": "Workspace-relative path prefix to include"
                }
            },
            "filter_languages": {
                "type": "array",
                "description": "Optional language filters such as rust or python",
                "items": {
                    "type": "string",
                    "description": "Language name to include"
                }
            }
        },
        "required": ["operation"],
        "additionalProperties": false
    }))
    .expect("code_search tool schema should deserialize");

    let mut tool = Tool::new(
        Cow::Borrowed(TOOL_NAME),
        Cow::Borrowed(TOOL_DESCRIPTION),
        Arc::new(schema),
    );
    tool.annotations = Some(ToolAnnotations::new().read_only(true));
    tool
}

impl ServerHandler for CodeSearchMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..ServerInfo::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if request.name != TOOL_NAME {
            return Err(McpError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let args = match request.arguments {
            Some(arguments) => {
                serde_json::Value::Object(arguments.into_iter().collect::<JsonObject>())
            }
            None => {
                return Err(McpError::invalid_params(
                    "missing arguments for code_search tool",
                    None,
                ));
            }
        };
        match self.execute(args) {
            Ok(output) => {
                let structured = serde_json::to_value(&output)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                Ok(CallToolResult {
                    content: vec![Content::text(result_summary(&output))],
                    structured_content: Some(structured),
                    is_error: Some(false),
                    meta: None,
                })
            }
            Err(message) => Ok(CallToolResult {
                content: vec![Content::text(message)],
                structured_content: None,
                is_error: Some(true),
                meta: None,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodeSearchInput {
    operation: CodeSearchOperation,
    query: Option<String>,
    file_path: Option<PathBuf>,
    line: Option<usize>,
    path: Option<PathBuf>,
    content: Option<ContentFilter>,
    top_k: Option<usize>,
    filter_paths: Option<Vec<String>>,
    filter_languages: Option<Vec<String>>,
}

enum CodeSearchRequest {
    Search(SearchRequest),
    FindRelated(RelatedRequest),
}

fn build_request(
    workspace_root: &Path,
    input: CodeSearchInput,
) -> Result<CodeSearchRequest, String> {
    let root = resolve_search_root(workspace_root, input.path.as_deref())?;
    let content = input.content.unwrap_or_default();
    let top_k = input.top_k.unwrap_or(DEFAULT_TOP_K);
    let filters = SearchFilters::normalized(
        input.filter_paths.unwrap_or_default(),
        input.filter_languages.unwrap_or_default(),
    );

    match input.operation {
        CodeSearchOperation::Search => {
            if input.file_path.is_some() || input.line.is_some() {
                return Err("`file_path` and `line` are only valid for find_related".to_string());
            }
            let query = input
                .query
                .ok_or_else(|| "`query` is required for search".to_string())?;
            Ok(CodeSearchRequest::Search(SearchRequest {
                root,
                query,
                content,
                top_k,
                filters,
            }))
        }
        CodeSearchOperation::FindRelated => {
            if input.query.is_some() {
                return Err("`query` is only valid for search".to_string());
            }
            let file_path = input
                .file_path
                .ok_or_else(|| "`file_path` is required for find_related".to_string())?;
            let line = input
                .line
                .ok_or_else(|| "`line` is required for find_related".to_string())?;
            Ok(CodeSearchRequest::FindRelated(RelatedRequest {
                root,
                file_path,
                line,
                content,
                top_k,
                filters,
            }))
        }
    }
}

fn resolve_search_root(
    workspace_root: &Path,
    requested_path: Option<&Path>,
) -> Result<PathBuf, String> {
    let workspace = workspace_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let candidate = match requested_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => workspace.join(path),
        None => workspace.clone(),
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !canonical.starts_with(&workspace) {
        return Err(format!(
            "`path` must be inside the workspace root: {}",
            candidate.display()
        ));
    }
    if !canonical.is_dir() {
        return Err(format!(
            "`path` must resolve to a directory: {}",
            candidate.display()
        ));
    }
    Ok(canonical)
}

fn map_code_search_error(error: CodeSearchError) -> String {
    match error {
        CodeSearchError::InvalidInput(message)
        | CodeSearchError::ModelUnavailable(message)
        | CodeSearchError::Index(message)
        | CodeSearchError::Io(message) => message,
    }
}

fn result_summary(output: &SearchOutput) -> String {
    let count = output.results.len();
    match output.operation {
        CodeSearchOperation::Search => {
            if count == 0 {
                "No code search results".to_string()
            } else {
                format!("{count} code search results")
            }
        }
        CodeSearchOperation::FindRelated => {
            if count == 0 {
                "No related code chunks".to_string()
            } else {
                format!("{count} related code chunks")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::{CodeSearchService, HashEmbeddingProvider};

    fn test_server(workspace: PathBuf, cache: PathBuf) -> CodeSearchMcpServer {
        let service =
            CodeSearchService::new(Arc::new(HashEmbeddingProvider::new("test", 16)), cache);
        CodeSearchMcpServer::with_service(Arc::new(service), workspace)
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: code_search MCP rejects missing search query.
    #[test]
    fn execute_rejects_missing_search_query() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = test_server(temp.path().to_path_buf(), temp.path().join("cache"));
        let error = server
            .execute(json!({ "operation": "search" }))
            .expect_err("missing query should fail");
        assert!(error.contains("`query`"));
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: code_search MCP rejects paths outside the workspace.
    #[test]
    fn execute_rejects_path_outside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let server = test_server(
            workspace.path().to_path_buf(),
            workspace.path().join("cache"),
        );
        let error = server
            .execute(json!({
                "operation": "search",
                "query": "parse",
                "path": outside.path()
            }))
            .expect_err("outside path should fail");
        assert!(error.contains("inside the workspace"));
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: code_search MCP returns structured search results.
    #[test]
    fn execute_returns_search_results() {
        let workspace = tempfile::tempdir().expect("workspace");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            workspace.path().join("parser.rs"),
            "pub fn parse_input() {}\n",
        )
        .expect("write");
        let server = test_server(workspace.path().to_path_buf(), cache.path().to_path_buf());

        let output = server
            .execute(json!({
                "operation": "search",
                "query": "parse input",
                "top_k": 1
            }))
            .expect("search succeeds");

        assert_eq!(output.operation, CodeSearchOperation::Search);
        assert_eq!(output.results.len(), 1);
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: the MCP server advertises tools capability and the code_search tool.
    #[test]
    fn server_advertises_code_search_tool() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = test_server(temp.path().to_path_buf(), temp.path().join("cache"));
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert_eq!(server.tools.len(), 1);
        assert_eq!(server.tools[0].name.as_ref(), TOOL_NAME);
    }
}
