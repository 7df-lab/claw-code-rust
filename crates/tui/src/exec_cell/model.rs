//! Data model for grouped exec-call history cells in the TUI transcript.
//!
//! An `ExecCell` can represent either a single command or an "exploring" group of related read/
//! list/search commands. The chat widget relies on stable `call_id` matching to route progress and
//! end events into the right cell, and it treats "call id not found" as a real signal (for
//! example, an orphan end that should render as a separate history entry).

use std::time::Duration;
use std::time::Instant;

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub(crate) struct CommandOutput {
    pub(crate) exit_code: i32,
    /// The aggregated stderr + stdout interleaved.
    pub(crate) aggregated_output: String,
    /// The formatted output of the command, as seen by the model.
    pub(crate) formatted_output: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecCall {
    pub(crate) call_id: String,
    pub(crate) command: Vec<String>,
    pub(crate) parsed: Vec<ParsedCommand>,
    pub(crate) output: Option<CommandOutput>,
    pub(crate) source: ExecCommandSource,
    pub(crate) start_time: Option<Instant>,
    pub(crate) duration: Option<Duration>,
    pub(crate) interaction_input: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) tool_input: Option<Value>,
    pub(crate) tool_output: Option<Value>,
    pub(crate) tool_display_content: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ExecCell {
    pub(crate) calls: Vec<ExecCall>,
    animations_enabled: bool,
}

impl ExecCell {
    pub(crate) fn new(call: ExecCall, animations_enabled: bool) -> Self {
        Self {
            calls: vec![call],
            animations_enabled,
        }
    }

    pub(crate) fn with_added_call(
        &self,
        call_id: String,
        command: Vec<String>,
        parsed: Vec<ParsedCommand>,
        source: ExecCommandSource,
        interaction_input: Option<String>,
    ) -> Option<Self> {
        let call = ExecCall {
            call_id,
            command,
            parsed,
            output: None,
            source,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            tool_display_content: None,
        };
        if self.is_exploring_cell() && Self::is_exploring_call(&call) {
            let mut calls = self.calls.clone();
            calls.push(call);
            Some(Self {
                calls,
                animations_enabled: self.animations_enabled,
            })
        } else {
            None
        }
    }

    /// Marks the most recently matching call as finished and returns whether a call was found.
    ///
    /// Callers should treat `false` as a routing mismatch rather than silently ignoring it. The
    /// chat widget uses that signal to avoid attaching an orphan `exec_end` event to an unrelated
    /// active exploring cell, which would incorrectly collapse two transcript entries together.
    pub(crate) fn complete_call(
        &mut self,
        call_id: &str,
        output: CommandOutput,
        duration: Duration,
    ) -> bool {
        let Some(call) = self.calls.iter_mut().rev().find(|c| c.call_id == call_id) else {
            return false;
        };
        match call.output.as_mut() {
            Some(existing) if output.aggregated_output.is_empty() => {
                existing.exit_code = output.exit_code;
                if !output.formatted_output.is_empty() {
                    existing.formatted_output = output.formatted_output;
                }
            }
            _ => {
                call.output = Some(output);
            }
        }
        call.duration = Some(duration);
        call.start_time = None;
        true
    }

    pub(crate) fn update_call(
        &mut self,
        call_id: &str,
        command: Vec<String>,
        parsed: Vec<ParsedCommand>,
    ) -> bool {
        let Some(call) = self.calls.iter_mut().rev().find(|c| c.call_id == call_id) else {
            return false;
        };
        call.command = command;
        call.parsed = parsed;
        true
    }

    pub(crate) fn should_flush(&self) -> bool {
        !self.is_exploring_cell() && self.calls.iter().all(|c| c.output.is_some())
    }

    pub(crate) fn mark_failed(&mut self) {
        for call in self.calls.iter_mut() {
            if call.output.is_none() {
                let elapsed = call
                    .start_time
                    .map(|st| st.elapsed())
                    .unwrap_or_else(|| Duration::from_millis(0));
                call.start_time = None;
                call.duration = Some(elapsed);
                call.output = Some(CommandOutput {
                    exit_code: 1,
                    formatted_output: String::new(),
                    aggregated_output: String::new(),
                });
            }
        }
    }

    pub(crate) fn is_exploring_cell(&self) -> bool {
        self.calls.iter().all(Self::is_exploring_call)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.calls.iter().any(|c| c.output.is_none())
    }

    pub(crate) fn active_start_time(&self) -> Option<Instant> {
        self.calls
            .iter()
            .find(|c| c.output.is_none())
            .and_then(|c| c.start_time)
    }

    pub(crate) fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    pub(crate) fn iter_calls(&self) -> impl Iterator<Item = &ExecCall> {
        self.calls.iter()
    }

    pub(crate) fn contains_call(&self, call_id: &str) -> bool {
        self.calls.iter().any(|call| call.call_id == call_id)
    }

    pub(crate) fn append_output(&mut self, call_id: &str, chunk: &str) -> bool {
        if chunk.is_empty() {
            return false;
        }
        let Some(call) = self.calls.iter_mut().rev().find(|c| c.call_id == call_id) else {
            return false;
        };
        let output = call.output.get_or_insert_with(CommandOutput::default);
        output.aggregated_output.push_str(chunk);
        true
    }

    pub(crate) fn set_tool_io_input(
        &mut self,
        call_id: &str,
        tool_name: String,
        input: Value,
    ) -> bool {
        let Some(call) = self.calls.iter_mut().rev().find(|c| c.call_id == call_id) else {
            return false;
        };
        call.tool_name = Some(tool_name);
        call.tool_input = Some(input);
        true
    }

    pub(crate) fn complete_tool_io(
        &mut self,
        call_id: &str,
        output: Value,
        display_content: Option<String>,
    ) -> bool {
        let Some(call) = self.calls.iter_mut().rev().find(|c| c.call_id == call_id) else {
            return false;
        };
        call.tool_output = Some(output);
        call.tool_display_content = display_content;
        true
    }

    pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
        !matches!(call.source, ExecCommandSource::UserShell)
            && !call.parsed.is_empty()
            && call.parsed.iter().all(|p| {
                matches!(
                    p,
                    ParsedCommand::Read { .. }
                        | ParsedCommand::ListFiles { .. }
                        | ParsedCommand::Search { .. }
                )
            })
    }
}

impl ExecCall {
    pub(crate) fn is_user_shell_command(&self) -> bool {
        matches!(self.source, ExecCommandSource::UserShell)
    }

    pub(crate) fn is_unified_exec_interaction(&self) -> bool {
        matches!(self.source, ExecCommandSource::UnifiedExecInteraction)
    }

    pub(crate) fn is_agent_shell_tool_call(&self) -> bool {
        if self.is_user_shell_command() || self.is_unified_exec_interaction() {
            return false;
        }
        match self.tool_name.as_deref() {
            Some(name) => crate::transcript::tool_state::is_shell_tool_name(name),
            None => matches!(self.source, ExecCommandSource::Agent),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::PathBuf;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    fn exploring_call(index: usize) -> ExecCall {
        ExecCall {
            call_id: format!("call-{index}"),
            command: vec!["rg".into(), format!("needle-{index}")],
            parsed: vec![ParsedCommand::Search {
                cmd: format!("rg needle-{index}"),
                query: Some(format!("needle-{index}")),
                path: Some("crates/tui/src".to_string()),
            }],
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            tool_display_content: None,
        }
    }

    fn exploring_cell(call_count: usize) -> ExecCell {
        ExecCell {
            calls: (0..call_count).map(exploring_call).collect(),
            animations_enabled: false,
        }
    }

    #[test]
    fn with_added_call_appends_exploring_call_in_order() {
        let cell = exploring_cell(2);

        let actual = cell
            .with_added_call(
                "call-2".to_string(),
                vec!["rg".into(), "needle-2".into()],
                vec![ParsedCommand::Search {
                    cmd: "rg needle-2".to_string(),
                    query: Some("needle-2".to_string()),
                    path: Some("crates/tui/src".to_string()),
                }],
                ExecCommandSource::Agent,
                None,
            )
            .expect("exploring call should append to exploring cell");

        let call_ids = actual
            .calls
            .iter()
            .map(|call| call.call_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(call_ids, vec!["call-0", "call-1", "call-2"]);
        assert!(cell.calls.iter().all(|call| call.output.is_none()));
    }

    #[test]
    fn with_added_call_rejects_user_shell_call() {
        let cell = exploring_cell(1);

        let actual = cell.with_added_call(
            "shell".to_string(),
            vec!["bash".into(), "-lc".into(), "date".into()],
            vec![ParsedCommand::Unknown {
                cmd: "date".to_string(),
            }],
            ExecCommandSource::UserShell,
            None,
        );

        assert!(actual.is_none());
    }

    #[test]
    #[ignore]
    fn bench_with_added_call_on_exploring_cell() {
        let cell = exploring_cell(256);
        let parsed = vec![ParsedCommand::Read {
            cmd: "sed -n '1,40p' crates/tui/src/exec_cell/model.rs".to_string(),
            name: "model.rs".to_string(),
            path: PathBuf::from("crates/tui/src/exec_cell/model.rs"),
        }];

        let started = Instant::now();
        let mut total_calls = 0;
        for index in 0..20_000 {
            let next = black_box(&cell)
                .with_added_call(
                    format!("new-call-{index}"),
                    vec!["sed".into(), "-n".into(), "1,40p".into()],
                    black_box(parsed.clone()),
                    ExecCommandSource::Agent,
                    None,
                )
                .expect("exploring call should append");
            total_calls += black_box(next.calls.len());
        }
        let elapsed = started.elapsed();

        assert_eq!(total_calls, 5_140_000);
        println!(
            "with_added_call_on_exploring_cell iterations=20000 existing_calls=256 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 20_000.0
        );
    }
}
