## Server runtime concurrency

The server runtime uses **one session actor per session**. Durable session state lives in `SessionActorState`, owned exclusively by that actor's mailbox loop (`run_session_actor`). Cross-session coordination and in-flight turn execution handles live on `ServerRuntime`.

### Ownership and actor boundaries

- **Mutate durable session state only through `SessionHandle` → `SessionCommand`.** Do not reach into `SessionActorState` from handlers or turn tasks except inside the actor loop or via explicit snapshot/command APIs.
- **Actor mailbox commands must be short.** No unbounded I/O (`query()`, tool waits, client reverse-RPC) inside the actor task. See `L2-DES-SERVER-002`.
- **Turns execute on a spawned task** with a checked-out `TurnWorkingSet`. Checkout / `MergeTurn` are the only turn↔actor crossings for conversation ownership.
- **`ActiveTurnRegistry` is the single source for in-flight turn execution handles** (cancel tokens, abort handles, connection routing, spawn snapshots, active stream state). Register on turn start. Use `clear_active_turn_interrupt_handles` during finalization so stream/spawn mirrors stay available until merge; use `clear_active_turn_runtime_handles` for full teardown.
- **Use `turn_lifecycle` helpers** (`register_active_turn_execution`, `spawn_active_turn_task`, `signal_active_turn_interrupt`) instead of touching `ActiveTurnRegistry` fields ad hoc from handlers.
- **Interactive waits (approval, `request_user_input`) live in `SessionInteractiveLanes`, not the session actor.**
- **Post-turn scheduling runs outside the turn task and actor.** After `MergeTurn`, continuation (queued follow-ups, goal continuation) is spawned via `spawn_post_turn_scheduling`—never inline in the mailbox handler.

### Lock usage

- **Never hold `ServerRuntime.sessions` (or other runtime `Mutex` maps) across `.await`.** Look up the `SessionHandle`, drop the lock, then call handle methods.
- **`state_change_gate` must not span unbounded I/O** (model calls, long disk waits). Hold it only for short admission / apply critical sections.
- **Mutate `pending_turn_queue` / `steer_input_queue` through shared mutexes** for mid-turn control (last-write-wins). Those Arcs are the control plane, not a blocked-mailbox workaround.
- **`SessionStreamState` uses `Arc<tokio::sync::Mutex<…>>`** and is shared with the turn event stream task. Prefer actor commands for durable merges; use the stream lock only for streaming-era fields (deferred assistant/reasoning, inline turn scratch state).
- **From turn event streams, prefer `try_send` on the session mailbox** for fire-and-forget updates when the caller might still be awaited by actor-side work.
- **Interrupt/cancel:** `signal_active_turn_interrupt` cancels the token only. Hard `abort_task` is for orphan recovery after the terminal-status wait times out—aborting immediately would skip `MergeTurn`.

### Turn lifecycle

- **Reservation:** use `TryBeginActiveTurn` (idle session + empty pending queue) or turn-reservation snapshots when starting turns from handlers.
- **Terminal status:** turn tasks finalize via `finalize_executed_turn`, then `MergeTurn` installs durable state; cancel-token interrupts record terminal status the same way.
- **Always record terminal turn status** (`record_terminal_turn_status`) and clear runtime handles when a turn ends or is interrupted.
- **Subagent usage:** only root sessions own a parent usage ledger; child turns publish into the parent's ledger.

### Queues

- **`pending_turn_queue`:** user-visible queued turns while a session is busy. Enqueue via shared mutex or `SessionHandle::enqueue_pending_turn_input`.
- **`steer_input_queue`:** input for injection into an active turn. Active-turn handlers mutate it through the reservation snapshot's shared mutex; finalization either consumes it or degrades unconsumed input to `pending_turn_queue`.
- **After dequeuing,** broadcast queue updates and start the next turn from a spawned task (`chain_queued_followup_turn` / `spawn_next_turn_from_queue`).

### Tests

- **Runtime concurrency changes need integration coverage** in `crates/server/tests/`: interrupt mid-stream, queued follow-ups, goal lifecycle interrupts, persistence/resume, and mid-turn read RPCs (`session/list`, `session/items/list`, `workspace/changes/read`, `runtime/ping`).
- **Prefer waiting on observable protocol outcomes** (notifications, terminal status) over sleeping or polling internal maps.
- Follow existing test conventions: `pretty_assertions::assert_eq`, compare whole objects where possible, platform-aware paths when touching filesystem behavior.

### Session persistence layers

- **Rollout JSONL** under `~/.devo/sessions/` is the canonical conversation history.
- **SQLite** (`devo.db` `sessions` table) stores a lightweight index (`rollout_path`, `parent_session_id`, title, cwd, timestamps) used by `session/list` and resume decisions.
- **In-memory session actors** are loaded on demand via `get_or_load_parent_session`; root sessions are LRU-evicted (capacity 16) when unpinned.
- **`session/list`** returns durable user-visible sessions only (non-ephemeral, no `agent_path`; includes forks with `parent_session_id`); subagent rows are indexed but hidden from list.
- **`session/resume`** loads parent sessions lazily from rollout files. Subagent session ids cannot be resumed directly; missing rollout files fail with an explicit restore error.
- **Startup** runs `index_rollout_metadata` in the background instead of replaying every rollout into memory.

### Session title generation

See [`specs/L2/server/L2-DES-SERVER-title-generation.md`](../../../specs/L2/server/L2-DES-SERVER-title-generation.md) (Rev 2 Draft: heuristic + optional LLM polish). Historical Rev 1: [`L2-DES-SERVER-title-generation.rev1.md`](../../../specs/L2/server/L2-DES-SERVER-title-generation.rev1.md).

Orchestration lives in `runtime/session_title.rs` (`prepare_title_from_user_input`, `notify_title_polish`, `cancel_auto_title_generation`). First user input applies `Final(Heuristic)` immediately; LLM polish runs only when the session is idle. Clients must not derive display titles locally — they consume `session/metadataUpdated`.
