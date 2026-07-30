#![allow(dead_code)]

use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use devo_core::ParsedRolloutLine;
use devo_core::RolloutLine;
use devo_core::V2InverseProjector;
use devo_core::parse_rollout_line;

/// Reads a rollout file that may freely mix legacy (v1) and v2 lines into
/// the legacy line stream the replay pipeline consumes. Dual-read mirror of
/// the server's `load_session_from_rollout` for tests.
pub fn read_rollout_lines_dual(path: &Path) -> Result<Vec<RolloutLine>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read rollout {}", path.display()))?;
    let inverse = V2InverseProjector::new();
    let mut out = Vec::new();
    for raw in text.lines().filter(|line| !line.trim().is_empty()) {
        match parse_rollout_line(raw)
            .with_context(|| format!("parse line in {}", path.display()))?
        {
            ParsedRolloutLine::Legacy(line) => out.push(*line),
            ParsedRolloutLine::V2(line) => out.extend(inverse.project_line(&line)?),
        }
    }
    Ok(out)
}
