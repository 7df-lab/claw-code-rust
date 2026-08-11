//! Runtime-neutral tool invocation and output values.
//!
//! This module deliberately stops at the protocol boundary: handlers can return
//! canonical model-facing content plus optional display-only text, while runtime
//! policy such as permissions, cancellation, and progress reporting lives in the
//! contracts module.

use std::fmt::Write as _;
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
    /// Optional text tailored for local display surfaces.
    ///
    /// This is intentionally separate from `ToolContent`: the canonical content
    /// is sent through protocol/replay paths, while display content may omit
    /// wrappers or metadata that would be noisy in the UI.
    fn display_content(&self) -> Option<&str> {
        None
    }
}

/// Canonical content returned by a tool invocation.
///
/// Callers may persist or forward this value, so display-only shortening should
/// use `ToolOutput::display_content` instead of changing these variants.
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
    pub fn text_part(&self) -> Option<&str> {
        match self {
            ToolContent::Text(text) => Some(text),
            ToolContent::Json(_) => None,
            ToolContent::Mixed { text, .. } => text.as_deref(),
        }
    }

    /// Model-facing serialization for tools whose `text` is the stream and whose
    /// `json` is non-model metadata (e.g. shell exit/cwd). Prefer
    /// [`Self::into_string`] when JSON carries model-visible payload (images,
    /// search hits, etc.).
    pub fn text_for_model(self) -> String {
        match self {
            ToolContent::Text(text) => text,
            ToolContent::Json(json) => json.to_string(),
            ToolContent::Mixed { text, json } => match text {
                Some(text) => text,
                None => json.map(|value| value.to_string()).unwrap_or_default(),
            },
        }
    }

    /// Byte length of [`Self::text_for_model`] without consuming `self`.
    pub fn text_for_model_byte_len(&self) -> usize {
        match self {
            ToolContent::Text(text) => text.len(),
            ToolContent::Json(json) => json.to_string().len(),
            ToolContent::Mixed { text, json } => match text {
                Some(text) => text.len(),
                None => json.as_ref().map_or(0, |value| value.to_string().len()),
            },
        }
    }

    /// Byte length of [`Self::into_string`] without consuming `self`.
    pub fn into_string_byte_len(&self) -> usize {
        match self {
            ToolContent::Text(text) => text.len(),
            ToolContent::Json(json) => json.to_string().len(),
            ToolContent::Mixed { text, json } => {
                let text_len = text.as_ref().map_or(0, String::len);
                let json_len = json.as_ref().map_or(0, |value| value.to_string().len());
                let separator = usize::from(text.is_some() && json.is_some());
                text_len + separator + json_len
            }
        }
    }

    pub fn into_string(self) -> String {
        match self {
            ToolContent::Text(t) => t,
            ToolContent::Json(v) => v.to_string(),
            ToolContent::Mixed { text, json } => match (text, json) {
                (Some(text), Some(json)) => {
                    let mut output = String::with_capacity(text.len() + 1);
                    output.push_str(&text);
                    output.push('\n');
                    let _ = write!(output, "{json}");
                    output
                }
                (Some(text), None) => text,
                (None, Some(json)) => json.to_string(),
                (None, None) => String::new(),
            },
        }
    }
}

pub struct FunctionToolOutput {
    pub content: ToolContent,
    pub is_error: bool,
    /// Optional UI-facing rendering of `content`.
    ///
    /// Some tools deliberately store both forms: one for the model/protocol and
    /// one for compact human display. Keep them distinct even when they contain
    /// similar text.
    pub display_content: Option<String>,
}

impl FunctionToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        FunctionToolOutput {
            content: ToolContent::Text(content.into()),
            is_error: false,
            display_content: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        FunctionToolOutput {
            content: ToolContent::Text(message.into()),
            is_error: true,
            display_content: None,
        }
    }

    pub fn success_with_metadata(content: impl Into<String>, metadata: serde_json::Value) -> Self {
        FunctionToolOutput {
            content: ToolContent::Mixed {
                text: Some(content.into()),
                json: Some(metadata),
            },
            is_error: false,
            display_content: None,
        }
    }

    pub fn with_display_content(mut self, display_content: impl Into<String>) -> Self {
        self.display_content = Some(display_content.into());
        self
    }
}

impl ToolOutput for FunctionToolOutput {
    fn to_content(self: Box<Self>) -> ToolContent {
        self.content
    }

    fn is_error(&self) -> bool {
        self.is_error
    }

    fn display_content(&self) -> Option<&str> {
        self.display_content.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

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
        assert_eq!(c.text_part(), Some("hello"));
        assert_eq!(c.into_string(), "hello");
    }

    #[test]
    fn tool_content_json() {
        let c = ToolContent::Json(serde_json::json!({"key": "val"}));
        assert_eq!(c.text_part(), None);
        assert!(c.into_string().contains("val"));
    }

    #[test]
    fn tool_content_mixed() {
        let c = ToolContent::Mixed {
            text: Some("text".into()),
            json: Some(serde_json::json!({"key": 1})),
        };
        assert_eq!(c.text_part(), Some("text"));
        assert_eq!(c.text_for_model_byte_len(), 4);
        let s = c.clone().into_string();
        assert!(s.contains("text"));
        assert!(s.contains("key"));
        assert_eq!(c.clone().text_for_model(), "text");
        assert_eq!(c.into_string_byte_len(), s.len());
    }

    #[test]
    fn tool_content_mixed_text_for_model_omits_json() {
        let output = "hello\nworld".to_string();
        let content = ToolContent::Mixed {
            text: Some(output.clone()),
            json: Some(serde_json::json!({
                "command": "echo hello",
                "exit": 0,
                "cwd": "/tmp",
            })),
        };
        let model = content.text_for_model();
        assert_eq!(model, output);
        assert_eq!(model.matches("hello").count(), 1);
        assert!(!model.contains("\"exit\""));
        assert!(!model.contains("\"command\""));
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
        assert_eq!(c.text_part(), None);
        assert_eq!(c.into_string(), "42");
    }

    #[test]
    fn function_tool_output_success() {
        let out = FunctionToolOutput::success("done");
        assert!(!out.is_error);
        assert_eq!(out.display_content(), None);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "done"));
    }

    #[test]
    fn function_tool_output_error() {
        let out = FunctionToolOutput::error("failed");
        assert!(out.is_error);
        assert_eq!(out.display_content(), None);
        assert!(matches!(out.content, ToolContent::Text(ref t) if t == "failed"));
    }

    #[test]
    fn function_tool_output_success_with_metadata() {
        let out =
            FunctionToolOutput::success_with_metadata("result", serde_json::json!({"key": "val"}));
        assert!(!out.is_error);
        assert_eq!(out.display_content(), None);
        match out.content {
            ToolContent::Mixed { text, json } => {
                assert_eq!(text, Some("result".into()));
                assert_eq!(json, Some(serde_json::json!({"key": "val"})));
            }
            _ => panic!("expected Mixed"),
        }
    }

    #[test]
    fn function_tool_output_with_display_content() {
        let out = FunctionToolOutput::success("canonical").with_display_content("display");
        assert_eq!(out.display_content(), Some("display"));
        assert!(matches!(out.content, ToolContent::Text(ref text) if text == "canonical"));
    }

    #[test]
    fn tool_output_trait_impl() {
        let out = Box::new(FunctionToolOutput::success("trait test"));
        assert!(!out.is_error());
        assert_eq!(out.display_content(), None);
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
