use std::str::FromStr;

use crate::AcpAvailableCommand;
use crate::AcpAvailableCommandInput;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashCommand {
    Theme,
    Model,
    Skills,
    Mcp,
    Compact,
    Resume,
    New,
    Rename,
    Delete,
    Status,
    Settings,
    Permissions,
    ShowReasoning,
    Diff,
    Exit,
    Btw,
    Goal,
}

impl SlashCommand {
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Theme => "open appearance settings to switch the UI theme",
            SlashCommand::Model => "choose the active model",
            SlashCommand::Skills => "browse available skills",
            SlashCommand::Mcp => "browse MCP servers",
            SlashCommand::Compact => "compact the current session context",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::New => "start a new chat",
            SlashCommand::Rename => "rename the current session",
            SlashCommand::Delete => "delete the current session and start a new one",
            SlashCommand::Status => "show cwd, permissions, and context window occupancy",
            SlashCommand::Settings => "open session and appearance settings",
            SlashCommand::Permissions => {
                "choose what Devo is allowed to do (also sets the OS sandbox)"
            }
            SlashCommand::ShowReasoning => {
                "choose how reasoning content is shown in the transcript"
            }
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Btw => {
                "Ask a quick side question without interrupting the main conversation"
            }
            SlashCommand::Goal => "set or view the goal for a long-running task",
            SlashCommand::Exit => "exit Devo",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            SlashCommand::Theme => "theme",
            SlashCommand::Model => "model",
            SlashCommand::Skills => "skills",
            SlashCommand::Mcp => "mcps",
            SlashCommand::Compact => "compact",
            SlashCommand::Resume => "resume",
            SlashCommand::New => "new",
            SlashCommand::Rename => "rename",
            SlashCommand::Delete => "delete",
            SlashCommand::Status => "status",
            SlashCommand::Settings => "settings",
            SlashCommand::Permissions => "permissions",
            SlashCommand::ShowReasoning => "show-reasoning",
            SlashCommand::Diff => "diff",
            SlashCommand::Btw => "btw",
            SlashCommand::Goal => "goal",
            SlashCommand::Exit => "exit",
        }
    }

    /// Soft alternate names accepted by parsing / popup filtering.
    ///
    /// These are not listed as separate built-in entries; the canonical
    /// [`SlashCommand::command`] remains the display and insertion name.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            SlashCommand::Exit => &["quit"],
            SlashCommand::Theme
            | SlashCommand::Model
            | SlashCommand::Skills
            | SlashCommand::Mcp
            | SlashCommand::Compact
            | SlashCommand::Resume
            | SlashCommand::New
            | SlashCommand::Rename
            | SlashCommand::Delete
            | SlashCommand::Status
            | SlashCommand::Settings
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Diff
            | SlashCommand::Btw
            | SlashCommand::Goal => &[],
        }
    }

    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Model | SlashCommand::Btw | SlashCommand::Goal | SlashCommand::Rename
        )
    }

    pub fn parameter_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Btw => Some("<side conversation message>"),
            SlashCommand::Goal => Some("<objective for autonomous work>"),
            SlashCommand::Rename => Some("<new title>"),
            SlashCommand::Theme
            | SlashCommand::Model
            | SlashCommand::Skills
            | SlashCommand::Mcp
            | SlashCommand::Compact
            | SlashCommand::Resume
            | SlashCommand::New
            | SlashCommand::Delete
            | SlashCommand::Status
            | SlashCommand::Settings
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Diff
            | SlashCommand::Exit => None,
        }
    }

    pub fn available_during_task(self) -> bool {
        !matches!(
            self,
            SlashCommand::Model
                | SlashCommand::Theme
                | SlashCommand::Compact
                | SlashCommand::Diff
                | SlashCommand::New
                | SlashCommand::Delete
                | SlashCommand::Resume
                | SlashCommand::Permissions
                | SlashCommand::Settings
        )
    }

    pub fn available_over_acp(self) -> bool {
        matches!(self, SlashCommand::Compact | SlashCommand::Goal)
    }

    fn acp_input_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Goal => Some("objective, pause, resume, or clear"),
            SlashCommand::Theme
            | SlashCommand::Model
            | SlashCommand::Skills
            | SlashCommand::Mcp
            | SlashCommand::Compact
            | SlashCommand::Resume
            | SlashCommand::New
            | SlashCommand::Rename
            | SlashCommand::Delete
            | SlashCommand::Status
            | SlashCommand::Settings
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Diff
            | SlashCommand::Exit
            | SlashCommand::Btw => None,
        }
    }
}

impl FromStr for SlashCommand {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "theme" => Ok(Self::Theme),
            "model" => Ok(Self::Model),
            "skills" => Ok(Self::Skills),
            "mcps" | "mcp" => Ok(Self::Mcp),
            "compact" => Ok(Self::Compact),
            "resume" => Ok(Self::Resume),
            "new" => Ok(Self::New),
            "rename" => Ok(Self::Rename),
            "delete" => Ok(Self::Delete),
            // `/context` remains a soft alias for the status panel.
            "status" | "context" => Ok(Self::Status),
            "settings" => Ok(Self::Settings),
            "permissions" | "approvals" => Ok(Self::Permissions),
            "show-reasoning" | "reasoning-view" => Ok(Self::ShowReasoning),
            "diff" => Ok(Self::Diff),
            "btw" => Ok(Self::Btw),
            "goal" => Ok(Self::Goal),
            // `/quit` is a soft alias for exit.
            "exit" | "quit" => Ok(Self::Exit),
            _ => Err(()),
        }
    }
}

pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    // Presentation order for the TUI popup. Do not alphabetize: keep frequently used
    // commands first so useful actions remain visible without scrolling.
    vec![
        ("model", SlashCommand::Model),
        ("permissions", SlashCommand::Permissions),
        ("status", SlashCommand::Status),
        ("new", SlashCommand::New),
        ("resume", SlashCommand::Resume),
        ("compact", SlashCommand::Compact),
        ("diff", SlashCommand::Diff),
        ("goal", SlashCommand::Goal),
        ("btw", SlashCommand::Btw),
        ("skills", SlashCommand::Skills),
        ("mcps", SlashCommand::Mcp),
        ("settings", SlashCommand::Settings),
        ("rename", SlashCommand::Rename),
        ("delete", SlashCommand::Delete),
        ("show-reasoning", SlashCommand::ShowReasoning),
        ("exit", SlashCommand::Exit),
    ]
}

pub fn acp_slash_commands() -> Vec<SlashCommand> {
    vec![SlashCommand::Compact, SlashCommand::Goal]
}

pub fn acp_available_slash_commands() -> Vec<AcpAvailableCommand> {
    acp_slash_commands()
        .into_iter()
        .map(|command| AcpAvailableCommand {
            name: command.command().to_string(),
            description: command.description().to_string(),
            input: command
                .acp_input_hint()
                .map(|hint| AcpAvailableCommandInput {
                    hint: hint.to_string(),
                    meta: None,
                }),
            meta: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_slash_commands_export_server_backed_subset() {
        assert_eq!(
            acp_slash_commands(),
            vec![SlashCommand::Compact, SlashCommand::Goal]
        );
        assert_eq!(
            acp_available_slash_commands(),
            vec![
                AcpAvailableCommand {
                    name: "compact".to_string(),
                    description: "compact the current session context".to_string(),
                    input: None,
                    meta: None,
                },
                AcpAvailableCommand {
                    name: "goal".to_string(),
                    description: "set or view the goal for a long-running task".to_string(),
                    input: Some(AcpAvailableCommandInput {
                        hint: "objective, pause, resume, or clear".to_string(),
                        meta: None,
                    }),
                    meta: None,
                },
            ]
        );
    }

    #[test]
    fn tui_only_slash_commands_are_not_available_over_acp() {
        assert!(!SlashCommand::Theme.available_over_acp());
        assert!(!SlashCommand::Model.available_over_acp());
        assert!(!SlashCommand::Btw.available_over_acp());
        assert!(!SlashCommand::Exit.available_over_acp());
        assert!(!SlashCommand::Rename.available_over_acp());
        assert!(!SlashCommand::Delete.available_over_acp());
        assert!(!SlashCommand::Settings.available_over_acp());
    }

    #[test]
    fn configuration_slash_commands_are_unavailable_during_task() {
        assert!(!SlashCommand::Model.available_during_task());
        assert!(!SlashCommand::Permissions.available_during_task());
        assert!(!SlashCommand::Theme.available_during_task());
        assert!(!SlashCommand::Delete.available_during_task());
        assert!(!SlashCommand::Settings.available_during_task());
        assert!(SlashCommand::Status.available_during_task());
        assert!(SlashCommand::Goal.available_during_task());
        assert!(SlashCommand::Rename.available_during_task());
    }

    #[test]
    fn theme_slash_command_is_hidden_alias_for_settings() {
        assert_eq!("theme".parse::<SlashCommand>(), Ok(SlashCommand::Theme));
        assert!(!SlashCommand::Theme.available_during_task());
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "theme")
        );
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "settings" && *command == SlashCommand::Settings)
        );
    }

    #[test]
    fn status_slash_command_parses_and_context_is_alias() {
        assert_eq!("status".parse::<SlashCommand>(), Ok(SlashCommand::Status));
        assert_eq!("context".parse::<SlashCommand>(), Ok(SlashCommand::Status));
        assert!(SlashCommand::Status.available_during_task());
        assert!(!SlashCommand::Status.available_over_acp());
        assert_eq!(
            SlashCommand::Status.description(),
            "show cwd, permissions, and context window occupancy"
        );
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "status" && *command == SlashCommand::Status)
        );
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "context")
        );
    }

    #[test]
    fn settings_slash_command_parses_and_is_unavailable_during_task() {
        assert_eq!(
            "settings".parse::<SlashCommand>(),
            Ok(SlashCommand::Settings)
        );
        assert!(!SlashCommand::Settings.available_during_task());
        assert_eq!(
            SlashCommand::Settings.description(),
            "open session and appearance settings"
        );
    }

    #[test]
    fn rename_and_delete_slash_commands_parse_and_describe() {
        assert_eq!("rename".parse::<SlashCommand>(), Ok(SlashCommand::Rename));
        assert_eq!("delete".parse::<SlashCommand>(), Ok(SlashCommand::Delete));
        assert!(SlashCommand::Rename.supports_inline_args());
        assert!(!SlashCommand::Delete.supports_inline_args());
        assert_eq!(SlashCommand::Rename.parameter_hint(), Some("<new title>"));
        assert_eq!(SlashCommand::Delete.parameter_hint(), None);
    }

    #[test]
    fn exit_slash_command_parses_and_quit_is_alias() {
        assert_eq!("exit".parse::<SlashCommand>(), Ok(SlashCommand::Exit));
        assert_eq!("quit".parse::<SlashCommand>(), Ok(SlashCommand::Exit));
        assert_eq!(SlashCommand::Exit.command(), "exit");
        assert_eq!(SlashCommand::Exit.aliases(), &["quit"]);
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "exit" && *command == SlashCommand::Exit)
        );
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "quit")
        );
    }

    #[test]
    fn clear_slash_command_is_removed() {
        assert_eq!("clear".parse::<SlashCommand>(), Err(()));
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "clear")
        );
    }
}
