//! Shell binary resolution and command rewriting.

use std::path::PathBuf;

/// Resolved shell program + argv prefix used to run a command string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShellSpec {
    pub(crate) program: &'static str,
    pub(crate) args: &'static [&'static str],
}

/// Map an optional shell override (and login flag) to a concrete [`ShellSpec`].
pub(crate) fn resolve_shell(shell: Option<&str>, login: bool) -> ShellSpec {
    let shell = shell.unwrap_or("");
    let normalized = shell.to_ascii_lowercase();

    if normalized.contains("powershell") || normalized == "pwsh" || normalized == "powershell" {
        return ShellSpec {
            program: "powershell",
            args: &["-NoLogo", "-NoProfile", "-Command"],
        };
    }

    if normalized.ends_with("cmd") || normalized.ends_with("cmd.exe") || normalized == "cmd" {
        return ShellSpec {
            program: "cmd",
            args: &["/C"],
        };
    }

    if normalized.contains("zsh") {
        return ShellSpec {
            program: "zsh",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    if normalized.contains("bash") {
        return ShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    if login {
        platform_shell(true)
    } else {
        platform_shell(false)
    }
}

fn platform_shell(login: bool) -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            program: "powershell",
            args: &["-NoProfile", "-Command"],
        }
    } else {
        ShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        }
    }
}

#[cfg(test)]
pub(crate) fn platform_shell_program(login: bool) -> &'static str {
    platform_shell(login).program
}

/// Rewrite the command string for PowerShell UTF-8 console encoding when needed.
pub(crate) fn normalize_command_for_shell(shell: &ShellSpec, command: String) -> String {
    // PowerShell often emits mojibake without an explicit UTF-8 console encoding.
    if cfg!(windows) && shell.program.eq_ignore_ascii_case("powershell") {
        format!(
            concat!(
                "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                "[System.Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                "{}"
            ),
            command
        )
    } else {
        command
    }
}

/// Shared post-resolution knobs for pipe and PTY runners.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedShellRun {
    pub(crate) shell: ShellSpec,
    pub(crate) command_to_run: String,
    pub(crate) workdir: PathBuf,
    pub(crate) description: String,
    pub(crate) timeout_ms: u64,
    pub(crate) yield_time_ms: u64,
    pub(crate) max_output_tokens: usize,
    pub(crate) sandbox_profile: Option<String>,
}
