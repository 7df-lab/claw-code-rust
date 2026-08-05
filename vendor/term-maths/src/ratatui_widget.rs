//! Ratatui widget backend — renders a `RenderedBlock` into a ratatui `Buffer`.
//!
//! Feature-gated behind `ratatui`.

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

use crate::rendered_block::RenderedBlock;

/// A ratatui widget that renders a `RenderedBlock` into a terminal buffer.
pub struct MathWidget<'a> {
    block: &'a RenderedBlock,
    style: Style,
}

impl<'a> MathWidget<'a> {
    pub fn new(block: &'a RenderedBlock) -> Self {
        Self {
            block,
            style: Style::default(),
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

impl Widget for MathWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let max_rows = area.height as usize;
        let max_cols = area.width as usize;

        for (r, row) in self.block.cells().iter().enumerate() {
            if r >= max_rows {
                break;
            }
            let y = area.y + r as u16;
            let mut x_offset = 0usize;
            for cell in row {
                if x_offset >= max_cols {
                    break;
                }
                let x = area.x + x_offset as u16;
                buf.set_string(x, y, cell, self.style);
                x_offset += unicode_width::UnicodeWidthStr::width(cell.as_str()).max(1);
            }
        }
    }
}
