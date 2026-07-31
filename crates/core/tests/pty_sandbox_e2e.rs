//! PTY sandbox e2e: `UnifiedExecProcess::spawn_with_sandbox` with `tty: true`
//! must enforce the sandbox profile through the OS launcher wrapper (macOS
//! `sandbox-exec`, Linux `bwrap`) — PTY spawns have no `pre_exec` hook.
//!
//! The test skips when no sandbox launcher is available on this machine (the
//! wrap API then declines to wrap, which is a supported degradation).

#![cfg(unix)]

use std::path::{Path, PathBuf};

use devo_core::tools::unified_exec::process::{UnifiedExecProcess, collect_output};

const MARKER: &str = "pty-e2e-marker-9d4c2b";
const PROFILE: &str = "ptye2edeny";

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_workspace(tag: &str) -> (PathBuf, TempDirGuard) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let workspace =
        std::env::temp_dir().join(format!("devo-pty-e2e-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(workspace.join(".devo")).expect("create .devo config directory");
    std::fs::write(
        workspace.join(".devo").join("sandbox.toml"),
        format!("[profiles.{PROFILE}]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n"),
    )
    .expect("write sandbox.toml");
    std::fs::write(workspace.join("secret.txt"), format!("SECRET={MARKER}"))
        .expect("write denied file");
    std::fs::write(workspace.join("control.txt"), "hello workspace").expect("write control file");
    let workspace = std::fs::canonicalize(&workspace).expect("canonicalize temp workspace");
    let guard = TempDirGuard(workspace.clone());
    (workspace, guard)
}

async fn run_sandboxed_command(
    command: &str,
    workspace: &Path,
    process_id: i32,
    tty: bool,
) -> String {
    let (process, mut rx) = UnifiedExecProcess::spawn_with_sandbox(
        process_id,
        command,
        workspace,
        /*shell*/ Some("bash"),
        /*login*/ false,
        tty,
        Some(PROFILE.to_string()),
    )
    .await
    .expect("PTY spawn should succeed");
    collect_output(&mut rx, &process, 15_000, 4_000)
        .await
        .output
}

#[tokio::test]
async fn pty_spawn_enforces_deny_read_and_write() {
    let (workspace, _guard) = temp_workspace("deny");

    // Skip when the machine has no sandbox launcher (the supported
    // warn-and-run-unwrapped degradation).
    match devo_sandbox::wrap_command_for_profile(
        Some(PROFILE),
        &workspace,
        devo_sandbox::WrapMode::PtyOnly,
        &devo_sandbox::SandboxLogger::new(),
    ) {
        Ok(devo_sandbox::SandboxWrap::Wrapped(_)) => {}
        other => {
            eprintln!("skipping: no sandbox launcher available ({other:?})");
            return;
        }
    }

    // A denied file must not be readable from inside the PTY.
    let output = run_sandboxed_command("cat secret.txt", &workspace, 1, /*tty*/ true).await;
    assert!(
        !output.contains(MARKER),
        "PTY child read a denied path:\n{output}"
    );

    // A denied file must not be writable from inside the PTY.
    let _ = run_sandboxed_command(
        "echo hijacked >> secret.txt",
        &workspace,
        2,
        /*tty*/ true,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("secret.txt")).expect("read denied file"),
        format!("SECRET={MARKER}"),
        "denied file content must be unchanged"
    );

    // A non-denied file stays readable (the sandbox did not break the PTY).
    let output = run_sandboxed_command("cat control.txt", &workspace, 3, /*tty*/ true).await;
    assert!(
        output.contains("hello workspace"),
        "PTY child must still read the control file:\n{output}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_spawn_enforces_profile_without_child_side_seatbelt() {
    let (workspace, _guard) = temp_workspace("pipe");

    match devo_sandbox::wrap_command_for_profile(
        Some(PROFILE),
        &workspace,
        devo_sandbox::WrapMode::PipeComposed,
        &devo_sandbox::SandboxLogger::new(),
    ) {
        Ok(devo_sandbox::SandboxWrap::Wrapped(_)) => {}
        other => {
            eprintln!("skipping: no pipe sandbox launcher available ({other:?})");
            return;
        }
    }

    for process_id in 10..20 {
        let output = run_sandboxed_command(
            "cat control.txt",
            &workspace,
            process_id,
            /*tty*/ false,
        )
        .await;
        assert!(
            output.contains("hello workspace"),
            "pipe child must read the control file:\n{output}"
        );
    }

    let output = run_sandboxed_command("cat secret.txt", &workspace, 20, /*tty*/ false).await;
    assert!(
        !output.contains(MARKER),
        "pipe child read a denied path:\n{output}"
    );
}
