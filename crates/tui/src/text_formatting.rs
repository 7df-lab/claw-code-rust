use serde::Serialize;
use serde_json::ser::Formatter;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

pub(crate) fn capitalize_first(input: &str) -> String {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) => {
            let mut capitalized = first.to_uppercase().collect::<String>();
            capitalized.push_str(chars.as_str());
            capitalized
        }
        None => String::new(),
    }
}

/// Truncate a tool result to fit within the given height and width. If the text is valid JSON, we format it in a compact way before truncating.
/// This is a best-effort approach that may not work perfectly for text where 1 grapheme is rendered as multiple terminal cells.
pub(crate) fn format_and_truncate_tool_result(
    text: &str,
    max_lines: usize,
    line_width: usize,
) -> String {
    // Work out the maximum number of graphemes we can display for a result.
    // It's not guaranteed that 1 grapheme = 1 cell, so we subtract 1 per line as a fudge factor.
    // It also won't handle future terminal resizes properly, but it's an OK approximation for now.
    let max_graphemes = (max_lines * line_width).saturating_sub(max_lines);

    if let Some(formatted_json) = format_json_compact(text) {
        truncate_text(&formatted_json, max_graphemes)
    } else {
        truncate_text(text, max_graphemes)
    }
}

/// Format JSON text in a compact single-line format with spaces for better Ratatui wrapping.
/// Ex: `{"a":"b",c:["d","e"]}` -> `{"a": "b", "c": ["d", "e"]}`
/// Returns the formatted JSON string if the input is valid JSON, otherwise returns None.
/// This is a little complicated, but it's necessary because Ratatui's wrapping is *very* limited
/// and can only do line breaks at whitespace. If we use the default serde_json format, we get lines
/// without spaces that Ratatui can't wrap nicely. If we use the serde_json pretty format as-is,
/// it's much too sparse and uses too many terminal rows.
/// Relevant issue: https://github.com/ratatui/ratatui/issues/293
pub(crate) fn format_json_compact(text: &str) -> Option<String> {
    let json = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let mut result = Vec::with_capacity(text.len() + text.len() / 8);
    {
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut result, SpacedCompactFormatter);
        json.serialize(&mut serializer).ok()?;
    }

    String::from_utf8(result).ok()
}

struct SpacedCompactFormatter;

impl Formatter for SpacedCompactFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        writer.write_all(b": ")
    }
}

/// Truncate `text` to `max_graphemes` graphemes. Using graphemes to avoid accidentally truncating in the middle of a multi-codepoint character.
pub(crate) fn truncate_text(text: &str, max_graphemes: usize) -> String {
    if max_graphemes >= text.len() {
        return text.to_string();
    }

    let mut graphemes = text.grapheme_indices(true);

    // Check if there's a grapheme at position max_graphemes (meaning there are more than max_graphemes total)
    if let Some((byte_index, _)) = graphemes.nth(max_graphemes) {
        // There are more than max_graphemes, so we need to truncate
        if max_graphemes >= 3 {
            // Truncate to max_graphemes - 3 and add "..." to stay within limit
            let mut truncate_graphemes = text.grapheme_indices(true);
            if let Some((truncate_byte_index, _)) = truncate_graphemes.nth(max_graphemes - 3) {
                let truncated = &text[..truncate_byte_index];
                format!("{truncated}...")
            } else {
                text.to_string()
            }
        } else {
            // max_graphemes < 3, so just return first max_graphemes without "..."
            let truncated = &text[..byte_index];
            truncated.to_string()
        }
    } else {
        // There are max_graphemes or fewer graphemes, return original text
        text.to_string()
    }
}

/// Truncate a path-like string to the given display width, keeping leading and trailing segments
/// where possible and inserting a single Unicode ellipsis between them. If an individual segment
/// cannot fit, it is front-truncated with an ellipsis.
pub(crate) fn center_truncate_path(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(path) <= max_width {
        return path.to_string();
    }

    let sep = std::path::MAIN_SEPARATOR;
    let has_leading_sep = path.starts_with(sep);
    let has_trailing_sep = path.ends_with(sep);
    let mut raw_segments: Vec<&str> = path.split(sep).collect();
    if has_leading_sep && !raw_segments.is_empty() && raw_segments[0].is_empty() {
        raw_segments.remove(0);
    }
    if has_trailing_sep
        && !raw_segments.is_empty()
        && raw_segments.last().is_some_and(|last| last.is_empty())
    {
        raw_segments.pop();
    }

    if raw_segments.is_empty() {
        if has_leading_sep {
            let root = sep.to_string();
            if UnicodeWidthStr::width(root.as_str()) <= max_width {
                return root;
            }
        }
        return "…".to_string();
    }

    struct Segment<'a> {
        original: &'a str,
        text: String,
        truncatable: bool,
        is_suffix: bool,
    }

    let assemble = |leading: bool, segments: &[Segment<'_>]| -> String {
        let mut result = String::new();
        if leading {
            result.push(sep);
        }
        for segment in segments {
            if !result.is_empty() && !result.ends_with(sep) {
                result.push(sep);
            }
            result.push_str(segment.text.as_str());
        }
        result
    };

    let front_truncate = |original: &str, allowed_width: usize| -> String {
        if allowed_width == 0 {
            return String::new();
        }
        if UnicodeWidthStr::width(original) <= allowed_width {
            return original.to_string();
        }
        if allowed_width == 1 {
            return "…".to_string();
        }

        let mut kept: Vec<char> = Vec::new();
        let mut used_width = 1; // reserve space for leading ellipsis
        for ch in original.chars().rev() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used_width + ch_width > allowed_width {
                break;
            }
            used_width += ch_width;
            kept.push(ch);
        }
        kept.reverse();
        let mut truncated = String::from("…");
        for ch in kept {
            truncated.push(ch);
        }
        truncated
    };

    let mut combos: Vec<(usize, usize)> = Vec::new();
    let segment_count = raw_segments.len();
    for left in 1..=segment_count {
        let min_right = if left == segment_count { 0 } else { 1 };
        for right in min_right..=(segment_count - left) {
            combos.push((left, right));
        }
    }
    let desired_suffix = if segment_count > 1 {
        std::cmp::min(2, segment_count - 1)
    } else {
        0
    };
    let mut prioritized: Vec<(usize, usize)> = Vec::new();
    let mut fallback: Vec<(usize, usize)> = Vec::new();
    for combo in combos {
        if combo.1 >= desired_suffix {
            prioritized.push(combo);
        } else {
            fallback.push(combo);
        }
    }
    let sort_combos = |items: &mut Vec<(usize, usize)>| {
        items.sort_by(|(left_a, right_a), (left_b, right_b)| {
            left_b
                .cmp(left_a)
                .then_with(|| right_b.cmp(right_a))
                .then_with(|| (left_b + right_b).cmp(&(left_a + right_a)))
        });
    };
    sort_combos(&mut prioritized);
    sort_combos(&mut fallback);

    let fit_segments =
        |segments: &mut Vec<Segment<'_>>, allow_front_truncate: bool| -> Option<String> {
            loop {
                let candidate = assemble(has_leading_sep, segments);
                let width = UnicodeWidthStr::width(candidate.as_str());
                if width <= max_width {
                    return Some(candidate);
                }

                if !allow_front_truncate {
                    return None;
                }

                let mut indices: Vec<usize> = Vec::new();
                for (idx, seg) in segments.iter().enumerate().rev() {
                    if seg.truncatable && seg.is_suffix {
                        indices.push(idx);
                    }
                }
                for (idx, seg) in segments.iter().enumerate().rev() {
                    if seg.truncatable && !seg.is_suffix {
                        indices.push(idx);
                    }
                }

                if indices.is_empty() {
                    return None;
                }

                let mut changed = false;
                for idx in indices {
                    let original_width = UnicodeWidthStr::width(segments[idx].original);
                    if original_width <= max_width && segment_count > 2 {
                        continue;
                    }
                    let seg_width = UnicodeWidthStr::width(segments[idx].text.as_str());
                    let other_width = width.saturating_sub(seg_width);
                    let allowed_width = max_width.saturating_sub(other_width).max(1);
                    let new_text = front_truncate(segments[idx].original, allowed_width);
                    if new_text != segments[idx].text {
                        segments[idx].text = new_text;
                        changed = true;
                        break;
                    }
                }

                if !changed {
                    return None;
                }
            }
        };

    for (left_count, right_count) in prioritized.into_iter().chain(fallback) {
        let mut segments: Vec<Segment<'_>> = raw_segments[..left_count]
            .iter()
            .map(|seg| Segment {
                original: seg,
                text: (*seg).to_string(),
                truncatable: true,
                is_suffix: false,
            })
            .collect();

        let need_ellipsis = left_count + right_count < segment_count;
        if need_ellipsis {
            segments.push(Segment {
                original: "…",
                text: "…".to_string(),
                truncatable: false,
                is_suffix: false,
            });
        }

        if right_count > 0 {
            segments.extend(
                raw_segments[segment_count - right_count..]
                    .iter()
                    .map(|seg| Segment {
                        original: seg,
                        text: (*seg).to_string(),
                        truncatable: true,
                        is_suffix: true,
                    }),
            );
        }

        let allow_front_truncate = need_ellipsis || segment_count <= 2;
        if let Some(candidate) = fit_segments(&mut segments, allow_front_truncate) {
            return candidate;
        }
    }

    front_truncate(path, max_width)
}

/// Join a list of strings with proper English punctuation.
/// Examples:
/// - [] -> ""
/// - ["apple"] -> "apple"
/// - ["apple", "banana"] -> "apple and banana"
/// - ["apple", "banana", "cherry"] -> "apple, banana and cherry"
pub(crate) fn proper_join<T: AsRef<str>>(items: &[T]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].as_ref().to_string(),
        2 => format!("{} and {}", items[0].as_ref(), items[1].as_ref()),
        _ => {
            let last = items[items.len() - 1].as_ref();
            let mut result = String::new();

            for (i, item) in items.iter().take(items.len() - 1).enumerate() {
                if i > 0 {
                    result.push_str(", ");
                }
                result.push_str(item.as_ref());
            }

            format!("{result} and {last}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn truncate_text_keeps_short_text() {
        assert_eq!(truncate_text("short text", 20), "short text");
    }

    #[test]
    fn truncate_text_uses_ellipsis_when_limit_allows_it() {
        assert_eq!(truncate_text("abcdef", 5), "ab...");
    }

    #[test]
    fn truncate_text_preserves_grapheme_boundaries() {
        assert_eq!(truncate_text("a👨‍👩‍👧‍👦b", 2), "a👨‍👩‍👧‍👦");
    }

    #[test]
    fn format_json_compact_formats_valid_json_on_one_line() {
        let actual = format_json_compact(r#"{"a":"b","c":["d","e"],"empty":[]}"#);
        assert_eq!(
            actual,
            Some(r#"{"a": "b", "c": ["d", "e"], "empty": []}"#.to_string())
        );
    }

    #[test]
    fn format_json_compact_preserves_string_punctuation() {
        let actual = format_json_compact(r#"{"text":"a:b,c","quote":"\"x,y\""}"#);
        let actual = actual
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        let expected = serde_json::json!({"text": "a:b,c", "quote": "\"x,y\""});
        assert_eq!(actual, Some(expected));
    }

    #[test]
    fn format_json_compact_preserves_unicode_string_contents() {
        let actual = format_json_compact(r#"{"text":"路径:值,emoji👨‍👩‍👧‍👦"}"#);
        assert_eq!(actual, Some(r#"{"text": "路径:值,emoji👨‍👩‍👧‍👦"}"#.to_string()));
    }

    #[test]
    fn format_json_compact_rejects_invalid_json() {
        assert_eq!(format_json_compact("not json"), None);
    }

    #[test]
    #[ignore]
    fn bench_truncate_text_ascii_no_truncation() {
        let text = "search result display name";

        let started = Instant::now();
        let mut total_len = 0;
        for _ in 0..500_000 {
            total_len += black_box(truncate_text(black_box(text), black_box(80))).len();
        }
        let elapsed = started.elapsed();

        assert_eq!(total_len, 13_000_000);
        println!(
            "truncate_text_ascii_no_truncation iterations=500000 bytes={} max_graphemes=80 elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 500_000.0
        );
    }

    #[test]
    #[ignore]
    fn bench_truncate_text_unicode_truncates() {
        let text = "路径/模块/文件/👨‍👩‍👧‍👦/very-long-display-name".repeat(20);

        let started = Instant::now();
        let mut total_len = 0;
        for _ in 0..50_000 {
            total_len += black_box(truncate_text(black_box(&text), black_box(80))).len();
        }
        let elapsed = started.elapsed();

        assert!(total_len > 0);
        println!(
            "truncate_text_unicode_truncates iterations=50000 bytes={} max_graphemes=80 elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 50_000.0
        );
    }

    #[test]
    #[ignore]
    fn bench_format_json_compact_nested_tool_result() {
        let text = serde_json::json!({
            "results": (0..64)
                .map(|index| {
                    serde_json::json!({
                        "path": format!("crates/tui/src/module_{index}/file.rs"),
                        "line": index * 7,
                        "preview": format!("function_{index}(\"value: {index}, next\")"),
                        "matches": [
                            { "start": index, "end": index + 4 },
                            { "start": index + 12, "end": index + 20 }
                        ]
                    })
                })
                .collect::<Vec<_>>(),
            "summary": {
                "query": "value: needle, path",
                "elapsed_ms": 42,
                "truncated": false
            }
        })
        .to_string();
        let expected_len = format_json_compact(&text)
            .expect("benchmark JSON should format")
            .len();

        let started = Instant::now();
        let mut total_len = 0;
        for _ in 0..5_000 {
            total_len += black_box(format_json_compact(black_box(&text)))
                .expect("benchmark JSON should format")
                .len();
        }
        let elapsed = started.elapsed();

        assert_eq!(total_len, expected_len * 5_000);
        println!(
            "format_json_compact_nested_tool_result iterations=5000 bytes={} elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 5_000.0
        );
    }
}
