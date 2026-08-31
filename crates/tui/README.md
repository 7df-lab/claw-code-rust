# devo-tui

The interactive terminal UI crate for Devo. It provides a Ratatui-based TUI with a chat-like conversation surface, a rich composer, streaming markdown rendering, tool execution display, syntax-highlighted diffs, and full-screen pager overlays.

## Architecture Overview

The crate follows a layered architecture where each layer has clear boundaries:

```
                        Terminal Backend (crossterm)
                                |
                        Tui (tui.rs) -- event stream, frame scheduling, raw mode
                                |
                        Host (host.rs) -- tokio event loop, channel bridging
                        /               |               \
               TuiEvent             AppEvent          WorkerEvent
                  |                   |                   |
            Tui input events    ChatWidget + BottomPane   Worker (worker.rs)
              (key/mouse)       (UI rendering)           (server child process)
```

The outer shell is `host.rs`, which owns the main `tokio` event loop. It bridges three concurrent concerns:

1. **Terminal input** arrives as `TuiEvent` from `tui.rs` and is dispatched to the chat widget or overlay.
2. **User actions** produce `AppEvent` or `AppCommand` values that flow either to the chat widget (for local UI updates) or to the worker (for server RPC).
3. **Server responses** arrive as `WorkerEvent` from the worker and are fed into the chat widget to update the transcript.

## Terminal UI Fundamentals

This crate is built on a few core terminal UI concepts. If you are comfortable programming but new to terminal UIs, the main mental model is:

> A TUI is an event-driven renderer over a terminal buffer.

Unlike a GUI framework, there is no persistent window tree owned by the OS. The application repeatedly:

1. Reads terminal events, such as key presses, mouse input, resize events, focus changes, and paste payloads.
2. Updates application state.
3. Requests a redraw.
4. Renders the entire visible UI for the current terminal size into a frame buffer.
5. Flushes the frame to the terminal backend.

The terminal is therefore closer to a canvas than to a DOM. Widgets are mostly pure rendering logic over state.

### Ratatui

`ratatui` is the terminal UI framework used by this crate. It provides the rendering abstraction:

- `Terminal`: owns the backend and drives frame rendering.
- `Frame`: the per-draw rendering context.
- `Rect`: rectangular areas in terminal cell coordinates.
- `Buffer`: the off-screen cell buffer that eventually gets flushed to the terminal.
- `Widget`: renderable UI components.
- `Layout`: utilities for splitting terminal space into regions.
- `Line`, `Span`, `Text`: styled text primitives.
- `Style`, `Color`, `Modifier`: styling primitives.

A typical Ratatui render pass looks like this conceptually:

```rust
terminal.draw(|frame| {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);

    transcript.render(chunks[0], frame.buffer_mut());
    composer.render(chunks[1], frame.buffer_mut());
})?;
```

The important point is that Ratatui does not usually own your state. Your application owns state, and Ratatui renders a view of that state into the current frame.

In this crate, `ChatWidget`, `BottomPane`, history cells, overlays, diff views, and execution cells all follow that model: state lives in application structs, and rendering converts that state into terminal cells.

### Crossterm

`crossterm` is the terminal backend used underneath Ratatui. It handles low-level terminal operations such as:

* entering and leaving the alternate screen,
* enabling and disabling raw mode,
* reading keyboard and mouse events,
* enabling bracketed paste,
* enabling focus reporting,
* controlling cursor visibility and position,
* writing terminal commands to stdout.

In this crate, `tui.rs` owns most of that terminal substrate. It is responsible for putting the terminal into the correct mode when the app starts and restoring it on exit, suspend, panic, or overlay transition.

The most important crossterm concepts are:

#### Raw mode

In normal terminal mode, the shell or terminal driver processes input before the application sees it. For example, Enter may be line-buffered, Ctrl+C may interrupt the process, and backspace may be handled by the terminal.

In raw mode, the application receives key events directly. This is required for interactive editors, TUIs, custom shortcuts, and real-time input handling.

Because raw mode changes terminal behavior globally for the process, it must be restored reliably. A broken exit path can leave the user’s terminal in a bad state.

#### Native text selection and Ctrl+C

The TUI deliberately leaves mouse capture disabled, so text selection, scrollback, and copy-on-select remain owned by the terminal emulator. If the terminal is configured to copy on selection, dragging across text copies it immediately. When a selection is active, the terminal must also bind Ctrl+C to copy and consume the key before it reaches devo.

When the terminal sends Ctrl+C to devo because there is no active selection, devo interrupts active work immediately. While idle, the first Ctrl+C shows a confirmation and a second press within two seconds exits. Terminals that do not provide copy-on-select or consume Ctrl+C for an active selection cannot be made to do so by the TUI because terminal-native selected text is not exposed to applications.

#### Alternate screen

The alternate screen is a separate terminal screen buffer. Full-screen terminal applications usually draw into the alternate screen so the user’s shell scrollback is restored when the app exits.

This crate uses the alternate screen both for the main TUI and for full-screen overlays such as the transcript pager.

#### Bracketed paste

Bracketed paste lets the terminal distinguish typed input from pasted input. Without it, a multi-line paste looks like a rapid sequence of key presses. With bracketed paste, the application can detect paste boundaries and handle large pasted content more safely.

This matters for the composer because pasted prompts, code blocks, logs, and multi-line content should not be interpreted as accidental command sequences.

### The historical `tui` crate

You may see the term `tui` in older Rust terminal UI material. Historically, `tui-rs` was a popular Rust TUI library. It is now effectively succeeded by `ratatui`, a maintained fork.

In this crate:

* `ratatui` is the external UI framework.
* `tui.rs` is this crate’s local terminal wrapper module.
* `Tui` is this crate’s abstraction around terminal mode, rendering, frame scheduling, and event streaming.

So when reading this codebase, distinguish between:

```text
ratatui     external rendering framework
crossterm   external terminal backend
tui.rs      local module that integrates terminal setup, events, and rendering
Tui         local runtime wrapper around the terminal
```

### Events

The TUI is event-driven. Nothing happens just because a widget exists. The host loop must receive an event, update state, and schedule a frame.

This crate separates events by responsibility:

```text
TuiEvent      terminal-originated input: keys, mouse, resize, paste, focus
AppEvent      UI/application events: redraw, submit input, open popup, exit
AppCommand    commands that should be sent to the worker/server
WorkerEvent   server-originated events: streamed text, tool calls, results, status
```

This separation is important because not every event has the same destination.

For example:

* A key press may be handled locally by the composer.
* Pressing Enter may become an `AppCommand::UserTurn`.
* A streamed assistant token arrives as a `WorkerEvent`.
* A popup selection may update UI state without touching the worker.
* A terminal resize requires re-rendering but not a server call.

The host loop is the bridge that decides where each event goes.

### Rendering versus state mutation

A common TUI design rule is:

> Event handlers mutate state; render functions display state.

Render code should generally avoid changing application state. This keeps redraws deterministic and makes it easier to handle resize events, animation ticks, snapshot tests, and frame coalescing.

For example, when assistant text streams in:

1. The worker emits a `WorkerEvent::AssistantTextDelta`.
2. `ChatWidget` mutates the active streaming cell.
3. The streaming controller decides what rendered lines are ready.
4. A frame is scheduled.
5. The next render pass draws the visible transcript from current state.

The render pass itself should not be responsible for consuming server events or advancing application logic beyond narrowly scoped animation/render bookkeeping.

### Frame scheduling

Terminals are relatively expensive to redraw compared with mutating in-memory state. A burst of events should not necessarily cause an equal burst of terminal draws.

This crate uses frame scheduling and rate limiting:

* `FrameRequester` lets code request a redraw without drawing immediately.
* Rapid requests are collapsed into fewer actual frames.
* `FrameRateLimiter` caps draw frequency.
* Streaming output is chunked so the UI remains responsive while avoiding excessive rendering work.

This is especially important during model streaming, command execution, large diffs, and transcript overlays.

### Terminal coordinates and layout

Terminal UIs render in a grid of cells, not pixels. A `Rect` has:

```text
x, y, width, height
```

All layout decisions eventually become cell rectangles.

Text width is also not always equal to byte length or character count. Unicode, emoji, CJK characters, combining marks, and ANSI escape sequences all complicate visual width. This crate has dedicated wrapping and truncation utilities to keep terminal layout stable.

When working on rendering code, prefer existing helpers such as:

* `live_wrap.rs`
* `wrapping.rs`
* `line_truncation.rs`
* `text_formatting.rs`
* `render/line_utils.rs`

rather than using raw string length.

### Input routing

The bottom pane, popups, overlays, and main transcript do not all receive input at the same time. Input is routed to the active surface.

Typical priority:

1. If an overlay is active, it receives input.
2. Else if a popup is active, the popup receives input.
3. Else the composer or chat widget receives input.
4. Some global keybindings may be intercepted by the host or top-level widget.

This prevents, for example, a key press intended for a popup from also editing the composer.

### Overlays

Full-screen overlays are temporary UI modes such as the transcript pager or static text viewer. An overlay usually:

1. Takes over the alternate screen.
2. Receives input instead of the main chat widget.
3. Renders a separate full-screen view.
4. Exits on Escape or another command.
5. Restores the previous UI surface.

The main application state still exists while the overlay is active. For the transcript overlay, the overlay can sync from the live `ChatWidget` so streamed output continues to appear in live-tail mode.

### Streaming text

LLM output arrives incrementally, often token by token. Rendering every token directly can produce unstable markdown and excessive redraws.

This crate uses a streaming pipeline that buffers markdown and commits displayable content in controlled chunks:

```text
raw token deltas
    ↓
MarkdownStreamCollector
    ↓
StreamCore
    ↓
AdaptiveChunkingPolicy
    ↓
HistoryCell lines
    ↓
ChatWidget transcript
```

The key idea is that streamed source text and rendered visible lines are related but not identical. Markdown may need to wait for newline boundaries or retained source re-rendering when terminal width changes.

### Worker bridge

The TUI is not the LLM server. The worker layer bridges between the UI and `devo-server`.

The UI emits commands such as “start a user turn,” “interrupt,” “approve this tool call,” or “switch session.” The worker translates those into server RPC calls. Server events are translated back into `WorkerEvent`s that the UI can render.

This keeps the terminal UI focused on interaction and display, while the worker owns server process communication and protocol translation.

### Practical rule of thumb

When changing this crate, first identify which layer you are modifying:

```text
Terminal behavior     tui.rs, tui/event_stream.rs, job control
Event routing         host.rs, app_event.rs, app_command.rs
Main chat state       chatwidget.rs
Input/composer UI     bottom_pane/*
Rendering primitive   history_cell.rs, render/*, markdown*
Server bridge         worker.rs, events.rs
Overlay behavior      pager_overlay.rs, host_overlay.rs
```

Most bugs become easier to reason about once the layer is clear.

## Module Map

### Entry Points

| Module | Purpose |
|--------|---------|
| `lib.rs` | Public API: `run_interactive_tui`, `AppExit`, `InteractiveTuiConfig`, `InitialTuiSession`, `SavedModelEntry` |
| `host.rs` | Main event loop. Bridges `Tui`, `ChatWidget`, and `QueryWorkerHandle`. Owns terminal mode enter/restore, overlay lifecycle, and session-switch orchestration. |
| `app.rs` | Startup/exit types that the CLI layer uses to configure and receive results from the TUI. |

### Terminal Substrate: `tui.rs` and Submodules

| Module | Purpose |
|--------|---------|
| `tui.rs` | `Tui` wrapper: enters raw mode, alternate screen, bracketed paste, focus reporting, keyboard enhancement. Owns the main render loop. |
| `tui/event_stream.rs` | Converts crossterm `Event` into `TuiEvent`. Handles stdin pause/resume for external interactive programs. |
| `tui/frame_requester.rs` | Lightweight redraw scheduling handle. Collapses rapid requests into single draws. |
| `tui/frame_rate_limiter.rs` | Caps draw frequency so animations feel responsive without wasting terminal work. |
| `tui/job_control.rs` | Unix job control: Ctrl+Z suspend and resume with terminal mode restoration. |

### Core UI Widget

[`chatwidget.rs`](/Users/tsiao/Desktop/devo/crates/tui/src/chatwidget.rs) (4200+ lines) is the central widget. It owns:

- The visible transcript (a `VecDeque` of `HistoryCell` entries).
- The active streaming cell (an `Option<HistoryCell>` that mutates in place during streaming).
- The `BottomPane` (composer + footer + popup stack).
- Session state: model selection, reasoning effort selection, permission preset, scroll position.
- `StreamController` instances for assistant output and plan streaming.
- Input dispatch: from `TuiEvent` through keybinding maps to `AppEvent`/`AppCommand` emission.

Submodules under `chatwidget/`:

| Module | Purpose |
|--------|---------|
| `status_surfaces.rs` | Status-line widgets shown in the footer. |
| `session_header.rs` | Session title and metadata display. |
| `realtime.rs` | Real-time timestamp and token-count display. |
| `plugins.rs` | Plugin connector rendering. |

### Bottom Pane (Input Surface) — `bottom_pane/`

The bottom pane is the user-facing input area. It contains:

| Module | Purpose |
|--------|---------|
| `mod.rs` | `BottomPane` orchestrator. Assembles composer, footer, and popup stack. Routes key events to the active surface (composer or popup). |
| `chat_composer.rs` | Multiline text input with slash commands, token-local `@` reference search (skills/MCP/files), paste handling, and input history. |
| `bottom_pane_view.rs` | Layout: splits the bottom area into composer + footer rows. |
| `textarea.rs` | Custom textarea with cursor positioning and selection support. |
| `footer.rs` | Status footer rendering: mode indicators, session info. |
| `unified_exec_footer.rs` | Footer variant for active command execution. |
| `chat_composer_history.rs` | Persistent input history with up/down navigation. |
| `slash_commands.rs` | Slash-command registration and dispatch. |
| `command_popup.rs` | Popup list for slash commands and other actions. |
| `list_selection_view.rs` | Generic scrollable selection list used by popups. |
| `approval_overlay.rs` | Permission-approval dialog for tool calls. |
| `pending_thread_approvals.rs` | Tracks approval requests that await user response. |
| `pending_input_preview.rs` | Preview of user input before submission. |
| `paste_burst.rs` | Multi-line paste detection and debouncing. |
| `theme_picker.rs` | Theme selection popup. |
| `reference_popup.rs` | Combined `@` fuzzy-search popup that renders server-provided skill, MCP server, and file reference rows. |
| `onboarding_view.rs` | First-run model/provider configuration. |
| `scroll_state.rs` | Scroll position tracking for the composer. |
| `selection_popup_common.rs` | Shared popup behavior. |
| `popup_consts.rs` | Popup sizing constants. |
| `prompt_args.rs` | Prompt argument parsing. |
| `status_line_setup.rs` | Status-line layout constants. |
| `title_setup.rs` | Title-line layout constants. |
| `custom_prompt_view.rs` | Custom prompt display. |

### Conversation Cells

| Module | Purpose |
|--------|---------|
| `history_cell.rs` | `HistoryCell`: the display unit for transcript entries. Supports user messages, assistant messages (with reasoning), tool calls, startup headers, and animation ticks. Handles inline rendering for the main viewport and transcript rendering for the pager overlay. |
| `tool_result_cell.rs` | Compact inline display for completed tool outputs. Limits preview to 3 rows; full output available in the transcript pager. |

### Execution Display — `exec_cell/`

Tool rows have one presentation owner per `tool_use_id`. A shell row stays in
the live viewport for the whole turn: completion changes its title in place
from `Running` to `Ran`, and the turn boundary commits exactly one durable
`Ran` row to scrollback. Missing authoritative results use the explicit
`result unavailable` degraded state; ordinary viewport flushes never invent a
successful or failed result.

| Module | Purpose |
|--------|---------|
| `model.rs` | `ExecCell` data model: command line, output state, exit status. |
| `render.rs` | Renders shell command output, truncated tool output previews, and active exec indicators. |
| `spinner.rs` | Animated spinner widget for in-progress operations. |

### Streaming Pipeline — `streaming/`

Streaming transforms streamed LLM token deltas into visible `HistoryCell` entries:

```
Token delta → MarkdownStreamCollector → StreamCore → queued HistoryCell → ChatWidget transcript
```

| Module | Purpose |
|--------|---------|
| `mod.rs` | `StreamState`: newline-gated markdown collection with a FIFO queue of committed render lines. Records arrival timestamps for backpressure decisions. |
| `controller.rs` | `StreamCore`: converts newline-complete markdown source into rendered `Line`s, manages enqueue/emit progress, handles width-change re-rendering from retained source. |
| `chunking.rs` | `AdaptiveChunkingPolicy`: computes optimal drain sizes from queue depth and line age, balancing responsiveness against rendering cost. |
| `commit_tick.rs` | Binds adaptive chunking decisions to controller drain operations. Runs on a timer to emit accumulated lines at a steady pace. |

### Markdown Rendering

| Module | Purpose |
|--------|---------|
| `markdown_stream.rs` | `MarkdownStreamCollector`: buffers raw markdown source and commits only at newline boundaries. Exposes `committed_source_len` to track progress. |
| `markdown.rs` | `append_markdown`: renders markdown source into ratatui `Line`s, resolving local file-link paths relative to the session working directory. |
| `markdown_render.rs` | Full markdown-to-`Text` renderer using `pulldown-cmark` (with `ENABLE_MATH`). Handles headings, lists, code blocks (with syntax highlighting via `syntect`), inline code, inline/display math, bold/italic emphasis, links, blockquotes, and citations. Display math renders as a LaTeX-highlighted block that is never text-wrapped; unclosed `$$` stays raw during streaming and is rebuilt when the block completes. Note: pulldown-cmark 0.13 still treats `\$` as a math delimiter rather than an escaped dollar. |

### Rendering Utilities — `render/`

| Module | Purpose |
|--------|---------|
| `mod.rs` | `Insets` and `RectExt` for area manipulation. |
| `renderable.rs` | `Renderable` trait with `render`, `desired_height`, and `cursor_pos`. Used by history cells, overlays, and exec cells for uniform rendering. |
| `line_utils.rs` | Line prefixing, concatenation, and owned-line helpers. |
| `highlight.rs` | Syntax highlighting via `syntect`/`two-face`. |

### Diff Rendering

[`diff_render.rs`](/Users/tsiao/Desktop/devo/crates/tui/src/diff_render.rs) renders unified diffs for `FileChange` entries. Features:
- Line numbers and gutter signs (`+`/`-`/` `)
- Syntax highlighting per hunk (preserving parser state across consecutive lines)
- Theme-aware backgrounds (dark terminal: muted tints; light terminal: GitHub pastels)
- Color-level fallback (truecolor → 256 → 16 color)
- Hard-wrap for long lines with style preservation across split points

### Pager Overlay

Full-screen overlays rendered in the terminal alternate screen:

| Module | Purpose |
|--------|---------|
| `pager_overlay.rs` | `TranscriptOverlay`: scrollable full-screen transcript viewer with search, Vim-style navigation, and live-tail mode. `StaticOverlay`: general-purpose full-screen text display. |
| `host_overlay.rs` | `OverlayState`: manages overlay enter/exit. Syncs transcript overlay content from the live `ChatWidget`, schedules frames for live-tail with animated cells. |

### Worker (Server Bridge)

[`worker.rs`](/Users/tsiao/Desktop/devo/crates/tui/src/worker.rs) (2900+ lines) spawns the `devo-server` as a child process over stdio and bridges TUI commands with server RPC:

1. Starts a server process via `StdioServerClient`.
2. Receives `AppCommand` and selected `AppEvent` requests from the host and translates them into server RPC calls (session start/resume/list, reference search start/update/cancel, turn start/interrupt/steer, approval respond, permission update, etc.).
3. Listens for `ServerEvent` from the server and converts them into `WorkerEvent` for the host.
4. Handles session lifecycle: ensures a session exists, starts turns, processes streamed item events.

### Supporting Modules

| Module | Purpose |
|--------|---------|
| `events.rs` | `WorkerEvent` enum and supporting types (`TranscriptItem`, `TranscriptItemKind`, `PlanStep`, `PlanStepStatus`, `SessionListEntry`, `SavedModelEntry`). Defines the data vocabulary between worker and UI. |
| `app_event.rs` | `AppEvent` enum: UI-level events (redraw, exit, submit input, open popups, status updates). |
| `app_command.rs` | `AppCommand` enum: commands sent to the worker (user turn, steer, approval, session switch, rollback, fork). |
| `app_event_sender.rs` | Channel sender wrapper for `AppEvent` with deduplication. |
| `theme.rs` | `Theme` color palette and `ThemeSet` with builtin themes (devo, dark, light, aurora). |
| `terminal_palette.rs` | Terminal color capability detection. |
| `style.rs` | Reusable style definitions. |
| `color.rs` | Color conversion and manipulation. |
| `text_formatting.rs` | Text truncation and formatting utilities. |
| `live_wrap.rs` | Incremental text wrapping into visual rows respecting Unicode width. |
| `wrapping.rs` | Adaptive wrapping with runtime width changes. |
| `line_truncation.rs` | Line truncation with Unicode-aware width. |
| `custom_terminal.rs` | Modified `ratatui::Terminal` that handles OSC escape sequences during diffing, supports synchronized updates, and provides efficient clear. |
| `key_hint.rs` | Keybinding hint display. |
| `shimmer.rs` | Animated shimmer/placeholder effect for loading states. |
| `status_indicator_widget.rs` | Animated status indicator for processing states. |
| `startup_header.rs` | Animated startup header with logo and version info. |
| `slash_command.rs` | Slash command definitions (`/model`, `/theme`, `/thinking`, `/status`, etc.). |
| `exec_command.rs` | Shell command execution helpers. |
| `get_git_diff.rs` | Extracts git diffs for display. |
| `clipboard_copy.rs` / `clipboard_paste.rs` | System clipboard integration via `arboard`. |
| `onboarding.rs` | Persists onboarding configuration (model, provider, API key) to `config.toml`. |
| `ui_consts.rs` | Shared UI constants (prefix columns, layout widths). |
| `version.rs` | Version tracking and display. |
| `test_backend.rs` | Test backend for snapshot testing. |
| `insert_history.rs` | History insertion utilities. |
| `ansi_escape.rs` | ANSI escape sequence handling. |

## Data Flow: A Turn Lifecycle

A typical user turn flows through these stages:

1. **User types input** in the `ChatComposer` and presses Enter.
2. `BottomPane` emits an `InputResult::Submit`, which `ChatWidget` converts into an `AppCommand::UserTurn`.
3. The `AppCommand` is sent via `app_event_tx` to the host's `AppEvent::Command` handler.
4. The host forwards the command through `QueryWorkerHandle::send_command`.
5. **Worker** receives the command, translates it into `TurnStartParams`, and calls the server RPC.
6. **Server** begins streaming response items. The worker receives `ItemEventPayload` and converts each into `WorkerEvent`s (e.g., `AssistantTextDelta`, `ReasoningTextDelta`, `ToolCall`, `ToolResult`).
7. **Worker events** flow back through `worker_event_rx` to the host, which feeds them to `ChatWidget::handle_worker_event`.
8. **ChatWidget** updates the active streaming `HistoryCell` in place (appending text, setting tool state). The streaming pipeline (`StreamCore` → `AdaptiveChunkingPolicy` → `commit_tick`) manages when partial lines become visible.
9. **Turn completion** (`TurnFinished`) finalizes the active cell into committed history. `CommitTickScope` drains remaining queued lines.
10. On every state change, `FrameRequester::schedule_frame()` triggers a terminal redraw.

## Overlay Lifecycle

Overlays (Ctrl+T transcript, static pages) enter and exit the alternate screen:

1. User presses Ctrl+T → host calls `OverlayState::open_transcript(tui, chat_widget)`.
2. `Tui::enter_alt_screen()` switches to alternate screen.
3. While overlay is active, `TuiEvent`s are routed through `OverlayState::handle_tui_event` instead of `ChatWidget`.
4. The transcript overlay syncs with `ChatWidget`'s live transcript cache (including the active cell tail) on each draw.
5. User presses Escape → overlay marks itself done → `OverlayState` calls `Tui::leave_alt_screen()` and schedules a normal frame.

## Key Dependencies

- **ratatui**: Terminal UI framework (buffer, layout, widgets, styling)
- **crossterm**: Terminal backend (raw mode, events, alternate screen)
- **tokio**: Async runtime for the event loop and worker communication
- **pulldown-cmark**: Markdown parsing
- **syntect / two-face**: Syntax highlighting
- **diffy**: Diff computation
- **arboard**: Clipboard access
- **devo-server**: Spawned child process that handles LLM communication
