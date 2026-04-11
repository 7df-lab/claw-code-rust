use std::io::{self, Stdout};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect, style::Style,
};

/// Shared terminal type used by the interactive UI.
pub(crate) type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Strategy for deciding whether the UI should enter the alternate screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    /// Always use the alternate screen.
    Always,
    /// Never use the alternate screen.
    Never,
    /// Prefer the alternate screen by default.
    Auto,
}

/// Owns terminal mode changes for the lifetime of the interactive UI.
pub(crate) struct ManagedTerminal {
    /// The ratatui terminal backend used for rendering.
    terminal: AppTerminal,
    /// Whether we switched the terminal into the alternate screen on startup.
    use_alternate_screen: bool,
    /// Current reserved inline viewport height when rendering in the main screen buffer.
    inline_viewport_height: u16,
}

impl ManagedTerminal {
    /// Enters raw mode and, when appropriate, the alternate screen before constructing the backend.
    pub(crate) fn new(mode: TerminalMode) -> io::Result<Self> {
        // This wrapper centralizes terminal setup so cleanup happens reliably even
        // when the TUI exits early or panics.
        let mut stdout = io::stdout();
        let use_alternate_screen = should_use_alternate_screen(mode);
        terminal::enable_raw_mode()?;
        if use_alternate_screen {
            execute!(stdout, EnterAlternateScreen, Hide)?;
        } else {
            execute!(stdout, Hide)?;
        }
        let backend = CrosstermBackend::new(stdout);
        let inline_viewport_height = 1;
        let terminal = if use_alternate_screen {
            Terminal::new(backend)?
        } else {
            Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Inline(inline_viewport_height),
                },
            )?
        };
        Ok(Self {
            terminal,
            use_alternate_screen,
            inline_viewport_height,
        })
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

    pub(crate) fn set_inline_viewport_height(&mut self, height: u16) -> io::Result<()> {
        if self.use_alternate_screen {
            return Ok(());
        }

        let next_height = height.max(1);
        if next_height == self.inline_viewport_height {
            return Ok(());
        }

        let mut stdout = io::stdout();
        execute!(stdout, Hide)?;
        self.terminal = Terminal::with_options(
            CrosstermBackend::new(stdout),
            TerminalOptions {
                viewport: Viewport::Inline(next_height),
            },
        )?;
        self.inline_viewport_height = next_height;
        Ok(())
    }

    pub(crate) fn uses_alternate_screen(&self) -> bool {
        self.use_alternate_screen
    }

    pub(crate) fn insert_history_block(&mut self, text: &str) -> io::Result<()> {
        if self.use_alternate_screen || text.is_empty() {
            return Ok(());
        }

        let lines = text
            .split('\n')
            .map(std::borrow::ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return Ok(());
        }

        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        self.terminal.insert_before(height, |buf| {
            for (index, line) in lines.iter().enumerate().take(buf.area.height as usize) {
                buf.set_stringn(
                    0,
                    index as u16,
                    line,
                    usize::from(buf.area.width),
                    Style::default(),
                );
            }
        })
    }

    pub(crate) fn flush_pending_inline_history(
        &mut self,
        pending_inline_history: &mut Vec<String>,
    ) -> io::Result<()> {
        if pending_inline_history.is_empty() {
            return Ok(());
        }

        let text = pending_inline_history.concat();
        pending_inline_history.clear();
        self.insert_history_block(&text)
    }

    /// Restores the terminal to normal mode.
    pub(crate) fn restore(&mut self) -> io::Result<()> {
        // Drop back to the original terminal state before returning control to the
        // shell and show the cursor again for non-TUI workflows.
        terminal::disable_raw_mode()?;
        if self.use_alternate_screen {
            execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen)?;
        } else {
            execute!(self.terminal.backend_mut(), Show)?;
        }
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for ManagedTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn should_use_alternate_screen(mode: TerminalMode) -> bool {
    match mode {
        TerminalMode::Always => true,
        TerminalMode::Never => false,
        TerminalMode::Auto => true,
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalMode;
    use super::should_use_alternate_screen;

    #[test]
    fn alternate_screen_is_disabled_when_explicitly_requested() {
        assert!(!should_use_alternate_screen(TerminalMode::Never));
    }

    #[test]
    fn auto_mode_defaults_to_alternate_screen() {
        assert!(should_use_alternate_screen(TerminalMode::Auto));
    }

    #[test]
    fn always_mode_forces_alternate_screen() {
        assert!(should_use_alternate_screen(TerminalMode::Always));
    }
}
