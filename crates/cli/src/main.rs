use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use clap::builder::PossibleValuesParser;
use clap::builder::TypedValueParser as _;
use devo_core::AppConfig;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_core::LoggingBootstrap;
use devo_core::LoggingRuntime;
use devo_server::ServerProcessArgs;
use devo_server::run_server_process;
use devo_utils::find_devo_home;
use tracing_subscriber::filter::LevelFilter;

mod agent_command;
mod doctor_command;
mod prompt_command;

use agent_command::run_agent;
use doctor_command::run_doctor;
use prompt_command::run_prompt;

/// Top-level `devo` command that dispatches to interactive agent mode or one
/// of the supporting runtime subcommands.
#[derive(Debug, Parser)]
#[command(name = "devo", version, about = "Devo CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Override the model used for this session.
    #[arg(long, global = true)]
    model: Option<String>,

    /// Override the logging level for this process.
    #[arg(
        long = "log-level",
        global = true,
        value_parser = PossibleValuesParser::new(["trace", "debug", "info", "warn", "error"])
            .try_map(|level| level.parse::<LevelFilter>())
    )]
    log_level: Option<LevelFilter>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // Resolve logging config early, install the process-wide file subscriber,
    // and keep its non-blocking writer guard alive for the command lifetime.
    let _logging = install_logging(&cli)?;
    let log_level = cli.log_level.map(|level| level.to_string());

    match cli.command {
        Some(Command::Server(args)) => run_server_process(args).await,
        Some(Command::Onboard) => {
            run_agent(/*force_onboarding*/ true, log_level.as_deref()).await
        }
        Some(Command::Prompt { input }) => {
            run_prompt(&input, cli.model.as_deref(), log_level.as_deref()).await
        }
        Some(Command::Doctor) => run_doctor().await,
        None => {
            run_agent(/*force_onboarding*/ false, log_level.as_deref()).await
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Launch the interactive onboarding flow to configure a model provider.
    Onboard,
    /// Start the transport-facing server runtime.
    Server(ServerProcessArgs),
    /// Send a single prompt to the model and print the response (non-interactive).
    Prompt {
        /// The prompt text to send to the model.
        input: String,
    },
    /// Diagnose configuration, provider connectivity, and system health.
    Doctor,
}

fn install_logging(cli: &Cli) -> Result<LoggingRuntime> {
    let home_dir = find_devo_home()?;
    let loader = FileSystemAppConfigLoader::new(home_dir.clone())
        .with_cli_overrides(cli_logging_overrides(cli));
    let current_dir = std::env::current_dir()?;
    let workspace_root = match &cli.command {
        Some(Command::Server(args)) => args.working_root.as_deref(),
        _ => Some(current_dir.as_path()),
    };
    let app_config = loader.load(workspace_root).unwrap_or_else(|err| {
        eprintln!("warning: failed to load app config for logging: {err}");
        AppConfig::default()
    });
    LoggingBootstrap {
        process_name: logging_process_name(&cli.command),
        config: app_config.logging,
        home_dir,
    }
    .install()
    .map_err(Into::into)
}

fn cli_logging_overrides(cli: &Cli) -> toml::Value {
    let Some(log_level) = cli.log_level else {
        return toml::Value::Table(Default::default());
    };

    toml::Value::Table(toml::map::Map::from_iter([(
        "logging".to_string(),
        toml::Value::Table(toml::map::Map::from_iter([(
            "level".to_string(),
            toml::Value::String(log_level.to_string()),
        )])),
    )]))
}

fn logging_process_name(command: &Option<Command>) -> &'static str {
    match command {
        Some(Command::Onboard) => "onboard",
        Some(Command::Server(_)) => "server",
        Some(Command::Prompt { .. }) => "prompt",
        Some(Command::Doctor) => "doctor",
        None => "cli",
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use pretty_assertions::assert_eq;
    use tracing_subscriber::filter::LevelFilter;

    use super::Cli;
    use super::Command;
    use super::ServerProcessArgs;
    use super::cli_logging_overrides;
    use super::logging_process_name;

    #[test]
    fn logging_process_name_exhausted_subcommand() {
        assert_eq!(logging_process_name(&None), "cli");
        assert_eq!(logging_process_name(&Some(Command::Onboard)), "onboard");
        assert_eq!(
            logging_process_name(&Some(Command::Server(ServerProcessArgs {
                working_root: None,
            }))),
            "server"
        );
        assert_eq!(
            logging_process_name(&Some(Command::Prompt {
                input: "".to_string()
            })),
            "prompt"
        );
        assert_eq!(logging_process_name(&Some(Command::Doctor)), "doctor");
    }

    #[test]
    fn cli_parses_supported_log_levels() {
        for (level, expected) in [
            ("trace", LevelFilter::TRACE),
            ("debug", LevelFilter::DEBUG),
            ("info", LevelFilter::INFO),
            ("warn", LevelFilter::WARN),
            ("error", LevelFilter::ERROR),
        ] {
            let cli = Cli::try_parse_from(["devo", "--log-level", level]).expect("parse log level");

            assert!(cli.command.is_none());
            assert_eq!(cli.model, None);
            assert_eq!(cli.log_level, Some(expected));
        }
    }

    #[test]
    fn cli_rejects_unsupported_log_levels() {
        let err = Cli::try_parse_from(["devo", "--log-level", "off"]).expect_err("reject off");

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn cli_logging_overrides_is_empty_without_log_level() {
        let cli = Cli {
            command: None,
            model: None,
            log_level: None,
        };

        assert_eq!(
            cli_logging_overrides(&cli),
            toml::Value::Table(Default::default())
        );
    }

    #[test]
    fn cli_logging_overrides_sets_logging_level() {
        for (level, expected) in [
            (LevelFilter::TRACE, "trace"),
            (LevelFilter::DEBUG, "debug"),
            (LevelFilter::INFO, "info"),
            (LevelFilter::WARN, "warn"),
            (LevelFilter::ERROR, "error"),
        ] {
            let cli = Cli {
                command: None,
                model: None,
                log_level: Some(level),
            };

            assert_eq!(
                cli_logging_overrides(&cli),
                toml::Value::Table(toml::map::Map::from_iter([(
                    "logging".to_string(),
                    toml::Value::Table(toml::map::Map::from_iter([(
                        "level".to_string(),
                        toml::Value::String(expected.to_string()),
                    )])),
                )]))
            );
        }
    }
}
