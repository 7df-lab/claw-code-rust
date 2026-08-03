# Terminal Math Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render Markdown math through `term-maths` in the TUI, preserve two-dimensional math rows, defer incomplete display-math blocks during streaming, and isolate ACP permission identity by session and turn.

**Architecture:** Add a TUI-owned adapter around `term_maths::render` that produces owned ratatui lines plus per-line no-wrap metadata. Thread that metadata through Markdown rendering, stream queues, and history cells. Keep the existing pulldown-cmark source model and delay streaming commits while a display-math delimiter is open.

**Tech Stack:** Rust, `pulldown-cmark 0.13.4`, `term-maths 1.0.0` core API, `ratatui 0.29`, Tokio tests, pretty_assertions.

## Global Constraints

- Preserve the existing Markdown parser and transcript source retention model.
- Keep ordinary Markdown rendering behavior unchanged.
- Use `term_maths::render` and `RenderedBlock::cells`; do not enable the optional `term-maths` ratatui feature.
- Preserve unrelated user changes in `crates/core/src/tools`, `crates/tools`, and all other pre-existing dirty files.
- Use `pretty_assertions::assert_eq` in tests and do not mutate process environment variables in tests.
- Run each targeted test after its failing test is added, then run the complete affected test suites.

---

### Task 1: Add the math engine adapter

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/tui/src/lib.rs`
- Create: `crates/tui/src/math_render.rs`
- Test: `crates/tui/src/math_render.rs`

**Interfaces:**
- `RenderedMathLine { line: Line<'static>, no_wrap: bool }`
- `render_math(latex: &str, style: Style) -> Vec<RenderedMathLine>`

- [ ] **Step 1: Add `term-maths = "1.0.0"` without features.** Run `cargo check -p devo-tui` and confirm the existing `ratatui 0.29` remains the only TUI ratatui API.

- [ ] **Step 2: Write failing tests before implementation.** Add tests for `render_math(r"\frac{a}{b}")`, `render_math(r"x^2 + \alpha")`, and the invariant that every returned math row is marked `no_wrap`.

```rust
#[test]
fn fraction_is_rendered_as_multiple_terminal_rows() {
    let rows = render_math(r"\frac{a}{b}", Style::default());
    assert!(rows.len() >= 3);
    assert!(rows.iter().all(|row| row.no_wrap));
    let text = rows.iter().map(|row| row.line.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains('a'));
    assert!(text.contains('b'));
}
```

Run `cargo test -p devo-tui math_render`; it must fail because the adapter is not implemented.

- [ ] **Step 3: Implement the minimal adapter.** Call `term_maths::render`, concatenate each `RenderedBlock::cells()` row into one owned span with the requested style, mark each row `no_wrap: true`, and return the trimmed source body as a one-row fallback if the block is empty.

- [ ] **Step 4: Run `cargo test -p devo-tui math_render` and require all adapter tests to pass.**

- [ ] **Step 5: Commit only this task:** `git add Cargo.toml Cargo.lock crates/tui/src/lib.rs crates/tui/src/math_render.rs`, then `git commit -m "feat(tui): add terminal math rendering adapter"`.

---

### Task 2: Integrate math rows into Markdown and history cells

**Files:**
- Modify: `crates/tui/src/markdown_render.rs`, `crates/tui/src/markdown.rs`, `crates/tui/src/history_cell.rs`
- Test: `crates/tui/src/markdown_render_tests.rs`, history-cell tests

**Interfaces:**
- A metadata-aware Markdown result containing `Vec<RenderedMathLine>` or an equivalent per-line `no_wrap` vector.
- Existing `append_markdown` callers remain valid; history and streaming callers use the metadata-aware path.

- [ ] **Step 1: Add failing tests.** Replace raw-LaTeX expectations with assertions that `$E = mc^2$` contains `mc²`, display `\frac{a}{b}` has multiple rows, delimiters are absent, and all display rows are no-wrap. Run the focused tests and confirm they fail against the current cyan raw-LaTeX implementation.

```rust
#[test]
fn inline_math_is_terminal_rendered() {
    let text = render_markdown_text(r"Energy is $E = mc^2$ in physics");
    let plain = text.lines.iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("mc²"));
    assert!(!plain.contains("$E"));
}
```

- [ ] **Step 2: Add writer metadata.** Track `current_line_no_wrap` and append the flag whenever `flush_current_line` appends a row. Route `InlineMath` and `DisplayMath` through `render_math` instead of `highlight_code_to_lines`.

- [ ] **Step 3: Integrate inline and display layout.** One-row inline blocks stay beside prose. Multi-row inline blocks flush the current prose row, emit the math rows as an intact group, and continue following prose on the next row. Display blocks emit all rows structurally without `$` delimiters.

- [ ] **Step 4: Make history wrapping metadata-aware.** Add a shared helper used by `AgentMessageCell` and `AgentMarkdownCell`: ordinary rows use `adaptive_wrap_lines`, while no-wrap math rows receive only their initial/continuation prefix and are emitted intact.

- [ ] **Step 5: Run `cargo test -p devo-tui markdown_render history_cell` and require the new and existing tests to pass.**

- [ ] **Step 6: Commit the task:** `git add crates/tui/src/math_render.rs crates/tui/src/markdown_render.rs crates/tui/src/markdown.rs crates/tui/src/history_cell.rs crates/tui/src/markdown_render_tests.rs`, then `git commit -m "feat(tui): render markdown math as terminal blocks"`.

---

### Task 3: Defer incomplete display math during streaming

**Files:**
- Modify: `crates/tui/src/markdown_stream.rs`, `crates/tui/src/streaming/mod.rs`, `crates/tui/src/streaming/controller.rs`, `crates/tui/src/chatwidget/text_stream.rs`
- Test: `crates/tui/src/markdown_stream.rs`, `crates/tui/src/streaming/controller.rs`

**Interfaces:**
- Collector commits only a newline outside an open display-math block.
- Stream queue entries carry `Line<'static>` plus `no_wrap`.
- Stream-emitted `AgentMessageCell`s receive the same metadata as finalized cells.

- [ ] **Step 1: Add a failing controller test.** Push and drain `Before\n`, `$$\n`, `\\frac{a}{b}\n`, then `$$\n`; finalize and assert the collected output contains no `$$` and contains numerator and denominator rows. Run the focused streaming test and confirm the current controller fails because it emits raw opening lines.

- [ ] **Step 2: Implement delimiter-aware commit boundaries.** Scan source while ignoring escaped dollars, fenced code blocks, and inline code spans. If the last newline lies inside an open `$$` block, cap the commit before that block. Once the closing delimiter arrives, commit the complete block together.

- [ ] **Step 3: Thread metadata through queues.** Extend `QueuedLine`, `StreamState::enqueue`, controller render caches, drain methods, and stream cell construction without changing FIFO ordering or chunking policy.

- [ ] **Step 4: Run `cargo test -p devo-tui streaming markdown_stream chatwidget_tests::completed_streaming_assistant_consolidates_to_source_backed_cell` and require no raw delimiters or regressions.**

- [ ] **Step 5: Commit the task:** `git add crates/tui/src/markdown_stream.rs crates/tui/src/streaming crates/tui/src/chatwidget/text_stream.rs`, then `git commit -m "fix(tui): defer incomplete display math during streaming"`.

---

### Task 4: Isolate ACP permission identities

**Files:**
- Modify: `crates/client/src/acp_permissions.rs`, `crates/tui/src/chatwidget.rs`, `crates/tui/src/chatwidget/worker_events.rs`
- Test: `crates/client/src/acp_permissions.rs`, `crates/tui/src/chatwidget_tests.rs`

**Interfaces:**
- External `ApprovalResponseParams.approval_id` remains the server/tool-call ID.
- Internal pending permission lookup uses `(SessionId, approval_id)`.
- UI decision de-duplication is scoped to turn lifecycle.

- [ ] **Step 1: Add failing collision tests.** Submit two requests from different sessions with the same `toolCallId`, resolve both, and assert both original JSON-RPC IDs are returned. Add a TUI test that reuses an approval ID in separate turns and asserts both decisions render.

- [ ] **Step 2: Implement composite internal keys.** Use a private key helper consistently for pending insertion, failed-send cleanup, and response removal; do not change the protocol payload ID.

- [ ] **Step 3: Clear UI deduplication on `TurnStarted`, `TurnFinished`, and `TurnFailed`, while preserving duplicate suppression inside one turn.**

- [ ] **Step 4: Run `cargo test -p devo-client acp_permissions` and the focused TUI approval tests.**

- [ ] **Step 5: Commit the task:** `git add crates/client/src/acp_permissions.rs crates/tui/src/chatwidget.rs crates/tui/src/chatwidget/worker_events.rs crates/tui/src/chatwidget_tests.rs`, then `git commit -m "fix: scope approval identities by session and turn"`.

---

### Task 5: Document and verify

**Files:**
- Modify: `crates/tui/README.md`

- [ ] **Step 1: Document `term-maths` Unicode terminal rendering, intentional omission of its ratatui feature, no-wrap math rows, and buffering of incomplete display math.**

- [ ] **Step 2: Run `cargo fmt --all -- --check` and `git diff --check`; both commands must exit 0.**

- [ ] **Step 3: Run `cargo test -p devo-tui --lib` and `cargo test -p devo-client --lib`; both suites must report zero failures.**

- [ ] **Step 4: Run `cargo clippy -p devo-tui --lib --all-targets -- -D warnings` and `cargo clippy -p devo-client --lib --all-targets -- -D warnings`; both commands must finish successfully.**

- [ ] **Step 5: Confirm `git diff --name-only` contains only implementation files plus pre-existing user files. Never stage or revert unrelated `crates/core/src/tools` or `crates/tools` changes.**

- [ ] **Step 6: Commit only the documentation:** `git add crates/tui/README.md`, then `git commit -m "docs(tui): document terminal math rendering"`.
