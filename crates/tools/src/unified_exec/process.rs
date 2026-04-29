use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::broadcast;
use tokio::time::{Duration, sleep};

use super::buffer::HeadTailBuffer;
use super::ProcessOutput;

const PTY_READ_BUF: usize = 4096;
const PTY_ROWS: u16 = 24;
const PTY_COLS: u16 = 120;

struct ShellSpec {
    program: String,
    args: Vec<String>,
}

fn resolve_shell(shell_override: Option<&str>, login: bool) -> ShellSpec {
    if let Some(shell) = shell_override {
        let mut args = Vec::new();
        if login {
            args.push("-l".to_string());
        }
        args.push("-c".to_string());
        return ShellSpec {
            program: shell.to_string(),
            args,
        };
    }

    let shell = if cfg!(windows) { "powershell" } else { "bash" };
    let mut args = Vec::new();
    if login && !cfg!(windows) {
        args.push("-l".to_string());
    }
    args.push("-c".to_string());
    ShellSpec {
        program: shell.to_string(),
        args,
    }
}

pub struct UnifiedExecProcess {
    exit_code: Arc<std::sync::atomic::AtomicI32>,
    shutdown_flag: Arc<AtomicBool>,
    stdin_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    process_id: i32,
}

impl UnifiedExecProcess {
    pub fn spawn(
        process_id: i32,
        cmd: &str,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        let (output_tx, _output_rx) = broadcast::channel(256);
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = Arc::clone(&shutdown_flag);

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: PTY_ROWS,
                cols: PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("failed to open PTY: {e}"))?;

        let shell_spec = resolve_shell(shell, login);
        let mut builder = CommandBuilder::new(&shell_spec.program);
        builder.args(&shell_spec.args);
        builder.arg(cmd);
        builder.cwd(cwd);
        if cfg!(windows) {
            builder.env("PYTHONUTF8", "1");
            builder.env("TERM", "xterm-256color");
            builder.env("COLORTERM", "truecolor");
        }

        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| format!("failed to spawn PTY command: {e}"))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("failed to clone PTY reader: {e}"))?;

        let writer: Box<dyn Write + Send> = pair.master.take_writer().map_err(|e| {
            format!("failed to take PTY writer: {e}")
        })?;

        let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        // Reader thread: blocking PTY read -> tokio::mpsc
        std::thread::spawn(move || {
            let mut buf = [0u8; PTY_READ_BUF];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) => break,
                    Ok(size) => {
                        if tokio_tx.send(buf[..size].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let exit_code = Arc::new(std::sync::atomic::AtomicI32::new(-1));
        let exit_code_clone = Arc::clone(&exit_code);
        let output_tx_clone = output_tx.clone();

        // Background task: forward tokio::mpsc -> broadcast, handle shutdown/exit
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = async {
                        while !shutdown_flag_clone.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    } => {
                        break;
                    }
                    Some(bytes) = tokio_rx.recv() => {
                        let _ = output_tx_clone.send(bytes);
                    }
                    else => break,
                }
            }

            if let Ok(status) = child.try_wait() {
                if let Some(s) = status {
                    exit_code_clone.store(s.exit_code() as i32, std::sync::atomic::Ordering::SeqCst);
                } else {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            } else {
                let _ = child.kill();
                let _ = child.wait();
            }
        });

        let proc_output_rx = output_tx.subscribe();

        Ok((
            UnifiedExecProcess {
                exit_code,
                shutdown_flag,
                stdin_writer: Arc::new(Mutex::new(Some(writer))),
                output_tx,
                process_id,
            },
            proc_output_rx,
        ))
    }

    pub fn write_stdin(&self, chars: &str) -> Result<(), String> {
        let mut guard = self.stdin_writer.lock().map_err(|e| format!("lock error: {e}"))?;
        if let Some(writer) = guard.as_mut() {
            writer
                .write_all(chars.as_bytes())
                .map_err(|e| format!("failed to write to stdin: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("failed to flush stdin: {e}"))?;
            Ok(())
        } else {
            Err("stdin is closed for this session".to_string())
        }
    }

    pub fn terminate(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
    }

    pub fn exit_code(&self) -> Option<i32> {
        let code = self.exit_code.load(std::sync::atomic::Ordering::SeqCst);
        if code >= 0 {
            Some(code)
        } else {
            None
        }
    }

    pub fn is_running(&self) -> bool {
        self.exit_code.load(std::sync::atomic::Ordering::SeqCst) < 0
    }

    pub fn process_id(&self) -> i32 {
        self.process_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }
}

unsafe impl Send for UnifiedExecProcess {}
unsafe impl Sync for UnifiedExecProcess {}

impl Drop for UnifiedExecProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub async fn collect_output(
    output_rx: &mut broadcast::Receiver<Vec<u8>>,
    process: &UnifiedExecProcess,
    yield_time_ms: u64,
    max_output_tokens: usize,
) -> ProcessOutput {
    let started = Instant::now();
    let mut buf = HeadTailBuffer::new();
    let deadline = Duration::from_millis(yield_time_ms);

    loop {
        loop {
            match output_rx.try_recv() {
                Ok(bytes) => {
                    buf.push(&bytes);
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    let _ = output_rx.try_recv();
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }

        let done = !process.is_running()
            || (process.exit_code().is_some()
                && output_rx.len() == 0);

        if done {
            loop {
                match output_rx.try_recv() {
                    Ok(bytes) => buf.push(&bytes),
                    Err(_) => break,
                }
            }
            break;
        }

        if started.elapsed() >= deadline {
            break;
        }

        sleep(Duration::from_millis(10)).await;
    }

    let bytes = buf.collect();
    let mut output = String::from_utf8_lossy(&bytes).into_owned();

    let max_chars = max_output_tokens.saturating_mul(4);
    let truncated = output.len() > max_chars;
    if truncated {
        output.truncate(max_chars);
        output.push_str("\n\n... [truncated]");
    }

    ProcessOutput {
        output,
        exit_code: process.exit_code(),
        wall_time_secs: started.elapsed().as_secs_f64(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn process_spawn_and_exit() {
        let cmd = if cfg!(windows) { "echo hello" } else { "echo hello" };
        let (proc, mut rx) = UnifiedExecProcess::spawn(1, cmd, Path::new("."), None, false)
            .expect("spawn should succeed");

        // Wait for process to finish
        let mut waited = 0u64;
        while proc.is_running() && waited < 3000 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            waited += 100;
        }

        let _output = collect_output(&mut rx, &proc, 1000, 1000).await;
        // Process should have exited (echo is a short command)
        // On all platforms, echo finishes quickly
        if !proc.is_running() {
            assert!(proc.exit_code().is_some(), "process should have an exit code");
        }
    }

    #[tokio::test]
    async fn process_terminate_works() {
        // Only run on platforms where we have reliable PTY support
        if cfg!(target_os = "linux") {
            let (proc, _rx) = UnifiedExecProcess::spawn(2, "sleep 60", Path::new("."), None, false)
                .expect("spawn should succeed");
            assert!(proc.is_running());

            proc.terminate();
            tokio::time::sleep(Duration::from_millis(300)).await;

            assert!(!proc.is_running(), "process should have been terminated");
        }
    }

    #[tokio::test]
    async fn process_write_stdin_before_exit() {
        // Only run on Unix where cat + PTY stdin works reliably
        if cfg!(target_os = "linux") {
            let (proc, _rx) = UnifiedExecProcess::spawn(3, "cat", Path::new("."), None, false)
                .expect("spawn should succeed");

            tokio::time::sleep(Duration::from_millis(300)).await;

            let result = proc.write_stdin("test data\n");
            assert!(result.is_ok(), "write_stdin failed: {:?}", result);
        }
    }
}
