mod auth;
mod catalog;
mod migration;
mod persistence;
mod request;
mod runtime;
mod schema;

pub use devo_protocol::ProviderWireApi;

pub use auth::AUTH_CONFIG_FILE_NAME;
pub use auth::default_provider_credential_id;
pub use auth::read_user_auth_config;
pub use auth::remove_user_auth_credential;
pub use auth::upsert_user_auth_api_key;
pub use catalog::{
    PROVIDER_CONFIG_FILE_VERSION, ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig,
    ProviderModelSelection, ProviderModelVariantConfig, model_reference,
};
pub use persistence::CONFIG_FILE_NAME;
pub use persistence::PROVIDER_CONFIG_FILE_NAME;
pub use request::provider_request_config;
pub use runtime::provider_runtime_config_changed;
pub use schema::*;

pub(crate) use migration::migrate_legacy_provider_config_on_startup;
pub(crate) use persistence::non_empty_string;
pub use persistence::read_provider_catalog_config;
pub(crate) use persistence::read_provider_config_document;
pub(crate) use persistence::write_atomic;
pub use persistence::write_provider_catalog_config;
