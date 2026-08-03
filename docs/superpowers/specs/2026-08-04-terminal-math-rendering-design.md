# Terminal Math Rendering Design

## Goal

Render Markdown LaTeX formulas as readable two-dimensional terminal output in the TUI. Inline and display formulas must no longer appear as raw `$...$`, `$$...$$`, or syntax-highlighted LaTeX source. Streaming output must not commit an incomplete display-math block into immutable transcript history.

## Constraints

- Preserve the existing Markdown parser and transcript source retention model.
- Keep ordinary Markdown rendering behavior unchanged.
- Keep finalized assistant messages backed by their original Markdown source so resize reflow remains correct.
- Use `term-maths` as the math layout engine rather than maintaining a local LaTeX-to-Unicode mapping.
- The repository currently uses `ratatui 0.29`; do not enable a `term-maths` feature that requires a second incompatible `ratatui` version.
- If a formula cannot be represented by the renderer, retain its content in a readable fallback instead of dropping it.

## Architecture

### Math engine adapter

Add a small TUI-owned adapter around `term_maths::render`. The adapter converts a `RenderedBlock` into the project's owned `ratatui::Line` representation and applies the existing math/code visual style. It must expose enough layout information to distinguish math rows from ordinary Markdown rows.

The adapter will use the core `render()` and `RenderedBlock::cells()` APIs. The optional `term-maths` `ratatui` widget feature is intentionally not used because its current release targets `ratatui ^0.30`, while this repository uses `ratatui 0.29`.

### Markdown integration

`InlineMath` events will be rendered through the adapter. One-row formulas can remain beside surrounding text. Multi-row formulas, such as fractions, will be composed with neighboring text using the block baseline and emitted as a small group of rows.

`DisplayMath` events will be rendered as a structural block. The resulting rows will carry no-wrap metadata so the later history-cell layout cannot split a formula horizontally. The original `$$` delimiters will never be emitted as visible content for a successfully parsed display block.

### Streaming boundaries

The Markdown stream collector will track whether the source contains an open display-math delimiter outside fenced code. A commit may end only at a newline that is outside such a block. Source inside an open display block remains buffered. Once the closing delimiter arrives, the complete block is parsed and queued together, so previously emitted history cells never contain a provisional `$$` representation.

Inline math remains newline-gated as before; a formula split across token deltas is held until its line is complete.

### History wrapping

The rendered Markdown result will retain per-line no-wrap information. Both live `AgentMessageCell` output and finalized `AgentMarkdownCell` output will use that metadata: ordinary lines use the existing adaptive wrapping, while math rows are emitted intact. Width changes re-render from source and recompute the math layout.

### Approval identity isolation

The externally visible `approval_id` remains the server/tool-call identifier so server and client decision events can be deduplicated. The pending ACP permission map will use a composite internal key of `(session_id, approval_id)`. The UI decision de-duplication set will be cleared at turn boundaries as well as session boundaries, preventing a reused tool-call identifier from suppressing a later legitimate decision.

## Error handling

- A renderer failure or empty result must fall back to the formula body without delimiters, preserving user-visible content.
- Unknown Markdown events keep their existing behavior.
- An unterminated display-math block is kept as source while streaming and is rendered as a normal fallback only when the stream is finalized or interrupted.
- Formula layout must not panic on malformed LaTeX or unusually large input.

## Tests

Add or update tests for:

- `x^2`, Greek symbols, `\frac{a}{b}`, nested fractions, sums, and matrices.
- Inline formulas beside text, including a multi-row inline formula.
- Display formulas preserving their two-dimensional rows and no-wrap behavior.
- Malformed or unsupported formulas retaining readable fallback content.
- A `StreamController` scenario that commits lines before the closing `$$` arrives and verifies no raw delimiters enter history.
- Resize/re-render behavior for math rows.
- ACP pending permissions with identical tool-call IDs in different sessions.
- Repeated approval IDs across separate turns.

## Verification

Run targeted math and stream tests first, then the complete TUI and client test suites, `cargo fmt --all -- --check`, `git diff --check`, and Clippy with warnings denied for the affected crates.
