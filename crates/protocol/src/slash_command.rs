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
    Context,
    Permissions,
    ShowReasoning,
    Clear,
    Diff,
    Exit,
    Btw,
    Goal,
}

impl SlashCommand {
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Theme => "switch the UI theme",
            SlashCommand::Model => "choose the active model",
            SlashCommand::Skills => "browse available skills",
            SlashCommand::Mcp => "browse MCP servers",
            SlashCommand::Compact => "compact the current session context",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::New => "start a new chat",
            SlashCommand::Rename => "rename the current session",
            SlashCommand::Delete => "delete the current session and start a new one",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Context => "show context window occupancy by category",
            SlashCommand::Permissions => {
                "choose what Devo is allowed to do (also sets the OS sandbox)"
            }
            SlashCommand::ShowReasoning => {
                "choose how reasoning content is shown in the transcript"
            }
            SlashCommand::Clear => "clear the current transcript",
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
            SlashCommand::Context => "context",
            SlashCommand::Permissions => "permissions",
            SlashCommand::ShowReasoning => "show-reasoning",
            SlashCommand::Clear => "clear",
            SlashCommand::Diff => "diff",
            SlashCommand::Btw => "btw",
            SlashCommand::Goal => "goal",
            SlashCommand::Exit => "exit",
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
            | SlashCommand::Context
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Clear
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
            | SlashCommand::Context
            | SlashCommand::Permissions
            | SlashCommand::ShowReasoning
            | SlashCommand::Clear
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
            "status" => Ok(Self::Status),
            "context" => Ok(Self::Context),
            "permissions" | "approvals" => Ok(Self::Permissions),
            "show-reasoning" | "reasoning-view" => Ok(Self::ShowReasoning),
            "clear" => Ok(Self::Clear),
            "diff" => Ok(Self::Diff),
            "btw" => Ok(Self::Btw),
            "goal" => Ok(Self::Goal),
            "exit" => Ok(Self::Exit),
            _ => Err(()),
        }
    }
}

pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    vec![
        ("theme", SlashCommand::Theme),
        ("model", SlashCommand::Model),
        ("skills", SlashCommand::Skills),
        ("mcps", SlashCommand::Mcp),
        ("compact", SlashCommand::Compact),
        ("resume", SlashCommand::Resume),
        ("new", SlashCommand::New),
        ("rename", SlashCommand::Rename),
        ("delete", SlashCommand::Delete),
        ("status", SlashCommand::Status),
        ("context", SlashCommand::Context),
        ("permissions", SlashCommand::Permissions),
        ("show-reasoning", SlashCommand::ShowReasoning),
        ("clear", SlashCommand::Clear),
        ("diff", SlashCommand::Diff),
        ("goal", SlashCommand::Goal),
        ("btw", SlashCommand::Btw),
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
    }

    #[test]
    fn configuration_slash_commands_are_unavailable_during_task() {
        assert!(!SlashCommand::Model.available_during_task());
        assert!(!SlashCommand::Permissions.available_during_task());
        assert!(!SlashCommand::Theme.available_during_task());
        assert!(!SlashCommand::Delete.available_during_task());
        assert!(SlashCommand::Status.available_during_task());
        assert!(SlashCommand::Context.available_during_task());
        assert!(SlashCommand::Goal.available_during_task());
        assert!(SlashCommand::Rename.available_during_task());
    }

    #[test]
    fn context_slash_command_parses_and_is_available_during_task() {
        assert_eq!("context".parse::<SlashCommand>(), Ok(SlashCommand::Context));
        assert!(SlashCommand::Context.available_during_task());
        assert!(!SlashCommand::Context.available_over_acp());
        assert_eq!(
            SlashCommand::Context.description(),
            "show context window occupancy by category"
        );
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "context" && *command == SlashCommand::Context)
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
}
