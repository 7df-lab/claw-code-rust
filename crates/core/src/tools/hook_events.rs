use serde_json::Map;
use serde_json::Value;

use crate::hooks::HookInput;
use crate::hooks::HookRuntimeContext;
use crate::tools::ToolCall;
use crate::tools::ToolCallResult;
use crate::tools::ToolContent;
use devo_config::HookEvent;

pub(super) async fn pre_tool_use_block_reason(
    hooks: Option<&HookRuntimeContext>,
    call: &ToolCall,
    tool_name: &str,
) -> Option<String> {
    let hooks = hooks?;
    let input = HookInput::new(
        &hooks.base,
        HookEvent::PreToolUse,
        tool_event_extra(call, tool_name),
    );
    hooks
        .runner
        .run(input)
        .await
        .first_blocking_reason()
        .map(str::to_string)
}

pub(super) async fn post_tool_use(
    hooks: Option<&HookRuntimeContext>,
    call: &ToolCall,
    tool_name: &str,
    result: &ToolCallResult,
) {
    let Some(hooks) = hooks else {
        return;
    };
    let mut extra = tool_event_extra(call, tool_name);
    extra.insert(
        "tool_response".to_string(),
        tool_content_value(&result.content),
    );
    let input = HookInput::new(&hooks.base, HookEvent::PostToolUse, extra);
    hooks.runner.run(input).await;
    for event in file_changed_events(result) {
        hooks
            .runner
            .run(HookInput::new(
                &hooks.base,
                HookEvent::FileChanged,
                event.into_extra(),
            ))
            .await;
    }
}

pub(super) async fn post_tool_use_failure(
    hooks: Option<&HookRuntimeContext>,
    call: &ToolCall,
    tool_name: &str,
    error: &str,
) {
    let Some(hooks) = hooks else {
        return;
    };
    let mut extra = tool_event_extra(call, tool_name);
    extra.insert("error".to_string(), Value::String(error.to_string()));
    let input = HookInput::new(&hooks.base, HookEvent::PostToolUseFailure, extra);
    hooks.runner.run(input).await;
}

fn tool_event_extra(call: &ToolCall, tool_name: &str) -> Map<String, Value> {
    Map::from_iter([
        (
            "tool_name".to_string(),
            Value::String(tool_name.to_string()),
        ),
        ("tool_input".to_string(), call.input.clone()),
        ("tool_use_id".to_string(), Value::String(call.id.clone())),
    ])
}

fn tool_content_value(content: &ToolContent) -> Value {
    match content {
        ToolContent::Text(text) => Value::String(text.clone()),
        ToolContent::Json(json) => json.clone(),
        ToolContent::Mixed { text, json } => {
            let mut value = Map::new();
            if let Some(text) = text {
                value.insert("text".to_string(), Value::String(text.clone()));
            }
            if let Some(json) = json {
                value.insert("json".to_string(), json.clone());
            }
            Value::Object(value)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileChangedEvent {
    file_path: String,
    event: &'static str,
}

impl FileChangedEvent {
    fn into_extra(self) -> Map<String, Value> {
        Map::from_iter([
            ("file_path".to_string(), Value::String(self.file_path)),
            ("event".to_string(), Value::String(self.event.to_string())),
        ])
    }
}

fn file_changed_events(result: &ToolCallResult) -> Vec<FileChangedEvent> {
    let Some(metadata) = result_metadata(&result.content) else {
        return Vec::new();
    };
    let Some(files) = metadata.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for file in files {
        push_file_changed_events(file, &mut events);
    }
    events
}

fn result_metadata(content: &ToolContent) -> Option<&Value> {
    match content {
        ToolContent::Json(json) => Some(json),
        ToolContent::Mixed {
            text: _,
            json: Some(json),
        } => Some(json),
        ToolContent::Text(_)
        | ToolContent::Mixed {
            text: _,
            json: None,
        } => None,
    }
}

fn push_file_changed_events(file: &Value, events: &mut Vec<FileChangedEvent>) {
    let Some(object) = file.as_object() else {
        return;
    };
    let kind = object
        .get("kind")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("change");
    if kind == "move" {
        if let Some(source) = file_path_field(object) {
            events.push(FileChangedEvent {
                file_path: source.to_string(),
                event: "unlink",
            });
        }
        if let Some(target) = object.get("movePath").and_then(Value::as_str) {
            events.push(FileChangedEvent {
                file_path: target.to_string(),
                event: "add",
            });
        }
        return;
    }
    let Some(file_path) = file_path_field(object) else {
        return;
    };
    let event = match kind {
        "add" => "add",
        "delete" | "remove" | "unlink" => "unlink",
        _ => "change",
    };
    events.push(FileChangedEvent {
        file_path: file_path.to_string(),
        event,
    });
}

fn file_path_field(object: &Map<String, Value>) -> Option<&str> {
    object
        .get("filePath")
        .or_else(|| object.get("path"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn file_changed_events_map_tool_metadata_to_hook_events() {
        let result = ToolCallResult {
            output_artifacts: Vec::new(),
            output_kind: crate::tools::ToolOutputKind::Ordinary,
            tool_use_id: "call-1".to_string(),
            content: ToolContent::Mixed {
                text: None,
                json: Some(serde_json::json!({
                    "files": [
                        {"filePath": "/tmp/new.txt", "kind": "add"},
                        {"filePath": "/tmp/old.txt", "kind": "delete"},
                        {"filePath": "/tmp/edit.txt", "kind": "update"}
                    ]
                })),
            },
            is_error: false,
            display_content: None,
        };

        assert_eq!(
            file_changed_events(&result),
            vec![
                FileChangedEvent {
                    file_path: "/tmp/new.txt".to_string(),
                    event: "add"
                },
                FileChangedEvent {
                    file_path: "/tmp/old.txt".to_string(),
                    event: "unlink"
                },
                FileChangedEvent {
                    file_path: "/tmp/edit.txt".to_string(),
                    event: "change"
                },
            ]
        );
    }
}
