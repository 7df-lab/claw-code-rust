//! Parse `update_plan` tool output into typed [`PlanEntry`] lists.
//!
//! Tool results and some persisted `TurnItem::Plan` blobs store the full
//! `{ explanation, plan: [...] }` JSON. UI-facing `Item::Plan` must carry one
//! entry per step; this module is the shared expansion used by persist,
//! restore, and wire projection.

use serde_json::Value as JsonValue;

use super::item::PlanEntry;
use super::item::PlanStepStatus;

/// Parse a single plan step object (`step`/`content` + `status`).
pub fn plan_entry_from_json(item: &JsonValue) -> Option<PlanEntry> {
    let step = item
        .get("step")
        .or_else(|| item.get("content"))
        .and_then(JsonValue::as_str)?
        .trim();
    if step.is_empty() {
        return None;
    }
    Some(PlanEntry {
        step: step.to_string(),
        status: plan_step_status_from_str(
            item.get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or("pending"),
        ),
    })
}

/// Map tool/wire status strings onto [`PlanStepStatus`].
pub fn plan_step_status_from_str(status: &str) -> PlanStepStatus {
    match status {
        "completed" => PlanStepStatus::Completed,
        "in_progress" | "inProgress" => PlanStepStatus::InProgress,
        _ => PlanStepStatus::Pending,
    }
}

/// Extract plan entries from an `update_plan` JSON object (or bare plan array).
pub fn plan_entries_from_update_plan_json(value: &JsonValue) -> Option<Vec<PlanEntry>> {
    let plan = match value {
        JsonValue::Array(items) => items.as_slice(),
        JsonValue::Object(_) => value.get("plan")?.as_array()?.as_slice(),
        _ => return None,
    };
    let entries: Vec<PlanEntry> = plan.iter().filter_map(plan_entry_from_json).collect();
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

/// Parse plan entries from persisted plan text.
///
/// Returns `None` when `text` is not structured `update_plan` JSON (e.g. a
/// Proposed Plan markdown blob), so callers can fall back to a single entry.
pub fn plan_entries_from_plan_text(text: &str) -> Option<Vec<PlanEntry>> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let value: JsonValue = serde_json::from_str(trimmed).ok()?;
    plan_entries_from_update_plan_json(&value)
}

/// Expand plan text into entries, falling back to one completed entry for
/// non-JSON / proposed-plan markdown.
pub fn plan_entries_from_plan_text_or_single(text: String) -> Vec<PlanEntry> {
    plan_entries_from_plan_text(&text).unwrap_or_else(|| {
        vec![PlanEntry {
            step: text,
            status: PlanStepStatus::Completed,
        }]
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parses_update_plan_object_with_step_field() {
        let value = serde_json::json!({
            "explanation": "track chores",
            "output": "…",
            "plan": [
                { "status": "completed", "step": "买酱油" },
                { "status": "pending", "step": "充电费" },
                { "status": "in_progress", "step": "用京东买点儿药" },
            ]
        });
        assert_eq!(
            plan_entries_from_update_plan_json(&value),
            Some(vec![
                PlanEntry {
                    step: "买酱油".into(),
                    status: PlanStepStatus::Completed,
                },
                PlanEntry {
                    step: "充电费".into(),
                    status: PlanStepStatus::Pending,
                },
                PlanEntry {
                    step: "用京东买点儿药".into(),
                    status: PlanStepStatus::InProgress,
                },
            ])
        );
    }

    #[test]
    fn parses_content_alias_and_camel_case_status() {
        let value = serde_json::json!([
            { "content": "A", "status": "completed" },
            { "content": "B", "status": "inProgress" },
        ]);
        assert_eq!(
            plan_entries_from_update_plan_json(&value),
            Some(vec![
                PlanEntry {
                    step: "A".into(),
                    status: PlanStepStatus::Completed,
                },
                PlanEntry {
                    step: "B".into(),
                    status: PlanStepStatus::InProgress,
                },
            ])
        );
    }

    #[test]
    fn markdown_proposed_plan_is_not_structured() {
        assert_eq!(
            plan_entries_from_plan_text("## Approach\n\n1. Inspect\n2. Patch\n"),
            None
        );
    }

    #[test]
    fn text_helper_falls_back_to_single_entry() {
        assert_eq!(
            plan_entries_from_plan_text_or_single("## Approach\n".into()),
            vec![PlanEntry {
                step: "## Approach\n".into(),
                status: PlanStepStatus::Completed,
            }]
        );
    }
}
