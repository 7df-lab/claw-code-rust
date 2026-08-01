---
artifact_id: L2-DES-TUI-CMD-002
revision: 2
status: Draft
active_baseline: no
supersedes: 1
superseded_by:
owner: Assistant
last_updated: 2026-08-01
---

# L2-DES-TUI-CMD-002 — Slash Command: /model

## Purpose

Define the TUI behavior for `/model`, the post-onboarding command for changing the active session model, provider binding, and reasoning effort where applicable.

## Command Contract

- Command: `/model`
- Description: `choose the active model`
- Parameters: optional model or binding name for direct selection without opening the picker.
- Mutability: session metadata for existing bindings; adding new providers or bindings is out of band via `devo onboard` in the current implementation.
- Active-turn availability: blocked while a turn is generating, running tools, or waiting on active execution.

## Design Requirement

`/model` without arguments opens a single below-composer picker that **replaces the composer input area** and combines:

1. A vertically scrollable list of configured model-provider bindings.
2. A horizontal reasoning-effort strip for the focused binding when that model exposes effort options.

Up/Down (and j/k) move the model focus. Left/Right cycle reasoning effort for the focused model. Enter applies the focused model and the selected effort in one step. Esc dismisses without changing the session.

The model list uses the shared popup row budget (`MAX_POPUP_ROWS`, currently 8): when there are more bindings than the budget, only a scroll window is rendered and the focused row is kept visible.

`Add model...` and the multi-step onboarding-style add-model wizard described in revision 1 are **not** part of the current `/model` surface; new bindings are created via `devo onboard`.

## UI Flow

```text
┃ /model

  › DeepSeek V4 Pro ‹  OpenRouter
    GPT 5.5            OpenAI
    Claude Sonnet 5    Anthropic
  …

  ‹ Off  Low  [Medium]  High  Max ›

  ↑↓ model  ←→ effort  Enter confirm  Esc cancel
```

- The picker replaces the composer input area while open; the normal bottom status line is not shown.
- Each model row is a two-column layout: model name (plus optional current-session `‹` right after the name) padded to a shared column width, then a muted provider hint starting in a common second column.
- `›` on the left marks keyboard focus. Focused labels use accent + bold + underline for stronger contrast.
- When the binding list exceeds the visible row budget, dim `…` markers indicate more items above and/or below the window.
- When the focused model has no reasoning options, the effort strip is omitted and Left/Right are no-ops.
- When effort options do not fit the available width, the strip shows a sliding window centered on the selected option with `‹` / `›` overflow markers.
- Changing the focused model recomputes available effort options. If the previously selected effort value is still supported, it is preserved; otherwise the picker falls back to that model's session-resolved default (or the first option).

Direct selection:

```text
/model gpt-5-codex
```

applies a matching saved binding or catalog model without opening the picker, resolving reasoning effort from the current session selection when the target model supports it.

## Step Behavior

- Opening `/model` with an empty argument shows the combined picker in place of the composer input area and hides the normal bottom status line while the picker is visible.
- The binding list is populated from saved model bindings in the session (effective configuration).
- Enter on the focused row applies the model (and effort when present) immediately via turn-context override.
- Esc or Ctrl+C dismisses the picker with no session change.
- Selecting an existing configured binding updates the current session selection only; it must not rewrite provider records, binding records, or default-selection fields after the first user message.
- If invoked during active work, the TUI shows a concise blocked message such as `Cannot change model while generating`.
- The selected model and reasoning effort affect the next turn, not an already-running invocation.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines `/model`, the required post-onboarding model-selection command. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model selection uses configured model-provider bindings. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines when model selection changes are persisted as defaults versus session state. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Defines supported models, user providers, and model-provider bindings. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Defines configuration write scope, persistence target behavior, and distinction between session selection and durable records. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery, popup, and invocation behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/model` command design. |
| 1 | 2026-05-25 | Human | Refinement | Aligned `/model` with the onboarding model setup sequence and specified one-step-at-a-time rendering below the composer. |
| 1 | 2026-05-25 | Human | Refinement | Changed the first `/model` screen to configured binding selection with `Add model...` as the entry to the add-model flow. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that the `/model` command surface replaces the bottom status line while visible. |
| 1 | 2026-05-25 | Human | Refinement | Split configured model, provider, and reasoning effort into distinct `/model` steps. |
| 1 | 2026-05-25 | Human | Refinement | Grouped model and provider back into the configured binding selection while keeping reasoning effort separate. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that existing binding selection is session state after the first user message, while newly created provider or binding records require configuration persistence. |
| 1 | 2026-05-26 | Human | Refinement | Added binding display names to configured model rows and to the add-model flow. |
| 1 | 2026-05-26 | Human | Refinement | Updated `/model` choice-list marker semantics so `>` marks focus and `●` marks the active model or reasoning value. |
| 2 | 2026-08-01 | Assistant | Refinement | Combined model list and horizontal reasoning-effort strip into one picker; capped list height with scroll; deferred Add model... to onboard. |
