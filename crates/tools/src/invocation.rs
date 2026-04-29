use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(pub SmolStr);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub call_id: ToolCallId,
    pub tool_name: ToolName,
    pub session_id: String,
    pub cwd: PathBuf,
    pub input: serde_json::Value,
}

pub trait ToolOutput: Send {
    fn to_content(self: Box<Self>) -> ToolContent;
    fn is_error(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolContent {
    Text(String),
    Json(serde_json::Value),
    Mixed {
        text: Option<String>,
        json: Option<serde_json::Value>,
    },
}

impl ToolContent {
    pub fn into_string(self) -> String {
        match self {
            ToolContent::Text(t) => t,
            ToolContent::Json(v) => v.to_string(),
            ToolContent::Mixed { text, json } => {
                let mut parts = Vec::new();
                if let Some(t) = text {
                    parts.push(t);
                }
                if let Some(j) = json {
                    parts.push(j.to_string());
                }
                parts.join("\n")
            }
        }
    }
}

pub struct FunctionToolOutput {
    pub content: ToolContent,
    pub is_error: bool,
}

impl FunctionToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        FunctionToolOutput {
            content: ToolContent::Text(content.into()),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        FunctionToolOutput {
            content: ToolContent::Text(message.into()),
            is_error: true,
        }
    }

    pub fn from_output(output: crate::ToolOutput) -> Self {
        FunctionToolOutput {
            content: if output.is_error {
                ToolContent::Text(output.content)
            } else {
                match output.metadata {
                    Some(meta) => ToolContent::Mixed {
                        text: Some(output.content),
                        json: Some(meta),
                    },
                    None => ToolContent::Text(output.content),
                }
            },
            is_error: output.is_error,
        }
    }
}

impl ToolOutput for FunctionToolOutput {
    fn to_content(self: Box<Self>) -> ToolContent {
        self.content
    }

    fn is_error(&self) -> bool {
        self.is_error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_newtype() {
        let name = ToolName("bash".into());
        assert_eq!(name.0.as_str(), "bash");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"bash\"");
    }

    #[test]
    fn tool_call_id_newtype() {
        let id = ToolCallId("call-1".into());
        assert_eq!(id.0, "call-1");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"call-1\"");
    }

    #[test]
    fn tool_content_text() {
        let c = ToolContent::Text("hello".into());
        assert_eq!(c.into_string(), "hello");
    }

    #[test]
    fn tool_content_json() {
        let c = ToolContent::Json(serde_json::json!({"key": "val"}));
        assert!(c.into_string().contains("val"));
    }

    #[test]
    fn tool_content_mixed() {
        let c = ToolContent::Mixed {
            text: Some("text".into()),
            json: Some(serde_json::json!({"key": 1})),
        };
        let s = c.into_string();
        assert!(s.contains("text"));
        assert!(s.contains("key"));
    }

    #[test]
    fn tool_content_mixed_text_only() {
        let c = ToolContent::Mixed {
            text: Some("just text".into()),
            json: None,
        };
        assert_eq!(c.into_string(), "just text");
    }

    #[test]
    fn tool_content_mixed_json_only() {
        let c = ToolContent::Mixed {
            text: None,
            json: Some(serde_json::json!(42)),
        };
        assert_eq!(c.into_string(), "42");
    }

    #[test]
    fn function_tool_output_success() {
        let out = FunctionToolOutput::success("done");
        assert!(!out.is_error);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "done"));
    }

    #[test]
    fn function_tool_output_error() {
        let out = FunctionToolOutput::error("failed");
        assert!(out.is_error);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "failed"));
    }

    #[test]
    fn function_tool_output_from_legacy_success_no_meta() {
        let legacy = crate::ToolOutput {
            content: "ok".into(),
            is_error: false,
            metadata: None,
        };
        let out = FunctionToolOutput::from_output(legacy);
        assert!(!out.is_error);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "ok"));
    }

    #[test]
    fn function_tool_output_from_legacy_success_with_meta() {
        let legacy = crate::ToolOutput {
            content: "result".into(),
            is_error: false,
            metadata: Some(serde_json::json!({"key": "val"})),
        };
        let out = FunctionToolOutput::from_output(legacy);
        assert!(!out.is_error);
        match out.content {
            ToolContent::Mixed { text, json } => {
                assert_eq!(text, Some("result".into()));
                assert!(json.is_some());
            }
            _ => panic!("expected Mixed"),
        }
    }

    #[test]
    fn function_tool_output_from_legacy_error() {
        let legacy = crate::ToolOutput {
            content: "err".into(),
            is_error: true,
            metadata: None,
        };
        let out = FunctionToolOutput::from_output(legacy);
        assert!(out.is_error);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "err"));
    }

    #[test]
    fn tool_output_trait_impl() {
        let out = Box::new(FunctionToolOutput::success("trait test"));
        assert!(!out.is_error());
        let content = out.to_content();
        assert!(matches!(content, ToolContent::Text(ref t) if t == "trait test"));
    }

    #[test]
    fn tool_name_serde_roundtrip() {
        let name = ToolName("exec_command".into());
        let json = serde_json::to_string(&name).unwrap();
        let back: ToolName = serde_json::from_str(&json).unwrap();
        assert_eq!(name, back);
    }

    #[test]
    fn tool_call_id_serde_roundtrip() {
        let id = ToolCallId("id-42".into());
        let json = serde_json::to_string(&id).unwrap();
        let back: ToolCallId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
