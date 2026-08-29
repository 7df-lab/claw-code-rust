# AGENTS.md

This file supplements the root [AGENTS.md](/AGENTS.md). When guidance conflicts, the rules here take precedence for code under `crates/tui/`.

## Architecture

The TUI crate follows a three-channel event loop driven by `host.rs`:

```
crossterm input  -->  TuiEvent  -->  host.rs  -->  ChatWidget.dispatch_tui_event()
user actions     -->  AppEvent  -->  host.rs  -->  ChatWidget.dispatch_app_event() or worker
server responses -->  WorkerEvent -> host.rs  -->  ChatWidget.apply_worker_event()
```

`AppCommand` values flow from the chat widget or bottom pane, through the event sender, back into `host.rs`, where they are translated into worker RPC calls.

## Modules

### Size Guidelines

- Keep modules under 500 lines excluding tests. Move new functionality into a submodule or sibling module once a file passes ~600 lines.
- `chatwidget.rs` (~4200 lines) and `history_cell.rs` (~1600 lines) are known exceptions due to deeply coupled variant rendering. Do not add significant new code to these files; prefer extracting to new modules under `chatwidget/` or as standalone files.
- `worker.rs` (~2900 lines) is the server child-process bridge and accumulates protocol translation logic. New protocol features should add well-bounded private methods rather than standalone modules unless the new code exceeds ~400 lines.

### Transcript projection (L2-DES-TUI-007)

Tool rows in the live viewport and scrollback are driven by a single pipeline:

```
Native item/* (or adapted WorkerEvent)
  → ItemLifecycleEvent (transcript/lifecycle.rs)
  → TranscriptProjector (transcript/projector.rs)
  → ToolCellModel / LiveTextCellModel + transcript/presentation.rs (semantic verbs)
  → chatwidget/history_commit.rs (canonical finished-tool commit)
  → transcript/render.rs → HistoryCell
  → transcript_sync.rs → active viewport (tools + streaming text)
```

- **Historical tool commits** must go through `chatwidget/history_commit.rs`. Do not append tool history via ad-hoc `ToolResultCell` or `complete_exec_tool_from_committed` on commit.
  - **Live turn** (`transcript_sync.rs`): `commit_committed_tool_to_live_turn` keeps exploration/exec groups in the live overlay until turn finish or compaction; `complete_exec_tool_from_committed` is only for in-flight exec output sync.
  - **Resume rebuild** (`restored_session.rs`): `commit_committed_tool_to_history` appends directly to scrollback history (resume semantics are canonical for rendering).
  - Decision order: `file_changes` → exploration (`ExecCell`) → exec-like shell (`ExecCell`) → paired `tool_io` (`ToolIoCell`) → semantic fallback (`ToolResultCell`).
- **Semantic verbs** live in `transcript/presentation.rs`: `Reading`/`Read`, `Writing`/`Wrote`, `Editing`/`Edited`, `Grepping`/`Grepped`, `Finding`/`Found`, `Running`/`Ran` (shell), `Loading`/`Loaded` (skill). Do not prepend a second generic `Running`/`Ran` prefix in renderers.
- **Exec-like tools** (`read`, `grep`, `glob`, shell) still route through `ExecCell` for grouping; `exec_cell/render.rs` uses the same verb pairs on action lines.
- **Assistant and reasoning text** use the same `ItemLifecycleEvent` pipeline (`TextStarted` / `TextDelta` / `TextCompleted`). `TranscriptProjector` is the single source of truth for live text bodies; `transcript/stream_text.rs` merges incremental and cumulative wire deltas. `transcript_sync.rs` mirrors projector state into `active_text_items` for viewport ordering and rendering only. Worker code emits `WorkerEvent::Transcript`; legacy `TextDelta`/`ReasoningDelta` are routed into the projector at the widget boundary. Tests use `worker_event_test_helpers::{text_item_started, text_item_delta, text_item_completed}`.
- **Tool input streaming**: `item/toolCall/inputDelta` maps to `ItemLifecycleEvent::ToolInputChunk` via `worker/tool_lifecycle.rs`.
- **Shell-family tools** (`shell_command`, `bash`, `exec_command`, `write_stdin`): inline viewport shows `Running`/`Ran` plus the model-provided `description`, then the command in dim gray; stdout/stderr stays in `transcript_lines` (Ctrl+T) only. User-composer `$` shell rows keep inline output.

### Adding a New Module

1. Add `mod my_module;` to `lib.rs` with the appropriate visibility.
2. If the module re-exports items used elsewhere, prefer `pub(crate) use` re-exports from the module rather than exposing internal structure.
3. Every new module MUST have a `//!` module-level doc comment explaining its role and how it fits into the larger architecture.
4. If the module is a submodule of an existing directory (e.g., `bottom_pane/`, `tui/`), declare it in the parent `mod.rs` instead of `lib.rs`.

### Module Visibility

- Default to `pub(crate)` for all types, functions, and modules.
- `pub` is reserved for the public API surface: `lib.rs` exports and the types in `app.rs` (`AppExit`, `InteractiveTuiConfig`, `InitialTuiSession`).
- Use `pub(super)` for items shared within a parent module (e.g., submodules of `chatwidget/` that share internal types).

## Event & Command Patterns

### Adding a New AppEvent

1. Add the variant to the `AppEvent` enum in `app_event.rs`.
2. Handle the variant in `ChatWidget::dispatch_app_event()`.
3. If `host.rs` needs to react (e.g., overlay lifecycle, session switching), add a match arm in the host event loop.
4. Events that carry structured data should use named fields in the enum variant, not free-form strings.

### Adding a New AppCommand

1. Add the variant to the `AppCommand` enum in `app_command.rs`.
2. Handle translation to worker RPC in `host.rs`.
3. If the command originates from a keybinding or UI action, dispatch it via `AppEventSender::send(AppEvent::Command(AppCommand::MyVariant { ... }))`.

### Channel Patterns

- `AppEventSender` wraps a tokio `mpsc` sender and provides a `send()` method. Use `try_send` for bounded channels and `send` for unbounded ones. Do not call the underlying channel directly.
- `FrameRequester` wraps a tokio `watch` channel for redraw scheduling. Call `request_frame()` when a widget changes state that should be visible to the user, and the frame loop collapses rapid requests automatically.
- Channel capacities: use 1024 for `AppEvent` channels unless there is a documented reason to deviate.

## Widget Rendering

### Renderable Trait

Any widget that appears in the conversation transcript or the transcript overlay should implement `crate::render::renderable::Renderable`:

```rust
trait Renderable {
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn desired_height(&self, width: u16) -> u16;
    fn cursor_pos(&self, _area: Rect) -> Option<(u16, u16)> { None }
}
```

- `render`: draws into a ratatui `Buffer`. Use the `RectExt` extension methods from `crate::render` for area subdivision.
- `desired_height`: returns the number of viewport rows this widget occupies at the given terminal width. Implementations MUST account for line wrapping. The default `HistoryCell::desired_height()` delegates to `Paragraph::line_count(Wrap { trim: false })`; cell types that don't wrap (e.g., `StartupHeaderCell`) may override with a fixed count.
- `cursor_pos`: return the (x, y) cursor position relative to `area` when the widget owns keyboard input. Return `None` otherwise.

### HistoryCell Trait

All conversation transcript entries implement `HistoryCell`:

- `display_lines(width)` — lines rendered in the main chat viewport. These may be truncated or compact (e.g., `ToolResultCell` shows only 3 preview rows).
- `transcript_lines(width)` — lines rendered in the Ctrl+T pager overlay. These should be complete. Override when the pager view differs from the inline view (e.g., `ExecCell` shows full command output with `$` prefix and exit status).
- When adding a new cell variant, implement both methods. If they are identical, only implement `display_lines` (the default `transcript_lines` delegates to it).
- Cells that animate over time (spinners, shimmer) should implement `transcript_animation_tick()` and update an internal timestamp. The transcript overlay cache key is invalidated when the active cell revision changes.

### Active Cell Pattern

During streaming, `ChatWidget` holds an `Option<HistoryCell>` as the active cell. This cell is mutated in place with each token delta. When streaming completes (the turn finishes), the active cell is committed into the transcript `VecDeque`. All mutations to the active cell MUST bump the active-cell revision counter so the transcript overlay cache is invalidated.

### Width Changes

When the terminal resizes:

1. `CustomTerminal` detects the resize via `autoresize()`.
2. `ChatWidget` calls `reflow_transcript()` which re-renders every committed `HistoryCell` at the new width and recomputes scroll positions.
3. Streaming `StreamCore` instances re-render their retained raw markdown source at the new width and rebuild pending queues. Already-emitted lines are preserved.

## Streaming Pipeline

The streaming pipeline transforms LLM token deltas into visible output with backpressure control:

```
Token delta --> MarkdownStreamCollector --> StreamCore --> StreamState queue --> commit_tick --> ChatWidget transcript
```

### Key Invariants

- `MarkdownStreamCollector` only commits source at newline boundaries. The trailing incomplete line stays in a buffer for the next delta.
- `StreamCore` tracks three offsets: `emitted_len`, `enqueued_len`, and `rendered_lines.len()`. These MUST remain in order: `emitted <= enqueued <= rendered`.
- `StreamState` records arrival timestamps for enqueued entries so backpressure can reason about queue age without peeking into line content.
- `commit_tick.rs` polls the queue on a timer and drains using `AdaptiveChunkingPolicy`, which balances responsiveness (don't wait too long) against rendering cost (don't redraw on every byte).

### Adding a New Stream Type

Follow the pattern in `ChatWidget` where assistant output and plan streams coexist:

1. Create a `StreamController` that owns a `StreamCore` and exposes `push_delta`, `tick`, and `finalize` methods.
2. Store the controller in `ChatWidget` alongside the active cell.
3. On `tick`, drain emitted lines and mutate the active cell's content.

## BottomPane & Popup Pattern

The `BottomPane` owns a stack of popups drawn above the composer:

### Adding a New Popup

1. Create the popup widget in `bottom_pane/` (e.g., `command_popup.rs`).
2. Add an enum variant to `BottomPane::popup_state` (or introduce a new state mechanism if needed).
3. Implement rendering: the popup receives a `Rect` and draws into a `Buffer`. Use `popup_consts.rs` for sizing constants (`POPUP_HEIGHT`, `POPUP_WIDTH_PERCENT`).
4. Implement key handling: `BottomPane::dispatch_key_event()` routes to the active popup. The popup returns an `InputResult` indicating the action taken.
5. If the popup shows a scrollable list, reuse `list_selection_view::ListSelectionView` rather than building a custom list.

### Focus Management

- `BottomPane` tracks `has_input_focus` and delegates to the active surface (composer or current popup).
- Popups that compete for focus (e.g., autocomplete vs. slash commands) use `PopupFocus` ordering in the compositor.
- When a popup closes, focus returns to the composer. The popup's result (model selection, theme choice, etc.) is surfaced through `InputResult`.

## Text & Markdown Rendering

### Wrapping

- For general text that may contain URLs or file paths, use `adaptive_wrap_line` / `adaptive_wrap_lines` from `wrapping.rs`. These detect URL-like tokens and prevent splitting them across lines.
- For code blocks and plain prose where URLs are impossible, use `word_wrap_line` / `word_wrap_lines` for performance.
- Never call `textwrap` directly; route through `wrapping.rs` so the wrapping behavior is consistent.

### File Links

- `markdown_render.rs` treats local file paths differently from web links. Local file-link text comes from the destination path, not the label, so transcripts show the actual file target.
- Paths are shortened relative to the session working directory when possible. Always pass `cwd` to `append_markdown` so streamed and non-streamed rendering produce the same relative-path text.

### Syntax Highlighting

- Use `crate::render::highlight::highlight_code_to_lines` for syntax highlighting in code blocks and diffs.
- Highlighting uses `syntect`/`two-face` under the hood. Language detection is extension-based; ensure new file types are registered in the highlight module.

## Testing

### Test File Conventions

- Place unit tests in dedicated `_tests.rs` files alongside the module they test (e.g., `chatwidget_tests.rs`, `custom_terminal_clear_tests.rs`, `markdown_render_tests.rs`).
- For smaller test suites (<50 lines), inline `#[cfg(test)] mod tests { ... }` in the module file is acceptable.
- Do not add test functions to production code files outside of `#[cfg(test)]` blocks.

### Test Harnesses

- Widget tests typically create a channel pair and construct the widget with an `AppEventSender` backed by the test channel:
  ```rust
  let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
  let widget = ChatWidget::new_with_app_event(ChatWidgetInit {
      app_event_tx: AppEventSender::new(app_event_tx),
      // ...
  });
  ```
- Use `FrameRequester::test_dummy()` for redraw scheduling in tests; do not depend on real frame-timing behavior.
- When testing streaming behavior, construct `StreamCore` / `StreamState` directly rather than going through the full TUI loop.
- Use `pretty_assertions::assert_eq` for all assertions. Deep-equality checks on entire objects are preferred over individual field comparisons.

### Platform-Specific Tests

- Use `#[cfg(unix)]` and `#[cfg(windows)]` to define platform-specific test cases when behavior differs.
- Tests that involve terminal modes (raw mode, alternate screen) should be `#[cfg(unix)]` unless Windows behavior is explicitly covered.

## Terminal Lifecycle

### Entering/Exiting Raw Mode

- Terminal mode entry and restoration are managed by `tui.rs`. Raw mode is entered once at startup and restored exactly once at exit via `TerminalRestoreGuard` in `host.rs`.
- The restore happens after the TUI area is cleared and before the shell prints the next prompt, preventing Terminal.app prompt drift.
- For temporary restoration during external interactive programs (Ctrl+Z, spawned subprocesses), use `tui.rs` suspend/resume pairs. Do not call crossterm raw-mode functions directly.

### Frame Scheduling

- `FrameRateLimiter` caps draw frequency to avoid wasting terminal work on intermediate states. Configurable in `tui.rs` initialization.
- `FrameRequester` collapses rapid redraw requests: multiple `request_frame()` calls between draws result in a single render.
- When a widget changes state that should push a frame, call `FrameRequester::request_frame()`. Do not call terminal draw methods directly.

### Overlays (Alternate Screen)

- The Ctrl+T transcript pager and other full-screen overlays use the terminal alternate screen. `pager_overlay.rs` owns the rendering and scroll state; `host_overlay.rs` owns the enter/exit lifecycle.
- Overlays receive their own key events; `host.rs` dispatches to the overlay rather than to `ChatWidget` when an overlay is active.
- Adding a new overlay: follow the `Overlay` enum + `OverlayState` pattern in `pager_overlay.rs` and `host_overlay.rs`.

## Dependencies

- **ratatui**: The immediate-mode TUI library. Widgets implement `ratatui::widgets::Widget`. Use `ratatui::text::Line` and `ratatui::text::Span` for styled text.
- **crossterm**: Terminal backend. Only use crossterm types in `tui.rs`, `host.rs`, and input-handling code. Higher-level widgets should not import crossterm directly unless handling key events.
- **pulldown-cmark**: Markdown parser. Used by `markdown_render.rs`. Do not use other markdown libraries.
- **syntect / two-face**: Syntax highlighting. Used by `render/highlight.rs`. Do not add other highlighting dependencies.
- **tokio**: Async runtime. The event loop in `host.rs` is a `tokio::select!` over three streams. Worker communication uses tokio `mpsc` channels and `tokio::process`.
- **textwrap**: Line wrapping with URL-aware heuristics. Always accessed through `wrapping.rs`, never directly.
- **unicode-width / unicode-segmentation**: Width calculations and grapheme cluster iteration. Use these when measuring or splitting user-visible text.

## Style & Naming

### Types

- Enum variants use PascalCase, not SCREAMING_CASE (e.g., `CompletionReason::NewTurn`, not `COMPLETION_REASON_NEW_TURN`).
- Parameter structs use a `Params` suffix (e.g., `BottomPaneParams`, `OutputLinesParams`, `SelectionViewParams`).
- Initialization structs use an `Init` suffix (e.g., `ChatWidgetInit`).

### Channel Names

- Sender ends: `_tx` suffix (e.g., `app_event_tx`).
- Receiver ends: `_rx` suffix (e.g., `app_event_rx`).

### Import Style

- Use crate-qualified paths for sibling modules (e.g., `crate::chatwidget::ChatWidget`, not `super::ChatWidget`).
- Group imports in this order: std library, external crates, devo crates, crate-local modules. Separate each group with a blank line.

## Commits & Documentation

- When adding or changing public API types in `app.rs` or `lib.rs`, ensure the `docs/` directory at the repository root is updated if applicable.
- Module-level `//!` doc comments should describe what the module does and how it fits into the broader architecture, not how it works internally. Internal details go on individual types and functions.
- Internal iterator patterns and streaming states should include brief comments explaining the invariants (e.g., the `emitted <= enqueued <= rendered` ordering).
