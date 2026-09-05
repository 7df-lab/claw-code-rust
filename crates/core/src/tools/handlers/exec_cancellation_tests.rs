#[tokio::test]
async fn yielded_foreground_command_stops_on_turn_interrupt() {
    let handler = test_exec_handler();
    let ctx = test_ctx(std::env::current_dir().unwrap());
    let token = ctx.cancel_token.clone();
    #[cfg(windows)]
    let command = "Start-Sleep 30";
    #[cfg(unix)]
    let command = "sleep 30";
    let result = handler.handle(ctx, serde_json::json!({
        "cmd": command, "yield_time_ms": 250, "login": false,
    }), /*progress*/ None).await.unwrap();
    assert!(!handler.store.is_empty().await, "command must yield while running: {result:?}");
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !handler.store.is_empty().await {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("interruption removes the yielded command");
}

#[tokio::test]
async fn dropped_stdin_call_stops_existing_process() {
    let store = Arc::new(ProcessStore::new());
    #[cfg(windows)]
    let command = "Start-Sleep 30";
    #[cfg(unix)]
    let command = "sleep 30";
    let cwd = std::env::current_dir().unwrap();
    let (process, _) = UnifiedExecProcess::spawn(
        /*process_id*/ 1234, command, &cwd, /*shell*/ None,
        /*login*/ false, /*tty*/ false,
    ).await.unwrap();
    let process = Arc::new(process);
    store.insert_reserved(1234, Arc::clone(&process)).await;
    let handler = WriteStdinHandler::new(Arc::clone(&store));
    let mut call = Box::pin(handler.handle(test_ctx(cwd), serde_json::json!({
        "process_id": 1234, "chars": "", "yield_time_ms": 30000,
    }), /*progress*/ None));
    tokio::select! {
        biased;
        _ = &mut call => panic!("stdin poll should remain pending"),
        () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
    }
    drop(call);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while process.is_running() || !store.is_empty().await {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("dropping stdin poll terminates the process");
}
