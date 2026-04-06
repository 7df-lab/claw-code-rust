use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Strongly typed identifier for a discovered skill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillId(
    /// The stable identifier value for the skill.
    pub SmolStr,
);

/// Stores metadata for one discovered skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    /// The stable unique identifier of the skill.
    pub id: SkillId,
    /// The human-readable skill name.
    pub name: String,
    /// A short description of what the skill provides.
    pub description: String,
    /// The canonical path to the skill document.
    pub path: PathBuf,
    /// Whether the skill is enabled for use.
    pub enabled: bool,
    /// The origin of the discovered skill.
    pub source: SkillSource,
}

/// Identifies where a discovered skill came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// The skill was discovered from a user-level root.
    User,
    /// The skill was discovered from a workspace root.
    Workspace {
        /// The workspace used during discovery.
        cwd: PathBuf,
    },
    /// The skill was discovered from a plugin-owned root.
    Plugin {
        /// The originating plugin identifier.
        plugin_id: String,
    },
}

/// Stores runtime configuration for skill discovery and change tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SkillsConfig {
    /// Whether skill discovery is enabled at all.
    pub enabled: bool,
    /// User-level roots scanned for skills.
    pub user_roots: Vec<PathBuf>,
    /// Workspace-level roots scanned for skills.
    pub workspace_roots: Vec<PathBuf>,
    /// Whether the runtime should watch skill roots for changes.
    pub watch_for_changes: bool,
}

/// Carries the skill content injected into a turn after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSkill {
    /// The skill metadata record.
    pub record: SkillRecord,
    /// The canonical textual content loaded from disk.
    pub content: String,
}

/// Provides discovery and lookup operations for skills.
pub trait SkillCatalog {
    /// Discovers skills for an optional workspace root.
    fn discover(&mut self, workspace_root: Option<&Path>) -> Result<Vec<SkillRecord>, SkillError>;

    /// Returns one discovered skill by identifier.
    fn get(&self, id: &SkillId) -> Option<&SkillRecord>;

    /// Loads the content for one discovered skill.
    fn load(&self, id: &SkillId) -> Result<ResolvedSkill, SkillError>;
}

/// Filesystem-backed implementation of `SkillCatalog`.
#[derive(Debug, Default)]
pub struct FileSystemSkillCatalog {
    /// The configured roots and discovery behavior.
    pub config: SkillsConfig,
    /// The in-memory cache of discovered skills keyed by id.
    cache: HashMap<SkillId, SkillRecord>,
}

impl FileSystemSkillCatalog {
    /// Creates a new filesystem-backed skill catalog.
    pub fn new(config: SkillsConfig) -> Self {
        Self {
            config,
            cache: HashMap::new(),
        }
    }

    fn roots<'a>(&'a self, workspace_root: Option<&'a Path>) -> Vec<(SkillSource, PathBuf)> {
        let mut roots = self
            .config
            .user_roots
            .iter()
            .cloned()
            .map(|root| (SkillSource::User, root))
            .collect::<Vec<_>>();

        roots.extend(self.config.workspace_roots.iter().cloned().map(|root| {
            let cwd = workspace_root
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.clone());
            (SkillSource::Workspace { cwd }, root)
        }));

        roots
    }

    fn discover_from_root(
        &self,
        root: &Path,
        source: SkillSource,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        if !root.exists() {
            return Err(SkillError::SkillRootUnavailable {
                root: root.to_path_buf(),
            });
        }

        let mut discovered = Vec::new();
        for entry in fs::read_dir(root).map_err(|_| SkillError::SkillRootUnavailable {
            root: root.to_path_buf(),
        })? {
            let entry = entry.map_err(|_| SkillError::SkillRootUnavailable {
                root: root.to_path_buf(),
            })?;
            let path = entry.path();
            if path.is_dir() {
                let skill_doc = path.join("SKILL.md");
                if skill_doc.exists() {
                    let name = path
                        .file_name()
                        .and_then(|segment| segment.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    discovered.push(SkillRecord {
                        id: SkillId(name.clone().into()),
                        name: name.clone(),
                        description: format!("Skill discovered at {}", skill_doc.display()),
                        path: skill_doc,
                        enabled: true,
                        source: source.clone(),
                    });
                }
            }
        }

        Ok(discovered)
    }
}

impl SkillCatalog for FileSystemSkillCatalog {
    fn discover(&mut self, workspace_root: Option<&Path>) -> Result<Vec<SkillRecord>, SkillError> {
        if !self.config.enabled {
            self.cache.clear();
            return Ok(Vec::new());
        }

        self.cache.clear();
        let mut all = Vec::new();
        for (source, root) in self.roots(workspace_root) {
            if root.exists() {
                for skill in self.discover_from_root(&root, source)? {
                    self.cache.insert(skill.id.clone(), skill.clone());
                    all.push(skill);
                }
            }
        }

        all.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(all)
    }

    fn get(&self, id: &SkillId) -> Option<&SkillRecord> {
        self.cache.get(id)
    }

    fn load(&self, id: &SkillId) -> Result<ResolvedSkill, SkillError> {
        let record = self
            .cache
            .get(id)
            .ok_or_else(|| SkillError::SkillNotFound { id: id.clone() })?;

        if !record.enabled {
            return Err(SkillError::SkillDisabled { id: id.clone() });
        }

        let content =
            fs::read_to_string(&record.path).map_err(|source| SkillError::SkillParseFailed {
                path: record.path.clone(),
                message: source.to_string(),
            })?;

        Ok(ResolvedSkill {
            record: record.clone(),
            content,
        })
    }
}

/// Enumerates the normalized failures exposed by the skill subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SkillError {
    /// The requested skill identifier was not discovered.
    #[error("skill not found: {id:?}")]
    SkillNotFound {
        /// The missing skill identifier.
        id: SkillId,
    },
    /// The requested skill exists but is disabled.
    #[error("skill disabled: {id:?}")]
    SkillDisabled {
        /// The disabled skill identifier.
        id: SkillId,
    },
    /// The skill document could not be read or parsed.
    #[error("skill parse failed at {path}: {message}")]
    SkillParseFailed {
        /// The skill document path that failed.
        path: PathBuf,
        /// The human-readable failure message.
        message: String,
    },
    /// A configured discovery root could not be accessed.
    #[error("skill root unavailable: {root}")]
    SkillRootUnavailable {
        /// The inaccessible root path.
        root: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{FileSystemSkillCatalog, SkillCatalog, SkillId, SkillsConfig};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clawcr-skill-{name}-{nanos}"));
        std::fs::create_dir_all(&root).expect("create root");
        root
    }

    #[test]
    fn discover_finds_skill_documents() {
        let root = temp_root("discover");
        let skill_dir = root.join("rust");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(skill_dir.join("SKILL.md"), "# Rust\n\nSkill body").expect("write skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });

        let discovered = catalog.discover(None).expect("discover");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "rust");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_reads_skill_content() {
        let root = temp_root("load");
        let skill_dir = root.join("docs");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(skill_dir.join("SKILL.md"), "body").expect("write skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });
        let _ = catalog.discover(None).expect("discover");
        let resolved = catalog
            .load(&SkillId("docs".into()))
            .expect("load resolved skill");

        assert_eq!(resolved.content, "body");

        let _ = std::fs::remove_dir_all(root);
    }
}
