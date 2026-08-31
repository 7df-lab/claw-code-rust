//! Derives projector tool state from structured tool facts (name + input).

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;

use super::model::ToolPhase;

pub(crate) fn is_streaming_param_tool(tool_name: &str) -> bool {
    matches!(tool_name, "write" | "edit" | "apply_patch")
}

pub(crate) fn input_is_incomplete(input: &serde_json::Value) -> bool {
    input.is_null() || matches!(input, serde_json::Value::Object(map) if map.is_empty())
}

pub(crate) fn initial_phase(tool_name: &str, input: &serde_json::Value) -> ToolPhase {
    if is_streaming_param_tool(tool_name) && input_is_incomplete(input) {
        ToolPhase::Preparing
    } else {
        ToolPhase::Running
    }
}

pub(crate) fn is_exec_like(parsed_commands: &[ParsedCommand]) -> bool {
    !parsed_commands.is_empty()
        && parsed_commands
            .iter()
            .all(|parsed| !matches!(parsed, ParsedCommand::Unknown { .. }))
}

pub(crate) fn shell_command_from_input(input: &serde_json::Value) -> Option<String> {
    input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub(crate) fn shell_description_from_input(input: Option<&serde_json::Value>) -> Option<String> {
    input
        .and_then(|value| value.get("description"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_shell_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "bash" | "shell_command" | "exec_command" | "write_stdin" | "shell"
    )
}

pub(crate) fn command_source_from_tool_name(tool_name: &str) -> Option<ExecCommandSource> {
    match tool_name {
        "bash" | "shell_command" | "exec_command" | "write_stdin" => Some(ExecCommandSource::Agent),
        _ => None,
    }
}

/// Extracts the members of a partially streamed tool-call JSON object.
///
/// Providers stream tool arguments as a JSON string in fragments; waiting for
/// the complete parse leaves the running row without any parameters for the
/// whole streaming window. This scanner collects only members whose value has
/// fully arrived (closing quote seen, or a scalar followed by a separator) and
/// skips nested objects/arrays — display fields (command, filePath, pattern,
/// …) are always flat strings or numbers. The result is display-only: the
/// authoritative input arrives with the item refresh or the tool result.
pub(crate) fn partial_object_members(partial: &str) -> Option<serde_json::Value> {
    let mut map = serde_json::Map::new();
    let mut chars = partial.char_indices().peekable();

    // Consume the opening brace; anything else is not a tool-argument object.
    skip_json_whitespace(&mut chars);
    if chars.peek().map(|(_, ch)| *ch) != Some('{') {
        return None;
    }
    chars.next();

    loop {
        skip_json_whitespace(&mut chars);
        match chars.peek() {
            None => break,
            Some((_, '}')) => break,
            Some((_, ',')) => {
                chars.next();
                continue;
            }
            Some((_, '"')) => {}
            Some(&(_, _)) => break,
        }
        // Key string.
        chars.next();
        let Some(key) = scan_json_string(&mut chars) else {
            break;
        };
        skip_json_whitespace(&mut chars);
        if chars.next().map(|(_, ch)| ch) != Some(':') {
            break;
        }
        skip_json_whitespace(&mut chars);
        let Some((_, value_start)) = chars.peek().copied() else {
            break;
        };
        match value_start {
            '"' => {
                chars.next();
                let Some(value) = scan_json_string(&mut chars) else {
                    break;
                };
                map.insert(key, serde_json::Value::String(value));
            }
            '{' | '[' => {
                // Nested values are not display fields; skip to their close.
                let (open, close) = if value_start == '{' {
                    ('{', '}')
                } else {
                    ('[', ']')
                };
                chars.next();
                if !skip_nested_value(&mut chars, open, close) {
                    break;
                }
            }
            _ => {
                // Scalar (number/bool/null): only complete when a separator
                // follows — a trailing partial number must not be recorded.
                let mut literal = String::new();
                let mut complete = false;
                while let Some(&(_, ch)) = chars.peek() {
                    if ch == ',' || ch == '}' {
                        complete = true;
                        break;
                    }
                    literal.push(ch);
                    chars.next();
                }
                if !complete {
                    break;
                }
                let literal = literal.trim().to_string();
                let value = if literal == "true" {
                    serde_json::Value::Bool(true)
                } else if literal == "false" {
                    serde_json::Value::Bool(false)
                } else if literal == "null" {
                    serde_json::Value::Null
                } else {
                    match literal.parse::<f64>() {
                        Ok(number) => serde_json::Value::from(number),
                        Err(_) => break,
                    }
                };
                map.insert(key, value);
            }
        }
    }

    (!map.is_empty()).then_some(serde_json::Value::Object(map))
}

/// Reads a JSON string body starting just after the opening quote; returns
/// `None` when the closing quote has not arrived yet.
fn scan_json_string(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Option<String> {
    let mut value = String::new();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next() {
                Some((_, '"')) => value.push('"'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, '/')) => value.push('/'),
                Some((_, 'n')) => value.push('\n'),
                Some((_, 't')) => value.push('\t'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, 'b')) => value.push('\u{8}'),
                Some((_, 'f')) => value.push('\u{c}'),
                Some((_, 'u')) => {
                    let mut code = String::new();
                    for _ in 0..4 {
                        match chars.next() {
                            Some((_, hex)) => code.push(hex),
                            None => return None,
                        }
                    }
                    match u32::from_str_radix(&code, 16).ok().and_then(char::from_u32) {
                        Some(decoded) => value.push(decoded),
                        None => return None,
                    }
                }
                _ => return None,
            },
            _ => value.push(ch),
        }
    }
    None
}

/// Skips a nested object/array value; returns `false` when it is still
/// incomplete.
fn skip_nested_value(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    open: char,
    close: char,
) -> bool {
    let mut depth = 1usize;
    while let Some(&(_, ch)) = chars.peek() {
        if ch == '"' {
            chars.next();
            if scan_json_string(chars).is_none() {
                return false;
            }
            continue;
        }
        chars.next();
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
    }
    false
}

fn skip_json_whitespace(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    while matches!(
        chars.peek().map(|(_, ch)| *ch),
        Some(' ' | '\t' | '\n' | '\r')
    ) {
        chars.next();
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn partial_members_collect_completed_string_values_only() {
        let partial = r#"{"filePath": "src/lib.rs", "offset": 12"#;
        assert_eq!(
            partial_object_members(partial),
            Some(serde_json::json!({ "filePath": "src/lib.rs" })),
        );
    }

    #[test]
    fn partial_members_skip_incomplete_trailing_string() {
        let partial = r#"{"pattern": "fn main", "path": "crate"#;
        assert_eq!(
            partial_object_members(partial),
            Some(serde_json::json!({ "pattern": "fn main" })),
        );
    }

    #[test]
    fn partial_members_record_number_only_after_separator() {
        // The trailing `40` has no separator yet — it may still grow, so only
        // the offset is displayable.
        let streaming = r#"{"offset": 12, "limit": 40"#;
        assert_eq!(
            partial_object_members(streaming),
            Some(serde_json::json!({ "offset": 12.0 })),
        );
        // With a separator after it, both members are complete.
        let complete = r#"{"offset": 12, "limit": 40,"#;
        assert_eq!(
            partial_object_members(complete),
            Some(serde_json::json!({ "offset": 12.0, "limit": 40.0 })),
        );
        // The trailing number may still grow — must not be recorded.
        let growing = r#"{"offset": 1"#;
        assert_eq!(partial_object_members(growing), None);
    }

    #[test]
    fn partial_members_skip_nested_values() {
        let partial = r#"{"filePath": "a.rs", "edits": [{"old": "x""#;
        assert_eq!(
            partial_object_members(partial),
            Some(serde_json::json!({ "filePath": "a.rs" })),
        );
    }

    #[test]
    fn partial_members_handle_escapes_and_empty_input() {
        let partial = r#"{"command": "echo \"hi\"", "description": "Say hi""#;
        assert_eq!(
            partial_object_members(partial),
            Some(serde_json::json!({
                "command": "echo \"hi\"",
                "description": "Say hi",
            })),
        );
        assert_eq!(partial_object_members(""), None);
        assert_eq!(partial_object_members(r#"{ "filePa"#), None);
    }
}
