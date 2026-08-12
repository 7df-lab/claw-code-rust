---
artifact_id: L2-DES-APP-009
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-02
---

# L2-DES-APP-009 — Event Stream Cutover to Canonical Typed Events

## Purpose

Define how the event path converges onto canonical typed events (L2-DES-APP-008 DD-3): what flows where, the delta revision model, and the deletion sequence for the ACP-envelope-with-`_meta`-smuggling status quo.

## Background / Current State

- Every wire notification is projected per-connection into an ACP `session/update` envelope (`acp_notification_from_server_event`); the native `ServerEvent` is smuggled in `_meta.devo.original_event` for first-party clients to unwrap. Opt-in `typed_items` connections get typed projections for `item/started` and `item/completed` **only** — everything else, including the high-volume token deltas, falls back to ACP envelopes.
- The canonical event vocabulary is already complete (`crates/protocol/src/native/event.rs` `ServerNotification`: turn lifecycle, item lifecycle + three delta kinds, queue, goal, usage, compaction, task, agent, permission).
- Two delivery systems coexist: legacy `events/subscribe` filters, and canonical `subscription/*` selectors fed by the rollout-derived event log (`events_from_v2_line`) with replay/cursors.
- The TUI consumes the queue through canonical types already (`parse_canonical_queue_updated`) and everything else through the ACP envelope + `_meta` unwrap.

## Design Decisions

### DD-1: Durable events flow through `subscription/*`; hot deltas flow through the live typed path

Session/turn/item lifecycle, queue, goal, usage and compaction events are durable facts derived from the rollout event log; `subscription/*` already delivers them as canonical notifications with replay and cursors. Token/output deltas are ephemeral, high-frequency, and never persisted per-chunk; they ride the live broadcast path as typed notifications. A first-party client needs exactly two consumption points: subscription streams (durable) and live typed deltas (ephemeral) — both canonical-shaped.

### DD-2: Delta model — per-item `chunk_index` assigned at the emit site

Canonical deltas (`item/assistantMessage/delta`, `item/reasoning/delta`, `item/commandExecution/outputDelta`) carry `{ item_id, base_revision, chunk_index, delta }`. The emit sites already maintain per-item delta counters (e.g. `assistant_delta_seq` in `crates/server/src/runtime/turn_exec/item_stream.rs:172`) but do not thread them into the legacy payload (`context.seq` is hardcoded to 0 at emit and overwritten per-connection at fan-out).

**Decision**: thread the per-item counter into the emitted delta payload as the canonical `chunk_index` (0-based, strictly increasing per item). `base_revision` is always the revision established by the item's `item/started` snapshot (1 at birth, the latest `item/updated` revision afterwards); the projector reads it from the item state, not from the legacy payload. Per-connection event `seq` remains a transport-level ordering concern and is not reused as `chunk_index`.

### DD-3: Typed projection is extended to the delta family and turn lifecycle

`typed_item_notification_from_server_event` grows into a `typed_notification_from_server_event` covering: the three delta kinds, `item/started`, `item/updated`, `item/completed`, `turn/started`, `turn/statusChanged`, `turn/completed`, `context/usageUpdated`, `context/compactionStarted`, `context/compactionCompleted`, `model/queryRetrying`, `turn/usage/updated`. Events outside this set keep their current delivery during migration (durable ones via subscription). All four former stragglers landed 2026-08-09: provider retry projects with additive `provider`/`model`/`phase` (missing `max_attempts` stays legacy); the usage meter projects to live `turn/usage/updated` with `sessionTotals`; plan updates project as full `Plan` items on `item/updated`; compaction lifecycle via emit-site enrichment. TUI typed consumption for all four is in place.

### DD-4: `notification_for` becomes typed-first for first-party connections

Connection projection order flips: typed canonical first; ACP envelope projection only for ACP adapter connections. The `typed_items` opt-in flag becomes the default for first-party transports once the TUI consumes typed events; `_meta.devo.original_event` embedding and `original_event_from_acp_notification` unwrapping (devo-client + TUI) are deleted in the same step as the TUI cutover — not before — because they are the TUI's current event source.

### DD-5: TUI consumption cutover

The TUI replaces `worker/acp_events.rs` dispatch with typed consumers: `ItemStarted/ItemUpdated/ItemCompleted` by id (replace-by-id on revision), the three delta kinds appended by `chunk_index`, turn lifecycle driving status lines. Legacy `events/subscribe` usage is replaced by `subscription/*` selectors with snapshots for resume. Acceptance: the TUI test baseline stays green with the ACP-envelope path deleted, plus a manual smoke (send, steer, queue, interrupt, resume).

### DD-6: Protocol surface is negotiated per connection at `initialize`

ACP and the canonical protocol share wire method names (`session/new`, `session/resume`, `session/list`, `session/delete`, …). Routing on the method name alone is wrong: the ACP adapter registry intercepted those names for every connection, so Native handlers registered under the same names were unreachable and first-party Native calls (e.g. the TUI's `session/new`) silently received ACP-shaped responses.

**Decision**: each connection negotiates its protocol surface at `initialize` via `_meta.devo.protocol` (`"native"` selects the Native surface; an absent marker selects the ACP v1 adapter). The runtime `ProtocolSet` must expose the selected adapter; otherwise initialization returns `InvalidRequest` and remains retryable. The server records the selection as `ConnectionProtocol` and dispatches exclusively through that adapter: ACP route registration is owned by the ACP boundary and ACP cannot fall through to Native routes. First-party clients declare `native` directly. Events are emitted once and projected once through each eligible connection's selected adapter, including reverse requests. Existing connections do not switch protocol or receive replay when the singleton's enabled set is extended.

## Migration Steps

1. Thread per-item `chunk_index` through delta emission (DD-2); extend the typed projector (DD-3) with unit tests per event kind.
2. TUI typed consumers behind the existing dispatch, then delete `acp_events.rs` + `_meta` unwrapping (DD-5).
3. Flip first-party connections to typed-first projection (DD-4); delete the embedding/unwrap helpers.
4. Inventory and migrate residual legacy-only events; `events/subscribe` removal lands with Phase E of L2-DES-APP-008.

### Staged cutover (2026-08-09 inventory)

The TUI's devo-envelope consumption (`worker.rs` main `ServerEvent` loop) already covers most event kinds — unprojected events arrive via `_meta.devo.original_event` smuggling, which the client unwraps back into the same `ServerEvent` the loop parses. The productive ACP-envelope consumption was reduced to exactly two paths: subagent discovery from ACP session-info, and spawn-result discovery from ACP tool-call updates. Status:

1. ✅ DD-3 projections for all four former stragglers (retry/usage/plan/compaction) + TUI typed consumption.
2. ✅ Subagent discovery migrated off the ACP envelope: `SessionStarted` devo event (carries the same `SessionMetadata` with parentage) and typed `item/completed` ToolResult (`maybe_discover_spawned_subagent_from_tool_output`).
3. ✅ `notification_for` flipped: native-surface connections get typed-if-projected else the raw devo envelope (no ACP wrap, no `_meta`); ACP-surface connections keep the ACP envelope unchanged.
4. ✅ Deleted the `ACP_SESSION_UPDATE_METHOD` block + `worker/acp_events.rs`. The monitor now consumes: discovery from `SessionStarted`, lifecycle from typed `turn/*` (session-filtered), tool cards from typed item events (`subagent_monitor_events_from_typed_item`). The `_meta.devo.original_event` embedding and `original_event_from_acp_notification` stay for ACP-surface connections only (pinned by `protocol-lock.json`); first-party no longer emits or consumes them.
5. ~~**Recorded gap**~~ — RESOLVED 2026-08-09: canonical `ItemDelta` now carries `session_id`, and the TUI routes child-session text deltas to the subagent monitor preview (`SubagentMonitorEvent::TextItemDelta`); main-session deltas are unchanged. The monitor has full parity again (discovery, lifecycle, tool cards, live text).
6. Also session-filtered the typed fast-path (`turn/*`, item events, usage/retry/context) so child-session events can never mutate main-session state — closing a pre-existing hole from the typed opt-in.

## Risks

- **Delta loss/reordering detection changes meaning**: `chunk_index` gaps indicate dropped chunks (best-effort delivery policy drops under backpressure); clients must treat gaps as "re-read the item snapshot" triggers. This is documented in the canonical event model and verified by a TUI test with a full-channel drop.
- **Double-render during cutover**: while both shapes are live, a notification must be consumed by exactly one path; the dispatch keys on the wire method, and the transition is per-event-kind.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-001 | 1 | specs/L1/L1-REQ-APP-001-client-server-arch.md | Event cutover refines the client-server event architecture onto the canonical surface. |
| related-to | L2-DES-APP-008 | 1 | specs/L2/app/L2-DES-APP-008-protocol-unification.md | Implements DD-3 (event protocol convergence) of the protocol unification. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-02 | Assistant | Initial | Initial draft. |
| 2 | 2026-08-09 | Assistant | Added DD-6 | Per-connection protocol-surface negotiation (`_meta.devo.protocol`) fixing the ACP/canonical method-name collision routing. |
