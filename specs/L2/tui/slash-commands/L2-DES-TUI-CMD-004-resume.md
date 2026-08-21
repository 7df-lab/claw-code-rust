---
artifact_id: L2-DES-TUI-CMD-004
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-20
---

# L2-DES-TUI-CMD-004 — Slash Command: /resume

## Purpose

Define the inline TUI workflow for finding, inspecting, maintaining, and reopening a saved session without hiding the current transcript.

## Command Contract

- Command: `/resume`
- Description: `resume a saved chat`
- Parameters: none.
- Mutability: session metadata may be renamed or deleted; `Enter` changes the active client session.
- Transcript effect: the command itself creates no model-visible user turn.
- Rendering mode: a `BottomPaneView` replaces the composer. It never enters or leaves the terminal alternate screen.
- Workspace identity: the normalized session `cwd` must exactly equal the normalized current `cwd`; Git repositories, subdirectories, and worktrees are not coalesced.
- Host input: `Ctrl+T` does not take focus while the picker is loading or open.

## Layout And Scope

The current transcript remains above the picker. Layout allocation keeps at least three transcript rows visible and gives the remaining lower area to the picker. Its list scrolls internally.

```text
Resume session (1 of 20)
╭ Search ─────────────────────────────────────────────────────────╮
│                                                                │
╰────────────────────────────────────────────────────────────────╯
ctf-bench

❯ hello
  56 minutes ago · 10.3KB

  /model
  13 hours ago · 2.1KB

Ctrl+A all projects · Space preview · Ctrl+R rename · Ctrl+D delete · Esc cancel
```

The default scope contains only sessions whose `cwd` equals the current workspace and shows the workspace directory name above the rows. `Ctrl+A` switches to all projects; in that mode each metadata line appends the session's absolute `cwd`, and the footer offers returning to the current workspace.

Each session uses two base lines:

1. title, with the selected row marked by `❯`;
2. relative last-activity time, decimal JSONL size (`B`, `KB`, `MB`, or `GB`, one decimal for scaled units), and absolute `cwd` in all-project mode. Git branch is searchable metadata but is not rendered in the row.

The title is `Resume session (position of filtered-count)`. The current active session is selected initially when it is visible; otherwise the newest visible session is selected. Empty scopes render `No saved sessions found.`. Long titles, CJK content, and paths are truncated by terminal display width.

## Search And Navigation

Printable input performs an immediate case-insensitive local search across title, first prompt/preview, `cwd`, and branch. `Backspace` removes the last search character. Filtering and scope changes retain the selected session by stable ID when it remains visible, otherwise they select the first (newest) result.

| Key | Behavior |
|---|---|
| Printable character / `Backspace` | Edit the local search query. |
| `Up` / `Down` | Move one result without wrapping. |
| `PageUp` / `PageDown` | Move by a bounded page without wrapping. |
| `Home` / `End` | Select the first or last result. |
| `Ctrl+A` | Toggle current-workspace and all-project scopes. |
| `Enter` | Close the picker and resume the selected session through the normal session-switch flow. Other-workspace sessions are resumed directly in this TUI. |
| `Esc` | Cancel rename/delete first; otherwise clear a non-empty search; otherwise close the picker and restore the composer. |

Selection changes collapse an expanded preview.

## Preview

`Space` expands or collapses preview content directly below the selected row. The first expansion issues a typed, ID-addressed `session/items/list` request and shows an inline loading state. The worker follows all pages and retains the last four non-empty user or assistant messages, ignoring tools, plans, reasoning, and other item kinds. Each message is limited to two displayed lines.

Empty content and failures are rendered inline. Responses carry the requested session ID; a view accepts a response only when that ID has an outstanding preview state, so a late response cannot replace another row's preview.

## Rename And Delete

`Ctrl+R` opens an inline title editor prefilled with the selected session title. `Enter` trims and validates the title, then sends canonical `session/metadata/update` for that explicit session ID. Empty titles remain in the editor with an error. Success updates the row in place; failure preserves the edited text and shows the targeted error.

Title updates support cold durable sessions: the server resolves the session from its index/rollout and persists the title without requiring that session to be the currently selected actor and without switching the client session.

`Ctrl+D` opens the existing Cancel/Delete confirmation state for the selected session. `Esc` cancels it. Deleting a non-active session removes its row and leaves the picker open. Deleting the active session uses the existing new-session preparation flow.

## Worker And Protocol Behavior

- `/resume` opens the loading bottom pane immediately and emits a typed list-sessions command. There is no `"session list"` string dispatch and no resume-specific host pending flag.
- List, preview, rename-by-ID, and delete operations return operation-specific success or failure events. Picker failures are not projected as `TurnFailed`.
- The TUI list DTO carries the raw UTC activity time, `cwd`, branch, preview, optional transcript byte count, and active marker. Relative time and responsive text formatting belong to the view.
- Canonical Native `Session.transcriptSizeBytes` is optional for compatibility. `session/list` fills it from filesystem metadata for the server-owned rollout path; unreadable or missing rollout files yield an omitted value. Other session responses may omit it.
- `Enter` clears the visible session UI, marks resume pending, and invokes the existing typed session-switch operation. `SessionSwitched` restores cwd, title, model settings, token totals, transcript items, and pending inputs exactly as before.
- Resuming never deletes the previously active durable session.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Resuming, renaming, and deleting saved sessions are lifecycle workflows. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Defines canonical list, items, metadata update, and resume behavior. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | The picker replaces the composer through the shared bottom-pane surface. |
| related-to | L2-DES-TUI-005 | 1 | specs/L2/tui/L2-DES-TUI-005-terminal-lifecycle-safety.md | The inline picker does not alter terminal screen lifecycle. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines slash-command parsing and routing. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/resume` command design. |
| 1 | 2026-05-25 | Assistant | Refinement | Documented the former alternate-screen browser. |
| 2 | 2026-08-20 | Assistant | Redesign | Replaced the full-screen browser with an inline, searchable, workspace-scoped picker with preview, rename, delete, cross-project resume, and transcript sizes. |
