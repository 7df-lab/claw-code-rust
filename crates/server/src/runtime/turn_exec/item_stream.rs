use std::sync::Arc;

use devo_core::{ItemId, SessionId, TextItem, TurnId, TurnItem};
use devo_protocol::native::item::{Item, PlanEntry, PlanStepStatus};

use super::super::ServerRuntime;
use super::super::proposed_plan::ProposedPlanSegment;
use crate::runtime::session_actor::state::SessionStreamState;
use crate::{ItemDeltaKind, ItemDeltaPayload, ServerEvent};

pub(super) async fn complete_reasoning_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: u64,
    text: String,
) {
    runtime
        .complete_native_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            Item::Reasoning {
                text: text.clone(),
                provider_payload_ref: None,
            },
            TurnItem::Reasoning(TextItem { text }),
        )
        .await;
}

pub(super) async fn complete_assistant_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: u64,
    text: String,
) {
    if text.trim().is_empty() {
        return;
    }
    runtime
        .complete_native_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            Item::AssistantMessage {
                text: text.clone(),
                phase: None,
            },
            TurnItem::AgentMessage(TextItem { text }),
        )
        .await;
}

#[derive(Debug, Default)]
pub(super) struct ProposedPlanStreamItem {
    item_id: Option<ItemId>,
    item_seq: Option<u64>,
    text: String,
    /// Native delta chunk counter for this item (L2-DES-APP-009 DD-2).
    delta_seq: u64,
}

impl ProposedPlanStreamItem {
    async fn start(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        if self.item_id.is_some() && self.item_seq.is_some() {
            return;
        }
        let (item_id, item_seq) = runtime
            .start_native_item(
                session_id,
                turn_id,
                Item::Plan {
                    entries: vec![PlanEntry {
                        step: String::new(),
                        status: PlanStepStatus::Completed,
                    }],
                },
            )
            .await;
        self.item_id = Some(item_id);
        self.item_seq = Some(item_seq);
    }

    async fn push_delta(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
        delta: String,
    ) {
        if delta.is_empty() {
            return;
        }
        self.start(runtime, session_id, turn_id).await;
        self.text.push_str(&delta);
        let chunk_index = self.delta_seq;
        self.delta_seq = self.delta_seq.saturating_add(1);
        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::PlanDelta,
                payload: ItemDeltaPayload {
                    context: crate::EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: self.item_id,
                        seq: 0,
                        item_seq: None,
                    },
                    delta,
                    stream_index: None,
                    channel: None,
                    chunk_index: Some(chunk_index),
                },
            })
            .await;
    }

    pub(super) async fn complete(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        let (Some(item_id), Some(item_seq)) = (self.item_id.take(), self.item_seq.take()) else {
            return;
        };
        let text = std::mem::take(&mut self.text);
        runtime
            .complete_native_item(
                session_id,
                turn_id,
                item_id,
                item_seq,
                Item::Plan {
                    entries: vec![PlanEntry {
                        step: text.clone(),
                        status: PlanStepStatus::Completed,
                    }],
                },
                TurnItem::Plan(TextItem { text }),
            )
            .await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn push_assistant_text_delta(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    text: String,
) {
    if text.is_empty() {
        return;
    }
    let (item_id, item_seq) = match (*assistant_item_id, *assistant_item_seq) {
        (Some(item_id), Some(item_seq)) => (item_id, item_seq),
        (None, None) => {
            let (item_id, item_seq) = runtime
                .start_native_item(
                    session_id,
                    turn_id,
                    Item::AssistantMessage {
                        text: String::new(),
                        phase: None,
                    },
                )
                .await;
            *assistant_item_id = Some(item_id);
            *assistant_item_seq = Some(item_seq);
            // A new item restarts the canonical delta chunk counter (DD-2).
            *assistant_delta_seq = 0;
            (item_id, item_seq)
        }
        _ => return,
    };
    assistant_text.push_str(&text);
    let chunk_index = *assistant_delta_seq;
    *assistant_delta_seq = (*assistant_delta_seq).saturating_add(1);
    let event = ServerEvent::ItemDelta {
        delta_kind: ItemDeltaKind::AgentMessageDelta,
        payload: ItemDeltaPayload {
            context: crate::EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
                item_seq: None,
            },
            delta: text,
            stream_index: None,
            channel: None,
            chunk_index: Some(chunk_index),
        },
    };
    // Fast path: avoid per-token registry scans and wait_agent buffer contention.
    runtime
        .broadcast_streaming_agent_message_delta(&event)
        .await;
    // Deferred assistant text is written once when the event stream drains.
    let _ = (event_stream, item_seq);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_proposed_plan_segments(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    segments: Vec<ProposedPlanSegment>,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    proposed_plan_item: &mut ProposedPlanStreamItem,
    leading_normal_buffer: &mut String,
) {
    for segment in segments {
        match segment {
            ProposedPlanSegment::Normal(delta) => {
                if delta.is_empty() {
                    continue;
                }
                if assistant_item_id.is_none() && delta.chars().all(char::is_whitespace) {
                    leading_normal_buffer.push_str(&delta);
                    continue;
                }
                let delta = if assistant_item_id.is_none() && !leading_normal_buffer.is_empty() {
                    format!("{}{}", std::mem::take(leading_normal_buffer), delta)
                } else {
                    delta
                };
                push_assistant_text_delta(
                    runtime,
                    event_stream,
                    session_id,
                    turn_id,
                    assistant_item_id,
                    assistant_item_seq,
                    assistant_text,
                    assistant_delta_seq,
                    delta,
                )
                .await;
            }
            ProposedPlanSegment::PlanStart => {
                leading_normal_buffer.clear();
                proposed_plan_item.start(runtime, session_id, turn_id).await;
            }
            ProposedPlanSegment::PlanDelta(delta) => {
                proposed_plan_item
                    .push_delta(runtime, session_id, turn_id, delta)
                    .await;
            }
            ProposedPlanSegment::PlanEnd => {}
        }
    }
}
