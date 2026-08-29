//! Server-owned reference search sessions for composer `@` lookups.
//!
//! The runtime aggregates skill metadata, configured MCP servers, and live file
//! search snapshots into protocol rows so UI clients only render and select.
//! Incremental file results are pushed to the requesting client through
//! connection-local `search/updated` and `search/completed` notifications.

use std::num::NonZero;
use std::path::PathBuf;
use std::sync::Arc;

use devo_file_search::FileMatch;
use devo_file_search::FileSearchOptions;
use devo_file_search::FileSearchSession;
use devo_file_search::FileSearchSnapshot;
use devo_file_search::SessionReporter;
use devo_protocol::ReferenceSearchId;
use devo_protocol::ReferenceSearchResult;
use devo_protocol::ReferenceSearchResultKind;
use devo_protocol::ReferenceSearchSnapshot;
use devo_protocol::ReferenceSearchStartParams;
use devo_protocol::ReferenceSearchUpdateParams;
use devo_util_fuzzy::fuzzy_match;
use tokio::sync::mpsc;

use super::ServerRuntime;
use crate::ProtocolErrorCode;
use crate::ServerEvent;
use crate::SkillRecord;
use crate::SkillScope;
use crate::SuccessResponse;
use crate::session_context::SessionRuntimeContext;
use devo_core::McpServerRecord;

const REFERENCE_FILE_LIMIT: usize = 20;

pub(super) struct ReferenceSearchState {
    connection_id: u64,
    query: String,
    skill_sources: Vec<SkillReferenceSource>,
    mcp_sources: Vec<McpReferenceSource>,
    file_matches: Vec<FileMatch>,
    total_file_match_count: usize,
    scanned_file_count: usize,
    file_search_complete: bool,
    file_session: FileSearchSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillReferenceSource {
    display_name: String,
    description: Option<String>,
    insert_text: String,
    mention_path: String,
    search_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpReferenceSource {
    id: String,
    display_name: String,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct ReferenceSearchFileUpdate {
    search_id: ReferenceSearchId,
    snapshot: FileSearchSnapshot,
}

struct ReferenceSearchReporter {
    search_id: ReferenceSearchId,
    update_tx: mpsc::UnboundedSender<ReferenceSearchFileUpdate>,
}

impl SessionReporter for ReferenceSearchReporter {
    fn on_update(&self, snapshot: &FileSearchSnapshot) {
        let _ = self.update_tx.send(ReferenceSearchFileUpdate {
            search_id: self.search_id.clone(),
            snapshot: snapshot.clone(),
        });
    }

    fn on_complete(&self) {}
}

impl ServerRuntime {
    /// Native `search/start` (L2-DES-APP-008): connection-local composer
    /// search; translates into the legacy machinery and projects the
    /// snapshot to the canonical camelCase shape.
    pub(super) async fn handle_native_search_start(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<
            devo_protocol::native::rpc_search::SearchStartParams,
        >(params)
        {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid canonical search/start params: {error}"),
                );
            }
        };
        match self
            .start_reference_search(
                connection_id,
                ReferenceSearchStartParams {
                    cwd: params.cwd,
                    query: params.query,
                },
            )
            .await
        {
            Ok(snapshot) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_search::SearchStartResult {
                    snapshot: snapshot.into(),
                },
            })
            .expect("serialize canonical search/start response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to start reference search: {error}"),
            ),
        }
    }

    /// Native `search/update` (L2-DES-APP-008).
    pub(super) async fn handle_native_search_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<
            devo_protocol::native::rpc_search::SearchUpdateParams,
        >(params)
        {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid canonical search/update params: {error}"),
                );
            }
        };
        match self
            .update_reference_search(ReferenceSearchUpdateParams {
                search_id: params.search_id,
                query: params.query,
            })
            .await
        {
            Ok(snapshot) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_search::SearchUpdateResult {
                    snapshot: snapshot.into(),
                },
            })
            .expect("serialize canonical search/update response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            ),
        }
    }

    /// Native `search/cancel` (L2-DES-APP-008).
    pub(super) async fn handle_native_search_cancel(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<
            devo_protocol::native::rpc_search::SearchCancelParams,
        >(params)
        {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid canonical search/cancel params: {error}"),
                );
            }
        };
        self.reference_searches
            .lock()
            .await
            .remove(&params.search_id);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_search::SearchCancelResult {},
        })
        .expect("serialize canonical search/cancel response")
    }

    async fn start_reference_search(
        self: &Arc<Self>,
        connection_id: u64,
        params: ReferenceSearchStartParams,
    ) -> anyhow::Result<ReferenceSearchSnapshot> {
        let cwd = params
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let runtime_context = self.deps.context_for_workspace(&cwd).await?;
        let search_id = ReferenceSearchId::new();
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let reporter = Arc::new(ReferenceSearchReporter {
            search_id: search_id.clone(),
            update_tx,
        });
        let file_session = devo_file_search::create_session(
            vec![cwd.clone()],
            FileSearchOptions {
                limit: NonZero::new(REFERENCE_FILE_LIMIT).expect("positive file limit"),
                compute_indices: true,
                ..FileSearchOptions::default()
            },
            reporter,
            /*cancel_flag*/ None,
        )?;
        let state = ReferenceSearchState {
            connection_id,
            query: params.query,
            skill_sources: skill_sources(&runtime_context.discover_skills(Some(&cwd), false)?),
            mcp_sources: Self::mcp_sources(&runtime_context),
            file_matches: Vec::new(),
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: false,
            file_session,
        };
        let snapshot = state.snapshot(&search_id);

        {
            let mut searches = self.reference_searches.lock().await;
            searches.insert(search_id.clone(), state);
        }

        self.spawn_reference_search_update_task(connection_id, update_rx);

        if let Some(state) = self.reference_searches.lock().await.get_mut(&search_id) {
            state.file_search_complete = false;
            state.file_session.update_query(&snapshot.query);
            return Ok(state.snapshot(&search_id));
        }

        Ok(snapshot)
    }

    async fn update_reference_search(
        &self,
        params: ReferenceSearchUpdateParams,
    ) -> anyhow::Result<ReferenceSearchSnapshot> {
        let mut searches = self.reference_searches.lock().await;
        let Some(state) = searches.get_mut(&params.search_id) else {
            anyhow::bail!("reference search session not found: {}", params.search_id);
        };

        if state.query == params.query {
            return Ok(state.snapshot(&params.search_id));
        }

        state.query = params.query;
        state.file_matches.clear();
        state.total_file_match_count = 0;
        state.scanned_file_count = 0;
        state.file_search_complete = false;
        state.file_session.update_query(&state.query);

        Ok(state.snapshot(&params.search_id))
    }

    fn spawn_reference_search_update_task(
        self: &Arc<Self>,
        connection_id: u64,
        mut update_rx: mpsc::UnboundedReceiver<ReferenceSearchFileUpdate>,
    ) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(update) = update_rx.recv().await {
                runtime
                    .apply_reference_search_file_snapshot(connection_id, update)
                    .await;
            }
        });
    }

    async fn apply_reference_search_file_snapshot(
        &self,
        connection_id: u64,
        update: ReferenceSearchFileUpdate,
    ) {
        let snapshot = {
            let mut searches = self.reference_searches.lock().await;
            let Some(state) = searches.get_mut(&update.search_id) else {
                return;
            };
            if state.connection_id != connection_id || state.query != update.snapshot.query {
                return;
            }

            state.file_matches = update
                .snapshot
                .matches
                .into_iter()
                .take(REFERENCE_FILE_LIMIT)
                .collect();
            state.total_file_match_count = update.snapshot.total_match_count;
            state.scanned_file_count = update.snapshot.scanned_file_count;
            state.file_search_complete = update.snapshot.walk_complete;
            state.snapshot(&update.search_id)
        };

        let (method, event) = if snapshot.file_search_complete {
            (
                "search/completed",
                ServerEvent::ReferenceSearchCompleted(snapshot),
            )
        } else {
            (
                "search/updated",
                ServerEvent::ReferenceSearchUpdated(snapshot),
            )
        };
        self.emit_connection_local_to_connection(connection_id, method, event)
            .await;
    }

    fn mcp_sources(runtime_context: &SessionRuntimeContext) -> Vec<McpReferenceSource> {
        runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .mcp_runtime
            .servers
            .iter()
            .map(mcp_source)
            .collect()
    }
}

impl ReferenceSearchState {
    pub(super) fn connection_id(&self) -> u64 {
        self.connection_id
    }

    fn snapshot(&self, search_id: &ReferenceSearchId) -> ReferenceSearchSnapshot {
        ReferenceSearchSnapshot {
            search_id: search_id.clone(),
            query: self.query.clone(),
            results: reference_results(
                &self.query,
                &self.skill_sources,
                &self.mcp_sources,
                &self.file_matches,
            ),
            total_file_match_count: self.total_file_match_count,
            scanned_file_count: self.scanned_file_count,
            file_search_complete: self.file_search_complete,
        }
    }
}

fn skill_sources(skills: &[SkillRecord]) -> Vec<SkillReferenceSource> {
    // Composer `@` only surfaces workspace (repo) skills — not global / system /
    // plugin / multi-agent catalog entries.
    skills
        .iter()
        .filter(|skill| skill.enabled && skill.scope == SkillScope::Repo)
        .map(skill_source)
        .collect()
}

fn skill_source(skill: &SkillRecord) -> SkillReferenceSource {
    let display_name = skill
        .interface
        .as_ref()
        .and_then(|interface| interface.display_name.as_deref())
        .unwrap_or(&skill.name)
        .to_string();
    let description = skill_description(skill);
    let search_terms = if display_name == skill.name {
        vec![skill.name.clone()]
    } else {
        vec![skill.name.clone(), display_name.clone()]
    };
    SkillReferenceSource {
        display_name,
        description,
        insert_text: format!("@{}", skill.name),
        mention_path: skill.path.to_string_lossy().into_owned(),
        search_terms,
    }
}

fn skill_description(skill: &SkillRecord) -> Option<String> {
    let description = skill
        .interface
        .as_ref()
        .and_then(|interface| interface.short_description.as_deref())
        .or(skill.short_description.as_deref())
        .unwrap_or(&skill.description);
    let trimmed = description.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn mcp_source(server: &McpServerRecord) -> McpReferenceSource {
    McpReferenceSource {
        id: server.id.0.clone(),
        display_name: server.display_name.clone(),
        enabled: server.enabled,
    }
}

fn reference_results(
    query: &str,
    skill_sources: &[SkillReferenceSource],
    mcp_sources: &[McpReferenceSource],
    file_matches: &[FileMatch],
) -> Vec<ReferenceSearchResult> {
    let filter = query.trim();
    let mut results = Vec::new();
    results.extend(skill_results(filter, skill_sources));
    results.extend(mcp_results(filter, mcp_sources));
    results.extend(file_results(file_matches));
    results
}

fn skill_results(filter: &str, sources: &[SkillReferenceSource]) -> Vec<ReferenceSearchResult> {
    let mut matches = sources
        .iter()
        .filter_map(|source| {
            let match_indices =
                reference_match_indices(filter, &source.display_name, &source.search_terms)?;
            Some((
                source,
                match_indices.score,
                ReferenceSearchResult {
                    kind: ReferenceSearchResultKind::Skill,
                    display_name: source.display_name.clone(),
                    description: source.description.clone(),
                    insert_text: source.insert_text.clone(),
                    mention_path: Some(source.mention_path.clone()),
                    file_path: None,
                    match_indices: match_indices.indices,
                    is_disabled: false,
                    disabled_reason: None,
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.display_name.cmp(&b.0.display_name))
    });
    matches.into_iter().map(|(_, _, result)| result).collect()
}

fn mcp_results(filter: &str, sources: &[McpReferenceSource]) -> Vec<ReferenceSearchResult> {
    let mut matches = sources
        .iter()
        .filter(|source| source.enabled)
        .filter_map(|source| {
            let match_indices = reference_match_indices(
                filter,
                &source.display_name,
                std::slice::from_ref(&source.id),
            )?;
            Some((
                source,
                match_indices.score,
                ReferenceSearchResult {
                    kind: ReferenceSearchResultKind::Mcp,
                    display_name: source.display_name.clone(),
                    description: Some(source.id.clone()),
                    insert_text: format!("@mcp:{}", source.id),
                    mention_path: Some(format!("mcp://server/{}", source.id)),
                    file_path: None,
                    match_indices: match_indices.indices,
                    is_disabled: false,
                    disabled_reason: None,
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.display_name.cmp(&b.0.display_name))
    });
    matches.into_iter().map(|(_, _, result)| result).collect()
}

fn file_results(file_matches: &[FileMatch]) -> Vec<ReferenceSearchResult> {
    file_matches
        .iter()
        .filter_map(|file_match| {
            let display_name = file_match.path.to_string_lossy().into_owned();
            if display_name.trim().is_empty() {
                return None;
            }
            let basename = file_match
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(display_name.as_str());
            let insert_text = format!("@{basename}");

            Some(ReferenceSearchResult {
                kind: ReferenceSearchResultKind::File,
                display_name: display_name.clone(),
                description: None,
                insert_text,
                mention_path: Some(display_name),
                file_path: Some(file_match.full_path()),
                match_indices: file_match
                    .indices
                    .as_ref()
                    .map(|indices| indices.iter().map(|idx| *idx as usize).collect()),
                is_disabled: false,
                disabled_reason: None,
            })
        })
        .collect()
}

struct MatchIndices {
    indices: Option<Vec<usize>>,
    score: i32,
}

fn reference_match_indices(
    filter: &str,
    display_name: &str,
    search_terms: &[String],
) -> Option<MatchIndices> {
    if filter.is_empty() {
        return Some(MatchIndices {
            indices: None,
            score: 0,
        });
    }
    if let Some((indices, score)) = fuzzy_match(display_name, filter) {
        return Some(MatchIndices {
            indices: Some(indices),
            score,
        });
    }
    search_terms
        .iter()
        .filter(|term| term.as_str() != display_name)
        .filter_map(|term| fuzzy_match(term, filter).map(|(_indices, score)| score))
        .min()
        .map(|score| MatchIndices {
            indices: None,
            score,
        })
}

#[cfg(test)]
mod tests {
    use devo_file_search::MatchType;
    use pretty_assertions::assert_eq;

    use super::*;

    fn skill(name: &str) -> SkillReferenceSource {
        SkillReferenceSource {
            display_name: name.to_string(),
            description: Some(format!("{name} skill")),
            insert_text: format!("@{name}"),
            mention_path: format!("skills/{name}/SKILL.md"),
            search_terms: vec![name.to_string()],
        }
    }

    fn mcp(id: &str, display_name: &str) -> McpReferenceSource {
        McpReferenceSource {
            id: id.to_string(),
            display_name: display_name.to_string(),
            enabled: true,
        }
    }

    fn file(path: &str) -> FileMatch {
        FileMatch {
            score: 100,
            path: PathBuf::from(path),
            match_type: MatchType::File,
            root: PathBuf::from("."),
            indices: Some(vec![0, 1]),
        }
    }

    #[test]
    fn skill_sources_keep_only_enabled_repo_skills() {
        let skills = [
            SkillRecord {
                id: "repo".into(),
                name: "repo-skill".into(),
                description: "workspace".into(),
                short_description: None,
                interface: None,
                dependencies: None,
                path: PathBuf::from(".devo/skills/repo-skill/SKILL.md"),
                enabled: true,
                source: crate::SkillSource::Workspace {
                    cwd: PathBuf::from("."),
                },
                scope: SkillScope::Repo,
                plugin_id: None,
            },
            SkillRecord {
                id: "user".into(),
                name: "create-skill".into(),
                description: "global".into(),
                short_description: None,
                interface: None,
                dependencies: None,
                path: PathBuf::from("/home/user/.devo/skills/create-skill/SKILL.md"),
                enabled: true,
                source: crate::SkillSource::User,
                scope: SkillScope::User,
                plugin_id: None,
            },
            SkillRecord {
                id: "system".into(),
                name: "multi-agent".into(),
                description: "system".into(),
                short_description: None,
                interface: None,
                dependencies: None,
                path: PathBuf::from("/system/skills/multi-agent/SKILL.md"),
                enabled: true,
                source: crate::SkillSource::System,
                scope: SkillScope::System,
                plugin_id: None,
            },
            SkillRecord {
                id: "disabled-repo".into(),
                name: "disabled-repo".into(),
                description: "off".into(),
                short_description: None,
                interface: None,
                dependencies: None,
                path: PathBuf::from(".devo/skills/disabled-repo/SKILL.md"),
                enabled: false,
                source: crate::SkillSource::Workspace {
                    cwd: PathBuf::from("."),
                },
                scope: SkillScope::Repo,
                plugin_id: None,
            },
        ];

        assert_eq!(
            skill_sources(&skills)
                .into_iter()
                .map(|source| source.display_name)
                .collect::<Vec<_>>(),
            vec!["repo-skill".to_string()]
        );
    }

    #[test]
    fn mcp_results_omit_disabled_servers() {
        let sources = [
            mcp("docs", "Docs"),
            McpReferenceSource {
                id: "off".to_string(),
                display_name: "Off".to_string(),
                enabled: false,
            },
        ];

        assert_eq!(
            mcp_results("", &sources)
                .into_iter()
                .map(|result| result.display_name)
                .collect::<Vec<_>>(),
            vec!["Docs".to_string()]
        );
    }

    #[test]
    fn empty_query_orders_skill_mcp_then_file() {
        let results = reference_results(
            "",
            &[skill("openai-docs")],
            &[mcp("docs", "Docs")],
            &[file("src/main.rs")],
        );

        assert_eq!(
            results
                .into_iter()
                .map(|result| (result.kind, result.display_name))
                .collect::<Vec<_>>(),
            vec![
                (ReferenceSearchResultKind::Skill, "openai-docs".to_string()),
                (ReferenceSearchResultKind::Mcp, "Docs".to_string()),
                (ReferenceSearchResultKind::File, "src/main.rs".to_string()),
            ]
        );
    }

    #[test]
    fn non_empty_query_filters_sources_and_keeps_category_order() {
        let results = reference_results(
            "docs",
            &[skill("docs-skill"), skill("imagegen")],
            &[mcp("docs", "Docs")],
            &[file("docs/tui-chat-composer.md")],
        );

        assert_eq!(
            results
                .into_iter()
                .map(|result| (result.kind, result.display_name))
                .collect::<Vec<_>>(),
            vec![
                (ReferenceSearchResultKind::Skill, "docs-skill".to_string()),
                (ReferenceSearchResultKind::Mcp, "Docs".to_string()),
                (
                    ReferenceSearchResultKind::File,
                    "docs/tui-chat-composer.md".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn empty_file_display_names_are_not_returned() {
        let results = reference_results("", &[], &[], &[file(""), file("apps")]);

        assert_eq!(
            results
                .into_iter()
                .map(|result| (result.kind, result.display_name))
                .collect::<Vec<_>>(),
            vec![(ReferenceSearchResultKind::File, "apps".to_string())]
        );
    }

    #[test]
    fn file_results_use_basename_insert_text_and_relative_mention_path() {
        let results = reference_results("", &[], &[], &[file("crates/tui/src/interactive.rs")]);

        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.kind, ReferenceSearchResultKind::File);
        assert_eq!(result.display_name, "crates/tui/src/interactive.rs");
        assert_eq!(result.insert_text, "@interactive.rs");
        assert_eq!(
            result.mention_path.as_deref(),
            Some("crates/tui/src/interactive.rs")
        );
    }

    #[test]
    fn stale_file_snapshot_does_not_mutate_state() {
        let search_id = ReferenceSearchId::new();
        let mut state = ReferenceSearchState {
            connection_id: 1,
            query: "new".to_string(),
            skill_sources: Vec::new(),
            mcp_sources: Vec::new(),
            file_matches: Vec::new(),
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: false,
            file_session: devo_file_search::create_session(
                vec![PathBuf::from(".")],
                FileSearchOptions::default(),
                Arc::new(ReferenceSearchReporter {
                    search_id: search_id.clone(),
                    update_tx: mpsc::unbounded_channel().0,
                }),
                None,
            )
            .expect("create file search session"),
        };
        let old_snapshot = FileSearchSnapshot {
            query: "old".to_string(),
            matches: vec![file("src/old.rs")],
            total_match_count: 1,
            scanned_file_count: 1,
            walk_complete: true,
        };

        if state.query == old_snapshot.query {
            state.file_matches = old_snapshot.matches;
        }

        assert_eq!(state.snapshot(&search_id).results, Vec::new());
    }
}
