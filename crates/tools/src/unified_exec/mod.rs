pub mod buffer;
pub mod process;
pub mod store;

pub const MAX_PROCESSES: usize = 64;
pub const WARNING_PROCESSES: usize = 60;
pub const DEFAULT_YIELD_MS: u64 = 10_000;
pub const DEFAULT_POLL_YIELD_MS: u64 = 250;
pub const MAX_OUTPUT_TOKENS: usize = 16_000;

pub struct ExecCommandArgs {
    pub cmd: String,
    pub workdir: Option<String>,
    pub shell: Option<String>,
    pub login: bool,
    pub tty: bool,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
}

pub struct WriteStdinArgs {
    pub session_id: i32,
    pub chars: String,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
}

pub struct ProcessOutput {
    pub output: String,
    pub exit_code: Option<i32>,
    pub wall_time_secs: f64,
    pub truncated: bool,
}
