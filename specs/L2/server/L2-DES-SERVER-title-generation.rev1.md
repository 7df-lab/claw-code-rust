# L2-DES-SERVER: Session title generation

| Field | Value |
|-------|-------|
| Artifact ID | L2-DES-SERVER-title-generation |
| Revision | 1 |
| Status | Approved |
| Active Baseline | yes (until Rev 2 is approved) |
| Superseded-By | Rev 2 (Draft) |

Approved design note for automatic session titles (historical baseline).

## State machine

```
Unset → Generating → Final(source)
         ↓ (LLM retries exhausted)
       Unset
```

- **Unset**: No display title. Clients show the fixed placeholder `"New Chat"`.
- **Generating**: First user input recorded; LLM title in progress. `title` remains `null` until Final; clients may show a lightweight spinner.
- **Final**: A durable title string is set. Auto-generation never overwrites after `Final`.

`Final` carries `SessionTitleFinalSource`:

| Source | Meaning |
|--------|---------|
| `ModelGenerated` | LLM title (awaited before first-turn work) |
| `UserRename` | `session/metadata/update` title patch |
| `ExplicitCreate` | `session/start.title` or ACP `session/new` title |

## Write priority (high → low)

1. `session/metadata/update` (`UserRename`)
2. `session/start.title` / ACP `session/new` title (`ExplicitCreate`)
3. Auto LLM (`ModelGenerated`)

`Final` blocks all lower-priority writers.

## First-turn sequencing

For an untitled session, title generation is **serial before turn work**:

1. `turn/start` (or idle goal set) marks `Generating` and broadcasts.
2. Handler **awaits** the title LLM (`Final(ModelGenerated)` or retries exhausted → `Unset`).
3. Only then does `execute_turn` / goal continuation start.

Consequently `turn/start` RPC latency on the first untitled prompt includes title generation. Title failure does not block the turn.

Post-turn `schedule_final_title_generation` remains a no-op fallback when the title is already `Final`, or a retry path if the pre-turn await was skipped / failed back to `Unset`.

## Server API (`runtime/session_title.rs`)

| Method | When | Behavior |
|--------|------|----------|
| `prepare_title_from_user_input` | Internal / deferred goal path | Records `first_user_input`; if `Unset`, sets `Generating`, persists, broadcasts with `title=null` |
| `await_title_before_first_turn` | First untitled `turn/start`; idle goal follow-up | `prepare` then **await** LLM; success → `Final(ModelGenerated)`; retries exhausted → `Unset` |
| `schedule_final_title_generation` | Post-turn fallback | Spawns detached LLM task when still `Unset`/`Generating` |
| `cancel_auto_title_generation` | User rename | Clears in-flight generation guard |

LLM calls must not run inside the session actor mailbox or across `state_change_gate` (see SERVER-002).

## Events

`SessionTitleUpdated` (Native: `session/metadataUpdated`) is orthogonal to turn streaming after work begins. On the first untitled turn, the Final title event is expected **before** agent stream output.

Clients update sidebar title from this event only — no local string derivation.

## Persistence compatibility

- SQLite: writes `"generating"`; reads legacy `"provisional"` as `Unset`.
- Rollout JSONL: serde `#[serde(alias = "Provisional")]` on `Generating` for old lines.

## Client contract

- Placeholder: `"New Chat"` everywhere (sidebar, SDK remember path, tray).
- Optional: spinner when `titleState === "Generating"` and `title` is empty.
- Native `Session.title_state` is projected on the wire as `titleState`.
