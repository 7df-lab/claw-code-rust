//! Wire-format text delta merge — single place for incremental vs cumulative semantics.

/// Applies one streamed text chunk, accepting incremental tokens or full
/// cumulative snapshots from the wire.
pub(crate) fn apply_stream_text_delta(existing: &mut String, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if delta.starts_with(existing.as_str()) {
        *existing = delta.to_string();
        return;
    }
    if existing.starts_with(delta) && is_shorter_cumulative_snapshot(existing, delta) {
        return;
    }
    existing.push_str(delta);
}

/// Returns true when `delta` is a shorter cumulative snapshot of `existing`.
///
/// Incremental tokens can be a prefix of the accumulated text from byte zero
/// (for example `"li"` against `"line-00-…"`) and must still append. Only
/// ignore the delta when it looks like a deliberate shorter snapshot.
fn is_shorter_cumulative_snapshot(existing: &str, delta: &str) -> bool {
    if delta.len() >= existing.len() || !existing.starts_with(delta) {
        return false;
    }
    let rest = &existing[delta.len()..];
    delta.contains('\n')
        || rest.starts_with(' ')
        || rest.starts_with('\n')
        || delta.len() * 2 >= existing.len()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn incremental_and_cumulative_chunks() {
        let mut text = String::new();
        apply_stream_text_delta(&mut text, "I");
        apply_stream_text_delta(&mut text, "'ll");
        assert_eq!(text, "I'll");
        apply_stream_text_delta(&mut text, "I'll create");
        assert_eq!(text, "I'll create");
    }

    #[test]
    fn incremental_prefix_token_appends_even_when_text_starts_with_it() {
        let mut text = "line-00-abc\n".to_string();
        apply_stream_text_delta(&mut text, "li");
        assert_eq!(text, "line-00-abc\nli");
        apply_stream_text_delta(&mut text, "ne-03\n");
        assert_eq!(text, "line-00-abc\nline-03\n");
    }

    #[test]
    fn shorter_cumulative_snapshot_is_ignored() {
        let mut text = "Hello world".to_string();
        apply_stream_text_delta(&mut text, "Hello");
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn fragmented_line_splits_preserve_full_text() {
        let mut text = String::new();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut expected = String::new();
        for index in 0..=3 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let line = format!("line-{index:02}-{seed:016x}");
            let streamed_line = format!("{line}\n");
            let split_at = 1 + (seed as usize % (streamed_line.len() - 1));
            for delta in [&streamed_line[..split_at], &streamed_line[split_at..]] {
                apply_stream_text_delta(&mut text, delta);
            }
            expected.push_str(&streamed_line);
            assert_eq!(text, expected, "text mismatch after line {index}");
        }
    }
}
