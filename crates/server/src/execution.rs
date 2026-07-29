use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::AppConfigStore;
use devo_core::ProviderVendorCatalog;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

use devo_core::AgentsMdConfig;
use devo_core::ModelCatalog;
use devo_core::SessionConfig;
use devo_core::SessionRecord;
use devo_core::SessionState;
use devo_core::SkillCatalog;
use devo_core::SkillError;
#[cfg(test)]
use devo_core::TurnConfig;
use devo_core::TurnId;
use devo_core::tools::ToolRegistry;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::PendingInputItem;
use devo_protocol::RequestUserInputResponse;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRouter;

#[cfg(test)]
use crate::InputItem;
use crate::SkillRecord;
use crate::db::Database;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
#[cfg(test)]
use crate::session_context::ResolvedInput;
use crate::session_context::SessionRuntimeContext;
use crate::turn::TurnMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedTurnItem {
    pub(crate) turn_id: TurnId,
    pub(crate) turn_kind: devo_core::TurnKind,
    pub(crate) item_id: devo_core::ItemId,
    pub(crate) turn_item: devo_core::TurnItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SandboxBypassKey {
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    pub(crate) sandbox_permissions: String,
}

pub(crate) fn sandbox_bypass_key_from_pending(
    pending: &PendingApproval,
) -> Option<SandboxBypassKey> {
    let command = pending.command.clone()?;
    Some(SandboxBypassKey {
        command,
        cwd: pending.cwd.clone(),
        sandbox_permissions: pending.sandbox_permissions.clone(),
    })
}

pub(crate) struct PendingApproval {
    pub(crate) owner_session_id: devo_protocol::SessionId,
    pub(crate) turn_id: TurnId,
    pub(crate) tool_name: String,
    pub(crate) resource: Option<devo_safety::ResourceKind>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) command_prefix: Option<Vec<String>>,
    pub(crate) command_pattern: Option<Vec<String>>,
    pub(crate) requests_escalation: bool,
    pub(crate) command: Option<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) sandbox_permissions: String,
    pub(crate) tx: oneshot::Sender<ApprovalDecisionValue>,
}

pub(crate) struct PendingUserInput {
    pub(crate) turn_id: TurnId,
    /// The questions the tool asked; kept so subscription snapshots can
    /// rebuild the waiting `UserInputRequest` item (08 §4).
    pub(crate) questions: Vec<devo_protocol::RequestUserInputQuestion>,
    pub(crate) tx: oneshot::Sender<RequestUserInputResponse>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ApprovalGrantCache {
    pub(crate) tools: HashSet<String>,
    pub(crate) hosts: HashSet<String>,
    pub(crate) read_path_prefixes: HashSet<PathBuf>,
    pub(crate) write_path_prefixes: HashSet<PathBuf>,
    pub(crate) command_prefixes: HashSet<Vec<String>>,
    pub(crate) command_patterns: HashSet<Vec<String>>,
    /// Exact shell command + cwd grants (session-scoped; no wildcards).
    pub(crate) exact_commands: HashSet<(String, PathBuf)>,
    pub(crate) sandbox_bypass_commands: HashSet<SandboxBypassKey>,
}

/// Shared server-owned runtime dependencies used by live turn execution.
pub struct ServerRuntimeDependencies {
    /// TODO: the router method is, take the binding of model and provider, then decide which ModelProviderSdk to call. so, let's move this functionality to ModelProviderSdkRegistry, as a method.
    /// Provider router facade for model invocation dispatch.
    #[allow(dead_code)]
    pub(crate) provider_router: Arc<dyn ProviderRouter>,
    /// ProviderVendor catalog used to resolve current provider.
    #[allow(dead_code)]
    pub(crate) provider_vendor_catalog: Arc<ProviderVendorCatalog>,
    /// Model catalog used to resolve builtin prompt metadata.
    pub(crate) model_catalog: Arc<dyn ModelCatalog>,
    /// SQLite database for session metadata, token stats, and pending queues.
    pub(crate) db: Arc<Database>,
    /// Shared app config loaded from user and optional workspace config files.
    pub(crate) config_store: Arc<std::sync::Mutex<AppConfigStore>>,
    /// User-level process context used before a concrete session exists.
    pub(crate) process_context: Arc<SessionRuntimeContext>,
}

impl ServerRuntimeDependencies {
    /// Creates a new bundle of runtime dependencies for the transport server.
    /// TODO: Should fix the clippy::too_many_arguments, decrease the arguments count.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn ModelProviderSDK>,
        provider_router: Arc<dyn ProviderRouter>,
        registry: Arc<ToolRegistry>,
        default_model: String,
        model_catalog: Arc<dyn ModelCatalog>,
        provider_vendor_catalog: Arc<ProviderVendorCatalog>,
        skill_catalog: Box<dyn SkillCatalog + Send>,
        agents_md: AgentsMdConfig,
        db: Arc<Database>,
        config_store: Arc<std::sync::Mutex<AppConfigStore>>,
    ) -> Self {
        let skill_catalog = Arc::new(StdMutex::new(skill_catalog));
        let process_context = Arc::new(SessionRuntimeContext::from_parts(
            Arc::clone(&provider),
            Arc::clone(&provider_router),
            Arc::clone(&registry),
            default_model.clone(),
            Arc::clone(&model_catalog),
            Arc::clone(&skill_catalog),
            agents_md.clone(),
            Arc::clone(&config_store),
        ));
        Self {
            provider_router,
            model_catalog,
            provider_vendor_catalog,
            db,
            config_store,
            process_context,
        }
    }

    pub(crate) async fn context_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> anyhow::Result<Arc<SessionRuntimeContext>> {
        let user_config_dir = self
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .user_config_dir()
            .to_path_buf();
        SessionRuntimeContext::load_for_workspace(
            user_config_dir,
            Some(workspace_root),
            &self.process_context,
        )
        .await
    }

    /// Resolves the full turn configuration used by the core query loop.
    #[cfg(test)]
    pub(crate) fn resolve_turn_config(
        &self,
        requested_model: Option<&str>,
        reasoning_effort_selection: Option<String>,
    ) -> TurnConfig {
        self.process_context
            .resolve_turn_config(requested_model, reasoning_effort_selection)
    }

    /// Should move the discover skill main logic to skills crate, and server just keep a simple wrapper.
    /// Returns the current skill catalog snapshot for one optional workspace root.
    pub(crate) fn discover_skills(
        &self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        self.process_context
            .discover_skills(workspace_root, force_reload)
    }

    pub(crate) fn set_skill_enabled(
        &self,
        path: PathBuf,
        enabled: bool,
        workspace_root: Option<&Path>,
    ) -> anyhow::Result<Vec<SkillRecord>> {
        self.process_context
            .set_skill_enabled(path, enabled, workspace_root)
    }

    /// Renders turn input items and resolves any referenced skills into prompt-visible messages.
    #[cfg(test)]
    pub(crate) fn resolve_input_items(
        &self,
        input: &[InputItem],
        workspace_root: Option<&Path>,
    ) -> Result<Option<ResolvedInput>, SkillError> {
        self.process_context
            .resolve_input_items(input, workspace_root)
    }
}

/// Mutable per-session runtime state owned by the server.
pub(crate) struct RuntimeSession {
    /// Workspace-scoped runtime dependencies resolved when this session was created.
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    /// Canonical persisted session metadata when the session is durable.
    pub(crate) record: Option<SessionRecord>,
    /// Transport-facing metadata exposed over the API.
    pub(crate) summary: SessionMetadata,
    /// Lock-free snapshot of the session configuration for server coordination paths.
    pub(crate) config: SessionConfig,
    /// Canonical core session state used by the query loop.
    pub(crate) core_session: Arc<Mutex<SessionState>>,
    /// Currently active turn, if any.
    pub(crate) active_turn: Option<TurnMetadata>,
    /// Latest terminal turn metadata for the session.
    pub(crate) latest_turn: Option<TurnMetadata>,
    /// Number of items loaded or appended for the session.
    pub(crate) loaded_item_count: u64,
    /// Replay-friendly ordered history used by interactive clients during session resume.
    pub(crate) history_items: Vec<SessionHistoryItem>,
    /// Canonical persisted turn items in prompt order for replay/compaction bookkeeping.
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    /// Latest compaction snapshot used to rebuild the model-facing prompt view.
    pub(crate) latest_compaction_snapshot: Option<devo_core::CompactionSnapshotLine>,
    /// Shared handle to the pending-turn queue owned by `core_session`.
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    /// Shared handle to the `/btw` queue owned by `core_session`.
    pub(crate) btw_input_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    /// Tool exposure policy for turns run in this session.
    pub(crate) agent_tool_policy: devo_protocol::AgentToolPolicy,
    /// Optional maximum number of turns allowed in this session.
    pub(crate) max_turns: Option<u32>,
    /// Deferred completion info for in-progress assistant text item.
    /// Cleared when the item is completed; used for crash/interrupt recovery.
    pub(crate) deferred_assistant: Option<(devo_core::ItemId, u64, String)>,
    /// Deferred completion info for in-progress reasoning text item.
    pub(crate) deferred_reasoning: Option<(devo_core::ItemId, u64, String)>,
    /// Monotonic session-scoped item sequence counter.
    pub(crate) next_item_seq: u64,
    /// First user input captured from the session's first turn, used for title generation.
    pub(crate) first_user_input: Option<String>,
    /// Session-specific tool registry, used when the session was created with
    /// request-scoped tool sources such as ACP MCP servers.
    pub(crate) tool_registry: Option<Arc<ToolRegistry>>,
    /// Session-scoped ledger of files read/written by tools (used by `edit`).
    pub(crate) file_read_ledger: Arc<devo_core::tools::FileReadLedger>,
    /// Session-scoped approvals granted through ACP permission responses.
    pub(crate) session_approval_cache: ApprovalGrantCache,
    /// Turn-scoped approvals granted through ACP permission responses.
    pub(crate) turn_approval_cache: ApprovalGrantCache,
    /// Whether the locked session context has been written to rollout storage.
    pub(crate) session_context_recorded: bool,
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::Model;
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderVendorCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::InputItem;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::ProviderWireApi;
    use devo_protocol::StreamEvent;
    use devo_provider::ModelProviderSDK;
    use devo_provider::ProviderRoute;
    use devo_provider::SingleProviderRouter;
    use futures::Stream;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::db::Database;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("not used by turn config resolution tests")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            unreachable!("not used by turn config resolution tests")
        }

        fn name(&self) -> &str {
            "noop"
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("devo-{name}-{nanos}-{id}"))
    }

    fn test_deps(config: &str) -> ServerRuntimeDependencies {
        let root = unique_temp_dir("turn-config-model-name");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("config.toml"), config).expect("write config");
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(Database::open(root.join("test.db")).expect("open db"));

        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            "catalog-slug".to_string(),
            Arc::new(PresetModelCatalog::new(vec![
                Model {
                    slug: "catalog-slug".to_string(),
                    display_name: "Catalog Model".to_string(),
                    ..Model::default()
                },
                Model {
                    slug: "catalog-slug-thinking".to_string(),
                    display_name: "Catalog Thinking Model".to_string(),
                    ..Model::default()
                },
            ])),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(root, /*workspace_root*/ None).expect("load config"),
            )),
        )
    }

    #[test]
    fn resolve_input_items_preserves_prompt_message_boundaries() {
        let deps = test_deps("");
        let resolved = deps
            .resolve_input_items(
                &[
                    InputItem::Text {
                        text: "first question".to_string(),
                    },
                    InputItem::Text {
                        text: "second context".to_string(),
                    },
                ],
                None,
            )
            .expect("resolve input")
            .expect("resolved input");

        assert_eq!(
            resolved,
            ResolvedInput {
                prompt_text: "first question\nsecond context".to_string(),
                prompt_messages: vec!["first question".to_string(), "second context".to_string()],
            }
        );
    }

    #[tokio::test]
    async fn context_for_workspace_loads_distinct_project_model_catalogs() {
        let deps = test_deps("");
        let root = unique_temp_dir("session-context-project-models");
        let workspace_a = root.join("workspace-a");
        let workspace_b = root.join("workspace-b");
        std::fs::create_dir_all(workspace_a.join(".devo")).expect("create workspace a config dir");
        std::fs::create_dir_all(workspace_b.join(".devo")).expect("create workspace b config dir");
        let legacy_models_path = workspace_a.join(".devo").join("models.json");
        let legacy_models =
            br#"[{"slug":"legacy-only-workspace-model","display_name":"Legacy Only"}]"#;
        std::fs::write(&legacy_models_path, legacy_models).expect("write legacy workspace models");
        let absent_legacy_models_path = workspace_b.join(".devo").join("models.json");
        std::fs::write(
            workspace_a.join(".devo").join("config.toml"),
            r#"
[model.workspace-a-model]
display_name = "Workspace A"
"#,
        )
        .expect("write workspace a config");
        std::fs::write(
            workspace_b.join(".devo").join("config.toml"),
            r#"
[model.workspace-b-model]
display_name = "Workspace B"
"#,
        )
        .expect("write workspace b config");

        let context_a = deps
            .context_for_workspace(&workspace_a)
            .await
            .expect("load workspace a context");
        let context_b = deps
            .context_for_workspace(&workspace_b)
            .await
            .expect("load workspace b context");

        assert_eq!(context_a.provider.name(), "noop");
        assert_eq!(context_b.provider.name(), "noop");
        assert_eq!(
            context_a
                .model_catalog
                .get("workspace-a-model")
                .expect("workspace a model")
                .display_name,
            "Workspace A"
        );
        assert_eq!(
            context_b
                .model_catalog
                .get("workspace-b-model")
                .expect("workspace b model")
                .display_name,
            "Workspace B"
        );
        assert!(context_a.model_catalog.get("workspace-b-model").is_none());
        assert!(context_b.model_catalog.get("workspace-a-model").is_none());
        assert!(
            context_a
                .model_catalog
                .get("legacy-only-workspace-model")
                .is_none()
        );
        assert!(
            context_b
                .model_catalog
                .get("legacy-only-workspace-model")
                .is_none()
        );
        assert_eq!(
            std::fs::read(&legacy_models_path).expect("read legacy workspace models"),
            legacy_models
        );
        assert!(!absent_legacy_models_path.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn context_for_workspace_reuses_provider_runtime_for_model_metadata_overrides() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );
        let workspace = unique_temp_dir("session-context-model-metadata");
        std::fs::create_dir_all(workspace.join(".devo")).expect("create workspace config dir");
        std::fs::write(
            workspace.join(".devo").join("config.toml"),
            r#"
[model.catalog-slug]
display_name = "Workspace Catalog Model"
"#,
        )
        .expect("write workspace config");

        let context = deps
            .context_for_workspace(&workspace)
            .await
            .expect("load workspace context");

        assert!(Arc::ptr_eq(
            &context.provider,
            &deps.process_context.provider
        ));
        assert!(Arc::ptr_eq(
            &context.provider_router,
            &deps.process_context.provider_router
        ));
        assert_eq!(
            context
                .model_catalog
                .get("catalog-slug")
                .expect("workspace catalog model")
                .display_name,
            "Workspace Catalog Model"
        );

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn context_for_workspace_rebuilds_provider_when_provider_http_changes() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );
        let workspace = unique_temp_dir("session-context-provider-http");
        std::fs::create_dir_all(workspace.join(".devo")).expect("create workspace config dir");
        std::fs::write(
            workspace.join(".devo").join("config.toml"),
            r#"
[provider_http]
proxy_url = "http://workspace-proxy.example:8080"
"#,
        )
        .expect("write workspace config");

        let context = deps
            .context_for_workspace(&workspace)
            .await
            .expect("load workspace context");

        assert_eq!(context.provider.name(), "openai");

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_turn_config_preserves_catalog_slug_and_uses_binding_request_model() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.other]
enabled = true
name = "Other"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );

        let loaded = deps
            .config_store
            .lock()
            .expect("config store")
            .effective_config()
            .provider
            .providers
            .get("openrouter")
            .and_then(|provider| provider.web_search.as_ref())
            .cloned();
        eprintln!("loaded provider web_search: {loaded:?}");

        let turn_config = deps.resolve_turn_config(
            Some("vendor/model-name"),
            /*reasoning_effort_selection*/ None,
        );

        assert_eq!(turn_config.model.slug, "catalog-slug");
        assert_eq!(turn_config.request_model, "vendor/model-name");
        assert_eq!(
            turn_config.provider_route,
            ProviderRoute::binding("openrouter", ProviderWireApi::OpenAIChatCompletions)
        );
    }

    #[test]
    fn resolve_turn_config_maps_variant_slug_to_binding_request_model() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"

[model_bindings.thinking]
enabled = true
model_slug = "catalog-slug-thinking"
provider = "openrouter"
request_model = "vendor/model-name-thinking"
invocation_method = "openai_chat_completions"

[model_bindings.other-thinking]
enabled = true
model_slug = "catalog-slug-thinking"
provider = "other"
request_model = "other-provider/model-name-thinking"
invocation_method = "openai_chat_completions"
"#,
        );

        let turn_config = deps.resolve_turn_config(Some("catalog-slug"), None);

        assert_eq!(
            turn_config.provider_request_model("catalog-slug-thinking"),
            "vendor/model-name-thinking"
        );
        assert_eq!(
            turn_config.provider_route,
            ProviderRoute::binding("openrouter", ProviderWireApi::OpenAIChatCompletions)
        );
    }

    #[test]
    fn resolve_turn_config_applies_web_search_provider_override() {
        let deps = test_deps(
            r#"
[tools.web_search]
mode = "disabled"

[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.openrouter.web_search]
mode = "provider"

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );

        let turn_config = deps.resolve_turn_config(
            Some("vendor/model-name"),
            /*reasoning_effort_selection*/ None,
        );

        assert_eq!(
            turn_config.web_search,
            devo_core::ResolvedWebSearchConfig::Provider
        );
    }

    #[test]
    fn resolve_turn_config_applies_web_search_binding_override() {
        let deps = test_deps(
            r#"
[tools.web_search]
mode = "disabled"

[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.openrouter.web_search]
mode = "provider"

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
request_model = "vendor/model-name"
invocation_method = "openai_chat_completions"

[model_bindings.main.web_search]
mode = "disabled"
"#,
        );

        let turn_config = deps.resolve_turn_config(
            Some("vendor/model-name"),
            /*reasoning_effort_selection*/ None,
        );

        assert_eq!(
            turn_config.web_search,
            devo_core::ResolvedWebSearchConfig::Disabled
        );
    }
}
