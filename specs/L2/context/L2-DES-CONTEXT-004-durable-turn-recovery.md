# L2-DES-CONTEXT-004 — Durable execution and turn recovery

- Revision: 1
- Status: Approved
- Active Baseline: yes
- Approval: user implementation request in the current task
- Supersedes: none; additive design across context, tools, persistence and clients

## Required behavior

Accepted tool batches are persisted before dispatch through a fallible acknowledgment.
Terminal outcomes are persisted before model continuation. Recovery must never
automatically rerun a call with an uncertain execution outcome. Compaction preserves
whole tool transactions, pending intents and the order of retained messages.

Full tool output is captured before truncation in session-owned artifacts. Only
registered artifact reads bypass authorization. Reads are bounded and support
continuation without dropping long lines. Capture failure leaves a bounded preview
and an explicit warning, without changing the tool's actual outcome.

Unexpectedly interrupted conversational turns offer Continue/Cancel in Desktop
and TUI. Continue retains turn identity, current durable context, usage and limits,
without a synthetic user message. Explicit Stop/Cancel prevents recovery. Recovery
availability is distinct from execution liveness. Live execution remains owned by
the active-turn registry through merge. Client reconnect alone is not interruption.

Recovery, intent and artifact records are append-only. Existing rollouts remain
readable. Native owns behavior; external adapter wire contracts remain unchanged.
Message editing is outside this revision.

## Verification

Required acceptance coverage includes transaction boundaries, summarizer retries, incomplete calls, persisted
outcomes, crash windows, idempotent recovery, artifact pagination and identity,
same-turn continuation, explicit cancellation and authoritative client state.

## Native transport mapping

The existing implementation uses `session/interrupt` with a session scope for
Stop and recovery Cancel. It does not add a parallel `turn/stop` endpoint.
Continue uses `turn/resume`; `turn/recovery/read` supports authoritative refresh.
The additive notifications are `turn/recoveryUpdated` and `turn/resumed`.
See [client behavior](../../../docs/turn-recovery.md).
