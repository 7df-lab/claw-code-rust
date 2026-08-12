# Session Interrupt Implementation Plan

**Goal:** Replace the Native `turn/interrupt` request with one `session/interrupt` operation that can stop the active turn, a Native task, or a sessionless command process.

**Architecture:** `session/interrupt` is a Native application command, not a second execution engine. Its server handler resolves the requested interrupt scope and delegates to the existing turn cancellation and command termination services. ACP v1 keeps its standard `session/cancel` wire method; Desktop may call an explicitly registered Devo ACP extension named `session/interrupt`, which delegates to the same handler.

**Constraints:**

- Do not accept or register `turn/interrupt` on the Native surface.
- Keep ACP v1 standard behavior and wire names unchanged, including `session/cancel`.
- Preserve the existing Ctrl-C copy/exit behavior; Esc remains the interactive stop gesture.
- Use Native terminology in new code and documentation.
- Preserve unrelated dirty-worktree changes.

## Files and responsibilities

- `crates/protocol/src/native/rpc_session.rs`: Native interrupt request, scope, and result types.
- `crates/protocol/src/native/methods.rs`: Native method registry replacement.
- `crates/protocol/src/acp/ts.rs`: generated protocol contract replacement for Desktop validation.
- `crates/client/src/client_core.rs`: Native request implementation.
- `crates/client/src/stdio.rs`, `crates/client/src/websocket.rs`: public Rust client wrappers.
- `crates/server/src/runtime/handlers/session_interrupt.rs`: Native application handler.
- `crates/server/src/runtime/handlers/acp.rs`, `crates/server/src/runtime/connection.rs`: ACP extension registration and dispatch.
- `crates/server/src/runtime/command_exec.rs`: connection/session-scoped process termination support.
- `crates/tui/src/worker.rs`, `crates/tui/src/interactive.rs`, `crates/tui/src/chatwidget/input.rs`, `crates/tui/src/chatwidget/worker_events.rs`: active-work interrupt routing and shell busy state.
- `apps/desktop/packages/devo-ai-sdk/src/v2/client.ts`: Desktop abort request migration.
- `apps/desktop/packages/devo-ai-sdk/src/v2/protocol-validation.ts`: Desktop extension validation shape.
- protocol/server/TUI/Desktop tests and protocol documentation: contract coverage and user-facing behavior.

## Implementation sequence

1. Add the typed scope contract. Use a tagged `scope` enum with `session`, `task`, and `command` variants so the process-only `!` path does not require a turn ID or an artificial session. The result reports whether work was accepted and which scope was acted on.
2. Remove `turn/interrupt` from Native method metadata, generated contract registration, Rust client wrappers, and Native dispatch. Keep the internal turn-cancellation helper because ACP `session/cancel` and `session/interrupt` both reuse it.
3. Add server dispatch. Session scope cancels the active turn when present and terminates session-owned command tasks. Task scope reuses the existing task-to-process ownership path. Command scope terminates a process owned by the requesting connection, including a sessionless process.
4. Register `session/interrupt` as an ACP adapter extension for Desktop only. It returns the ACP empty result shape while delegating to the Native application command. ACP standard `session/cancel` remains available and continues to be notification-compatible.
5. Update the TUI worker to send the new scope. Standalone `!` commands use the command scope; session-backed task items use the task scope; active turns use the session scope. Esc is accepted for all active work. Ctrl-C remains reserved for the existing copy/exit flow.
6. Update Desktop `session.abort` to call `session/interrupt`, add its validation contract, and preserve the existing optimistic status/event behavior.
7. Run `cargo fmt --all -- --check`, focused protocol/server/client/TUI tests, Desktop tests/type checks available in the workspace, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`. Search the final tree for executable Native `turn/interrupt` registrations or client calls.
