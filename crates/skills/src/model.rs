//! Shared skill metadata and load-result types used by discovery, rendering, and injection.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

/// Discovery scope for a skill root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// Skills discovered from the active repository.
    Repo,
    /// Skills installed for the current user.
    User,
    /// Skills shipped with Devo.
    System,
    /// Skills provided by administrator-level configuration.
    Admin,
    /// Skills contributed by an installed plugin.
    Plugin,
}

/// Metadata describing one discovered skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub interface: Option<SkillInterface>,
    pub dependencies: Option<SkillDependencies>,
    pub policy: Option<SkillPolicy>,
    pub path_to_skills_md: PathBuf,
    pub scope: SkillScope,
    pub plugin_id: Option<String>,
}

impl SkillMetadata {
    pub fn allows_implicit_invocation(&self) -> bool {
        self.policy
            .as_ref()
            .and_then(|policy| policy.allow_implicit_invocation)
            .unwrap_or(true)
    }
}

/// Optional policy data from `agents/openai.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkillPolicy {
    pub allow_implicit_invocation: Option<bool>,
    pub products: Vec<String>,
}

/// Optional UI-facing metadata from `agents/openai.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
    pub icon_small: Option<PathBuf>,
    pub icon_large: Option<PathBuf>,
    pub brand_color: Option<String>,
    pub default_prompt: Option<String>,
}

/// Optional tool dependency metadata from `agents/openai.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDependencies {
    pub tools: Vec<SkillToolDependency>,
}

/// One tool dependency declared by a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillToolDependency {
    pub r#type: String,
    pub value: String,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
}

/// Non-fatal discovery diagnostic for one skill path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillError {
    pub path: PathBuf,
    pub message: String,
}

/// Complete discovery output for a cwd/config pair.
#[derive(Debug, Clone, Default)]
pub struct SkillLoadOutcome {
    pub skills: Vec<SkillMetadata>,
    pub errors: Vec<SkillError>,
    pub disabled_paths: HashSet<PathBuf>,
    pub skill_roots: Vec<PathBuf>,
    pub skill_root_by_path: Arc<HashMap<PathBuf, PathBuf>>,
    pub implicit_skills_by_scripts_dir: Arc<HashMap<PathBuf, SkillMetadata>>,
    pub implicit_skills_by_doc_path: Arc<HashMap<PathBuf, SkillMetadata>>,
}

impl SkillLoadOutcome {
    pub fn is_skill_enabled(&self, skill: &SkillMetadata) -> bool {
        !path_set_contains_identity(&self.disabled_paths, &skill.path_to_skills_md)
    }

    pub fn is_skill_allowed_for_implicit_invocation(&self, skill: &SkillMetadata) -> bool {
        self.is_skill_enabled(skill) && skill.allows_implicit_invocation()
    }

    pub fn allowed_skills_for_implicit_invocation(&self) -> Vec<SkillMetadata> {
        self.skills
            .iter()
            .filter(|skill| self.is_skill_allowed_for_implicit_invocation(skill))
            .cloned()
            .collect()
    }

    pub fn skills_with_enabled(&self) -> impl Iterator<Item = (&SkillMetadata, bool)> {
        self.skills
            .iter()
            .map(|skill| (skill, self.is_skill_enabled(skill)))
    }
}

pub(crate) fn paths_equal(left: &Path, right: &Path) -> bool {
    canonicalize_for_identity(left) == canonicalize_for_identity(right)
}

pub(crate) fn path_set_contains_identity(paths: &HashSet<PathBuf>, path: &Path) -> bool {
    if paths.is_empty() {
        return false;
    }
    if paths.contains(path) {
        return true;
    }
    // Loaded skill paths and disabled-path rules are canonicalized up front, so
    // the direct lookup is the normal path. Keep the slower identity fallback for
    // manually constructed `SkillMetadata` values that may contain `..` or other
    // platform-specific aliases.
    paths.iter().any(|candidate| paths_equal(candidate, path))
}

pub fn canonicalize_for_identity(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    normalize_native_path(canonical)
}

pub fn normalize_native_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let normalized = path
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .map_or_else(|| path.to_string_lossy().into_owned(), ToOwned::to_owned);
        PathBuf::from(normalized)
    }

    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;

    use super::*;

    fn skill(path_to_skills_md: PathBuf) -> SkillMetadata {
        SkillMetadata {
            name: "example".to_string(),
            description: "Example skill".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md,
            scope: SkillScope::User,
            plugin_id: None,
        }
    }

    #[test]
    fn skill_enabled_uses_canonical_identity_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join("skills/example");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, "").expect("skill file");
        let disabled_path = canonicalize_for_identity(&skill_path);
        let equivalent_path = skill_dir.join("../example/SKILL.md");
        let outcome = SkillLoadOutcome {
            disabled_paths: HashSet::from([disabled_path]),
            ..SkillLoadOutcome::default()
        };

        assert!(!outcome.is_skill_enabled(&skill(equivalent_path)));
    }
}
