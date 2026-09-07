use super::*;
use std::time::Duration;

#[tokio::test]
async fn dropping_pipe_call_stops_descendant_work() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let command = "$p = Start-Process powershell.exe -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content delayed yes'; Set-Content started yes; Start-Sleep 30";
    #[cfg(unix)]
    let command = "(sleep 2; touch delayed) & touch started; wait";
    let mut call = Box::pin(execute_shell_command(
        ShellExecRequest {
            output_capture: None,
            command: command.into(),
            workdir: directory.path().to_path_buf(),
            description: "cancellation regression".into(),
            shell_override: None,
            tty: false,
            login: false,
            timeout_ms: 30000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            sandbox_profile: None,
            sandbox_permission_overlay: None,
        },
        /*progress*/ None,
        tokio_util::sync::CancellationToken::new(),
    ));
    tokio::select! {
        biased;
        _ = &mut call => panic!("command completed before interruption"),
        () = async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !directory.path().join("started").exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.unwrap();
        } => {}
    }
    drop(call);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !directory.path().join("delayed").exists(),
        "descendant survived interrupted pipe call"
    );
}
