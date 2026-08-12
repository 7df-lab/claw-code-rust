---
artifact_id: L2-DES-TUI-009
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-01
---

# L2-DES-TUI-009 — Settings Hub

## Purpose

Define the TUI Settings Hub opened by `/settings`: a tabbed overview that deep-links into existing configuration pickers and hosts the compaction-threshold editor.

## Source Requirements

- `L1-REQ-TUI-006` requires discoverable commands for product workflows.
- `L1-REQ-TUI-004` requires visible current execution and session state.
- `L2-DES-TUI-003` defines composer and bottom-pane popup ownership.
- `L2-DES-LLM-003` defines context pressure and compaction threshold observability.

## Hub Pattern

`/settings` is a hub, not a full reimplementation of every picker.

- Overview rows show the current value.
- Enter on Model, Theme, Permissions, or Show reasoning opens the existing picker on the bottom-pane view stack.
- Esc from a nested picker returns to the hub when the hub opened that picker.
- Existing slash commands (`/model`, `/theme`, `/permissions`, `/show-reasoning`) remain first-class aliases.

## Tabs

| Tab | Contents |
|-----|----------|
| Session | Model, Permissions, Mode (BUILD/PLAN), Compaction threshold |
| Appearance | Theme, Show reasoning |
| Agent | Placeholder: Coming soon (personality / agent style later) |

No Context tab and no filter bar in the first milestone.

## Chrome And Keys

```text
Settings

● Session    ○ Appearance    ○ Agent
────────────────────────────────────────

  Model                          deepseek-v4-flash
  Permissions                    default
  Mode                           BUILD

  Compaction threshold           190K

↑↓ navigate   Enter open   Tab switch tab   Esc close
```

Rules:

- Title, tab row, then one separator under the tabs. No separator above the footer.
- While the hub owns focus: `Tab` / `Shift+Tab` switch tabs; `↑↓` move rows; `Enter` opens or applies; `Esc` closes the hub.
- `replaces_composer` is true for the hub and nested settings editors.
- Configuration changes remain unavailable during an active task, matching other config slash commands.

## Mode Row

The Mode row shows the current TUI `InputMode` label (`BUILD` or `PLAN`). Enter cycles via the existing local composer mode cycle. Shell mode is not offered from Settings. No session-mode RPC is introduced for this row.

## Compaction Threshold

Enter on Compaction threshold opens an absolute-token list of round product
presets (`100K`…`1M`, including `250K`). The list is not built from the raw
model `context_window`; the header still shows the model window for context,
and apply clamps to that window on the server.

```text
Settings › Compaction

● Session    ○ Appearance    ○ Agent
────────────────────────────────────────

  Model     deepseek-v4-flash
  Window    1M

> 250K     (recommended)
  300K
  …
  1M       (current)

↑↓ navigate   Enter apply   Esc back
```

- `(recommended)` marks the product recommendation (`250K`).
- `(current)` marks the effective session limit (matched by value or display label).
- Near-million values render as `1M` (for example `996147` → `1M`).
- Scroll overflow uses `↑ more` / `↓ more`, matching the model picker.
- Both annotations may appear on the same row.
- Apply calls `session/metadata/update` with `settings.effectiveContextWindow`
  and must hot-update the session without process restart.
- Values above the model `context_window` are clamped on apply (not rejected).
- In-flight turns keep their frozen budget; the next compaction decision uses the new limit.
- First milestone is session-scoped persistence via `SessionSettings.effective_context_window` (resume restores it). No `config.toml` write.

Resolution order:

```text
resolved = min(
  session.effective_context_window ?? model.effective_context_window(),
  model.context_window
)
```

`resolved` drives both the context occupancy window and the automatic-compaction boundary.
## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Hub is a discoverable configuration command surface. |
| related-to | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | Hub exposes current session configuration values. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Hub is a bottom-pane view that replaces the composer while open. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Compaction threshold editing ties to context pressure observability. |
| related-to | L2-DES-TUI-CMD-013 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-013-settings.md | Slash command contract for `/settings`. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-01 | Assistant | Initial | Settings Hub IA, tabs, compaction list, hot-update contract. |
| 2 | 2026-08-01 | Assistant | Update | Persist session override as `effective_context_window`; clamp to model `context_window`. |
