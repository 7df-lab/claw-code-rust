use std::ops::Mul;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TruncationMode {
    Bytes,
    Tokens,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub struct TruncationPolicyConfig {
    pub mode: TruncationMode,
    pub limit: i64,
}

impl Default for TruncationPolicyConfig {
    fn default() -> Self {
        Self::bytes(8_000)
    }
}

impl TruncationPolicyConfig {
    pub const fn bytes(limit: i64) -> Self {
        Self {
            mode: TruncationMode::Bytes,
            limit,
        }
    }

    pub const fn tokens(limit: i64) -> Self {
        Self {
            mode: TruncationMode::Tokens,
            limit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationPolicy {
    Bytes(usize),
    Tokens(usize),
}

impl From<TruncationPolicyConfig> for TruncationPolicy {
    fn from(config: TruncationPolicyConfig) -> Self {
        match config.mode {
            TruncationMode::Bytes => Self::Bytes(config.limit as usize),
            TruncationMode::Tokens => Self::Tokens(config.limit as usize),
        }
    }
}

impl TruncationPolicy {
    pub fn token_budget(&self) -> usize {
        match self {
            TruncationPolicy::Bytes(bytes) => approx_tokens_from_byte_count(*bytes) as usize,
            TruncationPolicy::Tokens(tokens) => *tokens,
        }
    }

    pub fn byte_budget(&self) -> usize {
        match self {
            TruncationPolicy::Bytes(bytes) => *bytes,
            TruncationPolicy::Tokens(tokens) => approx_bytes_for_tokens(*tokens),
        }
    }
}

impl Mul<f64> for TruncationPolicy {
    type Output = Self;

    fn mul(self, multiplier: f64) -> Self::Output {
        match self {
            TruncationPolicy::Bytes(bytes) => {
                TruncationPolicy::Bytes((bytes as f64 * multiplier).ceil() as usize)
            }
            TruncationPolicy::Tokens(tokens) => {
                TruncationPolicy::Tokens((tokens as f64 * multiplier).ceil() as usize)
            }
        }
    }
}

pub fn deserialize_truncation_policy_config<'de, D>(
    deserializer: D,
) -> Result<TruncationPolicyConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(TruncationPolicyConfig::default()),
        serde_json::Value::String(text) if text.trim().is_empty() => {
            Ok(TruncationPolicyConfig::default())
        }
        other @ serde_json::Value::Object(_) => {
            serde_json::from_value(other).map_err(serde::de::Error::custom)
        }
        other => Err(serde::de::Error::custom(format!(
            "expected truncation policy object or empty string, got {other}"
        ))),
    }
}

const APPROX_BYTES_PER_TOKEN: usize = 4;

pub fn approx_bytes_for_tokens(tokens: usize) -> usize {
    tokens.saturating_mul(APPROX_BYTES_PER_TOKEN)
}

pub fn approx_tokens_from_byte_count(bytes: usize) -> u64 {
    let bytes_u64 = bytes as u64;
    bytes_u64.saturating_add((APPROX_BYTES_PER_TOKEN as u64).saturating_sub(1))
        / (APPROX_BYTES_PER_TOKEN as u64)
}
