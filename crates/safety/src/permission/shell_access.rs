//! Detect file reads and writes embedded in shell commands.

use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use tree_sitter::Node;

use super::CompiledPolicy;
use super::PermissionAccess;
use super::PolicyDecision;
use super::bash_command_splitting::ParsedCommand;
use super::bash_command_splitting::all_commands_from_script;
use super::bash_command_splitting::basename;
use super::bash_command_splitting::shell_dash_c_script;
use super::bash_command_splitting::unwrap_wrappers;
use super::file_access_model::ExtractedAccess;
use super::file_access_model::FileMode;
use super::file_access_model::command_file_accesses;
use super::file_access_model::extracted_path;

#[derive(Debug, Clone, Copy)]
enum PathDecisionMode {
    Direct,
    Shell,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum UnresolvedSymlinkPolicy {
    Ask,
    Ignore,
}

pub(super) fn evaluate_native_file_access(
    policy: &CompiledPolicy,
    access: &PermissionAccess,
    unresolved_symlink: UnresolvedSymlinkPolicy,
) -> PolicyDecision {
    let (mode, path, cwd) = match access {
        PermissionAccess::Read {
            path: Some(path),
            cwd,
        } => (FileMode::Read, path, cwd),
        PermissionAccess::Grep {
            path: Some(path),
            glob,
            cwd,
            recursive,
        } => (
            FileMode::Grep {
                glob: glob.clone(),
                recursive: *recursive,
            },
            path,
            cwd,
        ),
        PermissionAccess::Edit { path, cwd } => (FileMode::Edit, path, cwd),
        PermissionAccess::Read { path: None, cwd: _ }
        | PermissionAccess::Grep {
            path: None,
            glob: _,
            cwd: _,
            recursive: false,
        }
        | PermissionAccess::Bash { .. }
        | PermissionAccess::Mcp { .. }
        | PermissionAccess::WebFetch(_)
        | PermissionAccess::WebSearch(_) => return PolicyDecision::NoMatch,
        PermissionAccess::Grep {
            path: None,
            glob,
            cwd,
            recursive: true,
        } => {
            return evaluate_path(
                policy,
                &FileMode::Grep {
                    glob: glob.clone(),
                    recursive: true,
                },
                Path::new("."),
                cwd,
                PathDecisionMode::Direct,
                unresolved_symlink,
            );
        }
    };
    evaluate_path(
        policy,
        &mode,
        path,
        cwd,
        PathDecisionMode::Direct,
        unresolved_symlink,
    )
}

pub(super) fn evaluate_shell_file_access(
    policy: &CompiledPolicy,
    command: &str,
    cwd: &Path,
) -> PolicyDecision {
    evaluate_shell_file_access_at_depth(policy, command, cwd, 0)
}

/// Evaluate a shell command against explicit readable/writable root sets,
/// without using the rule engine. Returns `Ask` when the command cannot be
/// fully analyzed or touches a path outside the allowed roots, and `NoMatch`
/// when every detected file access stays within the roots.
pub fn evaluate_shell_access_with_roots(
    command: &str,
    cwd: &Path,
    readable_roots: &BTreeSet<PathBuf>,
    writable_roots: &BTreeSet<PathBuf>,
) -> PolicyDecision {
    evaluate_shell_access_with_roots_at_depth(command, cwd, readable_roots, writable_roots, 0)
}

fn evaluate_shell_access_with_roots_at_depth(
    command: &str,
    cwd: &Path,
    readable_roots: &BTreeSet<PathBuf>,
    writable_roots: &BTreeSet<PathBuf>,
    depth: usize,
) -> PolicyDecision {
    const MAX_NESTING: usize = 8;
    if depth >= MAX_NESTING {
        return PolicyDecision::Ask;
    }
    let Some(commands) = all_commands_from_script(command) else {
        return PolicyDecision::Ask;
    };
    let Some(tree) = devo_util_shell_command::bash::try_parse_shell(command) else {
        return PolicyDecision::Ask;
    };
    if tree.root_node().has_error() {
        return PolicyDecision::Ask;
    }

    let mut events = Vec::new();
    for parsed in &commands {
        events.push((parsed.start_byte(), 0, ShellEvent::Command(parsed)));
        if command_changes_cwd(parsed.words()) {
            events.push((parsed.end_byte(), 2, ShellEvent::CwdChange(parsed)));
        }
    }
    for (position, redirect) in redirect_accesses(tree.root_node(), command) {
        events.push((position, 1, ShellEvent::Redirect(redirect)));
    }
    events.sort_by_key(|(position, priority, _)| (*position, *priority));

    let mut decision = PolicyDecision::NoMatch;
    let mut effective_cwd = Some(cwd.to_path_buf());
    for (_, _, event) in events {
        match event {
            ShellEvent::Command(parsed) => {
                let raw = parsed.words();
                let words = unwrap_wrappers(raw);
                if wrapper_changes_cwd(raw) {
                    decision = decision.combine(PolicyDecision::Ask);
                }
                if !command_changes_cwd(raw) {
                    for access in command_file_accesses(words) {
                        decision = decision.combine(evaluate_access_with_roots(
                            access,
                            effective_cwd.as_deref(),
                            readable_roots,
                            writable_roots,
                            AccessSource::Command,
                        ));
                    }
                }
                if let Some(script) = shell_dash_c_script(words) {
                    decision = decision.combine(match effective_cwd.as_deref() {
                        Some(current_cwd) => evaluate_shell_access_with_roots_at_depth(
                            script,
                            current_cwd,
                            readable_roots,
                            writable_roots,
                            depth + 1,
                        ),
                        None => PolicyDecision::Ask,
                    });
                }
            }
            ShellEvent::Redirect(access) => {
                decision = decision.combine(evaluate_access_with_roots(
                    access,
                    effective_cwd.as_deref(),
                    readable_roots,
                    writable_roots,
                    AccessSource::Redirect,
                ));
            }
            ShellEvent::CwdChange(parsed) => {
                effective_cwd = apply_cwd_change(parsed.words(), effective_cwd.as_deref());
                if effective_cwd.is_none() {
                    decision = decision.combine(PolicyDecision::Ask);
                }
            }
        }
    }
    decision
}

#[derive(Debug, Clone, Copy)]
enum AccessSource {
    Command,
    Redirect,
}

fn evaluate_access_with_roots(
    access: ExtractedAccess,
    cwd: Option<&Path>,
    readable_roots: &BTreeSet<PathBuf>,
    writable_roots: &BTreeSet<PathBuf>,
    source: AccessSource,
) -> PolicyDecision {
    match access {
        ExtractedAccess::Path { mode, path } => {
            let path = normalize_windows_drive_relative_path(path);
            #[cfg(windows)]
            let mut path = path;
            let path_buf = Path::new(&path);
            let Some(cwd) = cwd.or_else(|| path_buf.is_absolute().then(|| Path::new("/"))) else {
                return PolicyDecision::Ask;
            };
            // If we got a rooted Windows path like `\workspace\file.txt` (no drive
            // prefix), treat it as absolute on the same drive as `cwd`.
            #[cfg(windows)]
            {
                if (path.starts_with('\\') || path.starts_with('/'))
                    && path.as_bytes().get(1).is_none_or(|b| *b != b':')
                    && let Some(prefix) = cwd.components().find_map(|c| match c {
                        Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
                        _ => None,
                    })
                {
                    let rest = path.trim_start_matches(['\\', '/']);
                    path = format!("{prefix}\\{rest}");
                }
            }
            let path = Path::new(&path);
            let absolute = if path.is_absolute() {
                lexical_normalize(path)
            } else {
                lexical_normalize(&cwd.join(path))
            };
            // Prefer the symlink-resolved path so a workspace symlink to /etc
            // cannot match writable_roots via lexical prefix alone.
            let absolute = resolve_following_symlinks(&absolute, 0).unwrap_or(absolute);
            let allowed = match mode {
                FileMode::Read | FileMode::Grep { .. } => {
                    path_matches_roots(&absolute, readable_roots)
                        || path_matches_roots(&absolute, writable_roots)
                }
                FileMode::Edit => path_matches_roots(&absolute, writable_roots),
            };
            if allowed {
                PolicyDecision::NoMatch
            } else {
                PolicyDecision::Ask
            }
        }
        ExtractedAccess::Ambiguous => match source {
            // Ambiguous commands might not touch files at all; do not force
            // approval merely because the analyzer is unsure.
            AccessSource::Command => PolicyDecision::NoMatch,
            // An ambiguous redirect target is definitely a file access we
            // cannot check, so it must ask.
            AccessSource::Redirect => PolicyDecision::Ask,
        },
    }
}

#[cfg(windows)]
fn normalize_windows_drive_relative_path(path: String) -> String {
    // Some bash grammars can cause backslashes to be consumed, producing a
    // drive-relative path like `C:workspace\\file.txt` instead of
    // `C:\\workspace\\file.txt`.
    //
    // Treat `X:foo` (where the 3rd byte is not `\\` or `/`) as `X:\\foo` so the
    // subsequent root-prefix matching can work reliably.
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[1] == b':'
        && bytes[0].is_ascii_alphabetic()
        && bytes[2] != b'\\'
        && bytes[2] != b'/'
    {
        let drive = &path[0..1];
        let rest = &path[2..];
        format!("{drive}:\\{rest}")
    } else {
        path
    }
}

#[cfg(not(windows))]
fn normalize_windows_drive_relative_path(path: String) -> String {
    path
}

fn path_matches_roots(path: &Path, roots: &BTreeSet<PathBuf>) -> bool {
    roots.iter().any(|root| {
        if path.starts_with(root) {
            return true;
        }
        #[cfg(windows)]
        {
            // Be resilient to small string-shape differences introduced by shell
            // parsing (e.g. `\` vs `/`, or different drive-letter casing).
            let path_s_raw = {
                let s = path.to_string_lossy();
                if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                    format!(r"\\{rest}")
                } else if let Some(rest) = s.strip_prefix(r"\\?\") {
                    rest.to_string()
                } else {
                    s.into_owned()
                }
            };
            let root_s_raw = {
                let s = root.to_string_lossy();
                if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                    format!(r"\\{rest}")
                } else if let Some(rest) = s.strip_prefix(r"\\?\") {
                    rest.to_string()
                } else {
                    s.into_owned()
                }
            };

            let path_s = path_s_raw.replace('\\', "/").to_ascii_lowercase();
            let root_s = root_s_raw.replace('\\', "/").to_ascii_lowercase();
            if path_s == root_s {
                return true;
            }
            if let Some(rest) = path_s.strip_prefix(&root_s) {
                // Ensure component boundary: `C:\workspace` should not match
                // `C:\workspace2`.
                return rest.starts_with('/');
            }
        }
        false
    })
}

fn evaluate_shell_file_access_at_depth(
    policy: &CompiledPolicy,
    command: &str,
    cwd: &Path,
    depth: usize,
) -> PolicyDecision {
    const MAX_NESTING: usize = 8;
    if depth >= MAX_NESTING {
        return PolicyDecision::Ask;
    }
    let Some(commands) = all_commands_from_script(command) else {
        return PolicyDecision::Ask;
    };
    let Some(tree) = devo_util_shell_command::bash::try_parse_shell(command) else {
        return PolicyDecision::Ask;
    };
    if tree.root_node().has_error() {
        return PolicyDecision::Ask;
    }

    let mut events = Vec::new();
    for parsed in &commands {
        events.push((parsed.start_byte(), 0, ShellEvent::Command(parsed)));
        if command_changes_cwd(parsed.words()) {
            events.push((parsed.end_byte(), 2, ShellEvent::CwdChange(parsed)));
        }
    }
    for (position, redirect) in redirect_accesses(tree.root_node(), command) {
        events.push((position, 1, ShellEvent::Redirect(redirect)));
    }
    events.sort_by_key(|(position, priority, _)| (*position, *priority));

    let mut decision = PolicyDecision::NoMatch;
    let mut effective_cwd = Some(cwd.to_path_buf());
    for (_, _, event) in events {
        match event {
            ShellEvent::Command(parsed) => {
                let raw = parsed.words();
                let words = unwrap_wrappers(raw);
                if wrapper_changes_cwd(raw) {
                    decision = decision.combine(PolicyDecision::Ask);
                }
                if !command_changes_cwd(raw) {
                    for access in command_file_accesses(words) {
                        decision = decision.combine(evaluate_extracted_access(
                            policy,
                            access,
                            effective_cwd.as_deref(),
                        ));
                    }
                }
                if let Some(script) = shell_dash_c_script(words) {
                    decision = decision.combine(match effective_cwd.as_deref() {
                        Some(current_cwd) => evaluate_shell_file_access_at_depth(
                            policy,
                            script,
                            current_cwd,
                            depth + 1,
                        ),
                        None => PolicyDecision::Ask,
                    });
                }
            }
            ShellEvent::Redirect(access) => {
                decision = decision.combine(evaluate_extracted_access(
                    policy,
                    access,
                    effective_cwd.as_deref(),
                ));
            }
            ShellEvent::CwdChange(parsed) => {
                effective_cwd = apply_cwd_change(parsed.words(), effective_cwd.as_deref());
                if effective_cwd.is_none() {
                    decision = decision.combine(PolicyDecision::Ask);
                }
            }
        }
    }
    decision
}

enum ShellEvent<'a> {
    Command(&'a ParsedCommand),
    Redirect(ExtractedAccess),
    CwdChange(&'a ParsedCommand),
}

fn evaluate_extracted_access(
    policy: &CompiledPolicy,
    access: ExtractedAccess,
    cwd: Option<&Path>,
) -> PolicyDecision {
    match access {
        ExtractedAccess::Path { mode, path } => {
            let path = Path::new(&path);
            let Some(cwd) = cwd.or_else(|| path.is_absolute().then(|| Path::new("/"))) else {
                return PolicyDecision::Ask;
            };
            evaluate_path(
                policy,
                &mode,
                path,
                cwd,
                PathDecisionMode::Shell,
                UnresolvedSymlinkPolicy::Ask,
            )
        }
        ExtractedAccess::Ambiguous => PolicyDecision::Ask,
    }
}

fn command_changes_cwd(words: &[String]) -> bool {
    unwrap_wrappers(words)
        .first()
        .is_some_and(|program| matches!(basename(program), "cd" | "pushd" | "popd"))
}

fn apply_cwd_change(words: &[String], cwd: Option<&Path>) -> Option<PathBuf> {
    let words = unwrap_wrappers(words);
    let program = basename(words.first()?);
    if !matches!(program, "cd") {
        return None;
    }
    let target = match words.get(1..)? {
        [target] => target.as_str(),
        [option, target] if option == "--" => target.as_str(),
        _ => return None,
    };
    if target == "-" || target.is_empty() {
        return None;
    }
    let target = Path::new(target);
    if target.is_absolute() {
        Some(lexical_normalize(target))
    } else {
        Some(lexical_normalize(&cwd?.join(target)))
    }
}

fn redirect_accesses(root: Node<'_>, source: &str) -> Vec<(usize, ExtractedAccess)> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "file_redirect"
            && let Some(access) = redirect_access(node, source)
        {
            found.push((node.start_byte(), access));
        }
        for index in 0..node.child_count() {
            if let Some(child) = node.child(index) {
                stack.push(child);
            }
        }
    }
    found
}

fn redirect_access(node: Node<'_>, source: &str) -> Option<ExtractedAccess> {
    let mut operator = None;
    for index in 0..node.child_count() {
        let kind = node.child(index)?.kind();
        if kind.contains("<<") {
            return None;
        }
        if kind.contains('>') || kind.contains('<') {
            operator = Some(kind);
            break;
        }
    }
    let operator = operator?;
    let mode = if operator.contains('>') {
        FileMode::Edit
    } else {
        FileMode::Read
    };
    let destination = node.child_by_field_name("destination")?;
    let Some(path) = literal_node(destination, source) else {
        return Some(ExtractedAccess::Ambiguous);
    };
    if path.is_empty()
        || path.starts_with('&')
        || (matches!(operator, ">&" | "<&")
            && (path == "-" || path.bytes().all(|byte| byte.is_ascii_digit())))
    {
        return None;
    }
    extracted_path(mode, path)
}

fn literal_node(node: Node<'_>, source: &str) -> Option<String> {
    if has_expansion(node) {
        return None;
    }
    let raw = node.utf8_text(source.as_bytes()).ok()?;
    // Normalize literal destination tokens:
    // - bash tokens sometimes include surrounding quotes
    // - tree-sitter grammar variants can return different node kinds
    // We trim quotes from the edges so prefix-matching writable roots works.
    let raw = raw.trim_matches(|ch| ch == '\'' || ch == '"');
    match node.kind() {
        "word" | "number" | "concatenation" => Some(raw.to_string()),
        "raw_string" => Some(
            raw.strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .unwrap_or(raw)
                .to_string(),
        ),
        "string" => Some(
            raw.strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(raw)
                .to_string(),
        ),
        // tree-sitter's bash grammar can emit different node kinds for
        // "destination" when the input contains Windows-style paths (e.g.
        // `C:\Users\...`). Since we already block known expansions above, it's
        // safe to fall back to the raw text for any other node kind.
        _ => Some(raw.to_string()),
    }
}

fn has_expansion(node: Node<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        for index in 0..current.child_count() {
            let Some(child) = current.child(index) else {
                continue;
            };
            if matches!(
                child.kind(),
                "expansion"
                    | "simple_expansion"
                    | "command_substitution"
                    | "arithmetic_expansion"
                    | "process_substitution"
            ) {
                return true;
            }
            stack.push(child);
        }
    }
    false
}

fn evaluate_path(
    policy: &CompiledPolicy,
    mode: &FileMode,
    path: &Path,
    cwd: &Path,
    decision_mode: PathDecisionMode,
    unresolved_symlink: UnresolvedSymlinkPolicy,
) -> PolicyDecision {
    let raw = path.to_path_buf();
    let raw_absolute = if raw.is_absolute() {
        raw.clone()
    } else {
        cwd.join(&raw)
    };
    let absolute = if raw.is_absolute() {
        lexical_normalize(&raw)
    } else {
        lexical_normalize(&raw_absolute)
    };
    let mut decision = path_decision(policy, mode, raw, cwd, decision_mode).combine(path_decision(
        policy,
        mode,
        absolute.clone(),
        cwd,
        decision_mode,
    ));
    match resolve_following_symlinks(&raw_absolute, 0) {
        Some(resolved) if resolved != absolute => {
            decision = decision.combine(path_decision(policy, mode, resolved, cwd, decision_mode));
        }
        Some(_) => {}
        None if matches!(unresolved_symlink, UnresolvedSymlinkPolicy::Ask)
            && path_has_symlink(&raw_absolute) =>
        {
            decision = decision.combine(PolicyDecision::Ask);
        }
        None => {}
    }
    decision
}

fn path_decision(
    policy: &CompiledPolicy,
    mode: &FileMode,
    path: PathBuf,
    cwd: &Path,
    decision_mode: PathDecisionMode,
) -> PolicyDecision {
    let access = match mode {
        FileMode::Read => PermissionAccess::Read {
            path: Some(path),
            cwd: cwd.to_path_buf(),
        },
        FileMode::Grep { glob, recursive } => PermissionAccess::Grep {
            path: Some(path),
            glob: glob.clone(),
            cwd: cwd.to_path_buf(),
            recursive: *recursive,
        },
        FileMode::Edit => PermissionAccess::Edit {
            path,
            cwd: cwd.to_path_buf(),
        },
    };
    let direct = policy.evaluate_rules(&access);
    let recursive = matches!(
        mode,
        FileMode::Grep {
            recursive: true,
            ..
        }
    )
    .then(|| policy.evaluate_recursive_scope(&access))
    .unwrap_or(PolicyDecision::NoMatch);
    match decision_mode {
        PathDecisionMode::Direct => direct.combine(recursive),
        PathDecisionMode::Shell => direct.combine(recursive).escalation_only(),
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component);
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component);
            }
        }
    }
    normalized
}

fn path_has_symlink(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if std::fs::symlink_metadata(&prefix)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return true;
        }
    }
    false
}

fn resolve_following_symlinks(path: &Path, depth: usize) -> Option<PathBuf> {
    const MAX_SYMLINK_DEPTH: usize = 40;
    if depth > MAX_SYMLINK_DEPTH || !path.is_absolute() {
        return None;
    }
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Some(lexical_normalize(&canonical));
    }
    let parent = path.parent()?;
    let file_name = path.file_name()?;
    let resolved_parent = resolve_following_symlinks(parent, depth + 1)?;
    let candidate = resolved_parent.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&candidate)
        && metadata.file_type().is_symlink()
    {
        let target = std::fs::read_link(&candidate).ok()?;
        let target = if target.is_absolute() {
            target
        } else {
            resolved_parent.join(target)
        };
        return resolve_following_symlinks(&target, depth + 1);
    }
    Some(lexical_normalize(&candidate))
}

fn wrapper_changes_cwd(words: &[String]) -> bool {
    let mut current = words;
    for _ in 0..8 {
        if current.first().is_some_and(|word| basename(word) == "env")
            && current.iter().any(|word| {
                matches!(word.as_str(), "-C" | "--chdir")
                    || word.starts_with("--chdir=")
                    || word
                        .strip_prefix("-C")
                        .is_some_and(|value| !value.is_empty())
            })
        {
            return true;
        }
        let inner = unwrap_wrappers(current);
        if inner.len() == current.len() {
            break;
        }
        current = inner;
    }
    false
}
