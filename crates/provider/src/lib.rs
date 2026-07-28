//! Provider adapters and routing for model request execution.
//!
//! This crate keeps wire-format details for OpenAI-family, Anthropic, and
//! provider-compatible APIs behind a small router interface so the runtime can
//! work with normalized protocol events.

pub mod anthropic;
mod dsml;
pub mod error;
mod hosted_tools;
mod http;
pub mod openai;
mod provider;
pub mod recovery_hint;
mod request;
pub mod router;
mod text_normalization;
pub mod timeout;

pub use http::ProviderHttpOptions;
pub use provider::*;
pub use recovery_hint::{
    AUTH_HINT, MODEL_NOT_FOUND_HINT, NETWORK_PROXY_HINT, recovery_hint_for_anyhow,
    recovery_hint_for_message,
};
pub(crate) use request::merge_extra_body;
pub use router::*;
