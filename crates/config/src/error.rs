use std::path::PathBuf;

/// Enumerates failures that can occur while loading or validating app config.
#[derive(Debug, thiserror::Error)]
pub enum AppConfigError {
    /// Reading a config file from disk failed.
    #[error("config IO failed at {path}: {source}")]
    Io {
        /// The config path that failed to read.
        path: PathBuf,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Parsing TOML into the config schema failed.
    #[error("config parse failed at {path}: {message}")]
    Parse { path: PathBuf, message: String },
    /// Cross-field validation rejected the normalized config.
    #[error("invalid app config: {message}")]
    Validation { message: String },
    /// Provider config loading or persistence failed.
    #[error("provider config failed: {source}")]
    Provider {
        /// The underlying provider config error.
        #[source]
        source: ProviderConfigError,
    },
}

/// Enumerates failures that can occur while loading, validating, or persisting provider config.
#[derive(Debug, thiserror::Error)]
pub enum ProviderConfigError {
    /// The config path could not be determined.
    #[error("{message}")]
    ConfigPath { message: String },
    /// Reading or writing a provider config file failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        /// The failed filesystem action.
        action: &'static str,
        /// The path being accessed.
        path: PathBuf,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Parsing provider TOML from a file failed.
    #[error("failed to parse {path}: {message}")]
    ParseTomlFile { path: PathBuf, message: String },
    /// Parsing provider JSON from a file failed.
    #[error("failed to parse {path}: {message}")]
    ParseJsonFile { path: PathBuf, message: String },
    /// Parsing user-scoped auth JSON failed.
    #[error("failed to parse {path}: {message}")]
    ParseAuth { path: PathBuf, message: String },
    /// Serializing provider config failed.
    #[error("failed to serialize provider config: {message}")]
    Serialize { message: String },
    /// Provider config validation failed.
    #[error("{message}")]
    Validation { message: String },
}
