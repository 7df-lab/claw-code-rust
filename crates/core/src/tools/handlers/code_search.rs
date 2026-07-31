use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use devo_code_search::{
    CodeSearchError, CodeSearchOperation, CodeSearchService, ContentFilter, DEFAULT_TOP_K,
    RelatedRequest, SearchFilters, SearchOutput, SearchRequest,
};
use serde::Deserialize;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgress, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::registry_plan::code_search_tool_spec;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

pub struct CodeSearchHandler {
    spec: ToolSpec,
    service: Arc<CodeSearchService>,
}

impl CodeSearchHandler {
    pub fn new() -> Self {
        Self::with_service(Arc::new(CodeSearchService::production()))
    }

    pub fn new_with_network_proxy(network_proxy: devo_network_proxy::NetworkProxyConfig) -> Self {
        Self::with_service(Arc::new(CodeSearchService::production_with_network_proxy(
            network_proxy,
        )))
    }

    pub fn with_service(service: Arc<CodeSearchService>) -> Self {
        let spec = code_search_tool_spec();
        Self { spec, service }
    }
}

impl Default for CodeSearchHandler {
    fn default() -> Self {
        Self::new()
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

#[async_trait]
impl ToolHandler for CodeSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        if ctx.cancel_token.is_cancelled() {
            return Err(ToolCallError::Cancelled);
        }
        let input: CodeSearchInput = serde_json::from_value(input)
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        let request = build_request(&ctx.workspace_root, input)?;

        if let Some(ref sender) = progress {
            let root = match &request {
                CodeSearchRequest::Search(r) => &r.root,
                CodeSearchRequest::FindRelated(r) => &r.root,
            };
            let content = match &request {
                CodeSearchRequest::Search(r) => r.content,
                CodeSearchRequest::FindRelated(r) => r.content,
            };
            if self.service.needs_index_build(root, content) {
                let _ = sender.send(ToolProgress::StatusUpdate {
                    message: "First code search: building index (may take a moment, usually quick)"
                        .to_string(),
                    percent: None,
                });
            }
        }

        let service = Arc::clone(&self.service);
        let output = tokio::task::spawn_blocking(move || match request {
            CodeSearchRequest::Search(request) => service.search(request),
            CodeSearchRequest::FindRelated(request) => service.find_related(request),
        })
        .await
        .map_err(|error| ToolCallError::ExecutionFailed(error.to_string()))?
        .map_err(map_code_search_error)?;
        if ctx.cancel_token.is_cancelled() {
            return Err(ToolCallError::Cancelled);
        }
        let summary = result_summary(&output);
        let display = display_output(&output, ctx.budgets.output_limit_bytes);
        let json = serde_json::to_value(&output)
            .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        let mut result = ToolResult::success(ToolResultContent::Json(json), summary);
        result.display_content = Some(display);
        Ok(result)
    }
}

enum CodeSearchRequest {
    Search(SearchRequest),
    FindRelated(RelatedRequest),
}

fn build_request(
    workspace_root: &Path,
    input: CodeSearchInput,
) -> Result<CodeSearchRequest, ToolCallError> {
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
                return Err(ToolCallError::InvalidInput(
                    "`file_path` and `line` are only valid for find_related".to_string(),
                ));
            }
            let query = input.query.ok_or_else(|| {
                ToolCallError::InvalidInput("`query` is required for search".to_string())
            })?;
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
                return Err(ToolCallError::InvalidInput(
                    "`query` is only valid for search".to_string(),
                ));
            }
            let file_path = input.file_path.ok_or_else(|| {
                ToolCallError::InvalidInput("`file_path` is required for find_related".to_string())
            })?;
            let line = input.line.ok_or_else(|| {
                ToolCallError::InvalidInput("`line` is required for find_related".to_string())
            })?;
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
) -> Result<PathBuf, ToolCallError> {
    let workspace = workspace_root
        .canonicalize()
        .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
    let candidate = match requested_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => workspace.join(path),
        None => workspace.clone(),
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
    if !canonical.starts_with(&workspace) {
        return Err(ToolCallError::InvalidInput(format!(
            "`path` must be inside the workspace root: {}",
            candidate.display()
        )));
    }
    if !canonical.is_dir() {
        return Err(ToolCallError::InvalidInput(format!(
            "`path` must resolve to a directory: {}",
            candidate.display()
        )));
    }
    Ok(canonical)
}

fn map_code_search_error(error: CodeSearchError) -> ToolCallError {
    match error {
        CodeSearchError::InvalidInput(message) => ToolCallError::InvalidInput(message),
        CodeSearchError::ModelUnavailable(message) => ToolCallError::NeedsConfiguration(message),
        CodeSearchError::Index(message) | CodeSearchError::Io(message) => {
            ToolCallError::ExecutionFailed(message)
        }
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

fn display_output(output: &SearchOutput, output_limit_bytes: usize) -> String {
    if output.results.is_empty() {
        return result_summary(output);
    }
    let mut display = String::new();
    for result in &output.results {
        let first_line = result
            .chunk
            .content
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        display.push_str(&format!(
            "{} score={:.4}\n{}\n\n",
            result.chunk.location(),
            result.score,
            first_line
        ));
        if display.len() >= output_limit_bytes {
            display.truncate(output_limit_bytes);
            display.push_str("\n(truncated)");
            break;
        }
    }
    display.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use devo_code_search::HashEmbeddingProvider;
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    use crate::contracts::ToolBudgets;
    use crate::invocation::ToolCallId;

    use super::*;

    fn context(root: PathBuf) -> ToolContext {
        ToolContext {
            tool_call_id: ToolCallId("call-1".to_string()),
            session_id: "session-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            workspace_root: root,
            budgets: ToolBudgets {
                output_limit_bytes: 32_768,
                wall_time_limit_ms: None,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_profile: None,
        }
    }

    fn test_handler(cache_dir: PathBuf) -> CodeSearchHandler {
        let service =
            CodeSearchService::new(Arc::new(HashEmbeddingProvider::new("test", 16)), cache_dir);
        CodeSearchHandler::with_service(Arc::new(service))
    }

    #[test]
    fn handler_constructor_sets_code_search_spec() {
        let handler = CodeSearchHandler::new();

        assert_eq!(handler.spec().name, "code_search");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code_search validates operation-specific required fields.
    #[tokio::test]
    async fn handler_rejects_missing_search_query() {
        let temp = tempfile::tempdir().expect("tempdir");
        let handler = test_handler(temp.path().join("cache"));
        let error = handler
            .handle(
                context(temp.path().to_path_buf()),
                serde_json::json!({ "operation": "search" }),
                None,
            )
            .await
            .expect_err("missing query should fail");

        assert!(matches!(error, ToolCallError::InvalidInput(_)));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code_search search roots cannot escape the workspace.
    #[tokio::test]
    async fn handler_rejects_path_outside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let handler = test_handler(workspace.path().join("cache"));
        let error = handler
            .handle(
                context(workspace.path().to_path_buf()),
                serde_json::json!({
                    "operation": "search",
                    "query": "parse",
                    "path": outside.path()
                }),
                None,
            )
            .await
            .expect_err("outside path should fail");

        assert!(matches!(error, ToolCallError::InvalidInput(_)));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code_search returns structured JSON and display content for successful searches.
    #[tokio::test]
    async fn handler_returns_json_results_and_display_content() {
        let workspace = tempfile::tempdir().expect("workspace");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            workspace.path().join("parser.rs"),
            "pub fn parse_input() {}\n",
        )
        .expect("write");
        let handler = test_handler(cache.path().to_path_buf());

        let result = handler
            .handle(
                context(workspace.path().to_path_buf()),
                serde_json::json!({
                    "operation": "search",
                    "query": "parse input",
                    "top_k": 1
                }),
                None,
            )
            .await
            .expect("search succeeds");

        let ToolResultContent::Json(json) = result.content else {
            panic!("expected JSON result");
        };
        assert_eq!(json["operation"], "search");
        assert_eq!(json["results"].as_array().expect("results").len(), 1);
        assert!(
            result
                .display_content
                .expect("display")
                .contains("parser.rs")
        );
    }
}
