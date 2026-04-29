use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct SkillHandler;

#[async_trait]
impl ToolHandler for SkillHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Skill
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let name = invocation.input["name"].as_str().unwrap_or("");

        let found = find_skill(&invocation.cwd, name).ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Skill \"{name}\" not found"),
            }
        })?;

        let content =
            fs::read_to_string(&found)
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to read skill: {e}"),
                })?;

        let dir = found.parent().unwrap_or(Path::new("")).to_path_buf();
        let files = sample_files(&dir);
        let file_list = files.join("\n");

        Ok(Box::new(FunctionToolOutput::success(format!(
            "<skill_content name=\"{name}\">\n# Skill: {name}\n\n{content}\n\nBase directory for this skill: {}\nRelative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\nNote: file list is sampled.\n\n<skill_files>\n{file_list}\n</skill_files>\n</skill_content>",
            dir.display(),
        ))))
    }
}

fn find_skill(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|x| x.to_str()) == Some("SKILL.md")
                    && path.parent()?.file_name().and_then(|x| x.to_str()) == Some(name)
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn sample_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|x| x.to_str()) == Some("SKILL.md") {
                continue;
            }
            files.push(format!("<file>{}</file>", path.display()));
            if files.len() >= 10 {
                break;
            }
        }
    }
    files
}
