use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use clap::ValueEnum;
use devo_core::AgentsMdConfig;
use devo_core::AppConfigStore;
use devo_core::FileSystemSkillCatalog;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::tools::ToolPlanConfig;
use devo_core::tools::handlers;
use devo_mcp::manager::RmcpMcpManager;
use devo_util_paths::FileSystemConfigPathResolver;

use crate::ListenTarget;
use crate::ProtocolSet;
use crate::ServerRuntime;
use crate::db::Database;
use crate::execution::ServerRuntimeDependencies;
use crate::load_server_provider;
use crate::resolve_listen_targets;
use crate::run_listeners_with_internal_proxy;
use crate::singleton::ServerControlAction;
use crate::singleton::SingletonRole;
use crate::singleton::acquire_singleton_role;
use crate::singleton::run_server_control;
use crate::singleton::run_stdio_proxy;
use crate::transport::DEFAULT_WEBSOCKET_BIND_ADDRESS;
use crate::transport::InternalProxyControl;
use crate::transport::InternalProxyEndpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServerTransportMode {
    Config,
    Stdio,
    #[value(name = "websocket")]
    WebSocket,
}

/// Command-line arguments accepted by the standalone server process entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "devo-server", version, about)]
pub struct ServerProcessArgs {
    /// Override the transport mode used by this server process.
    #[arg(long, value_enum, hide = true, default_value_t = ServerTransportMode::Config)]
    pub transport: ServerTransportMode,

    /// Protocol adapters exposed by this server process.
    #[arg(long, default_value = "native")]
    pub protocols: ProtocolSet,

    /// Print status for an existing singleton server and exit.
    #[arg(long, hide = true)]
    pub status: bool,

    /// Ask an existing singleton server to shut down and exit.
    #[arg(long, hide = true)]
    pub shutdown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerProcessAction {
    Run,
    Status,
    Shutdown,
}

impl ServerProcessArgs {
    fn action(&self) -> Result<ServerProcessAction> {
        match (self.status, self.shutdown) {
            (false, false) => Ok(ServerProcessAction::Run),
            (true, false) => Ok(ServerProcessAction::Status),
            (false, true) => Ok(ServerProcessAction::Shutdown),
            (true, true) => anyhow::bail!("--status and --shutdown cannot be used together"),
        }
    }
}

#[derive(Default)]
pub struct ServerProcessRunOptions {
    pub external_shutdown: Option<tokio_util::sync::CancellationToken>,
}

/// Starts the transport-facing server runtime using the resolved application
/// configuration and listener set.
///
/// ## Singleton server (`singleton.rs`)
///
/// Devo allows at most **one real server process** per `DEVO_HOME`. Coordination
/// uses a file lock (`server.lock`) plus metadata (`server.lock.json`) that
/// records pid, a loopback WebSocket endpoint, and an auth token.
///
/// - **`SingletonRole::Real`**: this process acquired the lock and becomes the
///   sole server. It binds an internal proxy listener, writes metadata, and runs
///   until shutdown.
/// - **`SingletonRole::Proxy`**: another process already holds the lock. This
///   process does not start a second runtime: stdio mode forwards to the
///   existing server via `run_stdio_proxy`; `--status` / `--shutdown` talk to
///   the internal control channel on that server.
///
/// ## Internal proxy (`run_listeners_with_internal_proxy`)
///
/// The real server exposes an extra **loopback-only** WebSocket listener
/// (`127.0.0.1:0`, ephemeral port). It is used for:
///
/// 1. **Stdio proxy clients** — a second `devo server --transport stdio` connects
///    here and pipes stdin/stdout through WebSocket frames (see `run_stdio_proxy`).
/// 2. **Control plane** — `devo server --status` / `--shutdown` send
///    `server/status` or `server/shutdown` after token auth.
///
/// The published `endpoint` in `server.lock.json` is this internal proxy URL, not
/// the public config WebSocket address.
pub async fn run_server_process(
    args: ServerProcessArgs,
    options: ServerProcessRunOptions,
) -> Result<()> {
    let resolver = FileSystemConfigPathResolver::from_env()?;
    let action = args.action()?;
    // Decide whether this process is the one true server or a lightweight proxy/
    // control client. Lock file lives under DEVO_HOME (see singleton.rs).
    let singleton_role = acquire_singleton_role(&resolver.user_config_dir())?;
    let real_server_guard = match singleton_role {
        SingletonRole::Real(guard) => match action {
            ServerProcessAction::Run => guard,
            ServerProcessAction::Status | ServerProcessAction::Shutdown => {
                // We hold the lock but were not asked to run — no metadata file yet.
                println!("devo server is not running");
                return Ok(());
            }
        },
        SingletonRole::Proxy(metadata) => match action {
            ServerProcessAction::Run => {
                let result = run_server_control(
                    &metadata,
                    ServerControlAction::EnableProtocols(args.protocols.clone()),
                )
                .await?;
                if args.transport == ServerTransportMode::Stdio {
                    tracing::info!(
                        pid = metadata.pid,
                        endpoint = %metadata.endpoint,
                        protocols = %result
                            .enabled_protocols
                            .as_ref()
                            .unwrap_or(&args.protocols),
                        "proxying stdio to existing singleton server"
                    );
                    return run_stdio_proxy(metadata).await;
                }
                print_existing_server_status(
                    &metadata,
                    "already running",
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
            // `--status` / `--shutdown`: one-shot WebSocket control, then exit.
            ServerProcessAction::Status => {
                let result = run_server_control(&metadata, ServerControlAction::Status).await?;
                print_existing_server_status(
                    &metadata,
                    result.status.as_str(),
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
            ServerProcessAction::Shutdown => {
                let result = run_server_control(&metadata, ServerControlAction::Shutdown).await?;
                print_existing_server_status(
                    &metadata,
                    result.status.as_str(),
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
        },
    };
    // Real server: bind ephemeral loopback WS for stdio-proxy + control clients.
    let internal_proxy = InternalProxyEndpoint::bind().await?;
    // Persist ws://127.0.0.1:<port> + random token into server.lock.json so
    // proxy/control processes know where and how to connect.
    let singleton_metadata =
        real_server_guard.publish_endpoint(internal_proxy.endpoint().to_string())?;

    let config_store = Arc::new(std::sync::Mutex::new(AppConfigStore::load(
        resolver.user_config_dir(),
        /*workspace_root*/ None,
    )?));
    let config = config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .clone();
    let effective_listen = match args.transport {
        ServerTransportMode::Config => config.server.listen.clone(),
        ServerTransportMode::Stdio => vec!["stdio://".to_string()],
        ServerTransportMode::WebSocket => {
            vec![format!("ws://{DEFAULT_WEBSOCKET_BIND_ADDRESS}")]
        }
    };
    let listen_targets = resolve_listen_targets(&effective_listen)?;
    let effective_listen = listen_targets
        .iter()
        .map(|target| match target {
            ListenTarget::Stdio => "stdio://".to_string(),
            ListenTarget::WebSocket { bind_address } => format!("ws://{bind_address}"),
        })
        .collect::<Vec<_>>();

    tracing::info!(
        user_config = %resolver.user_config_file().display(),
        configured_listen = ?config.server.listen,
        effective_listen = ?effective_listen,
        max_connections = config.server.max_connections,
        protocols = %args.protocols,
        "loaded server config"
    );

    let mcp_manager: Arc<dyn devo_core::McpManager> = Arc::new(RmcpMcpManager::new(
        config.mcp.clone().with_code_search_workspace_cwd(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        ),
        config.mcp_oauth_credentials_store.unwrap_or_default(),
    ));
    let tool_plan = ToolPlanConfig::from_app_config(&config);
    let registry =
        handlers::build_registry_from_plan_with_mcp(&tool_plan, Arc::clone(&mcp_manager)).await;
    let model_catalog: Arc<dyn ModelCatalog> = Arc::new(PresetModelCatalog::load_from_config(
        &config.provider.model_overrides,
    )?);
    let default_model = model_catalog.resolve_for_turn(None)?.slug.clone();
    if !config.has_provider_configuration() {
        tracing::warn!(
            "No provider configured. Run `devo onboard` to complete setup; continuing with onboarding-capable server"
        );
    }
    let provider = load_server_provider(
        &config,
        Some(default_model.as_str()),
        &resolver.user_config_dir(),
    )?;
    let skill_catalog = Box::new(FileSystemSkillCatalog::with_devo_home(
        config.skills.clone(),
        resolver.user_config_dir(),
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        config.project_root_markers.clone(),
    ));
    // Initialize SQLite database
    let db_path = resolver.user_config_dir().join("devo.db");
    tracing::info!(db_path = %db_path.display(), "opening database");
    let db = Arc::new(Database::open(db_path)?);

    let registry = Arc::new(registry);
    let provider_router = Arc::clone(&provider.provider_router);
    let runtime = ServerRuntime::with_protocols(
        resolver.user_config_dir(),
        ServerRuntimeDependencies::new(
            provider.provider,
            provider_router,
            Arc::clone(&registry),
            mcp_manager,
            provider.default_model,
            model_catalog,
            Arc::new(ProviderVendorCatalog::default()),
            skill_catalog,
            AgentsMdConfig {
                project_root_markers: config.project_root_markers.clone(),
                ..AgentsMdConfig::default()
            },
            db,
            config_store,
        ),
        args.protocols.clone(),
    );
    runtime
        .run_global_hook(
            devo_core::HookEvent::Setup,
            serde_json::Map::from_iter([("trigger".to_string(), serde_json::json!("init"))]),
        )
        .await;
    if runtime.backfill_session_index_if_required()? {
        tracing::info!("rollout metadata index backfill completed");
    }
    // Delivery-log reconciliation (08 §7): backfill event_log rows a crash
    // prevented the append path from writing. Runs in the background;
    // session/list correctness never depends on it.
    {
        let rollout_store = runtime.rollout_store();
        let db = runtime.deps_db();
        tokio::task::spawn_blocking(move || {
            match crate::event_reconcile::reconcile_event_log(&rollout_store, &db) {
                Ok(stats) => {
                    if stats.rows_inserted > 0 || stats.files_damaged > 0 {
                        tracing::info!(
                            rows_inserted = stats.rows_inserted,
                            files_damaged = stats.files_damaged,
                            "event_log reconciliation completed"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "event_log reconciliation failed");
                }
            }
        });
    }

    let shutdown_signal = tokio_util::sync::CancellationToken::new();
    let internal_proxy_control = InternalProxyControl::new(shutdown_signal.clone());
    let external_shutdown = options.external_shutdown.clone();

    // Concurrent listeners: configured stdio/ws targets + internal proxy task.
    // Returns when any listener exits; shutdown also via Ctrl+C, external token,
    // or internal-proxy `server/shutdown` (cancels shutdown_signal).
    tokio::select! {
        result = run_listeners_with_internal_proxy(
            runtime.clone(),
            &effective_listen,
            internal_proxy,
            singleton_metadata.token.clone(),
            internal_proxy_control,
        ) => {
            result?;
        }
        result = tokio::signal::ctrl_c() => {
            result?;
            tracing::info!("server shutdown requested");
        }
        _ = wait_for_external_shutdown(external_shutdown.as_ref()) => {
            tracing::info!("server shutdown requested from external process controller");
        }
        _ = shutdown_signal.cancelled() => {
            tracing::info!("server shutdown requested from singleton control");
        }
    }

    tracing::info!("terminating unified exec processes");
    registry.terminate_unified_exec_processes().await;
    tracing::info!("completing deferred items for active turns");
    runtime.shutdown().await;
    Ok(())
}

fn print_existing_server_status(
    metadata: &crate::singleton::ServerLockMetadata,
    status: &str,
    enabled_protocols: Option<&ProtocolSet>,
) {
    println!("devo server {status}");
    println!("pid: {}", metadata.pid);
    println!("endpoint: {}", metadata.endpoint);
    println!("started_at: {}", metadata.started_at);
    if let Some(enabled_protocols) = enabled_protocols {
        println!("protocols: {enabled_protocols}");
    }
}

async fn wait_for_external_shutdown(
    external_shutdown: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Some(token) = external_shutdown {
        token.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ServerProcessArgs;
    use super::ServerTransportMode;
    use crate::ProtocolSet;
    use clap::Parser;

    #[test]
    fn server_process_args_default_to_config_transport() {
        let args = ServerProcessArgs::parse_from(["devo-server"]);

        assert_eq!(args.transport, ServerTransportMode::Config);
        assert_eq!(args.protocols, ProtocolSet::default());
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_protocol_sets() {
        let args =
            ServerProcessArgs::parse_from(["devo-server", "--protocols", "native, acp,native"]);

        assert_eq!(args.protocols.names(), vec!["native", "acp"]);
    }

    #[test]
    fn server_process_args_accept_stdio_transport_override() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--transport", "stdio"]);

        assert_eq!(args.transport, ServerTransportMode::Stdio);
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_websocket_transport_override() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--transport", "websocket"]);

        assert_eq!(args.transport, ServerTransportMode::WebSocket);
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_status_action() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--status"]);

        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Status
        );
    }

    #[test]
    fn server_process_args_accept_shutdown_action() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--shutdown"]);

        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Shutdown
        );
    }

    #[test]
    fn server_process_args_reject_conflicting_actions() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--status", "--shutdown"]);

        assert_eq!(
            args.action().expect_err("conflicting actions").to_string(),
            "--status and --shutdown cannot be used together"
        );
    }

    #[test]
    fn server_process_args_reject_working_root() {
        let error = ServerProcessArgs::try_parse_from(["devo-server", "--working-root", "."])
            .expect_err("working root is no longer a server bootstrap parameter");

        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
