# Tool interruption

Interrupting a turn drops its active tool calls. Foreground `exec_command`
processes retain their turn cancellation watcher after yielding a process ID;
interrupting that turn terminates them and removes them from the process store.
Explicit background tasks retain their independent lifetime.

Dropping an active `write_stdin` call also terminates its process, including
processes started in an earlier turn. Returning normally from a stdin poll
keeps a still-running process available for subsequent calls.

Piped `shell_command` calls own process-tree cleanup through a drop guard, so
router cancellation cannot bypass it. Unix kills the dedicated process group;
Windows assigns the suspended shell to a job and terminates that job on drop.
Interruption does not roll back file writes or other effects already performed.
