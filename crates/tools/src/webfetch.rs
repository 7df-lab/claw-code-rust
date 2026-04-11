use async_trait::async_trait;
use base64::Engine;
use serde_json::json;
use tokio::time::{Duration, timeout};

use crate::{Tool, ToolContext, ToolOutput};

const DESCRIPTION: &str = include_str!("webfetch.txt");
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string"},
                "format": {"type": "string", "enum": ["text", "markdown", "html"], "default": "markdown"},
                "timeout": {"type": "number"}
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let url = input["url"].as_str().unwrap_or("");
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(ToolOutput::error("URL must start with http:// or https://"));
        }

        let format = input["format"].as_str().unwrap_or("markdown");
        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS / 1000)
            .saturating_mul(1000)
            .min(MAX_TIMEOUT_MS);

        let accept = match format {
            "markdown" => {
                "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1"
            }
            "text" => "text/plain;q=1.0, text/markdown;q=0.9, text/html;q=0.8, */*;q=0.1",
            "html" => {
                "text/html;q=1.0, application/xhtml+xml;q=0.9, text/plain;q=0.8, text/markdown;q=0.7, */*;q=0.1"
            }
            _ => "*/*",
        };

        let client = reqwest::Client::builder().user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36").build()?;
        let request = client
            .get(url)
            .header(reqwest::header::ACCEPT, accept)
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");

        let response = timeout(Duration::from_millis(timeout_ms), request.send()).await;
        let response = match response {
            Ok(result) => result?,
            Err(_) => return Ok(ToolOutput::error("Request timed out")),
        };

        if !response.status().is_success() {
            return Ok(ToolOutput::error(format!(
                "Request failed with status code: {}",
                response.status()
            )));
        }

        if response
            .content_length()
            .is_some_and(|len| len as usize > MAX_RESPONSE_SIZE)
        {
            return Ok(ToolOutput::error("Response too large (exceeds 5MB limit)"));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let title = format!("{url} ({content_type})");

        let bytes = response.bytes().await?;
        if bytes.len() > MAX_RESPONSE_SIZE {
            return Ok(ToolOutput::error("Response too large (exceeds 5MB limit)"));
        }

        if is_image_mime(&mime) {
            return Ok(ToolOutput {
                content: "Image fetched successfully".to_string(),
                is_error: false,
                metadata: Some(json!({
                    "title": title,
                    "mime": mime,
                    "image_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                })),
            });
        }

        let content = String::from_utf8_lossy(&bytes).into_owned();
        let output = match format {
            "text" => {
                if content_type.contains("text/html") {
                    extract_text_from_html(&content)
                } else {
                    content
                }
            }
            "html" => content,
            "markdown" => {
                if content_type.contains("text/html") {
                    convert_html_to_markdown(&content)
                } else {
                    content
                }
            }
            _ => content,
        };

        Ok(ToolOutput {
            content: output,
            is_error: false,
            metadata: Some(json!({ "title": title, "mime": mime })),
        })
    }
}

fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml" && mime != "image/vnd.fastbidsheet"
}

fn extract_text_from_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut skip = false;
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if lower_bytes[i..].starts_with(b"<script")
                || lower_bytes[i..].starts_with(b"<style")
                || lower_bytes[i..].starts_with(b"<noscript")
                || lower_bytes[i..].starts_with(b"<iframe")
                || lower_bytes[i..].starts_with(b"<object")
                || lower_bytes[i..].starts_with(b"<embed")
            {
                skip = true;
            }
            in_tag = true;
        } else if bytes[i] == b'>' {
            in_tag = false;
            if skip
                && (lower_bytes[i.saturating_sub(10)..=i]
                    .windows(2)
                    .any(|w| w == b"</"))
            {
                skip = false;
            }
        } else if !in_tag && !skip {
            text.push(bytes[i] as char);
        }
        i += 1;
    }
    text.trim().to_string()
}

fn convert_html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use clawcr_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};

    fn default_context() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("."),
            permissions: Arc::new(RuleBasedPolicy::new(PermissionMode::AutoApprove)),
            session_id: "test".into(),
        }
    }

    #[test]
    fn image_mime_detects_known_images() {
        assert!(is_image_mime("image/png"));
        assert!(is_image_mime("image/jpeg"));
        assert!(!is_image_mime("image/svg+xml"));
        assert!(!is_image_mime("image/vnd.fastbidsheet"));
    }

    #[test]
    fn extract_text_strips_scripts_and_tags() {
        let html = r#"<html><body><h1>Hi</h1><script>alert()</script><p>There</p></body></html>"#;
        assert_eq!(extract_text_from_html(html), "HiThere");
    }

    #[test]
    fn convert_html_to_plaintext() {
        let html = "<div><p>hello</p><span> world</span></div>";
        assert_eq!(convert_html_to_markdown(html), "hello world");
    }

    #[tokio::test]
    async fn execute_rejects_invalid_url() {
        let tool = WebFetchTool;
        let ctx = default_context();
        let output = tool
            .execute(&ctx, json!({"url": "ftp://example.com"}))
            .await
            .expect("execution should succeed even for invalid URL");
        assert!(output.is_error);
        assert_eq!(output.content, "URL must start with http:// or https://");
    }
}
