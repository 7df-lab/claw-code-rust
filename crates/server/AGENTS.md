## Server runtime concurrency

The server runtime uses **one session actor per session**. Durable session state lives in `SessionActorState`, owned exclusively by that actor's mailbox loop (`run_session_actor`). Cross-session coordination and in-flight turn execution handles live on `ServerRuntime`.

### Ownership and actor boundaries

- **Mutate durable session state only through `SessionHandle` → `SessionCommand`.** Do not reach into `SessionActorState` from handlers or turn tasks except inside the actor loop or via explicit snapshot/command APIs.
- **`ActiveTurnRegistry` is the single source for in-flight turn execution handles** (cancel tokens, abort handles, connection routing, spawn snapshots, active stream state). Register on turn start. Use `clear_active_turn_interrupt_handles` during in-actor finalization so stream/spawn mirrors stay available until final state merges; use `clear_active_turn_runtime_handles` for full teardown.
- **Use `turn_lifecycle` helpers** (`register_active_turn_execution`, `spawn_active_turn_task`, `signal_active_turn_interrupt`) instead of touching `ActiveTurnRegistry` fields ad hoc from handlers.
- **Turns execute in-actor** via `SessionCommand::ExecuteTurn`.
- **Interactive waits (approval, `request_user_input`) live in `SessionInteractiveLanes`, not the session actor.** The actor must not block the mailbox waiting on client responses.
- **Post-turn scheduling runs outside the actor.** After `ExecuteTurn` replies, continuation (queued follow-ups, goal continuation) is spawned in a background task—never inline in the mailbox handler when interrupts may still be in flight.

### Lock usage

- **Never hold `ServerRuntime.sessions` (or other runtime `Mutex` maps) across `.await`.** Look up the `SessionHandle`, drop the lock, then call handle methods.
- **Mutate `pending_turn_queue` only through actor commands** (`EnqueuePendingTurnInput`, `RemoveQueuedTurnInput`, `TakeQueuedTurnInputForSteer`, `PopQueuedTurnInput`). Handlers must not lock the queue directly while a session actor is running.
- **`SessionStreamState` uses `Arc<tokio::sync::Mutex<…>>`** and is shared with the turn event stream task. Prefer actor commands for durable merges; use the stream lock only for streaming-era fields (deferred assistant/reasoning, inline turn scratch state).
- **From turn event streams, use `try_send` on the session mailbox** for fire-and-forget updates (`SetActiveGoal`, `ApplyParentUsageSnapshot`, `TouchLastActivity`). Blocking `send().await` from a stream the actor is waiting on can deadlock.
- **Interrupt/cancel:** call `signal_active_turn_interrupt` before relying on mailbox round-trips—the actor may be blocked in permission wait.

### Turn lifecycle

- **Reservation:** use `TryBeginActiveTurn` (idle session + empty pending queue) or turn-reservation snapshots when starting turns from handlers.
- **Terminal status:** in-actor turns finalize via `finalize_executed_turn` when the cancel token fires.
- **Always record terminal turn status** (`record_terminal_turn_status`) and clear runtime handles when a turn ends or is interrupted.
- **Subagent usage:** only root sessions own a parent usage ledger; child turns publish into the parent's ledger.

### Queues

- **`pending_turn_queue`:** user-visible queued turns while a session is busy. Enqueue via `SessionHandle::enqueue_pending_turn_input`; pop/remove/steer via actor commands only.
- **`steer_input_queue`:** input for injection into an active turn. Active-turn
  handlers mutate it through the reservation snapshot's shared mutex rather
  than waiting on the actor mailbox; finalization either consumes it or
  degrades unconsumed input to `pending_turn_queue`.
- **After dequeuing,** broadcast queue updates and start the next turn from a spawned task (`chain_queued_followup_turn` / `spawn_next_turn_from_queue`).

### Tests

- **Runtime concurrency changes need integration coverage** in `crates/server/tests/`: interrupt mid-stream, queued follow-ups, goal lifecycle interrupts, and persistence/resume.
- **Prefer waiting on observable protocol outcomes** (notifications, terminal status) over sleeping or polling internal maps.
- Follow existing test conventions: `pretty_assertions::assert_eq`, compare whole objects where possible, platform-aware paths when touching filesystem behavior.

### Session persistence layers

- **Rollout JSONL** under `~/.devo/sessions/` is the canonical conversation history.
- **SQLite** (`devo.db` `sessions` table) stores a lightweight index (`rollout_path`, `parent_session_id`, title, cwd, timestamps) used by `session/list` and resume decisions.
- **In-memory session actors** are loaded on demand via `get_or_load_parent_session`; root sessions are LRU-evicted (capacity 16) when unpinned.
- **`session/list`** returns durable user-visible sessions only (non-ephemeral, no `agent_path`; includes forks with `parent_session_id`); subagent rows are indexed but hidden from list.
- **`session/resume`** loads parent sessions lazily from rollout files. Subagent session ids cannot be resumed directly; missing rollout files fail with an explicit restore error.
- **Startup** runs `index_rollout_metadata` in the background instead of replaying every rollout into memory.
