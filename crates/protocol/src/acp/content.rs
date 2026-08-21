use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::AcpMeta;
use crate::InputItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpContentBlock {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        text: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Audio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    ResourceLink {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "mimeType")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<i64>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Resource {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        resource: AcpEmbeddedResource,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
}

impl AcpContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            annotations: None,
            text: text.into(),
            meta: None,
        }
    }

    pub fn into_input_items(self) -> Result<Vec<InputItem>, String> {
        match self {
            Self::Text { text, .. } => Ok(vec![InputItem::Text { text }]),
            Self::Image { .. } => {
                Err("session/prompt image content is not supported by this agent".to_string())
            }
            Self::Audio { .. } => {
                Err("session/prompt audio content is not supported by this agent".to_string())
            }
            Self::ResourceLink { uri, name, .. } => {
                if let Some(path) = path_from_file_uri(&uri) {
                    Ok(vec![InputItem::Mention {
                        path: path.to_string_lossy().into_owned(),
                        name: Some(name),
                    }])
                } else {
                    Ok(vec![InputItem::Text {
                        text: format!("Resource {name}: {uri}"),
                    }])
                }
            }
            Self::Resource { resource, .. } => Ok(vec![InputItem::Text {
                text: resource.into_prompt_text(),
            }]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum AcpRole {
    Assistant,
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<AcpRole>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AcpEmbeddedResource {
    Text(AcpTextResourceContents),
    Blob(AcpBlobResourceContents),
}

impl AcpEmbeddedResource {
    fn into_prompt_text(self) -> String {
        match self {
            Self::Text(resource) => format!("Resource {}:\n{}", resource.uri, resource.text),
            Self::Blob(resource) => {
                let mime_type = resource.mime_type.unwrap_or_else(|| "unknown".to_string());
                format!(
                    "Resource {} ({mime_type}; base64):\n{}",
                    resource.uri, resource.blob
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpTextResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub text: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpBlobResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub blob: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

fn path_from_file_uri(uri: &str) -> Option<std::path::PathBuf> {
    let path = uri.strip_prefix("file://")?;
    #[cfg(windows)]
    {
        let path = path.strip_prefix('/').unwrap_or(path);
        Some(std::path::PathBuf::from(path.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        Some(std::path::PathBuf::from(path))
    }
}
