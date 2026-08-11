---
artifact_id: L2-DES-TUI-CMD-013
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-01
---

# L2-DES-TUI-CMD-013 — Slash Command: /settings

## Purpose

Define the TUI behavior for `/settings`, which opens the Settings Hub panel.

## Command Contract

- Command: `/settings`
- Description: `open session and appearance settings`
- Parameters: none in the first milestone.
- Mutability: may mutate session configuration through nested editors (model, permissions, theme, reasoning view, compaction threshold, local input mode).
- Active-turn availability: unavailable during an active task (same class as `/model`, `/theme`, `/permissions`).

## UI Flow

`/settings` opens the Settings Hub bottom-pane view defined by `L2-DES-TUI-009`.

Rules:

- The hub replaces the composer while open.
- Nested pickers stack above the hub; Esc returns to the hub, then closes the hub.
- Compaction threshold apply uses `session/compaction/update` (`effectiveContextWindow`) and must take effect without restarting the process.

## State And Error Behavior

- The hub displays server-confirmed or TUI-local current values; it must not invent compaction limits.
- Values above the model context window are clamped on apply.
- Failed updates (other errors) keep the previous threshold visible and show the rejection reason.
- `/settings` must not create a model-visible transcript turn.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L2-DES-TUI-009 | 1 | specs/L2/tui/L2-DES-TUI-009-settings-hub.md | Hub layout and nested editor behavior. |
| related-to | L2-DES-TUI-CMD-002 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-002-model.md | Deep-links to the existing model picker. |
| related-to | L2-DES-TUI-CMD-001 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-001-theme.md | Deep-links to the existing theme picker. |
| related-to | L2-DES-TUI-CMD-007 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-007-permissions.md | Deep-links to the existing permissions picker. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-01 | Assistant | Initial | Initial `/settings` command design. |
| 2 | 2026-08-01 | Assistant | Update | Compaction apply persists `effectiveContextWindow` with clamp semantics. |
