use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

/// OpenAI reasoning-effort levels supported by reasoning models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl fmt::Display for OpenAIReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OpenAIReasoningEffort::None => "none",
            OpenAIReasoningEffort::Minimal => "minimal",
            OpenAIReasoningEffort::Low => "low",
            OpenAIReasoningEffort::Medium => "medium",
            OpenAIReasoningEffort::High => "high",
            OpenAIReasoningEffort::XHigh => "xhigh",
            OpenAIReasoningEffort::Max => "max",
        })
    }
}

impl FromStr for OpenAIReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(OpenAIReasoningEffort::None),
            "minimal" => Ok(OpenAIReasoningEffort::Minimal),
            "low" => Ok(OpenAIReasoningEffort::Low),
            "medium" => Ok(OpenAIReasoningEffort::Medium),
            "high" => Ok(OpenAIReasoningEffort::High),
            "xhigh" => Ok(OpenAIReasoningEffort::XHigh),
            "max" => Ok(OpenAIReasoningEffort::Max),
            other => Err(format!("invalid OpenAI reasoning effort: {other}")),
        }
    }
}
