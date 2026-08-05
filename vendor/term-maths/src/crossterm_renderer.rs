//! Crossterm output backend — writes a `RenderedBlock` to the terminal
//! at a specified cursor position.
//!
//! Feature-gated behind `crossterm`.

use std::io::Write;

use crossterm::{cursor::MoveTo, execute, style::Print};

use crate::rendered_block::RenderedBlock;

/// Renders a `RenderedBlock` to a terminal writer using crossterm commands.
pub struct CrosstermRenderer;

impl CrosstermRenderer {
    /// Write a rendered block to the terminal at the given (col, row) position.
    pub fn render_at<W: Write>(
        writer: &mut W,
        block: &RenderedBlock,
        col: u16,
        row: u16,
    ) -> std::io::Result<()> {
        for (r, cells) in block.cells().iter().enumerate() {
            execute!(writer, MoveTo(col, row + r as u16))?;
            let line: String = cells.iter().map(|s| s.as_str()).collect();
            execute!(writer, Print(&line))?;
        }
        Ok(())
    }

    /// Write a rendered block to stdout at the given position.
    pub fn print_at(block: &RenderedBlock, col: u16, row: u16) -> std::io::Result<()> {
        let mut stdout = std::io::stdout();
        Self::render_at(&mut stdout, block, col, row)
    }
}
