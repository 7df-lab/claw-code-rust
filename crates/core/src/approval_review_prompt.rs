//! Auto-review prompt appended after the in-flight turn request prefix.
//!
//! The markdown template is the instruction copy. Callers still append the
//! live permission profile and tool-request dump after this text.

pub const APPROVAL_REVIEW_PROMPT: &str = include_str!("../prompts/approval_review.md");
