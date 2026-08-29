use std::sync::Arc;

use devo_core::{ItemId, SessionId, TurnId};

use super::super::ServerRuntime;
use super::super::proposed_plan::ProposedPlanParser;
use super::context_compaction::ContextCompactionLifecycle;
use super::item_stream::{
    ProposedPlanStreamItem, complete_assistant_item, complete_reasoning_item,
    handle_proposed_plan_segments, push_assistant_text_delta,
};
use super::tool_display::{
    command_display_from_input, tool_start_item_from_input, tool_start_item_from_result,
};
use super::tool_results;
use super::tool_results::complete_pending_tool_call;
use super::trace::{
    QueryEventDeliveryPolicy, query_event_delivery_policy, query_event_trace_delta_len,
    query_event_trace_kind, query_event_trace_token_preview, stream_trace_elapsed_ms,
};
use super::types::{PendingToolCall, ToolDisplayKind, TurnEventStreamSummary};
use crate::runtime::session_actor::state::SessionStreamState;
use crate::{ItemDeltaKind, ItemDeltaPayload, ServerEvent};
use tokio::sync::mpsc;

pub(crate) const QUERY_EVENT_CHANNEL_CAPACITY: usize = 8192;

/// Enqueue a query event into the turn event stream.
///
/// Token deltas and lifecycle/control events must not be dropped: when the
/// channel is full they apply backpressure to the producer. High-volume tool
/// progress and usage updates remain best effort so they cannot wedge the turn
/// forever on a stalled consumer.
pub(super) async fn enqueue_query_event(
    event_tx: &mpsc::Sender<devo_core::QueryEvent>,
    event: devo_core::QueryEvent,
) {
    let kind = query_event_trace_kind(&event);
    if query_event_delivery_policy(&event) == QueryEventDeliveryPolicy::MustDeliver {
        let _ = event_tx.send(event).await;
        return;
    }
    match event_tx.try_send(event) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                capacity = QUERY_EVENT_CHANNEL_CAPACITY,
                event_kind = kind,
                "dropping query event because the turn event channel is full"
            );
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_turn_event_stream(
    runtime: Arc<ServerRuntime>,
    event_stream: Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn: crate::TurnMetadata,
    collaboration_mode: devo_protocol::CollaborationMode,
    event_tool_registry: Arc<devo_core::tools::ToolRegistry>,
    usage_parent_session_id: Option<SessionId>,
    usage_context_window: Option<u64>,
    mut event_rx: tokio::sync::mpsc::Receiver<devo_core::QueryEvent>,
) -> tokio::task::JoinHandle<TurnEventStreamSummary> {
    let turn_for_events = turn.clone();
    let turn_for_plan_updates = turn;
    tokio::spawn(async move {
        let mut assistant_item_id = None;
        let mut assistant_item_seq = None;
        let mut assistant_delta_seq = 0_u64;
        let mut assistant_text = String::new();
        let mut reasoning_item_id = None;
        let mut reasoning_item_seq = None;
        let mut reasoning_text = String::new();
        // Native delta chunk counters (L2-DES-APP-009 DD-2): per item,
        // reset when a new item starts.
        let mut reasoning_delta_seq = 0_u64;
        let mut command_output_delta_seqs: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut tool_input_delta_seqs: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut tool_names_by_id = std::collections::HashMap::new();
        let mut pending_tool_calls: std::collections::HashMap<String, PendingToolCall> =
            std::collections::HashMap::new();
        let mut proposed_plan_parser = (collaboration_mode
            == devo_protocol::CollaborationMode::Plan)
            .then(ProposedPlanParser::default);
        let mut proposed_plan_item = ProposedPlanStreamItem::default();
        let mut proposed_plan_leading_normal = String::new();
        let mut turn_usage = None;
        let mut latest_query_usage = None;
        let mut stop_reason = None;
        let mut context_compaction = ContextCompactionLifecycle::default();
        while let Some(event) = event_rx.recv().await {
            log_dequeued_query_event(&event);
            match event {
                devo_core::QueryEvent::ProviderRetryStatus(status) => {
                    runtime
                        .broadcast_event(ServerEvent::TurnProviderRetryStatus(
                            devo_protocol::TurnProviderRetryStatusPayload {
                                session_id,
                                turn_id: turn_for_events.turn_id,
                                attempt: status.attempt,
                                max_attempts: u32::try_from(status.max_attempts).ok(),
                                backoff_ms: status.backoff_ms,
                                provider: status.provider,
                                model: status.model,
                                phase: match status.phase {
                                    devo_core::QueryProviderRetryPhase::Scheduled => {
                                        devo_protocol::ProviderRetryPhase::Scheduled
                                    }
                                    devo_core::QueryProviderRetryPhase::Resumed => {
                                        devo_protocol::ProviderRetryPhase::Resumed
                                    }
                                },
                                message: status.message,
                            },
                        ))
                        .await;
                }
                devo_core::QueryEvent::ContextCompactionStarted => {
                    context_compaction
                        .start(&runtime, session_id, turn_for_events.turn_id)
                        .await;
                }
                devo_core::QueryEvent::ContextCompactionCompleted { compacted_items } => {
                    context_compaction
                        .complete(
                            &runtime,
                            session_id,
                            turn_for_events.turn_id,
                            compacted_items,
                        )
                        .await;
                }
                devo_core::QueryEvent::ContextCompactionFailed { message } => {
                    context_compaction
                        .fail(&runtime, session_id, turn_for_events.turn_id, message)
                        .await;
                }
                devo_core::QueryEvent::TextDelta(text) => {
                    if let Some(parser) = proposed_plan_parser.as_mut() {
                        let segments = parser.push_str(&text);
                        handle_proposed_plan_segments(
                            &runtime,
                            &event_stream,
                            session_id,
                            turn_for_events.turn_id,
                            segments,
                            &mut assistant_item_id,
                            &mut assistant_item_seq,
                            &mut assistant_text,
                            &mut assistant_delta_seq,
                            &mut proposed_plan_item,
                            &mut proposed_plan_leading_normal,
                        )
                        .await;
                    } else {
                        push_assistant_text_delta(
                            &runtime,
                            &event_stream,
                            session_id,
                            turn_for_events.turn_id,
                            &mut assistant_item_id,
                            &mut assistant_item_seq,
                            &mut assistant_text,
                            &mut assistant_delta_seq,
                            text,
                        )
                        .await;
                    }
                }
                devo_core::QueryEvent::ReasoningDelta(text) => {
                    handle_reasoning_delta(
                        &runtime,
                        &event_stream,
                        session_id,
                        turn_for_events.turn_id,
                        text,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                        &mut reasoning_delta_seq,
                    )
                    .await;
                }
                devo_core::QueryEvent::ReasoningCompleted => {
                    complete_open_reasoning_item(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                        &event_stream,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolUseStart { id, name, input } => {
                    handle_tool_use_start(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        id,
                        name,
                        input,
                        &mut tool_names_by_id,
                        &mut pending_tool_calls,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                        &mut assistant_item_id,
                        &mut assistant_item_seq,
                        &mut assistant_text,
                        &event_tool_registry,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolUseInputDelta { id, partial_json } => {
                    handle_tool_input_delta(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        id,
                        partial_json,
                        &pending_tool_calls,
                        &mut tool_input_delta_seqs,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolExecutionStart { id } => {
                    runtime
                        .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                            devo_protocol::ToolCallStatusUpdatedPayload {
                                session_id,
                                turn_id: turn_for_events.turn_id,
                                tool_call_id: id,
                                status: "in_progress".to_string(),
                            },
                        ))
                        .await;
                }
                devo_core::QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name: final_tool_name,
                    input: final_input,
                    content,
                    display_content,
                    is_error,
                    summary,
                } => {
                    handle_tool_result(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        &turn_for_plan_updates,
                        tool_use_id,
                        final_tool_name,
                        final_input,
                        content,
                        display_content,
                        is_error,
                        summary,
                        &tool_names_by_id,
                        &mut pending_tool_calls,
                        &event_tool_registry,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolProgress {
                    tool_use_id,
                    progress,
                } => {
                    handle_tool_progress(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        tool_use_id,
                        progress,
                        &pending_tool_calls,
                        &mut command_output_delta_seqs,
                    )
                    .await;
                }
                devo_core::QueryEvent::UsageDelta { usage } => {
                    let usage = devo_core::TurnUsage::from_usage(&usage);
                    turn_usage = Some(usage.clone());
                    latest_query_usage = Some(usage.clone());
                    let kind = super::super::subagent_usage::UsageUpdateKind::InFlight;
                    if usage_parent_session_id.is_some() {
                        let _ = runtime
                            .publish_subagent_turn_usage(
                                session_id,
                                turn_for_events.turn_id,
                                usage,
                                kind,
                            )
                            .await;
                    } else if let Some(snapshot) = runtime
                        .publish_parent_turn_usage(
                            session_id,
                            turn_for_events.turn_id,
                            usage,
                            usage_context_window,
                            kind,
                        )
                        .await
                    {
                        turn_usage = Some(snapshot.turn_usage.to_turn_usage());
                        latest_query_usage = Some(snapshot.latest_query_usage.to_turn_usage());
                    }
                }
                devo_core::QueryEvent::Usage { usage } => {
                    let usage = devo_core::TurnUsage::from_usage(&usage);
                    turn_usage = Some(usage.clone());
                    latest_query_usage = Some(usage.clone());
                    let kind = super::super::subagent_usage::UsageUpdateKind::CompletedLeg;
                    if usage_parent_session_id.is_some() {
                        let _ = runtime
                            .publish_subagent_turn_usage(
                                session_id,
                                turn_for_events.turn_id,
                                usage,
                                kind,
                            )
                            .await;
                    } else if let Some(snapshot) = runtime
                        .publish_parent_turn_usage(
                            session_id,
                            turn_for_events.turn_id,
                            usage,
                            usage_context_window,
                            kind,
                        )
                        .await
                    {
                        turn_usage = Some(snapshot.turn_usage.to_turn_usage());
                        latest_query_usage = Some(snapshot.latest_query_usage.to_turn_usage());
                    }
                }
                devo_core::QueryEvent::TurnComplete {
                    stop_reason: terminal_stop_reason,
                } => {
                    stop_reason = Some(terminal_stop_reason);
                }
            }
        }
        context_compaction
            .close_if_open(&runtime, session_id, turn_for_events.turn_id)
            .await;
        finish_proposed_plan_stream(
            &runtime,
            &event_stream,
            session_id,
            turn_for_events.turn_id,
            &mut proposed_plan_parser,
            &mut assistant_item_id,
            &mut assistant_item_seq,
            &mut assistant_text,
            &mut assistant_delta_seq,
            &mut proposed_plan_item,
            &mut proposed_plan_leading_normal,
        )
        .await;
        {
            let mut stream = event_stream.lock().await;
            if let (Some(item_id), Some(item_seq)) = (assistant_item_id, assistant_item_seq) {
                stream.deferred_assistant =
                    Some((item_id, item_seq, std::mem::take(&mut assistant_text)));
            }
            if let (Some(item_id), Some(item_seq)) = (reasoning_item_id, reasoning_item_seq) {
                stream.deferred_reasoning =
                    Some((item_id, item_seq, std::mem::take(&mut reasoning_text)));
            }
        }
        complete_deferred_stream_items(
            &runtime,
            &event_stream,
            session_id,
            turn_for_events.turn_id,
        )
        .await;
        complete_pending_tool_calls_as_interrupted(
            &runtime,
            session_id,
            turn_for_events.turn_id,
            &turn_for_plan_updates,
            &tool_names_by_id,
            &mut pending_tool_calls,
        )
        .await;
        tracing::debug!(
            session_id = %session_id,
            turn_id = %turn_for_events.turn_id,
            "query event stream drained"
        );
        TurnEventStreamSummary {
            turn_usage,
            latest_query_usage,
            stop_reason,
        }
    })
}

fn log_dequeued_query_event(event: &devo_core::QueryEvent) {
    let assistant_token_text = query_event_trace_token_preview(event);
    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            event_kind = query_event_trace_kind(event),
            delta_len = query_event_trace_delta_len(event),
            assistant_token_text,
            "query event bridge dequeued by turn event task"
        );
    } else {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            event_kind = query_event_trace_kind(event),
            delta_len = query_event_trace_delta_len(event),
            "query event bridge dequeued by turn event task"
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_reasoning_delta(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    text: String,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
    reasoning_delta_seq: &mut u64,
) {
    let (item_id, item_seq) = match (*reasoning_item_id, *reasoning_item_seq) {
        (Some(item_id), Some(item_seq)) => (item_id, item_seq),
        (None, None) => {
            let (item_id, item_seq) = runtime
                .start_item(
                    session_id,
                    turn_id,
                    crate::ItemKind::Reasoning,
                    serde_json::json!({ "title": "Reasoning", "text": "" }),
                )
                .await;
            *reasoning_item_id = Some(item_id);
            *reasoning_item_seq = Some(item_seq);
            *reasoning_delta_seq = 0;
            (item_id, item_seq)
        }
        _ => return,
    };
    reasoning_text.push_str(&text);
    let chunk_index = *reasoning_delta_seq;
    *reasoning_delta_seq = reasoning_delta_seq.saturating_add(1);
    runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ReasoningTextDelta,
            payload: ItemDeltaPayload {
                chunk_index: Some(chunk_index),
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
            },
        })
        .await;
    // Deferred reasoning text is written once when the event stream drains.
    let _ = (event_stream, item_seq, item_id, reasoning_text);
}

async fn complete_open_reasoning_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
) {
    if let (Some(item_id), Some(item_seq)) = (reasoning_item_id.take(), reasoning_item_seq.take()) {
        if let Ok(mut stream) = event_stream.try_lock() {
            stream.deferred_reasoning.take();
        }
        complete_reasoning_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            reasoning_text.clone(),
        )
        .await;
        reasoning_text.clear();
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_tool_use_start(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    id: String,
    name: String,
    input: serde_json::Value,
    tool_names_by_id: &mut std::collections::HashMap<String, String>,
    pending_tool_calls: &mut std::collections::HashMap<String, PendingToolCall>,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    event_tool_registry: &Arc<devo_core::tools::ToolRegistry>,
) {
    tool_names_by_id.insert(id.clone(), name.clone());
    if let Some(pending) = pending_tool_calls.get_mut(&id) {
        pending.input = input.clone();
        pending.command = command_display_from_input(&name, &input);
        return;
    }
    if let (Some(item_id), Some(item_seq)) = (reasoning_item_id.take(), reasoning_item_seq.take()) {
        complete_reasoning_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            reasoning_text.clone(),
        )
        .await;
        reasoning_text.clear();
    }
    if let (Some(item_id), Some(item_seq)) = (assistant_item_id.take(), assistant_item_seq.take()) {
        complete_assistant_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            assistant_text.clone(),
        )
        .await;
        assistant_text.clear();
    }
    let display_kind = ToolDisplayKind::for_tool_name(&name);
    let command = command_display_from_input(&name, &input);
    let preparation_feedback = event_tool_registry.preparation_feedback(&name);
    let start_item = tool_start_item_from_input(
        &id,
        &name,
        &command,
        &input,
        display_kind,
        preparation_feedback,
    );
    let (item_id, item_seq) = runtime
        .start_item(
            session_id,
            turn_id,
            start_item.item_kind,
            start_item.payload,
        )
        .await;
    pending_tool_calls.insert(
        id,
        PendingToolCall {
            item_id: Some(item_id),
            item_seq: Some(item_seq),
            input,
            display_kind,
            command,
        },
    );
}

#[allow(clippy::too_many_arguments)]
async fn handle_tool_result(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    turn_for_plan_updates: &crate::TurnMetadata,
    tool_use_id: String,
    final_tool_name: String,
    final_input: serde_json::Value,
    content: devo_core::tools::ToolContent,
    display_content: Option<String>,
    is_error: bool,
    summary: String,
    tool_names_by_id: &std::collections::HashMap<String, String>,
    pending_tool_calls: &mut std::collections::HashMap<String, PendingToolCall>,
    event_tool_registry: &Arc<devo_core::tools::ToolRegistry>,
) {
    let tool_name = if final_tool_name.is_empty() {
        tool_names_by_id.get(&tool_use_id).cloned()
    } else {
        Some(final_tool_name)
    };
    let mut result_input = (!final_input.is_null()).then(|| final_input.clone());
    if let Some(mut pending) = pending_tool_calls.remove(&tool_use_id) {
        if !final_input.is_null() {
            pending.command = tool_name
                .as_deref()
                .map(|tool_name| command_display_from_input(tool_name, &final_input))
                .unwrap_or_default();
            pending.input = final_input;
        }
        result_input = Some(pending.input.clone());
        if (pending.item_id.is_none() || pending.item_seq.is_none())
            && let Some(tool_name) = tool_name.clone()
        {
            let preparation_feedback = event_tool_registry.preparation_feedback(&tool_name);
            let start_item = tool_start_item_from_result(
                &tool_use_id,
                &tool_name,
                &pending.command,
                &pending.input,
                pending.display_kind,
                preparation_feedback,
                &summary,
            );
            let (item_id, item_seq) = runtime
                .start_item(
                    session_id,
                    turn_id,
                    start_item.item_kind,
                    start_item.payload,
                )
                .await;
            pending.item_id = Some(item_id);
            pending.item_seq = Some(item_seq);
        }
        if complete_pending_tool_call(
            runtime,
            session_id,
            turn_id,
            turn_for_plan_updates,
            &tool_use_id,
            tool_name.clone(),
            &pending,
            &content,
            display_content.clone(),
            is_error,
            &summary,
        )
        .await
        {
            return;
        }
    }
    tool_results::emit_tool_result_item(
        runtime,
        session_id,
        turn_id,
        tool_use_id,
        tool_name,
        result_input,
        content,
        display_content,
        is_error,
        summary,
    )
    .await;
}

async fn complete_pending_tool_calls_as_interrupted(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    turn_for_plan_updates: &crate::TurnMetadata,
    tool_names_by_id: &std::collections::HashMap<String, String>,
    pending_tool_calls: &mut std::collections::HashMap<String, PendingToolCall>,
) {
    if pending_tool_calls.is_empty() {
        return;
    }
    let pending = std::mem::take(pending_tool_calls);
    for (tool_use_id, pending) in pending {
        let tool_name = tool_names_by_id
            .get(&tool_use_id)
            .cloned()
            .unwrap_or_default();
        let content = devo_core::tools::ToolContent::Text(
            devo_core::tools::INTERRUPTED_TOOL_RESULT_MESSAGE.to_string(),
        );
        let summary = if tool_name.is_empty() {
            "interrupted".to_string()
        } else {
            format!("{tool_name}: interrupted")
        };
        let suppress_separate_result = if pending.item_id.is_some() && pending.item_seq.is_some() {
            complete_pending_tool_call(
                runtime,
                session_id,
                turn_id,
                turn_for_plan_updates,
                &tool_use_id,
                (!tool_name.is_empty()).then(|| tool_name.clone()),
                &pending,
                &content,
                None,
                /*is_error*/ true,
                &summary,
            )
            .await
        } else {
            false
        };
        if !suppress_separate_result {
            tool_results::emit_tool_result_item(
                runtime,
                session_id,
                turn_id,
                tool_use_id,
                (!tool_name.is_empty()).then_some(tool_name),
                Some(pending.input),
                content,
                None,
                /*is_error*/ true,
                summary,
            )
            .await;
        }
    }
}

async fn handle_tool_input_delta(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: String,
    partial_json: String,
    pending_tool_calls: &std::collections::HashMap<String, PendingToolCall>,
    tool_input_delta_seqs: &mut std::collections::HashMap<String, u64>,
) {
    let Some(pending) = pending_tool_calls.get(&tool_use_id) else {
        return;
    };
    let Some(item_id) = pending.item_id.clone() else {
        return;
    };
    let chunk_index = tool_input_delta_seqs
        .get(&tool_use_id)
        .copied()
        .unwrap_or(0);
    tool_input_delta_seqs.insert(tool_use_id.clone(), chunk_index + 1);
    let _ = runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ToolCallInputDelta,
            payload: ItemDeltaPayload {
                context: crate::EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                    item_seq: None,
                },
                delta: serde_json::json!({
                    "tool_use_id": tool_use_id,
                    "partial_json": partial_json,
                })
                .to_string(),
                stream_index: None,
                channel: None,
                chunk_index: Some(chunk_index),
            },
        })
        .await;
}

async fn handle_tool_progress(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: String,
    progress: devo_core::tools::ToolProgress,
    pending_tool_calls: &std::collections::HashMap<String, PendingToolCall>,
    command_output_delta_seqs: &mut std::collections::HashMap<String, u64>,
) {
    let content = match progress {
        devo_core::tools::ToolProgress::OutputDelta { delta } => Some(delta),
        devo_core::tools::ToolProgress::StatusUpdate { message, percent } => Some(match percent {
            Some(percent) => format!("{message} ({percent}%)"),
            None => message,
        }),
        devo_core::tools::ToolProgress::Completion { summary } => Some(summary),
    };
    let Some(content) = content else {
        return;
    };
    let item_id = super::tool_display::command_execution_item_id_for_progress(
        pending_tool_calls,
        &tool_use_id,
    );
    let chunk_index = command_output_delta_seqs
        .get(&tool_use_id)
        .copied()
        .unwrap_or(0);
    command_output_delta_seqs.insert(tool_use_id.clone(), chunk_index + 1);
    let _ = runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::CommandExecutionOutputDelta,
            payload: ItemDeltaPayload {
                context: crate::EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id,
                    seq: 0,
                    item_seq: None,
                },
                delta: serde_json::json!({
                    "tool_use_id": tool_use_id,
                    "text": content,
                })
                .to_string(),
                stream_index: None,
                channel: None,
                chunk_index: Some(chunk_index),
            },
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn finish_proposed_plan_stream(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    proposed_plan_parser: &mut Option<ProposedPlanParser>,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    proposed_plan_item: &mut ProposedPlanStreamItem,
    proposed_plan_leading_normal: &mut String,
) {
    if let Some(parser) = proposed_plan_parser.as_mut() {
        let segments = parser.finish();
        handle_proposed_plan_segments(
            runtime,
            event_stream,
            session_id,
            turn_id,
            segments,
            assistant_item_id,
            assistant_item_seq,
            assistant_text,
            assistant_delta_seq,
            proposed_plan_item,
            proposed_plan_leading_normal,
        )
        .await;
        proposed_plan_item
            .complete(runtime, session_id, turn_id)
            .await;
    }
}

async fn complete_deferred_stream_items(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
) {
    if let Some((item_id, item_seq, text)) = {
        let mut stream = event_stream.lock().await;
        stream.deferred_reasoning.take()
    } {
        complete_reasoning_item(runtime, session_id, turn_id, item_id, item_seq, text).await;
    }
    if let Some((item_id, item_seq, text)) = {
        let mut stream = event_stream.lock().await;
        stream.deferred_assistant.take()
    } {
        complete_assistant_item(runtime, session_id, turn_id, item_id, item_seq, text).await;
    }
}
