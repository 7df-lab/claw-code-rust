//! Full old/new file text for path-scoped Full expands.
//!
//! `@pierre/diffs` only enables expand-up/down when diffs are built from full
//! file sides (`isPartial: false`). Path-scoped Full already returns a unified
//! patch; this module optionally attaches `old_text`/`new_text` onto matching
//! `WorkspaceChangedFile` rows.

use std::path::Path;
use std::process::Output;

use devo_protocol::{WorkspaceChangeBase, WorkspaceChangeScope, WorkspaceChangeView};
use tokio::process::Command;

/// Cap each side so expand never ships multi-MB blobs over stdio.
pub(crate) const MAX_SIDE_CHARS: usize = 400_000;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SideRefs<'a> {
    pub old: Option<&'a str>,
    pub new: SideSource<'a>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SideSource<'a> {
    /// `git show <spec>` where `spec` is `REV:path` or `:path` (index).
    GitShow(&'a str),
    /// Read the worktree file at `repo_root/path`.
    Worktree,
}

/// Resolve git-show / worktree refs for a scope + optional base metadata.
pub(crate) fn side_refs_for_scope<'a>(
    scope: WorkspaceChangeScope,
    base: Option<&'a WorkspaceChangeBase>,
    turn_checkpoint_override: Option<&'a str>,
) -> Option<SideRefs<'a>> {
    match scope {
        WorkspaceChangeScope::Branch => {
            let Some(WorkspaceChangeBase::Branch { merge_base, .. }) = base else {
                return None;
            };
            Some(SideRefs {
                old: Some(merge_base.as_str()),
                new: SideSource::GitShow("HEAD"),
            })
        }
        WorkspaceChangeScope::Staged => Some(SideRefs {
            old: Some("HEAD"),
            new: SideSource::GitShow(""),
        }),
        WorkspaceChangeScope::Unstaged => Some(SideRefs {
            old: Some(""),
            new: SideSource::Worktree,
        }),
        WorkspaceChangeScope::Uncommitted => Some(SideRefs {
            old: Some("HEAD"),
            new: SideSource::Worktree,
        }),
        WorkspaceChangeScope::Turn => {
            let checkpoint = turn_checkpoint_override.or(match base {
                Some(WorkspaceChangeBase::TurnCheckpoint { checkpoint_id, .. }) => {
                    Some(checkpoint_id.as_str())
                }
                _ => None,
            })?;
            Some(SideRefs {
                old: Some(checkpoint),
                new: SideSource::Worktree,
            })
        }
    }
}

/// Attach `old_text`/`new_text` onto each file in a path-scoped Full view.
///
/// Oversize or binary sides are omitted entirely (PatchDiff fallback). When a
/// side is truncated we also set `diff_truncated` and add a warning.
pub(crate) async fn attach_file_sides(
    mut view: WorkspaceChangeView,
    turn_checkpoint_override: Option<&str>,
) -> WorkspaceChangeView {
    let Some(refs) = side_refs_for_scope(view.scope, view.base.as_ref(), turn_checkpoint_override)
    else {
        return view;
    };
    let repo_root = view.workspace_root.clone();
    let mut truncated_any = false;

    for file in &mut view.files {
        if file.binary {
            file.old_text = None;
            file.new_text = None;
            continue;
        }
        let path = file.path.to_string_lossy().replace('\\', "/");
        if path.is_empty() {
            continue;
        }

        let (old, old_truncated) = match refs.old {
            Some(rev) => read_git_blob_text(&repo_root, rev, &path).await,
            None => (None, false),
        };
        let (new, new_truncated) = match refs.new {
            SideSource::GitShow(rev) => read_git_blob_text(&repo_root, rev, &path).await,
            SideSource::Worktree => read_worktree_text(&repo_root, &path).await,
        };

        if old_truncated || new_truncated {
            truncated_any = true;
            file.diff_truncated = true;
            file.old_text = None;
            file.new_text = None;
            continue;
        }

        // Binary detection from either side → omit both.
        if matches!(
            (&old, &new),
            (Some(SideRead::Binary), _) | (_, Some(SideRead::Binary))
        ) {
            file.binary = true;
            file.old_text = None;
            file.new_text = None;
            continue;
        }

        let old_text = old.and_then(SideRead::into_text);
        let new_text = new.and_then(SideRead::into_text);

        // Need at least one present side to drive MultiFileDiff.
        if old_text.is_none() && new_text.is_none() {
            file.old_text = None;
            file.new_text = None;
            continue;
        }

        file.old_text = old_text;
        file.new_text = new_text;
    }

    if truncated_any {
        view.warnings.push("file_sides_truncated".to_string());
        view.warnings.sort();
        view.warnings.dedup();
    }
    view
}

#[derive(Debug)]
enum SideRead {
    Text(String),
    Binary,
}

impl SideRead {
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::Binary => None,
        }
    }
}

/// `rev` empty string means index blob (`:path`).
async fn read_git_blob_text(repo_root: &Path, rev: &str, path: &str) -> (Option<SideRead>, bool) {
    let spec = if rev.is_empty() {
        format!(":{path}")
    } else {
        format!("{rev}:{path}")
    };
    match git_stdout_bytes(repo_root, &["show".to_string(), spec]).await {
        Ok(bytes) => decode_side_bytes(&bytes),
        Err(_) => (None, false),
    }
}

async fn read_worktree_text(repo_root: &Path, path: &str) -> (Option<SideRead>, bool) {
    let abs = repo_root.join(path);
    match tokio::fs::read(&abs).await {
        Ok(bytes) => decode_side_bytes(&bytes),
        Err(_) => (None, false),
    }
}

fn decode_side_bytes(bytes: &[u8]) -> (Option<SideRead>, bool) {
    if bytes.contains(&0) {
        return (Some(SideRead::Binary), false);
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return (Some(SideRead::Binary), false);
    };
    // Normalize EOLs so expand context matches git patches (usually LF) and
    // client-side jsdiff fallbacks do not treat every line as changed.
    let normalized = normalize_newlines(text);
    if normalized.len() > MAX_SIDE_CHARS {
        return (
            Some(SideRead::Text(normalized[..MAX_SIDE_CHARS].to_string())),
            true,
        );
    }
    (Some(SideRead::Text(normalized)), false)
}

fn normalize_newlines(text: &str) -> String {
    if !text.as_bytes().contains(&b'\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

async fn git_stdout_bytes(cwd: &Path, args: &[String]) -> Result<Vec<u8>, String> {
    let output = git_output(cwd, args).await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn git_output(cwd: &Path, args: &[String]) -> Result<Output, String> {
    Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn branch_refs_use_merge_base_and_head() {
        let base = WorkspaceChangeBase::Branch {
            base_branch: "main".into(),
            merge_base: "abc123".into(),
            head: "def456".into(),
        };
        let refs = side_refs_for_scope(WorkspaceChangeScope::Branch, Some(&base), None).unwrap();
        assert_eq!(refs.old, Some("abc123"));
        assert!(matches!(refs.new, SideSource::GitShow("HEAD")));
    }

    #[test]
    fn staged_new_side_is_index() {
        let refs = side_refs_for_scope(WorkspaceChangeScope::Staged, None, None).unwrap();
        assert_eq!(refs.old, Some("HEAD"));
        assert!(matches!(refs.new, SideSource::GitShow("")));
    }

    #[test]
    fn unstaged_old_side_is_index() {
        let refs = side_refs_for_scope(WorkspaceChangeScope::Unstaged, None, None).unwrap();
        assert_eq!(refs.old, Some(""));
        assert!(matches!(refs.new, SideSource::Worktree));
    }

    #[test]
    fn turn_refs_use_checkpoint_override() {
        let refs = side_refs_for_scope(WorkspaceChangeScope::Turn, None, Some("ckpt123")).unwrap();
        assert_eq!(refs.old, Some("ckpt123"));
        assert!(matches!(refs.new, SideSource::Worktree));
    }

    #[test]
    fn decode_rejects_nul_as_binary() {
        let (side, truncated) = decode_side_bytes(b"a\0b");
        assert!(matches!(side, Some(SideRead::Binary)));
        assert!(!truncated);
    }

    #[test]
    fn decode_marks_oversize_truncated() {
        let big = "x".repeat(MAX_SIDE_CHARS + 10);
        let (side, truncated) = decode_side_bytes(big.as_bytes());
        assert!(truncated);
        assert!(matches!(side, Some(SideRead::Text(_))));
    }

    #[test]
    fn decode_normalizes_crlf_and_lone_cr_to_lf() {
        let (side, truncated) = decode_side_bytes(b"a\r\nb\rc\n");
        assert!(!truncated);
        match side {
            Some(SideRead::Text(text)) => assert_eq!(text, "a\nb\nc\n"),
            other => panic!("expected text side, got {other:?}"),
        }
    }

    #[test]
    fn normalize_newlines_leaves_lf_untouched() {
        assert_eq!(normalize_newlines("a\nb\n"), "a\nb\n");
    }
}
