#![cfg(windows)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use devo_rmcp_client::{LocalStdioServerLauncher, RmcpClient};
use pretty_assertions::assert_eq;

// Run in a separate process so exit skips all Rust and Tokio cleanup.
#[tokio::test]
#[ignore]
async fn stdio_owner() {
    let Ok(directory) = std::env::var("DEVO_MCP_CLEANUP_TEST_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let _client = RmcpClient::new_stdio_client(
        "powershell.exe".into(),
        vec!["-NoProfile".into(), "-Command".into(),
            "$child = Start-Process powershell.exe -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile','-Command','Start-Sleep 120'; [IO.File]::WriteAllText('child.pid', $child.Id.ToString()); [IO.File]::WriteAllText('server.pid', $PID.ToString()); Start-Sleep 120".into()],
        /*env*/ None, &[], Some(directory.clone()),
        std::sync::Arc::new(LocalStdioServerLauncher::new(directory.clone())),
    ).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !directory.join("server.pid").exists() {
        assert!(Instant::now() < deadline, "server did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    std::process::exit(0);
}

#[test]
fn owner_exit_kills_stdio_server_and_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "stdio_owner"])
        .env("DEVO_MCP_CLEANUP_TEST_DIR", directory.path())
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let pids = ["server.pid", "child.pid"].map(|name| {
        std::fs::read_to_string(directory.path().join(name))
            .unwrap()
            .parse::<u32>()
            .unwrap()
    });
    let script = format!(
        "Start-Sleep -Milliseconds 500; @(Get-Process -Id {},{} -ErrorAction SilentlyContinue).Count",
        pids[0], pids[1]
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
}
