use serde::{Deserialize, Serialize};

use crate::JsonSchema;

impl ToolSpec {
    pub fn new(name: &str, description: &str, input_schema: JsonSchema) -> Self {
        ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            input_schema,
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_spec_defaults() {
        let spec = ToolSpec::new("test", "a test tool", JsonSchema::string(None));
        assert_eq!(spec.name, "test");
        assert_eq!(spec.description, "a test tool");
        assert_eq!(spec.output_mode, ToolOutputMode::Text);
        assert_eq!(spec.execution_mode, ToolExecutionMode::Mutating);
        assert!(spec.capability_tags.is_empty());
        assert!(!spec.supports_parallel);
    }

    #[test]
    fn tool_output_mode_serde() {
        let json = serde_json::to_string(&ToolOutputMode::Mixed).unwrap();
        assert_eq!(json, "\"Mixed\"");
        let deserialized: ToolOutputMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ToolOutputMode::Mixed);
    }

    #[test]
    fn tool_execution_mode_ord() {
        assert_ne!(ToolExecutionMode::ReadOnly, ToolExecutionMode::Mutating);
        assert!(format!("{:?}", ToolExecutionMode::ReadOnly).contains("ReadOnly"));
    }

    #[test]
    fn capability_tag_unique() {
        let tags = vec![
            ToolCapabilityTag::ReadFiles,
            ToolCapabilityTag::WriteFiles,
            ToolCapabilityTag::ExecuteProcess,
            ToolCapabilityTag::NetworkAccess,
            ToolCapabilityTag::SearchWorkspace,
            ToolCapabilityTag::ReadImages,
        ];
        let mut deduped = tags.clone();
        deduped.sort_by_key(|t| format!("{t:?}"));
        deduped.dedup();
        assert_eq!(tags.len(), deduped.len());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolOutputMode {
    StructuredJson,
    Text,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolExecutionMode {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCapabilityTag {
    ReadFiles,
    WriteFiles,
    ExecuteProcess,
    NetworkAccess,
    SearchWorkspace,
    ReadImages,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: JsonSchema,
    pub output_mode: ToolOutputMode,
    pub execution_mode: ToolExecutionMode,
    pub capability_tags: Vec<ToolCapabilityTag>,
    pub supports_parallel: bool,
}
