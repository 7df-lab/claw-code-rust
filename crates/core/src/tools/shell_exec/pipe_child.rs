//! Kill the complete pipe process tree before dropping its direct child.

use process_wrap::tokio::ChildWrapper;

pub(super) struct PipeChild(pub(super) Box<dyn ChildWrapper>);

impl PipeChild {
    pub(super) fn terminate_tree(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.id() {
            let _ = devo_util_process::process_group::kill_process_group(pid);
        }
        // On Windows the wrapper terminates the job, including descendants.
        let _ = self.0.start_kill();
    }
}

impl Drop for PipeChild {
    fn drop(&mut self) {
        if self.0.id().is_some() {
            self.terminate_tree();
        }
    }
}
