//! Restore output read capabilities from the owning session's durable history.

use std::io::BufRead;
use std::path::Path;

use devo_tools::output_store::OutputArtifact;

use crate::durable_execution::{ExecutionRecord, ExecutionReplay};
use crate::{
    InternalRecordV2, ParsedRolloutLine, RolloutLineReadError, RolloutLineV2, parse_rollout_line,
};

/// Manifests alone are not capabilities. Only committed session references,
/// including explicitly inherited references, authorize restored artifact reads.
pub fn read_output_references(path: &Path) -> anyhow::Result<Vec<OutputArtifact>> {
    let mut replay = ExecutionReplay::default();
    let mut lines = std::io::BufReader::new(std::fs::File::open(path)?)
        .lines()
        .peekable();
    while let Some(line) = lines.next() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(ParsedRolloutLine::V2(line)) => {
                if let RolloutLineV2::Internal {
                    entry:
                        InternalRecordV2::Execution {
                            record: record @ ExecutionRecord::OutputArtifacts { .. },
                        },
                    ..
                } = *line
                {
                    replay.apply(&record)?;
                }
            }
            Ok(ParsedRolloutLine::Legacy(_)) => {}
            Err(RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(replay.artifacts.into_values().collect())
}
