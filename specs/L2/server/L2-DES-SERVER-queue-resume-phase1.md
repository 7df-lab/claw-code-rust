# L2-DES-SERVER: Queue Resume Phase 1

**Status:** Draft  
**Scope:** Cold restart, warm resume, and session delete boundaries for the pending turn queue and active turn.

## Resume Phase 1 semantics

When a durable session is hydrated (`load_persisted_sessions`, LRU reload, or `session/resume`) and **no turn is active**, the server calls `resume_pending_queue_if_idle` to drain the first queued entry and start a new turn.

- **Interrupted turns are not auto-resumed.** A turn interrupted by shutdown remains in the rollout transcript with `Interrupted` status. Phase 1 does not continue execution from the interrupted checkpoint.
- **Queued input is preserved** in SQLite and restored into the in-memory `pending_turn_queue` on hydrate.
- **Stale steer rows** in the SQLite steer queue degrade into the turn queue on hydrate, except when the steer text (or `clientUserMessageId`) already appears as a materialized `UserMessage` with `entry = steer` in the rollout.

## Warm reconnect / subscription snapshot

`subscription/create` with `includeSnapshot: true` returns `SnapshotData::Session` containing:

- `session` — persisted session metadata
- `active_turn` — current in-flight turn, when any
- `queue` — pending queue entries (same shape as `session/queue/list`)

Desktop clients must apply this snapshot on subscribe/resume so sidebar and composer queue state match the server before incremental notifications arrive.

## Session delete

`session/delete` on a session tree:

1. Signals interrupt on any active turn (waits up to 5 seconds; logs a warning if the turn remains non-terminal).
2. Clears both turn and steer pending queues in SQLite.
3. Removes rollout files and session metadata.

Delete is idempotent for already-deleted sessions.

## L1 gap (open)

`L1-REQ-APP-002` calls for automatically resuming an incomplete active task after restart. Phase 1 **does not** satisfy this for interrupted-turn scenarios: only the pending queue auto-drains when the session is idle. Interrupted-turn auto-resume is deferred to a follow-up epic (checkpoint resume vs. new-turn continuation).
