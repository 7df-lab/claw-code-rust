---
artifact_id: L2-DES-CONV-002
revision: 1
status: Approved
active_baseline: yes
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-02
---

# L2-DES-CONV-002 — Two-Plane Session Settings

## Purpose

Define the architecture, API contract, and per-setting promises for updating session settings at any time, including during an active turn, with immediate persistence and deterministic effect timing. Refines L1-REQ-CONV-006.

## Scope

This document covers:
- The two-plane state model (session plane / turn plane) and state ownership rules
- The unified settings update API and its promises
- The write path (persist-first), read path (decision points), and recovery path
- Field-level settings persistence and epoch semantics
- The turn control plane and settings override channel
- Per-setting decision points and mid-turn semantics
- Phased rollout, risks, and mitigations

This document does **not** cover:
- Permission profile semantics themselves (see L2-DES-SAFETY-001)
- Approval solicitation and caching mechanics (see L2-DES-SAFETY-002)
- Queue/steer message semantics (see L1-REQ-CONV-003 and its L2)

## Current State (Audit Summary)

Settings writes today flow through the session actor mailbox, which is blocked for the entire duration of an active turn (`crates/server/src/runtime/session_actor/actor_loop.rs`, `ExecuteTurn` awaits `execute_turn_in_actor` inline). Consequences:

- `session/metadata/update` is the unified settings write. It persists first,
  notifies the actor best-effort, and returns without waiting for an active turn
  to finish.
- Persistence is record-level and actor-dependent: the handler waits for the actor, then appends a full-record `SessionMeta` rollout line (`crates/server/src/runtime/handlers/session.rs:391`). The crash-loss window equals the turn duration.
- The same setting has up to five independently captured copies with no synchronization discipline: actor `state.config` / `state.core.config`, `TurnInlineState.hook_context.config` (turn-start snapshot; updated by approval grants but not by preset changes), the by-value `permission_mode` captured in `build_permission_checker` (`crates/server/src/runtime/turn_exec/query.rs:98`), the by-value `TurnConfig` in the core query loop, and `ToolRuntimeContext.sandbox_profile` (consumed per tool call at `crates/core/src/tools/router.rs:277`).
- The implicit, undocumented promise for every setting is: *blocks until turn end; effective next turn; persisted after actor processing.*

Already aligned with the target model: queue (session plane, durable) vs steer (turn plane, ephemeral channel); the two-level session/turn approval caches; mid-turn approval grants applied directly to `TurnInlineState` (`crates/server/src/runtime/approval.rs:512`); per-turn cancellation tokens.

## Design Decisions

### DD-1: Two planes, and every piece of state declares its plane

The session plane is the durable plane: it owns cross-restart, cross-turn truth. The turn plane is the execution plane: it owns ephemeral, per-execution state derived from the session plane and merged back at turn end.

**Decision**: every settings-related state is explicitly classified as session-durable, session-runtime, turn-ephemeral, or turn-produced-and-merged. Turn-plane state is never persisted; session-plane state is never written by a turn directly (merge at turn end is the only crossing). The system already follows this model implicitly (queue/steer, session/turn approval caches, `TurnInlineState` merge); this design makes it explicit and extends it to settings.

### DD-2: One user-facing API with a complete promise; two internal paths

User intent is atomic ("change this setting"); the API surface must be too.

**Decision**: the unified entry point is the canonical `session/metadata/update` (`SessionSettings` patch + `expected_version`), extended with `{ epoch, applied_to_active_turn }` in its result — no separate `session/settings/update` method is invented (aligned with L2-DES-APP-008 DD-5). The handler internally executes two ordered paths: (1) session-plane persistence, then (2) turn-plane override delivery when a turn is active. Clients never issue two calls for default behavior. An optional `ephemeral: true` flag (turn-only, never persisted) is reserved pending the L1 open question.

Promises of the call:
- **Durability**: the change is persisted before success is returned; persistence failure yields an explicit error.
- **Response timing**: the response never waits for the session actor or an active turn.
- **Effect timing**: with an active turn, at the next decision point per field (DD-6); otherwise from the next turn. `applied_to_active_turn` reports which case applied.
- **Concurrency**: epoch last-write-wins; the resolved value is identical in memory, on disk, and after restart. `expected_version` provides optimistic concurrency against `Session.version`; **`0` skips the check** as a transitional escape for clients that do not track versions yet (removed when all first-party clients track versions).

### DD-3: Persist-first write path

**Decision**: the handler, holding the per-session `state_change_gate`, performs: (1) synchronous append of field-level settings lines to the rollout store; (2) best-effort mailbox notification to the actor (epoch-tagged) to refresh its cached copies, update summary/record, clear caches, and broadcast; (3) when a turn is active, delivery of the override to the turn control plane (DD-5). Success is returned after step (1).

Why the mailbox notification can be best-effort: mailbox FIFO guarantees the notification is processed before the next `ExecuteTurn`, so the next turn's baseline snapshot always includes the change; a crash is covered by the recovery path (DD-4). The actor never writes the live override channel; handlers do.

### DD-4: Field-level append-only settings log with epochs

**Decision**: settings are persisted as field-level append-only rollout lines `(session, field, value, epoch)`, superseding whole-record `SessionMeta` rewrites for settings. Recovery replays the log: the latest value per field wins, and the existing re-derivation logic (`crates/server/src/persistence.rs:1347`) seeds runtime state from it. The epoch is a per-session logical sequence number; replay continues numbering from the maximum seen. During migration, writers dual-write (record meta + field lines) and readers prefer field lines, falling back to `SessionMeta`; dual-write is removed per setting as its slice completes.

### DD-5: Turn control plane; settings override is an ephemeral channel

**Decision**: the mechanisms that reach into a running turn are unified as the *turn control plane*: the cancellation token, the steer channel, and a new settings-override channel. All share the same properties: ephemeral, die with the turn, consumed at decision points, never persisted, delivered without the actor mailbox. The override lives in `TurnInlineState` (already shared behind the active-stream lock, the same path used by mid-turn approval grants) and is written directly by handlers. Decision points synchronously read `effective = overlay ∪ baseline_snapshot`.

### DD-6: Per-field decision points and mid-turn semantics (promise matrix)

Each field declares the decision point at which a change becomes visible and the fate of in-flight work:

| Field | Decision point | Mid-turn semantics |
|---|---|---|
| permission preset / mode / sandbox | each tool-call authorization; each process spawn | Subsequent authorizations and spawns use the new profile; already-spawned processes run to completion; pending interactive approvals are not retracted and their explicit user grants are honored |
| model / model binding | each model call | The next model call in the turn uses the new model; the in-flight request completes; prompt-cache efficiency loss is expected and acceptable |
| reasoning effort | each model call | Same as model |
| collaboration mode | each system-prompt construction (per model call) | The next model call carries the new mode's prompt and tool policy; already-executed tool calls are unaffected. **Implementation status (Phase 4): deferred** — mode drives the session-context/system-prompt build, which is captured once per turn today; live mode requires per-iteration prompt rebuild and lands as a follow-up slice. Mid-turn mode changes currently take effect from the next turn. |
| effective context window (compaction limit) | each auto-compaction check | The next check evaluates against the new limit; past compactions are not undone |

The effective context window has a **different durability target** than the other fields, and this is intentional: the value is persisted globally to the user's `config.toml` (`compaction_token_limit`), so it survives restarts and applies to every session. Each session applies it **clamped** to its own model: `resolved_compaction_limit(global, session_model)` caps the requested window at the model's `context_window` (e.g. requesting 500k against a 128k model yields the model's limit), because a session physically cannot hold more context than its model supports. The clamped value is then pushed live into the session ("hot apply"). The session does not persist its own clamped copy in the rollout; on restore it re-derives the clamped value from the global config and its model. The update also fans out to all other loaded sessions under the same rule (existing behavior, preserved).

**Permission/sandbox interaction (human-approved 2026-08-02)**: permission profile and sandbox profile are separate fields of the canonical `SessionSettings` — policy decision vs execution enforcement — and a single patch may change both atomically. The contractual interaction: when a patch changes `permission_profile` and omits `sandbox_profile`, the sandbox is re-derived from the new preset (`implied_sandbox_profile`, current `ApplyPermissionProfile` behavior); when the patch explicitly carries `sandbox_profile`, the explicit value wins.

Structural session state (cwd, tool registry composition, hook configuration) is excluded from live override and keeps turn-snapshot semantics; mid-turn changes there would make a single turn's behavior unattributable.

### DD-7: Cache invalidation by epoch

**Decision**: approval-cache grants are stamped with the settings epoch under which they were issued; grants from a stale epoch are ignored at authorization time. Explicit user approvals issued *after* the change are unaffected. Turn-scoped caches are cleared when an override is applied to a running turn. This replaces the current actor-side cache clearing, which cannot run during a turn.

### DD-8: Auditability

**Decision**: settings epochs are stamped into authorization decision traces (extending `trace_permission_decision`) and into turn records (epoch at turn start and end). This is a precondition for live behavior, not an optional extra: without it, mid-turn variability would make post-hoc investigation impossible.

### DD-9: Legacy method mapping and behavior change

**Decision**: the four legacy update methods map onto the unified write path (equivalent to one `session/settings/update` call each). Their observable behavior changes in one way: they no longer block until actor processing. This must be called out in the changelog. Deprecation of the legacy methods is deferred to the L1 open question.

### DD-10: Align the wire protocol with the canonical `SessionSettings`

The protocol layer currently carries two divergent models for the same concept: the canonical `SessionSettings` struct with `expected_version` optimistic concurrency (`crates/protocol/src/native/session.rs:128`, `native/rpc_session.rs:169`), and the legacy flat per-concern params (`crates/protocol/src/session.rs:289`, `permissions.rs:62`, `sandbox.rs:9`). This duplication is the source of much of the settings code sprawl.

**Decision**: per L2-DES-APP-008, canonical is the single retained protocol surface. The settings domain converges on the canonical `SessionSettings` model end-to-end: canonical `session/metadata/update` params → handler → settings log → core `TurnConfig`. The legacy flat params are kept only as deserialization aliases that translate into the canonical model at the handler boundary (L2-DES-APP-008 DD-4), then removed with the rest of the legacy surface. No new settings-specific types may be introduced outside the canonical model; where the canonical model lacks a concept needed here (e.g. `applied_to_active_turn` in the result), it is added to the canonical model rather than to a parallel one. The epoch from DD-4 is distinct from `expected_version`: the epoch orders settings writes and stamps traces; `expected_version` lets a client guard against overwriting a concurrent edit.

## Settings Inventory (Current Implementation)

| Setting | Write API → mailbox command | Mid-turn read points | Persistence |
|---|---|---|---|
| permission preset | `session/metadata/update.settings.permissionProfile` → `ApplyPermissionProfile` | mode: by-value capture (`turn_exec/query.rs:98`); profile: turn-inline (`approval.rs:487`); caches: turn-inline / actor | record-level `SessionMeta` (`handlers/session.rs:391`); recovery re-derives profile (`persistence.rs:1347`) |
| sandbox profile | `session/metadata/update.settings.sandboxProfile` → `ApplySandboxProfile` | admission: turn-inline (`approval.rs:576`); execution: `ToolRuntimeContext.sandbox_profile` per call (`core/tools/router.rs:277`) | record-level `SessionMeta` |
| model / binding / effort / collaboration mode | `session/metadata/update` → `UpdateSessionMetadata` | `TurnConfig` by value in the core query loop (`core/query/mod.rs:365`) | record-level `SessionMeta` + SQLite `upsert_session` |
| effective context window | `session/metadata/update.settings.effectiveContextWindow`: persist to `config.toml` first (`config_store.set_compaction_token_limit`), then fan out to all loaded sessions → `ApplyEffectiveContextWindow` per session | `session.config.token_budget` at compaction check (`core/query/mod.rs:151`) | global `config.toml` (`compaction_token_limit`); per-session clamped value re-derived on restore (deliberately not stored in rollout) |

Note: the canonical protocol already defines a unified `SessionSettings` struct (`crates/protocol/src/native/session.rs:128`) and an `expected_version` optimistic-concurrency field on canonical update params; the wire-served handlers currently use the older flat params, and `SessionMetadataUpdateParams` exists in two divergent shapes (`crates/protocol/src/session.rs:289` vs `crates/protocol/src/native/rpc_session.rs:169`). Per L2-DES-APP-008, the settings domain rides the protocol unification: the canonical `session/metadata/update` is the single entry point (DD-10).

## Phased Rollout

Status (2026-08-02): **Phases 1–4 implemented.** Phase 5 (closeout) is in progress; dual-write removal waits for the legacy surface deletion (L2-DES-APP-008 Phase E).

1. **Phase 1 — Field-level settings log (persistence only, no behavior change).** ✅ Implemented: `InternalRecordV2::SessionSettings` + legacy `RolloutLine::SessionSettings` mirror; replay precedence and divergence logging in `persistence.rs`; the canonical sandbox settings patch now persists explicitly (previously lost on restart).
2. **Phase 2 — Persist-first write path + protocol alignment, permission preset first.** ✅ Implemented: dual-shape dispatch on canonical `session/metadata/update`; `SessionSettingsPatch` (partial semantics); `expectedVersion: 0` escape; best-effort `notify_*` actor methods; consolidated metadata notify (the actor overwrites absent fields on non-mode-only updates).
3. **Phase 3 — Turn override channel, permission first.** ✅ Implemented: overlay writes into turn-inline config + `sandbox_profile_live`; `authorize_tool_request` reads the live permission mode; the tool router reads the live sandbox handle per spawn; behavior test `settings_mid_turn::mid_turn_tighten_to_default_triggers_approval_for_network_tool`.
4. **Phase 4 — Remaining fields by template.** ✅ Implemented for model/effort/compact-limit via `LiveTurnSettings` + generation counter in the core query loop (behavior test `mid_turn_model_switch_reaches_next_model_request`). **Deferred**: collaboration mode mid-turn (needs per-iteration system-prompt rebuild); legacy methods remain until Phase E instead of becoming aliases (the TUI is already off them).
5. **Phase 5 — Closeout.** Remove dual-writes and stale read paths; mark actor config copies as caches; remove the legacy flat params after the deprecation deadline; update docs and `AGENTS.md`; full regression.

Rollback: Phase 1 removes the field-line reader branch; Phase 2 reverts the handler; Phase 3 disables overlay writes (readers see an empty overlay, falling back to next-turn semantics). Each phase is independently shippable.

## Phase 1 Implementation Notes (2026-08-02)

Findings that shape Phase 1, recorded during implementation:

- **The write path is already single-write v2**: `append_line` projects legacy `RolloutLine` through the per-file `LegacyProjector` into `RolloutLineV2` rows (`persistence.rs:693-792`); replay inverse-projects (`V2InverseProjector`) back into legacy lines for `ReplayState`. The field-level log therefore rides `InternalRecordV2` (the GoalState/UsageRecord precedent), not a new top-level line family: add `InternalRecordV2::SessionSettings`, a legacy `RolloutLine::SessionSettings` mirror for replay consumption, and both projector mappings.
- The sandbox settings patch persists explicitly; sandbox recovery precedence follows the approved patch-interaction rule (a `permissionProfile` line clears any explicit `sandboxProfile` override seen so far; a `sandboxProfile` line sets the override).
- **No epoch field in Phase 1**: line order in the append-only file already provides total ordering (per-file write lock), and no Phase 1 consumer needs epochs. The `epoch` column of DD-4 is introduced in Phase 2 when writes cross paths (handler-direct vs actor) and races become possible.
- **compaction does not dual-write**: its durability target is global `config.toml` per DD-6.
- **Ephemeral degrade (added 2026-08-02)**: ephemeral sessions have neither a rollout file nor a SQLite index row, so the persist-first path cannot apply. The canonical handler degrades through three metadata sources in order — rollout history (durable), SQLite index metadata, actor summary (the only mailbox read, explicitly scoped to the ephemeral degrade) — skipping field-line writes (ephemeral has no durability by definition) and building the response snapshot from the index/summary projection (`canonical_session_from_index_metadata`). Ephemeral sessions' settings updates remain fully functional; there is simply nothing to persist.
- **Model resolution for the compaction clamp (added 2026-08-02)**: the clamp resolves the session model through the same two-catalog chain as the legacy handler (workspace runtime-context catalog first, then the deps catalog), mailbox-free — the deps catalog alone misses models that only exist in workspace-scoped catalogs.
- **SQLite index refresh (added 2026-08-02)**: after a settings write the handler upserts the SQLite session index from index metadata + applied patch values, so the session list reflects new model/effort/preset values immediately instead of waiting for the next turn event.

## Risks and Mitigations

- **Split-brain during migration** (three copies: store / overlay / actor cache): single write path, epoch last-write-wins, dual-read divergence logging during the dual-write window; keep the window short.
- **Unattributable mid-turn behavior**: epoch-stamped traces and turn records (DD-8) are a precondition.
- **Synchronous disk I/O on the request path**: bounded wait with explicit error on failure; settings writes are rare, user-initiated actions.
- **Persisted-but-unacknowledged window**: idempotent retry via epoch last-write-wins; documented.
- **Half-live slices** (admission live, execution snapshot): one setting's write side and read side land in the same PR.
- **Flaky timing tests**: deterministic blocking fixtures only; no sleeps.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-CONV-006 | 1 | specs/L1/L1-REQ-CONV-006-live-session-settings-update.md | This design refines the live settings update requirement. |
| related-to | L2-DES-SAFETY-001 | 1 | specs/L2/safety/L2-DES-SAFETY-001-permission-system.md | Permission profile semantics consumed by the promise matrix. |
| related-to | L2-DES-SAFETY-002 | 1 | specs/L2/safety/L2-DES-SAFETY-002-approval-mechanism.md | Approval cache invalidation interacts with epoch semantics. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | The field-level settings log extends the rollout data model. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-02 | Assistant | Initial | Initial draft. Status Approved by human 2026-08-02, including the canonical-alignment revision (DD-10), compaction semantics, and the permission/sandbox patch-interaction rule. |
