use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

/// Stores logging defaults for the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// The default logging level string.
    pub level: String,
    /// Whether logs should be emitted in JSON format.
    pub json: bool,
    /// Whether secrets should be redacted from logs before emission.
    pub redact_secrets_in_logs: bool,
    /// Durable file-log persistence settings.
    pub file: LoggingFileConfig,
}

/// Selects the rolling cadence used for persisted log files.
///
/// This intentionally mirrors the supported `tracing_appender::rolling::Rotation`
/// constants instead of using that type directly: the appender type is a runtime
/// implementation detail with private internals and no serde support, while this
/// enum is part of Devo's persisted configuration schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogRotation {
    /// Keep appending to one file until the process rotates it manually.
    Never,
    /// Rotate once per minute.
    Minutely,
    /// Rotate once per hour.
    Hourly,
    /// Rotate once per day.
    Daily,
}

/// Stores persistence settings for rolling file logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingFileConfig {
    /// The directory used for persisted log files. Relative paths resolve under `DEVO_HOME`.
    pub directory: Option<PathBuf>,
    /// The stable filename prefix written before the process suffix and rotation timestamp.
    pub filename_prefix: String,
    /// The file-rotation cadence applied to persisted logs.
    pub rotation: LogRotation,
    /// The maximum number of rotated files retained on disk.
    pub max_files: usize,
}
