# Interrupted turn recovery

Native clients read `turn/recovery/read` with `sessionId`. A non-null recovery
contains the existing `turnId`, a `revision`, an execution `attempt`, and a short
reason. Recovery is independent of the historical turn status; a connected live
server task does not offer recovery merely because a client reconnects.

Continue calls `turn/resume` with `sessionId`, `expectedTurnId`,
`recoveryRevision`, and an `idempotencyKey`. Retrying the same request must reuse
the key. A stale revision requires a fresh recovery read. The response identifies
the same turn and its execution attempt. No user input is appended by Continue.
Cancel uses the existing `session/interrupt` session scope, including when there
is no live cancellation token. Explicit cancellation is persisted.

Native `turn/recoveryUpdated` informs clients of recovery availability after the
execution state has merged. `turn/resumed` identifies the existing turn and new
attempt. Clients must update that turn rather than append another transcript turn.
ACP does not gain these methods or notifications.

Desktop displays a nonmodal panel above the composer. TUI uses a dedicated bottom
pane with Ctrl+R to Continue and Ctrl+X to Cancel. Composer drafts remain editable;
ordinary Enter cannot accept recovery or submit a new message while a decision is
pending. Failed requests retain the panel and display an error.

The execution journal acknowledges complete tool batches before dispatch and
terminal outcomes before model continuation. An uncertain call produces an
interrupted outcome describing possible execution; Continue is not authorization
to repeat its side effects. A saved final model response can finish terminal
bookkeeping without another model request.

Generated output notices identify a saved artifact. Registered output reads use
bounded line pages (`offset`, `limit`) or byte pages (`byteOffset`, `byteLimit`).
The page footer provides a continuation offset for long lines. A byte page has a
minimum four-byte budget so a UTF-8 codepoint can be returned intact. Capture
status distinguishes complete output, output captured so far, and an incomplete
capture. This read exemption does not authorize writes, execution, or unrelated
filesystem paths.
