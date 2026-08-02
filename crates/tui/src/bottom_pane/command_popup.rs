use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::WidgetRef;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::render_rows;
use super::slash_commands;
use crate::render::Insets;
use crate::render::RectExt;
use crate::slash_command::SlashCommand;

const ALIAS_COMMANDS: &[SlashCommand] = &[];

/// A selectable item in the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandItem {
    Builtin(SlashCommand),
}

pub(crate) struct CommandPopup {
    command_filter: String,
    builtins: Vec<(&'static str, SlashCommand)>,
    state: ScrollState,
    accent_color: Color,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CommandPopupFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) fast_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) realtime_conversation_enabled: bool,
    pub(crate) audio_device_selection_enabled: bool,
    pub(crate) windows_degraded_sandbox_active: bool,
}

impl From<CommandPopupFlags> for slash_commands::BuiltinCommandFlags {
    fn from(value: CommandPopupFlags) -> Self {
        Self {
            collaboration_modes_enabled: value.collaboration_modes_enabled,
            connectors_enabled: value.connectors_enabled,
            plugins_command_enabled: value.plugins_command_enabled,
            fast_command_enabled: value.fast_command_enabled,
            personality_command_enabled: value.personality_command_enabled,
            realtime_conversation_enabled: value.realtime_conversation_enabled,
            audio_device_selection_enabled: value.audio_device_selection_enabled,
            allow_elevate_sandbox: value.windows_degraded_sandbox_active,
        }
    }
}

impl CommandPopup {
    pub(crate) fn new(flags: CommandPopupFlags, accent_color: Color) -> Self {
        // Keep built-in availability in sync with the composer.
        let builtins: Vec<(&'static str, SlashCommand)> =
            slash_commands::builtins_for_input(flags.into())
                .into_iter()
                .collect();
        Self {
            command_filter: String::new(),
            builtins,
            state: ScrollState::new(),
            accent_color,
        }
    }

    /// Update the filter string based on the current composer text. The text
    /// passed in is expected to start with a leading '/'. Everything after the
    /// *first* '/' on the *first* line becomes the active filter that is used
    /// to narrow down the list of available commands.
    pub(crate) fn on_composer_text_change(&mut self, text: String) {
        let first_line = text.lines().next().unwrap_or("");

        if let Some(stripped) = first_line.strip_prefix('/') {
            // Extract the *first* token (sequence of non-whitespace
            // characters) after the slash so that `/status something` still
            // shows the help for `/status`.
            let token = stripped.trim_start();
            let cmd_token = token.split_whitespace().next().unwrap_or("");

            // Update the filter keeping the original case (commands are all
            // lower-case for now but this may change in the future).
            self.command_filter = cmd_token.to_string();
        } else {
            // The composer no longer starts with '/'. Reset the filter so the
            // popup shows the *full* command list if it is still displayed
            // for some reason.
            self.command_filter.clear();
        }

        // Reset or clamp selected index based on new filtered list.
        let matches_len = self.filtered_items().len();
        self.state.clamp_selection(matches_len);
        self.state
            .ensure_visible(matches_len, MAX_POPUP_ROWS.min(matches_len));
    }

    /// Determine the preferred height of the popup for a given width.
    /// Accounts for wrapped descriptions so that long tooltips don't overflow.
    pub(crate) fn calculate_required_height(&self, width: u16) -> u16 {
        use super::selection_popup_common::measure_rows_height;
        let rows = self.rows_from_matches(self.filtered());

        measure_rows_height(&rows, &self.state, MAX_POPUP_ROWS, width)
    }

    /// Compute exact/prefix matches over built-in commands and user prompts,
    /// paired with optional highlight indices. Preserves the original
    /// presentation order for built-ins and prompts.
    fn filtered(&self) -> Vec<(CommandItem, Option<Vec<usize>>)> {
        let filter = self.command_filter.trim();
        let mut out: Vec<(CommandItem, Option<Vec<usize>>)> = Vec::new();
        if filter.is_empty() {
            for (_, cmd) in self.builtins.iter() {
                if ALIAS_COMMANDS.contains(cmd) {
                    continue;
                }
                out.push((CommandItem::Builtin(*cmd), None));
            }
            return out;
        }

        let filter_lower = filter.to_lowercase();
        let filter_chars = filter.chars().count();
        let mut exact: Vec<(CommandItem, Option<Vec<usize>>)> = Vec::new();
        let mut prefix: Vec<(CommandItem, Option<Vec<usize>>)> = Vec::new();
        let indices_for = |offset| Some((offset..offset + filter_chars).collect());

        let mut push_match = |item: CommandItem, display: &str, aliases: &[&str]| {
            let display_lower = display.to_lowercase();
            let display_exact = display_lower == filter_lower;
            let alias_exact = aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(filter));
            if display_exact || alias_exact {
                // Alias-only hits keep the canonical display name without a
                // misleading highlight span (filter text is not a substring).
                let indices = if display_exact { indices_for(0) } else { None };
                exact.push((item, indices));
                return;
            }
            let display_prefix = display_lower.starts_with(&filter_lower);
            let alias_prefix = aliases
                .iter()
                .any(|alias| alias.to_lowercase().starts_with(&filter_lower));
            if display_prefix || alias_prefix {
                let indices = if display_prefix { indices_for(0) } else { None };
                prefix.push((item, indices));
            }
        };

        for (_, cmd) in self.builtins.iter() {
            push_match(CommandItem::Builtin(*cmd), cmd.command(), cmd.aliases());
        }

        out.extend(exact);
        out.extend(prefix);
        out
    }

    fn filtered_items(&self) -> Vec<CommandItem> {
        self.filtered().into_iter().map(|(c, _)| c).collect()
    }

    fn rows_from_matches(
        &self,
        matches: Vec<(CommandItem, Option<Vec<usize>>)>,
    ) -> Vec<GenericDisplayRow> {
        matches
            .into_iter()
            .map(|(item, indices)| {
                let CommandItem::Builtin(cmd) = item;
                let name = format!("/{}", cmd.command());
                let description = cmd.description().to_string();
                GenericDisplayRow {
                    name,
                    name_prefix_spans: Vec::new(),
                    match_indices: indices.map(|v| v.into_iter().map(|i| i + 1).collect()),
                    display_shortcut: None,
                    description: Some(description),
                    category_tag: None,
                    wrap_indent: None,
                    is_disabled: false,
                    disabled_reason: None,
                }
            })
            .collect()
    }

    /// Move the selection cursor one step up.
    pub(crate) fn move_up(&mut self) {
        let len = self.filtered_items().len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    /// Move the selection cursor one step down.
    pub(crate) fn move_down(&mut self) {
        let matches_len = self.filtered_items().len();
        self.state.move_down_wrap(matches_len);
        self.state
            .ensure_visible(matches_len, MAX_POPUP_ROWS.min(matches_len));
    }

    /// Return currently selected command, if any.
    pub(crate) fn selected_item(&self) -> Option<CommandItem> {
        let matches = self.filtered_items();
        self.state
            .selected_idx
            .and_then(|idx| matches.get(idx).copied())
    }
}

impl WidgetRef for CommandPopup {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let rows = self.rows_from_matches(self.filtered());
        render_rows(
            area.inset(Insets::tlbr(
                /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
            )),
            buf,
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            "no matches",
            self.accent_color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::WidgetRef;

    use super::super::popup_consts::MAX_POPUP_ROWS;

    #[test]
    fn filter_returns_empty_for_unknown_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/in".to_string());
        assert!(popup.filtered_items().is_empty());
    }

    #[test]
    fn exact_match_selects_model() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/model".to_string());

        let selected = popup.selected_item();
        match selected {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "model"),
            None => panic!("expected a selected command for exact match"),
        }
    }

    #[test]
    fn model_is_first_suggestion_for_mo() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/mo".to_string());
        let matches = popup.filtered_items();
        match matches.first() {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "model"),
            None => panic!("expected at least one match for '/mo'"),
        }
    }

    #[test]
    fn filtered_commands_keep_presentation_order_for_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/m".to_string());

        let cmds: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();
        assert_eq!(cmds, vec!["model", "mcps"]);
    }

    #[test]
    fn prefix_filter_limits_matches_for_ac() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/ac".to_string());

        let cmds: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();
        assert!(
            !cmds.contains(&"compact"),
            "expected prefix search for '/ac' to exclude 'compact', got {cmds:?}"
        );
    }

    #[test]
    fn exit_is_visible_for_matching_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/ex".to_string());

        match popup.selected_item() {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "exit"),
            other => panic!("expected exit to be selected for exact match, got {other:?}"),
        }
    }

    #[test]
    fn quit_alias_prefix_selects_exit_without_listing_quit() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/".to_string());
        let listed: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();
        assert!(listed.contains(&"exit"));
        assert!(!listed.contains(&"quit"));

        popup.on_composer_text_change("/qu".to_string());
        match popup.selected_item() {
            Some(CommandItem::Builtin(cmd)) => {
                assert_eq!(cmd.command(), "exit");
                assert_eq!(cmd.aliases(), &["quit"]);
            }
            other => panic!("expected exit via quit alias, got {other:?}"),
        }
    }

    #[test]
    fn popup_lists_only_supported_commands() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/".to_string());

        let cmds: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();
        assert_eq!(
            cmds,
            vec![
                "model",
                "skills",
                "mcps",
                "compact",
                "resume",
                "new",
                "rename",
                "delete",
                "status",
                "settings",
                "permissions",
                "show-reasoning",
                "diff",
                "goal",
                "btw",
                "exit",
            ]
        );
    }

    #[test]
    fn settings_command_hidden_when_audio_device_selection_is_disabled() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                collaboration_modes_enabled: false,
                connectors_enabled: false,
                plugins_command_enabled: false,
                fast_command_enabled: false,
                personality_command_enabled: true,
                realtime_conversation_enabled: true,
                audio_device_selection_enabled: false,
                windows_degraded_sandbox_active: false,
            },
            Color::Cyan,
        );
        popup.on_composer_text_change("/aud".to_string());

        let cmds: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();

        assert!(
            !cmds.contains(&"settings"),
            "expected '/settings' to be hidden when audio device selection is disabled, got {cmds:?}"
        );
    }

    #[test]
    fn debug_commands_are_hidden_from_popup() {
        let popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        let cmds: Vec<&str> = popup
            .filtered_items()
            .into_iter()
            .map(|item| match item {
                CommandItem::Builtin(cmd) => cmd.command(),
            })
            .collect();

        assert!(
            !cmds.iter().any(|name| name.starts_with("debug")),
            "expected no /debug* command in popup menu, got {cmds:?}"
        );
    }

    fn render_popup(popup: &CommandPopup, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        popup.render_ref(area, &mut buf);
        (0..height)
            .map(|row| {
                (0..width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn full_command_list_shows_more_below_when_overflowing() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/".to_string());
        assert!(
            popup.filtered_items().len() > MAX_POPUP_ROWS,
            "expected more commands than visible popup rows"
        );

        let height = popup.calculate_required_height(80);
        let rendered = render_popup(&popup, 80, height);
        assert!(
            rendered.contains("↓ more"),
            "expected ↓ more when command list overflows:\n{rendered}"
        );
        assert!(
            !rendered.contains("↑ more"),
            "wrap-around slash lists should not show ↑ more:\n{rendered}"
        );
    }

    #[test]
    fn scrolled_command_list_does_not_show_more_above() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Color::Cyan);
        popup.on_composer_text_change("/".to_string());
        let len = popup.filtered_items().len();
        assert!(len > MAX_POPUP_ROWS);

        for _ in 0..MAX_POPUP_ROWS {
            popup.move_down();
        }

        let height = popup.calculate_required_height(80);
        let rendered = render_popup(&popup, 80, height);
        assert!(
            !rendered.contains("↑ more"),
            "wrap-around slash lists should not show ↑ more after scrolling:\n{rendered}"
        );
    }
}
