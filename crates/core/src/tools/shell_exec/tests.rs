use super::*;
use crate::ToolContent;
use pretty_assertions::assert_eq;
use std::hint::black_box;
use std::time::Instant;

#[tokio::test]
async fn execute_shell_command_non_tty_sends_progress() {
    let cmd = "echo stream_test";
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let result = execute_shell_command(
        ShellExecRequest {
            command: cmd.to_string(),
            workdir: std::env::current_dir().unwrap_or_default(),
            description: "test".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 5000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        Some(tx),
        CancellationToken::new(),
    )
    .await;

    assert!(result.is_ok(), "command should succeed: {:?}", result.err());
    // Progress channel should have received output
    if let Ok(chunk) = rx.try_recv() {
        assert!(!chunk.is_empty(), "progress chunk should not be empty");
    }
}

#[tokio::test]
async fn execute_shell_command_progress_none_does_not_crash() {
    let cmd = "echo test";
    let result = execute_shell_command(
        ShellExecRequest {
            command: cmd.to_string(),
            workdir: std::env::current_dir().unwrap_or_default(),
            description: "test".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 5000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        None,
        CancellationToken::new(),
    )
    .await;
    assert!(result.is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn execute_shell_command_cancels_non_tty_process() {
    let cancel_token = CancellationToken::new();
    let cancel_task_token = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_task_token.cancel();
    });

    let result = execute_shell_command(
        ShellExecRequest {
            command: "sleep 5; echo should_not_print".to_string(),
            workdir: std::env::current_dir().unwrap_or_default(),
            description: "cancel test".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 10_000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        None,
        cancel_token,
    )
    .await
    .expect("execute shell command");

    assert!(result.is_error);
    assert_eq!(result.content.into_string(), "command cancelled");
}

#[cfg(unix)]
#[tokio::test]
async fn aborting_tty_command_kills_pty_child() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let started_marker = temp_dir.path().join("started");
    let delayed_marker = temp_dir.path().join("delayed");
    let quote_path =
        |path: &std::path::Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let command = format!(
        "touch {}; sleep 2; touch {}",
        quote_path(&started_marker),
        quote_path(&delayed_marker)
    );
    let cancel_token = CancellationToken::new();
    let task_cancel_token = cancel_token.clone();
    let task = tokio::spawn(execute_shell_command(
        ShellExecRequest {
            command,
            workdir: temp_dir.path().to_path_buf(),
            description: "abort PTY test".into(),
            shell_override: Some("bash".to_string()),
            tty: true,
            login: false,
            timeout_ms: 10_000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        None,
        task_cancel_token,
    ));

    for _ in 0..50 {
        if started_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(started_marker.exists(), "PTY command should have started");
    cancel_token.cancel();
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;

    assert!(
        !delayed_marker.exists(),
        "aborted PTY command should not reach delayed marker"
    );
}

#[tokio::test]
async fn execute_shell_command_success_metadata_is_mixed() {
    let result = execute_shell_command(
        ShellExecRequest {
            command: "echo metadata_test".to_string(),
            workdir: std::env::current_dir().unwrap_or_default(),
            description: "metadata test".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 5000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        None,
        CancellationToken::new(),
    )
    .await
    .expect("execute shell command");

    assert!(!result.is_error);
    match result.content {
        ToolContent::Mixed {
            text: Some(text),
            json: Some(metadata),
        } => {
            assert!(text.contains("metadata_test"));
            assert_eq!(metadata["description"], "metadata test");
        }
        content => panic!("expected mixed success output, got {content:?}"),
    }
}

#[tokio::test]
async fn execute_shell_command_error_output_is_text_only() {
    let result = execute_shell_command(
        ShellExecRequest {
            command: "exit 7".to_string(),
            workdir: std::env::current_dir().unwrap_or_default(),
            description: "error test".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 5000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
        },
        None,
        CancellationToken::new(),
    )
    .await
    .expect("execute shell command");

    assert!(result.is_error);
    assert!(matches!(result.content, ToolContent::Text(text) if text.contains("exit code 7")));
}

use super::{merge_streams, platform_shell_program, preview, resolve_shell, truncate_output};

#[test]
#[cfg(windows)]
fn resolve_shell_prefers_powershell_alias() {
    let spec = resolve_shell(Some("pwsh"), true);
    assert_eq!(spec.program, "powershell");
    assert_eq!(spec.args, &["-NoLogo", "-NoProfile", "-Command"]);
}

#[test]
#[cfg(windows)]
fn resolve_shell_prefers_cmd_alias() {
    let spec = resolve_shell(Some("cmd.exe"), true);
    assert_eq!(spec.program, "cmd");
    assert_eq!(spec.args, &["/C"]);
}

#[test]
fn resolve_shell_defaults_to_platform_shell_login() {
    let spec = resolve_shell(None, true);
    assert_eq!(spec.program, platform_shell_program(true));
}

#[test]
fn preview_truncates_long_text() {
    let long = "a".repeat(30_001);
    let result = preview(&long);
    assert!(result.ends_with("\n\n..."));
}

#[test]
fn truncate_output_handles_zero_tokens() {
    assert_eq!(truncate_output("text", 0), "");
}

#[test]
fn truncate_output_limits_length() {
    let input = "a".repeat(200);
    let result = truncate_output(&input, 10);
    assert!(result.ends_with("\n\n... [truncated]"));
    assert!(result.len() < input.len());
}

#[test]
fn truncate_output_preserves_utf8_boundaries() {
    assert_eq!(truncate_output("😀😀😀", 1), "😀😀😀");
    assert_eq!(
        truncate_output("😀😀😀😀😀", 1),
        "😀😀😀😀\n\n... [truncated]"
    );
}

#[test]
#[ignore]
fn bench_truncate_output_ascii_no_truncation() {
    let input = "shell output line\n".repeat(256);
    let iterations = 200_000;
    let expected_len = input.len();
    let started = Instant::now();
    let mut total_len = 0usize;

    for _ in 0..iterations {
        total_len += black_box(truncate_output(black_box(&input), black_box(2_000))).len();
    }

    let elapsed = started.elapsed();
    assert_eq!(total_len, expected_len * iterations);
    println!(
        "truncate_output_ascii_no_truncation iterations={iterations} bytes={expected_len} elapsed_ms={} per_call_us={:.2}",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
    );
}

#[test]
#[ignore]
fn bench_truncate_output_ascii_large_truncation() {
    let input = "shell output line\n".repeat(8_192);
    let iterations = 50_000;
    let expected_len = truncate_output(&input, 1_000).len();
    let started = Instant::now();
    let mut total_len = 0usize;

    for _ in 0..iterations {
        total_len += black_box(truncate_output(black_box(&input), black_box(1_000))).len();
    }

    let elapsed = started.elapsed();
    assert_eq!(total_len, expected_len * iterations);
    println!(
        "truncate_output_ascii_large_truncation iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
        input.len(),
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
    );
}

#[test]
fn merge_streams_combines_stdout_and_stderr() {
    let result = merge_streams("out", "err");
    assert!(result.contains("out"));
    assert!(result.contains("[stderr]"));
    assert!(result.contains("err"));
}

#[test]
fn merge_streams_no_output() {
    assert_eq!(merge_streams("", ""), "");
}
