---
artifact_id: L1-REQ-CONV-005
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-05-22
---

# L1-REQ-CONV-005 — Immediate Message Editing

## Purpose

Define how the user can edit the immediately preceding message in a session without corrupting durable history.

## Why This Matters

Users often notice a typo, missing constraint, wrong file mention, or incorrect instruction immediately after submitting a message. They need a fast correction path, while the transcript still needs to remain auditable and recoverable.

## Background / Context

The current conversation requirements preserve transcript history as an append-only audit trail. They do not define editing of arbitrary historical messages.

Editing older historical messages in place would conflict with append-only persistence, model-context auditability, and tool side-effect visibility. For older corrections, session forking is the safer general mechanism. A narrower edit feature is still useful for the immediately preceding user-authored message in the current session branch.

When the immediately preceding message produced file changes, editing that message semantically supersedes the latest turn. The replacement turn should run against the workspace state that existed before the superseded turn where that state can be restored safely. Tool-driven file changes such as `write` and `apply_patch` can usually be reverted from captured before/after content. Shell commands may also modify files, but their file effects are harder to attribute unless the program captures a turn-level workspace checkpoint.

## User / Business Requirement

The program must support editing the immediately preceding eligible user-authored message in the current session branch, and must attempt to restore file changes made by the superseded latest turn before continuing from the edited message.

## Real User Scenarios

- A user submits a message and immediately notices the wrong file mention.
- A user asks a question, receives an answer, then edits the immediately previous user message to correct the request and regenerate from that corrected input.
- A user edits the immediately previous coding request and expects files modified by that superseded turn to be restored before the corrected request runs.
- A user manually edits a file after the superseded turn, then edits the previous message; the program keeps the user's current file content for that file instead of overwriting it during restoration.
- A user has a queued follow-up message that has not started yet and wants to correct it before execution.
- A user tries to edit an older transcript message and is directed to fork from that point instead of mutating history.

## Functional Requirements

- The program must identify the immediately preceding eligible user-authored message in the current session branch.
- The user must be able to replace that message's content and mentions through an explicit edit action.
- The edit must be represented as a new durable event or revision record rather than in-place mutation of the original transcript record.
- If the target message already produced a completed, failed, or interrupted turn, the original turn and its outputs must remain recoverable for audit.
- After accepting an edit for a completed previous turn, the program should restore the workspace changes attributable to that superseded turn where safely possible, then continue from the edited message by creating a replacement continuation path or replacement turn.
- The program must record enough file-change or checkpoint data during a turn to attempt restoration if that turn is later superseded by immediate message editing.
- For file changes made by tools with structured edit semantics, such as `write` and `apply_patch`, the program should capture before/after file state or an equivalent inverse operation.
- For file changes made by shell commands, the program should restore them only when they are captured by a turn-level workspace checkpoint or otherwise attributable with sufficient confidence.
- If a file has been manually changed after the superseded turn or cannot be restored safely, the program must skip restoration for that file, preserve the current file state, and record that the file was not restored.
- The program may use a git-based turn checkpoint or hidden ghost commit as one possible restoration mechanism, but it must keep that mechanism separate from user-visible commits and branch history unless the user explicitly asks otherwise.
- If the target message is a queued message that has not started, the edit may update the queued message's effective content while preserving the original queued revision for audit.
- If the target message belongs to an active running turn, the program must not silently mutate the already-started model or tool execution. It must interrupt that turn first, wait until the turn reaches a terminal state, then accept the edit under the same rules as editing a completed, failed, or interrupted latest turn (workspace restoration where safe, supersede the interrupted turn, and start a replacement continuation from the edited message). If interruption fails, the edit must fail with a clear explanation rather than mutating in-flight work.
- Older historical messages must not be edited in place. The program should direct the user to fork from the relevant turn when they need to revise older history.
- All connected clients subscribed to the session must observe accepted edits and resulting turn state changes.

## Non-Functional Requirements

- Message editing must preserve an auditable record of the original message and the edited revision.
- Message editing must not hide tool side effects that already occurred before the edit.
- Message editing must not silently discard user-created file changes made after the superseded turn.
- Workspace restoration for immediate message editing must be transparent enough for the user to see which files were restored, skipped, or unsupported.
- Message editing must remain compatible with append-only session persistence and stable context-prefix behavior.
- The current model context after an accepted edit should use the edited message for the replacement continuation path rather than treating both original and edited text as ordinary user intent.

## Acceptance Criteria

- Given the latest completed turn was created from a user message, when the user edits that immediately previous message, then the program records the edit and starts or prepares a replacement continuation from the edited content.
- Given the latest completed turn changed files through restorable file-editing tools, when the user edits the immediately previous message, then those files are restored to their pre-turn state before the replacement continuation runs.
- Given a file changed by the superseded turn has been modified again after that turn, when restoration is attempted, then restoration for that file is skipped and the current file state is preserved.
- Given the superseded turn changed files through a shell command, when those changes are not attributable or checkpointed, then the program reports that automatic restoration for those files is unsupported or skipped.
- Given restoration is partially skipped, when the replacement continuation starts, then the transcript or client state records which files were restored and which current file states were kept.
- Given the edited message replaces a previous completed turn, when the transcript is reviewed, then the original turn remains recoverable or visibly superseded rather than silently deleted.
- Given a queued message has not started, when the user edits it, then the queued message's effective content changes while the original revision remains auditable.
- Given the immediately previous message is part of an active running turn, when the user requests an edit, then the program interrupts that turn, accepts the edit after the turn is terminal, and starts a replacement continuation from the edited message.
- Given the user attempts to edit an older historical message, when the edit is requested, then the program rejects direct editing and offers or indicates session forking as the appropriate path.
- Given one client edits the immediately previous message, when other clients are subscribed to the session, then they receive the edit and resulting turn updates in order.

## Out of Scope

- This requirement does not require arbitrary historical message editing.
- This requirement does not define exact client keybindings, popup layout, or transcript rendering details.
- This requirement does not automatically undo external API calls, network effects, process side effects, published git operations, or other non-file side effects produced by the superseded turn.
- This requirement does not require automatic restoration of shell-created file changes unless a turn-level checkpoint or equivalent attribution exists.
- This requirement does not define branch comparison UI between original and edited continuations.

## Open Questions

- Should accepting an edit automatically regenerate immediately, or should the edited message be staged until the user confirms execution?
- Should the default transcript view collapse superseded turns, or show them inline with a superseded marker?
- Should edits to restored `steer` messages be supported in the same feature, or handled by active-turn message controls?
- Should git-based turn checkpoints be required for git workspaces, optional, or controlled by a user setting?
- Should users be able to opt into destructive turn reset behavior that may discard post-turn manual edits, or should the default per-file skip behavior remain mandatory?

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| related-to | L1-REQ-CONV-003 | 1 | specs/L1/L1-REQ-CONV-003-active-turn-message-handling.md | Active running turns may require steer, queue, or interruption instead of direct mutation. |
| related-to | L1-REQ-CONV-004 | 1 | specs/L1/L1-REQ-CONV-004-session-forking.md | Older historical message revision should use session forking instead of in-place editing. |
| related-to | L1-REQ-CHANGE-001 | 1 | specs/L1/L1-REQ-CHANGE-001-rollback-and-recovery.md | Workspace restoration is a rollback and recovery behavior for superseded turns. |
| related-to | L1-REQ-EDIT-001 | 1 | specs/L1/L1-REQ-EDIT-001-file-editing-workflow.md | Structured file edit tools should capture enough data for restoration. |
| related-to | L1-REQ-GIT-001 | 1 | specs/L1/L1-REQ-GIT-001-change-management.md | Git checkpoints may be used as an internal restoration mechanism without publishing commits. |
| refined-by | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | L2 defines append-only edit records and replacement turn references. |
| related-to | L2-DES-APP-003 | 1 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | L2 defines edit request and broadcast protocol behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial immediate previous message editing requirement. |
| 1 | 2026-05-22 | Human | Refinement | Added turn file restoration behavior when immediate message editing supersedes the latest turn. |
