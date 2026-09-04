//! Reasoning-effort picker data for the chat widget.
//!
//! Model capability resolution stays in `devo_protocol::Model`; this module
//! only converts those protocol choices into labels and list entries for the UI.

use devo_protocol::Model;
use devo_protocol::ReasoningEffort;
use devo_protocol::normalize_reasoning_effort_literal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReasoningEffortListEntry {
    pub(crate) is_current: bool,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) value: String,
}

pub(super) fn reasoning_effort_entries_for_model(
    model: &Model,
    current_selection: Option<&str>,
) -> Vec<ReasoningEffortListEntry> {
    let current_selection = current_reasoning_effort_selection_for_model(model, current_selection);
    model
        .effective_reasoning_capability()
        .options()
        .into_iter()
        .map(|option| ReasoningEffortListEntry {
            is_current: current_selection.as_deref() == Some(option.value.as_str()),
            label: option.label,
            description: option.description,
            value: option.value,
        })
        .collect()
}

pub(super) fn current_reasoning_effort_selection_for_model(
    model: &Model,
    selection: Option<&str>,
) -> Option<String> {
    let options = model.effective_reasoning_capability().options();
    if options.is_empty() {
        return None;
    }

    let normalized_selection = selection
        .map(str::trim)
        .filter(|selection| !selection.is_empty())
        .filter(|selection| !selection.eq_ignore_ascii_case("default"))
        .map(normalize_reasoning_effort_literal);

    if let Some(selection) = normalized_selection.as_deref() {
        if options.iter().any(|option| option.value == selection) {
            return Some(selection.to_string());
        }

        if let Some(effort) = model
            .resolve_reasoning_effort_selection(Some(selection))
            .effective_reasoning_effort
        {
            let value = effort.label().to_lowercase();
            if options.iter().any(|option| option.value == value) {
                return Some(value);
            }
        }
    }

    if let Some(default_selection) = model.default_reasoning_effort_selection()
        && options
            .iter()
            .any(|option| option.value == default_selection)
    {
        return Some(default_selection);
    }

    options.first().map(|option| option.value.clone())
}

pub(super) fn status_line_reasoning_effort_label(effort: Option<ReasoningEffort>) -> &'static str {
    match effort {
        Some(ReasoningEffort::None) | None => "default",
        Some(ReasoningEffort::Minimal) => "minimal",
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::ReasoningCapability;

    #[test]
    fn status_line_reasoning_effort_labels_are_compact() {
        let actual = [
            status_line_reasoning_effort_label(None),
            status_line_reasoning_effort_label(Some(ReasoningEffort::Minimal)),
            status_line_reasoning_effort_label(Some(ReasoningEffort::XHigh)),
            status_line_reasoning_effort_label(Some(ReasoningEffort::Max)),
        ];

        assert_eq!(actual, ["default", "minimal", "xhigh", "max"]);
    }

    #[test]
    fn entries_preserve_explicit_levels_with_off_selection() {
        let model = Model {
            slug: "deepseek-v4".to_string(),
            display_name: "Deepseek V4".to_string(),
            reasoning_capability: ReasoningCapability::Levels(
                devo_protocol::levels_with_leading_off([
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ]),
            ),
            default_reasoning_effort: Some(ReasoningEffort::High),
            ..Model::default()
        };

        assert_eq!(
            reasoning_effort_entries_for_model(&model, Some("disabled")),
            vec![
                ReasoningEffortListEntry {
                    is_current: true,
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "off".to_string(),
                },
                ReasoningEffortListEntry {
                    is_current: false,
                    label: "High".to_string(),
                    description: "More deliberate for harder tasks".to_string(),
                    value: "high".to_string(),
                },
                ReasoningEffortListEntry {
                    is_current: false,
                    label: "Max".to_string(),
                    description: "Most deliberate, highest effort".to_string(),
                    value: "max".to_string(),
                },
            ]
        );

        assert_eq!(
            reasoning_effort_entries_for_model(&model, Some("max")),
            vec![
                ReasoningEffortListEntry {
                    is_current: false,
                    label: "Off".to_string(),
                    description: "Disable reasoning effort for this turn".to_string(),
                    value: "off".to_string(),
                },
                ReasoningEffortListEntry {
                    is_current: false,
                    label: "High".to_string(),
                    description: "More deliberate for harder tasks".to_string(),
                    value: "high".to_string(),
                },
                ReasoningEffortListEntry {
                    is_current: true,
                    label: "Max".to_string(),
                    description: "Most deliberate, highest effort".to_string(),
                    value: "max".to_string(),
                },
            ]
        );
    }
}
