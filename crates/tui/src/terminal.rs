use std::io::{self, Stdout};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};

/// Shared terminal type used by the interactive UI.
pub(crate) type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Owns terminal mode changes for the lifetime of the interactive UI.
pub(crate) struct ManagedTerminal {
    /// The ratatui terminal backend used for rendering.
    terminal: AppTerminal,
}


// TODO(terminal-mode):
// Current TUI always enters alternate screen + raw mode, which:
// - clears scrollback
// - disables native terminal scrolling
// - makes transcript fully app-controlled
//
// This does not match a "shell-first" UX (e.g. Codex / Gemini CLI).
//
// Refactor terminal handling to support multiple modes:
//
// 1) Alternate screen strategy (configurable):
//    - always: always enter alternate screen (current behavior)
//    - never: render in main buffer (preserve scrollback)
//    - auto: detect environment (tmux/zellij/VSCode/xterm.js) and decide
//
// 2) Decouple terminal setup from TuiApp:
//    - pass `use_alternate_screen` into ManagedTerminal
//    - only call EnterAlternateScreen / LeaveAlternateScreen when enabled
//
// 3) Improve non-alt-screen behavior:
//    - avoid full-screen redraw assumptions
//    - ensure output remains readable in scrollback
//    - minimize flicker / cursor jumps
//    - rely more on terminal-native scrollback instead of app-managed transcript scrolling
//
// 4) Keyboard semantics (keep consistent across modes):
//    - Up / Down:
//        * navigate composer history OR slash suggestions
//        * MUST NOT change meaning based on terminal mode
//    - PageUp / PageDown (or equivalent):
//        * scroll transcript when in alt-screen mode
//    - In non-alt-screen mode:
//        * transcript scrolling should be delegated to terminal scrollback
//        * avoid introducing separate in-app scroll behavior
//
//    Rationale:
//    - avoid mode-dependent surprises for primary keys
//    - keep input-related navigation (history/suggestions) consistent
//    - separate "input navigation" from "output browsing"
//
// 5) Future direction:
//    - support hybrid mode: streaming output to stdout + interactive composer
//    - reduce reliance on app-internal scrolling for transcript
//
// Goal:
// make TUI compatible with both full-screen workflows AND shell-native workflows,
// while keeping interaction semantics consistent and predictable.

impl ManagedTerminal {
    /// Enters raw mode and the alternate screen before constructing the backend.
    pub(crate) fn new() -> io::Result<Self> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    /// Returns the current terminal area.
    pub(crate) fn area(&self) -> Rect {
        let size = self.terminal.size().unwrap_or_default();
        Rect::new(0, 0, size.width, size.height)
    }

    /// Returns a mutable reference to the underlying ratatui terminal.
    pub(crate) fn terminal_mut(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    /// Restores the terminal to normal mode.
    pub(crate) fn restore(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for ManagedTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
