//! History cell for completed generic tool calls.
//!
//! Inline rendering keeps tool output compact, while transcript rendering keeps
//! the full output available for the Ctrl+T pager.

use crate::ansi_escape::ansi_escape_line;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::exec_cell::truncated_tool_output_preview;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::HistoryCell;

const INLINE_OUTPUT_PREVIEW_ROWS: usize = 3;
const INLINE_OUTPUT_PREVIEW_LINE_LIMIT: usize = 2;

#[derive(Debug)]
pub(crate) struct ToolResultCell {
    title_line: Option<Line<'static>>,
    output: String,
    dot_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
    output_style: Style,
    show_empty_ellipsis: bool,
}

impl ToolResultCell {
    pub(crate) fn new(
        title_line: Option<Line<'static>>,
        output: String,
        dot_prefix: Line<'static>,
        subsequent_prefix: Line<'static>,
        output_style: Style,
        show_empty_ellipsis: bool,
    ) -> Self {
        Self {
            title_line,
            output,
            dot_prefix,
            subsequent_prefix,
            output_style,
            show_empty_ellipsis,
        }
    }

    fn inline_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.title_line.iter().cloned().collect::<Vec<_>>();
        let mut preview_lines = truncated_tool_output_preview(
            &self.output,
            width,
            INLINE_OUTPUT_PREVIEW_ROWS,
            INLINE_OUTPUT_PREVIEW_LINE_LIMIT,
        );
        if self.output_style != Style::default() {
            for line in &mut preview_lines {
                line.spans = std::mem::take(&mut line.spans)
                    .into_iter()
                    .map(|span| span.patch_style(self.output_style))
                    .collect();
            }
        }
        if self.show_empty_ellipsis && preview_lines.is_empty() {
            preview_lines.push(Line::from("...").patch_style(self.output_style));
        }
        lines.extend(preview_lines);
        lines
    }

    fn full_output_lines(&self) -> Vec<Line<'static>> {
        self.output
            .lines()
            .map(|line| {
                let mut line = ansi_escape_line(line);
                if self.output_style != Style::default() {
                    line.spans = line
                        .spans
                        .into_iter()
                        .map(|span| span.patch_style(self.output_style))
                        .collect();
                }
                line
            })
            .collect()
    }

    fn prefixed_cell(&self, lines: Vec<Line<'static>>) -> AgentMessageCell {
        AgentMessageCell::new_with_prefix(
            lines,
            self.dot_prefix.clone(),
            self.subsequent_prefix.clone(),
            /*is_stream_continuation*/ false,
        )
    }
}

impl HistoryCell for ToolResultCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.prefixed_cell(self.inline_lines(width))
            .display_lines(width)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.title_line.iter().cloned().collect::<Vec<_>>();
        lines.extend(self.full_output_lines());
        if self.show_empty_ellipsis && lines.len() == self.title_line.iter().count() {
            lines.push(Line::from("...").patch_style(self.output_style));
        }
        self.prefixed_cell(lines).display_lines(width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use ratatui::text::Span;
    use std::hint::black_box;
    use std::time::Instant;

    fn plain(lines: Vec<Line<'static>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn large_default_cell() -> ToolResultCell {
        let output = (1..=512)
            .map(|index| format!("line {index}: tool output payload"))
            .collect::<Vec<_>>()
            .join("\n");
        ToolResultCell::new(
            Some(Line::from("Ran test")),
            output,
            Line::from(vec![Span::from("| ")]),
            Line::from("  "),
            Style::default(),
            false,
        )
    }

    #[test]
    fn inline_output_is_truncated_but_transcript_output_is_full() {
        let output = (1..=8)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cell = ToolResultCell::new(
            Some(Line::from("Ran test")),
            output,
            Line::from(vec![Span::from("| ")]),
            Line::from("  "),
            Style::default(),
            false,
        );

        let inline = plain(cell.display_lines(80)).join("\n");
        let transcript = plain(cell.transcript_lines(80)).join("\n");

        assert!(inline.contains("line 1"));
        assert!(inline.contains("line 2"));
        assert!(inline.contains("ctrl + t to view transcript"));
        assert!(!inline.contains("line 3"));
        assert!(!inline.contains("line 7"));
        assert!(!inline.contains("line 8"));
        assert!(transcript.contains("line 5"));
        assert!(transcript.contains("line 8"));
    }

    #[test]
    fn output_style_patches_transcript_output() {
        let cell = ToolResultCell::new(
            None,
            "styled output".to_string(),
            Line::from(vec![Span::from("| ")]),
            Line::from("  "),
            Style::default().fg(Color::Red),
            false,
        );

        let lines = cell.transcript_lines(80);

        assert!(lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
            span.content.contains("styled output") && span.style.fg == Some(Color::Red)
        }));
    }

    #[test]
    #[ignore]
    fn bench_display_lines_default_style() {
        let cell = large_default_cell();
        let iterations = 20_000;
        let expected_len = cell.display_lines(100).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(cell.display_lines(black_box(100))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "tool_result_cell_display_lines_default_style iterations={iterations} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_transcript_lines_default_style() {
        let cell = large_default_cell();
        let iterations = 10_000;
        let expected_len = cell.transcript_lines(100).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(cell.transcript_lines(black_box(100))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "tool_result_cell_transcript_lines_default_style iterations={iterations} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
