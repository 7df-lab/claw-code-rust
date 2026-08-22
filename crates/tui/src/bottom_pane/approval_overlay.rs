use crossterm::event::KeyCode;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::SessionId;
use devo_protocol::TurnId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::key_hint;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::text_formatting::truncate_text;

/// Max graphemes of a command/path embedded in an approval option label.
/// Keeps the two-column selection list readable when the pending command is a
/// long heredoc or multi-line script; the header still shows the full summary.
const APPROVAL_LABEL_SNIPPET_MAX_GRAPHEMES: usize = 48;

/// Collapse a command (or path) for use inside an approval option label.
fn snippet_for_approval_label(value: &str) -> String {
    let first_line = value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(value)
        .trim();
    let collapsed = if value.lines().nth(1).is_some() {
        format!("{first_line}…")
    } else {
        first_line.to_string()
    };
    truncate_text(&collapsed, APPROVAL_LABEL_SNIPPET_MAX_GRAPHEMES)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalOverlayRequest {
    pub(crate) session_id: SessionId,
    pub(crate) turn_id: TurnId,
    pub(crate) approval_id: String,
    pub(crate) action_summary: String,
    pub(crate) justification: String,
    pub(crate) resource: Option<String>,
    pub(crate) available_scopes: Vec<String>,
    pub(crate) path: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) command_pattern: Option<Vec<String>>,
    pub(crate) command_prefix: Option<Vec<String>>,
}

pub(crate) struct ApprovalOverlay {
    approval_id: String,
    list: ListSelectionView,
}

impl ApprovalOverlay {
    pub(crate) fn new(
        request: ApprovalOverlayRequest,
        app_event_tx: AppEventSender,
        accent_color: Color,
    ) -> Self {
        let approval_id = request.approval_id.clone();
        Self {
            approval_id,
            list: ListSelectionView::new(build_params(request), app_event_tx, accent_color),
        }
    }
}

impl BottomPaneView for ApprovalOverlay {
    fn approval_id(&self) -> Option<&str> {
        Some(&self.approval_id)
    }

    fn handle_key_event(&mut self, key_event: crossterm::event::KeyEvent) {
        self.list.handle_key_event(key_event);
    }

    fn is_complete(&self) -> bool {
        self.list.is_complete()
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.list.on_ctrl_c()
    }
}

impl Renderable for ApprovalOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.list.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.list.desired_height(width)
    }
}

fn has_scope(request: &ApprovalOverlayRequest, scope: &str) -> bool {
    request
        .available_scopes
        .iter()
        .any(|available| available.eq_ignore_ascii_case(scope))
}

fn looks_like_tool_call_id(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("call_") || trimmed.starts_with("call-")
}

fn build_params(request: ApprovalOverlayRequest) -> SelectionViewParams {
    let header = build_header(&request);
    let mut items = Vec::new();
    if request.available_scopes.is_empty() || has_scope(&request, "once") {
        items.push(approval_item(
            "Yes, proceed",
            KeyCode::Char('y'),
            &request,
            ApprovalDecisionValue::Approve,
            ApprovalScopeValue::Once,
        ));
    }
    if has_scope(&request, "session") {
        let name = request
            .command_pattern
            .as_ref()
            .map(|pattern| {
                format!(
                    "Yes, and don't ask again for `{}` in this session",
                    snippet_for_approval_label(&pattern.join(" "))
                )
            })
            .or_else(|| {
                request
                    .target
                    .as_ref()
                    .filter(|command| !looks_like_tool_call_id(command))
                    .map(|command| {
                        format!(
                            "Yes, and don't ask again for `{}` in this session",
                            snippet_for_approval_label(command)
                        )
                    })
            })
            .unwrap_or_else(|| {
                "Yes, and don't ask again for this command in this session".to_string()
            });
        items.push(approval_item(
            &name,
            KeyCode::Char('s'),
            &request,
            ApprovalDecisionValue::Approve,
            ApprovalScopeValue::Session,
        ));
    }
    if has_scope(&request, "command_prefix_persist")
        && let Some(prefix) = request.command_prefix.as_ref()
    {
        let rendered = snippet_for_approval_label(&prefix.join(" "));
        items.push(approval_item(
            &format!("Yes, and don't ask again for commands that start with `{rendered}`"),
            KeyCode::Char('p'),
            &request,
            ApprovalDecisionValue::Approve,
            ApprovalScopeValue::CommandPrefixPersist,
        ));
    }
    if has_scope(&request, "path_prefix")
        && let Some(path) = request.path.as_ref()
    {
        items.push(approval_item(
            &format!(
                "Yes, and don't ask again for files under `{}`",
                snippet_for_approval_label(path)
            ),
            KeyCode::Char('f'),
            &request,
            ApprovalDecisionValue::Approve,
            ApprovalScopeValue::PathPrefix,
        ));
    }
    if has_scope(&request, "host")
        && let Some(host) = request.host.as_ref()
    {
        items.push(approval_item(
            &format!("Yes, and allow `{host}` for this session"),
            KeyCode::Char('h'),
            &request,
            ApprovalDecisionValue::Approve,
            ApprovalScopeValue::Host,
        ));
    }
    items.push(approval_item(
        "No, continue without running it",
        KeyCode::Char('n'),
        &request,
        ApprovalDecisionValue::Deny,
        ApprovalScopeValue::Once,
    ));

    SelectionViewParams {
        title: Some("Permission approval required".to_string()),
        footer_hint: Some(Line::from(
            "Use ↑/↓ to choose, Enter to confirm, Esc to cancel (interrupt).",
        )),
        header: Box::new(header),
        items,
        on_cancel: Some(Box::new(move |app_event_tx| {
            app_event_tx.send(AppEvent::Command(AppCommand::ApprovalRespond {
                session_id: request.session_id,
                turn_id: request.turn_id,
                approval_id: request.approval_id.clone(),
                decision: ApprovalDecisionValue::Cancel,
                scope: ApprovalScopeValue::Once,
            }));
        })),
        ..Default::default()
    }
}

fn approval_item(
    name: &str,
    shortcut: KeyCode,
    request: &ApprovalOverlayRequest,
    decision: ApprovalDecisionValue,
    scope: ApprovalScopeValue,
) -> SelectionItem {
    let session_id = request.session_id;
    let turn_id = request.turn_id;
    let approval_id = request.approval_id.clone();
    SelectionItem {
        name: name.to_string(),
        display_shortcut: Some(key_hint::plain(shortcut)),
        dismiss_on_select: true,
        actions: vec![Box::new(move |app_event_tx| {
            app_event_tx.send(AppEvent::Command(AppCommand::ApprovalRespond {
                session_id,
                turn_id,
                approval_id: approval_id.clone(),
                decision: decision.clone(),
                scope: scope.clone(),
            }));
        })],
        ..Default::default()
    }
}

fn build_header(request: &ApprovalOverlayRequest) -> ColumnRenderable<'static> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from(request.action_summary.clone()).bold());
    header.push(Line::from(""));
    push_field(&mut header, "reason", Some(&request.justification));
    push_field(&mut header, "path", request.path.as_ref());
    push_field(&mut header, "host", request.host.as_ref());
    header
}

fn push_field(header: &mut ColumnRenderable<'static>, label: &str, value: Option<&String>) {
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    header.push(Line::from(format!("{label}: {value}")).dim());
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn overlay_request(
        available_scopes: Vec<String>,
        command_pattern: Option<Vec<String>>,
        command_prefix: Option<Vec<String>>,
        path: Option<String>,
        host: Option<String>,
    ) -> ApprovalOverlayRequest {
        ApprovalOverlayRequest {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            approval_id: "approval-1".to_string(),
            action_summary: "run git add file.txt".to_string(),
            justification: "Tool execution requires approval.".to_string(),
            resource: Some("ShellExec".to_string()),
            available_scopes,
            path,
            host,
            target: Some("git add file.txt".to_string()),
            command_pattern,
            command_prefix,
        }
    }

    fn item_names(params: &SelectionViewParams) -> Vec<&str> {
        params.items.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn session_item_label_includes_command_pattern() {
        let params = build_params(overlay_request(
            vec!["once".to_string(), "session".to_string()],
            Some(vec!["git".to_string(), "add".to_string(), "*".to_string()]),
            /*command_prefix*/ None,
            /*path*/ None,
            /*host*/ None,
        ));

        assert_eq!(
            item_names(&params),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for `git add *` in this session",
                "No, continue without running it",
            ]
        );
    }

    #[test]
    fn session_item_label_falls_back_without_pattern() {
        let params = build_params(overlay_request(
            vec!["once".to_string(), "session".to_string()],
            /*command_pattern*/ None,
            /*command_prefix*/ None,
            /*path*/ None,
            /*host*/ None,
        ));

        assert_eq!(
            item_names(&params),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for `git add file.txt` in this session",
                "No, continue without running it",
            ]
        );
    }

    #[test]
    fn session_item_hidden_without_session_scope() {
        let params = build_params(overlay_request(
            vec!["once".to_string()],
            /*command_pattern*/ None,
            /*command_prefix*/ None,
            /*path*/ None,
            /*host*/ None,
        ));

        assert_eq!(
            item_names(&params),
            vec!["Yes, proceed", "No, continue without running it"]
        );
    }

    #[test]
    fn prefix_persist_item_shown_when_scope_and_prefix_present() {
        let params = build_params(overlay_request(
            vec![
                "once".to_string(),
                "session".to_string(),
                "command_prefix_persist".to_string(),
            ],
            /*command_pattern*/ None,
            Some(vec!["git".to_string(), "pull".to_string()]),
            /*path*/ None,
            /*host*/ None,
        ));

        assert_eq!(
            item_names(&params),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for `git add file.txt` in this session",
                "Yes, and don't ask again for commands that start with `git pull`",
                "No, continue without running it",
            ]
        );
    }

    #[test]
    fn session_item_ignores_opaque_tool_call_id_target() {
        let mut request = overlay_request(
            vec!["once".to_string(), "session".to_string()],
            /*command_pattern*/ None,
            /*command_prefix*/ None,
            /*path*/ None,
            /*host*/ None,
        );
        request.target = Some("call_00_O1jvONaD7gYFgfVFYGrr6527".to_string());
        let params = build_params(request);

        assert_eq!(
            item_names(&params),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for this command in this session",
                "No, continue without running it",
            ]
        );
    }

    #[test]
    fn path_and_host_items_shown_when_scoped() {
        let params = build_params(overlay_request(
            vec![
                "once".to_string(),
                "path_prefix".to_string(),
                "host".to_string(),
            ],
            /*command_pattern*/ None,
            /*command_prefix*/ None,
            Some("/tmp/out".to_string()),
            Some("api.example.com".to_string()),
        ));

        assert_eq!(
            item_names(&params),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for files under `/tmp/out`",
                "Yes, and allow `api.example.com` for this session",
                "No, continue without running it",
            ]
        );
    }

    #[test]
    fn session_item_label_truncates_multiline_heredoc() {
        let mut request = overlay_request(
            vec!["once".to_string(), "session".to_string()],
            /*command_pattern*/ None,
            /*command_prefix*/ None,
            /*path*/ None,
            /*host*/ None,
        );
        request.target =
            Some("cat > ~/Desktop/note.txt << 'EOF'\n窗外蝉鸣不止，风扇吱呀作响\nEOF".to_string());
        let params = build_params(request);
        let session = params
            .items
            .iter()
            .find(|item| item.name.contains("don't ask again"))
            .expect("session item");
        assert!(
            session.name.contains("cat > ~/Desktop/note.txt << 'EOF'…"),
            "expected first-line snippet with ellipsis, got {}",
            session.name
        );
        assert!(
            !session.name.contains("窗外"),
            "heredoc body must not appear in the option label: {}",
            session.name
        );
        assert!(
            !session.name.contains('\n'),
            "option label must stay single-line: {}",
            session.name
        );
    }

    #[test]
    fn snippet_for_approval_label_truncates_long_single_line() {
        let long = "a".repeat(80);
        let snippet = snippet_for_approval_label(&long);
        assert_eq!(
            snippet.chars().count(),
            APPROVAL_LABEL_SNIPPET_MAX_GRAPHEMES
        );
        assert!(snippet.ends_with("..."));
    }
}
