# L2-DES-SERVER: Session title generation

| Field | Value |
|-------|-------|
| Artifact ID | L2-DES-SERVER-title-generation |
| Revision | 2 |
| Status | Draft |
| Active Baseline | no |
| Supersedes | Rev 1 after approval |

Draft design note for automatic session titles (heuristic first, optional LLM polish).

Rev 1 remains the historical Approved baseline and active implementation authority until this revision is approved.

## State machine

```
Unset → Final(Heuristic) → Final(ModelGenerated)   // polish success
              ↓ (polish fail / retry)
         Final(Heuristic)   // keep readable title

Unset → Final(ExplicitCreate)   // session create with title
*     → Final(UserRename)       // metadata title patch
```

- **Unset**: No display title. Clients show the fixed placeholder `"New Chat"`.
- **Final(Heuristic)**: Immediate truncated title from first user input. Always set before turn work continues. Clients show this string — never an empty placeholder after a real message.
- **Final(ModelGenerated)**: Optional LLM polish that may replace **only** `Final(Heuristic)`.
- **Final(UserRename)** / **Final(ExplicitCreate)**: Higher-priority durable titles; auto polish never overwrites them.
- **Generating**: Retained for wire/rollout compatibility only. The auto first-message path must **not** broadcast empty-title `Generating`.

`Final` carries `SessionTitleFinalSource`:

| Source | Meaning |
|--------|---------|
| `Heuristic` | Truncated first user message (no LLM) |
| `ModelGenerated` | LLM polish of the heuristic title |
| `UserRename` | `session/metadata/update` title patch |
| `ExplicitCreate` | `session/start.title` or ACP `session/new` title |

## Write priority (high → low)

1. `session/metadata/update` (`UserRename`)
2. `session/start.title` / ACP `session/new` title (`ExplicitCreate`)
3. Auto LLM polish (`ModelGenerated`) — may overwrite **only** `Heuristic`
4. Heuristic (`Heuristic`) — may apply only from `Unset`

Auto polish never overwrites rename / explicit create / already model-generated. Polish failure never resets to `Unset`.

## First-turn sequencing

For an untitled session:

1. `turn/start` (or goal first input) records `first_user_input`.
2. If still `Unset`, apply `Final(Heuristic)` immediately, persist, broadcast `SessionTitleUpdated`.
3. Mark a **TitlePolishPending** job for the session (in-memory; re-armed on resume when still Heuristic).
4. Start turn work without awaiting the title LLM.
5. When the session has **no active turn**, the polish runner may call the title LLM.

Consequently `turn/start` RPC latency does **not** include title LLM time. Users always see a readable title as soon as the first message is accepted.

## Title polish readiness

Title polish runs only when that session has no active turn, so polish does not race the turn started at the same `turn/start`.

## Server API (`runtime/session_title.rs`)

| Method | When | Behavior |
|--------|------|----------|
| `prepare_title_from_user_input` | `turn/start`; goal first-input paths | Records `first_user_input`; if `Unset`, applies `Final(Heuristic)`, persists, broadcasts; marks polish pending |
| `notify_title_polish` | Post-turn idle wake; goal idle wake | Wakes the polish runner (no LLM inline) |
| `cancel_auto_title_generation` | User rename | Clears pending polish / in-flight guard |

Removed from the auto path: empty-title `Generating`, sync `await_title_before_first_turn` blocking the first turn, and polish failure → `Unset`.

LLM calls must not run inside the session actor mailbox or across `state_change_gate` (see SERVER-002).

## Events

`SessionTitleUpdated` (Native: `session/metadataUpdated`) carries the session summary including `title` and `titleState`.

- Heuristic Final is expected **promptly after** first-input accept (before or alongside turn streaming).
- Model polish Final arrives later when the session is idle.

Clients update sidebar title from this event — they do not derive heuristic strings locally.

## Persistence compatibility

- SQLite: Finals still store `"final"` (source not retained in the index). Rollout JSONL remains source of truth for `Heuristic` vs `ModelGenerated`.
- Rollout JSONL: serde aliases keep legacy `Provisional`/`Generating` readable.
- On resume: if `Final(Heuristic)` and first user input is recoverable, re-arm polish pending and notify when idle.

## Client contract

- Placeholder `"New Chat"` only when `title` is absent (`Unset`).
- After first message, `title` is always the heuristic (then possibly polished) string.
- No empty-title `Generating` spinner. Polish is silent.
- Native `Session.title_state` is projected on the wire as `titleState`.
