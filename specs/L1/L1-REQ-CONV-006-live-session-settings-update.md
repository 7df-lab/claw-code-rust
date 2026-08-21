---
artifact_id: L1-REQ-CONV-006
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-08-02
---

# L1-REQ-CONV-006 — Live Session Settings Update

## Purpose

Define what must happen when the user changes a session-scoped setting — permission preset, sandbox profile, model, reasoning effort, collaboration mode, or compaction limit — at any point in the session lifecycle, including while a turn is actively running.

## Background / Context

Session settings today are applied through the session actor's mailbox. Because the actor is occupied for the whole duration of an active turn, a settings change issued mid-turn is queued behind the turn: the client request appears to hang until the turn completes, the change only takes effect from the next turn, and the change is persisted only after the actor processes it — leaving a crash window as long as the turn itself in which the user's change would be lost.

Users think of a settings change as a single intent: "from now on, play by the new rules, and do not lose it." The product must honor that intent with one call, explicit promises, and no hidden waiting.

The product model distinguishes two levels of state:

- `session` level: durable state that must survive restarts and apply to future turns.
- `turn` level: ephemeral control state that affects the currently running turn and dies with it.

## User / Business Requirement

When a user changes a session setting, the program must acknowledge the change only after it is durably persisted, must never make the client wait for an active turn to finish, and must apply the change to the currently running turn at the next well-defined decision point (or to the next turn when no turn is active).

## Real User Scenarios

- A user watches the agent run commands with broad permissions and tightens the permission preset mid-turn; subsequent tool calls in the same turn are authorized under the new preset.
- A user sees the current model struggling and switches the model mid-turn; the next model call within the same turn uses the new model.
- A user changes a setting, quits the program before the active turn finishes, relaunches, and finds the new setting restored.
- A user changes a setting twice in quick succession; the final state reflects the last change, both in the running process and after a restart.

## Functional Requirements

- A single API call carries the user's full intent; clients are never required to issue separate "session" and "turn" calls for the default behavior.
- The change is persisted before the server acknowledges success; a failure to persist produces an explicit error response.
- The server responds without waiting for any active turn to complete.
- If a turn is active, the change takes effect at the next decision point for that setting (e.g. next tool-call authorization, next model call, next compaction check); the response indicates whether an active turn was affected.
- If no turn is active, the change applies from the next turn.
- Concurrent changes to the same session resolve deterministically: last write wins, and the resolved state is identical in memory, on disk, and after restart.
- Changing a setting invalidates implicit cached approvals derived from the previous setting; explicit user-granted approvals already issued are not retracted.

## Non-Functional Requirements

- Every authorization-relevant decision made under live settings must be attributable after the fact: decision traces and turn records carry the settings epoch in effect.
- The settings write path must not perform unbounded waits on the session actor or on disk; failures surface as request errors.
- The behavior of each setting under mid-turn change must be documented as a contract, not inferred from implementation.

## Acceptance Criteria

- Given an active turn blocked inside a tool call, when the user updates the permission preset, then the request returns promptly with success, the new preset is persisted, and subsequent tool-call authorizations in that turn use the new preset.
- Given an active turn, when the user updates a setting and the process crashes before the turn ends, then after restart the session shows the new setting.
- Given no active turn, when the user updates a setting, then the request returns promptly and the next turn uses the new setting.
- Given two rapid consecutive updates to the same setting, when both complete, then the last update is the effective value in the running process and after restart.
- Given a setting change mid-turn, when later investigating an unexpected tool authorization, then traces show which settings epoch authorized it.

## Out of Scope

- Revoking or modifying already-running tool executions (e.g. killing a spawned process when the sandbox tightens); the new setting applies from the next decision point onward.
- Turn-structural changes that would make a single turn internally inconsistent (working-directory changes, tool registry composition); these keep turn-snapshot semantics.
- Ephemeral "this turn only, never persisted" overrides as a user-facing feature (see Open Questions).
- Changes to global/workspace configuration files as a side effect of session settings updates.

## Open Questions

- Should the API also expose an explicit ephemeral mode (`ephemeral: true`, applied to the active turn but never persisted), or is the default "persist + apply everywhere" sufficient for all clients?
- The legacy per-concern update methods are removed; `session/metadata/update`
  with its partial `settings` patch is the unified external method.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refined-by | L2-DES-CONV-002 | 1 | specs/L2/conv/L2-DES-CONV-002-two-plane-session-settings.md | The two-plane session settings design refines this requirement into an architecture, API contract, and per-setting promise matrix. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-02 | Assistant | Initial | Initial draft. |
