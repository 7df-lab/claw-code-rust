//! Single-binary dispatch via argv[0].
//!
//! Allows `devo` and `devo-server` to share a single executable. The trick:
//!
//! - On Unix, symlinks (e.g. `devo-server -> devo`) are placed on PATH so the
//!   right sub‑function runs based on `argv[0]`.
//! - On Windows, `.bat` wrappers are generated that re‑invoke the binary with a
//!   sentinel argument so the same dispatch logic can work.
//!
//! Usage from `main()`:
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     devo_arg0::run_as(|_paths| async {
//!         // regular CLI logic here
//!     })
//! }
//! ```

use std::future::Future;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;

/// Alias name for the server sub‑binary.
const SERVER_ALIAS: &str = "devo-server";

/// Directory (under DEVO_HOME/tmp/arg0) where alias entries are created.
const ALIAS_TEMP_ROOT: &str = "arg0";
const LOCK_FILENAME: &str = ".lock";

/// Stack size for Tokio worker threads (16 MB).
const TOKIO_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

// ── Public types ───────────────────────────────────────────────────────────

/// Paths to the current executable and its helper aliases.
///
/// Passed to the main closure so child processes can re‑invoke the binary
/// without relying on [`std::env::current_exe()`] (which can be unreliable
/// under test harnesses).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Arg0DispatchPaths {
    /// Stable path to the current devo executable for child re-execs.
    pub devo_self_exe: Option<PathBuf>,
}

// ── Public API ────────────────────────────────────────────────────────────

/// Entry‑point wrapper that performs argv[0] dispatch first.
///
/// If the current executable was invoked as `devo-server` (via symlink or batch
/// script), `run_as` runs the server entry‑point and exits the process.
/// Otherwise it calls `main_fn`, which should contain the normal CLI logic.
///
/// The function also:
/// - Loads `~/.devo/.env` (if present) before any threading starts, filtering
///   out `DEVO_`-prefixed vars for security.
/// - Injects a temporary directory on `PATH` so that alias names (symlinks /
///   `.bat` scripts) are found by child processes.
/// - Cleans up stale temp directories from previous sessions.
pub fn run_as<F, Fut>(main_fn: F) -> Result<()>
where
    F: FnOnce(Arg0DispatchPaths) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    // ── argv[0] dispatch (must happen before any threading) ──
    if let Some(alias) = argv0_alias() {
        match alias {
            SERVER_ALIAS => {
                // Called as `devo-server` — run the server directly.
                let runtime = build_runtime()?;
                runtime.block_on(run_server_dispatch());
                // Never returns normally; the server stays up until signaled.
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argv[0] alias: {other}");
                std::process::exit(1);
            }
        }
    }

    // ── Load .env from home dir (single-threaded, before any threading) ──
    load_dotenv();

    // ── Inject aliases on PATH (best‑effort) ──
    let _guard = prepend_path_for_aliases().unwrap_or_else(|err| {
        eprintln!("WARNING: could not update PATH for devo aliases: {err}");
        None
    });

    let runtime = build_runtime()?;
    runtime.block_on(async move {
        let current_exe = std::env::current_exe().ok();
        let paths = Arg0DispatchPaths {
            devo_self_exe: current_exe,
        };
        main_fn(paths).await
    })
}

// ── argv[0] detection ─────────────────────────────────────────────────────

/// Returns the alias name if the process was invoked through one of our
/// symlinks or batch scripts, or `None` for a normal `devo` invocation.
fn argv0_alias() -> Option<&'static str> {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let file_name = Path::new(&argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Direct alias hit: `devo-server` symlink or bare name.
    if file_name == SERVER_ALIAS {
        return Some(SERVER_ALIAS);
    }

    // Windows batch scripts pass the intended alias via a sentinel arg.
    let mut args = std::env::args_os();
    args.next(); // skip argv[0]
    if let Some(sentinel) = args.next()
        && let Some(s) = sentinel.to_str()
        && let Some(rest) = s.strip_prefix("--devo-alias=")
        && rest == SERVER_ALIAS
    {
        return Some(SERVER_ALIAS);
    }

    None
}

/// Build a multi‑thread Tokio runtime.
fn build_runtime() -> Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    builder.thread_stack_size(TOKIO_WORKER_STACK_SIZE_BYTES);
    Ok(builder.build()?)
}

/// Run the server dispatch: parse `ServerProcessArgs` and invoke the server.
///
/// Uses `clap::Parser` directly; the `--devo-alias` sentinel has already been
/// consumed by `argv0_alias()` so the remaining args are the server's own.
async fn run_server_dispatch() {
    use clap::Parser;
    let args = devo_server::ServerProcessArgs::parse();
    let home_dir = match devo_utils::find_devo_home() {
        Ok(d) => d,
        Err(err) => {
            eprintln!("error: could not locate DEVO_HOME: {err}");
            std::process::exit(1);
        }
    };
    let loader = devo_core::FileSystemAppConfigLoader::new(home_dir.clone());
    use devo_core::AppConfigLoader;
    let app_config = loader
        .load(args.working_root.as_deref())
        .unwrap_or_else(|err| {
            eprintln!("warning: failed to load app config for logging: {err}");
            devo_core::AppConfig::default()
        });
    let _logging = devo_core::LoggingBootstrap {
        process_name: "server",
        config: app_config.logging,
        home_dir,
    }
    .install();
    if let Err(err) = devo_server::run_server_process(args).await {
        eprintln!("server error: {err}");
        std::process::exit(1);
    }
}

// ── .env loading ───────────────────────────────────────────────────────────

const ILLEGAL_ENV_VAR_PREFIX: &str = "DEVO_";

/// Load env vars from `~/.devo/.env`.
///
/// Security: Do not allow `.env` files to create or modify any variables with
/// names starting with `DEVO_`.
fn load_dotenv() {
    let devo_home = match devo_utils::find_devo_home() {
        Ok(d) => d,
        Err(_) => return,
    };
    let env_path = devo_home.join(".env");
    if !env_path.exists() {
        return;
    }
    let iter = match dotenvy::from_path_iter(&env_path) {
        Ok(iter) => iter,
        Err(_) => return,
    };
    set_filtered(iter);
}

/// Set env vars from a dotenvy iterator, filtering out `DEVO_`-prefixed keys.
fn set_filtered<I>(iter: I)
where
    I: IntoIterator<Item = std::result::Result<(String, String), dotenvy::Error>>,
{
    for (key, value) in iter.into_iter().flatten() {
        if !key.to_ascii_uppercase().starts_with(ILLEGAL_ENV_VAR_PREFIX) {
            // SAFETY: called before any threads are spawned.
            unsafe { std::env::set_var(&key, &value) };
        }
    }
}

// ── PATH injection ────────────────────────────────────────────────────────

/// Creates a temporary directory with alias entries and prepends it to `PATH`.
///
/// Returns a guard whose lifetime keeps the directory alive.
fn prepend_path_for_aliases() -> std::io::Result<Option<PathGuard>> {
    let devo_home = devo_utils::find_devo_home()?;
    let temp_root = devo_home.join("tmp").join(ALIAS_TEMP_ROOT);
    std::fs::create_dir_all(&temp_root)?;

    // Best-effort cleanup of stale per-session dirs.
    if let Err(err) = janitor_cleanup(&temp_root) {
        eprintln!("WARNING: failed to clean up stale arg0 temp dirs: {err}");
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("devo-arg0")
        .tempdir_in(&temp_root)?;
    let path = temp_dir.path();

    // Create a lock file so janitor can detect this session is still live.
    let lock_path = path.join(LOCK_FILENAME);
    let lock_file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    // Acquire an exclusive advisory lock so the janitor (which tries a
    // non-blocking lock) knows this session is alive.
    use fs2::FileExt;
    lock_file.try_lock_exclusive()?;

    let exe = std::env::current_exe()?;

    // ── Create alias entries ──
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = path.join(SERVER_ALIAS);
        symlink(&exe, &link)?;
    }
    #[cfg(windows)]
    {
        // Each alias gets a .bat script that passes the alias name via a
        // sentinel argument.
        let batch = path.join(format!("{SERVER_ALIAS}.bat"));
        std::fs::write(
            &batch,
            format!(
                "@echo off\r\n\"{}\" --devo-alias={SERVER_ALIAS} %*\r\n",
                exe.display()
            ),
        )?;
    }

    // ── Prepend to PATH ──
    let path_separator = if cfg!(windows) { ";" } else { ":" };
    let updated_path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut new =
                std::ffi::OsString::with_capacity(path.as_os_str().len() + 1 + existing.len());
            new.push(path);
            new.push(path_separator);
            new.push(existing);
            new
        }
        None => path.as_os_str().to_owned(),
    };
    // SAFETY: called before any threads are spawned.
    unsafe {
        std::env::set_var("PATH", updated_path);
    }

    Ok(Some(PathGuard {
        _temp_dir: temp_dir,
        _lock_file: lock_file,
    }))
}

// ── Janitor (stale directory cleanup) ──────────────────────────────────────

/// Remove stale (unlocked) session directories under `temp_root`.
fn janitor_cleanup(temp_root: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(temp_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Skip the directory if locking fails or the lock is currently held.
        let Some(_lock_file) = try_lock_dir(&path)? else {
            continue;
        };

        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            // Expected TOCTOU race: directory can disappear after read_dir/lock checks.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

/// Attempt to acquire an exclusive lock on a directory's lock file.
///
/// Returns `Ok(Some(File))` if the lock was acquired, `Ok(None)` if the lock
/// file doesn't exist or is held by another process.
fn try_lock_dir(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    use fs2::FileExt;

    let lock_path = dir.join(LOCK_FILENAME);
    let lock_file = match std::fs::File::options().read(true).write(true).open(&lock_path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock_file)),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(err),
    }
}

// ── PathGuard ──────────────────────────────────────────────────────────────

/// Keeps the temporary alias directory alive for the process lifetime.
struct PathGuard {
    _temp_dir: tempfile::TempDir,
    _lock_file: std::fs::File,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fs2::FileExt;
    use pretty_assertions::assert_eq;

    use super::janitor_cleanup;
    use super::LOCK_FILENAME;

    fn create_lock(dir: &Path) -> std::io::Result<std::fs::File> {
        let lock_path = dir.join(LOCK_FILENAME);
        std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
    }

    #[test]
    fn argv0_alias_returns_none_for_normal_invocation() {
        assert_eq!(super::argv0_alias(), None);
    }

    #[test]
    fn janitor_skips_dirs_without_lock_file() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("no-lock");
        fs::create_dir(&dir)?;

        janitor_cleanup(root.path())?;

        assert!(dir.exists());
        Ok(())
    }

    #[test]
    #[cfg_attr(windows, ignore = "fs2 LockFileEx allows same-process locking differently on Windows")]
    fn janitor_skips_dirs_with_held_lock() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("locked");
        fs::create_dir(&dir)?;
        // On Windows, opening the file with different share modes can cause
        // LockFileEx to fail.  Open the lock file the same way
        // prepend_path_for_aliases does (shared read/write sharing).
        let lock_path = dir.join(LOCK_FILENAME);
        let lock_file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock_file.try_lock_exclusive()?;

        janitor_cleanup(root.path())?;

        assert!(dir.exists());
        Ok(())
    }

    #[test]
    fn janitor_removes_dirs_with_unlocked_lock() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("stale");
        fs::create_dir(&dir)?;
        create_lock(&dir)?;

        janitor_cleanup(root.path())?;

        assert!(!dir.exists());
        Ok(())
    }
}
