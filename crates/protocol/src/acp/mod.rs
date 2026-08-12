mod auth;
mod capabilities;
mod client_io;
mod common;
mod content;
mod event_to_update;
mod schema_aliases;
mod session;
mod session_config;
mod session_mode;
mod session_update;
pub mod ts;

pub use auth::*;
pub use capabilities::*;
pub use client_io::*;
pub use common::*;
pub use content::*;
pub use schema_aliases::*;
pub use session::*;
pub use session_config::*;
pub use session_mode::*;
pub use session_update::*;

use crate::InputItem;

pub const ACP_INITIALIZE_METHOD: &str = "initialize";
pub const ACP_AUTHENTICATE_METHOD: &str = "authenticate";
pub const ACP_LOGOUT_METHOD: &str = "logout";
pub const ACP_SESSION_NEW_METHOD: &str = "session/new";
pub const ACP_SESSION_LIST_METHOD: &str = "session/list";
pub const ACP_SESSION_LOAD_METHOD: &str = "session/load";
pub const ACP_SESSION_RESUME_METHOD: &str = "session/resume";
pub const ACP_SESSION_CLOSE_METHOD: &str = "session/close";
pub const ACP_SESSION_DELETE_METHOD: &str = "session/delete";
pub const ACP_SESSION_PROMPT_METHOD: &str = "session/prompt";
pub const ACP_SESSION_CANCEL_METHOD: &str = "session/cancel";
pub const ACP_SESSION_UPDATE_METHOD: &str = "session/update";
pub const ACP_SESSION_REQUEST_PERMISSION_METHOD: &str = "session/request_permission";
pub const ACP_SESSION_SET_MODE_METHOD: &str = "session/set_mode";
pub const ACP_SESSION_SET_CONFIG_OPTION_METHOD: &str = "session/set_config_option";
pub const ACP_FS_READ_TEXT_FILE_METHOD: &str = "fs/read_text_file";
pub const ACP_FS_WRITE_TEXT_FILE_METHOD: &str = "fs/write_text_file";
pub const ACP_JSONRPC_VERSION: &str = "2.0";
pub const DEVO_ORIGINAL_METHOD_META: &str = "devo/originalMethod";
pub const DEVO_ORIGINAL_EVENT_META: &str = "devo/originalEvent";
pub const DEVO_SESSION_META: &str = "devo/session";
pub const DEVO_SESSION_RESUME_META: &str = "devo/sessionResume";
pub const DEVO_TURN_ID_META: &str = "devo/turnId";
pub const DEVO_ITEM_ID_META: &str = "devo/itemId";
pub const DEVO_ACTIVITY_AT_META: &str = "devo/activityAt";
pub const DEVO_HISTORY_INDEX_META: &str = "devo/historyIndex";
pub const DEVO_PARENT_MESSAGE_ID_META: &str = "devo/parentMessageId";
pub const DEVO_ITEM_KIND_META: &str = "devo/itemKind";
pub const DEVO_TURN_USAGE_META: &str = "devo/turnUsage";
/// Top-level `_meta` object key that carries devo extension capabilities as
/// a nested object, e.g. `_meta: { "devo": { "typedItems": true } }`.
pub const DEVO_EXTENSION_META: &str = "devo";
/// Capability key inside the `devo` extension meta object: the client opts
/// in to native typed `item/started` / `item/completed` notifications
/// carrying the Native `ItemEnvelope` (P2, 06-item-model step 2).
pub const DEVO_TYPED_ITEMS_META: &str = "typedItems";
/// Capability key inside the `devo` extension meta object: the client
/// declares which protocol surface colliding method names route to
/// (L2-DES-APP-008 / L2-DES-APP-009). `_meta: { "devo": { "protocol":
/// "native" } }` selects the Native protocol; absent or any other value
/// keeps the connection on the ACP adapter surface.
pub const DEVO_PROTOCOL_META: &str = "protocol";
/// Preferred wire value selecting the Native protocol surface.
pub const DEVO_PROTOCOL_NATIVE: &str = "native";

pub type AcpMeta = serde_json::Map<String, serde_json::Value>;

pub use event_to_update::acp_notification_from_server_event;
pub use event_to_update::original_event_from_acp_notification;

/// Returns whether the given `_meta` map opts in to typed item
/// notifications (`{ "devo": { "typedItems": true } }`).
pub fn devo_typed_items_opted_in(meta: Option<&AcpMeta>) -> bool {
    meta.and_then(|meta| meta.get(DEVO_EXTENSION_META))
        .and_then(|devo| devo.get(DEVO_TYPED_ITEMS_META))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Returns whether the given `_meta` map declares the Native protocol
/// surface (`{ "devo": { "protocol": "native" } }`).
pub fn devo_native_protocol_opted_in(meta: Option<&AcpMeta>) -> bool {
    meta.and_then(|meta| meta.get(DEVO_EXTENSION_META))
        .and_then(|devo| devo.get(DEVO_PROTOCOL_META))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value == DEVO_PROTOCOL_NATIVE)
}

pub fn input_items_from_acp_prompt(prompt: Vec<AcpContentBlock>) -> Result<Vec<InputItem>, String> {
    let mut input = Vec::new();
    for block in prompt {
        input.extend(block.into_input_items()?);
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::event_to_update::acp_update_from_server_event;
    use super::*;
    use crate::CommandExecutionPayload;
    use crate::EventContext;
    use crate::FileChangePayload;
    use crate::ItemDeltaKind;
    use crate::ItemDeltaPayload;
    use crate::ItemEventPayload;
    use crate::ItemId;
    use crate::ItemKind;
    use crate::ServerEvent;
    use crate::SessionId;
    use crate::ToolCallPayload;
    use crate::TurnId;
    fn turn_item_meta(turn_id: &TurnId, item_id: &ItemId) -> AcpMeta {
        AcpMeta::from_iter([
            (
                DEVO_TURN_ID_META.to_string(),
                serde_json::Value::String(turn_id.to_string()),
            ),
            (
                DEVO_ITEM_ID_META.to_string(),
                serde_json::Value::String(item_id.to_string()),
            ),
        ])
    }
    fn strip_activity_at(meta: &mut Option<AcpMeta>) {
        if let Some(meta) = meta {
            meta.remove(DEVO_ACTIVITY_AT_META);
        }
    }
    fn strip_update_activity_at(mut update: Option<AcpSessionUpdate>) -> Option<AcpSessionUpdate> {
        if let Some(update) = &mut update {
            match update {
                AcpSessionUpdate::UserMessageChunk { meta, .. }
                | AcpSessionUpdate::AgentMessageChunk { meta, .. }
                | AcpSessionUpdate::AgentThoughtChunk { meta, .. }
                | AcpSessionUpdate::ToolCall { meta, .. }
                | AcpSessionUpdate::ToolCallUpdate { meta, .. }
                | AcpSessionUpdate::Plan { meta, .. }
                | AcpSessionUpdate::AvailableCommandsUpdate { meta, .. }
                | AcpSessionUpdate::CurrentModeUpdate { meta, .. }
                | AcpSessionUpdate::ConfigOptionUpdate { meta, .. }
                | AcpSessionUpdate::SessionInfoUpdate { meta, .. }
                | AcpSessionUpdate::UsageUpdate { meta, .. } => strip_activity_at(meta),
            }
        }
        update
    }
    fn strip_json_activity_at(update: &mut serde_json::Value) {
        if let Some(meta) = update
            .get_mut("_meta")
            .and_then(serde_json::Value::as_object_mut)
        {
            meta.remove(DEVO_ACTIVITY_AT_META);
        }
    }
    fn assert_activity_at(update: &serde_json::Value) {
        let activity_at = update["_meta"][DEVO_ACTIVITY_AT_META]
            .as_str()
            .expect("activity timestamp");
        chrono::DateTime::parse_from_rfc3339(activity_at).expect("activity timestamp is RFC3339");
    }
    use super::event_to_update::file_change_tool_content;
    use super::event_to_update::tool_result_content;

    #[test]
    fn initialize_result_uses_acp_field_names() {
        let result = AcpInitializeResult {
            protocol_version: 1,
            agent_capabilities: AcpAgentCapabilities {
                prompt_capabilities: AcpPromptCapabilities {
                    embedded_context: true,
                    ..AcpPromptCapabilities::default()
                },
                ..AcpAgentCapabilities::default()
            },
            auth_methods: Vec::new(),
            agent_info: Some(AcpImplementation::new("devo", "1.2.3").with_title("Devo")),
            meta: None,
        };

        let json = serde_json::to_value(result).expect("serialize initialize result");

        assert_eq!(
            json,
            serde_json::json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": false,
                    "promptCapabilities": {
                        "image": false,
                        "audio": false,
                        "embeddedContext": true
                    },
                    "mcpCapabilities": {
                        "http": false,
                        "sse": false
                    },
                    "sessionCapabilities": {}
                },
                "agentInfo": {
                    "name": "devo",
                    "title": "Devo",
                    "version": "1.2.3"
                }
            })
        );
    }

    #[test]
    fn new_session_params_accepts_stdio_mcp_server_shape() {
        #[cfg(windows)]
        let cwd = r"C:\Users\user\project";
        #[cfg(windows)]
        let command = r"C:\mcp\filesystem.exe";
        #[cfg(unix)]
        let cwd = "/home/user/project";
        #[cfg(unix)]
        let command = "/path/to/mcp-server";

        let params: AcpNewSessionParams = serde_json::from_value(serde_json::json!({
            "cwd": cwd,
            "mcpServers": [
                {
                    "name": "filesystem",
                    "command": command,
                    "args": ["--stdio"],
                    "env": []
                }
            ]
        }))
        .expect("deserialize ACP session/new params");

        assert_eq!(
            params,
            AcpNewSessionParams {
                cwd: PathBuf::from(cwd),
                additional_directories: Vec::new(),
                mcp_servers: vec![AcpMcpServer::Stdio(AcpMcpServerStdio {
                    name: "filesystem".to_string(),
                    command: PathBuf::from(command),
                    args: vec!["--stdio".to_string()],
                    env: Vec::new(),
                    meta: None,
                })],
                meta: None,
            }
        );
    }

    #[test]
    fn mcp_servers_use_transport_type_discriminator() {
        let params: AcpNewSessionParams = serde_json::from_value(serde_json::json!({
            "cwd": std::env::current_dir().expect("current dir"),
            "mcpServers": [
                {
                    "type": "http",
                    "name": "api-server",
                    "url": "https://api.example.com/mcp",
                    "headers": [
                        {
                            "name": "Authorization",
                            "value": "Bearer token123"
                        }
                    ]
                },
                {
                    "type": "sse",
                    "name": "event-stream",
                    "url": "https://events.example.com/mcp",
                    "headers": [
                        {
                            "name": "X-API-Key",
                            "value": "apikey456"
                        }
                    ]
                }
            ]
        }))
        .expect("deserialize ACP HTTP/SSE MCP servers");

        assert_eq!(
            params.mcp_servers,
            vec![
                AcpMcpServer::Http(AcpMcpServerHttp {
                    transport_type: AcpMcpServerHttpType::Http,
                    name: "api-server".to_string(),
                    url: "https://api.example.com/mcp".to_string(),
                    headers: vec![AcpHttpHeader {
                        name: "Authorization".to_string(),
                        value: "Bearer token123".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
                AcpMcpServer::Sse(AcpMcpServerSse {
                    transport_type: AcpMcpServerSseType::Sse,
                    name: "event-stream".to_string(),
                    url: "https://events.example.com/mcp".to_string(),
                    headers: vec![AcpHttpHeader {
                        name: "X-API-Key".to_string(),
                        value: "apikey456".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
            ]
        );
        assert_eq!(
            serde_json::to_value(params.mcp_servers).expect("serialize MCP servers"),
            serde_json::json!([
                {
                    "type": "http",
                    "name": "api-server",
                    "url": "https://api.example.com/mcp",
                    "headers": [
                        {
                            "name": "Authorization",
                            "value": "Bearer token123"
                        }
                    ]
                },
                {
                    "type": "sse",
                    "name": "event-stream",
                    "url": "https://events.example.com/mcp",
                    "headers": [
                        {
                            "name": "X-API-Key",
                            "value": "apikey456"
                        }
                    ]
                }
            ])
        );
    }

    #[test]
    fn content_blocks_use_acp_content_shapes() {
        let text: AcpContentBlock = serde_json::from_value(serde_json::json!({
            "type": "text",
            "text": "hello",
            "annotations": {
                "audience": ["user"],
                "lastModified": "2026-06-17T00:00:00Z",
                "priority": 0.7
            }
        }))
        .expect("deserialize text content");
        assert_eq!(
            text,
            AcpContentBlock::Text {
                annotations: Some(AcpAnnotations {
                    audience: Some(vec![AcpRole::User]),
                    last_modified: Some("2026-06-17T00:00:00Z".to_string()),
                    priority: Some(0.7),
                    meta: None,
                }),
                text: "hello".to_string(),
                meta: None,
            }
        );

        assert_eq!(
            serde_json::to_value(AcpContentBlock::Image {
                annotations: None,
                data: "iVBORw0KGgo=".to_string(),
                mime_type: "image/png".to_string(),
                uri: Some("file:///tmp/image.png".to_string()),
                meta: None,
            })
            .expect("serialize image content"),
            serde_json::json!({
                "type": "image",
                "data": "iVBORw0KGgo=",
                "mimeType": "image/png",
                "uri": "file:///tmp/image.png"
            })
        );

        assert_eq!(
            serde_json::from_value::<AcpContentBlock>(serde_json::json!({
                "type": "audio",
                "data": "UklGRg==",
                "mimeType": "audio/wav"
            }))
            .expect("deserialize audio content"),
            AcpContentBlock::Audio {
                annotations: None,
                data: "UklGRg==".to_string(),
                mime_type: "audio/wav".to_string(),
                meta: None,
            }
        );

        assert_eq!(
            serde_json::to_value(AcpContentBlock::ResourceLink {
                annotations: None,
                uri: "file:///tmp/document.pdf".to_string(),
                name: "document.pdf".to_string(),
                title: Some("Document".to_string()),
                description: Some("A PDF".to_string()),
                mime_type: Some("application/pdf".to_string()),
                size: Some(1024),
                meta: None,
            })
            .expect("serialize resource link"),
            serde_json::json!({
                "type": "resource_link",
                "uri": "file:///tmp/document.pdf",
                "name": "document.pdf",
                "title": "Document",
                "description": "A PDF",
                "mimeType": "application/pdf",
                "size": 1024
            })
        );
    }

    #[test]
    fn embedded_resource_union_accepts_acp_extension_fields() {
        let value = serde_json::json!({
            "type": "resource",
            "resource": {
                "uri": "file:///tmp/data.bin",
                "text": "hello",
                "blob": "AA==",
                "provider": "mcp"
            }
        });

        assert!(serde_json::from_value::<AcpContentBlock>(value).is_ok());
    }

    #[test]
    fn acp_prompt_conversion_rejects_unadvertised_image_and_preserves_blob_resource() {
        let error = input_items_from_acp_prompt(vec![AcpContentBlock::Image {
            annotations: None,
            data: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
            uri: None,
            meta: None,
        }])
        .expect_err("image prompt content should be rejected");
        assert_eq!(
            error,
            "session/prompt image content is not supported by this agent"
        );

        assert_eq!(
            input_items_from_acp_prompt(vec![AcpContentBlock::Resource {
                annotations: None,
                resource: AcpEmbeddedResource::Blob(AcpBlobResourceContents {
                    uri: "file:///tmp/data.bin".to_string(),
                    mime_type: Some("application/octet-stream".to_string()),
                    blob: "AA==".to_string(),
                    meta: None,
                }),
                meta: None,
            }])
            .expect("blob resource converts to prompt text"),
            vec![InputItem::Text {
                text: "Resource file:///tmp/data.bin (application/octet-stream; base64):\nAA=="
                    .to_string()
            }]
        );
    }

    #[test]
    fn tool_result_content_preserves_acp_content_blocks() {
        assert_eq!(
            tool_result_content(
                None,
                serde_json::json!({
                    "content": [
                        {
                            "type": "image",
                            "data": "iVBORw0KGgo=",
                            "mimeType": "image/png"
                        }
                    ]
                })
            ),
            vec![AcpToolCallContent::content(AcpContentBlock::Image {
                annotations: None,
                data: "iVBORw0KGgo=".to_string(),
                mime_type: "image/png".to_string(),
                uri: None,
                meta: None,
            })]
        );
    }

    #[test]
    fn tool_call_updates_round_trip_full_wire_shape() {
        let path = PathBuf::from("/workspace/src/main.rs");
        let path_json = serde_json::to_value(&path).expect("serialize path");
        let tool_call = AcpSessionUpdate::ToolCall {
            tool_call_id: "call-1".to_string(),
            title: "Read file".to_string(),
            kind: Some(AcpToolKind::Read),
            status: Some(AcpToolCallStatus::Pending),
            raw_input: Some(serde_json::json!({ "path": path_json.clone() })),
            raw_output: Some(serde_json::json!({ "ok": true })),
            content: vec![AcpToolCallContent::content(AcpContentBlock::text(
                "reading",
            ))],
            locations: vec![AcpToolCallLocation {
                path: path.clone(),
                line: Some(7),
                meta: None,
            }],
            meta: None,
        };
        let value = serde_json::to_value(&tool_call).expect("serialize tool call");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "call-1",
                "title": "Read file",
                "kind": "read",
                "status": "pending",
                "rawInput": { "path": path_json.clone() },
                "rawOutput": { "ok": true },
                "content": [
                    {
                        "type": "content",
                        "content": {
                            "type": "text",
                            "text": "reading"
                        }
                    }
                ],
                "locations": [
                    {
                        "path": path_json.clone(),
                        "line": 7
                    }
                ]
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpSessionUpdate>(value).expect("deserialize tool call"),
            tool_call
        );

        let minimal = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-2",
            "title": "Read file"
        });
        let parsed = serde_json::from_value::<AcpSessionUpdate>(minimal)
            .expect("deserialize minimal tool call");
        assert_eq!(
            parsed,
            AcpSessionUpdate::ToolCall {
                tool_call_id: "call-2".to_string(),
                title: "Read file".to_string(),
                kind: None,
                status: None,
                raw_input: None,
                raw_output: None,
                content: Vec::new(),
                locations: Vec::new(),
                meta: None,
            }
        );

        let update = AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: "call-1".to_string(),
            title: Some("Updated file".to_string()),
            kind: Some(AcpToolKind::Edit),
            status: Some(AcpToolCallStatus::Completed),
            raw_input: Some(serde_json::json!({ "path": path_json.clone() })),
            raw_output: Some(serde_json::json!({ "changed": true })),
            content: Some(vec![AcpToolCallContent::Diff {
                path: path.clone(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                meta: None,
            }]),
            locations: Some(vec![AcpToolCallLocation {
                path: path.clone(),
                line: None,
                meta: None,
            }]),
            meta: None,
        };
        let value = serde_json::to_value(&update).expect("serialize tool call update");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "title": "Updated file",
                "kind": "edit",
                "status": "completed",
                "rawInput": { "path": path_json.clone() },
                "rawOutput": { "changed": true },
                "content": [
                    {
                        "type": "diff",
                        "path": path_json.clone(),
                        "oldText": "old\n",
                        "newText": "new\n"
                    }
                ],
                "locations": [
                    {
                        "path": path_json
                    }
                ]
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpSessionUpdate>(value)
                .expect("deserialize tool call update"),
            update
        );
    }

    #[test]
    fn tool_call_update_accepts_nullable_collections_and_terminal_content() {
        let update: AcpToolCallUpdate = serde_json::from_value(serde_json::json!({
            "toolCallId": "call-1",
            "content": null,
            "locations": null
        }))
        .expect("deserialize nullable ACP tool-call update");
        assert_eq!(update.content, None);
        assert_eq!(update.locations, None);

        assert_eq!(
            serde_json::to_value(AcpToolCallContent::Terminal {
                terminal_id: "terminal-1".to_string(),
                meta: None,
            })
            .expect("serialize terminal tool content"),
            serde_json::json!({
                "type": "terminal",
                "terminalId": "terminal-1"
            })
        );
    }

    #[test]
    fn session_update_variants_round_trip_documented_wire_names() {
        let commands = AcpSessionUpdate::AvailableCommandsUpdate {
            available_commands: vec![AcpAvailableCommand {
                name: "create_plan".to_string(),
                description: "Create a plan".to_string(),
                input: Some(AcpAvailableCommandInput {
                    hint: "task".to_string(),
                    meta: None,
                }),
                meta: None,
            }],
            meta: Some(serde_json::Map::from_iter([(
                "trace".to_string(),
                serde_json::json!(true),
            )])),
        };
        let commands_value =
            serde_json::to_value(&commands).expect("serialize available commands update");
        assert_eq!(
            commands_value,
            serde_json::json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    {
                        "name": "create_plan",
                        "description": "Create a plan",
                        "input": {
                            "hint": "task"
                        }
                    }
                ],
                "_meta": {
                    "trace": true
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpSessionUpdate>(commands_value)
                .expect("deserialize available commands update"),
            commands
        );

        let current_mode = AcpSessionUpdate::CurrentModeUpdate {
            current_mode_id: "build".to_string(),
            meta: None,
        };
        assert_eq!(
            serde_json::to_value(&current_mode).expect("serialize current mode update"),
            serde_json::json!({
                "sessionUpdate": "current_mode_update",
                "currentModeId": "build"
            })
        );

        let config_update = AcpSessionUpdate::ConfigOptionUpdate {
            config_options: vec![crate::AcpSessionConfigOption::Select {
                id: "model".to_string(),
                name: "Model".to_string(),
                description: Some("Controls the model used for this session".to_string()),
                category: Some(crate::AcpSessionConfigOptionCategory::Known(
                    crate::AcpSessionConfigOptionCategoryKnown::Model,
                )),
                current_value: "default".to_string(),
                options: crate::AcpSessionConfigSelectOptions::Ungrouped(Vec::new()),
                meta: None,
            }],
            meta: None,
        };
        assert_eq!(
            serde_json::to_value(&config_update).expect("serialize config option update"),
            serde_json::json!({
                "sessionUpdate": "config_option_update",
                "configOptions": [
                    {
                        "type": "select",
                        "id": "model",
                        "name": "Model",
                        "description": "Controls the model used for this session",
                        "category": "model",
                        "currentValue": "default",
                        "options": []
                    }
                ]
            })
        );

        let usage = AcpSessionUpdate::UsageUpdate {
            used: 42,
            size: 200_000,
            cost: Some(AcpCost {
                amount: 1.25,
                currency: "USD".to_string(),
                meta: None,
            }),
            meta: None,
        };
        assert_eq!(
            serde_json::to_value(&usage).expect("serialize usage update"),
            serde_json::json!({
                "sessionUpdate": "usage_update",
                "used": 42,
                "size": 200000,
                "cost": {
                    "amount": 1.25,
                    "currency": "USD"
                }
            })
        );
    }

    #[test]
    fn file_change_item_emits_acp_diff_content_and_locations() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let path = PathBuf::from("/workspace/src/lib.rs");
        let raw_input = serde_json::json!({
            "path": serde_json::to_value(&path).expect("serialize path"),
        });
        let payload_value = serde_json::to_value(FileChangePayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("apply_patch".to_string()),
            input: Some(raw_input.clone()),
            changes: vec![(
                path.clone(),
                crate::protocol::FileChange::Add {
                    content: "hello\n".to_string(),
                },
            )],
            is_error: false,
        })
        .expect("serialize file change payload");
        let event = ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 1,
                item_seq: None,
            },
            item: crate::ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: ItemKind::FileChange,
                payload: payload_value.clone(),
            },
        });

        assert_eq!(
            strip_update_activity_at(acp_update_from_server_event(&event)),
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: "call-1".to_string(),
                title: Some("apply_patch".to_string()),
                kind: Some(AcpToolKind::Edit),
                status: Some(AcpToolCallStatus::Completed),
                raw_input: Some(raw_input),
                raw_output: Some(payload_value),
                content: Some(vec![AcpToolCallContent::Diff {
                    path: path.clone(),
                    old_text: None,
                    new_text: "hello\n".to_string(),
                    meta: None,
                }]),
                locations: Some(vec![AcpToolCallLocation {
                    path,
                    line: None,
                    meta: None,
                }]),
                meta: Some(turn_item_meta(&turn_id, &item_id)),
            })
        );
    }

    #[test]
    fn file_change_update_emits_text_content_when_old_new_text_is_unavailable() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let unified_diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let change = FileChangePayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("apply_patch".to_string()),
            input: None,
            changes: vec![(
                path,
                crate::protocol::FileChange::Update {
                    unified_diff: unified_diff.to_string(),
                    old_text: None,
                    new_text: None,
                    move_path: None,
                },
            )],
            is_error: false,
        };

        assert_eq!(
            file_change_tool_content(&change),
            vec![AcpToolCallContent::content(AcpContentBlock::text(
                unified_diff
            ))]
        );
    }

    #[test]
    fn file_change_update_emits_acp_diff_when_old_new_text_is_available() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let change = FileChangePayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("write".to_string()),
            input: None,
            changes: vec![(
                path.clone(),
                crate::protocol::FileChange::Update {
                    unified_diff: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"
                        .to_string(),
                    old_text: Some("old\n".to_string()),
                    new_text: Some("new\n".to_string()),
                    move_path: None,
                },
            )],
            is_error: false,
        };

        assert_eq!(
            file_change_tool_content(&change),
            vec![AcpToolCallContent::Diff {
                path,
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                meta: None,
            }]
        );
    }

    #[test]
    fn command_execution_completion_emits_text_content() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let payload_value = serde_json::to_value(CommandExecutionPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "exec_command".to_string(),
            command: "cargo test".to_string(),
            input: Some(serde_json::json!({"cmd": "cargo test"})),
            source: crate::protocol::ExecCommandSource::Agent,
            command_actions: Vec::new(),
            output: Some(serde_json::Value::String("tests passed\n".to_string())),
            is_error: false,
        })
        .expect("serialize command execution payload");
        let event = ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 1,
                item_seq: None,
            },
            item: crate::ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: ItemKind::CommandExecution,
                payload: payload_value.clone(),
            },
        });

        assert_eq!(
            strip_update_activity_at(acp_update_from_server_event(&event)),
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: "call-1".to_string(),
                title: Some("cargo test".to_string()),
                kind: Some(AcpToolKind::Execute),
                status: Some(AcpToolCallStatus::Completed),
                raw_input: Some(serde_json::json!({"cmd": "cargo test"})),
                raw_output: Some(serde_json::Value::String("tests passed\n".to_string())),
                content: Some(vec![AcpToolCallContent::content(AcpContentBlock::text(
                    "tests passed\n"
                ))]),
                locations: None,
                meta: Some(turn_item_meta(&turn_id, &item_id)),
            })
        );
    }

    #[test]
    fn usage_update_size_uses_context_window() {
        let session_id = SessionId::new();
        let payload = crate::TurnUsageUpdatedPayload {
            session_id,
            turn_id: TurnId::new(),
            usage: crate::TurnUsage {
                input_tokens: 3,
                output_tokens: 4,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            },
            total_input_tokens: 30,
            total_output_tokens: 12,
            total_tokens: 42,
            total_cache_read_tokens: 0,
            last_query_input_tokens: 3,
            context_window: Some(200_000),
        };
        let event = ServerEvent::TurnUsageUpdated(payload.clone());

        let (_, value) = acp_notification_from_server_event("turn/usage/updated", &event);

        assert_eq!(
            value["update"]["sessionUpdate"],
            serde_json::json!("usage_update")
        );
        assert_eq!(value["update"]["used"], serde_json::json!(42));
        assert_eq!(value["update"]["size"], serde_json::json!(200_000));
        let actual_payload = serde_json::from_value::<crate::TurnUsageUpdatedPayload>(
            value["update"]["_meta"][DEVO_TURN_USAGE_META].clone(),
        )
        .expect("usage update should preserve Devo turn usage payload");
        assert_eq!(actual_payload, payload);
    }

    #[test]
    fn tool_item_started_preserves_original_event_for_legacy_clients() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let started = ServerEvent::ItemStarted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq: None,
            },
            item: crate::ItemEnvelope {
                item_id,
                item_kind: ItemKind::ToolCall,
                payload: serde_json::to_value(ToolCallPayload {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "code_search".to_string(),
                    parameters: serde_json::json!({
                        "operation": "search",
                        "query": "context length display",
                        "path": "."
                    }),
                    command_actions: vec![crate::parse_command::ParsedCommand::Search {
                        cmd: "code_search context length display in .".to_string(),
                        query: Some("context length display".to_string()),
                        path: Some(".".to_string()),
                    }],
                })
                .expect("serialize tool payload"),
            },
        });

        let (method, value) = acp_notification_from_server_event("item/started", &started);
        let notification: AcpSessionNotification =
            serde_json::from_value(value.clone()).expect("deserialize ACP notification");

        assert_eq!(method, ACP_SESSION_UPDATE_METHOD);
        assert_eq!(
            value["update"]["sessionUpdate"],
            serde_json::json!("tool_call")
        );
        assert_eq!(
            original_event_from_acp_notification(&notification),
            Some(("item/started".to_string(), started))
        );
    }

    #[test]
    fn tool_status_maps_pending_then_in_progress_update() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let started = ServerEvent::ItemStarted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq: None,
            },
            item: crate::ItemEnvelope {
                item_id,
                item_kind: ItemKind::ToolCall,
                payload: serde_json::to_value(ToolCallPayload {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "read".to_string(),
                    parameters: serde_json::json!({"path": "src/lib.rs"}),
                    command_actions: Vec::new(),
                })
                .expect("serialize tool payload"),
            },
        });
        let (_, started_value) = acp_notification_from_server_event("item/started", &started);
        let mut started_update = started_value["update"].clone();
        assert_activity_at(&started_update);
        strip_json_activity_at(&mut started_update);

        assert_eq!(started_update["status"], serde_json::json!("pending"));
        assert_eq!(
            started_update["_meta"],
            serde_json::json!({
                "devo/turnId": turn_id.to_string(),
                "devo/itemId": item_id.to_string()
            })
        );
        assert!(
            started_value["_meta"]
                .get(DEVO_ORIGINAL_METHOD_META)
                .is_some(),
            "tool item/started should keep original method for legacy clients"
        );

        let update = ServerEvent::ToolCallStatusUpdated(crate::ToolCallStatusUpdatedPayload {
            session_id,
            turn_id,
            tool_call_id: "call-1".to_string(),
            status: "in_progress".to_string(),
        });
        let (_, update_value) =
            acp_notification_from_server_event("tool_call/status_updated", &update);
        let mut update_json = update_value["update"].clone();
        assert_activity_at(&update_json);
        strip_json_activity_at(&mut update_json);

        assert_eq!(
            update_json,
            serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": "in_progress",
                "_meta": {
                    "devo/turnId": turn_id.to_string()
                }
            })
        );
    }

    #[test]
    fn native_session_update_omits_devo_event_meta() {
        let session_id = SessionId::new();
        let item_id = ItemId::new();
        let event = ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: None,
                    item_id: Some(item_id),
                    seq: 7,
                    item_seq: None,
                },
                delta: "hello".to_string(),
                stream_index: None,
                channel: None,
                chunk_index: None,
            },
        };

        let (method, value) = acp_notification_from_server_event("item/agentMessage/delta", &event);
        let notification: AcpSessionNotification =
            serde_json::from_value(value.clone()).expect("deserialize ACP notification");

        assert_eq!(method, ACP_SESSION_UPDATE_METHOD);
        let mut update_json = value["update"].clone();
        assert_activity_at(&update_json);
        strip_json_activity_at(&mut update_json);
        assert_eq!(
            update_json,
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": "hello"
                },
                "messageId": item_id.to_string(),
                "_meta": {
                    "devo/itemId": item_id.to_string()
                }
            })
        );
        assert_eq!(value.get("_meta"), None);
        assert_eq!(original_event_from_acp_notification(&notification), None);

        let reasoning_item_id = ItemId::new();
        let reasoning = ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ReasoningTextDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: None,
                    item_id: Some(reasoning_item_id),
                    seq: 8,
                    item_seq: None,
                },
                delta: "thinking".to_string(),
                stream_index: None,
                channel: None,
                chunk_index: None,
            },
        };

        let (method, value) =
            acp_notification_from_server_event("item/reasoning/textDelta", &reasoning);
        let notification: AcpSessionNotification =
            serde_json::from_value(value.clone()).expect("deserialize ACP notification");

        assert_eq!(method, ACP_SESSION_UPDATE_METHOD);
        let mut update_json = value["update"].clone();
        assert_activity_at(&update_json);
        strip_json_activity_at(&mut update_json);
        assert_eq!(
            update_json,
            serde_json::json!({
                "sessionUpdate": "agent_thought_chunk",
                "content": {
                    "type": "text",
                    "text": "thinking"
                },
                "messageId": reasoning_item_id.to_string(),
                "_meta": {
                    "devo/itemId": reasoning_item_id.to_string()
                }
            })
        );
        assert_eq!(value.get("_meta"), None);
        assert_eq!(original_event_from_acp_notification(&notification), None);
    }
}
