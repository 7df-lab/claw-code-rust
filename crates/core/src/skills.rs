use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::SkillsConfig;

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

    fn parse_skill_document(
        &self,
        skill_doc: &Path,
        fallback_name: &str,
    ) -> Result<ParsedSkillDocument, SkillError> {
        let content =
            fs::read_to_string(skill_doc).map_err(|source| SkillError::SkillParseFailed {
                path: skill_doc.to_path_buf(),
                message: source.to_string(),
            })?;
        let (frontmatter, body) = parse_skill_frontmatter(skill_doc, &content)?;
        let name = frontmatter
            .name
            .unwrap_or_else(|| fallback_name.to_string())
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(SkillError::SkillParseFailed {
                path: skill_doc.to_path_buf(),
                message: "skill name must not be empty".into(),
            });
        }

        Ok(ParsedSkillDocument {
            id: SkillId(name.clone().into()),
            name,
            description: frontmatter
                .description
                .unwrap_or_else(|| format!("Skill discovered at {}", skill_doc.display())),
            enabled: frontmatter.enabled.unwrap_or(true),
            content: body.trim_start_matches(['\r', '\n']).to_string(),
        })
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

        let mut skill_dirs = fs::read_dir(root)
            .map_err(|_| SkillError::SkillRootUnavailable {
                root: root.to_path_buf(),
            })?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|_| SkillError::SkillRootUnavailable {
                        root: root.to_path_buf(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        skill_dirs.sort();

        let mut discovered = Vec::new();
        for path in skill_dirs {
            if path.is_dir() {
                let skill_doc = path.join("SKILL.md");
                if skill_doc.exists() {
                    let fallback_name = path
                        .file_name()
                        .and_then(|segment| segment.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let parsed = self.parse_skill_document(&skill_doc, &fallback_name)?;
                    discovered.push(SkillRecord {
                        id: parsed.id,
                        name: parsed.name,
                        description: parsed.description,
                        path: normalize_canonical_path(fs::canonicalize(&skill_doc).map_err(
                            |source| SkillError::SkillParseFailed {
                                path: skill_doc.clone(),
                                message: source.to_string(),
                            },
                        )?),
                        enabled: parsed.enabled,
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
        for (source, root) in self.roots(workspace_root) {
            if root.exists() {
                for skill in self.discover_from_root(&root, source)? {
                    if let Some(existing) = self.cache.insert(skill.id.clone(), skill.clone()) {
                        return Err(SkillError::DuplicateSkillId {
                            id: skill.id,
                            first_path: existing.path,
                            second_path: skill.path,
                        });
                    }
                }
            }
        }

        let mut all = self.cache.values().cloned().collect::<Vec<_>>();
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

        let parsed = self.parse_skill_document(&record.path, &record.name)?;

        Ok(ResolvedSkill {
            record: record.clone(),
            content: parsed.content,
        })
    }
}

#[derive(Debug, Default)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug)]
struct ParsedSkillDocument {
    id: SkillId,
    name: String,
    description: String,
    enabled: bool,
    content: String,
}

fn parse_skill_frontmatter<'a>(
    skill_doc: &Path,
    content: &'a str,
) -> Result<(SkillFrontmatter, &'a str), SkillError> {
    let mut lines = content.split_inclusive('\n');
    let Some(first_line) = lines.next() else {
        return Ok((SkillFrontmatter::default(), content));
    };
    if first_line.trim() != "---" {
        return Ok((SkillFrontmatter::default(), content));
    }

    let mut consumed = first_line.len();
    let mut frontmatter = SkillFrontmatter::default();
    let mut found_end = false;
    for line in lines {
        consumed += line.len();
        if line.trim() == "---" {
            found_end = true;
            break;
        }
        parse_frontmatter_line(skill_doc, &mut frontmatter, line)?;
    }

    if !found_end {
        return Err(SkillError::SkillParseFailed {
            path: skill_doc.to_path_buf(),
            message: "unterminated skill frontmatter".into(),
        });
    }

    Ok((frontmatter, &content[consumed..]))
}

fn parse_frontmatter_line(
    skill_doc: &Path,
    frontmatter: &mut SkillFrontmatter,
    line: &str,
) -> Result<(), SkillError> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(());
    }
    let Some((key, value)) = trimmed.split_once(':') else {
        return Err(SkillError::SkillParseFailed {
            path: skill_doc.to_path_buf(),
            message: format!("invalid skill frontmatter line: {trimmed}"),
        });
    };
    let key = key.trim();
    let value = value.trim();
    match key {
        "name" => frontmatter.name = Some(parse_frontmatter_string(value)),
        "description" => frontmatter.description = Some(parse_frontmatter_string(value)),
        "enabled" => {
            frontmatter.enabled =
                Some(
                    value
                        .parse::<bool>()
                        .map_err(|_| SkillError::SkillParseFailed {
                            path: skill_doc.to_path_buf(),
                            message: format!("invalid boolean in skill frontmatter: {value}"),
                        })?,
                )
        }
        _ => {}
    }
    Ok(())
}

fn parse_frontmatter_string(value: &str) -> String {
    let trimmed = value.trim();
    let quoted = trimmed
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|text| text.strip_suffix('\''))
        });
    quoted.unwrap_or(trimmed).trim().to_string()
}

pub fn normalize_canonical_path(path: PathBuf) -> PathBuf {
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
    /// Two discovered skills resolved to the same id.
    #[error("duplicate skill id {id:?} discovered at {first_path} and {second_path}")]
    DuplicateSkillId {
        /// The conflicting stable skill identifier.
        id: SkillId,
        /// The first discovered skill document path.
        first_path: PathBuf,
        /// The second discovered skill document path.
        second_path: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        FileSystemSkillCatalog, SkillCatalog, SkillError, SkillId, normalize_canonical_path,
    };
    use crate::SkillsConfig;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devo-skill-{name}-{nanos}"));
        std::fs::create_dir_all(&root).expect("create root");
        root
    }

    #[test]
    fn discover_finds_skill_documents() {
        let root = temp_root("discover");
        let skill_dir = root.join("rust");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rust-docs\ndescription: Official Rust docs\nenabled: true\n---\n# Rust\n\nSkill body",
        )
        .expect("write skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });

        let discovered = catalog.discover(None).expect("discover");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, SkillId("rust-docs".into()));
        assert_eq!(discovered[0].name, "rust-docs");
        assert_eq!(discovered[0].description, "Official Rust docs");
        assert!(discovered[0].enabled);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_reads_skill_content() {
        let root = temp_root("load");
        let skill_dir = root.join("docs");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: docs\ndescription: Documentation skill\n---\nbody",
        )
        .expect("write skill");

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

    #[test]
    fn load_rejects_disabled_skill() {
        let root = temp_root("disabled");
        let skill_dir = root.join("disabled-skill");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: disabled-skill\nenabled: false\ndescription: Disabled skill\n---\nbody",
        )
        .expect("write skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });
        let discovered = catalog.discover(None).expect("discover");

        assert_eq!(discovered[0].enabled, false);
        assert_eq!(
            catalog.load(&SkillId("disabled-skill".into())),
            Err(SkillError::SkillDisabled {
                id: SkillId("disabled-skill".into()),
            })
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discover_rediscovers_updated_skill_metadata() {
        let root = temp_root("rediscovers");
        let skill_dir = root.join("skill");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: original\ndescription: Original description\n---\nbody",
        )
        .expect("write original skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });
        let first = catalog.discover(None).expect("first discover");
        assert_eq!(first[0].id, SkillId("original".into()));

        std::fs::write(
            &skill_path,
            "---\nname: updated\ndescription: Updated description\nenabled: false\n---\nbody",
        )
        .expect("write updated skill");

        let second = catalog.discover(None).expect("second discover");
        assert_eq!(second[0].id, SkillId("updated".into()));
        assert_eq!(second[0].description, "Updated description");
        assert_eq!(second[0].enabled, false);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discover_rejects_duplicate_skill_ids() {
        let root = temp_root("duplicate");
        let alpha_dir = root.join("alpha");
        let bravo_dir = root.join("bravo");
        std::fs::create_dir_all(&alpha_dir).expect("create alpha dir");
        std::fs::create_dir_all(&bravo_dir).expect("create bravo dir");
        let alpha_path = alpha_dir.join("SKILL.md");
        let bravo_path = bravo_dir.join("SKILL.md");
        std::fs::write(
            &alpha_path,
            "---\nname: shared\ndescription: First\n---\nalpha",
        )
        .expect("write alpha skill");
        std::fs::write(
            &bravo_path,
            "---\nname: shared\ndescription: Second\n---\nbravo",
        )
        .expect("write bravo skill");

        let mut catalog = FileSystemSkillCatalog::new(SkillsConfig {
            enabled: true,
            user_roots: vec![root.clone()],
            workspace_roots: Vec::new(),
            watch_for_changes: false,
        });

        assert_eq!(
            catalog.discover(None),
            Err(SkillError::DuplicateSkillId {
                id: SkillId("shared".into()),
                first_path: normalize_canonical_path(
                    std::fs::canonicalize(alpha_path).expect("canonicalize alpha"),
                ),
                second_path: normalize_canonical_path(
                    std::fs::canonicalize(bravo_path).expect("canonicalize bravo"),
                ),
            })
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
