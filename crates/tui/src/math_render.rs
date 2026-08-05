use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedMathLine {
    pub(crate) line: Line<'static>,
    pub(crate) no_wrap: bool,
}

pub(crate) fn render_math(latex: &str, style: Style) -> Vec<RenderedMathLine> {
    let block = term_maths::render(latex.trim());
    let rows = block
        .cells()
        .iter()
        .map(|cells| {
            let text = cells.iter().map(String::as_str).collect::<String>();
            RenderedMathLine {
                line: Line::from(Span::styled(text, style)),
                no_wrap: true,
            }
        })
        .collect::<Vec<_>>();

    if rows.is_empty() {
        return vec![RenderedMathLine {
            line: Line::from(Span::styled(latex.trim().to_string(), style)),
            no_wrap: true,
        }];
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn plain_rows(rows: &[RenderedMathLine]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                row.line
                    .spans
                    .iter()
                    .map(|span| span.content.clone())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn fraction_is_rendered_as_multiple_terminal_rows() {
        let rows = render_math(r"\frac{a}{b}", Style::default());
        assert!(rows.len() >= 3);
        assert!(rows.iter().all(|row| row.no_wrap));
        let text = plain_rows(&rows).join("\n");
        assert!(text.contains('a'));
        assert!(text.contains('b'));
    }

    #[test]
    fn renders_superscript_and_greek_symbols() {
        let rows = render_math(r"x^2 + \alpha", Style::default());
        let text = plain_rows(&rows).join("\n");
        assert!(text.contains('²'));
        assert!(text.contains('α'));
    }

    #[test]
    fn empty_math_has_a_readable_fallback() {
        let rows = render_math("", Style::default());
        assert_eq!(plain_rows(&rows), vec![String::new()]);
        assert!(rows[0].no_wrap);
    }
}
