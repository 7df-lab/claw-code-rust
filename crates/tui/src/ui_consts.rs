//! Shared UI constants for layout and alignment within the TUI.

/// Width (in terminal columns) reserved for the left gutter/prefix used by
/// live cells and aligned widgets.
///
/// Semantics:
/// - Chat composer reserves this many columns for the left border + padding.
/// - Status indicator lines begin with this many spaces for alignment.
/// - User history lines account for this many columns (e.g., "❯ ") when wrapping.
pub(crate) const LIVE_PREFIX_COLS: u16 = 2;
pub(crate) const FOOTER_INDENT_COLS: usize = LIVE_PREFIX_COLS as usize;

/// Warm amber used for the "Thought" heading and related accents.
pub(crate) const REASONING_ACCENT_COLOR: ratatui::style::Color =
    ratatui::style::Color::Rgb(210, 150, 60);
/// Orange used for soft alert history lines (`■ …`).
pub(crate) const ALERT_COLOR: ratatui::style::Color = ratatui::style::Color::Rgb(245, 142, 53);
/// Green used for completed, idle, and done indicators.
pub(crate) const COMPLETED_COLOR: ratatui::style::Color = ratatui::style::Color::Rgb(120, 220, 160);
