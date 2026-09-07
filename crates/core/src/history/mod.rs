pub mod compaction;
mod context_insertion;
mod transactions;
pub use transactions::response_items_to_messages;
pub mod normalize;
pub mod summarizer;

pub use context_insertion::insert_context_diff_message;

use std::collections::HashSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use devo_protocol::{InputModality, RequestContent, RequestMessage, Role, UserInput};

use crate::context::ContextualUserFragment;
use crate::response_item::ResponseItem;

// ---------------------------------------------------------------------------
// TokenInfo
// ---------------------------------------------------------------------------

/// Token usage information for the history.
///
/// Stores the token counts as reported by the LLM provider. The design is
/// provider-agnostic and covers the common fields supported by OpenAI chat
/// completions, OpenAI responses, and Anthropic messages APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TokenInfo {
    /// Total input (prompt) tokens consumed.
    pub input_tokens: usize,
    /// Input tokens served from a cache, when reported by the provider.
    pub cached_input_tokens: usize,
    /// Total output (completion) tokens generated.
    pub output_tokens: usize,
}

impl TokenInfo {
    /// Returns the sum of all tracked tokens.
    pub fn total(&self) -> usize {
        self.input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.output_tokens)
    }

    /// Accumulates another `TokenInfo` into this one.
    pub fn accumulate(&mut self, other: &TokenInfo) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
    }
}

// ---------------------------------------------------------------------------
// ContextView
// ---------------------------------------------------------------------------

/// Snapshot of the environment and model context at a point in time.
///
/// Used to detect context changes and produce a "diff prompt" so the LLM
/// can be informed about what has changed since its last view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextView {
    /// Operating system identifier (e.g. "windows", "linux", "macos").
    pub os: String,
    /// Shell name (e.g. "bash", "zsh", "powershell").
    pub shell: String,
    /// IANA timezone identifier (e.g. "Asia/Shanghai", "America/New_York").
    pub timezone: String,
    /// Active model slug.
    pub model: String,
    /// Current reasoning effort selection, if any.
    #[serde(
        default,
        alias = "thinking_effort",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_effort_selection: Option<String>,
    /// Active persona or system persona identifier, if any.
    pub persona: Option<String>,
    /// Current date in ISO-8601 format (YYYY-MM-DD).
    pub date: String,
    /// Current working directory.
    pub cwd: String,
}

impl ContextView {
    /// Creates a new `ContextView` from the supplied parameters.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        os: impl Into<String>,
        shell: impl Into<String>,
        timezone: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort_selection: Option<String>,
        persona: Option<String>,
        date: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Self {
        Self {
            os: os.into(),
            shell: shell.into(),
            timezone: timezone.into(),
            model: model.into(),
            reasoning_effort_selection,
            persona,
            date: date.into(),
            cwd: cwd.into(),
        }
    }

    /// Renders the full context as a structured prompt fragment.
    pub fn to_prompt(&self) -> String {
        let os = &self.os;
        let shell = &self.shell;
        let timezone = &self.timezone;
        let model = &self.model;
        let date = &self.date;
        let cwd = &self.cwd;
        let mut prompt = String::new();
        let _ = write!(
            prompt,
            "<os>{os}</os>\n<shell>{shell}</shell>\n<timezone>{timezone}</timezone>\n<model>{model}</model>\n<date>{date}</date>\n<cwd>{cwd}</cwd>"
        );
        if let Some(ref selection) = self.reasoning_effort_selection {
            let _ = write!(
                prompt,
                "\n<reasoning_effort_selection>{selection}</reasoning_effort_selection>"
            );
        }
        if let Some(ref persona) = self.persona {
            let _ = write!(prompt, "\n<persona>{persona}</persona>");
        }
        prompt
    }

    /// Produces a diff prompt describing what has changed since `other`.
    ///
    /// When the context has changed (e.g. the user switched model or working
    /// directory), this returns a structured message that can be injected
    /// into the prompt to inform the LLM.
    pub fn diff_since(&self, previous: &ContextView) -> Option<String> {
        let mut diff = String::from("<context_changes>\n");
        let mut changed = false;

        if self.os != previous.os {
            let previous_os = &previous.os;
            let os = &self.os;
            let _ = write!(diff, "os: {previous_os} -> {os}");
            changed = true;
        }
        if self.shell != previous.shell {
            if changed {
                diff.push('\n');
            }
            let previous_shell = &previous.shell;
            let shell = &self.shell;
            let _ = write!(diff, "shell: {previous_shell} -> {shell}");
            changed = true;
        }
        if self.timezone != previous.timezone {
            if changed {
                diff.push('\n');
            }
            let previous_timezone = &previous.timezone;
            let timezone = &self.timezone;
            let _ = write!(diff, "timezone: {previous_timezone} -> {timezone}");
            changed = true;
        }
        if self.model != previous.model {
            if changed {
                diff.push('\n');
            }
            let previous_model = &previous.model;
            let model = &self.model;
            let _ = write!(diff, "model: {previous_model} -> {model}");
            changed = true;
        }
        if self.reasoning_effort_selection != previous.reasoning_effort_selection {
            if changed {
                diff.push('\n');
            }
            let previous_reasoning_effort_selection = &previous.reasoning_effort_selection;
            let reasoning_effort_selection = &self.reasoning_effort_selection;
            let _ = write!(
                diff,
                "reasoning_effort_selection: {previous_reasoning_effort_selection:?} -> {reasoning_effort_selection:?}"
            );
            changed = true;
        }
        if self.persona != previous.persona {
            if changed {
                diff.push('\n');
            }
            let previous_persona = &previous.persona;
            let persona = &self.persona;
            let _ = write!(diff, "persona: {previous_persona:?} -> {persona:?}");
            changed = true;
        }
        if self.date != previous.date {
            if changed {
                diff.push('\n');
            }
            let previous_date = &previous.date;
            let date = &self.date;
            let _ = write!(diff, "date: {previous_date} -> {date}");
            changed = true;
        }
        if self.cwd != previous.cwd {
            if changed {
                diff.push('\n');
            }
            let previous_cwd = &previous.cwd;
            let cwd = &self.cwd;
            let _ = write!(diff, "cwd: {previous_cwd} -> {cwd}");
            changed = true;
        }

        if !changed {
            return None;
        }

        diff.push_str("\n</context_changes>");
        Some(diff)
    }
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// Manages a sequence of `ResponseItem`s together with token usage metadata
/// and environment context.
///
/// Provides utilities for mutation, normalization, and prompt preparation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct History {
    /// The ordered sequence of conversation items.
    pub items: Vec<ResponseItem>,
    /// Aggregate token usage for the history.
    pub token_info: TokenInfo,
    /// The environment and model context snapshot.
    pub context: ContextView,
}

impl History {
    /// Creates a new `History` with the given context.
    pub fn new(context: ContextView) -> Self {
        Self {
            items: Vec::new(),
            token_info: TokenInfo::default(),
            context,
        }
    }

    /// Appends a `ResponseItem` to the end of the history.
    pub fn push(&mut self, item: ResponseItem) {
        self.items.push(item);
    }

    /// Inserts a `ResponseItem` at the given index.
    pub fn insert(&mut self, index: usize, item: ResponseItem) {
        self.items.insert(index, item);
    }

    /// Removes the item at the given index.
    pub fn remove(&mut self, index: usize) -> ResponseItem {
        self.items.remove(index)
    }

    /// Replaces all items in-place with the given sequence.
    ///
    /// Used by compaction to atomically swap the full item list without
    /// constructing a new `History` wrapper.
    pub fn replace_items(&mut self, items: Vec<ResponseItem>) {
        self.items = items;
    }

    /// Returns the number of items in the history.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the history contains no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Removes the last user message (and its associated reasoning / tool-call
    /// items) from the tail of history.
    ///
    /// This is used when the user "draws back" their last message. Returns
    /// `true` if an item was removed.
    ///
    /// The method walks backward from the end and removes everything that
    /// belongs to the last user-initiated turn: the user message itself,
    /// any preceding reasoning and tool-call items from the assistant turn
    /// that responded to it, and the tool-call outputs that followed.
    /// If the preceding item is a `CompactionSummary` fragment, it is removed
    /// together with the user message.
    pub fn remove_tail_user_message(&mut self) -> bool {
        // Find the last user Message from the end.
        let last_user_pos = self.items.iter().rposition(|item| match item {
            ResponseItem::Message(msg) => msg.role == devo_protocol::Role::User,
            _ => false,
        });

        let Some(start) = last_user_pos else {
            return false;
        };

        // If the preceding item is a compaction-summary fragment, remove it too.
        let truncate_at = if start > 0
            && matches!(&self.items[start - 1], ResponseItem::Message(msg) if msg.content.iter().any(|block| {
                matches!(block, devo_protocol::ContentBlock::Text { text } if crate::context::compaction_summary::CompactionSummary::matches_text(text))
            })) {
            start - 1
        } else {
            start
        };

        self.items.truncate(truncate_at);
        true
    }

    /// Prepares the history for an LLM call by:
    ///
    /// 1. Normalizing tool-call / tool-call-output pairing
    /// 2. Filtering items according to the model's supported modalities
    /// 3. Converting to `Vec<RequestMessage>`
    /// 4. Merging consecutive assistant messages split from one assistant turn
    ///    (prevents orphan tool-call messages that violate provider protocol
    ///    requirements)
    pub fn for_prompt(&self, modalities: &[InputModality]) -> Vec<RequestMessage> {
        if modalities.contains(&InputModality::Text)
            && normalize::text_modality_keeps_all_items(&self.items)
        {
            let mut tool_call_ids = None::<HashSet<&str>>;
            let mut tool_output_ids = None::<HashSet<&str>>;
            for item in &self.items {
                match item {
                    ResponseItem::ToolCall { id, .. } => {
                        tool_call_ids
                            .get_or_insert_with(|| HashSet::with_capacity(self.items.len() / 2))
                            .insert(id.as_str());
                    }
                    ResponseItem::ToolCallOutput { tool_use_id, .. } => {
                        tool_output_ids
                            .get_or_insert_with(|| HashSet::with_capacity(self.items.len() / 2))
                            .insert(tool_use_id.as_str());
                    }
                    ResponseItem::Reason { .. } | ResponseItem::Message(_) => {}
                }
            }

            let mut messages = Vec::with_capacity(self.items.len());
            for item in &self.items {
                match item {
                    ResponseItem::ToolCall { id, .. } => {
                        if tool_output_ids
                            .as_ref()
                            .is_some_and(|ids| ids.contains(id.as_str()))
                        {
                            messages.push(item.into());
                        }
                    }
                    ResponseItem::ToolCallOutput { tool_use_id, .. } => {
                        if tool_call_ids
                            .as_ref()
                            .is_some_and(|ids| ids.contains(tool_use_id.as_str()))
                        {
                            messages.push(item.into());
                        }
                    }
                    ResponseItem::Reason { .. } | ResponseItem::Message(_) => {
                        messages.push(item.into());
                    }
                }
            }
            merge_consecutive_assistant_messages(&mut messages);
            devo_protocol::normalize_tool_result_messages(&mut messages);
            return messages;
        }

        let mut items = normalize::filter_by_modality(&self.items, modalities);
        normalize::pair_tool_call_items(&mut items);
        let mut messages: Vec<RequestMessage> = items.into_iter().map(Into::into).collect();
        merge_consecutive_assistant_messages(&mut messages);
        devo_protocol::normalize_tool_result_messages(&mut messages);
        messages
    }

    /// Updates the context view and produces a diff prompt if anything changed.
    pub fn update_context(&mut self, new_context: ContextView) -> Option<String> {
        let diff = self.context.diff_since(&new_context);
        self.context = new_context;
        diff
    }

    /// Builds prompt-visible request messages by prepending locked session
    /// inputs and normalizing the existing history items.
    pub fn for_prompt_with_prefix(
        &self,
        prefix_user_inputs: &[UserInput],
        modalities: &[InputModality],
    ) -> Vec<RequestMessage> {
        let mut messages = self.for_prompt(modalities);
        prepend_user_inputs(&mut messages, prefix_user_inputs);
        messages
    }
}

/// Merges consecutive assistant `RequestMessage`s into a single message by
/// concatenating their content arrays.
///
/// This is the inverse of the split performed by
/// [`crate::response_item::message_to_response_items`]. For example, a mixed
/// assistant turn that became:
///
/// ```text
/// ResponseItem::Reason   { "hmm" }
/// ResponseItem::Message  { Assistant, [Text("hello")] }
/// ResponseItem::ToolCall { "tu-1", "bash", ... }
/// ```
///
/// converts to three consecutive assistant `RequestMessage`s. Providers such as
/// OpenAI require an assistant message that carries `tool_calls` to be followed
/// immediately by tool-result messages, not by another assistant message. Merging
/// restores a single assistant request message before the tool results are sent.
fn merge_consecutive_assistant_messages(messages: &mut Vec<RequestMessage>) {
    let assistant_role = Role::Assistant.as_str();
    let capacity = messages.len();
    let previous = std::mem::replace(messages, Vec::with_capacity(capacity));
    for mut message in previous {
        match messages.last_mut() {
            Some(last) if last.role == assistant_role && message.role == assistant_role => {
                last.content.append(&mut message.content);
            }
            _ => messages.push(message),
        }
    }
}

/// Converts locked prefix `UserInput`s into request messages and prepends them
/// ahead of the existing prompt-visible history.
pub fn prepend_user_inputs(messages: &mut Vec<RequestMessage>, user_inputs: &[UserInput]) {
    messages.splice(
        0..0,
        user_inputs.iter().filter_map(|input| match input {
            UserInput::Text { text, .. } if !text.trim().is_empty() => Some(RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::Text { text: text.clone() }],
            }),
            UserInput::Text { .. }
            | UserInput::Image { .. }
            | UserInput::LocalImage { .. }
            | UserInput::Skill { .. }
            | UserInput::Mention { .. }
            | _ => None,
        }),
    );
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::response_item::ResponseItem;
    use devo_protocol::{ContentBlock, Message, Role};

    fn test_context() -> ContextView {
        ContextView::new(
            "linux",
            "bash",
            "UTC",
            "test-model",
            None,
            None,
            "2026-04-27",
            "/home/test",
        )
    }

    #[test]
    fn history_new_is_empty() {
        let h = History::new(test_context());
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn history_push_and_len() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::user("hello")));
        assert_eq!(h.len(), 1);
        assert!(!h.is_empty());
    }

    #[test]
    fn history_remove_tail_user_message() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::user("hello")));
        h.push(ResponseItem::Message(Message::assistant_text("world")));
        h.push(ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        });
        h.push(ResponseItem::ToolCallOutput {
            tool_use_id: "tc-1".into(),
            content: "ok".into(),
            is_error: false,
        });

        assert!(h.remove_tail_user_message());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn history_remove_tail_user_message_no_user_message() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::assistant_text("hello")));
        assert!(!h.remove_tail_user_message());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn history_context_diff_no_changes() {
        let ctx = test_context();
        let diff = ctx.diff_since(&ctx);
        assert!(diff.is_none());
    }

    #[test]
    fn history_context_diff_detects_change() {
        let ctx1 = test_context();
        let mut ctx2 = test_context();
        ctx2.cwd = "/home/other".into();
        let diff = ctx2.diff_since(&ctx1);
        assert_eq!(
            diff,
            Some("<context_changes>\ncwd: /home/test -> /home/other\n</context_changes>".into())
        );
    }

    #[test]
    fn history_context_to_prompt_contains_fields() {
        let ctx = test_context();
        let prompt = ctx.to_prompt();
        assert_eq!(
            prompt,
            "<os>linux</os>\n<shell>bash</shell>\n<timezone>UTC</timezone>\n<model>test-model</model>\n<date>2026-04-27</date>\n<cwd>/home/test</cwd>"
        );
    }

    #[test]
    fn history_for_prompt_respects_modalities() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        }));
        h.push(ResponseItem::Message(Message::assistant_text("hi")));

        let msgs = h.for_prompt(&[InputModality::Text]);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn history_for_prompt_text_drops_orphaned_tool_items() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::user("start")));
        h.push(ResponseItem::ToolCall {
            id: "orphan-call".into(),
            name: "bash".into(),
            input: serde_json::json!({ "cmd": "missing output" }),
        });
        h.push(ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({ "cmd": "date" }),
        });
        h.push(ResponseItem::ToolCallOutput {
            tool_use_id: "tc-1".into(),
            content: "ok".into(),
            is_error: false,
        });
        h.push(ResponseItem::ToolCallOutput {
            tool_use_id: "orphan-output".into(),
            content: "missing call".into(),
            is_error: false,
        });

        let msgs = h.for_prompt(&[InputModality::Text]);
        let expected = vec![
            Message::user("start").to_request_message(),
            RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::ToolUse {
                    id: "tc-1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({ "cmd": "date" }),
                }],
            },
            RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id: "tc-1".into(),
                    content: "ok".into(),
                    is_error: None,
                }],
            },
        ];

        assert_eq!(
            serde_json::to_value(&msgs).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }

    #[test]
    #[ignore]
    fn bench_history_for_prompt_with_paired_tools() {
        let mut h = History::new(test_context());
        for index in 0..500 {
            h.push(ResponseItem::Message(Message::assistant_text(format!(
                "message {index}"
            ))));
            h.push(ResponseItem::ToolCall {
                id: format!("tc-{index}"),
                name: "bash".into(),
                input: serde_json::json!({ "cmd": "date" }),
            });
            h.push(ResponseItem::ToolCallOutput {
                tool_use_id: format!("tc-{index}"),
                content: "ok".into(),
                is_error: false,
            });
        }

        let started = Instant::now();
        let mut total_messages = 0;
        for _ in 0..2_000 {
            total_messages += black_box(h.for_prompt(black_box(&[InputModality::Text]))).len();
        }
        let elapsed = started.elapsed();

        assert_eq!(total_messages, 2_000_000);
        println!(
            "history_for_prompt_with_paired_tools iterations=2000 items=1500 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 2_000.0
        );
    }

    #[test]
    fn history_for_prompt_with_prefix_prepends_user_inputs() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::user("hello")));

        let msgs = h.for_prompt_with_prefix(
            &[UserInput::Text {
                text: "<environment_context>locked</environment_context>".into(),
                text_elements: Vec::new(),
            }],
            &[InputModality::Text],
        );

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        let RequestContent::Text { text } = &msgs[0].content[0] else {
            panic!("expected text prefix");
        };
        assert!(text.contains("environment_context"));
    }

    #[test]
    fn insert_context_diff_message_places_diff_before_latest_user_message() {
        let mut messages = vec![
            Message::user("first"),
            Message::assistant_text("reply"),
            Message::user("second"),
        ];

        insert_context_diff_message(
            &mut messages,
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "<context_changes>\nmodel: a -> b\n</context_changes>".into(),
                }],
            },
        );

        assert_eq!(messages.len(), 4);
        let ContentBlock::Text { text } = &messages[2].content[0] else {
            panic!("expected diff text");
        };
        assert!(text.contains("<context_changes>"));
        assert_eq!(messages[3], Message::user("second"));
    }

    #[test]
    fn token_info_default() {
        let info = TokenInfo::default();
        assert_eq!(info.input_tokens, 0);
        assert_eq!(info.cached_input_tokens, 0);
        assert_eq!(info.output_tokens, 0);
        assert_eq!(info.total(), 0);
    }

    #[test]
    fn token_info_accumulate() {
        let mut info = TokenInfo {
            input_tokens: 100,
            cached_input_tokens: 10,
            output_tokens: 50,
        };
        info.accumulate(&TokenInfo {
            input_tokens: 50,
            cached_input_tokens: 5,
            output_tokens: 25,
        });
        assert_eq!(info.input_tokens, 150);
        assert_eq!(info.cached_input_tokens, 15);
        assert_eq!(info.output_tokens, 75);
    }

    #[test]
    fn token_info_total() {
        let info = TokenInfo {
            input_tokens: 100,
            cached_input_tokens: 20,
            output_tokens: 50,
        };
        assert_eq!(info.total(), 170);
    }

    #[test]
    fn remove_tail_multiple_turns() {
        let mut h = History::new(test_context());
        // Turn 1
        h.push(ResponseItem::Message(Message::user("first")));
        h.push(ResponseItem::Message(Message::assistant_text("reply1")));
        // Turn 2
        h.push(ResponseItem::Message(Message::user("second")));
        h.push(ResponseItem::Message(Message::assistant_text("reply2")));

        assert!(h.remove_tail_user_message());
        assert_eq!(h.len(), 2);
        // Only Turn 1 remains
        if let Some(item) = h.items.first() {
            match item {
                ResponseItem::Message(msg) => {
                    assert_eq!(msg.role, Role::User);
                }
                _ => panic!("expected user message"),
            }
        }
    }

    #[test]
    fn for_prompt_merges_assistant_tool_calls_and_groups_tool_results() {
        let mut h = History::new(test_context());

        // User message
        h.push(ResponseItem::Message(Message::user("hello")));

        // Assistant response: text + two tool calls — simulating the
        // split that message_to_response_items produces from a single
        // assistant Message with [Text, ToolUse, ToolUse].
        h.push(ResponseItem::Message(Message::assistant_text("ok")));
        h.push(ResponseItem::ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            input: serde_json::json!({"filePath": "/tmp/test.txt"}),
        });
        h.push(ResponseItem::ToolCall {
            id: "call-2".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "date"}),
        });

        // Tool results (stored as ToolCallOutput)
        h.push(ResponseItem::ToolCallOutput {
            tool_use_id: "call-1".into(),
            content: "file content".into(),
            is_error: false,
        });
        h.push(ResponseItem::ToolCallOutput {
            tool_use_id: "call-2".into(),
            content: "Mon Apr 28".into(),
            is_error: false,
        });

        let msgs = h.for_prompt(&[InputModality::Text]);

        // Should produce: user, assistant(merged), user(grouped tool results).
        assert_eq!(msgs.len(), 3);

        // Second message: assistant with text + both tool calls merged
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content.len(), 3);
        assert!(matches!(&msgs[1].content[0], RequestContent::Text { .. }));
        assert!(
            matches!(&msgs[1].content[1], RequestContent::ToolUse { id, .. } if id == "call-1")
        );
        assert!(
            matches!(&msgs[1].content[2], RequestContent::ToolUse { id, .. } if id == "call-2")
        );

        // Third message: user tool results are grouped for provider adjacency.
        assert_eq!(msgs[2].role, "user");
        assert_eq!(msgs[2].content.len(), 2);
        assert!(
            matches!(&msgs[2].content[0], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-1")
        );
        assert!(
            matches!(&msgs[2].content[1], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-2")
        );
    }

    #[test]
    fn for_prompt_preserves_consecutive_user_messages() {
        let mut h = History::new(test_context());
        h.push(ResponseItem::Message(Message::user("first")));
        h.push(ResponseItem::Message(Message::user("second")));

        let msgs = h.for_prompt(&[InputModality::Text]);
        let expected = vec![
            Message::user("first").to_request_message(),
            Message::user("second").to_request_message(),
        ];

        assert_eq!(
            serde_json::to_value(&msgs).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
}
