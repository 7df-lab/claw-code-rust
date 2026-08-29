//! Session preview and input-history helpers for the TUI worker.

use std::collections::VecDeque;

use anyhow::Result;
use devo_core::SessionId;
use devo_server::StdioServerClient;

use crate::events::SessionPreviewMessage;
use crate::events::SessionPreviewRole;

const MAX_PREVIEW_MESSAGES: usize = 4;

pub(crate) async fn collect_user_input_texts(
    client: &mut StdioServerClient,
    session_id: SessionId,
) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    let mut cursor = None;
    loop {
        let page = client
            .session_items_list_native(session_id, cursor.clone(), Some(500))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for item in &page.data {
            if let devo_protocol::native::item::Item::UserMessage { content, .. } = &item.item {
                let text = content
                    .iter()
                    .filter_map(|input| match input {
                        devo_protocol::native::item::UserInput::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    texts.push(text);
                }
            }
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    Ok(texts)
}

/// Loads only the recent user/assistant dialogue needed by the inline resume picker.
pub(crate) async fn collect_session_preview(
    client: &mut StdioServerClient,
    session_id: SessionId,
) -> Result<Vec<SessionPreviewMessage>> {
    let mut messages = VecDeque::with_capacity(MAX_PREVIEW_MESSAGES);
    let mut cursor = None;
    loop {
        let page = client
            .session_items_list_native(session_id, cursor.clone(), Some(500))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for item in page.data {
            append_preview_item(&mut messages, item.item);
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    Ok(messages.into_iter().collect())
}

pub(crate) fn append_preview_item(
    messages: &mut VecDeque<SessionPreviewMessage>,
    item: devo_protocol::native::item::Item,
) {
    let message = match item {
        devo_protocol::native::item::Item::UserMessage { content, .. } => {
            let text = content
                .into_iter()
                .filter_map(|input| match input {
                    devo_protocol::native::item::UserInput::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(SessionPreviewMessage {
                role: SessionPreviewRole::User,
                text,
            })
        }
        devo_protocol::native::item::Item::AssistantMessage { text, .. } => {
            (!text.trim().is_empty()).then_some(SessionPreviewMessage {
                role: SessionPreviewRole::Assistant,
                text,
            })
        }
        devo_protocol::native::item::Item::Reasoning { .. }
        | devo_protocol::native::item::Item::Plan { .. }
        | devo_protocol::native::item::Item::ToolCall { .. }
        | devo_protocol::native::item::Item::ToolResult { .. }
        | devo_protocol::native::item::Item::CommandExecution { .. }
        | devo_protocol::native::item::Item::HostedToolCall { .. }
        | devo_protocol::native::item::Item::FileChange { .. }
        | devo_protocol::native::item::Item::Approval { .. }
        | devo_protocol::native::item::Item::UserInputRequest { .. }
        | devo_protocol::native::item::Item::SubAgent { .. }
        | devo_protocol::native::item::Item::BackgroundTask { .. }
        | devo_protocol::native::item::Item::ContextCompaction { .. }
        | devo_protocol::native::item::Item::GoalProgress { .. }
        | devo_protocol::native::item::Item::Warning { .. } => None,
    };
    if let Some(message) = message {
        if messages.len() == MAX_PREVIEW_MESSAGES {
            messages.pop_front();
        }
        messages.push_back(message);
    }
}
