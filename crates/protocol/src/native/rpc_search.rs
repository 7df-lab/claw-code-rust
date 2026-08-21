//! Native `search/*` (connection-local composer reference search).
//!
//! The search is ephemeral and connection-scoped — it deliberately does NOT
//! ride the durable `subscription/*` selector model. Types mirror the legacy
//! reference-search shapes with native camelCase naming; conversions are
//! 1:1.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// Reused legacy uuid newtype: the wire form (a uuid string) is identical on
/// both surfaces.
pub type SearchId = crate::ReferenceSearchId;

// ── search/start ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchStartResult {
    pub snapshot: SearchSnapshot,
}

// ── search/update ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchUpdateParams {
    pub search_id: SearchId,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchUpdateResult {
    pub snapshot: SearchSnapshot,
}

// ── search/cancel ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchCancelParams {
    pub search_id: SearchId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchCancelResult {}

// ── shared snapshot types ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnapshot {
    pub search_id: SearchId,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total_file_match_count: usize,
    pub scanned_file_count: usize,
    pub file_search_complete: bool,
}

impl From<crate::ReferenceSearchSnapshot> for SearchSnapshot {
    fn from(snapshot: crate::ReferenceSearchSnapshot) -> Self {
        Self {
            search_id: snapshot.search_id,
            query: snapshot.query,
            results: snapshot
                .results
                .into_iter()
                .map(SearchResult::from)
                .collect(),
            total_file_match_count: snapshot.total_file_match_count,
            scanned_file_count: snapshot.scanned_file_count,
            file_search_complete: snapshot.file_search_complete,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub kind: SearchResultKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub insert_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_indices: Option<Vec<usize>>,
    #[serde(default)]
    pub is_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl From<crate::ReferenceSearchResult> for SearchResult {
    fn from(result: crate::ReferenceSearchResult) -> Self {
        Self {
            kind: result.kind.into(),
            display_name: result.display_name,
            description: result.description,
            insert_text: result.insert_text,
            mention_path: result.mention_path,
            file_path: result.file_path,
            match_indices: result.match_indices,
            is_disabled: result.is_disabled,
            disabled_reason: result.disabled_reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SearchResultKind {
    Skill,
    Mcp,
    File,
}

impl From<crate::ReferenceSearchResultKind> for SearchResultKind {
    fn from(kind: crate::ReferenceSearchResultKind) -> Self {
        match kind {
            crate::ReferenceSearchResultKind::Skill => Self::Skill,
            crate::ReferenceSearchResultKind::Mcp => Self::Mcp,
            crate::ReferenceSearchResultKind::File => Self::File,
        }
    }
}

// Inverse conversions: first-party consumers (the TUI popup) still render
// legacy snapshots while server notifications stay legacy-shaped during the
// event cutover.
impl From<SearchSnapshot> for crate::ReferenceSearchSnapshot {
    fn from(snapshot: SearchSnapshot) -> Self {
        Self {
            search_id: snapshot.search_id,
            query: snapshot.query,
            results: snapshot
                .results
                .into_iter()
                .map(crate::ReferenceSearchResult::from)
                .collect(),
            total_file_match_count: snapshot.total_file_match_count,
            scanned_file_count: snapshot.scanned_file_count,
            file_search_complete: snapshot.file_search_complete,
        }
    }
}

impl From<SearchResult> for crate::ReferenceSearchResult {
    fn from(result: SearchResult) -> Self {
        Self {
            kind: result.kind.into(),
            display_name: result.display_name,
            description: result.description,
            insert_text: result.insert_text,
            mention_path: result.mention_path,
            file_path: result.file_path,
            match_indices: result.match_indices,
            is_disabled: result.is_disabled,
            disabled_reason: result.disabled_reason,
        }
    }
}

impl From<SearchResultKind> for crate::ReferenceSearchResultKind {
    fn from(kind: SearchResultKind) -> Self {
        match kind {
            SearchResultKind::Skill => Self::Skill,
            SearchResultKind::Mcp => Self::Mcp,
            SearchResultKind::File => Self::File,
        }
    }
}
