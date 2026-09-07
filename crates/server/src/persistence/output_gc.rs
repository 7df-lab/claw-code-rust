//! Reference-aware cleanup after a session rollout is deleted.

use std::collections::HashSet;

use devo_core::tools::output_store::{OutputArtifact, OutputStore};

impl super::RolloutStore {
    pub(super) fn collect_unreferenced_outputs(
        &self,
        candidates: &[OutputArtifact],
    ) -> anyhow::Result<()> {
        let mut referenced = HashSet::new();
        for path in self.rollout_paths()? {
            // On an unreadable rollout keep everything: absence of readable
            // evidence must not delete a fork's still-owned output.
            referenced.extend(
                devo_core::output_replay::read_output_references(&path)?
                    .into_iter()
                    .map(|artifact| artifact.path),
            );
        }
        for artifact in candidates {
            if !referenced.contains(&artifact.path) {
                OutputStore::delete_registered_artifact(artifact)?;
            }
        }
        Ok(())
    }
}
