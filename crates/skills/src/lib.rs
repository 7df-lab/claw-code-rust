//! Skills crate for package mechanics, discovery, rendering, injection, and system skills.

pub mod config_rules;
pub mod injection;
pub mod installer;
pub mod invocation_utils;
pub mod loader;
pub mod manager;
pub mod mention_counts;
pub mod model;
pub mod package;
pub mod parser;
pub mod render;
pub mod system;

pub use injection::SkillInjection;
pub use injection::SkillInjections;
pub use injection::SkillSelection;
pub use injection::build_skill_injections;
pub use injection::collect_explicit_skill_mentions;
pub use installer::{
    DefaultSkillInstallFailure, DefaultSkillInstallMode, DefaultSkillInstallOptions,
    DefaultSkillInstallReport, DefaultSkillInstaller,
};
pub use invocation_utils::detect_implicit_skill_invocation_for_command;
pub use manager::PluginSkillRoot;
pub use manager::SkillsManager;
pub use manager::SkillsRuntimeConfig;
pub use mention_counts::build_skill_name_counts;
pub use model::SkillDependencies;
pub use model::SkillError;
pub use model::SkillInterface;
pub use model::SkillLoadOutcome;
pub use model::SkillMetadata;
pub use model::SkillPolicy;
pub use model::SkillScope;
pub use model::SkillToolDependency;
pub use model::canonicalize_for_identity;
pub use model::normalize_native_path;
pub use package::{
    FrontmatterFormat, Sha256Digest, SkillCompatibility, SkillDefinition, SkillDiagnostic,
    SkillName, SkillPackage, SkillPackageId, SkillResourceKind, SkillResourceRef, SkillSourceKind,
};
pub use parser::parse_skill_md;
pub use render::AvailableSkills;
pub use render::SkillMetadataBudget;
pub use render::SkillRenderReport;
pub use render::build_available_skills;
pub use render::default_skill_metadata_budget;
pub use render::render_available_skills_body;
pub use system::install_system_skills;
pub use system::system_cache_root_dir;
pub use system::uninstall_system_skills;
