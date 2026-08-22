use super::WindowsSandboxCancellationToken;
use super::logging::log_failure;
use super::logging::log_success;
use super::process::ConsoleMode;
use super::process::create_process_as_user;
use super::sandbox_utils::ensure_devo_home_exists;
use super::spawn_prep::LegacyAclSids;
use super::spawn_prep::SpawnPrepOptions;
use super::spawn_prep::allow_null_device_for_workspace_write;
use super::spawn_prep::apply_legacy_session_acl_rules;
use super::spawn_prep::legacy_session_capability_roots;
use super::spawn_prep::prepare_legacy_session_security;
use super::spawn_prep::prepare_legacy_spawn_context;
use super::spawn_prep::root_capability_sids;
use crate::protocol::models::PermissionProfile;
use anyhow::Result;
use devo_util_paths::absolute_path::AbsolutePathBuf;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::ptr;
use std::time::Duration;
use std::time::Instant;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
use windows_sys::Win32::Foundation::SetHandleInformation;
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::Threading::TerminateProcess;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x0000_0102;

fn create_kill_on_close_job() -> Option<HANDLE> {
    unsafe {
        let job = CreateJobObjectW(ptr::null_mut(), ptr::null());
        if job == 0 {
            return None;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            ptr::from_mut(&mut limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

type PipeHandles = ((HANDLE, HANDLE), (HANDLE, HANDLE), (HANDLE, HANDLE));

enum WaitOutcome {
    Exited,
    TimedOut,
    Cancelled,
}

fn wait_for_process(
    process: HANDLE,
    timeout_ms: Option<u64>,
    cancellation: Option<&WindowsSandboxCancellationToken>,
) -> WaitOutcome {
    let Some(cancellation) = cancellation else {
        let timeout = timeout_ms.map(|ms| ms as u32).unwrap_or(INFINITE);
        let res = unsafe { WaitForSingleObject(process, timeout) };
        return if res == WAIT_TIMEOUT {
            WaitOutcome::TimedOut
        } else {
            WaitOutcome::Exited
        };
    };

    let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    loop {
        if cancellation.is_cancelled() {
            return WaitOutcome::Cancelled;
        }
        let wait_ms = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return WaitOutcome::TimedOut;
                }
                remaining.min(Duration::from_millis(50)).as_millis() as u32
            }
            None => 50,
        };
        let res = unsafe { WaitForSingleObject(process, wait_ms) };
        if res == WAIT_OBJECT_0 {
            return WaitOutcome::Exited;
        }
        // WAIT_TIMEOUT: poll cancellation again. WAIT_FAILED / other
        // codes must not be treated as exit — that skips TerminateProcess
        // and leaves the stdout reader blocked on a live pipe.
    }
}

unsafe fn setup_stdio_pipes() -> io::Result<PipeHandles> {
    let mut in_r: HANDLE = 0;
    let mut in_w: HANDLE = 0;
    let mut out_r: HANDLE = 0;
    let mut out_w: HANDLE = 0;
    let mut err_r: HANDLE = 0;
    let mut err_w: HANDLE = 0;
    if CreatePipe(&mut in_r, &mut in_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if CreatePipe(&mut out_r, &mut out_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if CreatePipe(&mut err_r, &mut err_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(in_r, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(out_w, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(err_w, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    Ok(((in_r, in_w), (out_r, out_w), (err_r, err_w)))
}

pub struct CaptureResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    devo_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    cancellation: Option<WindowsSandboxCancellationToken>,
    use_private_desktop: bool,
) -> Result<CaptureResult> {
    run_windows_sandbox_capture_with_filesystem_overrides(
        permission_profile,
        workspace_roots,
        devo_home,
        command,
        cwd,
        env_map,
        timeout_ms,
        cancellation,
        &[],
        &[],
        use_private_desktop,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture_with_filesystem_overrides(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    devo_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    mut env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    cancellation: Option<WindowsSandboxCancellationToken>,
    additional_deny_read_paths: &[AbsolutePathBuf],
    additional_deny_write_paths: &[AbsolutePathBuf],
    use_private_desktop: bool,
) -> Result<CaptureResult> {
    let additional_deny_read_paths = additional_deny_read_paths
        .iter()
        .map(AbsolutePathBuf::to_path_buf)
        .collect::<Vec<_>>();
    let additional_deny_write_paths = additional_deny_write_paths
        .iter()
        .map(AbsolutePathBuf::to_path_buf)
        .collect::<Vec<_>>();
    let common = prepare_legacy_spawn_context(
        permission_profile,
        workspace_roots,
        devo_home,
        cwd,
        &mut env_map,
        &command,
        SpawnPrepOptions {
            inherit_path: false,
            add_git_safe_directory: false,
        },
    )?;
    let permissions = common.permissions;
    let current_dir = common.current_dir;
    let logs_base_dir = common.logs_base_dir.as_deref();
    let uses_write_capabilities = common.uses_write_capabilities;
    if !permissions.has_full_disk_read_access() {
        anyhow::bail!("Restricted read-only access requires the elevated Windows sandbox backend");
    }
    // WRITE_RESTRICTED tokens consult restricting SIDs only for writes, so this
    // backend cannot make capability-SID deny-read ACLs authoritative.
    if !additional_deny_read_paths.is_empty() {
        anyhow::bail!("deny-read overrides require the elevated Windows sandbox backend");
    }
    let capability_roots =
        legacy_session_capability_roots(&permissions, &current_dir, &env_map, devo_home);
    let security =
        prepare_legacy_session_security(uses_write_capabilities, devo_home, cwd, capability_roots)?;
    allow_null_device_for_workspace_write(uses_write_capabilities);
    apply_legacy_session_acl_rules(
        &permissions,
        devo_home,
        &current_dir,
        &env_map,
        &additional_deny_read_paths,
        &additional_deny_write_paths,
        LegacyAclSids {
            readonly_sid: security.readonly_sid.as_ref(),
            readonly_sid_str: security.readonly_sid_str.as_deref(),
            write_root_sids: &security.write_root_sids,
        },
    )?;
    if cancellation
        .as_ref()
        .is_some_and(WindowsSandboxCancellationToken::is_cancelled)
    {
        unsafe {
            CloseHandle(security.h_token);
        }
        log_failure(&command, "cancelled before spawn", logs_base_dir);
        return Ok(CaptureResult {
            exit_code: 1,
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }
    let (stdin_pair, stdout_pair, stderr_pair) = unsafe { setup_stdio_pipes()? };
    let ((in_r, in_w), (out_r, out_w), (err_r, err_w)) = (stdin_pair, stdout_pair, stderr_pair);
    let spawn_res = unsafe {
        create_process_as_user(
            security.h_token,
            &command,
            cwd,
            &env_map,
            logs_base_dir,
            Some((in_r, out_w, err_w)),
            ConsoleMode::NoWindow,
            use_private_desktop,
        )
    };
    let created = match spawn_res {
        Ok(v) => v,
        Err(err) => {
            unsafe {
                CloseHandle(in_r);
                CloseHandle(in_w);
                CloseHandle(out_r);
                CloseHandle(out_w);
                CloseHandle(err_r);
                CloseHandle(err_w);
                CloseHandle(security.h_token);
            }
            return Err(err);
        }
    };
    let pi = created.process_info;
    let _desktop = created;
    let mut job = create_kill_on_close_job();
    if let Some(job) = job {
        unsafe {
            let _ = AssignProcessToJobObject(job, pi.hProcess);
        }
    }

    unsafe {
        CloseHandle(in_r);
        // Close the parent's stdin write end so the child sees EOF immediately.
        CloseHandle(in_w);
        CloseHandle(out_w);
        CloseHandle(err_w);
    }

    let (tx_out, rx_out) = std::sync::mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = std::sync::mpsc::channel::<Vec<u8>>();
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    out_r,
                    tmp.as_mut_ptr(),
                    tmp.len() as u32,
                    &mut read_bytes,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read_bytes == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read_bytes as usize]);
        }
        let _ = tx_out.send(buf);
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    err_r,
                    tmp.as_mut_ptr(),
                    tmp.len() as u32,
                    &mut read_bytes,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read_bytes == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read_bytes as usize]);
        }
        let _ = tx_err.send(buf);
    });

    let wait_outcome = wait_for_process(pi.hProcess, timeout_ms, cancellation.as_ref());
    let timed_out = matches!(wait_outcome, WaitOutcome::TimedOut);
    let cancelled = matches!(wait_outcome, WaitOutcome::Cancelled);
    let mut exit_code_u32: u32 = 1;
    if !timed_out && !cancelled {
        unsafe {
            GetExitCodeProcess(pi.hProcess, &mut exit_code_u32);
        }
    } else {
        unsafe {
            let _ = TerminateProcess(pi.hProcess, 1);
        }
        if let Some(job) = job.take() {
            // Kill the process tree before joining pipe readers. Closing the
            // job is what actually reaps children that still hold stdout.
            unsafe { CloseHandle(job) };
        }
        unsafe {
            let _ = WaitForSingleObject(pi.hProcess, 2_000);
        }
        // CancelIoEx is a no-op if ReadFile is not pending yet, so retry
        // until the reader threads finish or a short deadline elapses.
        let join_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < join_deadline && (!t_out.is_finished() || !t_err.is_finished()) {
            unsafe {
                let _ = CancelIoEx(out_r, ptr::null_mut());
                let _ = CancelIoEx(err_r, ptr::null_mut());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    unsafe {
        if pi.hThread != 0 {
            CloseHandle(pi.hThread);
        }
        if pi.hProcess != 0 {
            CloseHandle(pi.hProcess);
        }
        CloseHandle(security.h_token);
        if let Some(job) = job {
            CloseHandle(job);
        }
    }
    let _ = t_out.join();
    let _ = t_err.join();
    unsafe {
        CloseHandle(out_r);
        CloseHandle(err_r);
    }
    let stdout = rx_out.recv().unwrap_or_default();
    let stderr = rx_err.recv().unwrap_or_default();
    let exit_code = if timed_out {
        128 + 64
    } else {
        exit_code_u32 as i32
    };

    if exit_code == 0 {
        log_success(&command, logs_base_dir);
    } else {
        log_failure(&command, &format!("exit code {exit_code}"), logs_base_dir);
    }

    Ok(CaptureResult {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

pub fn run_windows_sandbox_legacy_preflight(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    devo_home: &Path,
    cwd: &Path,
    env_map: &HashMap<String, String>,
) -> Result<()> {
    let Ok(permissions) = super::resolved_permissions::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            workspace_roots,
        ) else {
            return Ok(());
        };
    if !permissions.uses_write_capabilities_for_cwd(cwd, env_map) {
        return Ok(());
    }

    ensure_devo_home_exists(devo_home)?;
    let current_dir = cwd.to_path_buf();
    let capability_roots =
        legacy_session_capability_roots(&permissions, &current_dir, env_map, devo_home);
    let write_root_sids = root_capability_sids(devo_home, cwd, capability_roots)?;
    apply_legacy_session_acl_rules(
        &permissions,
        devo_home,
        &current_dir,
        env_map,
        &[],
        &[],
        LegacyAclSids {
            readonly_sid: None,
            readonly_sid_str: None,
            write_root_sids: &write_root_sids,
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::protocol::models::PermissionProfile;
    use crate::protocol::permissions::NetworkSandboxPolicy;
    use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
    use std::collections::HashMap;
    use std::path::Path;

    fn workspace_profile(network_policy: NetworkSandboxPolicy) -> PermissionProfile {
        PermissionProfile::workspace_write_with(
            &[],
            network_policy,
            /*exclude_tmpdir_env_var*/ false,
            /*exclude_slash_tmp*/ false,
        )
    }

    fn should_apply_network_block(permission_profile: &PermissionProfile) -> bool {
        ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            &[],
        )
        .expect("managed permissions")
        .should_apply_network_block()
    }

    #[test]
    fn applies_network_block_when_access_is_disabled() {
        assert!(should_apply_network_block(&workspace_profile(
            NetworkSandboxPolicy::Restricted
        )));
    }

    #[test]
    fn skips_network_block_when_access_is_allowed() {
        assert!(!should_apply_network_block(&workspace_profile(
            NetworkSandboxPolicy::Enabled
        )));
    }

    #[test]
    fn applies_network_block_for_read_only() {
        assert!(should_apply_network_block(&PermissionProfile::read_only()));
    }

    #[test]
    fn legacy_preflight_skips_profiles_without_managed_filesystem_permissions() {
        for permission_profile in [
            PermissionProfile::Disabled,
            PermissionProfile::External {
                network: NetworkSandboxPolicy::Restricted,
            },
        ] {
            super::run_windows_sandbox_legacy_preflight(
                &permission_profile,
                &[],
                Path::new("."),
                Path::new("."),
                &HashMap::new(),
            )
            .expect("unsupported profiles do not need ACL preflight");
        }
    }
}
