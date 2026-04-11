use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::json;
use tokio::fs;
use tracing::debug;

use crate::{Tool, ToolContext, ToolOutput};

const DESCRIPTION: &str = include_str!("apply_patch.txt");

pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "patchText": {
                    "type": "string",
                    "description": "The full patch text that describes all changes to be made"
                }
            },
            "required": ["patchText"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let patch_text = input["patchText"].as_str().unwrap_or("");
        debug!(
            tool = self.name(),
            cwd = %ctx.cwd.display(),
            session_id = %ctx.session_id,
            input = %input,
            patch_text = patch_text,
            patch_text_len = patch_text.len(),
            "received apply_patch request"
        );
        if patch_text.trim().is_empty() {
            debug!("rejecting apply_patch request because patchText is empty");
            return Ok(ToolOutput::error("patchText is required"));
        }

        let patch = parse_patch(patch_text)?;
        debug!(change_count = patch.len(), "parsed apply_patch request");
        if patch.is_empty() {
            let normalized = patch_text
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .trim()
                .to_string();
            if normalized == "*** Begin Patch\n*** End Patch" {
                debug!("rejecting apply_patch request because patch contained no changes");
                return Ok(ToolOutput::error("patch rejected: empty patch"));
            }
            debug!("rejecting apply_patch request because no hunks were found");
            return Ok(ToolOutput::error(
                "apply_patch verification failed: no hunks found",
            ));
        }

        let mut files = Vec::with_capacity(patch.len());
        let mut summary = Vec::with_capacity(patch.len());
        let mut total_diff = String::new();

        for change in &patch {
            let source_path = resolve_relative(&ctx.cwd, &change.path)?;
            let target_path = change
                .move_path
                .as_deref()
                .map(|path| resolve_relative(&ctx.cwd, path))
                .transpose()?;
            debug!(
                kind = %change.kind.as_str(),
                source_path = %source_path.display(),
                target_path = ?target_path.as_ref().map(|path| path.display().to_string()),
                content_len = change.content.len(),
                "prepared apply_patch change"
            );

            let old_content = match change.kind {
                PatchKind::Add => String::new(),
                _ => read_file(&source_path).await?,
            };
            let new_content = match change.kind {
                PatchKind::Add => change.content.clone(),
                PatchKind::Update | PatchKind::Move => apply_hunks(&old_content, &change.hunks)?,
                PatchKind::Delete => String::new(),
            };

            let additions = new_content.lines().count();
            let deletions = old_content.lines().count();
            let relative_path =
                relative_worktree_path(target_path.as_ref().unwrap_or(&source_path), &ctx.cwd);
            let kind_name = change.kind.as_str();
            let diff = format!("--- {}\n+++ {}\n", relative_path, relative_path);

            files.push(json!({
                "filePath": source_path,
                "relativePath": relative_path,
                "type": kind_name,
                "patch": diff,
                "additions": additions,
                "deletions": deletions,
                "movePath": target_path,
            }));
            total_diff.push_str(&diff);
            total_diff.push('\n');

            summary.push(match change.kind {
                PatchKind::Add => format!("A {}", relative_worktree_path(&source_path, &ctx.cwd)),
                PatchKind::Delete => {
                    format!("D {}", relative_worktree_path(&source_path, &ctx.cwd))
                }
                PatchKind::Update | PatchKind::Move => {
                    format!(
                        "M {}",
                        relative_worktree_path(
                            target_path.as_ref().unwrap_or(&source_path),
                            &ctx.cwd
                        )
                    )
                }
            });
        }

        for change in &patch {
            debug!(
                kind = %change.kind.as_str(),
                path = %change.path,
                move_path = ?change.move_path,
                "applying patch change"
            );
            apply_change(&ctx.cwd, change).await?;
        }

        debug!(
            updated_files = summary.len(),
            summary = ?summary,
            "apply_patch completed successfully"
        );
        Ok(ToolOutput {
            content: format!(
                "Success. Updated the following files:\n{}",
                summary.join("\n")
            ),
            is_error: false,
            metadata: Some(json!({
                "diff": total_diff,
                "files": files,
                "diagnostics": {},
            })),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchKind {
    Add,
    Update,
    Delete,
    Move,
}

impl PatchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Move => "move",
        }
    }
}

#[derive(Debug, Clone)]
struct PatchChange {
    path: String,
    move_path: Option<String>,
    content: String,
    hunks: Vec<PatchHunk>,
    kind: PatchKind,
}

#[derive(Debug, Clone)]
struct PatchHunk {
    lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

fn parse_patch(patch_text: &str) -> anyhow::Result<Vec<PatchChange>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.lines().peekable();

    let Some(first_line) = lines.next() else {
        return Ok(Vec::new());
    };
    if first_line != "*** Begin Patch" {
        return Err(anyhow::anyhow!("patch must start with *** Begin Patch"));
    }

    let mut changes = Vec::new();
    let mut saw_end_patch = false;
    while let Some(line) = lines.next() {
        if line == "*** End Patch" {
            saw_end_patch = true;
            break;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let contents = collect_plus_block(&mut lines)?;
            changes.push(PatchChange {
                path: path.to_string(),
                move_path: None,
                content: contents,
                hunks: Vec::new(),
                kind: PatchKind::Add,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            changes.push(PatchChange {
                path: path.to_string(),
                move_path: None,
                content: String::new(),
                hunks: Vec::new(),
                kind: PatchKind::Delete,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let mut move_path = None;
            if matches!(lines.peek(), Some(next) if next.starts_with("*** Move to: ")) {
                let next = lines.next().unwrap_or_default();
                move_path = Some(next.trim_start_matches("*** Move to: ").to_string());
            }
            let hunks = collect_hunk_block(&mut lines)?;
            let kind = if move_path.is_some() {
                PatchKind::Move
            } else {
                PatchKind::Update
            };
            changes.push(PatchChange {
                path: path.to_string(),
                move_path,
                content: String::new(),
                hunks,
                kind,
            });
            continue;
        }

        return Err(anyhow::anyhow!(
            "expected file operation header, got: {line}"
        ));
    }

    if !saw_end_patch {
        return Err(anyhow::anyhow!("patch must end with *** End Patch"));
    }

    Ok(changes)
}

fn is_hunk_header_line(line: &str) -> bool {
    line == "@@" || line.starts_with("@@ ")
}

fn is_hunk_body_line(line: &str) -> bool {
    matches!(line.chars().next(), Some('+') | Some('-') | Some(' '))
}

fn collect_plus_block(
    lines: &mut std::iter::Peekable<std::str::Lines<'_>>,
) -> anyhow::Result<String> {
    let mut content = String::new();
    while let Some(next) = lines.peek() {
        if next.starts_with("*** ") {
            break;
        }
        let line = lines.next().unwrap_or_default();
        if let Some(rest) = line.strip_prefix('+') {
            content.push_str(rest);
            content.push('\n');
        } else {
            return Err(anyhow::anyhow!(
                "add file lines must start with +, got: {line}"
            ));
        }
    }
    Ok(content)
}

fn collect_hunk_block(
    lines: &mut std::iter::Peekable<std::str::Lines<'_>>,
) -> anyhow::Result<Vec<PatchHunk>> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<PatchHunk> = None;
    let mut saw_hunk = false;

    while let Some(next) = lines.peek() {
        if next.starts_with("*** ") && !next.starts_with("*** End of File") {
            break;
        }
        let line = lines.next().unwrap_or_default();
        if line == "*** End of File" {
            break;
        }
        if line.is_empty() {
            if !matches!(lines.peek(), Some(next) if is_hunk_header_line(next) || is_hunk_body_line(next))
            {
                break;
            }
            continue;
        }
        if is_hunk_header_line(line) {
            saw_hunk = true;
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            current_hunk = Some(PatchHunk { lines: Vec::new() });
            continue;
        }
        let Some(hunk) = current_hunk.as_mut() else {
            return Err(anyhow::anyhow!(
                "encountered patch lines before a hunk header"
            ));
        };
        match line.chars().next() {
            Some('+') => hunk.lines.push(HunkLine::Add(line[1..].to_string())),
            Some(' ') => hunk.lines.push(HunkLine::Context(line[1..].to_string())),
            Some('-') => {
                saw_hunk = true;
                hunk.lines.push(HunkLine::Remove(line[1..].to_string()));
            }
            _ => return Err(anyhow::anyhow!("unsupported hunk line: {line}")),
        };
    }

    if let Some(hunk) = current_hunk.take() {
        hunks.push(hunk);
    }

    if !saw_hunk && hunks.iter().all(|hunk| hunk.lines.is_empty()) {
        return Err(anyhow::anyhow!("no hunks found"));
    }

    Ok(hunks)
}

fn resolve_relative(base: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let candidate = Path::new(rel);
    if candidate.is_absolute() {
        return Err(anyhow::anyhow!(
            "file references can only be relative, NEVER ABSOLUTE."
        ));
    }

    let mut out = base.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => out.push(".."),
            Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow::anyhow!(
                    "file references can only be relative, NEVER ABSOLUTE."
                ));
            }
        }
    }
    Ok(out)
}

fn relative_worktree_path(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

async fn read_file(path: &Path) -> anyhow::Result<String> {
    Ok(fs::read_to_string(path).await?)
}

async fn apply_change(base: &Path, change: &PatchChange) -> anyhow::Result<()> {
    let source = resolve_relative(base, &change.path)?;
    match change.kind {
        PatchKind::Add => {
            if let Some(parent) = source.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&source, &change.content).await?;
        }
        PatchKind::Update => {
            let old_content = read_file(&source).await?;
            let new_content = apply_hunks(&old_content, &change.hunks)?;
            fs::write(&source, &new_content).await?;
        }
        PatchKind::Delete => {
            let _ = fs::remove_file(&source).await;
        }
        PatchKind::Move => {
            if let Some(dest) = &change.move_path {
                let dest = resolve_relative(base, dest)?;
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).await?;
                }
                let old_content = read_file(&source).await?;
                let new_content = apply_hunks(&old_content, &change.hunks)?;
                fs::write(&dest, &new_content).await?;
                let _ = fs::remove_file(&source).await;
            }
        }
    }
    Ok(())
}

fn apply_hunks(old_content: &str, hunks: &[PatchHunk]) -> anyhow::Result<String> {
    let old_lines = normalized_lines(old_content);
    let mut output = Vec::new();
    let mut cursor = 0usize;

    for hunk in hunks {
        let start = find_hunk_start(&old_lines, cursor, hunk)?;
        output.extend_from_slice(&old_lines[cursor..start]);
        let mut position = start;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(expected) => {
                    let actual = old_lines.get(position).ok_or_else(|| {
                        anyhow::anyhow!("context line beyond end of file: {expected}")
                    })?;
                    if actual != expected {
                        return Err(anyhow::anyhow!(
                            "context mismatch while applying patch: expected {expected:?}, got {actual:?}"
                        ));
                    }
                    output.push(expected.clone());
                    position += 1;
                }
                HunkLine::Remove(expected) => {
                    let actual = old_lines.get(position).ok_or_else(|| {
                        anyhow::anyhow!("removed line beyond end of file: {expected}")
                    })?;
                    if actual != expected {
                        return Err(anyhow::anyhow!(
                            "remove mismatch while applying patch: expected {expected:?}, got {actual:?}"
                        ));
                    }
                    position += 1;
                }
                HunkLine::Add(line) => output.push(line.clone()),
            }
        }
        cursor = position;
    }

    output.extend_from_slice(&old_lines[cursor..]);
    Ok(if output.is_empty() {
        String::new()
    } else {
        format!("{}\n", output.join("\n"))
    })
}

fn find_hunk_start(old_lines: &[String], cursor: usize, hunk: &PatchHunk) -> anyhow::Result<usize> {
    let expected = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(text) | HunkLine::Remove(text) => Some(text),
            HunkLine::Add(_) => None,
        })
        .collect::<Vec<_>>();

    if expected.is_empty() {
        return Ok(cursor);
    }

    for start in cursor..=old_lines.len().saturating_sub(expected.len()) {
        if expected
            .iter()
            .enumerate()
            .all(|(offset, line)| old_lines.get(start + offset) == Some(&line.to_string()))
        {
            return Ok(start);
        }
    }

    Err(anyhow::anyhow!(
        "failed to locate hunk context in source file"
    ))
}

fn normalized_lines(content: &str) -> Vec<String> {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use clawcr_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{ApplyPatchTool, HunkLine, PatchKind, parse_patch, resolve_relative};
    use crate::{Tool, ToolContext};

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("clawcr-apply-patch-{name}-{nanos}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn make_ctx(cwd: std::path::PathBuf) -> ToolContext {
        ToolContext {
            cwd,
            permissions: Arc::new(RuleBasedPolicy::new(PermissionMode::AutoApprove)),
            session_id: "test-session".into(),
        }
    }

    #[test]
    fn parse_patch_supports_all_change_kinds() {
        let patch = parse_patch(
            "*** Begin Patch
*** Add File: add.txt
+hello
*** Update File: update.txt
@@
-old
+new
*** Delete File: delete.txt
*** Update File: from.txt
*** Move to: to.txt
@@
-before
+after
*** End Patch",
        )
        .expect("parse patch");

        assert_eq!(patch.len(), 4);
        assert_eq!(patch[0].path, "add.txt");
        assert_eq!(patch[0].kind, PatchKind::Add);
        assert_eq!(patch[0].content, "hello\n");

        assert_eq!(patch[1].path, "update.txt");
        assert_eq!(patch[1].kind, PatchKind::Update);
        assert!(patch[1].content.is_empty());
        assert_eq!(patch[1].hunks.len(), 1);
        assert_eq!(
            patch[1].hunks[0].lines,
            vec![
                HunkLine::Remove("old".to_string()),
                HunkLine::Add("new".to_string())
            ]
        );

        assert_eq!(patch[2].path, "delete.txt");
        assert_eq!(patch[2].kind, PatchKind::Delete);

        assert_eq!(patch[3].path, "from.txt");
        assert_eq!(patch[3].move_path.as_deref(), Some("to.txt"));
        assert_eq!(patch[3].kind, PatchKind::Move);
        assert!(patch[3].content.is_empty());
        assert_eq!(patch[3].hunks.len(), 1);
        assert_eq!(
            patch[3].hunks[0].lines,
            vec![
                HunkLine::Remove("before".to_string()),
                HunkLine::Add("after".to_string())
            ]
        );
    }

    #[test]
    fn parse_patch_requires_begin_end_markers() {
        let error = parse_patch(
            "*** Update File: README.md
@@
 **If you find this project useful, please consider giving it a ⭐**
+Bye",
        )
        .expect_err("patch without envelope should fail");

        assert!(error.to_string().contains("*** Begin Patch"));
    }

    #[test]
    fn parse_patch_requires_end_marker() {
        let error = parse_patch(
            "*** Begin Patch
*** Update File: README.md
@@
 **If you find this project useful, please consider giving it a ⭐**
+Bye",
        )
        .expect_err("patch without end marker should fail");

        assert!(error.to_string().contains("*** End Patch"));
    }

    #[test]
    fn parse_patch_rejects_surrounding_log_text() {
        let error = parse_patch(
            "request tool=\"apply_patch\"\ninput={...}\n*** Begin Patch
*** Update File: README.md
@@
 **If you find this project useful, please consider giving it a ⭐**
+Bye
*** End Patch",
        )
        .expect_err("surrounding log text should fail");

        assert!(error.to_string().contains("*** Begin Patch"));
    }

    #[test]
    fn parse_patch_rejects_non_prefixed_add_file_content() {
        let error = parse_patch(
            "*** Begin Patch
*** Add File: hello.txt
hello
*** End Patch",
        )
        .expect_err("non-prefixed add content should fail");

        assert!(error.to_string().contains("must start with +"));
    }

    #[test]
    fn resolve_relative_rejects_absolute_paths() {
        let base = std::path::Path::new("C:\\workspace");

        #[cfg(windows)]
        let path = "C:\\absolute\\file.txt";
        #[cfg(unix)]
        let path = "/absolute/file.txt";

        let error = resolve_relative(base, path).expect_err("absolute path should fail");
        assert!(error.to_string().contains("NEVER ABSOLUTE"));
    }

    #[tokio::test]
    async fn execute_applies_changes_and_returns_summary() {
        let cwd = unique_temp_dir("execute");
        std::fs::write(cwd.join("update.txt"), "old\n").expect("write update file");
        std::fs::write(cwd.join("from.txt"), "before\n").expect("write move source");
        std::fs::write(cwd.join("delete.txt"), "remove me\n").expect("write delete source");
        let ctx = make_ctx(cwd.clone());

        let output = ApplyPatchTool
            .execute(
                &ctx,
                json!({
                    "patchText": "*** Begin Patch
*** Add File: add.txt
+hello
*** Update File: update.txt
@@
-old
+new
*** Delete File: delete.txt
*** Update File: from.txt
*** Move to: moved/to.txt
@@
-before
+after
*** End Patch"
                }),
            )
            .await
            .expect("execute apply_patch");

        assert!(!output.is_error);
        assert!(
            output
                .content
                .contains("Success. Updated the following files:")
        );
        assert!(output.content.contains("A add.txt"));
        assert!(output.content.contains("M update.txt"));
        assert!(output.content.contains("D delete.txt"));
        assert!(output.content.contains("M moved/to.txt"));

        assert_eq!(
            std::fs::read_to_string(cwd.join("add.txt")).expect("read added file"),
            "hello\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("update.txt")).expect("read updated file"),
            "new\n"
        );
        assert!(!cwd.join("delete.txt").exists());
        assert!(!cwd.join("from.txt").exists());
        assert_eq!(
            std::fs::read_to_string(cwd.join("moved").join("to.txt")).expect("read moved file"),
            "after\n"
        );

        let metadata = output.metadata.expect("metadata");
        let files = metadata["files"].as_array().expect("files metadata");
        assert_eq!(files.len(), 4);
        assert_eq!(files[0]["additions"], 1);
        assert_eq!(files[0]["deletions"], 0);
        assert_eq!(files[1]["additions"], 1);
        assert_eq!(files[1]["deletions"], 1);
        assert_eq!(files[2]["additions"], 0);
        assert_eq!(files[2]["deletions"], 1);
        assert_eq!(files[3]["additions"], 1);
        assert_eq!(files[3]["deletions"], 1);
    }

    #[tokio::test]
    async fn execute_rejects_empty_patch() {
        let cwd = unique_temp_dir("empty");
        let ctx = make_ctx(cwd);

        let output = ApplyPatchTool
            .execute(
                &ctx,
                json!({
                    "patchText": "*** Begin Patch\n*** End Patch"
                }),
            )
            .await
            .expect("execute apply_patch");

        assert!(output.is_error);
        assert_eq!(output.content, "patch rejected: empty patch");
    }
}
