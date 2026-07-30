//! Prompt token estimation for assembled model requests.

use devo_protocol::ModelRequest;
use devo_protocol::approx_tokens_from_byte_count;

/// Rough prompt-token estimate for a fully built [`ModelRequest`].
///
/// Converts serialized request bytes with the shared protocol heuristic
/// (~4 bytes/token) so query and persistence stay aligned.
pub(crate) fn estimate_request_prompt_tokens(request: &ModelRequest) -> usize {
    let system_bytes = request.system.as_ref().map_or(0, String::len);
    let message_bytes = request
        .messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
        .sum::<usize>();
    let tool_bytes = request
        .tools
        .as_ref()
        .map(|tools| serde_json::to_string(tools).map_or(0, |json| json.len()))
        .unwrap_or(0);
    let hosted_tool_bytes =
        serde_json::to_string(&request.hosted_tools).map_or(0, |json| json.len());
    approx_tokens_from_byte_count(system_bytes + message_bytes + tool_bytes + hosted_tool_bytes)
        .try_into()
        .unwrap_or(usize::MAX)
}
