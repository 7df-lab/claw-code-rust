//! Permission evaluation and approval pipeline.
//!
//! Implements L3-BEH-CORE-004. Core owns permission profile resolution,
//! access evaluation, the `authorize_tool_request()` entry point with
//! four-layer pipeline, approval scoping/caching, and auto-reviewer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Permission Types ────────────────────────────────────────────────

/// Resolved permission profile for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionProfile {
    pub filesystem_policy: Vec<FsPolicyEntry>,
    pub network_policy: NetworkPolicy,
}

/// One filesystem policy entry with materialized absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsPolicyEntry {
    pub path: PathBuf,
    pub access: AccessMode,
    pub is_explicit: bool,
}

/// Access mode for a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    None,
    Read,
    Write,
}

/// Network access policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetworkPolicy {
    pub enabled: bool,
    pub allowed_domains: Vec<String>,
    pub denied_domains: Vec<String>,
    pub proxy_url: Option<String>,
}

/// Runtime permission profile with workspace roots and per-call additions.
#[derive(Debug, Clone)]
pub struct RuntimePermissionProfile {
    pub profile: PermissionProfile,
    pub workspace_roots: Vec<PathBuf>,
    pub additional_per_call: Option<AdditionalPermissions>,
}

/// Additional permissions granted for a single invocation.
#[derive(Debug, Clone)]
pub struct AdditionalPermissions {
    pub extra_writable_paths: Vec<PathBuf>,
    pub extra_allowed_domains: Vec<String>,
}

// ── Profile Resolution ──────────────────────────────────────────────

/// Resolve a named profile into a concrete PermissionProfile.
pub fn resolve_permission_profile(
    name: &str,
    workspace_roots: &[PathBuf],
    _custom_profiles: &HashMap<String, CustomProfile>,
) -> Result<PermissionProfile, ProfileError> {
    let mut profile = match name {
        ":read-only" => build_read_only_profile(),
        ":workspace" => build_workspace_profile(workspace_roots),
        ":danger-full-access" => build_danger_full_access_profile(),
        other => {
            return Err(ProfileError::UnknownProfile(other.to_string()));
        }
    };
    materialize_workspace_roots(&mut profile, workspace_roots);
    Ok(profile)
}

/// Placeholder for custom profile definitions.
#[derive(Debug, Clone)]
pub struct CustomProfile {
    pub filesystem_policy: Vec<FsPolicyEntry>,
    pub network_policy: NetworkPolicy,
}

/// Error during profile resolution.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProfileError {
    #[error("unknown profile: {0}")]
    UnknownProfile(String),
    #[error("invalid profile: {0}")]
    InvalidProfile(String),
}

fn build_read_only_profile() -> PermissionProfile {
    PermissionProfile {
        filesystem_policy: vec![],
        network_policy: NetworkPolicy {
            enabled: false,
            ..Default::default()
        },
    }
}

fn build_workspace_profile(roots: &[PathBuf]) -> PermissionProfile {
    let mut entries: Vec<FsPolicyEntry> = roots
        .iter()
        .map(|r| FsPolicyEntry {
            path: r.clone(),
            access: AccessMode::Write,
            is_explicit: true,
        })
        .collect();

    // Add read-only carveouts for protected directories within writable roots
    for root in roots {
        for protected in &[".git", ".devo", ".agents"] {
            let protected_path = root.join(protected);
            // Only add if not already covered by a more specific entry
            if !entries.iter().any(|e| e.path == protected_path) {
                entries.push(FsPolicyEntry {
                    path: protected_path,
                    access: AccessMode::Read,
                    is_explicit: false,
                });
            }
        }
    }

    PermissionProfile {
        filesystem_policy: entries,
        network_policy: NetworkPolicy {
            enabled: false,
            ..Default::default()
        },
    }
}

fn build_danger_full_access_profile() -> PermissionProfile {
    PermissionProfile {
        filesystem_policy: vec![],
        network_policy: NetworkPolicy {
            enabled: true,
            ..Default::default()
        },
    }
}

/// Materialize workspace-relative paths to absolute paths.
pub fn materialize_workspace_roots(profile: &mut PermissionProfile, roots: &[PathBuf]) {
    for entry in &mut profile.filesystem_policy {
        if entry.path.is_relative() {
            for root in roots {
                let candidate = root.join(&entry.path);
                if candidate.exists() || entry.path.starts_with(".") {
                    entry.path = candidate;
                    break;
                }
            }
        }
    }
}

// ── Access Evaluation ───────────────────────────────────────────────

/// Resolve the effective access mode for a path.
pub fn resolve_access(path: &Path, profile: &PermissionProfile) -> AccessMode {
    let canonical = normalize_access_path(path);

    let mut best_match: Option<&FsPolicyEntry> = None;
    let mut best_len = 0;

    for entry in &profile.filesystem_policy {
        let entry_path = normalize_access_path(&entry.path);
        if let Ok(_stripped) = canonical.strip_prefix(&entry_path) {
            // Longest prefix match wins
            let prefix_len = entry_path.as_os_str().len();
            if prefix_len > best_len {
                best_len = prefix_len;
                best_match = Some(entry);
            } else if prefix_len == best_len {
                // At equal specificity: None > Write > Read
                if let Some(current) = best_match
                    && match entry.access {
                        AccessMode::None => 2,
                        AccessMode::Write => 1,
                        AccessMode::Read => 0,
                    } > match current.access {
                        AccessMode::None => 2,
                        AccessMode::Write => 1,
                        AccessMode::Read => 0,
                    }
                {
                    best_match = Some(entry);
                }
            }
        }
    }

    best_match.map(|e| e.access).unwrap_or(AccessMode::None)
}

fn normalize_access_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if let Ok(canonical) = normalized.canonicalize() {
                    normalized = canonical;
                }
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => {
                let candidate = normalized.join(part);
                if std::fs::symlink_metadata(&candidate).is_ok()
                    && let Ok(canonical) = candidate.canonicalize()
                {
                    normalized = canonical;
                } else {
                    normalized.push(part);
                }
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

/// Check if a path is readable.
pub fn can_read(path: &Path, profile: &PermissionProfile) -> bool {
    matches!(
        resolve_access(path, profile),
        AccessMode::Read | AccessMode::Write
    )
}

/// Check if a path is writable.
pub fn can_write(path: &Path, profile: &PermissionProfile) -> bool {
    matches!(resolve_access(path, profile), AccessMode::Write)
}

/// Check if network access is enabled for a host.
pub fn network_enabled(profile: &PermissionProfile, host: &str) -> bool {
    if !profile.network_policy.enabled {
        return false;
    }
    if profile
        .network_policy
        .denied_domains
        .iter()
        .any(|d| host.contains(d))
    {
        return false;
    }
    if profile.network_policy.allowed_domains.is_empty() {
        return true;
    }
    profile
        .network_policy
        .allowed_domains
        .iter()
        .any(|d| host.contains(d))
}

// ── Tool Permission Request ─────────────────────────────────────────

/// A request to authorize a tool operation.
#[derive(Debug, Clone)]
pub struct ToolPermissionRequest {
    pub tool_name: String,
    pub tool_category: ToolCategory,
    pub resource: ResourceKind,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    pub command: Option<String>,
    pub command_description: Option<String>,
    pub justification: Option<String>,
    pub sandbox_mode: SandboxMode,
}

/// Category of a tool for permission gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    ReadOnly,
    Mutating,
    Command,
    BackgroundProcess,
    UserPrompt,
    Planning,
    GoalStatus,
    Delegation,
    Web,
    Internal,
}

/// The resource kind being accessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    FileRead,
    FileWrite,
    ShellExec,
    Network,
    ExternalTool,
}

/// Sandbox mode for the current operation.
#[derive(Debug, Clone)]
pub enum SandboxMode {
    Normal,
    AdditionalPermissions(AdditionalPermissions),
    RequireEscalated {
        justification: String,
        prefix_rule: Option<Vec<String>>,
    },
}

// ── Permission Decision ─────────────────────────────────────────────

/// The outcome of the authorization pipeline.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow,
    Deny {
        reason: String,
    },
    Ask {
        approval_id: ApprovalId,
        summary: String,
        details: String,
        available_scopes: Vec<ApprovalScope>,
        expires_at: Option<DateTime<Utc>>,
    },
}

/// Unique identifier for an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub uuid::Uuid);

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// Scopes available for an approval decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    Once,
    Turn,
    Session,
    PathPrefix(PathBuf),
    Host(String),
    CommandPrefix(Vec<String>),
    McpTool { server: String, tool: String },
}

// ── Approval Cache ──────────────────────────────────────────────────

/// Caches approval decisions within a session and turn.
#[derive(Debug, Clone)]
pub struct ApprovalCache {
    pub turn_tools: HashSet<(String, ResourceKind)>,
    pub session_tools: HashSet<(String, ResourceKind)>,
    pub turn_hosts: HashSet<String>,
    pub session_hosts: HashSet<String>,
    pub turn_path_prefixes: HashSet<PathBuf>,
    pub session_path_prefixes: HashSet<PathBuf>,
    pub turn_command_prefixes: HashSet<Vec<String>>,
    pub session_command_prefixes: HashSet<Vec<String>>,
    pub denied_session: HashSet<(String, ResourceKind)>,
}

impl ApprovalCache {
    pub fn new() -> Self {
        Self {
            turn_tools: HashSet::new(),
            session_tools: HashSet::new(),
            turn_hosts: HashSet::new(),
            session_hosts: HashSet::new(),
            turn_path_prefixes: HashSet::new(),
            session_path_prefixes: HashSet::new(),
            turn_command_prefixes: HashSet::new(),
            session_command_prefixes: HashSet::new(),
            denied_session: HashSet::new(),
        }
    }

    /// Clear turn-level entries after a turn completes.
    pub fn clear_turn(&mut self) {
        self.turn_tools.clear();
        self.turn_hosts.clear();
        self.turn_path_prefixes.clear();
        self.turn_command_prefixes.clear();
    }

    /// Check if a tool+resource is cached as allowed.
    pub fn is_allowed(&self, tool_name: &str, resource: &ResourceKind) -> bool {
        let key = (tool_name.to_string(), *resource);
        self.session_tools.contains(&key) || self.turn_tools.contains(&key)
    }

    /// Check if a tool+resource is denied for the session.
    pub fn is_denied(&self, tool_name: &str, resource: &ResourceKind) -> bool {
        let key = (tool_name.to_string(), *resource);
        self.denied_session.contains(&key)
    }

    /// Cache an allowed decision.
    pub fn allow(&mut self, scope: &ApprovalScope, tool_name: &str, resource: ResourceKind) {
        let key = (tool_name.to_string(), resource);
        match scope {
            ApprovalScope::Once | ApprovalScope::Turn => {
                self.turn_tools.insert(key);
            }
            ApprovalScope::Session => {
                self.session_tools.insert(key);
            }
            ApprovalScope::PathPrefix(path) => {
                self.turn_path_prefixes.insert(path.clone());
            }
            ApprovalScope::Host(host) => {
                self.turn_hosts.insert(host.clone());
            }
            ApprovalScope::CommandPrefix(tokens) => {
                self.turn_command_prefixes.insert(tokens.clone());
            }
            ApprovalScope::McpTool { .. } => {
                self.turn_tools.insert(key);
            }
        }
    }
}

impl Default for ApprovalCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Approval Policy ─────────────────────────────────────────────────

/// Policy configuration for the approval pipeline.
#[derive(Debug, Clone)]
pub struct ApprovalPolicy {
    pub permission_mode: PermissionMode,
    pub approval_timeout_seconds: u64,
    pub auto_reviewer_enabled: bool,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            permission_mode: PermissionMode::Default,
            approval_timeout_seconds: 300,
            auto_reviewer_enabled: false,
        }
    }
}

/// High-level permission mode for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Default,
    #[serde(alias = "auto_approve")]
    Yolo,
    Deny,
}

// ── Auto-Reviewer ───────────────────────────────────────────────────

/// Decision from an auto-reviewer model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Deny,
    Uncertain,
}

/// State of the auto-reviewer circuit breaker.
#[derive(Debug, Clone)]
pub struct AutoReviewerState {
    pub consecutive_denials: u32,
    pub denials_in_window: VecDeque<DateTime<Utc>>,
    pub tripped: bool,
}

impl AutoReviewerState {
    pub fn new() -> Self {
        Self {
            consecutive_denials: 0,
            denials_in_window: VecDeque::new(),
            tripped: false,
        }
    }

    pub fn check_and_update(&mut self, decision: &ReviewDecision) -> AutoReviewerStatus {
        if self.tripped {
            return AutoReviewerStatus::Tripped;
        }

        match decision {
            ReviewDecision::Deny => {
                self.consecutive_denials += 1;
                self.denials_in_window.push_back(Utc::now());
                while self.denials_in_window.len() > 50 {
                    self.denials_in_window.pop_front();
                }
            }
            ReviewDecision::Approve => {
                self.consecutive_denials = 0;
                self.denials_in_window.push_back(Utc::now());
            }
            ReviewDecision::Uncertain => { /* no change */ }
        }

        if self.consecutive_denials >= 3 || self.denials_in_window.len() >= 10 {
            self.tripped = true;
            AutoReviewerStatus::Tripped
        } else {
            AutoReviewerStatus::Active
        }
    }

    pub fn reset(&mut self) {
        self.consecutive_denials = 0;
        self.denials_in_window.clear();
        self.tripped = false;
    }
}

impl Default for AutoReviewerState {
    fn default() -> Self {
        Self::new()
    }
}

/// Status returned by the auto-reviewer after a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReviewerStatus {
    Active,
    Tripped,
}

// ── Central Authorization Entry Point ───────────────────────────────

/// Authorize a tool request through the four-layer pipeline.
///
/// Layer 1 — PermissionMode override (Yolo/Deny)
/// Layer 2 — Profile evaluation against RuntimePermissionProfile
/// Layer 3 — Approval cache check
/// Layer 4 — Return Ask for user prompt (handled by server)
pub fn authorize_tool_request(
    request: &ToolPermissionRequest,
    profile: &RuntimePermissionProfile,
    cache: &mut ApprovalCache,
    policy: &ApprovalPolicy,
) -> PermissionDecision {
    // Layer 1 — PermissionMode override
    match policy.permission_mode {
        PermissionMode::Yolo => return PermissionDecision::Allow,
        PermissionMode::Deny => {
            return PermissionDecision::Deny {
                reason: "permission mode is Deny".into(),
            };
        }
        PermissionMode::Default => { /* continue */ }
    }

    // Check denied cache first
    if cache.is_denied(&request.tool_name, &request.resource) {
        return PermissionDecision::Deny {
            reason: "previously denied for this session".into(),
        };
    }

    // Layer 2 — Profile evaluation
    let profile_allowed = match request.resource {
        ResourceKind::FileRead => request
            .path
            .as_ref()
            .map(|p| can_read(p, &profile.profile))
            .unwrap_or(false),
        ResourceKind::FileWrite => request
            .path
            .as_ref()
            .map(|p| can_write(p, &profile.profile))
            .unwrap_or(false),
        ResourceKind::Network => request
            .host
            .as_ref()
            .map(|h| network_enabled(&profile.profile, h))
            .unwrap_or(false),
        ResourceKind::ShellExec => {
            // Shell exec always requires approval unless cache says otherwise
            false
        }
        ResourceKind::ExternalTool => false,
    };

    if profile_allowed {
        return PermissionDecision::Allow;
    }

    // Layer 3 — Check approval cache
    if cache.is_allowed(&request.tool_name, &request.resource) {
        return PermissionDecision::Allow;
    }

    // Layer 4 — User prompt needed
    let approval_id = ApprovalId::new();
    let summary = format!(
        "Allow {} to {}?",
        request.tool_name,
        match request.resource {
            ResourceKind::FileRead => "read file",
            ResourceKind::FileWrite => "write file",
            ResourceKind::ShellExec => "execute command",
            ResourceKind::Network => "access network",
            ResourceKind::ExternalTool => "use external tool",
        }
    );

    PermissionDecision::Ask {
        approval_id,
        summary,
        details: request
            .justification
            .clone()
            .unwrap_or_else(|| "no justification provided".into()),
        available_scopes: vec![
            ApprovalScope::Once,
            ApprovalScope::Turn,
            ApprovalScope::Session,
        ],
        expires_at: Some(
            Utc::now() + chrono::Duration::seconds(policy.approval_timeout_seconds as i64),
        ),
    }
}

// ── Execution Grant ──────────────────────────────────────────────

/// A scoped grant token representing an approved tool execution.
///
/// Per L3-BEH-SAFETY-002, `ExecutionGrant` is produced by the approval
/// pipeline and consumed by the sandbox enforcement layer. It captures
/// the scope and constraints of an approved operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGrant {
    /// Unique identifier for this grant.
    pub grant_id: String,
    /// The tool call this grant authorizes.
    pub tool_call_id: String,
    /// The tool name this grant authorizes.
    pub tool_name: String,
    /// The scope of the approval (Once, Turn, Session, etc.).
    pub scope: ApprovalScope,
    /// The permission profile that was active when the grant was issued.
    pub permission_mode: PermissionMode,
    /// When the grant was issued.
    pub issued_at: chrono::DateTime<chrono::Utc>,
    /// When the grant expires, if applicable.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Additional constraints on the grant (e.g., specific paths, commands).
    pub constraints: GrantConstraints,
}

/// Additional constraints on an execution grant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantConstraints {
    /// Allowed working directory paths.
    pub allowed_paths: Vec<PathBuf>,
    /// Allowed command prefixes.
    pub allowed_command_prefixes: Vec<Vec<String>>,
    /// Allowed network hosts.
    pub allowed_hosts: Vec<String>,
}

impl ExecutionGrant {
    /// Check if the grant is still valid (not expired).
    pub fn is_valid(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            chrono::Utc::now() < expires_at
        } else {
            true
        }
    }

    /// Check if the grant matches the given tool call.
    pub fn matches(&self, tool_call_id: &str, tool_name: &str) -> bool {
        self.tool_call_id == tool_call_id && self.tool_name == tool_name
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AccessMode ───────────────────────────────────────────────

    #[test]
    fn access_mode_ordering() {
        assert!(AccessMode::Write > AccessMode::Read);
        assert!(AccessMode::Read > AccessMode::None);
        assert_eq!(AccessMode::None, AccessMode::None);
    }

    #[test]
    fn access_mode_serde_roundtrip() {
        for mode in &[AccessMode::Read, AccessMode::Write, AccessMode::None] {
            let json = serde_json::to_string(mode).expect("serialize");
            let restored: AccessMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *mode);
        }
    }

    // ── Profile Resolution ───────────────────────────────────────

    #[test]
    fn resolve_read_only_profile() {
        let profile =
            resolve_permission_profile(":read-only", &[], &HashMap::new()).expect("resolve");
        // read-only: root is Read, no Write entries, network disabled
        assert!(!profile.network_policy.enabled);
    }

    #[test]
    fn resolve_workspace_profile() {
        let roots = vec![PathBuf::from("/tmp/test")];
        let profile =
            resolve_permission_profile(":workspace", &roots, &HashMap::new()).expect("resolve");
        assert!(!profile.network_policy.enabled);
        // Should have Write on workspace root
        assert!(
            profile
                .filesystem_policy
                .iter()
                .any(|e| e.path == Path::new("/tmp/test") && e.access == AccessMode::Write)
        );
    }

    #[test]
    fn resolve_danger_full_access_profile() {
        let profile = resolve_permission_profile(":danger-full-access", &[], &HashMap::new())
            .expect("resolve");
        assert!(profile.network_policy.enabled);
    }

    #[test]
    fn resolve_unknown_profile_errors() {
        let result = resolve_permission_profile(":unknown", &[], &HashMap::new());
        assert!(result.is_err());
    }

    // ── Access Evaluation ────────────────────────────────────────

    #[test]
    fn resolve_access_no_entries_returns_none() {
        let profile = PermissionProfile {
            filesystem_policy: vec![],
            network_policy: NetworkPolicy::default(),
        };
        assert_eq!(
            resolve_access(Path::new("/tmp/foo"), &profile),
            AccessMode::None
        );
    }

    #[test]
    fn resolve_access_longest_prefix_wins() {
        let profile = PermissionProfile {
            filesystem_policy: vec![
                FsPolicyEntry {
                    path: PathBuf::from("/tmp"),
                    access: AccessMode::Read,
                    is_explicit: true,
                },
                FsPolicyEntry {
                    path: PathBuf::from("/tmp/sub"),
                    access: AccessMode::Write,
                    is_explicit: true,
                },
            ],
            network_policy: NetworkPolicy::default(),
        };
        // /tmp/sub should get Write from the more specific entry
        assert_eq!(
            resolve_access(Path::new("/tmp/sub/file.txt"), &profile),
            AccessMode::Write
        );
        // /tmp/other should get Read from the less specific entry
        assert_eq!(
            resolve_access(Path::new("/tmp/other.txt"), &profile),
            AccessMode::Read
        );
    }

    #[test]
    fn can_read_accepts_read_or_write() {
        let profile = PermissionProfile {
            filesystem_policy: vec![FsPolicyEntry {
                path: PathBuf::from("/tmp"),
                access: AccessMode::Read,
                is_explicit: true,
            }],
            network_policy: NetworkPolicy::default(),
        };
        assert!(can_read(Path::new("/tmp/file.txt"), &profile));
        assert!(!can_write(Path::new("/tmp/file.txt"), &profile));
    }

    #[test]
    fn network_enabled_checks_policy() {
        let profile = PermissionProfile {
            filesystem_policy: vec![],
            network_policy: NetworkPolicy {
                enabled: true,
                allowed_domains: vec!["api.example.com".into()],
                denied_domains: vec!["evil.com".into()],
                proxy_url: None,
            },
        };
        assert!(network_enabled(&profile, "api.example.com"));
        assert!(!network_enabled(&profile, "evil.com"));
        assert!(!network_enabled(&profile, "other.com"));
    }

    #[test]
    fn network_disabled_denies_all() {
        let profile = PermissionProfile {
            filesystem_policy: vec![],
            network_policy: NetworkPolicy {
                enabled: false,
                ..Default::default()
            },
        };
        assert!(!network_enabled(&profile, "anything.com"));
    }

    // ── ApprovalCache ────────────────────────────────────────────

    #[test]
    fn cache_clear_turn_clears_turn_entries() {
        let mut cache = ApprovalCache::new();
        cache.allow(&ApprovalScope::Turn, "read", ResourceKind::FileRead);
        cache.allow(&ApprovalScope::Session, "write", ResourceKind::FileWrite);
        assert!(cache.is_allowed("read", &ResourceKind::FileRead));
        assert!(cache.is_allowed("write", &ResourceKind::FileWrite));

        cache.clear_turn();
        assert!(!cache.is_allowed("read", &ResourceKind::FileRead));
        assert!(cache.is_allowed("write", &ResourceKind::FileWrite)); // session persists
    }

    #[test]
    fn cache_denied_persists() {
        let mut cache = ApprovalCache::new();
        let key = (String::from("shell"), ResourceKind::ShellExec);
        cache.denied_session.insert(key.clone());
        assert!(cache.is_denied("shell", &ResourceKind::ShellExec));
        cache.clear_turn();
        assert!(cache.is_denied("shell", &ResourceKind::ShellExec)); // still denied
    }

    // ── AutoReviewerState ────────────────────────────────────────

    #[test]
    fn auto_reviewer_starts_active() {
        let state = AutoReviewerState::new();
        assert!(!state.tripped);
        assert_eq!(state.consecutive_denials, 0);
    }

    #[test]
    fn auto_reviewer_trips_after_three_consecutive_denials() {
        let mut state = AutoReviewerState::new();
        assert_eq!(
            state.check_and_update(&ReviewDecision::Deny),
            AutoReviewerStatus::Active
        );
        assert_eq!(
            state.check_and_update(&ReviewDecision::Deny),
            AutoReviewerStatus::Active
        );
        assert_eq!(
            state.check_and_update(&ReviewDecision::Deny),
            AutoReviewerStatus::Tripped
        );
        assert!(state.tripped);
    }

    #[test]
    fn auto_reviewer_approve_resets_denials() {
        let mut state = AutoReviewerState::new();
        state.check_and_update(&ReviewDecision::Deny);
        state.check_and_update(&ReviewDecision::Deny);
        assert_eq!(state.consecutive_denials, 2);
        state.check_and_update(&ReviewDecision::Approve);
        assert_eq!(state.consecutive_denials, 0);
    }

    #[test]
    fn auto_reviewer_uncertain_no_change() {
        let mut state = AutoReviewerState::new();
        state.check_and_update(&ReviewDecision::Deny);
        assert_eq!(state.consecutive_denials, 1);
        state.check_and_update(&ReviewDecision::Uncertain);
        assert_eq!(state.consecutive_denials, 1); // unchanged
    }

    #[test]
    fn auto_reviewer_reset_clears_state() {
        let mut state = AutoReviewerState::new();
        state.check_and_update(&ReviewDecision::Deny);
        state.check_and_update(&ReviewDecision::Deny);
        state.check_and_update(&ReviewDecision::Deny);
        assert!(state.tripped);
        state.reset();
        assert!(!state.tripped);
        assert_eq!(state.consecutive_denials, 0);
    }

    // ── authorize_tool_request ───────────────────────────────────

    #[test]
    fn yolo_mode_allows_everything() {
        let request = ToolPermissionRequest {
            tool_name: "shell".into(),
            tool_category: ToolCategory::Command,
            resource: ResourceKind::ShellExec,
            path: None,
            host: None,
            command: Some("rm -rf /".into()),
            command_description: None,
            justification: None,
            sandbox_mode: SandboxMode::Normal,
        };
        let profile = RuntimePermissionProfile {
            profile: PermissionProfile {
                filesystem_policy: vec![],
                network_policy: NetworkPolicy::default(),
            },
            workspace_roots: vec![],
            additional_per_call: None,
        };
        let mut cache = ApprovalCache::new();
        let policy = ApprovalPolicy {
            permission_mode: PermissionMode::Yolo,
            ..Default::default()
        };

        let decision = authorize_tool_request(&request, &profile, &mut cache, &policy);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn deny_mode_denies_everything() {
        let request = ToolPermissionRequest {
            tool_name: "read".into(),
            tool_category: ToolCategory::ReadOnly,
            resource: ResourceKind::FileRead,
            path: Some(PathBuf::from("/tmp/file.txt")),
            host: None,
            command: None,
            command_description: None,
            justification: None,
            sandbox_mode: SandboxMode::Normal,
        };
        let profile = RuntimePermissionProfile {
            profile: PermissionProfile {
                filesystem_policy: vec![],
                network_policy: NetworkPolicy::default(),
            },
            workspace_roots: vec![],
            additional_per_call: None,
        };
        let mut cache = ApprovalCache::new();
        let policy = ApprovalPolicy {
            permission_mode: PermissionMode::Deny,
            ..Default::default()
        };

        let decision = authorize_tool_request(&request, &profile, &mut cache, &policy);
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn default_mode_asks_for_shell_exec() {
        let request = ToolPermissionRequest {
            tool_name: "shell".into(),
            tool_category: ToolCategory::Command,
            resource: ResourceKind::ShellExec,
            path: None,
            host: None,
            command: Some("npm install".into()),
            command_description: Some("Install dependencies".into()),
            justification: Some("Need to build project".into()),
            sandbox_mode: SandboxMode::Normal,
        };
        let profile = RuntimePermissionProfile {
            profile: PermissionProfile {
                filesystem_policy: vec![],
                network_policy: NetworkPolicy::default(),
            },
            workspace_roots: vec![],
            additional_per_call: None,
        };
        let mut cache = ApprovalCache::new();
        let policy = ApprovalPolicy::default();

        let decision = authorize_tool_request(&request, &profile, &mut cache, &policy);
        assert!(matches!(decision, PermissionDecision::Ask { .. }));
    }

    #[test]
    fn cached_allow_bypasses_approval() {
        let request = ToolPermissionRequest {
            tool_name: "shell".into(),
            tool_category: ToolCategory::Command,
            resource: ResourceKind::ShellExec,
            path: None,
            host: None,
            command: Some("ls".into()),
            command_description: None,
            justification: None,
            sandbox_mode: SandboxMode::Normal,
        };
        let profile = RuntimePermissionProfile {
            profile: PermissionProfile {
                filesystem_policy: vec![],
                network_policy: NetworkPolicy::default(),
            },
            workspace_roots: vec![],
            additional_per_call: None,
        };
        let mut cache = ApprovalCache::new();
        cache.allow(&ApprovalScope::Session, "shell", ResourceKind::ShellExec);
        let policy = ApprovalPolicy::default();

        let decision = authorize_tool_request(&request, &profile, &mut cache, &policy);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    // ── ResourceKind serde ───────────────────────────────────────

    #[test]
    fn resource_kind_serde_roundtrip() {
        for kind in &[
            ResourceKind::FileRead,
            ResourceKind::FileWrite,
            ResourceKind::ShellExec,
            ResourceKind::Network,
            ResourceKind::ExternalTool,
        ] {
            let json = serde_json::to_string(kind).expect("serialize");
            let restored: ResourceKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *kind);
        }
    }

    #[test]
    fn approval_scope_serde_roundtrip() {
        let scopes = vec![
            ApprovalScope::Once,
            ApprovalScope::Turn,
            ApprovalScope::Session,
            ApprovalScope::PathPrefix(PathBuf::from("/tmp")),
            ApprovalScope::Host("example.com".into()),
            ApprovalScope::CommandPrefix(vec!["npm".into(), "run".into()]),
            ApprovalScope::McpTool {
                server: "srv".into(),
                tool: "t".into(),
            },
        ];
        for scope in &scopes {
            let json = serde_json::to_string(scope).expect("serialize");
            let restored: ApprovalScope = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *scope);
        }
    }

    // ── ToolCategory serde ───────────────────────────────────────

    #[test]
    fn tool_category_serde_roundtrip() {
        for cat in &[
            ToolCategory::ReadOnly,
            ToolCategory::Mutating,
            ToolCategory::Command,
            ToolCategory::BackgroundProcess,
            ToolCategory::UserPrompt,
            ToolCategory::Planning,
            ToolCategory::GoalStatus,
            ToolCategory::Delegation,
            ToolCategory::Web,
            ToolCategory::Internal,
        ] {
            let json = serde_json::to_string(cat).expect("serialize");
            let restored: ToolCategory = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *cat);
        }
    }
}
