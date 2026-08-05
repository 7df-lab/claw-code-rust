use std::fmt;
use unicode_width::UnicodeWidthStr;

/// A rectangular character grid with dimensional metadata.
///
/// This is the core data structure for 2D math rendering. Each cell contains
/// a string (to handle multi-codepoint grapheme clusters). The baseline marks
/// the row used for horizontal alignment when composing blocks side-by-side.
#[derive(Debug, Clone)]
pub struct RenderedBlock {
    /// Rows of character cells. Each cell is a `String` occupying one terminal column.
    cells: Vec<Vec<String>>,
    /// Width in terminal columns (via unicode-width).
    width: usize,
    /// Height in rows.
    height: usize,
    /// Row index of the alignment baseline (0-indexed from top).
    baseline: usize,
}

impl RenderedBlock {
    /// Create a new block from rows of cell strings.
    ///
    /// Width is computed from the first row (all rows must have equal width).
    /// The baseline defaults to `height / 2` if not specified.
    pub fn new(cells: Vec<Vec<String>>, baseline: usize) -> Self {
        let height = cells.len();
        let width = cells.first().map_or(0, |row| {
            row.iter().map(|c| UnicodeWidthStr::width(c.as_str())).sum()
        });
        Self {
            cells,
            width,
            height,
            baseline,
        }
    }

    /// Create a block containing a single character.
    pub fn from_char(ch: char) -> Self {
        let s = ch.to_string();
        let width = UnicodeWidthStr::width(s.as_str()).max(1);
        Self {
            cells: vec![vec![s]],
            width,
            height: 1,
            baseline: 0,
        }
    }

    /// Create a block from a string of text (single row).
    pub fn from_text(text: &str) -> Self {
        if text.is_empty() {
            return Self::empty();
        }
        let cells: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        let width = UnicodeWidthStr::width(text);
        Self {
            cells: vec![cells],
            width,
            height: 1,
            baseline: 0,
        }
    }

    /// Create an empty block with zero dimensions.
    pub fn empty() -> Self {
        Self {
            cells: vec![],
            width: 0,
            height: 0,
            baseline: 0,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn baseline(&self) -> usize {
        self.baseline
    }

    pub fn cells(&self) -> &[Vec<String>] {
        &self.cells
    }

    pub fn is_empty(&self) -> bool {
        self.height == 0 || self.width == 0
    }

    /// Place two blocks side-by-side, aligned on baselines.
    /// Pads the shorter block with empty rows above/below as needed.
    pub fn beside(&self, other: &RenderedBlock) -> RenderedBlock {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }

        let baseline = self.baseline.max(other.baseline);
        let above_baseline = baseline;

        let self_below = self.height.saturating_sub(self.baseline + 1);
        let other_below = other.height.saturating_sub(other.baseline + 1);
        let below_baseline = self_below.max(other_below);

        let total_height = above_baseline + 1 + below_baseline;
        let total_width = self.width + other.width;

        let self_top_pad = above_baseline - self.baseline;
        let other_top_pad = above_baseline - other.baseline;

        let mut rows = Vec::with_capacity(total_height);
        for row_idx in 0..total_height {
            let mut row = Vec::new();

            // Left block cells
            let self_row = row_idx.checked_sub(self_top_pad);
            if let Some(sr) = self_row {
                if sr < self.height {
                    row.extend(self.cells[sr].iter().cloned());
                } else {
                    row.extend(std::iter::repeat_n(" ".to_string(), self.width));
                }
            } else {
                row.extend(std::iter::repeat_n(" ".to_string(), self.width));
            }

            // Right block cells
            let other_row = row_idx.checked_sub(other_top_pad);
            if let Some(or_idx) = other_row {
                if or_idx < other.height {
                    row.extend(other.cells[or_idx].iter().cloned());
                } else {
                    row.extend(std::iter::repeat_n(" ".to_string(), other.width));
                }
            } else {
                row.extend(std::iter::repeat_n(" ".to_string(), other.width));
            }

            rows.push(row);
        }

        RenderedBlock {
            cells: rows,
            width: total_width,
            height: total_height,
            baseline,
        }
    }

    /// Stack two blocks vertically. The baseline is set to `baseline_row`
    /// (typically the dividing row between them, or top/bottom block's baseline).
    pub fn above(
        top: &RenderedBlock,
        bottom: &RenderedBlock,
        baseline_row: usize,
    ) -> RenderedBlock {
        let width = top.width.max(bottom.width);
        let mut rows = Vec::with_capacity(top.height + bottom.height);

        for r in 0..top.height {
            rows.push(Self::pad_row_to_width(&top.cells[r], top.width, width));
        }
        for r in 0..bottom.height {
            rows.push(Self::pad_row_to_width(
                &bottom.cells[r],
                bottom.width,
                width,
            ));
        }

        RenderedBlock {
            cells: rows,
            width,
            height: top.height + bottom.height,
            baseline: baseline_row,
        }
    }

    /// Add empty space around a block.
    pub fn pad(&self, left: usize, right: usize, top: usize, bottom: usize) -> RenderedBlock {
        let new_width = left + self.width + right;
        let new_height = top + self.height + bottom;

        let mut rows = Vec::with_capacity(new_height);

        // Top padding
        for _ in 0..top {
            rows.push(vec![" ".to_string(); new_width]);
        }

        // Content rows with left/right padding
        for r in 0..self.height {
            let mut row = Vec::with_capacity(new_width);
            row.extend(std::iter::repeat_n(" ".to_string(), left));
            row.extend(self.cells[r].iter().cloned());
            row.extend(std::iter::repeat_n(" ".to_string(), right));
            rows.push(row);
        }

        // Bottom padding
        for _ in 0..bottom {
            rows.push(vec![" ".to_string(); new_width]);
        }

        RenderedBlock {
            cells: rows,
            width: new_width,
            height: new_height,
            baseline: self.baseline + top,
        }
    }

    /// Horizontally centre a block within a given width.
    pub fn center_in(&self, target_width: usize) -> RenderedBlock {
        if target_width <= self.width {
            return self.clone();
        }
        let total_pad = target_width - self.width;
        let left_pad = total_pad / 2;
        let right_pad = total_pad - left_pad;
        self.pad(left_pad, right_pad, 0, 0)
    }

    /// Helper: pad a row of cells to a target width by appending spaces.
    fn pad_row_to_width(row: &[String], current_width: usize, target_width: usize) -> Vec<String> {
        let mut result = row.to_vec();
        let pad = target_width.saturating_sub(current_width);
        result.extend(std::iter::repeat_n(" ".to_string(), pad));
        result
    }

    /// Create a horizontal line of a given character and width.
    pub fn hline(ch: char, width: usize) -> RenderedBlock {
        let cells = vec![vec![ch.to_string(); width]];
        RenderedBlock {
            cells,
            width,
            height: 1,
            baseline: 0,
        }
    }
}

impl fmt::Display for RenderedBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, row) in self.cells.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            for cell in row {
                write!(f, "{}", cell)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_char() {
        let block = RenderedBlock::from_char('x');
        assert_eq!(block.width(), 1);
        assert_eq!(block.height(), 1);
        assert_eq!(block.baseline(), 0);
        assert_eq!(format!("{}", block), "x");
    }

    #[test]
    fn test_from_text() {
        let block = RenderedBlock::from_text("hello");
        assert_eq!(block.width(), 5);
        assert_eq!(block.height(), 1);
        assert_eq!(format!("{}", block), "hello");
    }

    #[test]
    fn test_beside_baseline_aligned() {
        // Two single-row blocks
        let a = RenderedBlock::from_text("ab");
        let b = RenderedBlock::from_text("cd");
        let result = a.beside(&b);
        assert_eq!(result.width(), 4);
        assert_eq!(result.height(), 1);
        assert_eq!(format!("{}", result), "abcd");
    }

    #[test]
    fn test_beside_different_heights() {
        // a is 3 rows tall with baseline at row 1
        let a = RenderedBlock::new(
            vec![vec!["a".into()], vec!["b".into()], vec!["c".into()]],
            1,
        );
        // d is 1 row tall with baseline at row 0
        let d = RenderedBlock::from_char('d');
        let result = a.beside(&d);
        assert_eq!(result.height(), 3);
        assert_eq!(result.baseline(), 1);
        // d should be on the baseline row (row 1)
        let output = format!("{}", result);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "a ");
        assert_eq!(lines[1], "bd");
        assert_eq!(lines[2], "c ");
    }

    #[test]
    fn test_center_in() {
        let block = RenderedBlock::from_text("ab");
        let centered = block.center_in(6);
        assert_eq!(centered.width(), 6);
        assert_eq!(format!("{}", centered), "  ab  ");
    }

    #[test]
    fn test_above() {
        let top = RenderedBlock::from_text("abc");
        let bottom = RenderedBlock::from_text("de");
        let result = RenderedBlock::above(&top, &bottom, 0);
        assert_eq!(result.height(), 2);
        assert_eq!(result.width(), 3);
        let output = format!("{}", result);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "abc");
        assert_eq!(lines[1], "de ");
    }

    #[test]
    fn test_pad() {
        let block = RenderedBlock::from_char('x');
        let padded = block.pad(1, 1, 1, 1);
        assert_eq!(padded.width(), 3);
        assert_eq!(padded.height(), 3);
        assert_eq!(padded.baseline(), 1);
        let output = format!("{}", padded);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "   ");
        assert_eq!(lines[1], " x ");
        assert_eq!(lines[2], "   ");
    }
}
