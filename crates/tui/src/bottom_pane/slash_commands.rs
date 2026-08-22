use std::str::FromStr;

use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuiltinCommandFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) fast_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) realtime_conversation_enabled: bool,
    pub(crate) audio_device_selection_enabled: bool,
    pub(crate) allow_elevate_sandbox: bool,
}

pub(crate) fn builtins_for_input(_flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    built_in_slash_commands()
}

pub(crate) fn find_builtin_command(
    name: &str,
    _flags: BuiltinCommandFlags,
) -> Option<SlashCommand> {
    SlashCommand::from_str(name).ok()
}
