use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{Model, ModelRequest, RequestMessage, ResponseContent, SamplingControls};
use devo_provider::ModelProviderSDK;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::compaction::{CompactionError, HistorySummarizer};

/// Concrete implementation of `HistorySummarizer` that delegates to a
/// `ModelProviderSDK`.
///
/// Detects `context_length_exceeded` provider errors and maps them to
/// `CompactionError::ContextTooLong` so the compaction retry loop can
/// recover by shrinking the input.
pub struct DefaultHistorySummarizer {
    provider: Arc<dyn ModelProviderSDK>,
    model_slug: String,
    request_model: String,
    max_tokens: usize,
}

impl DefaultHistorySummarizer {
    pub fn new(provider: Arc<dyn ModelProviderSDK>, model: &Model) -> Self {
        let max_tokens = model.max_tokens.unwrap_or(4096) as usize;
        Self {
            provider,
            model_slug: model.slug.clone(),
            request_model: model.slug.clone(),
            max_tokens,
        }
    }

    /// Convenience constructor for a catalog slug and provider wire model.
    pub fn with_models(
        provider: Arc<dyn ModelProviderSDK>,
        model_slug: impl Into<String>,
        request_model: impl Into<String>,
        max_tokens: usize,
    ) -> Self {
        Self {
            provider,
            model_slug: model_slug.into(),
            request_model: request_model.into(),
            max_tokens,
        }
    }
}

fn sanitize_compaction_summary(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len());
    for line in text.lines().filter(|line| should_keep_summary_line(line)) {
        if !sanitized.is_empty() {
            sanitized.push('\n');
        }
        sanitized.push_str(line);
    }
    sanitized.trim().to_string()
}

fn should_keep_summary_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.contains("DSML")
        && !trimmed.starts_with("<｜")
        && !trimmed.ends_with("｜>")
        && !trimmed.starts_with("<|")
        && !trimmed.ends_with("|>")
}

#[async_trait]
impl HistorySummarizer for DefaultHistorySummarizer {
    async fn summarize(
        &self,
        messages: Vec<RequestMessage>,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<String, CompactionError> {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug(self.model_slug.clone()),
            model: self.request_model.clone(),
            system: None,
            messages,
            max_tokens: self.max_tokens,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        let request_preview = serde_json::to_string_pretty(&request).unwrap_or_else(|error| {
            format!("<failed to serialize compaction request for logging: {error}>")
        });
        debug!(
            model = %self.request_model,
            message_count = request.messages.len(),
            max_tokens = request.max_tokens,
            compaction_request = %request_preview,
            "sending LLM compaction request"
        );

        let completion = self.provider.completion(request);
        let response = match cancel_token {
            Some(cancel_token) => {
                tokio::select! {
                    biased;
                    () = cancel_token.cancelled() => {
                        return Err(CompactionError::Canceled);
                    }
                    result = completion => result,
                }
            }
            None => completion.await,
        };
        let response = match response {
            Ok(r) => r,
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("context_length_exceeded")
                    || err_msg.contains("maximum context length")
                {
                    return Err(CompactionError::ContextTooLong);
                }
                return Err(CompactionError::SummarizationFailed { message: err_msg });
            }
        };

        let mut text = String::new();
        let mut saw_text = false;
        for block in &response.content {
            let ResponseContent::Text(block_text) = block else {
                continue;
            };
            if saw_text {
                text.push('\n');
            }
            text.push_str(block_text);
            saw_text = true;
        }

        let text = sanitize_compaction_summary(&text);

        if text.is_empty() {
            return Err(CompactionError::EmptyResponse);
        }

        debug!(
            model = %self.request_model,
            response_chars = text.len(),
            compaction_response = %text,
            "received LLM compaction response"
        );

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::sanitize_compaction_summary;

    #[test]
    fn sanitize_compaction_summary_strips_dsml_tool_markup() {
        let input = r#"Progress so far
<｜DSML｜tool_calls>
<｜DSML｜invoke name="grep">
<｜DSML｜parameter name="path" string="true">src</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>
Next step"#;

        assert_eq!(
            sanitize_compaction_summary(input),
            "Progress so far\nNext step"
        );
    }

    #[test]
    fn sanitize_compaction_summary_preserves_internal_spacing() {
        assert_eq!(
            sanitize_compaction_summary("  Summary line  \n\n  Next line  \n"),
            "Summary line  \n\n  Next line"
        );
    }
}
