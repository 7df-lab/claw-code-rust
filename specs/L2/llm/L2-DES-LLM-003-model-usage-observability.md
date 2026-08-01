---
artifact_id: L2-DES-LLM-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-06-23
---

# L2-DES-LLM-003 — Model Usage Observability

## Purpose

Refine model usage observability into a technical design for usage metrics, context-window pressure reporting, model invocation correlation, and trace-mode streaming response diagnostics.

## Background / Context

Model calls are expensive, latency-sensitive, and provider-dependent. Users need to know how many tokens were read, written, cached, or estimated; whether the current context is near the effective limit; and whether provider streaming behaved as expected. The design must preserve provider-reported facts without inventing unavailable values and must protect sensitive prompt, response, and credential data.

## Source Requirements

- `L1-REQ-LLM-003` requires model usage, cached input usage, context-window usage, estimate labeling, unavailable-value reporting, and trace-mode stream records.
- `L1-REQ-APP-004` requires structured logging, configurable log levels, actionable diagnostics, and trace logging for large language model streaming response events.
- `L1-REQ-LLM-001` requires token-efficient context construction.
- `L1-REQ-CONTEXT-001` requires useful active context management.
- `L1-REQ-CONTEXT-003` requires context compression near model limits.
- `L1-REQ-MODEL-001` defines model capability metadata and effective context length.
- `L2-DES-APP-004` defines the cross-system observability architecture.
- `L2-DES-AGENT-001` defines where model invocations occur in the execution engine.
- `L2-DES-CONV-001` defines durable usage records in the session JSONL data model.
- `L2-DES-MODEL-001` defines resolved model profiles and model-provider bindings.

## Design Requirement

The program should represent model observability as invocation-scoped facts with explicit provenance.

For each model invocation, the program should track:

- Which session and turn caused the invocation.
- Which model binding and provider method were used.
- What context size was assembled or estimated.
- What usage values the provider reported.
- Which usage values were estimated locally.
- Which usage values were unavailable.
- How the response stream progressed when trace mode is enabled.

The program must not infer exact provider usage when the provider does not report it. Locally computed token counts are estimates unless an adapter can prove they match provider accounting.

## Invocation Identity

Each model call should receive an `invocation_id` before provider execution starts.

Conceptual invocation fields:

- `invocation_id`
- `session_id`
- `turn_id`
- `context_snapshot_id`
- `model_binding_id`
- `canonical_model_slug`
- `provider_id`
- `invocation_method`
- `reasoning_effort`
- `started_at`
- `completed_at`
- `status`: running, completed, failed, interrupted, or canceled.
- `streaming`: whether the invocation used streaming.
- `trace_id` when trace mode is enabled.

The `invocation_id` should appear in logs, usage records, server-client usage events, provider error diagnostics, and trace-mode stream records.

## Conservative Token Usage Model

Runtime model usage should use a conservative normalized token model.

Canonical runtime token fields:

| Metric | Meaning |
|---|---|
| `input_tokens` | Tokens read by the model for this invocation. |
| `output_tokens` | Provider-mapped primary output tokens for this invocation. |
| `cache_creation_input_tokens` | Input tokens used to create provider-side cache where reported. |
| `cache_read_input_tokens` | Input tokens served from provider-side cache where reported. |
| `reasoning_output_tokens` | Optional provider-reported reasoning breakdown. |
| `total_tokens` | Optional provider-reported total where available. |
| `context_tokens` | Token count or estimate for active model-visible context. |
| `effective_context_window` | Effective context limit used by the program for this model call. |

Rules:

- `output_tokens` is the primary output count after provider adapter mapping.
- `reasoning_output_tokens` is a breakdown only. It must not be added to `output_tokens`, session totals, goal accounting, or display totals.
- If the provider returns `total_tokens`, UI and session display totals should use that value.
- If the provider does not return `total_tokens`, the derived display total is `input_tokens + output_tokens`.
- Provider adapters must not synthesize and persist `total_tokens` when the provider did not report it; fallback derivation belongs in shared display helpers.
- Cached input fields remain separate breakdowns and do not change the derived total rule.

## Durable UsageMetric Compatibility

Earlier design notes described a generic per-field `UsageMetric` wrapper with `value`, `source`, `provider_field`, `included_in`, and related metadata. That wrapper is now a historical and durable-record compatibility shape, not the canonical runtime model usage IR.

Existing durable or diagnostic records may continue to contain `UsageMetric` values when needed for replay or backward compatibility. New runtime paths should use the conservative normalized token fields above and should avoid expanding the `UsageMetric` wrapper into ordinary provider, server, TUI, or CLI code.

## Read, Write, And Cached-Read Display

For user-facing display, the program should map core token metrics into readable terms:

- Read: `input_tokens`
- Cached read: `cache_read_input_tokens`
- Write: `output_tokens`

If a provider uses different names, the provider adapter should map them into the normalized runtime fields. Diagnostic or durable compatibility records may preserve the original `provider_field` when useful.

Display rules:

- Provider-reported values should be labeled as measured or provider-reported.
- Local estimates should be labeled as estimates.
- Unavailable values should be shown as unavailable, not zero.
- Redacted values should be shown as redacted.
- Totals should use provider-reported `total_tokens` when present; otherwise they should derive `input_tokens + output_tokens`.
- Totals must not add `reasoning_output_tokens` separately.

## Context Pressure

Context pressure describes how close the invocation is to the effective context window.

Conceptual context pressure fields:

- `context_tokens`
- `context_tokens_source`
- `effective_context_window`
- `pressure_ratio`
- `pressure_state`: normal, high, near_limit, over_limit, or unknown.
- `compaction_threshold`
- `compaction_status`: not_needed, recommended, scheduled, running, completed, failed, or unavailable.

When exact context tokenization is unavailable, the program may estimate context size. Estimated context size must be labeled as an estimate.

Context pressure should be emitted before or around model invocation where possible, and updated when provider usage confirms or corrects the estimate.

## Durable Usage Records

Durable session records should preserve usage summaries needed for replay and later inspection.

Durable usage records should include:

- `session_id`
- `turn_id`
- `invocation_id`
- `model_binding_id`
- `canonical_model_slug`
- `provider_id`
- Conservative token usage fields.
- Context pressure summary.
- Whether values are provider-reported, estimated, unavailable, or redacted.
- `recorded_at`

Durable usage records should not store prompt content, response content, credential material, or raw provider request headers.

The durable session JSONL file remains the source of truth for usage summaries that affect session inspection. Trace-mode stream records are diagnostic artifacts and should be stored under observability retention policy rather than treated as transcript content.

### Per-turn context occupancy snapshots

Category occupancy (`ContextOccupancy`) for `/context` and related projections should be stored on each completed `TurnRecord` and on `CompactionSnapshotLine` when compaction runs. Resume/replay takes the latest turn or compaction occupancy as the session tip; fork and rollback cuts must read the cut-turn (or an applicable compaction snapshot whose `turn_id` is still kept), not the source session tip. Session-level `session_stats.last_context_occupancy` may cache the tip for fast hydrate, but rollout remains authoritative for history cuts. Forked sessions reset cumulative token totals to a new ledger while preserving cut-point occupancy and last-query usage.

## Server-Client Usage Projection

Clients should receive safe usage projections through the server-client protocol.

Usage projections should include:

- Current invocation usage where known.
- Turn-level usage delta.
- Session-level usage totals.
- Context pressure.
- Metric source labels.
- Unavailable or redacted markers.

Clients should not need provider-specific parsing logic to display token usage. Provider-specific details may be exposed as safe metadata when useful for diagnostics.

## Trace-Mode Stream Records

When trace logging is enabled, the model provider adapter and execution engine should record stream events in invocation order.

Conceptual stream trace fields:

- `trace_id`
- `invocation_id`
- `session_id`
- `turn_id`
- `sequence`
- `timestamp`
- `elapsed_ms`
- `provider_event_kind`
- `normalized_event_kind`
- `content_policy`: omitted, redacted, inline, or content_ref.
- `delta_text` or `delta_ref` where allowed.
- `delta_bytes`
- `delta_chars`
- `finish_reason` where available.
- `usage_fragment` where available.
- `error_fragment` where available.
- `raw_event_ref` where configured and allowed.

Trace records should preserve timing and ordering even when content is redacted. If content logging is disabled or redacted, the record should still preserve event kind, sequence, timing, content length, and completion state where available.

## Trace Content Policy

Trace-mode stream records are more sensitive than ordinary usage records because response deltas may reveal user data, generated code, or secrets.

The program should support these trace content modes:

| Mode | Behavior |
|---|---|
| `metadata_only` | Records event kind, sequence, timing, sizes, usage fragments, and completion state, but omits streamed text. |
| `redacted_content` | Records deltas after configured redaction. |
| `content_ref` | Stores content in a protected diagnostic artifact and references it from the trace record. |
| `inline_content` | Stores streamed content inline only when explicitly enabled for local debugging. |

Default trace behavior should prefer `metadata_only` or `redacted_content`. Inline content should be opt-in because it increases privacy risk.

## Provider Adapter Responsibilities

Provider adapters should normalize usage and stream events without hiding provider-specific uncertainty.

Adapter responsibilities:

- Map provider usage fields into conservative normalized runtime fields.
- Preserve original provider field names in diagnostic records where useful.
- Mark missing values as unavailable.
- Mark local token counts as estimates unless exact.
- Treat reasoning-token fields as breakdowns only.
- Emit normalized stream event kinds.
- Preserve provider finish reasons and error categories.
- Apply redaction before writing content-bearing trace records.

Provider adapters should not calculate monetary cost unless a later requirement defines provider pricing inputs and billing rules.

## Context Occupancy Breakdown

Clients can read category-level context-window occupancy through
`context/usage/read` and the `context/usageUpdated` event.

Occupancy snapshot fields:

| Field | Meaning |
|---|---|
| `totalTokens` | Current occupied tokens for the effective context window. |
| `contextWindowTokens` | Effective context window (`context_window * percent / 100`). |
| `categories[]` | Ordered category shares with `id`, `tokens`, and `shareBps`. |

Category ids:

| Id | Contents |
|---|---|
| `base` | System prompt (including mode introductions); AGENTS.md / workspace instructions; environment, language, collaboration/context-change metadata; related instruction updates. |
| `skills` | Available-skills listing and skill injection / skill-tool content. |
| `toolsBuiltin` | Non-MCP tool schemas plus hosted tools. |
| `toolsMcp` | MCP tool schemas (`mcp__*` names). |
| `conversation` | Remaining messages: dialogue, tool calls/results, compaction summary, and other non-base/skills fragments. |

Update rules:

- After a completed turn query, scale the latest assembled-request byte estimates
  so category tokens sum exactly to the latest provider query display total
  (`total_tokens` when reported, otherwise `input_tokens + output_tokens`).
- After successful compaction, refresh immediately:
  `conversation` becomes the post-compact prompt-message estimate (summary plus
  preserved suffix); `base` / `skills` / `toolsBuiltin` / `toolsMcp` keep the
  previous occupancy buckets; `totalTokens` is their sum.
- Do not expose separate `source` / `measured` fields; turn occupancy is
  anchored to the latest provider query, and post-compact occupancy is the
  refresh formula above until the next query completes.

## Diagnostic Examples

Example usage projection:

```json
{
  "invocation_id": "inv_01",
  "usage": {
    "input_tokens": 18420,
    "output_tokens": 930,
    "cache_read_input_tokens": 12000,
    "reasoning_output_tokens": 460,
    "total_tokens": 19350
  },
  "display_total_tokens": 19350,
  "context_pressure": {
    "pressure_state": "near_limit",
    "pressure_ratio": 0.91,
    "source": "local_estimate"
  }
}
```

Example unavailable value:

```json
{
  "invocation_id": "inv_02",
  "cached_read": {
    "source": "unavailable",
    "notes": "Provider did not report cached input tokens."
  }
}
```

## Privacy And Retention

Rules:

- Usage summaries may be durable session metadata because they are needed for later inspection.
- Stream traces are diagnostic logs, not transcript records.
- Trace records should follow the retention policy from `L2-DES-APP-004`.
- Prompt content and response content should not be written to normal logs.
- Trace content should be redacted, referenced, or omitted according to trace content policy.
- Credential material and authorization headers must never be recorded.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-LLM-003 | 1 | specs/L1/L1-REQ-LLM-003-observability.md | Defines model usage metrics, context pressure, measured versus estimated values, unavailable values, and trace-mode stream records. |
| related-to | L1-REQ-APP-004 | 1 | specs/L1/L1-REQ-APP-004-observability.md | Uses application observability controls for logs, trace mode, privacy, and diagnostics. |
| related-to | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Context pressure and cached input usage inform token efficiency. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Active context size and pressure are context-management diagnostics. |
| related-to | L1-REQ-CONTEXT-003 | 1 | specs/L1/L1-REQ-CONTEXT-003-compress.md | Context pressure explains compaction behavior. |
| related-to | L2-DES-APP-004 | 1 | specs/L2/app/L2-DES-APP-004-observability-architecture.md | Provides the cross-system observability architecture used by model-specific observability. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Model invocations occur inside the execution engine. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable usage records are stored with session data. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Usage records reference model bindings and resolved provider profiles. |
| specified-by | L3-BEH-PROVIDER-003 | 1 | specs/L3/provider/L3-BEH-PROVIDER-003-model-usage-observability.md | Defines invocation identity, usage normalization, context pressure, durable usage records, safe client projections, and trace-mode model stream records. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial model usage and stream observability design. |
| 1 | 2026-06-23 | Assistant | Refinement | Adopted conservative token usage semantics, kept `UsageMetric` as durable compatibility only, and specified provider-total display fallback behavior. |
