use std::pin::Pin;

use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::RequestRole;
use devo_protocol::StreamEvent;
use futures::Stream;

/// Capability flags that describe what a provider family or model can emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Roles accepted by the provider wire format.
    pub supported_roles: Vec<RequestRole>,
    /// Whether the provider accepts a reasoning-effort style control.
    pub supports_reasoning_effort: bool,
    /// Whether the provider accepts temperature.
    pub supports_temperature: bool,
    /// Whether the provider accepts top-p.
    pub supports_top_p: bool,
    /// Whether the provider accepts top-k.
    pub supports_top_k: bool,
    /// Whether the provider supports tool calls in the wire format.
    pub supports_tool_calls: bool,
    /// Whether the provider can surface reasoning content in responses.
    pub supports_reasoning_content: bool,
}

impl ProviderCapabilities {
    /// Returns the default OpenAI-family capability set.
    pub fn openai() -> Self {
        Self {
            supported_roles: vec![
                RequestRole::System,
                RequestRole::Developer,
                RequestRole::User,
                RequestRole::Assistant,
                RequestRole::Tool,
                RequestRole::Function,
            ],
            supports_reasoning_effort: true,
            supports_temperature: true,
            supports_top_p: true,
            supports_top_k: false,
            supports_tool_calls: true,
            supports_reasoning_content: false,
        }
    }

    /// Returns the default Anthropic-family capability set.
    pub fn anthropic() -> Self {
        Self {
            supported_roles: vec![RequestRole::User, RequestRole::Assistant],
            supports_reasoning_effort: true,
            supports_temperature: true,
            supports_top_p: true,
            supports_top_k: true,
            supports_tool_calls: true,
            supports_reasoning_content: true,
        }
    }

    /// Returns whether the provider accepts the given role.
    pub fn supports_role(&self, role: RequestRole) -> bool {
        self.supported_roles.contains(&role)
    }
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self::openai()
    }
}

/// Optional capability adapter implemented by provider SDKs.
///
/// This trait describes the provider family and the capabilities for a
/// particular model. It lets adapters specialize behavior per vendor or per
/// model without introducing a separate provider family.
#[async_trait]
pub trait ProviderAdapter: ModelProviderSDK {
    /// Returns the provider family handled by this adapter.
    fn family(&self) -> ProviderWireApi;

    /// Returns the capabilities that should be used for this model.
    fn capabilities(&self, model: &str) -> ProviderCapabilities;
}

/// A unified interface for model provider SDKs.
///
/// Implementations handle the specifics of each provider SDK while exposing a
/// common completion and completion-stream API.
#[async_trait]
pub trait ModelProviderSDK: Send + Sync {
    /// Send a request and get a complete response.
    async fn completion(&self, request: ModelRequest) -> anyhow::Result<ModelResponse>;

    /// Send a request and get a stream of incremental events.
    ///
    /// Dropping the returned stream should cancel the in-flight request and
    /// close the underlying transport if the provider supports streaming.
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>>;

    /// Human-readable provider name (e.g. "anthropic", "openai").
    fn name(&self) -> &str;
}
