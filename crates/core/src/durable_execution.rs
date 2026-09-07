//! Acknowledged execution records, independent of lossy UI event delivery.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ResponseItem;

/// One atomic execution fact. Calls are accepted only after complete model assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ExecutionRecord {
    OutputArtifacts {
        artifacts: Vec<devo_tools::output_store::OutputArtifact>,
    },
    ModelCompleted {
        items: Vec<ResponseItem>,
        stop_reason: Option<crate::StopReason>,
    },
    Recovery {
        state: RecoveryState,
    },
    IntentBatch {
        calls: Vec<ResponseItem>,
    },
    Outcomes {
        results: Vec<ResponseItem>,
    },
    PromptCheckpoint {
        items: Vec<ResponseItem>,
        #[serde(default)]
        counters: Option<ExecutionCounters>,
    },
}

/// Accounting at a durable prompt boundary; continuing does not reset usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCounters {
    pub turn_count: usize,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize,
    pub total_cache_read_tokens: usize,
    pub last_input_tokens: usize,
    pub last_turn_tokens: usize,
}

impl ExecutionCounters {
    pub fn capture(session: &crate::SessionState) -> Self {
        Self {
            turn_count: session.turn_count,
            total_input_tokens: session.total_input_tokens,
            total_output_tokens: session.total_output_tokens,
            total_tokens: session.total_tokens,
            total_cache_creation_tokens: session.total_cache_creation_tokens,
            total_cache_read_tokens: session.total_cache_read_tokens,
            last_input_tokens: session.last_input_tokens,
            last_turn_tokens: session.last_turn_tokens,
        }
    }

    pub fn restore(&self, session: &mut crate::SessionState) {
        session.turn_count = self.turn_count;
        session.total_input_tokens = self.total_input_tokens;
        session.total_output_tokens = self.total_output_tokens;
        session.total_tokens = self.total_tokens;
        session.total_cache_creation_tokens = self.total_cache_creation_tokens;
        session.total_cache_read_tokens = self.total_cache_read_tokens;
        session.last_input_tokens = self.last_input_tokens;
        session.last_turn_tokens = self.last_turn_tokens;
    }
}

/// An explicit user decision survives restart independently of turn status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryState {
    pub revision: u64,
    pub attempt: u32,
    pub disposition: RecoveryDisposition,
    pub reason: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryDisposition {
    Available,
    Resuming,
    Canceled,
}

/// Durable acknowledgment for a turn's execution facts.
///
/// Implementations must serialize writes, synchronize storage before returning
/// success, and reject conflicting commits for a call identity. Failure must be
/// returned to the caller; sending an observation event is not acknowledgment.
/// A turn must not dispatch an intent or continue past an outcome until committed.
#[async_trait]
pub trait ToolIntentJournal: Send + Sync {
    async fn commit(&self, record: ExecutionRecord) -> anyhow::Result<()>;

    /// Read the acknowledged state without executing pending calls.
    async fn replay(&self) -> anyhow::Result<ExecutionReplay>;
}

/// Fold acknowledged records without executing any calls during replay.
#[derive(Debug, Default, Clone)]
pub struct ExecutionReplay {
    pub artifacts: std::collections::HashMap<String, devo_tools::output_store::OutputArtifact>,
    pub counters: Option<ExecutionCounters>,
    pub completed: bool,
    pub stop_reason: Option<crate::StopReason>,
    pub recovery: Option<RecoveryState>,
    pub items: Vec<ResponseItem>,
    pub has_checkpoint: bool,
    committed: std::collections::HashMap<String, ResponseItem>,
}

impl ExecutionReplay {
    /// Validate and apply a fact atomically, rejecting conflicting identities.
    pub fn apply(&mut self, record: &ExecutionRecord) -> anyhow::Result<()> {
        let (prefix, entries) = match record {
            ExecutionRecord::OutputArtifacts { artifacts } => {
                for artifact in artifacts {
                    if let Some(previous) = self.artifacts.get(&artifact.id) {
                        anyhow::ensure!(
                            previous.file_identity == artifact.file_identity
                                && previous.path == artifact.path
                                && previous.owner == artifact.owner
                                && previous.bytes <= artifact.bytes,
                            "conflicting output artifact reference"
                        );
                    }
                }
                for artifact in artifacts {
                    self.artifacts.insert(artifact.id.clone(), artifact.clone());
                }
                return Ok(());
            }
            ExecutionRecord::ModelCompleted { items, stop_reason } => {
                if self.completed {
                    anyhow::ensure!(
                        self.items == *items && self.stop_reason == *stop_reason,
                        "conflicting final model response"
                    );
                } else {
                    self.items.clone_from(items);
                    self.has_checkpoint = true;
                    self.completed = true;
                    self.stop_reason = stop_reason.clone();
                }
                return Ok(());
            }
            ExecutionRecord::Recovery { state } => {
                if let Some(previous) = &self.recovery {
                    anyhow::ensure!(
                        state.revision > previous.revision || state == previous,
                        "conflicting recovery revision"
                    );
                }
                self.recovery = Some(state.clone());
                return Ok(());
            }
            ExecutionRecord::IntentBatch { calls } => ("call", calls),
            ExecutionRecord::Outcomes { results } => ("result", results),
            ExecutionRecord::PromptCheckpoint { items, counters } => {
                self.counters.clone_from(counters);
                self.items.clone_from(items);
                self.has_checkpoint = true;
                return Ok(());
            }
        };
        let mut updated = self.clone();
        for item in entries {
            let id = match (prefix, item) {
                ("call", ResponseItem::ToolCall { id, .. }) => id,
                ("result", ResponseItem::ToolCallOutput { tool_use_id, .. }) => tool_use_id,
                _ => anyhow::bail!("invalid execution record item"),
            };
            let key = format!("{prefix}:{id}");
            if let Some(previous) = updated.committed.get(&key) {
                anyhow::ensure!(previous == item, "conflicting execution record for {key}");
                continue;
            }
            if !updated.items.contains(item) {
                updated.items.push(item.clone());
            }
            updated.committed.insert(key, item.clone());
        }
        *self = updated;
        Ok(())
    }

    /// Missing outcomes are uncertainty reports, never instructions to rerun.
    pub fn interrupted_outcomes(&self) -> Vec<ResponseItem> {
        let completed = self
            .items
            .iter()
            .filter_map(ResponseItem::tool_call_output_id)
            .collect::<std::collections::HashSet<_>>();
        self.items.iter().filter_map(|item| {
            let ResponseItem::ToolCall { id, .. } = item else { return None };
            (!completed.contains(id.as_str())).then(|| ResponseItem::ToolCallOutput {
                tool_use_id: id.clone(),
                content: "Tool execution was interrupted. Execution may have occurred; verify its outcome before retrying. This call was not automatically rerun.".into(),
                is_error: true,
            })
        }).collect()
    }
}

/// Load acknowledged facts for one turn, tolerating only a truncated crash tail.
pub fn read_execution_replay(
    path: &std::path::Path,
    turn_id: crate::TurnId,
) -> anyhow::Result<ExecutionReplay> {
    use crate::{
        InternalRecordV2, ParsedRolloutLine, RolloutLineReadError, RolloutLineV2,
        parse_rollout_line,
    };
    use std::io::BufRead;
    let mut replay = ExecutionReplay::default();
    let mut lines = std::io::BufReader::new(std::fs::File::open(path)?)
        .lines()
        .peekable();
    while let Some(line) = lines.next() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(ParsedRolloutLine::V2(line)) => match *line {
                RolloutLineV2::Internal {
                    turn_id: Some(owner),
                    entry: InternalRecordV2::Execution { record },
                    ..
                } if owner.as_str() == turn_id.to_string() => replay.apply(&record)?,
                RolloutLineV2::CompactionSnapshot { .. } => replay.has_checkpoint = false,
                _ => {}
            },
            Ok(ParsedRolloutLine::Legacy(_)) => {}
            Err(RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(replay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn replay_is_idempotent_and_rejects_conflicting_arguments() {
        let call = ResponseItem::ToolCall {
            id: "a".into(),
            name: "write".into(),
            input: serde_json::json!({"value": 1}),
        };
        let fact = ExecutionRecord::IntentBatch {
            calls: vec![call.clone()],
        };
        let mut replay = ExecutionReplay::default();
        replay.apply(&fact).unwrap();
        replay.apply(&fact).unwrap();
        assert_eq!(replay.items, vec![call.clone()]);
        let bad = ExecutionRecord::IntentBatch {
            calls: vec![ResponseItem::ToolCall {
                id: "a".into(),
                name: "write".into(),
                input: serde_json::json!({"value": 2}),
            }],
        };
        assert!(replay.apply(&bad).is_err());
        assert_eq!(replay.items, vec![call]);
        let outcomes = replay.interrupted_outcomes();
        assert_eq!(outcomes.len(), 1);
        replay
            .apply(&ExecutionRecord::Outcomes {
                results: outcomes.clone(),
            })
            .unwrap();
        replay
            .apply(&ExecutionRecord::Outcomes { results: outcomes })
            .unwrap();
        assert_eq!(replay.interrupted_outcomes(), Vec::new());
    }
}
