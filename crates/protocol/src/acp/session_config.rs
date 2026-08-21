use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::AcpMeta;

pub type AcpSessionConfigGroupId = String;
pub type AcpSessionConfigId = String;
pub type AcpSessionConfigValueId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpSessionConfigOptionCategoryKnown {
    Mode,
    Model,
    ModelConfig,
    ThoughtLevel,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AcpSessionConfigOptionCategory {
    Known(AcpSessionConfigOptionCategoryKnown),
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionConfigSelectOption {
    pub value: AcpSessionConfigValueId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionConfigSelectGroup {
    pub group: AcpSessionConfigGroupId,
    pub name: String,
    pub options: Vec<AcpSessionConfigSelectOption>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AcpSessionConfigSelectOptions {
    Ungrouped(Vec<AcpSessionConfigSelectOption>),
    Grouped(Vec<AcpSessionConfigSelectGroup>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionConfigSelect {
    #[serde(rename = "currentValue")]
    pub current_value: AcpSessionConfigValueId,
    pub options: AcpSessionConfigSelectOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpSessionConfigOption {
    Select {
        id: AcpSessionConfigId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<AcpSessionConfigOptionCategory>,
        #[serde(rename = "currentValue")]
        current_value: AcpSessionConfigValueId,
        options: AcpSessionConfigSelectOptions,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Boolean {
        id: AcpSessionConfigId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<AcpSessionConfigOptionCategory>,
        #[serde(rename = "currentValue")]
        current_value: bool,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
}
