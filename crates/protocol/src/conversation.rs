use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{RequestContent, RequestMessage, Usage};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl JsonSchema for $name {
            fn schema_name() -> String {
                String::from(stringify!($name))
            }

            fn json_schema(
                generator: &mut schemars::r#gen::SchemaGenerator,
            ) -> schemars::schema::Schema {
                String::json_schema(generator)
            }
        }

        impl TS for $name {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;

            fn name(_: &ts_rs::Config) -> String {
                String::from(stringify!($name))
            }

            fn inline(cfg: &ts_rs::Config) -> String {
                Self::name(cfg)
            }

            fn decl(_: &ts_rs::Config) -> String {
                String::from(concat!("type ", stringify!($name), " = string;"))
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = uuid::Error;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }

        impl TryFrom<String> for $name {
            type Error = uuid::Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::try_from(value.as_str())
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::try_from(s)
            }
        }
    };
}

define_id!(SessionId);
define_id!(TurnId);
define_id!(ItemId);
define_id!(PendingInputId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum SessionTitleState {
    Unset,
    Provisional,
    Final(SessionTitleFinalSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum SessionTitleFinalSource {
    ModelGenerated,
    UserRename,
    ExplicitCreate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum TurnStatus {
    Pending,
    Running,
    WaitingApproval,
    Interrupted,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

impl TurnUsage {
    pub fn from_usage(usage: &Usage) -> Self {
        Self {
            input_tokens: saturating_u32(usage.input_tokens),
            output_tokens: saturating_u32(usage.output_tokens),
            cache_creation_input_tokens: usage.cache_creation_input_tokens.map(saturating_u32),
            cache_read_input_tokens: usage.cache_read_input_tokens.map(saturating_u32),
            reasoning_output_tokens: usage.reasoning_output_tokens.map(saturating_u32),
            total_tokens: usage.total_tokens.map(saturating_u32),
        }
    }

    pub fn derived_total_tokens(&self) -> usize {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .try_into()
            .unwrap_or(usize::MAX)
    }

    pub fn display_total_tokens(&self) -> usize {
        self.total_tokens
            .map(|tokens| tokens.try_into().unwrap_or(usize::MAX))
            .unwrap_or_else(|| self.derived_total_tokens())
    }
}

fn saturating_u32(value: usize) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    #[serde(rename = "provider_reasoning")]
    ProviderReasoning {
        provider: String,
        payload: serde_json::Value,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "hosted_tool_use")]
    HostedToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                ContentBlock::Text { .. }
                | ContentBlock::Reasoning { .. }
                | ContentBlock::ProviderReasoning { .. }
                | ContentBlock::HostedToolUse { .. }
                | ContentBlock::ToolResult { .. } => None,
            })
            .collect()
    }

    pub fn to_request_message(&self) -> RequestMessage {
        let content = self
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => RequestContent::Text { text: text.clone() },
                ContentBlock::Reasoning { text } => {
                    RequestContent::Reasoning { text: text.clone() }
                }
                ContentBlock::ProviderReasoning { provider, payload } => {
                    RequestContent::ProviderReasoning {
                        provider: provider.clone(),
                        payload: payload.clone(),
                    }
                }
                ContentBlock::ToolUse { id, name, input } => RequestContent::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                ContentBlock::HostedToolUse {
                    id,
                    name,
                    input,
                    output,
                    status,
                } => RequestContent::HostedToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    output: output.clone(),
                    status: status.clone(),
                },
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => RequestContent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: if *is_error { Some(true) } else { None },
                },
            })
            .collect();

        RequestMessage {
            role: self.role.as_str().to_string(),
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn role_system_as_str() {
        assert_eq!(Role::System.as_str(), "system");
    }

    #[test]
    fn role_user_as_str() {
        assert_eq!(Role::User.as_str(), "user");
    }

    #[test]
    fn role_assistant_as_str() {
        assert_eq!(Role::Assistant.as_str(), "assistant");
    }

    #[test]
    fn message_system_creates_system_role() {
        let msg = Message::system("budget notice");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content.len(), 1);
        assert!(
            matches!(msg.content[0], ContentBlock::Text { ref text } if text == "budget notice")
        );
    }

    #[test]
    fn message_system_to_request_message() {
        let msg = Message::system("system instruction");
        let req = msg.to_request_message();
        assert_eq!(req.role, "system");
    }

    #[test]
    fn turn_usage_from_usage_preserves_provider_total_breakdown() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: Some(2),
            cache_read_input_tokens: Some(3),
            reasoning_output_tokens: Some(7),
            total_tokens: Some(20),
        };

        let turn_usage = TurnUsage::from_usage(&usage);

        assert_eq!(
            turn_usage,
            TurnUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: Some(2),
                cache_read_input_tokens: Some(3),
                reasoning_output_tokens: Some(7),
                total_tokens: Some(20),
            }
        );
        assert_eq!(turn_usage.derived_total_tokens(), 15);
        assert_eq!(turn_usage.display_total_tokens(), 20);
    }

    #[test]
    fn message_user_to_request_message_role() {
        let msg = Message::user("hello");
        let req = msg.to_request_message();
        assert_eq!(req.role, "user");
    }
}
