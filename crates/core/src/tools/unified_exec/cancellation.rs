//! Cancellation ownership for foreground commands and active stdin calls.

use super::process::UnifiedExecProcess;
use super::store::ProcessStore;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) struct CancelProcessOnDrop {
    process: Arc<UnifiedExecProcess>,
    store: Arc<ProcessStore>,
    id: i32,
    armed: bool,
}

impl CancelProcessOnDrop {
    pub(crate) fn new(process: Arc<UnifiedExecProcess>, store: Arc<ProcessStore>, id: i32) -> Self {
        Self {
            process,
            store,
            id,
            armed: true,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelProcessOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.process.terminate();
            let store = Arc::clone(&self.store);
            let id = self.id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    store.remove(id).await;
                });
            }
        }
    }
}

pub(crate) async fn watch_turn(
    token: CancellationToken,
    process: Arc<UnifiedExecProcess>,
    store: Arc<ProcessStore>,
    id: i32,
) {
    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                process.terminate();
                store.remove(id).await;
                return;
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if !process.is_running() || process.exit_code().is_some() { return; }
            }
        }
    }
}
