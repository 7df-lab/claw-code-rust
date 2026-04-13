use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use clawcr_core::{AppConfigLoader, FileSystemAppConfigLoader, ModelCatalog, PresetModelCatalog};
use clawcr_tools::ToolRegistry;
use clawcr_utils::FileSystemConfigPathResolver;

use crate::{
    ListenTarget, ServerRuntime, execution::ServerRuntimeDependencies, load_server_provider,
    resolve_listen_targets, run_listeners,
};

/// Command-line arguments accepted by the standalone server process entrypoint.
#[derive(Debug, Clone, Parser)]
#[command(name = "clawcr-server", version, about)]
pub struct ServerProcessArgs {
    /// Optional workspace root used for project-level config resolution.
    #[arg(long)]
    pub working_root: Option<PathBuf>,
}

/// Starts the transport-facing server runtime using the resolved application
/// configuration and listener set.
pub async fn run_server_process(args: ServerProcessArgs) -> Result<()> {
    let resolver = FileSystemConfigPathResolver::from_env()?;
    let loader = FileSystemAppConfigLoader::new(resolver.user_config_dir());
    let config = loader.load(args.working_root.as_deref())?;
    let listen_targets = resolve_listen_targets(&config.server.listen)?;
    let effective_listen = listen_targets
        .iter()
        .map(|target| match target {
            ListenTarget::Stdio => "stdio://".to_string(),
            ListenTarget::WebSocket { bind_address } => format!("ws://{bind_address}"),
        })
        .collect::<Vec<_>>();

    tracing::info!(
        user_config = %resolver.user_config_file().display(),
        project_config = args
            .working_root
            .as_deref()
            .map(|root| resolver.project_config_file(root).display().to_string())
            .unwrap_or_else(|| "<none>".into()),
        configured_listen = ?config.server.listen,
        effective_listen = ?effective_listen,
        max_connections = config.server.max_connections,
        "loaded server config"
    );

    let mut registry = ToolRegistry::new();
    clawcr_tools::register_builtin_tools(&mut registry);
    let provider = load_server_provider(&resolver.user_config_file(), None)?;
    let model_catalog: Arc<dyn ModelCatalog> = Arc::new(PresetModelCatalog::load()?);
    let runtime = ServerRuntime::new(
        resolver.user_config_dir(),
        ServerRuntimeDependencies::new(
            provider.provider,
            Arc::new(registry),
            provider.default_model,
            model_catalog,
        ),
    );
    tracing::info!("starting persisted session restore");
    runtime.load_persisted_sessions().await?;
    tracing::info!("persisted session restore completed");
    tracing::info!("server bootstrap completed; starting listeners");
    tokio::select! {
        result = run_listeners(runtime, &config.server.listen) => {
            result?;
        }
        result = tokio::signal::ctrl_c() => {
            result?;
            tracing::info!("server shutdown requested");
        }
    }
    Ok(())
}
