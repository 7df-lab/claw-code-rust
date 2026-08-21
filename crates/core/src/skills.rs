//! Core-facing skill catalog wrapper backed by `devo-skills`.

use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub use devo_skills::SkillDependencies;
pub use devo_skills::SkillInterface;
pub use devo_skills::SkillScope;
pub use devo_skills::SkillsManager;
pub use devo_skills::SkillsRuntimeConfig;
pub use devo_skills::build_available_skills;
pub use devo_skills::build_skill_injections;
pub use devo_skills::collect_explicit_skill_mentions;
pub use devo_skills::default_skill_metadata_budget;
pub use devo_skills::normalize_native_path;
pub use devo_skills::render_available_skills_body;

use crate::SkillsConfig;

/// Strongly typed legacy name identifier for a discovered skill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillId(pub SmolStr);

/// Stores metadata for one discovered skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub interface: Option<SkillInterface>,
    pub dependencies: Option<SkillDependencies>,
    pub path: PathBuf,
    pub enabled: bool,
    pub source: SkillSource,
    pub scope: SkillScope,
    pub plugin_id: Option<String>,
}

/// Identifies where a discovered skill came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    User,
    Workspace { cwd: PathBuf },
    Plugin { plugin_id: String },
    System,
    Admin,
}

/// Carries the skill content injected into a turn after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSkill {
    pub record: SkillRecord,
    pub content: String,
}

/// Selects a skill by exact path or by legacy unambiguous name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSelector {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// Provides discovery and lookup operations for skills.
pub trait SkillCatalog {
    /// Discovers skills for an optional workspace root.
    fn discover(
        &mut self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> Result<Vec<SkillRecord>, SkillError>;

    /// Loads the content for one discovered skill.
    fn load(
        &mut self,
        selector: &SkillSelector,
        workspace_root: Option<&Path>,
    ) -> Result<ResolvedSkill, SkillError>;

    /// Returns whether model-visible available-skills instructions are enabled.
    fn include_instructions(&self) -> bool;

    /// Renders the available skills instructions block for one workspace.
    fn available_skills_instructions(
        &mut self,
        workspace_root: Option<&Path>,
        context_window: Option<i64>,
    ) -> Result<Option<String>, SkillError>;

    /// Refreshes skill runtime configuration and clears cached discovery results.
    fn set_config(&mut self, config: SkillsConfig, project_root_markers: Vec<String>);
}

/// Filesystem-backed implementation of `SkillCatalog`.
#[derive(Debug)]
pub struct FileSystemSkillCatalog {
    manager: SkillsManager,
    default_workspace_root: PathBuf,
}

impl FileSystemSkillCatalog {
    pub fn new(config: SkillsConfig) -> Self {
        let devo_home = devo_util_paths::find_devo_home()
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::with_devo_home(config, devo_home, cwd, vec![".git".to_string()])
    }

    pub fn with_devo_home(
        config: SkillsConfig,
        devo_home: PathBuf,
        default_workspace_root: PathBuf,
        project_root_markers: Vec<String>,
    ) -> Self {
        let runtime_config = skills_runtime_config(config, project_root_markers);
        Self {
            manager: SkillsManager::new(devo_home, runtime_config),
            default_workspace_root,
        }
    }

    fn outcome(
        &self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> devo_skills::SkillLoadOutcome {
        let root = workspace_root.unwrap_or(self.default_workspace_root.as_path());
        self.manager.skills_for_cwd(root, force_reload)
    }

    fn record_from_skill(
        skill: &devo_skills::SkillMetadata,
        enabled: bool,
        workspace_root: Option<&Path>,
    ) -> SkillRecord {
        SkillRecord {
            id: SkillId(skill.name.clone().into()),
            name: skill.name.clone(),
            description: skill.description.clone(),
            short_description: skill.short_description.clone(),
            interface: skill.interface.clone(),
            dependencies: skill.dependencies.clone(),
            path: normalize_native_path(skill.path_to_skills_md.clone()),
            enabled,
            source: skill_source(skill, workspace_root),
            scope: skill.scope,
            plugin_id: skill.plugin_id.clone(),
        }
    }

    fn find_skill(
        outcome: &devo_skills::SkillLoadOutcome,
        selector: &SkillSelector,
    ) -> Result<devo_skills::SkillMetadata, SkillError> {
        if let Some(path) = selector
            .path
            .as_ref()
            .filter(|path| !path.as_os_str().is_empty())
        {
            let path = devo_skills::model::canonicalize_for_identity(path);
            return outcome
                .skills
                .iter()
                .find(|skill| skill.path_to_skills_md == path)
                .cloned()
                .ok_or_else(|| SkillError::SkillNotFound {
                    name: selector.name.clone(),
                    path: Some(path),
                });
        }

        let matches = outcome
            .skills
            .iter()
            .filter(|skill| skill.name == selector.name)
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [skill] => Ok(skill.clone()),
            [] => Err(SkillError::SkillNotFound {
                name: selector.name.clone(),
                path: None,
            }),
            _ => Err(SkillError::AmbiguousSkillName {
                name: selector.name.clone(),
                paths: matches
                    .iter()
                    .map(|skill| skill.path_to_skills_md.clone())
                    .collect(),
            }),
        }
    }
}

impl SkillCatalog for FileSystemSkillCatalog {
    fn discover(
        &mut self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        let outcome = self.outcome(workspace_root, force_reload);
        if let Some(error) = outcome.errors.first() {
            tracing::warn!(
                path = %error.path.display(),
                message = %error.message,
                "skill discovery warning"
            );
        }
        Ok(outcome
            .skills_with_enabled()
            .map(|(skill, enabled)| Self::record_from_skill(skill, enabled, workspace_root))
            .collect())
    }

    fn load(
        &mut self,
        selector: &SkillSelector,
        workspace_root: Option<&Path>,
    ) -> Result<ResolvedSkill, SkillError> {
        let outcome = self.outcome(workspace_root, false);
        let skill = Self::find_skill(&outcome, selector)?;
        if !outcome.is_skill_enabled(&skill) {
            return Err(SkillError::SkillDisabled {
                name: skill.name,
                path: skill.path_to_skills_md,
            });
        }
        let content = std::fs::read_to_string(&skill.path_to_skills_md).map_err(|source| {
            SkillError::SkillParseFailed {
                path: skill.path_to_skills_md.clone(),
                message: source.to_string(),
            }
        })?;
        Ok(ResolvedSkill {
            record: Self::record_from_skill(&skill, true, workspace_root),
            content: strip_frontmatter(&content)
                .trim_start_matches(['\r', '\n'])
                .to_string(),
        })
    }

    fn include_instructions(&self) -> bool {
        self.manager.include_instructions()
    }

    fn available_skills_instructions(
        &mut self,
        workspace_root: Option<&Path>,
        context_window: Option<i64>,
    ) -> Result<Option<String>, SkillError> {
        if !self.include_instructions() {
            return Ok(None);
        }
        let outcome = self.outcome(workspace_root, false);
        let Some(available) =
            build_available_skills(&outcome, default_skill_metadata_budget(context_window))
        else {
            return Ok(None);
        };
        Ok(Some(render_available_skills_body(
            &available.skill_root_lines,
            &available.skill_lines,
        )))
    }

    fn set_config(&mut self, config: SkillsConfig, project_root_markers: Vec<String>) {
        self.manager
            .set_config(skills_runtime_config(config, project_root_markers));
    }
}

fn skills_runtime_config(
    config: SkillsConfig,
    project_root_markers: Vec<String>,
) -> SkillsRuntimeConfig {
    SkillsRuntimeConfig {
        enabled: config.enabled,
        user_roots: config.user_roots,
        workspace_roots: config.workspace_roots,
        include_instructions: config.include_instructions.unwrap_or(true),
        bundled_enabled: config.bundled.unwrap_or_default().enabled,
        config_rules: devo_skills::config_rules::SkillConfigRules::from_entries(
            config
                .config
                .into_iter()
                .map(|entry| (entry.path, entry.name, entry.enabled)),
        ),
        project_root_markers,
    }
}

fn skill_source(skill: &devo_skills::SkillMetadata, workspace_root: Option<&Path>) -> SkillSource {
    match skill.scope {
        SkillScope::Repo => SkillSource::Workspace {
            cwd: workspace_root.map(Path::to_path_buf).unwrap_or_default(),
        },
        SkillScope::User => SkillSource::User,
        SkillScope::System => SkillSource::System,
        SkillScope::Admin => SkillSource::Admin,
        SkillScope::Plugin => SkillSource::Plugin {
            plugin_id: skill.plugin_id.clone().unwrap_or_default(),
        },
    }
}

fn strip_frontmatter(content: &str) -> &str {
    let Some(stripped) = content.strip_prefix("---") else {
        return content;
    };
    let Some(end_index) = stripped.find("\n---") else {
        return content;
    };
    &stripped[end_index + 4..]
}

/// Enumerates the normalized failures exposed by the skill subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {name}")]
    SkillNotFound { name: String, path: Option<PathBuf> },
    #[error("skill name is ambiguous: {name}")]
    AmbiguousSkillName { name: String, paths: Vec<PathBuf> },
    #[error("skill disabled: {name} at {path}")]
    SkillDisabled { name: String, path: PathBuf },
    #[error("skill parse failed at {path}: {message}")]
    SkillParseFailed { path: PathBuf, message: String },
    #[error("skill root unavailable: {root}")]
    SkillRootUnavailable { root: PathBuf },
    #[error("duplicate skill id {id:?} discovered at {first_path} and {second_path}")]
    DuplicateSkillId {
        id: SkillId,
        first_path: PathBuf,
        second_path: PathBuf,
    },
}
