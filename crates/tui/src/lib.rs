//! Interactive terminal UI.
//!
//! public entry point for launching the CLI TUI.
#![allow(dead_code)]
mod ansi_escape;
mod app;
pub(crate) mod app_command;
mod app_event;
mod app_event_sender;
mod bottom_pane;
mod chatwidget;
#[cfg(test)]
mod chatwidget_tail_follow_tests;
#[cfg(test)]
mod chatwidget_tests;
mod clipboard_copy;
mod clipboard_paste;
mod color;
mod custom_terminal;
#[cfg(test)]
mod custom_terminal_clear_tests;
mod diff_render;
mod elapsed;
mod events;
mod exec_cell;
mod exec_command;
mod get_git_diff;
mod history_cell;
mod host_overlay;
#[cfg(test)]
mod inline_onboarding_tests;
mod insert_history;
mod interactive;
mod key_hint;
mod line_truncation;
mod live_wrap;
mod markdown;
pub mod markdown_render;
mod markdown_stream;
#[cfg(test)]
mod mcp_command_tests;
mod mcp_picker;
mod mcp_servers;
#[cfg(test)]
mod model_display_tests;
mod onboarding;
mod onboarding_viewport;
mod onboarding_widget;
#[cfg(test)]
mod onboarding_widget_tests;
mod pager_overlay;
mod queue_ops;
mod read_display;
mod render;
mod shell_output;
mod shimmer;
mod skills_picker;
mod slash_command;
mod startup_header;
mod startup_logo_cell;
mod state;
mod status_indicator_widget;
mod streaming;
mod style;
mod terminal_detection;
mod terminal_palette;
mod test_backend;
mod text_formatting;
mod theme;
mod tool_io_cell;
#[cfg(test)]
mod tool_rendering_e2e_tests;
mod tool_result_cell;
mod tui;
mod ui_consts;
mod version;
mod worker;
#[cfg(test)]
mod worker_queue_compaction_tests;
mod wrapping;

pub use interactive::run_interactive_tui;

pub use app::AppExit;
pub use app::InitialTuiSession;
pub use app::InteractiveTuiConfig;
pub use events::SavedModelEntry;
