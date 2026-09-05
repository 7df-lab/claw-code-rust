//! Process ownership for stdio servers, including abrupt owner exit.

use process_wrap::tokio::{ChildWrapper, CommandWrap, CommandWrapper, JobObject, KillOnDrop};
use std::io;
use tokio::process::Command;

#[derive(Debug)]
/// Starts a child suspended and owns its descendants through a kill-on-close job.
pub struct OwnedJob;

impl CommandWrapper for OwnedJob {
    fn pre_spawn(&mut self, command: &mut Command, core: &CommandWrap) -> io::Result<()> {
        JobObject.pre_spawn(command, core)
    }

    fn wrap_child(
        &mut self,
        child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        // process-wrap 9.1 removes its wrappers while spawning, so JobObject
        // cannot discover KillOnDrop through the supplied core. Provide an
        // explicit context to ensure the job has KILL_ON_JOB_CLOSE enabled.
        let mut context = CommandWrap::from(Command::new(""));
        context.wrap(KillOnDrop);
        JobObject.wrap_child(child, &context)
    }
}
