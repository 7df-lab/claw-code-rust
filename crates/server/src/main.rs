use anyhow::Result;
use clap::Parser;
use devo_core::{
    AppConfig, AppConfigLoader, FileSystemAppConfigLoader, LoggingBootstrap, LoggingRuntime,
};
use devo_server::{ServerProcessArgs, run_server_process};
use devo_utils::find_devo_home;

#[tokio::main]
async fn main() -> Result<()> {
    let args = ServerProcessArgs::parse();
    let _logging = install_logging(&args)?;
    run_server_process(args).await
}

fn install_logging(args: &ServerProcessArgs) -> Result<LoggingRuntime> {
    let home_dir = find_devo_home()?;
    let loader = FileSystemAppConfigLoader::new(home_dir.clone());
    let app_config = loader
        .load(args.working_root.as_deref())
        .unwrap_or_else(|err| {
            eprintln!("warning: failed to load app config for logging: {err}");
            AppConfig::default()
        });
    LoggingBootstrap {
        process_name: "server",
        config: app_config.logging,
        home_dir,
    }
    .install()
    .map_err(Into::into)
}
