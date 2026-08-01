//! Semantic code retrieval and the bundled `code_search` MCP server.
//!
//! This crate owns the retrieval engine and the stdio MCP adapter Devo launches
//! as `devo-code-search-mcp`.

mod cache;
mod chunking;
mod dense;
mod files;
mod grammars;
mod index;
mod matrix;
mod mcp;
mod ranking;
mod refresh;
mod semantic;
mod service;
mod singleflight;
mod tokens;
mod types;
mod watch;

pub use dense::EmbeddingProvider;
pub use dense::HashEmbeddingProvider;
pub use dense::Model2VecEmbeddingProvider;
pub use mcp::CodeSearchMcpServer;
pub use service::CodeSearchService;
pub use types::Chunk;
pub use types::CodeSearchError;
pub use types::CodeSearchOperation;
pub use types::ContentFilter;
pub use types::ContentKind;
pub use types::DEFAULT_TOP_K;
pub use types::IndexStats;
pub use types::MAX_TOP_K;
pub use types::RelatedRequest;
pub use types::SearchFilters;
pub use types::SearchOutput;
pub use types::SearchRequest;
pub use types::SearchResult;
pub use types::validate_top_k;
