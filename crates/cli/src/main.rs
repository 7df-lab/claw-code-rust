//! User-facing CLI entrypoint for interactive, prompt, diagnostic, upgrade,
//! and hidden server process modes.
//!
//! This binary owns command-line parsing, startup update checks, logging
//! bootstrap, and final exit messages. Long-lived runtime behavior is delegated
//! to the server, TUI, and core crates so CLI changes stay focused on process
//! orchestration and display.

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
use devo_core::SessionId;
use devo_core::UpdateCheckOutcome;
use devo_core::UpdateChecker;
use devo_core::format_update_notification;
use devo_server::ServerProcessArgs;
use devo_server::ServerProcessRunOptions;
use devo_server::ServerTransportMode;
use devo_server::run_server_process;
use devo_util_paths::find_devo_home;
use tracing_subscriber::filter::LevelFilter;

mod agent_command;
mod doctor_command;
mod mcp_command;
mod prompt_command;
mod upgrade_command;

use agent_command::run_agent;
use doctor_command::run_doctor;
use mcp_command::McpCommand;
use mcp_command::run_mcp;
use prompt_command::PromptOutputFormat;
use prompt_command::run_prompt;
use upgrade_command::run_upgrade;

/// Top-level `devo` command that dispatches to interactive agent mode or one
/// of the supporting runtime subcommands.
///
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

    /// Start with full-access permissions, skipping approval prompts.
    #[arg(
        long = "dangerously-skip-permissions",
        visible_alias = "yolo",
        global = true
    )]
    dangerously_skip_permissions: bool,
}

fn main() -> Result<()> {
    devo_arg0::run_as_with_early_dispatch(
        |_paths| async {
            let result = run_cli().await;
            tracing::info!(success = result.is_ok(), "run_cli future completed");
            result
        },
        |_paths| direct_server_early_dispatch(),
    )
}

fn format_with_separators(value: usize) -> String {
    let digits = value.to_string();
    let separator_count = digits.len().saturating_sub(1) / 3;
    let first_group_len = digits.len() - separator_count * 3;
    let mut out = String::with_capacity(digits.len() + separator_count);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && index >= first_group_len && (index - first_group_len).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn format_token_usage_line(exit: &devo_tui::AppExit, color_enabled: bool) -> Option<String> {
    let total = exit.total_tokens;
    let non_cached_input = exit
        .total_input_tokens
        .saturating_sub(exit.total_cache_read_tokens);
    if total == 0 && exit.total_cache_read_tokens == 0 {
        return None;
    }
    let total_value = format_with_separators(total);
    let input_value = format_with_separators(non_cached_input);
    let output_value = format_with_separators(exit.total_output_tokens);
    let cached_suffix = if exit.total_cache_read_tokens > 0 {
        let cached_value = format_with_separators(exit.total_cache_read_tokens);
        if color_enabled {
            format!(" (+ \u{1b}[1;33m{cached_value}\u{1b}[0m \u{1b}[33mcached\u{1b}[0m)")
        } else {
            format!(" (+ {cached_value} cached)")
        }
    } else {
        String::new()
    };
    Some(format!(
        "Token usage: total={} input={}{} output={}",
        if color_enabled {
            format!("\u{1b}[1;36m{total_value}\u{1b}[0m")
        } else {
            total_value
        },
        if color_enabled {
            format!("\u{1b}[1;32m{input_value}\u{1b}[0m")
        } else {
            input_value
        },
        cached_suffix,
        if color_enabled {
            format!("\u{1b}[1;35m{output_value}\u{1b}[0m")
        } else {
            output_value
        },
    ))
}

fn exit_messages(exit: &devo_tui::AppExit, color_enabled: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(line) = format_token_usage_line(exit, color_enabled) {
        lines.push(line);
    }
    if let Some(session_id) = exit.session_id {
        let command = format!("devo resume {session_id}");
        let command = if color_enabled {
            format!("\u{1b}[1;36m{command}\u{1b}[0m")
        } else {
            command
        };
        let prefix = if color_enabled {
            "\u{1b}[2mTo continue this session, run\u{1b}[0m"
        } else {
            "To continue this session, run"
        };
        lines.push(format!("{prefix} {command}"));
    }
    lines
}

fn onboarding_exit_messages(exit: &devo_tui::AppExit, color_enabled: bool) -> Vec<String> {
    if !exit.onboarding_completed {
        return Vec::new();
    }
    let complete = if color_enabled {
        "\u{1b}[1;32mConfiguration complete\u{1b}[0m".to_string()
    } else {
        "Configuration complete".to_string()
    };
    let command = if color_enabled {
        "\u{1b}[1;36mdevo\u{1b}[0m".to_string()
    } else {
        "devo".to_string()
    };
    vec![
        complete,
        String::new(),
        "Next step:".to_string(),
        format!("  {command}"),
    ]
}

async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let log_level = cli.log_level.map(|level| level.to_string());

    match &cli.command {
        Some(Command::Onboard) => {
            // Resolve logging config early, install the process-wide file subscriber,
            // and keep its non-blocking writer guard alive for the command lifetime.
            let _logging = install_logging(&cli)?;
            let exit = run_agent(
                /*force_onboarding*/ true,
                /*exit_after_onboarding*/ true,
                log_level.as_deref(),
                None,
                cli.dangerously_skip_permissions,
            )
            .await?;
            for line in onboarding_exit_messages(&exit, /*color_enabled*/ true) {
                println!("{line}");
            }
            Ok(())
        }
        Some(Command::Prompt { input, format }) => {
            maybe_print_startup_update(&cli).await;
            let _logging = install_logging(&cli)?;
            run_prompt(input, cli.model.as_deref(), log_level.as_deref(), *format).await
        }
        Some(Command::Doctor) => {
            let _logging = install_logging(&cli)?;
            run_doctor().await
        }
        Some(Command::Mcp { command }) => {
            let _logging = install_logging(&cli)?;
            run_mcp(command)
        }
        Some(Command::Upgrade) => run_upgrade(),
        Some(Command::Resume { session_id }) => {
            maybe_print_startup_update(&cli).await;
            let _logging = install_logging(&cli)?;
            let exit = run_agent(
                /*force_onboarding*/ false,
                /*exit_after_onboarding*/ false,
                log_level.as_deref(),
                Some(*session_id),
                cli.dangerously_skip_permissions,
            )
            .await?;
            for line in exit_messages(&exit, /*color_enabled*/ true) {
                println!("{line}");
            }
            Ok(())
        }
        Some(Command::Server {
            transport: _,
            status: _,
            shutdown: _,
        }) => {
            // Start tokio-console before file logging so the console subscriber
            // can capture task instrumentation. File logging will fall back
            // gracefully if a subscriber is already installed.
            devo_arg0::maybe_init_tokio_console();
            let args = server_process_args_from_cli(&cli).expect("server command args");
            let _logging = install_server_logging(&cli)?;
            run_server_process(args, ServerProcessRunOptions::default()).await
        }
        None => {
            maybe_print_startup_update(&cli).await;
            let _logging = install_logging(&cli)?;
            tracing::info!("default interactive command starting");
            let exit = run_agent(
                /*force_onboarding*/ false,
                /*exit_after_onboarding*/ false,
                log_level.as_deref(),
                None,
                cli.dangerously_skip_permissions,
            )
            .await?;
            let exit_lines = exit_messages(&exit, /*color_enabled*/ true);
            tracing::info!(
                line_count = exit_lines.len(),
                "printing default interactive exit messages"
            );
            for line in exit_lines {
                println!("{line}");
            }
            tracing::info!("default interactive command completed");
            Ok(())
        }
    }
}

fn direct_server_early_dispatch() -> devo_arg0::EarlyDispatch {
    let args: Vec<String> = std::env::args().collect();
    match devo_windows_sandbox::run_as_windows_sandbox_if_requested(&args) {
        Ok(true) => {
            // The helper exits the process on success; reaching here is unexpected.
            devo_arg0::EarlyDispatch::Handled(Ok(()))
        }
        Ok(false) => devo_arg0::EarlyDispatch::Continue,
        Err(error) => {
            eprintln!("windows sandbox wrapper failed: {error}");
            std::process::exit(1);
        }
    }
}

fn server_process_args_from_cli(cli: &Cli) -> Option<ServerProcessArgs> {
    match &cli.command {
        Some(Command::Server {
            transport,
            status,
            shutdown,
        }) => Some(ServerProcessArgs {
            transport: *transport,
            status: *status,
            shutdown: *shutdown,
        }),
        Some(Command::Onboard)
        | Some(Command::Resume { .. })
        | Some(Command::Prompt { .. })
        | Some(Command::Doctor)
        | Some(Command::Mcp { .. })
        | Some(Command::Upgrade)
        | None => None,
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Launch the interactive onboarding flow to configure a model provider.
    Onboard,
    /// Resume a saved interactive session by id.
    Resume {
        /// Session identifier printed by Devo at exit time.
        session_id: SessionId,
    },
    /// Send a single prompt to the model and print the response (non-interactive).
    Prompt {
        /// Output format for non-interactive prompt execution.
        #[arg(long, value_enum, default_value_t = PromptOutputFormat::Text)]
        format: PromptOutputFormat,
        /// The prompt text to send to the model.
        input: String,
    },
    /// Diagnose configuration, provider connectivity, and system health.
    Doctor,
    /// Manage MCP server entries in the user config.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Upgrade Devo to the latest released version.
    Upgrade,
    /// Start the runtime server process.
    #[command(hide = true)]
    Server {
        /// Override the transport mode used by this server process.
        #[arg(long, value_enum, hide = true, default_value_t = ServerTransportMode::Config)]
        transport: ServerTransportMode,
        /// Print status for an existing singleton server and exit.
        #[arg(long, hide = true)]
        status: bool,
        /// Ask an existing singleton server to shut down and exit.
        #[arg(long, hide = true)]
        shutdown: bool,
    },
}

async fn maybe_print_startup_update(cli: &Cli) {
    let Ok(home_dir) = find_devo_home() else {
        return;
    };
    let app_config = FileSystemAppConfigLoader::new(home_dir.clone())
        .with_cli_overrides(cli_logging_overrides(cli))
        .load(Some(
            std::env::current_dir()
                .ok()
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(".")),
        ))
        .unwrap_or_else(|_| AppConfig::default());
    let Ok(checker) = UpdateChecker::new(home_dir, app_config.updates) else {
        return;
    };

    if let UpdateCheckOutcome::UpdateAvailable(notification) =
        checker.check_for_startup_update().await
    {
        eprintln!("{}", format_update_notification(&notification));
    }
}

fn install_logging(cli: &Cli) -> Result<LoggingRuntime> {
    let home_dir = find_devo_home()?;
    let app_config = devo_core::FileSystemAppConfigLoader::new(home_dir.clone())
        .with_cli_overrides(cli_logging_overrides(cli))
        .load(Some(std::env::current_dir()?.as_path()))
        .unwrap_or_else(|err| {
            eprintln!("warning: failed to load app config for logging: {err}");
            devo_core::AppConfig::default()
        });
    LoggingBootstrap {
        process_name: "cli",
        config: app_config.logging,
        home_dir,
    }
    .install()
    .map_err(Into::into)
}

fn install_server_logging(cli: &Cli) -> Result<LoggingRuntime> {
    let home_dir = find_devo_home()?;
    let loader = devo_core::FileSystemAppConfigLoader::new(home_dir.clone())
        .with_cli_overrides(cli_logging_overrides(cli));
    let app_config = loader.load(/*workspace_root*/ None).unwrap_or_else(|err| {
        eprintln!("warning: failed to load app config for logging: {err}");
        devo_core::AppConfig::default()
    });
    LoggingBootstrap {
        process_name: "server",
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

#[cfg(test)]
mod tests {
    use clap::Parser;
    use devo_core::SessionId;
    use pretty_assertions::assert_eq;
    use tracing_subscriber::filter::LevelFilter;

    use super::Cli;
    use super::Command;
    use super::McpCommand;
    use super::PromptOutputFormat;
    use super::cli_logging_overrides;
    use super::exit_messages;
    use super::format_token_usage_line;
    use super::mcp_command::McpTransportKind;
    use super::onboarding_exit_messages;

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
            assert_eq!(cli.log_level, Some(expected));
        }
    }

    #[test]
    fn cli_parses_dangerously_skip_permissions_flag() {
        let cli = Cli::try_parse_from(["devo", "--dangerously-skip-permissions"])
            .expect("parse dangerously-skip-permissions");

        assert!(cli.command.is_none());
        assert!(cli.dangerously_skip_permissions);
    }

    #[test]
    fn cli_parses_yolo_alias_for_dangerously_skip_permissions() {
        let cli = Cli::try_parse_from(["devo", "--yolo"]).expect("parse yolo");

        assert!(cli.command.is_none());
        assert!(cli.dangerously_skip_permissions);
    }

    #[test]
    fn cli_parses_yolo_alias_on_resume_subcommand() {
        let session_id = SessionId::new();
        let cli = Cli::try_parse_from(["devo", "resume", &session_id.to_string(), "--yolo"])
            .expect("parse resume with yolo");

        assert!(matches!(cli.command, Some(Command::Resume { .. })));
        assert!(cli.dangerously_skip_permissions);
    }

    #[test]
    fn cli_rejects_unsupported_log_levels() {
        let err = Cli::try_parse_from(["devo", "--log-level", "off"]).expect_err("reject off");

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
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
                dangerously_skip_permissions: false,
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

    #[test]
    fn startup_update_check_scope_covers_expected_user_facing_commands() {
        for cli in [
            Cli {
                command: None,
                model: None,
                log_level: None,
                dangerously_skip_permissions: false,
            },
            Cli {
                command: Some(Command::Onboard),
                model: None,
                log_level: None,
                dangerously_skip_permissions: false,
            },
            Cli {
                command: Some(Command::Prompt {
                    input: "hello".to_string(),
                    format: PromptOutputFormat::Text,
                }),
                model: None,
                log_level: None,
                dangerously_skip_permissions: false,
            },
        ] {
            assert_eq!(
                matches!(
                    cli.command,
                    None | Some(Command::Onboard) | Some(Command::Prompt { .. })
                ),
                true
            );
        }
    }

    #[test]
    fn startup_update_check_scope_skips_server_and_doctor() {
        let doctor = Cli {
            command: Some(Command::Doctor),
            model: None,
            log_level: None,
            dangerously_skip_permissions: false,
        };
        let server = Cli {
            command: Some(Command::Server {
                transport: devo_server::ServerTransportMode::Config,
                status: false,
                shutdown: false,
            }),
            model: None,
            log_level: None,
            dangerously_skip_permissions: false,
        };

        assert_eq!(
            matches!(
                doctor.command,
                None | Some(Command::Onboard) | Some(Command::Prompt { .. })
            ),
            false
        );
        assert_eq!(
            matches!(
                server.command,
                None | Some(Command::Onboard) | Some(Command::Prompt { .. })
            ),
            false
        );
    }

    #[test]
    fn cli_parses_resume_subcommand() {
        let session_id = SessionId::new();
        let cli =
            Cli::try_parse_from(["devo", "resume", &session_id.to_string()]).expect("parse resume");

        match cli.command {
            Some(Command::Resume { session_id: actual }) => assert_eq!(actual, session_id),
            other => panic!("expected resume command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_prompt_jsonl_output_format() {
        let cli =
            Cli::try_parse_from(["devo", "prompt", "--format", "jsonl", "hello"]).expect("parse");

        match cli.command {
            Some(Command::Prompt { input, format }) => {
                assert_eq!(input, "hello");
                assert_eq!(format, PromptOutputFormat::Jsonl);
            }
            other => panic!("expected prompt command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_upgrade_subcommand() {
        let cli = Cli::try_parse_from(["devo", "upgrade"]).expect("parse upgrade");

        match cli.command {
            Some(Command::Upgrade) => {}
            other => panic!("expected upgrade command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_add_stdio_trailing_command() {
        let cli = Cli::try_parse_from([
            "devo", "mcp", "add", "time", "--", "docker", "run", "-i", "mcp/time",
        ])
        .expect("parse mcp add stdio");

        match cli.command {
            Some(Command::Mcp {
                command:
                    McpCommand::Add {
                        name,
                        transport,
                        rest,
                        ..
                    },
            }) => {
                assert_eq!(name, "time");
                assert_eq!(transport, McpTransportKind::Stdio);
                assert_eq!(
                    rest,
                    vec![
                        "docker".to_string(),
                        "run".to_string(),
                        "-i".to_string(),
                        "mcp/time".to_string(),
                    ]
                );
            }
            other => panic!("expected mcp add, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_add_http_and_sse() {
        let http = Cli::try_parse_from([
            "devo",
            "mcp",
            "add",
            "--transport",
            "http",
            "hello",
            "http://localhost:8080/mcp",
        ])
        .expect("parse mcp add http");
        match http.command {
            Some(Command::Mcp {
                command:
                    McpCommand::Add {
                        name,
                        transport,
                        rest,
                        ..
                    },
            }) => {
                assert_eq!(name, "hello");
                assert_eq!(transport, McpTransportKind::Http);
                assert_eq!(rest, vec!["http://localhost:8080/mcp".to_string()]);
            }
            other => panic!("expected mcp add http, got {other:?}"),
        }

        let sse = Cli::try_parse_from([
            "devo",
            "mcp",
            "add",
            "--transport",
            "sse",
            "legacy",
            "https://example.com/mcp/sse",
        ])
        .expect("parse mcp add sse");
        match sse.command {
            Some(Command::Mcp {
                command:
                    McpCommand::Add {
                        name,
                        transport,
                        rest,
                        ..
                    },
            }) => {
                assert_eq!(name, "legacy");
                assert_eq!(transport, McpTransportKind::Sse);
                assert_eq!(rest, vec!["https://example.com/mcp/sse".to_string()]);
            }
            other => panic!("expected mcp add sse, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_list_remove_enable_disable() {
        for (args, expected) in [
            (vec!["devo", "mcp", "list"], "list"),
            (vec!["devo", "mcp", "remove", "time"], "remove"),
            (vec!["devo", "mcp", "enable", "time"], "enable"),
            (vec!["devo", "mcp", "disable", "time"], "disable"),
        ] {
            let cli = Cli::try_parse_from(args).expect("parse mcp management");
            match (expected, cli.command) {
                (
                    "list",
                    Some(Command::Mcp {
                        command: McpCommand::List,
                    }),
                ) => {}
                (
                    "remove",
                    Some(Command::Mcp {
                        command: McpCommand::Remove { name },
                    }),
                ) => assert_eq!(name, "time"),
                (
                    "enable",
                    Some(Command::Mcp {
                        command: McpCommand::Enable { name },
                    }),
                ) => assert_eq!(name, "time"),
                (
                    "disable",
                    Some(Command::Mcp {
                        command: McpCommand::Disable { name },
                    }),
                ) => assert_eq!(name, "time"),
                (label, other) => panic!("expected {label}, got {other:?}"),
            }
        }
    }

    #[test]
    fn cli_parses_server_status_and_shutdown_flags() {
        let status = Cli::try_parse_from(["devo", "server", "--status"]).expect("parse status");
        let shutdown =
            Cli::try_parse_from(["devo", "server", "--shutdown"]).expect("parse shutdown");

        match status.command {
            Some(Command::Server {
                transport,
                status,
                shutdown,
            }) => {
                assert_eq!(transport, devo_server::ServerTransportMode::Config);
                assert_eq!([status, shutdown], [true, false]);
            }
            other => panic!("expected server command, got {other:?}"),
        }
        match shutdown.command {
            Some(Command::Server {
                transport,
                status,
                shutdown,
            }) => {
                assert_eq!(transport, devo_server::ServerTransportMode::Config);
                assert_eq!([status, shutdown], [false, true]);
            }
            other => panic!("expected server command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_websocket_server_transport_override() {
        let cli = Cli::try_parse_from(["devo", "server", "--transport", "websocket"])
            .expect("parse websocket server transport");

        match cli.command {
            Some(Command::Server { transport, .. }) => {
                assert_eq!(transport, devo_server::ServerTransportMode::WebSocket);
            }
            other => panic!("expected server command, got {other:?}"),
        }
    }

    #[test]
    fn server_process_args_from_cli_extracts_stdio_server_command() {
        let cli =
            Cli::try_parse_from(["devo", "server", "--transport", "stdio"]).expect("parse server");

        assert_eq!(
            super::server_process_args_from_cli(&cli),
            Some(devo_server::ServerProcessArgs {
                transport: devo_server::ServerTransportMode::Stdio,
                status: false,
                shutdown: false,
            })
        );
    }

    #[test]
    fn server_process_args_from_cli_preserves_global_log_level_parse() {
        let cli = Cli::try_parse_from(["devo", "--log-level", "debug", "server", "--status"])
            .expect("parse server");

        assert_eq!(cli.log_level, Some(LevelFilter::DEBUG));
        assert_eq!(
            super::server_process_args_from_cli(&cli),
            Some(devo_server::ServerProcessArgs {
                transport: devo_server::ServerTransportMode::Config,
                status: true,
                shutdown: false,
            })
        );
    }

    #[test]
    fn server_process_args_from_cli_skips_non_server_command() {
        let cli = Cli::try_parse_from(["devo", "doctor"]).expect("parse doctor");

        assert_eq!(super::server_process_args_from_cli(&cli), None);
    }

    #[test]
    fn exit_messages_includes_usage_and_resume_hint() {
        let session_id = SessionId::new();
        let exit = devo_tui::AppExit {
            session_id: Some(session_id),
            onboarding_completed: false,
            turn_count: 1,
            total_input_tokens: 10,
            total_output_tokens: 2,
            total_tokens: 12,
            total_cache_read_tokens: 5,
        };

        let lines = exit_messages(&exit, /*color_enabled*/ false);
        assert_eq!(
            lines[0],
            "Token usage: total=12 input=5 (+ 5 cached) output=2"
        );
        assert_eq!(
            lines[1],
            format!("To continue this session, run devo resume {session_id}")
        );
    }

    #[test]
    fn colorized_exit_messages_include_ansi_sequences() {
        let session_id = SessionId::new();
        let exit = devo_tui::AppExit {
            session_id: Some(session_id),
            onboarding_completed: false,
            turn_count: 1,
            total_input_tokens: 10,
            total_output_tokens: 2,
            total_tokens: 12,
            total_cache_read_tokens: 5,
        };

        let usage = format_token_usage_line(&exit, /*color_enabled*/ true).expect("usage line");
        assert!(usage.contains("\u{1b}["));

        let lines = exit_messages(&exit, /*color_enabled*/ true);
        assert!(lines[1].contains("\u{1b}["));
    }

    #[test]
    fn exit_usage_uses_accumulated_display_total() {
        let exit = devo_tui::AppExit {
            session_id: Some(SessionId::new()),
            onboarding_completed: false,
            turn_count: 1,
            total_input_tokens: 10,
            total_output_tokens: 2,
            total_tokens: 25,
            total_cache_read_tokens: 0,
        };

        assert_eq!(
            format_token_usage_line(&exit, /*color_enabled*/ false),
            Some("Token usage: total=25 input=10 output=2".to_string())
        );
    }

    #[test]
    fn onboarding_exit_messages_include_next_step_after_success() {
        let session_id = SessionId::new();
        let exit = devo_tui::AppExit {
            session_id: Some(session_id),
            onboarding_completed: true,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_read_tokens: 0,
        };

        let lines = onboarding_exit_messages(&exit, /*color_enabled*/ false);

        assert_eq!(
            lines,
            vec![
                "Configuration complete".to_string(),
                String::new(),
                "Next step:".to_string(),
                "  devo".to_string(),
            ]
        );
        assert_eq!(lines.iter().any(|line| line.contains("devo resume")), false);
    }

    #[test]
    fn onboarding_exit_messages_are_empty_without_success() {
        let session_id = SessionId::new();
        let exit = devo_tui::AppExit {
            session_id: Some(session_id),
            onboarding_completed: false,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_read_tokens: 0,
        };

        assert_eq!(
            onboarding_exit_messages(&exit, /*color_enabled*/ false),
            Vec::<String>::new()
        );
    }

    #[test]
    fn colorized_onboarding_exit_messages_include_ansi_sequences() {
        let exit = devo_tui::AppExit {
            session_id: None,
            onboarding_completed: true,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_read_tokens: 0,
        };

        let lines = onboarding_exit_messages(&exit, /*color_enabled*/ true);

        assert!(lines[0].contains("\u{1b}["));
        assert!(lines[3].contains("\u{1b}["));
    }
}
