use std::fs;
use std::path::Path;

use crate::ProviderConfigError;

use super::persistence::write_atomic;
use super::schema::AUTH_CONFIG_VERSION;
use super::schema::AuthCredentialConfig;
use super::schema::AuthCredentialKind;
use super::schema::UserAuthConfigFile;

pub const AUTH_CONFIG_FILE_NAME: &str = "auth.json";

/// Returns the stable credential id generated when onboarding receives an API
/// key without an explicit credential id.
pub fn default_provider_credential_id(provider_id: &str) -> String {
    let normalized = provider_id
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        "provider_api_key".to_string()
    } else {
        format!("{normalized}_api_key")
    }
}

/// Upserts one API key credential into user-scoped `auth.json`.
pub fn upsert_user_auth_api_key(
    user_config_dir: &Path,
    credential_id: &str,
    value: &str,
) -> Result<(), ProviderConfigError> {
    let auth_file = user_config_dir.join(AUTH_CONFIG_FILE_NAME);
    let mut auth = read_user_auth_config(&auth_file)?;
    auth.credentials.insert(
        credential_id.to_string(),
        AuthCredentialConfig {
            kind: AuthCredentialKind::ApiKey,
            value: value.to_string(),
        },
    );
    write_user_auth_config(&auth_file, &auth)
}

/// Removes a user-scoped credential from auth.json.
///
/// Returning Ok(false) means the credential id was not present. The file is
/// still written when a credential was removed so the operation is durable.
pub fn remove_user_auth_credential(
    user_config_dir: &Path,
    credential_id: &str,
) -> Result<bool, ProviderConfigError> {
    let auth_file = user_config_dir.join(AUTH_CONFIG_FILE_NAME);
    let mut auth = read_user_auth_config(&auth_file)?;
    let removed = auth.credentials.remove(credential_id).is_some();
    if removed {
        write_user_auth_config(&auth_file, &auth)?;
    }
    Ok(removed)
}

pub fn read_user_auth_config(auth_file: &Path) -> Result<UserAuthConfigFile, ProviderConfigError> {
    if !auth_file.exists() {
        return Ok(UserAuthConfigFile::default());
    }

    let data = fs::read_to_string(auth_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: auth_file.to_path_buf(),
        source,
    })?;
    let auth: UserAuthConfigFile =
        serde_json::from_str(&data).map_err(|error| ProviderConfigError::ParseAuth {
            path: auth_file.to_path_buf(),
            message: error.to_string(),
        })?;
    if auth.version != AUTH_CONFIG_VERSION {
        return Err(ProviderConfigError::Validation {
            message: format!(
                "unsupported auth.json schema version {} at {}",
                auth.version,
                auth_file.display()
            ),
        });
    }
    for (credential_id, credential) in &auth.credentials {
        if credential.value.is_empty() {
            return Err(ProviderConfigError::Validation {
                message: format!("credential `{credential_id}` in auth.json has an empty value"),
            });
        }
    }
    Ok(auth)
}

fn write_user_auth_config(
    auth_file: &Path,
    auth: &UserAuthConfigFile,
) -> Result<(), ProviderConfigError> {
    if let Some(parent) = auth_file.parent() {
        fs::create_dir_all(parent).map_err(|source| ProviderConfigError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data = serde_json::to_vec_pretty(auth).map_err(|error| ProviderConfigError::Serialize {
        message: error.to_string(),
    })?;
    write_atomic(auth_file, &data)
}
