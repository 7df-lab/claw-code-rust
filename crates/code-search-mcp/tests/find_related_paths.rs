use std::path::PathBuf;
use std::sync::Arc;

use devo_code_search_mcp::{
    CodeSearchError, CodeSearchService, ContentFilter, HashEmbeddingProvider, RelatedRequest,
    SearchFilters,
};

#[test]
fn find_related_rejects_relative_source_paths_outside_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache");
    let service = CodeSearchService::new(
        Arc::new(HashEmbeddingProvider::new("test", 16)),
        cache.path().to_path_buf(),
    );
    let outside_paths = vec![
        PathBuf::from("..").join("outside.rs"),
        PathBuf::from("src")
            .join("..")
            .join("..")
            .join("outside.rs"),
    ];

    for file_path in outside_paths {
        let error = service
            .find_related(RelatedRequest {
                root: temp.path().to_path_buf(),
                file_path,
                line: 1,
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect_err("outside source path should be rejected");

        let CodeSearchError::InvalidInput(message) = error else {
            panic!("expected invalid input, got {error:?}");
        };
        assert!(message.contains("outside the search root"));
    }
}
