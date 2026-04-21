use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Once;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::Builder as RollingFileAppenderBuilder;
use tracing_appender::rolling::InitError as RollingInitError;
use tracing_appender::rolling::Rotation;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;

use crate::LogRotation;
use crate::LoggingConfig;

/// Defines the runtime inputs used when installing process-wide tracing.
#[derive(Debug, Clone)]
pub struct LoggingBootstrap {
    /// The human-readable process role used in filenames and bootstrap events.
    pub process_name: &'static str,
    /// The resolved application logging policy.
    pub config: LoggingConfig,
    /// The stable home directory used to derive default log paths.
    pub home_dir: PathBuf,
}

/// Keeps non-blocking tracing sinks alive for the process lifetime.
#[derive(Debug)]
pub struct LoggingRuntime {
    _file_guard: WorkerGuard,
}

/// Enumerates logging-install failures.
#[derive(Debug, thiserror::Error)]
pub enum LoggingInitError {
    /// The configured log directory could not be created.
    #[error("failed to create log directory at {path}: {source}")]
    CreateDirectory {
        /// The log directory that failed to initialize.
        path: PathBuf,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The rolling file appender could not be created.
    #[error("failed to initialize rolling log file under {path}: {source}")]
    BuildFileAppender {
        /// The target log directory.
        path: PathBuf,
        /// The underlying appender error.
        #[source]
        source: RollingInitError,
    },
    /// Another subscriber was already installed for this process.
    #[error("global tracing subscriber is already installed")]
    SubscriberAlreadyInstalled,
}

impl LoggingBootstrap {
    /// Installs the process-wide tracing subscriber with durable rolling file logging.
    pub fn install(self) -> Result<LoggingRuntime, LoggingInitError> {
        let file_level = file_level(&self.config.level);
        let file_dir = resolve_log_directory(&self.home_dir, self.config.file.directory.as_deref());
        fs::create_dir_all(&file_dir).map_err(|source| LoggingInitError::CreateDirectory {
            path: file_dir.clone(),
            source,
        })?;

        let filename_prefix = format!("{}-{}", self.config.file.filename_prefix, self.process_name);
        let file_appender = RollingFileAppenderBuilder::new()
            .rotation(self.config.file.rotation.into())
            .filename_prefix(filename_prefix.as_str())
            .filename_suffix("log")
            .max_log_files(self.config.file.max_files)
            .build(&file_dir)
            .map_err(|source| LoggingInitError::BuildFileAppender {
                path: file_dir.clone(),
                source,
            })?;
        let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);

        install_subscriber(file_level, self.config.json, file_writer)?;
        install_panic_hook();

        tracing::info!(
            process = self.process_name,
            level = self.config.level,
            json = self.config.json,
            sink = "file",
            redact_secrets = self.config.redact_secrets_in_logs,
            log_directory = %file_dir.display(),
            rotation = ?self.config.file.rotation,
            max_files = self.config.file.max_files,
            "tracing initialized"
        );

        Ok(LoggingRuntime {
            _file_guard: file_guard,
        })
    }
}

/// Builds and installs the global tracing subscriber for file-based logging output.
fn install_subscriber<W>(
    file_level: LevelFilter,
    json: bool,
    file_writer: W,
) -> Result<(), LoggingInitError>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    if json {
        let subscriber = Registry::default().with(
            fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(file_writer)
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .with_filter(file_level),
        );
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|_| LoggingInitError::SubscriberAlreadyInstalled)
    } else {
        let subscriber = Registry::default().with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer)
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .with_filter(file_level),
        );
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|_| LoggingInitError::SubscriberAlreadyInstalled)
    }
}

/// Parses the configured log level and falls back to `INFO` when parsing fails.
fn file_level(default_level: &str) -> LevelFilter {
    LevelFilter::from_str(default_level).unwrap_or(LevelFilter::INFO)
}

/// Resolves the effective log directory from the home directory and optional override.
fn resolve_log_directory(home_dir: &Path, configured_directory: Option<&Path>) -> PathBuf {
    match configured_directory {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => home_dir.join(path),
        None => home_dir.join("logs"),
    }
}

/// Installs a panic hook that records panic details through tracing before delegating.
fn install_panic_hook() {
    static INSTALL_PANIC_HOOK: Once = Once::new();

    INSTALL_PANIC_HOOK.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let location = panic_info
                .location()
                .map(|location| format!("{}:{}", location.file(), location.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let payload = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
                message.clone()
            } else {
                "non-string panic payload".to_string()
            };
            tracing::error!(
                panic.location = location,
                panic.payload = payload,
                "process panicked"
            );
            previous_hook(panic_info);
        }));
    });
}

impl From<LogRotation> for Rotation {
    /// Converts the configuration-facing rotation enum into the appender rotation type.
    fn from(value: LogRotation) -> Self {
        match value {
            LogRotation::Never => Rotation::NEVER,
            LogRotation::Minutely => Rotation::MINUTELY,
            LogRotation::Hourly => Rotation::HOURLY,
            LogRotation::Daily => Rotation::DAILY,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use tracing_appender::rolling::Rotation;
    use tracing_subscriber::filter::LevelFilter;

    use crate::LogRotation;

    use super::file_level;
    use super::resolve_log_directory;

    #[test]
    /// Verifies that recognized log levels are parsed into tracing level filters.
    fn file_level_parses_configured_level() {
        assert_eq!(file_level("trace"), LevelFilter::TRACE);
        assert_eq!(file_level("debug"), LevelFilter::DEBUG);
        assert_eq!(file_level("info"), LevelFilter::INFO);
        assert_eq!(file_level("warn"), LevelFilter::WARN);
        assert_eq!(file_level("error"), LevelFilter::ERROR);
    }

    #[test]
    /// Verifies that invalid log levels fall back to `INFO`.
    fn file_level_defaults_to_info_for_invalid_level() {
        let parsed = file_level("chatty");
        assert_eq!(parsed, LevelFilter::INFO);
    }

    #[test]
    /// Verifies that config-facing rotations map to the appender rotation constants.
    fn log_rotation_maps_to_appender_rotation() {
        let rotations: Vec<Rotation> = [
            LogRotation::Never,
            LogRotation::Minutely,
            LogRotation::Hourly,
            LogRotation::Daily,
        ]
        .into_iter()
        .map(Rotation::from)
        .collect();

        assert_eq!(
            rotations,
            vec![
                Rotation::NEVER,
                Rotation::MINUTELY,
                Rotation::HOURLY,
                Rotation::DAILY,
            ]
        );
    }

    #[test]
    /// Verifies that logs default to a `logs` directory under the configured home directory.
    fn resolve_log_directory_defaults_under_home() {
        let resolved = resolve_log_directory(Path::new("/tmp/.devo"), None);
        assert_eq!(resolved, PathBuf::from("/tmp/.devo/logs"));
    }

    #[cfg(windows)]
    #[test]
    /// Verifies that a relative Windows override is resolved under the home directory.
    fn resolve_log_directory_supports_relative_override_windows() {
        let resolved = resolve_log_directory(
            Path::new("C:\\Users\\tester\\.devo"),
            Some(Path::new("diagnostics")),
        );
        assert_eq!(
            resolved,
            PathBuf::from("C:\\Users\\tester\\.devo\\diagnostics")
        );
    }

    #[cfg(windows)]
    #[test]
    /// Verifies that an absolute Windows override is preserved as-is.
    fn resolve_log_directory_preserves_absolute_override_windows() {
        let resolved = resolve_log_directory(
            Path::new("C:\\Users\\tester\\.devo"),
            Some(Path::new("D:\\devo-logs")),
        );
        assert_eq!(resolved, PathBuf::from("D:\\devo-logs"));
    }

    #[cfg(unix)]
    #[test]
    /// Verifies that a relative Unix override is resolved under the home directory.
    fn resolve_log_directory_supports_relative_override_unix() {
        let resolved = resolve_log_directory(
            Path::new("/home/tester/.devo"),
            Some(Path::new("diagnostics")),
        );
        assert_eq!(resolved, PathBuf::from("/home/tester/.devo/diagnostics"));
    }

    #[cfg(unix)]
    #[test]
    /// Verifies that an absolute Unix override is preserved as-is.
    fn resolve_log_directory_preserves_absolute_override_unix() {
        let resolved = resolve_log_directory(
            Path::new("/home/tester/.devo"),
            Some(Path::new("/var/log/devo")),
        );
        assert_eq!(resolved, PathBuf::from("/var/log/devo"));
    }
}
