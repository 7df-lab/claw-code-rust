use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use textwrap::Options;
use unicode_width::UnicodeWidthChar;

use crate::{
    app::{AuxPanelContent, TuiApp},
    events::TranscriptItemKind,
};

/// Draws the full interactive UI for the current application state.
pub(crate) fn draw(frame: &mut Frame, app: &TuiApp) {
    let content_area = centered_content_area(frame.area());
    let composer_height = composer_height(app, content_area);
    let transcript_height = transcript_height(app, content_area);
    let [transcript_area, spacer_area, composer_area, footer_area] = Layout::vertical([
        Constraint::Length(transcript_height),
        Constraint::Length(1),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(content_area);

    frame.render_widget(render_transcript(app, transcript_area), transcript_area);
    frame.render_widget(Paragraph::new(""), spacer_area);
    frame.render_widget(
        render_composer(app, composer_area.width.max(1)),
        composer_area,
    );
    frame.render_widget(render_footer(app), footer_area);

    let cursor = composer_cursor(app, composer_area);
    frame.set_cursor_position(cursor);
}

pub(crate) fn centered_content_area(area: Rect) -> Rect {
    const MAX_CONTENT_WIDTH: u16 = 100;

    if area.width <= MAX_CONTENT_WIDTH {
        return area;
    }

    Rect {
        x: area.x,
        y: area.y,
        width: MAX_CONTENT_WIDTH,
        height: area.height,
    }
}

pub(crate) fn get_max_scroll(app: &TuiApp, area: Rect) -> u16 {
    let line_count = transcript_line_count(app, area.width.max(1));
    line_count.saturating_sub(area.height)
}

pub(crate) fn transcript_height(app: &TuiApp, area: Rect) -> u16 {
    let line_count = transcript_line_count(app, area.width.max(1)).max(1);
    let composer_height = composer_height(app, area);
    let available = area
        .height
        .saturating_sub(composer_height.saturating_add(2));
    line_count.min(available.max(1))
}

fn render_transcript(app: &TuiApp, area: Rect) -> Paragraph<'static> {
    let content = transcript_text(app, area.width.max(1));
    let max_scroll = content.lines.len().saturating_sub(area.height as usize) as u16;
    let scroll = if app.follow_output {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };

    Paragraph::new(content).scroll((scroll, 0))
}

fn render_composer(app: &TuiApp, inner_width: u16) -> Paragraph<'_> {
    let mut lines = Vec::new();
    let rendered_input = app.input.rendered_lines(inner_width);

    if app.input.text().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("> ", Style::new().cyan().add_modifier(Modifier::BOLD)),
            Span::styled("Type a message or / for commands", Style::new().dark_gray()),
        ]));
    } else {
        for line in rendered_input {
            if let Some(rest) = line.strip_prefix("> ") {
                lines.push(Line::from(vec![
                    Span::styled("> ", Style::new().cyan().add_modifier(Modifier::BOLD)),
                    Span::raw(rest.to_string()),
                ]));
            } else if let Some(rest) = line.strip_prefix("  ") {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::new().cyan().add_modifier(Modifier::BOLD)),
                    Span::raw(rest.to_string()),
                ]));
            } else {
                lines.push(Line::from(line));
            }
        }
    }

    let suggestions = app.slash_suggestions();
    if !suggestions.is_empty() {
        lines.push(Line::from(""));

        for (index, suggestion) in suggestions.iter().enumerate() {
            let selected = index == app.slash_selection.min(suggestions.len() - 1);

            let text_style = if selected {
                Style::new().black().on_gray().add_modifier(Modifier::BOLD)
            } else {
                Style::new().dark_gray()
            };

            append_wrapped_composer_text(
                &mut lines,
                &format!("  {}  {}", suggestion.name, suggestion.description),
                inner_width,
                text_style,
            );
        }
    }

    if let Some(panel) = &app.aux_panel {
        lines.push(Line::from(""));

        append_wrapped_composer_line(
            &mut lines,
            &format!("  {}", panel.title),
            inner_width,
            Style::new().dark_gray().add_modifier(Modifier::BOLD),
        );

        match &panel.content {
            AuxPanelContent::Text(body) => {
                for line in body.lines() {
                    append_wrapped_composer_line(
                        &mut lines,
                        &format!("  {line}"),
                        inner_width,
                        Style::new().dark_gray(),
                    );
                }
            }
            AuxPanelContent::SessionList(entries) => {
                if entries.is_empty() {
                    append_wrapped_composer_line(
                        &mut lines,
                        "  No saved sessions found.",
                        inner_width,
                        Style::new().dark_gray(),
                    );
                }

                for (index, entry) in entries.iter().enumerate() {
                    let selected =
                        index == app.aux_panel_selection.min(entries.len().saturating_sub(1));
                    let marker = if entry.is_active { "*" } else { " " };
                    let style = if selected {
                        Style::new().black().on_gray()
                    } else {
                        Style::new().dark_gray()
                    };
                    let title_style = if selected {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().dark_gray().add_modifier(Modifier::BOLD)
                    };

                    append_wrapped_composer_session_entry(
                        &mut lines,
                        &format!(
                            "  {} {marker} {}  [{}]  {}",
                            if selected { ">" } else { "•" },
                            entry.title,
                            entry.session_id,
                            entry.updated_at
                        ),
                        inner_width,
                        style,
                        title_style,
                    );
                }
            }
            AuxPanelContent::ModelList(entries) => {
                if entries.is_empty() {
                    append_wrapped_composer_line(
                        &mut lines,
                        "  No models available.",
                        inner_width,
                        Style::new().dark_gray(),
                    );
                }

                for (index, entry) in entries.iter().enumerate() {
                    let selected =
                        index == app.aux_panel_selection.min(entries.len().saturating_sub(1));
                    let marker = if entry.is_current { "*" } else { " " };
                    let label = if entry.is_builtin { "✨" } else { "custom" };
                    let style = if selected {
                        Style::new().black().on_gray()
                    } else {
                        Style::new().dark_gray()
                    };
                    let title_style = if selected {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().dark_gray().add_modifier(Modifier::BOLD)
                    };

                    let description = entry
                        .description
                        .as_deref()
                        .filter(|description| !description.trim().is_empty())
                        .unwrap_or(label);
                    append_wrapped_composer_session_entry(
                        &mut lines,
                        &format!(
                            "  {} {marker} {}  [{}]  {}",
                            if selected { ">" } else { "•" },
                            entry.display_name,
                            entry.slug,
                            description
                        ),
                        inner_width,
                        style,
                        title_style,
                    );
                }
            }
        }

        lines.push(Line::from(""));
        append_wrapped_composer_line(
            &mut lines,
            "  press enter to choose, esc to leave",
            inner_width,
            Style::new().dark_gray(),
        );
    }

    Paragraph::new(Text::from(lines))
}
fn render_footer(app: &TuiApp) -> Paragraph<'static> {
    let cwd_name = app
        .cwd
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| app.cwd.to_string_lossy().into_owned());
    let meta = format!("model {}   cwd {}", app.model, cwd_name);
    let status = if app.status_message.is_empty() {
        "Ready".to_string()
    } else {
        app.status_message.clone()
    };
    Paragraph::new(Line::from(vec![
        Span::styled(status, Style::new().dark_gray()),
        Span::styled("   ", Style::new().dark_gray()),
        Span::styled(meta, Style::new().dark_gray()),
    ]))
}

fn transcript_text(app: &TuiApp, inner_width: u16) -> Text<'static> {
    let mut lines = brand_lines();
    lines.push(Line::from(""));

    if app.transcript.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No conversation yet. Ask ClawCR to inspect code, explain behavior, or make changes.",
            Style::new().dark_gray(),
        )]));
        return Text::from(lines);
    }

    let mut previous_kind = None;
    for item in &app.transcript {
        if matches!(item.kind, TranscriptItemKind::User)
            && previous_kind.is_some()
            && !matches!(previous_kind, Some(TranscriptItemKind::User))
        {
            lines.push(Line::from(""));
        }
        if matches!(previous_kind, Some(TranscriptItemKind::User))
            && !matches!(item.kind, TranscriptItemKind::User)
        {
            lines.push(Line::from(""));
        }
        append_transcript_item(&mut lines, item, app.spinner_index, inner_width);
        previous_kind = Some(item.kind);
    }
    Text::from(lines)
}

fn transcript_line_count(app: &TuiApp, inner_width: u16) -> u16 {
    transcript_text(app, inner_width).lines.len() as u16
}

fn brand_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "   ________               _______ ",
            Style::new().cyan().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            "  / ____/ /___ __      __/ ____/ |",
            Style::new().cyan().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            " / /   / / __ `/ | /| / / /   | |",
            Style::new().cyan().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            "/ /___/ / /_/ /| |/ |/ / /___ | |",
            Style::new().cyan().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            "\\____/_/\\__,_/ |__/|__/\\____/ |_|",
            Style::new().cyan().add_modifier(Modifier::BOLD),
        )]),
    ]
}

fn append_transcript_item(
    lines: &mut Vec<Line<'static>>,
    item: &crate::events::TranscriptItem,
    spinner_index: usize,
    inner_width: u16,
) {
    match item.kind {
        TranscriptItemKind::User => {
            append_plain_message(lines, item, "> ", "  ", inner_width);
        }
        TranscriptItemKind::Assistant => {
            append_plain_message(lines, item, "• ", "  ", inner_width);
        }
        TranscriptItemKind::System if item.title == "Thinking" => {
            let spinner = ["⠋", "⠙", "⠹", "⠸", "⠴", "⠦"][spinner_index % 6];
            append_wrapped_styled_text(
                lines,
                &format!("{spinner} Thinking"),
                "• ",
                "  ",
                inner_width,
                Style::new().yellow().add_modifier(Modifier::BOLD),
            );
        }
        TranscriptItemKind::System if item.title == "Interrupted" => {
            append_wrapped_styled_text(
                lines,
                "Interrupted",
                "• ",
                "  ",
                inner_width,
                Style::new().yellow().add_modifier(Modifier::BOLD),
            );
        }
        TranscriptItemKind::ToolCall
        | TranscriptItemKind::ToolResult
        | TranscriptItemKind::System
        | TranscriptItemKind::Error => {
            append_wrapped_title(lines, &item.title, item.kind, inner_width);
            append_transcript_body(lines, item, inner_width);
        }
    }
}

fn append_plain_message(
    lines: &mut Vec<Line<'static>>,
    item: &crate::events::TranscriptItem,
    first_prefix: &'static str,
    continuation_prefix: &'static str,
    inner_width: u16,
) {
    append_wrapped_styled_text(
        lines,
        item.body.trim_end_matches('\n'),
        first_prefix,
        continuation_prefix,
        inner_width,
        Style::new().fg(item.kind.accent()),
    );
}

fn append_transcript_body(
    lines: &mut Vec<Line<'static>>,
    item: &crate::events::TranscriptItem,
    inner_width: u16,
) {
    let body = rendered_transcript_body(item);
    if body.is_empty() {
        return;
    }
    let style = match item.kind {
        TranscriptItemKind::Error => Style::new().fg(item.kind.accent()),
        TranscriptItemKind::ToolCall | TranscriptItemKind::ToolResult => Style::new().dark_gray(),
        _ => Style::new(),
    };
    append_wrapped_styled_text(lines, &body, "  └ ", "    ", inner_width, style);
}

fn rendered_transcript_body(item: &crate::events::TranscriptItem) -> String {
    match item.kind {
        TranscriptItemKind::ToolResult => match item.fold_stage {
            0 => item.body.trim_end_matches('\n').to_string(),
            1 => fold_tool_output(&item.body, 6),
            _ => fold_tool_output(&item.body, 3),
        },
        _ => item.body.trim_end_matches('\n').to_string(),
    }
}

fn fold_tool_output(body: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    if lines.len() <= max_lines {
        return body.trim_end_matches('\n').to_string();
    }

    let mut folded = lines[..max_lines].join("\n");
    folded.push_str("\n...");
    folded
}

fn title_style(kind: TranscriptItemKind) -> Style {
    match kind {
        TranscriptItemKind::ToolCall | TranscriptItemKind::ToolResult => {
            Style::new().dark_gray().add_modifier(Modifier::BOLD)
        }
        _ => Style::new().fg(kind.accent()).add_modifier(Modifier::BOLD),
    }
}

fn wrapped_line_count_with_prefix(text: &str, width: u16, prefix_width: u16) -> u16 {
    let width_limit = usize::from(width.max(1));
    let mut x = usize::from(prefix_width);
    let mut y = 0usize;

    for ch in text.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if x + char_width > width_limit {
            x = 0;
            y += 1;
        }
        x += char_width;
        if x >= width_limit {
            x = 0;
            y += 1;
        }
    }
    (y + 1) as u16
}

fn append_wrapped_title(
    lines: &mut Vec<Line<'static>>,
    title: &str,
    kind: TranscriptItemKind,
    inner_width: u16,
) {
    let prefix = "• ";
    let continuation = "  ";
    let content_width = inner_width.saturating_sub(prefix.len() as u16).max(1) as usize;
    let wrapped = textwrap::wrap(title, Options::new(content_width).break_words(false));
    for (index, segment) in wrapped.iter().enumerate() {
        let prefix_text = if index == 0 { prefix } else { continuation };
        lines.push(Line::from(vec![
            Span::styled(
                prefix_text,
                Style::new().fg(kind.accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(segment.to_string(), title_style(kind)),
        ]));
    }
}

fn append_wrapped_styled_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    first_prefix: &'static str,
    continuation_prefix: &'static str,
    inner_width: u16,
    style: Style,
) {
    let prefix_style = style.add_modifier(Modifier::BOLD);
    if text.is_empty() {
        lines.push(Line::from(vec![Span::styled(first_prefix, prefix_style)]));
        return;
    }

    let first_width = inner_width.saturating_sub(first_prefix.len() as u16).max(1) as usize;
    let continuation_width = inner_width
        .saturating_sub(continuation_prefix.len() as u16)
        .max(1) as usize;
    let mut first_visual_line = true;

    for logical_line in text.split('\n') {
        let options = if first_visual_line {
            Options::new(first_width).break_words(false)
        } else {
            Options::new(continuation_width).break_words(false)
        };
        let wrapped = textwrap::wrap(logical_line, options);
        if wrapped.is_empty() {
            let prefix = if first_visual_line {
                first_prefix
            } else {
                continuation_prefix
            };
            lines.push(Line::from(vec![Span::styled(prefix, prefix_style)]));
            first_visual_line = false;
            continue;
        }

        for segment in wrapped {
            let prefix = if first_visual_line {
                first_prefix
            } else {
                continuation_prefix
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, prefix_style),
                Span::styled(segment.to_string(), style),
            ]));
            first_visual_line = false;
        }
    }
}

fn append_wrapped_composer_line(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    inner_width: u16,
    style: Style,
) {
    let content_width = inner_width.max(1) as usize;
    let wrapped = textwrap::wrap(text, Options::new(content_width).break_words(false));
    if wrapped.is_empty() {
        lines.push(Line::from(vec![Span::styled(String::new(), style)]));
        return;
    }
    for segment in wrapped {
        lines.push(Line::from(vec![Span::styled(segment.to_string(), style)]));
    }
}

fn append_wrapped_composer_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    inner_width: u16,
    text_style: Style,
) {
    let content_width = inner_width.max(1) as usize;
    let wrapped = textwrap::wrap(text, Options::new(content_width).break_words(false));

    for segment in wrapped {
        lines.push(Line::from(vec![Span::styled(
            segment.to_string(),
            text_style,
        )]));
    }
}

fn append_wrapped_composer_session_entry(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    inner_width: u16,
    style: Style,
    title_style: Style,
) {
    let content_width = inner_width.max(1) as usize;
    let wrapped = textwrap::wrap(text, Options::new(content_width).break_words(false));
    for (index, segment) in wrapped.iter().enumerate() {
        if index == 0 {
            let marker_width = segment.find(' ').unwrap_or(0);
            let marker = &segment[..marker_width];
            let rest = segment[marker_width..].trim_start().to_string();
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::styled(" ", style),
                Span::styled(rest, title_style),
            ]));
        } else {
            lines.push(Line::from(vec![Span::styled(segment.to_string(), style)]));
        }
    }
}

pub(crate) fn composer_height(app: &TuiApp, area: Rect) -> u16 {
    let inner_width = area.width.max(1);
    let mut total = app.input.visual_line_count(inner_width);

    let suggestions = app.slash_suggestions();
    if !suggestions.is_empty() {
        total = total.saturating_add(1);
        for (index, suggestion) in suggestions.iter().enumerate() {
            let selected = index == app.slash_selection.min(suggestions.len() - 1);
            let rendered = format!(
                "  {} {}  {}",
                if selected { ">" } else { "•" },
                suggestion.name,
                suggestion.description
            );
            total = total.saturating_add(wrapped_line_count_with_prefix(&rendered, inner_width, 0));
        }
    }

    if let Some(panel) = &app.aux_panel {
        total = total.saturating_add(1);
        total = total.saturating_add(wrapped_line_count_with_prefix(
            &format!("  {}", panel.title),
            inner_width,
            0,
        ));
        match &panel.content {
            AuxPanelContent::Text(body) => {
                for line in body.lines() {
                    total = total.saturating_add(wrapped_line_count_with_prefix(
                        &format!("  {line}"),
                        inner_width,
                        0,
                    ));
                }
            }
            AuxPanelContent::SessionList(entries) => {
                if entries.is_empty() {
                    total = total.saturating_add(wrapped_line_count_with_prefix(
                        "  No saved sessions found.",
                        inner_width,
                        0,
                    ));
                }
                for (index, entry) in entries.iter().enumerate() {
                    let selected =
                        index == app.aux_panel_selection.min(entries.len().saturating_sub(1));
                    let marker = if entry.is_active { "*" } else { " " };
                    let rendered = format!(
                        "  {} {marker} {}  [{}]  {}",
                        if selected { ">" } else { "•" },
                        entry.title,
                        entry.session_id,
                        entry.updated_at
                    );
                    total = total.saturating_add(wrapped_line_count_with_prefix(
                        &rendered,
                        inner_width,
                        0,
                    ));
                }
            }
            AuxPanelContent::ModelList(entries) => {
                if entries.is_empty() {
                    total = total.saturating_add(wrapped_line_count_with_prefix(
                        "  No models available.",
                        inner_width,
                        0,
                    ));
                }
                for (index, entry) in entries.iter().enumerate() {
                    let selected =
                        index == app.aux_panel_selection.min(entries.len().saturating_sub(1));
                    let marker = if entry.is_current { "*" } else { " " };
                    let description = entry
                        .description
                        .as_deref()
                        .filter(|description| !description.trim().is_empty())
                        .unwrap_or("custom model");
                    let rendered = format!(
                        "  {} {marker} {}  [{}]  {}",
                        if selected { ">" } else { "•" },
                        entry.display_name,
                        entry.slug,
                        description
                    );
                    total = total.saturating_add(wrapped_line_count_with_prefix(
                        &rendered,
                        inner_width,
                        0,
                    ));
                }
            }
        }
        total = total.saturating_add(1);
        total = total.saturating_add(wrapped_line_count_with_prefix(
            "  press enter to choose, esc to leave",
            inner_width,
            0,
        ));
    }

    total.clamp(1, area.height.saturating_sub(1).max(1).min(16))
}

fn composer_cursor(app: &TuiApp, area: Rect) -> (u16, u16) {
    let (cursor_x, cursor_y) = app.input.visual_cursor(area.width);
    (
        area.x + cursor_x,
        area.y + cursor_y.min(area.height.saturating_sub(1)),
    )
}
