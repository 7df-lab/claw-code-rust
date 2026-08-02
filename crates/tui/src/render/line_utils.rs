use ratatui::text::Line;
use ratatui::text::Span;

/// Clone a borrowed ratatui `Line` into an owned `'static` line.
pub fn line_to_static(line: &Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|s| Span {
                style: s.style,
                content: std::borrow::Cow::Owned(s.content.to_string()),
            })
            .collect(),
    }
}

/// Append owned copies of borrowed lines to `out`.
pub fn push_owned_lines<'a>(src: &[Line<'a>], out: &mut Vec<Line<'static>>) {
    out.reserve(src.len());
    for l in src {
        out.push(line_to_static(l));
    }
}

/// Consider a line blank if it has no spans or only spans whose contents are
/// empty or consist solely of spaces (no tabs/newlines).
pub fn is_blank_line_spaces_only(line: &Line<'_>) -> bool {
    if line.spans.is_empty() {
        return true;
    }
    line.spans
        .iter()
        .all(|s| s.content.is_empty() || s.content.bytes().all(|byte| byte == b' '))
}

/// True when the line is a non-empty horizontal rule made only of `─` characters.
pub fn is_horizontal_rule_line(line: &Line<'_>) -> bool {
    let mut saw_rule = false;
    for span in &line.spans {
        for ch in span.content.chars() {
            if ch != '─' {
                return false;
            }
            saw_rule = true;
        }
    }
    saw_rule
}

/// Prefix each line with `initial_prefix` for the first line and
/// `subsequent_prefix` for following lines. Returns a new Vec of owned lines.
pub fn prefix_lines(
    lines: Vec<Line<'static>>,
    initial_prefix: Span<'static>,
    subsequent_prefix: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let mut spans = Vec::with_capacity(l.spans.len() + 1);
            spans.push(if i == 0 {
                initial_prefix.clone()
            } else {
                subsequent_prefix.clone()
            });
            spans.extend(l.spans);
            Line::from(spans).style(l.style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Stylize;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn push_owned_lines_appends_owned_lines() {
        let mut out = vec![Line::from("existing")];
        let src = [
            Line::from(vec!["alpha".red(), Span::raw(" beta")]),
            Line::from("gamma").centered(),
        ];

        push_owned_lines(&src, &mut out);

        let expected = vec![
            Line::from("existing"),
            Line::from(vec!["alpha".red(), Span::raw(" beta")]),
            Line::from("gamma").centered(),
        ];
        assert_eq!(expected, out);
    }

    #[test]
    fn blank_line_detection_accepts_only_empty_and_space_spans() {
        let lines = [
            Line::from(""),
            Line::from("    "),
            Line::from(vec![Span::raw("  "), Span::raw("")]),
        ];
        let detected = lines
            .iter()
            .map(is_blank_line_spaces_only)
            .collect::<Vec<_>>();

        assert_eq!(detected, vec![true, true, true]);
    }

    #[test]
    fn horizontal_rule_line_detection() {
        assert!(is_horizontal_rule_line(&Line::from("─".repeat(8).dim())));
        assert!(is_horizontal_rule_line(&Line::from("────")));
        assert!(!is_horizontal_rule_line(&Line::from("")));
        assert!(!is_horizontal_rule_line(&Line::from("── x ──")));
        assert!(!is_horizontal_rule_line(&Line::from("  ")));
    }

    #[test]
    fn blank_line_detection_rejects_tabs_newlines_and_text() {
        let lines = [
            Line::from("\t"),
            Line::from(vec![Span::raw("\n")]),
            Line::from(vec![Span::raw("  "), Span::raw("x")]),
        ];
        let detected = lines
            .iter()
            .map(is_blank_line_spaces_only)
            .collect::<Vec<_>>();

        assert_eq!(detected, vec![false, false, false]);
    }

    #[ignore]
    #[test]
    fn bench_blank_line_spaces_only_many_spans() {
        let line = Line::from(
            (0..256)
                .map(|idx| Span::raw(" ".repeat((idx % 8) + 1)))
                .collect::<Vec<_>>(),
        );
        let iterations = 500_000u32;
        let start = Instant::now();
        let mut blank_count = 0usize;

        for _ in 0..iterations {
            if black_box(is_blank_line_spaces_only(black_box(&line))) {
                blank_count += 1;
            }
        }

        let elapsed = start.elapsed();
        assert_eq!(blank_count, iterations as usize);
        eprintln!(
            "bench_blank_line_spaces_only_many_spans: {:.2?} total, {:.2?}/iter",
            elapsed,
            elapsed / iterations
        );
    }

    #[ignore]
    #[test]
    fn bench_push_owned_lines_many_lines() {
        let src: Vec<Line<'static>> = (0..256)
            .map(|i| {
                Line::from(vec![
                    Span::raw(format!("line {i}: ")),
                    "status".green(),
                    Span::raw(" rendered output with enough text to allocate"),
                ])
            })
            .collect();
        let iterations = 50_000u32;
        let start = Instant::now();
        let mut total_lines = 0usize;

        for _ in 0..iterations {
            let mut out = Vec::new();
            push_owned_lines(black_box(&src), black_box(&mut out));
            total_lines += out.len();
            black_box(out);
        }

        let elapsed = start.elapsed();
        assert_eq!(src.len() * iterations as usize, total_lines);
        eprintln!(
            "bench_push_owned_lines_many_lines: {:.2?} total, {:.2?}/iter",
            elapsed,
            elapsed / iterations
        );
    }

    #[ignore]
    #[test]
    fn bench_prefix_lines_many_lines() {
        let src: Vec<Line<'static>> = (0..256)
            .map(|i| {
                Line::from(vec![
                    Span::raw(format!("line {i}: ")),
                    "status".green(),
                    Span::raw(" rendered output with enough text to allocate"),
                ])
            })
            .collect();
        let iterations = 50_000u32;
        let start = Instant::now();
        let mut total_lines = 0usize;

        for _ in 0..iterations {
            let out = prefix_lines(
                black_box(src.clone()),
                black_box(Span::raw("  ")),
                black_box(Span::raw("  ")),
            );
            total_lines += out.len();
            black_box(out);
        }

        let elapsed = start.elapsed();
        assert_eq!(src.len() * iterations as usize, total_lines);
        eprintln!(
            "bench_prefix_lines_many_lines: {:.2?} total, {:.2?}/iter",
            elapsed,
            elapsed / iterations
        );
    }
}
