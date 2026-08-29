//! Transcript-only tool input/output rendering for the Ctrl+T overlay.
//!
//! Inline chat rendering stays compact, while this cell expands completed and
//! running tools into tool-specific input and full output sections for the
//! transcript pager.

use std::collections::HashMap;
use std::path::PathBuf;

use devo_protocol::protocol::FileChange;
use ratatui::prelude::*;
use ratatui::style::Style;
use ratatui::style::Stylize;
use serde_json::Value;

use crate::ansi_escape::ansi_escape_line;
use crate::diff_render::create_diff_summary;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::HistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::tool_result_cell::ToolResultCell;
use crate::transcript::tool_state::shell_command_from_input;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;
use textwrap::WordSplitter;

#[derive(Debug)]
pub(crate) struct ToolIoCellOptions {
    pub(crate) title_line: Option<Line<'static>>,
    pub(crate) dot_prefix: Line<'static>,
    pub(crate) subsequent_prefix: Line<'static>,
    pub(crate) output_style: Style,
    pub(crate) show_empty_ellipsis: bool,
}

#[derive(Debug)]
pub(crate) struct ToolIoCell {
    title_line: Option<Line<'static>>,
    tool_name: String,
    input: Value,
    output: Option<Value>,
    display_content: Option<String>,
    dot_prefix: Line<'static>,
    subsequent_prefix: Line<'static>,
    output_style: Style,
    show_empty_ellipsis: bool,
}

impl ToolIoCell {
    pub(crate) fn new(
        options: ToolIoCellOptions,
        tool_name: String,
        input: Value,
        output: Option<Value>,
        display_content: Option<String>,
    ) -> Self {
        Self {
            title_line: options.title_line,
            tool_name,
            input,
            output,
            display_content,
            dot_prefix: options.dot_prefix,
            subsequent_prefix: options.subsequent_prefix,
            output_style: options.output_style,
            show_empty_ellipsis: options.show_empty_ellipsis,
        }
    }

    pub(crate) fn from_text_output(
        options: ToolIoCellOptions,
        tool_name: String,
        input: Value,
        output: String,
    ) -> Self {
        Self::new(options, tool_name, input, Some(Value::String(output)), None)
    }

    fn display_output_text(&self) -> String {
        // Shell-family output may carry a trailing JSON envelope; show only the
        // real command output (covers restored sessions and raw-output paths).
        self.display_content
            .clone()
            .or_else(|| self.output.as_ref().map(value_text))
            .map(|text| crate::shell_output::strip_shell_envelope(&text))
            .unwrap_or_default()
    }

    fn legacy_cell(&self) -> ToolResultCell {
        ToolResultCell::new(
            self.title_line.clone(),
            self.display_output_text(),
            self.dot_prefix.clone(),
            self.subsequent_prefix.clone(),
            self.output_style,
            self.show_empty_ellipsis,
        )
    }

    fn transcript_body_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.extend(section_lines(
            "Input",
            tool_input_lines(&self.tool_name, &self.input),
        ));
        lines.extend(section_lines(
            "Output",
            output_lines(&self.display_output_text(), self.output_style),
        ));
        if self.show_empty_ellipsis && self.display_output_text().is_empty() {
            lines.push(Line::from("  ...").patch_style(self.output_style));
        }
        lines
    }

    fn shell_compact_display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut body: Vec<Line<'static>> = self.title_line.iter().cloned().collect();
        if let Some(command) = shell_command_from_input(&self.input) {
            let command_line = Line::from(Span::styled(command, Style::default().dim()));
            let wrapped = adaptive_wrap_lines(
                std::slice::from_ref(&command_line),
                RtOptions::new(width.max(1) as usize).word_splitter(WordSplitter::NoHyphenation),
            );
            body.extend(prefix_lines(
                wrapped,
                Span::from("  ").dim(),
                Span::from("    ").dim(),
            ));
        }
        AgentMessageCell::new_with_prefix(
            body,
            self.dot_prefix.clone(),
            self.subsequent_prefix.clone(),
            false,
        )
        .display_lines(width)
    }
}

impl HistoryCell for ToolIoCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if crate::transcript::tool_state::is_shell_tool_name(&self.tool_name) {
            return self.shell_compact_display_lines(width);
        }
        self.legacy_cell().display_lines(width)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.title_line.iter().cloned().collect::<Vec<_>>();
        lines.extend(self.transcript_body_lines());
        AgentMessageCell::new_with_prefix(
            lines,
            self.dot_prefix.clone(),
            self.subsequent_prefix.clone(),
            false,
        )
        .transcript_lines(width)
    }
}

#[derive(Debug)]
pub(crate) struct FileChangeToolIoCell {
    title_line: Option<Line<'static>>,
    tool_name: String,
    input: Value,
    changes: HashMap<PathBuf, FileChange>,
    cwd: PathBuf,
}

impl FileChangeToolIoCell {
    pub(crate) fn new(
        title_line: Option<Line<'static>>,
        tool_name: String,
        input: Value,
        changes: HashMap<PathBuf, FileChange>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            title_line,
            tool_name,
            input,
            changes,
            cwd,
        }
    }
}

impl HistoryCell for FileChangeToolIoCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        create_diff_summary(&self.changes, &self.cwd, width as usize)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.title_line.iter().cloned().collect::<Vec<_>>();
        lines.extend(section_lines(
            "Input",
            tool_input_lines(&self.tool_name, &self.input),
        ));
        lines.extend(section_lines(
            "Output",
            create_diff_summary(&self.changes, &self.cwd, width as usize),
        ));
        lines
    }
}

fn section_lines(title: &str, body: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(title.to_string()).bold()];
    if body.is_empty() {
        lines.push("  {}".dim().into());
    } else {
        lines.extend(body);
    }
    lines
}

pub(crate) fn tool_input_lines(tool_name: &str, input: &Value) -> Vec<Line<'static>> {
    match tool_name {
        "read" => fields(
            input,
            &[
                ("file", &["filePath", "path"]),
                ("offset", &["offset"]),
                ("limit", &["limit"]),
            ],
        ),
        "grep" => fields(input, &[("pattern", &["pattern"]), ("path", &["path"])]),
        "find" | "glob" => fields(input, &[("pattern", &["pattern"]), ("path", &["path"])]),
        "code_search" | "mcp__code_search__code_search" => fields(
            input,
            &[
                ("operation", &["operation"]),
                ("query", &["query"]),
                ("path", &["path", "file_path"]),
                ("line", &["line"]),
            ],
        ),
        "bash" | "shell_command" | "exec_command" => fields(
            input,
            &[
                ("command", &["cmd", "command"]),
                ("workdir", &["workdir", "cwd"]),
                ("login", &["login"]),
                ("tty", &["tty"]),
            ],
        ),
        "write_stdin" => fields(
            input,
            &[
                ("session", &["session_id"]),
                ("chars", &["chars"]),
                ("yield", &["yield_time_ms"]),
            ],
        ),
        "write" | "edit" | "apply_patch" => {
            let mut lines = fields(input, &[("file", &["path", "filePath"])]);
            lines.extend(fields(
                input,
                &[
                    ("content", &["content"]),
                    ("patch", &["patchText", "patch"]),
                    ("old", &["oldString"]),
                    ("new", &["newString"]),
                ],
            ));
            if lines.is_empty() {
                pretty_json_lines(input)
            } else {
                lines
            }
        }
        _ => pretty_json_lines(input),
    }
}

fn fields(input: &Value, specs: &[(&str, &[&str])]) -> Vec<Line<'static>> {
    specs
        .iter()
        .filter_map(|(label, keys)| {
            let value = keys.iter().find_map(|key| input.get(*key))?;
            Some(labeled_value_lines(label, value))
        })
        .flatten()
        .collect()
}

fn labeled_value_lines(label: &str, value: &Value) -> Vec<Line<'static>> {
    let text = value_text(value);
    if text.contains('\n') {
        let mut lines = vec![Line::from(format!("  {label}:"))];
        lines.extend(text.lines().map(|line| Line::from(format!("    {line}"))));
        lines
    } else {
        vec![Line::from(format!("  {label}: {text}"))]
    }
}

fn pretty_json_lines(value: &Value) -> Vec<Line<'static>> {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    text.lines()
        .map(|line| Line::from(format!("  {line}")))
        .collect()
}

fn output_lines(text: &str, style: Style) -> Vec<Line<'static>> {
    let mut lines = text
        .lines()
        .map(|line| {
            let mut line = ansi_escape_line(line);
            if style != Style::default() {
                line.spans = line
                    .spans
                    .into_iter()
                    .map(|span| span.patch_style(style))
                    .collect();
            }
            line
        })
        .collect::<Vec<_>>();
    if lines.is_empty() && !text.is_empty() {
        lines.push(Line::from(text.to_string()).patch_style(style));
    }
    lines
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "{}".to_string(),
        Value::Object(_) | Value::Array(_) => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

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

    #[test]
    fn read_input_renders_path_offset_and_limit() {
        assert_eq!(
            plain(tool_input_lines(
                "read",
                &serde_json::json!({"filePath": "src/lib.rs", "offset": 10, "limit": 20})
            )),
            vec!["  file: src/lib.rs", "  offset: 10", "  limit: 20"]
        );
    }

    #[test]
    fn search_input_renders_tool_specific_fields() {
        assert_eq!(
            plain(tool_input_lines(
                "code_search",
                &serde_json::json!({"operation": "search", "query": "ctrl t", "path": "crates"})
            )),
            vec!["  operation: search", "  query: ctrl t", "  path: crates"]
        );
    }

    #[test]
    fn write_input_keeps_multiline_content() {
        let lines = plain(tool_input_lines(
            "write",
            &serde_json::json!({"filePath": "foo.txt", "content": "one\ntwo"}),
        ));
        assert_eq!(
            lines,
            vec!["  file: foo.txt", "  content:", "    one", "    two"]
        );
    }

    #[test]
    fn unknown_tool_falls_back_to_pretty_json() {
        let lines = plain(tool_input_lines(
            "custom",
            &serde_json::json!({"alpha": 1, "beta": true}),
        ));
        assert!(lines.join("\n").contains("\"alpha\": 1"));
    }
}
