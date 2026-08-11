---
artifact_id: L2-DES-AGENT-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-22
---

# L2-DES-AGENT-002 — Interrupt And Resume Control

## Purpose

Define how the server interrupts, cancels, inspects, and resumes agent work running inside the execution engine.

## Background / Context

`L2-DES-AGENT-001` defines the normal execution path from accepted user input to terminal turn status. Long-running agent work may be in model generation, tool execution, approval waiting, question waiting, background process supervision, context compaction, or finalization when the user interrupts it.

Interrupt and resume must be server-owned because multiple clients can observe or control the same session. A TUI, desktop client, or IDE client may initiate the interrupt, but the server owns canonical runtime state, tool cleanup, durable records, and resumed execution.

## Source Requirements

- `L1-REQ-AGENT-002` requires users to interrupt, cancel, inspect, and resume work where recovery is possible.
- `L1-REQ-AGENT-001` requires a complete execution workflow with visible task state.
- `L1-REQ-TOOL-005` requires background process inspection and manual stop behavior.
- `L1-REQ-CONV-002` requires observable and durable turn lifecycle behavior.
- `L1-REQ-APP-002` requires persistence and recovery behavior.
- `L1-REQ-APP-011` requires actionable error recovery.
- `L1-REQ-TOOL-001` requires safe tool execution and redaction.
- `L2-DES-AGENT-001` defines the execution engine being interrupted or resumed.
- `L2-DES-APP-003` defines client requests and server notifications.
- `L2-DES-CONV-001` defines durable turn and item records.

## Design Requirement

The server should provide timely user control over active execution while preserving completed work, partial outputs, file-change state, and enough context to resume when possible.

Interrupting a turn should stop or transition active work into a safe state. It should not silently discard durable history, silently leave program-started background work running, or make clients guess whether execution is still active.

## Control Actions

The design distinguishes these conceptual actions:

- `interrupt`: request that the active turn stop as soon as possible while preserving partial state.
- `cancel_tool`: request cancellation of one running tool call where the tool supports cancellation.
- `stop_background_process`: request termination of a tracked background process started by the program.
- `resume`: start a continuation from an interrupted turn with awareness of prior progress.
- `inspect_active_work`: return the active turn phase, running tool calls, pending prompts, and tracked background processes.

The exact protocol method names are defined by `L2-DES-APP-003`, but the runtime semantics are owned by this design.

## Interrupt Targets

An interrupt request may target:

- The active model invocation.
- The currently executing tool call or parallel tool group.
- The entire active turn.
- A tracked background process associated with the session or turn.
- A waiting approval or question prompt.
- Manual session compaction started via `session/compact` (client `/compact`),
  which is an active `ManualCompaction` turn.

The server should resolve the target into a runtime cancellation token, tool supervisor command, provider stream cancellation, or waiting-state transition. If the target is no longer active, the server should return an idempotent success or a structured stale-state error.

Manual `/compact` is canceled with `turn/interrupt` or ACP `session/cancel`
(which routes to interrupt when a turn is active). Cancellation dual-emits
`session/compaction/failed` with a canceled message plus the normal interrupted
turn lifecycle; turn-scoped automatic compaction remains under the parent turn's
cancel token / `turn/interrupt`.

## Interrupt Flow

Conceptual interrupt flow:

```text
Client sends interrupt request
        ↓
Server validates session, turn, target, and permissions
        ↓
Server records interrupt requested state
        ↓
Server signals provider/tool/process/waiting state
        ↓
Server drains or bounds partial output
        ↓
Server records final interrupted state
        ↓
Server broadcasts canonical turn/tool/process updates
```

The immediate client response confirms that interruption was accepted or rejected. It must not wait for every process cleanup action to finish.

## Runtime State

Conceptual interrupt state fields:

- `interrupt_id`
- `session_id`
- `turn_id`
- `requested_by_client_id`
- `target_kind`
- `target_id`
- `interrupt_mode`
- `requested_at`
- `accepted_at`
- `status`: requested, stopping, interrupted, completed_before_interrupt, failed, or rejected.
- `cleanup_state`
- `message`

Execution phases should map to interrupt behavior:

| Phase | Expected Interrupt Behavior |
|---|---|
| Admission | Reject if no active execution exists or return stale success if already terminal. |
| Context assembly | Stop before provider invocation where possible and mark the turn interrupted. |
| Model invocation | Cancel or drop the provider stream, preserve partial content, and mark the turn interrupted. |
| Tool dispatch | Request tool cancellation where supported and record completed, failed, or canceled tool states. |
| Waiting for approval or question | Resolve the wait as interrupted or canceled and mark the turn interrupted. |
| Background process running after tool return | Keep process visible unless the user explicitly stops it or policy requires cleanup. |
| Finalization | If terminal status has already been persisted, return stale success with the existing terminal state. |

## Provider Interruption

When a model invocation is interrupted, the engine should:

- Stop reading provider stream events where possible.
- Cancel the underlying HTTP request or provider stream where supported.
- Persist partial assistant or reasoning content already accepted by the engine.
- Record usage received before the interruption if available.
- Mark the active invocation as interrupted or canceled.
- Mark the turn as interrupted unless execution has already reached a terminal state.

If the provider cannot be canceled cleanly, the server should stop forwarding additional output to the turn and record cleanup status separately.

## Tool Interruption

Tool interruption depends on tool capabilities:

- Read-only or short-lived tools may complete before cancellation takes effect.
- Structured mutating tools should either complete atomically or report partial failure state.
- Command execution tools should attempt process-group or runtime-specific termination according to tool design.
- Background processes that outlive the originating tool call must remain visible until they exit or are stopped.
- Tool output already emitted before interruption remains part of the interrupted turn history.

The engine should not claim a tool was stopped until the tool supervisor reports stopped, exited, failed-to-stop, or detached-visible state.

## Active Work Inspection

The server should expose enough active work state for clients to let users make informed stop decisions.

Conceptual active work projection fields:

- `session_id`
- `active_turn_id`
- `turn_status`
- `turn_phase`
- `active_invocation_id`
- `running_tool_calls`
- `pending_approvals`
- `pending_questions`
- `background_processes`
- `workspace_change_set_status`
- `last_event_sequence`

This projection should be safe for client display and should not include plaintext secrets or unredacted sensitive tool output.

## Durable Recording

Interrupt behavior must be append-only from the durable session perspective.

Durable records should preserve:

- The interrupt request.
- The interrupted turn terminal state.
- Partial assistant, reasoning, tool call, and tool result items already accepted.
- Tool cancellation outcomes.
- Background process state or references.
- Workspace change-set state for file changes completed before interruption.
- Resume links if work is resumed later.

The program should not remove partial records merely because a turn was interrupted.

## Resume Semantics

Resuming an interrupted task should create a continuation turn linked to the interrupted turn. The interrupted turn remains durable and terminal; the resume turn carries a `resume_of_turn_id` or equivalent provenance link.

Resume context should include:

- The original user request.
- Partial assistant output where useful.
- Completed tool calls and tool results.
- File-change summary or workspace change-set state.
- Background process state relevant to the task.
- Any user-provided resume instruction.
- Current session metadata and model selection.

The resumed turn should use the normal execution engine from `L2-DES-AGENT-001`. It should not reinterpret already-executed model or tool work as if it had never happened.

## Resume Eligibility

A resume request should be accepted only when:

- The target turn is interrupted or otherwise recoverable.
- The session still exists and can be opened.
- The workspace is available or the user has accepted degraded behavior.
- Required context records are available or a safe degraded context can be assembled.
- The requested resume does not conflict with an active turn unless it is queued or explicitly allowed.

If context is missing, compacted, deleted, or unsafe to reuse, the server should reject the resume or start a new turn with an explicit warning that it cannot fully resume prior state.

## Crash And Restart Recovery

After process restart, durable replay should reconstruct completed and interrupted turn history. If replay finds a turn that was active without a terminal record when the server stopped, the program should mark it as interrupted, failed-recoverable, or recovery-required according to L3 policy before allowing resume.

The server should not pretend that an in-flight provider stream or external process continued safely across a crash unless a supervisor can prove that state.

## Client Behavior

Clients may initiate interrupts, display active work, and initiate resume, but clients do not own the canonical state transition.

Clients should:

- Present immediate interrupt acknowledgement.
- Reconcile local display with server-confirmed status events.
- Show cleanup-pending state when tools or background processes have not stopped yet.
- Display partial work and file-change summaries after interruption.
- Initiate resume through server protocol rather than replaying local transcript content themselves.

## Invariants

- Interrupt responses are timely and do not wait for all cleanup to finish.
- Completed records remain durable after interruption.
- A turn interrupted by the user reaches a visible terminal or cleanup-pending state.
- Background processes started by the program remain visible if they continue after interruption.
- Resume creates linked continuation state instead of mutating the interrupted turn in place.
- Resumed execution uses the normal execution engine and normal safety policy.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-AGENT-002 | 1 | specs/L1/L1-REQ-AGENT-002-interrupt-resume.md | Defines server-owned interrupt, active work inspection, and resume behavior. |
| related-to | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Interrupt and resume act on the execution engine workflow. |
| related-to | L1-REQ-TOOL-005 | 1 | specs/L1/L1-REQ-TOOL-005-background-process-management.md | Background process inspection and stopping are part of interrupt control. |
| related-to | L1-REQ-CONV-002 | 1 | specs/L1/L1-REQ-CONV-002-turn-lifecycle.md | Interrupt and resume update visible turn lifecycle state. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Defines the execution runtime being interrupted and resumed. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Protocol methods and events expose interrupt and resume control to clients. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable records preserve interrupted and resumed state. |
| specified-by | L3-BEH-SERVER-001 | 4 | specs/L3/server/L3-BEH-SERVER-001-server-runtime-transport.md | L3 defines server cancellation token wiring and active work ownership. |
| specified-by | L3-BEH-SERVER-002 | 2 | specs/L3/server/L3-BEH-SERVER-002-interrupt-resume.md | L3 defines interruption targets, resume turns, active work inspection, and crash recovery. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial interrupt and resume control design. |
