use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use toml::Value;

use crate::ProviderConfigError;

use super::ProviderConfigFile;

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const PROVIDER_CONFIG_FILE_NAME: &str = "providers.json";

pub fn read_provider_catalog_config(
    config_file: &Path,
) -> Result<ProviderConfigFile, ProviderConfigError> {
    if !config_file.exists() {
        return Ok(ProviderConfigFile::default());
    }

    let data = fs::read_to_string(config_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: config_file.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&data).map_err(|error| ProviderConfigError::ParseJsonFile {
        path: config_file.to_path_buf(),
        message: error.to_string(),
    })
}

pub fn write_provider_catalog_config(
    config_file: &Path,
    config: &ProviderConfigFile,
) -> Result<(), ProviderConfigError> {
    if let Some(parent) = config_file.parent() {
        fs::create_dir_all(parent).map_err(|source| ProviderConfigError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data =
        serde_json::to_vec_pretty(config).map_err(|error| ProviderConfigError::Serialize {
            message: error.to_string(),
        })?;
    let mut data = data;
    data.push(b'\n');
    write_atomic(config_file, &data)
}

/// Reads raw TOML so unrelated app configuration sections can be updated safely.
pub(crate) fn read_provider_config_document(
    config_file: &Path,
) -> Result<Value, ProviderConfigError> {
    if !config_file.exists() {
        return Ok(Value::Table(Default::default()));
    }

    let data = fs::read_to_string(config_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: config_file.to_path_buf(),
        source,
    })?;
    toml::from_str(&data).map_err(|error| ProviderConfigError::ParseTomlFile {
        path: config_file.to_path_buf(),
        message: error.to_string(),
    })
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<(), ProviderConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    for attempt in 0..16 {
        let temp_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            nanos + attempt
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(data).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ProviderConfigError::Io {
                        action: "write",
                        path: path.to_path_buf(),
                        source: error,
                    });
                }
                if let Err(error) = fs::rename(&temp_path, path) {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ProviderConfigError::Io {
                        action: "write",
                        path: path.to_path_buf(),
                        source: error,
                    });
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ProviderConfigError::Io {
                    action: "create",
                    path: temp_path,
                    source: error,
                });
            }
        }
    }

    Err(ProviderConfigError::Validation {
        message: format!(
            "failed to create temporary config file in {}",
            parent.display()
        ),
    })
}

pub(crate) fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
