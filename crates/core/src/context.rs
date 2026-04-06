use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ItemId, SessionId, SummaryModelSelection, TurnId};

/// Stores the token budget configuration for a session or turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenBudget {
    /// The maximum total context window supported by the active model.
    pub context_window: usize,
    /// The maximum number of tokens reserved for model output.
    pub max_output_tokens: usize,
    /// The threshold at which automatic compaction should trigger.
    pub compact_threshold: f64,
}

impl TokenBudget {
    /// Creates a new token budget with the default compaction threshold.
    pub fn new(context_window: usize, max_output_tokens: usize) -> Self {
        Self {
            context_window,
            max_output_tokens,
            compact_threshold: 0.9,
        }
    }

    /// Returns the available budget for prompt input.
    pub fn input_budget(&self) -> usize {
        self.context_window.saturating_sub(self.max_output_tokens)
    }

    /// Returns whether compaction should run for the supplied prompt token count.
    pub fn should_compact(&self, current_tokens: usize) -> bool {
        current_tokens as f64 > self.input_budget() as f64 * self.compact_threshold
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new(200_000, 16_000)
    }
}

/// Carries a conservative token estimate for one assembled prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindowEstimate {
    /// Estimated tokens contributed by fixed prompt prefixes.
    pub prefix_tokens: u32,
    /// Estimated tokens contributed by history and current input.
    pub history_tokens: u32,
    /// Estimated tokens contributed by tool definitions.
    pub tool_tokens: u32,
    /// Estimated tokens contributed by the current input item or items.
    pub current_input_tokens: u32,
    /// Estimated tokens contributed by model-visible safety constraints.
    pub safety_tokens: u32,
    /// Estimated total prompt tokens.
    pub total_tokens: u32,
}

/// Carries the normalized prompt segments used for token estimation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptAssemblyInput {
    /// The serialized base-instruction segment.
    pub base_instructions: String,
    /// Serialized tool descriptions or schemas.
    pub tool_definitions: Vec<String>,
    /// Serialized safety-summary lines.
    pub safety_constraints: Vec<String>,
    /// Serialized prompt-visible history items.
    pub history_items: Vec<String>,
    /// The current serialized input item payload.
    pub current_input: Vec<String>,
}

/// Estimates prompt token usage before provider invocation.
pub trait TokenEstimator: Send + Sync {
    /// Returns a conservative token estimate for the supplied prompt assembly input.
    fn estimate_prompt(
        &self,
        budget: &TokenBudget,
        prompt: &PromptAssemblyInput,
    ) -> ContextWindowEstimate;
}

/// Byte-heuristic token estimator used as the default local estimator.
pub struct ByteTokenEstimator;

impl TokenEstimator for ByteTokenEstimator {
    fn estimate_prompt(
        &self,
        _budget: &TokenBudget,
        prompt: &PromptAssemblyInput,
    ) -> ContextWindowEstimate {
        let prefix_bytes = prompt.base_instructions.len();
        let safety_bytes = prompt
            .safety_constraints
            .iter()
            .map(String::len)
            .sum::<usize>();
        let history_bytes = prompt.history_items.iter().map(String::len).sum::<usize>();
        let current_input_bytes = prompt.current_input.iter().map(String::len).sum::<usize>();
        let tool_bytes = prompt
            .tool_definitions
            .iter()
            .map(String::len)
            .sum::<usize>();

        let prefix_tokens = bytes_to_tokens(prefix_bytes);
        let safety_tokens = bytes_to_tokens(safety_bytes);
        let history_tokens = bytes_to_tokens(history_bytes);
        let current_input_tokens = bytes_to_tokens(current_input_bytes);
        let tool_tokens = bytes_to_tokens(tool_bytes);
        ContextWindowEstimate {
            prefix_tokens,
            history_tokens: history_tokens.saturating_add(current_input_tokens),
            tool_tokens,
            current_input_tokens,
            safety_tokens,
            total_tokens: prefix_tokens
                .saturating_add(safety_tokens)
                .saturating_add(history_tokens)
                .saturating_add(current_input_tokens)
                .saturating_add(tool_tokens),
        }
    }
}

fn bytes_to_tokens(bytes: usize) -> u32 {
    bytes.div_ceil(4).try_into().unwrap_or(u32::MAX)
}

/// Stores the summary payload created during compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSummaryPayload {
    /// The human-readable summary text visible to the model.
    pub summary_text: String,
    /// The covered turn sequence numbers.
    pub covered_turn_sequences: Vec<u32>,
    /// Important facts preserved by the summary.
    pub preserved_facts: Vec<String>,
    /// Outstanding tasks or unresolved loops preserved by the summary.
    pub open_loops: Vec<String>,
    /// The model slug used to generate the summary.
    pub generated_by_model: String,
}

/// Identifies which snapshot backend succeeded for one compaction event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotBackendKind {
    /// Only the canonical JSON snapshot metadata was written.
    JsonOnly,
    /// JSON snapshot metadata was written and a git ghost snapshot was also captured.
    JsonAndGit {
        /// The detached git commit identifier for the ghost snapshot.
        ghost_commit_id: String,
        /// The optional parent commit identifier used to seed the ghost snapshot.
        parent_commit_id: Option<String>,
    },
}

/// Stores the canonical metadata needed to reconstruct one compaction event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSnapshot {
    /// The owning session identifier.
    pub session_id: SessionId,
    /// The turn during which compaction occurred.
    pub turn_id: TurnId,
    /// The first replaced item identifier.
    pub replaced_from_item_id: ItemId,
    /// The last replaced item identifier.
    pub replaced_to_item_id: ItemId,
    /// The summary item that now stands in for the replaced range.
    pub summary_item_id: ItemId,
    /// The model slug used to generate the summary.
    pub model_slug: String,
    /// The summary-model selection mode active when compaction ran.
    pub summary_model_selection: SummaryModelSelection,
    /// The stable ordering metadata required to rebuild prompt segments deterministically.
    pub prompt_segment_order: Vec<ItemId>,
    /// Optional workspace hint used for recovery tooling.
    pub workspace_root: Option<PathBuf>,
    /// Optional repository-root hint used for recovery tooling.
    pub repo_root: Option<PathBuf>,
    /// The backend that produced the durable snapshot record.
    pub snapshot_backend: SnapshotBackendKind,
}

/// Carries the normalized detail for one failed snapshot persistence attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SnapshotPersistFailure {
    /// Writing the canonical JSON snapshot failed.
    #[error("json snapshot write failed: {message}")]
    JsonSnapshotWriteFailed {
        /// The human-readable failure detail.
        message: String,
    },
    /// Git-backed snapshots were requested but unavailable.
    #[error("git snapshot unavailable: {message}")]
    GitSnapshotUnavailable {
        /// The human-readable failure detail.
        message: String,
    },
    /// Capturing a git-backed ghost snapshot failed.
    #[error("git snapshot capture failed: {message}")]
    GitSnapshotCaptureFailed {
        /// The human-readable failure detail.
        message: String,
    },
    /// Restoring a git-backed ghost snapshot failed.
    #[error("git snapshot restore failed: {message}")]
    GitSnapshotRestoreFailed {
        /// The human-readable failure detail.
        message: String,
    },
}

/// Describes failures that can occur during context compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum CompactionError {
    /// Local estimation data was not available.
    #[error("estimate unavailable")]
    EstimateUnavailable,
    /// The summarizer model call failed.
    #[error("summary provider failed: {message}")]
    SummaryProviderFailed {
        /// The human-readable provider failure message.
        message: String,
    },
    /// Compaction would violate a structural invariant.
    #[error("compaction invariant violation: {message}")]
    InvariantViolation {
        /// The human-readable invariant failure message.
        message: String,
    },
    /// Snapshot persistence failed.
    #[error("snapshot persistence failed: {source}")]
    SnapshotPersistFailed {
        /// The normalized backend-specific snapshot failure.
        source: SnapshotPersistFailure,
    },
    /// No valid compaction plan could fit within the active constraints.
    #[error("compaction not possible: {message}")]
    CompactionNotPossible {
        /// The human-readable planning failure message.
        message: String,
    },
}

/// Pluggable compaction strategy contract for context management.
#[async_trait]
pub trait ContextCompactor: Send + Sync {
    /// Compacts the supplied prompt-visible history into a summary payload.
    async fn compact(
        &self,
        prompt: PromptAssemblyInput,
        budget: TokenBudget,
    ) -> Result<ContextSummaryPayload, CompactionError>;
}

#[cfg(test)]
mod tests {
    use super::{
        ByteTokenEstimator, PromptAssemblyInput, SnapshotPersistFailure, TokenBudget,
        TokenEstimator,
    };

    #[test]
    fn token_budget_default_values() {
        let budget = TokenBudget::default();
        assert_eq!(budget.context_window, 200_000);
        assert_eq!(budget.max_output_tokens, 16_000);
        assert!((budget.compact_threshold - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn token_budget_input_budget_saturates() {
        let budget = TokenBudget::new(100, 200);
        assert_eq!(budget.input_budget(), 0);
    }

    #[test]
    fn estimator_counts_prompt_segments() {
        let estimate = ByteTokenEstimator.estimate_prompt(
            &TokenBudget::default(),
            &PromptAssemblyInput {
                base_instructions: "abcd".into(),
                tool_definitions: vec!["1234".into()],
                safety_constraints: vec!["zzzz".into()],
                history_items: vec!["history".into()],
                current_input: vec!["input".into()],
            },
        );

        assert!(estimate.prefix_tokens > 0);
        assert!(estimate.history_tokens > 0);
        assert!(estimate.tool_tokens > 0);
        assert!(estimate.current_input_tokens > 0);
        assert!(estimate.safety_tokens > 0);
        assert_eq!(
            estimate.total_tokens,
            estimate.prefix_tokens
                + estimate.safety_tokens
                + estimate.history_tokens
                + estimate.tool_tokens
        );
    }

    #[test]
    fn snapshot_failure_is_structured() {
        let failure = SnapshotPersistFailure::GitSnapshotUnavailable {
            message: "git executable not found".into(),
        };
        let json = serde_json::to_string(&failure).expect("serialize");
        let restored: SnapshotPersistFailure = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(failure, restored);
    }
}
