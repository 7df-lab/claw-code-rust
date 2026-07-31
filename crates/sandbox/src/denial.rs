//! Heuristics for detecting OS sandbox denials in command output.

/// Whether sandbox was active for this attempt. When false, never treat as sandbox denial.
pub fn is_likely_sandbox_denied(
    sandbox_was_active: bool,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> bool {
    if !sandbox_was_active || exit_code == 0 {
        return false;
    }

    if streams_contain_sandbox_keyword(stdout, stderr) {
        return true;
    }

    const QUICK_REJECT_EXIT_CODES: [i32; 3] = [2, 126, 127];
    if QUICK_REJECT_EXIT_CODES.contains(&exit_code) {
        return false;
    }

    #[cfg(unix)]
    {
        const EXIT_CODE_SIGNAL_BASE: i32 = 128;
        if exit_code == EXIT_CODE_SIGNAL_BASE + libc::SIGSYS {
            return true;
        }
    }

    false
}

/// Detect a signal that specifically indicates sandbox enforcement.
///
/// Empty output is not evidence by itself: a child can crash in `pre_exec` or
/// abort for unrelated reasons. Linux seccomp's `SIGSYS` is the only bare
/// signal classified as a denial; other signals require a denial keyword.
pub fn is_likely_sandbox_denied_after_signal(
    sandbox_was_active: bool,
    signal: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> bool {
    if !sandbox_was_active {
        return false;
    }
    if streams_contain_sandbox_keyword(stdout, stderr) {
        return true;
    }
    #[cfg(unix)]
    if signal == Some(libc::SIGSYS) {
        return true;
    }
    #[cfg(not(unix))]
    let _ = signal;
    false
}

fn streams_contain_sandbox_keyword(stdout: &str, stderr: &str) -> bool {
    [stderr, stdout]
        .into_iter()
        .any(output_text_suggests_sandbox_denial)
}

/// Whether free-form command output text looks like an OS sandbox denial.
///
/// Kept narrower than a bare substring search for `"sandbox"` so routine tool
/// chatter does not look like a denial. Prefer [`is_likely_sandbox_denied`]
/// when an exit code is available (excludes 2/126/127).
pub fn output_text_suggests_sandbox_denial(text: &str) -> bool {
    const SANDBOX_DENIED_KEYWORDS: [&str; 5] = [
        "operation not permitted",
        "permission denied",
        "read-only file system",
        "seccomp",
        "landlock",
    ];
    let lower = text.to_lowercase();
    SANDBOX_DENIED_KEYWORDS
        .iter()
        .any(|needle| lower.contains(needle))
}

fn sandbox_was_active(profile: Option<&str>) -> bool {
    match profile {
        None | Some("off") | Some("none") => false,
        Some(_) => true,
    }
}

pub(crate) fn format_sandbox_denied_error(exit_code: i32, result_text: &str) -> String {
    format!(
        "SANDBOX_DENIED: The command was blocked by the OS sandbox. Retry with sandbox_permissions: \"require_escalated\" and a justification if you need to run outside the sandbox.\n\nexit code {exit_code}\n{result_text}"
    )
}

pub fn shell_error_message(
    sandbox_profile: Option<&str>,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
    result_text: &str,
) -> String {
    shell_error_message_with_signal(
        sandbox_profile,
        Some(exit_code),
        /*signal*/ None,
        stdout,
        stderr,
        result_text,
    )
}

/// Like [`shell_error_message`], but distinguishes signal death (`exit_code` is
/// `None`) from a normal non-zero exit.
pub fn shell_error_message_with_signal(
    sandbox_profile: Option<&str>,
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout: &str,
    stderr: &str,
    result_text: &str,
) -> String {
    let sandbox_active = sandbox_was_active(sandbox_profile);
    let reported_code = exit_code.unwrap_or_else(|| signal.map(|sig| 128 + sig).unwrap_or(-1));
    let denied = match exit_code {
        Some(code) => is_likely_sandbox_denied(sandbox_active, code, stdout, stderr),
        None => is_likely_sandbox_denied_after_signal(sandbox_active, signal, stdout, stderr),
    };
    if denied {
        format_sandbox_denied_error(reported_code, result_text)
    } else {
        format!("exit code {reported_code}\n{result_text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn inactive_sandbox_never_reports_denial() {
        assert!(!is_likely_sandbox_denied(
            false,
            1,
            "",
            "operation not permitted"
        ));
    }

    #[test]
    fn success_exit_never_reports_denial() {
        assert!(!is_likely_sandbox_denied(true, 0, "", "permission denied"));
    }

    #[test]
    fn bare_sandbox_word_is_not_enough_without_exit_code_gate() {
        assert!(!output_text_suggests_sandbox_denial(
            "retrying outside the sandbox helper"
        ));
        assert!(!output_text_suggests_sandbox_denial(
            "failed to write file: disk full"
        ));
        assert!(output_text_suggests_sandbox_denial(
            "Operation not permitted"
        ));
    }

    #[test]
    fn keyword_in_stderr_detects_denial() {
        assert!(is_likely_sandbox_denied(
            true,
            1,
            "",
            "bash: line 1: /tmp/x: Operation not permitted"
        ));
    }

    #[test]
    fn keyword_in_stdout_detects_denial() {
        assert!(is_likely_sandbox_denied(
            true,
            1,
            "read-only file system",
            ""
        ));
    }

    #[test]
    fn quick_reject_exit_codes_are_not_denials() {
        for exit_code in [2, 126, 127] {
            assert!(
                !is_likely_sandbox_denied(true, exit_code, "", "command not found"),
                "exit code {exit_code} should not be treated as sandbox denial"
            );
        }
    }

    #[test]
    fn unrelated_failure_is_not_denial() {
        assert!(!is_likely_sandbox_denied(true, 1, "command not found", ""));
    }

    #[cfg(unix)]
    #[test]
    fn sigsys_exit_code_detects_denial() {
        assert!(is_likely_sandbox_denied(true, 128 + libc::SIGSYS, "", ""));
    }

    #[cfg(unix)]
    #[test]
    fn unrelated_empty_signal_death_is_not_denial() {
        for signal in [libc::SIGTRAP, libc::SIGABRT, libc::SIGKILL] {
            assert!(!is_likely_sandbox_denied_after_signal(
                true,
                Some(signal),
                "",
                ""
            ));
        }
        assert!(!is_likely_sandbox_denied_after_signal(
            false,
            Some(9),
            "",
            ""
        ));
        assert!(!is_likely_sandbox_denied_after_signal(
            true, /*signal*/ None, "", ""
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sigsys_signal_detects_denial() {
        assert!(is_likely_sandbox_denied_after_signal(
            true,
            Some(libc::SIGSYS),
            "",
            ""
        ));
    }

    #[test]
    fn shell_error_message_prefixes_sandbox_hint() {
        let message = shell_error_message(
            Some("strict"),
            1,
            "",
            "operation not permitted",
            "[stderr]\noperation not permitted",
        );
        assert!(message.starts_with("SANDBOX_DENIED:"));
        assert!(message.contains("exit code 1"));
    }

    #[test]
    fn shell_error_message_skips_hint_when_sandbox_off() {
        let message = shell_error_message(
            Some("off"),
            1,
            "",
            "operation not permitted",
            "[stderr]\noperation not permitted",
        );
        assert_eq!(message, "exit code 1\n[stderr]\noperation not permitted");
    }

    #[cfg(unix)]
    #[test]
    fn shell_error_message_with_signal_reports_unrelated_crash() {
        let message = shell_error_message_with_signal(
            Some("workspace"),
            /*exit_code*/ None,
            Some(libc::SIGTRAP),
            "",
            "",
            "",
        );
        assert_eq!(message, format!("exit code {}\n", 128 + libc::SIGTRAP));
    }
}
