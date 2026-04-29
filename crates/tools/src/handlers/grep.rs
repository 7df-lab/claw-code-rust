use async_trait::async_trait;
use std::path::PathBuf;
use tracing::debug;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct GrepHandler;

#[async_trait]
impl ToolHandler for GrepHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::Grep
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let pattern_str = invocation.input["pattern"].as_str().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: "missing 'pattern' field".into(),
            }
        })?;

        let case_insensitive = invocation.input["case_insensitive"]
            .as_bool()
            .unwrap_or(false);

        let re = {
            let mut builder = regex::RegexBuilder::new(pattern_str);
            builder.case_insensitive(case_insensitive);
            match builder.build() {
                Ok(r) => r,
                Err(e) => {
                    return Ok(Box::new(FunctionToolOutput::error(format!(
                        "invalid regex: {e}"
                    ))));
                }
            }
        };

        let base = match invocation.input["path"].as_str() {
            Some(p) => {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    invocation.cwd.join(pb)
                }
            }
            None => invocation.cwd.clone(),
        };

        let glob_pattern = invocation.input["glob"].as_str();
        debug!(pattern = pattern_str, base = %base.display(), "grep search");

        let files = collect_files(&base, glob_pattern);
        let mut results: Vec<String> = Vec::new();
        const MAX_RESULTS: usize = 500;

        'outer: for file in &files {
            let content = match tokio::fs::read_to_string(file).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (lineno, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(format!(
                        "{}:{}:{}",
                        file.to_string_lossy(),
                        lineno + 1,
                        line
                    ));
                    if results.len() >= MAX_RESULTS {
                        results.push(format!("(truncated at {} matches)", MAX_RESULTS));
                        break 'outer;
                    }
                }
            }
        }

        if results.is_empty() {
            return Ok(Box::new(FunctionToolOutput::success("(no matches)")));
        }

        Ok(Box::new(FunctionToolOutput::success(results.join("\n"))))
    }
}

fn collect_files(base: &std::path::Path, glob_pattern: Option<&str>) -> Vec<PathBuf> {
    let pattern = match glob_pattern {
        Some(g) => base.join("**").join(g).to_string_lossy().to_string(),
        None => base.join("**").join("*").to_string_lossy().to_string(),
    };

    glob::glob(&pattern)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|p| p.is_file())
        .collect()
}
