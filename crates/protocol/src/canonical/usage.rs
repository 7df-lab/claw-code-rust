//! Usage accounting: every model call produces a metering record, without
//! exception. Metering is a property of the call channel
//! (`InstrumentedProvider` + `CallContext`), not of each call site's
//! diligence.
//!
//! Truth source: `devo-api-design/09-usage.md`.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::SessionId;
use super::ids::TurnId;
use super::model::ModelBinding;

/// Metering context that every model call must carry; the instrumented
/// provider wrapper makes it compile-time impossible to call without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CallContext {
    pub session_id: SessionId,
    /// Attribution: AutoReview/Compaction belong to their triggering turn;
    /// title generation etc. belong to the session only (`None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub purpose: UsagePurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum UsagePurpose {
    /// Main loop, including model retries and post-tool follow-ups.
    TurnQuery,
    /// Model reviewer before approval.
    AutoReview,
    /// History compaction summary.
    Compaction,
    /// Session title generation.
    TitleGeneration,
    /// Goal classification/orchestration (reserved).
    GoalClassification,
    /// Memory extraction/merge (reserved).
    MemoryExtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum UsageCallOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// Provider-reported token usage for one call. Distinct from
/// `ContextUsage` (context-window occupancy): usage answers "how much was
/// spent", context answers "how full is the window".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Billed at a different rate by most providers, hence separate.
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// Monetary amount in micro-units (1/1_000_000) of `currency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub currency: String,
    pub micros: i64,
}

/// One append-only metering record per model call attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub call_id: String,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub purpose: UsagePurpose,
    /// Snapshot: the model this call actually used.
    pub model: ModelBinding,
    pub outcome: UsageCallOutcome,
    /// `None` when the provider did not report usage; never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// `None` when no price list is available; never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
    pub recorded_at: DateTime<Utc>,
}

/// Aggregated token/call totals. `call_count` includes failed and cancelled
/// attempts; `metered_call_count` counts attempts with reported usage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub call_count: u32,
    pub metered_call_count: u32,
    pub failed_call_count: u32,
    pub cancelled_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PurposeUsage {
    pub purpose: UsagePurpose,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub call_count: u32,
    pub metered_call_count: u32,
    pub failed_call_count: u32,
    pub cancelled_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Money>,
}

/// Split so that "how much did this turn cost" and "the bill" reconcile:
/// `query` covers only `TurnQuery`; `overhead` covers the turn's
/// `AutoReview + Compaction`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub query: UsageTotals,
    pub overhead: UsageTotals,
}

/// Derived cache aggregated from the usage ledger (truth = ledger sum).
/// Exists solely so list views need not page through turn details; on any
/// disagreement the ledger wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub total: UsageTotals,
    pub by_purpose: Vec<PurposeUsage>,
    /// Pre-upgrade historical totals that cannot be decomposed into calls;
    /// not disguised as call records. `total = legacy + ledger`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<UsageTotals>,
    pub updated_at: DateTime<Utc>,
}
