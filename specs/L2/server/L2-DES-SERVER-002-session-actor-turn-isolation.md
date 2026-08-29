---
artifact_id: L2-DES-SERVER-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-29
---

# L2-DES-SERVER-002 — Session Actor / Turn Execution Isolation

## Purpose

Define the concurrency boundary between the per-session actor mailbox and turn execution so mailbox commands remain short, RPC handlers stay responsive during active turns, and durable session state has a single writer.

## Source Requirements

- `L1-REQ-APP-001` — shared server-side agent capability must stay usable while work runs.
- `L1-REQ-CONV-006` / `L2-DES-CONV-002` — settings writes must not wait on turn completion; live overrides ride the turn control plane.
- `L1-REQ-AGENT-002` / `L2-DES-AGENT-002` — interrupt must cancel in-flight work without hanging the session control surface.
- `L2-DES-AGENT-001` — execution engine owns model/tool I/O.

## Problem

Historically, `SessionCommand::ExecuteTurn` ran `query()` inline on the session actor task. The mailbox stopped draining for the full model+tool duration. Handlers that still awaited mailbox round-trips (`summary()`, `record()`, reservation snapshots) appeared hung; desktop clients hit their 10s RPC timeout. Compaction already used the correct pattern (spawned task + short mailbox commands); regular turns did not.

## Design Decisions

### DD-1: Actor mailbox commands are short

Every `SessionCommand` must complete without awaiting unbounded I/O (provider streams, tool processes, client reverse-RPC). Allowed: in-memory mutation, short synchronous disk appends, cloning Arcs. Forbidden: `query()`, waiting on approval/user-input oneshots from inside the actor task, holding `state_change_gate` across those awaits.

### DD-2: Turns execute on a spawned task with a working copy

Turn admission (`BeginActiveTurn` / `TryBeginActiveTurn`) remains an actor command. Execution:

1. Handler registers runtime handles via `spawn_active_turn_task`.
2. The turn task performs a short `CheckoutTurnWorkingSet` mailbox round-trip (install `TurnInlineState`, clone turn-owned state, share queue/stream Arcs).
3. The task runs model query + finalization against the working copy.
4. The task sends a short `MergeTurn` command; the actor is the only writer that installs durable conversation state.

`MergeTurn` is the sole turn→session crossing for conversation ownership.

### DD-3: Two planes (aligned with L2-DES-CONV-002)

| Plane | Owner | Mid-turn writes |
|---|---|---|
| Session-durable | Actor + persist-first handlers | Settings/title via disk then `notify_*`; never blocked on turn I/O |
| Turn-ephemeral | Turn task + shared control plane | Cancel token, steer queue, pending queue mutex, `TurnInlineState` live overlays |

Control-plane Arcs exist so decision points read the latest value with zero mailbox hops—not as a workaround for a blocked actor.

### DD-4: `state_change_gate` never spans unbounded I/O

Admission, rollback, message-edit, and compaction apply take the gate only for short critical sections (snapshot / commit). Title generation and compaction summarization must not hold the gate while awaiting the model.

### DD-5: Interrupt is one path

Cancel the turn token and wait for terminal status recorded by finalization/`MergeTurn`. Hard-abort the spawned task and claim leftover `active_turn` via the mailbox only as orphan recovery when the task dies without merging—same shape for regular turns and manual compaction.

### DD-6: Merge must not clobber session-plane updates

During a turn, persist-first settings may update actor `config` / summary via `notify_*` while the working copy still holds turn-start conversation state. `MergeTurn` installs turn-owned fields (messages, tokens, history/items produced by the turn, terminal turn metadata) and preserves actor-side session-plane config/settings that landed mid-turn.

## Ownership Matrix

**Turn-owned (working copy → MergeTurn):** `SessionState` conversation (`messages`, turn bookkeeping, token counters), turn-produced history/persisted items (via inline merge), terminal `latest_turn` / cleared `active_turn` as finalized by the turn task.

**Actor-owned always:** mailbox command processing, idle-session structural edits, responding to short reads (`GetSummary`, `GetRecord`, …).

**Shared control plane (Arc):** `pending_turn_queue`, `steer_input_queue`, `SessionStreamState` / `TurnInlineState`, cancel token in `ActiveTurnRegistry`.

## Non-Goals

- Changing Native `turn/start` to block until the turn ends.
- Putting pending-queue ops back onto a pure mailbox serial path.
- Sharing one locked `SessionState` across threads instead of checkout/merge.

## Implementation Anchors

- `crates/server/src/runtime/session_actor/` — mailbox, `TurnWorkingSet`, `MergeTurn`
- `crates/server/src/runtime/turn_exec/` — query + finalize on working set
- `crates/server/AGENTS.md` — concurrency rules of record
- `specs/L2/conv/L2-DES-CONV-002-two-plane-session-settings.md` — live settings overlay

## Verification

- Mid-turn `session/list`, `session/items/list`, `workspace/changes/read`, `runtime/ping` return within a short client-facing bound (well under desktop 10s timeout).
- Existing mid-turn settings, queue push/steer, and interrupt suites remain green.
- Parent mailbox remains responsive while a child turn publishes usage.

## Revision Notes

- Rev 1: Initial draft capturing actor/turn isolation after diagnosing mailbox blocking under inline `ExecuteTurn`.
