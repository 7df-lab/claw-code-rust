//! Windows OS sandbox for Devo.
#![allow(unsafe_op_in_unsafe_fn)]

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct WindowsSandboxCancellationToken {
    is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl WindowsSandboxCancellationToken {
    pub fn new(is_cancelled: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            is_cancelled: Arc::new(is_cancelled),
        }
    }
    pub fn is_cancelled(&self) -> bool {
        (self.is_cancelled)()
    }
}

impl fmt::Debug for WindowsSandboxCancellationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsSandboxCancellationToken")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowsSandboxProxySettingsMode {
    #[default]
    Reconcile,
    Preserve,
}

pub fn windows_sandbox_available() -> bool {
    cfg!(windows)
}

pub fn should_wrap_profile(profile: Option<&str>) -> bool {
    if !windows_sandbox_available() {
        return false;
    }
    match profile.map(str::trim) {
        None | Some("") | Some("off") | Some("none") => false,
        Some(_) => true,
    }
}

#[derive(Debug, Clone)]
pub struct WindowsSandboxRequest {
    pub command: String,
    pub shell_program: String,
    pub shell_args: Vec<String>,
    pub cwd: PathBuf,
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
    pub restrict_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsSandboxLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub use protocol::config_types::WindowsSandboxLevel;
pub use protocol::models::PermissionProfile;

pub fn prepare_windows_sandbox_launch(
    req: &WindowsSandboxRequest,
) -> anyhow::Result<Option<WindowsSandboxLaunch>> {
    #[cfg(windows)]
    {
        Ok(Some(launch::prepare_launch(req)?))
    }
    #[cfg(not(windows))]
    {
        let _ = req;
        Ok(None)
    }
}

/// CLI early-dispatch hook: if argv requests the Windows sandbox wrapper, run it
/// (never returns on success). Otherwise returns `Ok(false)`.
pub fn run_as_windows_sandbox_if_requested(args: &[String]) -> anyhow::Result<bool> {
    let has_sentinel = args
        .iter()
        .any(|arg| arg == "--run-as-windows-sandbox" || arg == "--run-as-devo-windows-sandbox");
    if !has_sentinel {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        let _ = args;
        run_windows_sandbox_wrapper_main();
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        anyhow::bail!("windows sandbox wrapper invoked on a non-Windows host");
    }
}

#[cfg(not(windows))]
pub use capture_stub::CaptureResult;
#[cfg(not(windows))]
pub use capture_stub::run_windows_sandbox_capture;
#[cfg(not(windows))]
pub use capture_stub::run_windows_sandbox_legacy_preflight;
#[cfg(windows)]
pub use windows_impl::CaptureResult;
#[cfg(windows)]
pub use windows_impl::run_windows_sandbox_capture;
#[cfg(windows)]
pub use windows_impl::run_windows_sandbox_capture_with_filesystem_overrides;
#[cfg(windows)]
pub use windows_impl::run_windows_sandbox_legacy_preflight;

#[cfg(windows)]
mod acl;
#[cfg(windows)]
mod allow;
#[cfg(windows)]
mod audit;
#[cfg(windows)]
mod cap;
#[cfg(not(windows))]
mod capture_stub;
#[cfg(windows)]
mod conpty;
#[cfg(windows)]
mod deny_read_acl;
mod deny_read_resolver;
#[cfg(windows)]
mod deny_read_state;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod dpapi;
#[cfg(windows)]
mod elevated;
#[cfg(windows)]
mod elevated_impl;
#[cfg(windows)]
mod env;
#[cfg(windows)]
mod helper_materialization;
#[cfg(windows)]
mod hide_users;
#[cfg(windows)]
mod identity;
#[cfg(windows)]
mod launch;
#[cfg(windows)]
mod logging;
mod otel_stub;
#[cfg(windows)]
mod path_normalization;
mod path_util;
#[cfg(windows)]
mod proc_thread_attr;
#[cfg(windows)]
mod process;
mod protocol;
#[cfg(windows)]
mod pty;
#[cfg(windows)]
mod request_adapter;
#[cfg(windows)]
mod resolved_permissions;
#[cfg(windows)]
mod sandbox_utils;
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod setup_error;
#[cfg(windows)]
mod spawn_prep;
#[cfg(any(windows, test))]
mod ssh_config_dependencies;
#[cfg(windows)]
mod stdio_bridge;
mod string_util;
#[cfg(windows)]
mod token;
#[cfg(windows)]
mod unified_exec;
#[cfg(windows)]
mod wfp;
#[cfg(windows)]
mod wfp_setup;
#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
mod winutil;
#[cfg(windows)]
mod workspace_acl;
#[cfg(windows)]
mod wrapper;
#[cfg(windows)]
pub(crate) use elevated::ipc_framed;
#[cfg(windows)]
pub(crate) use elevated::runner_client;
#[cfg(windows)]
pub(crate) use elevated::runner_pipe;

#[cfg(windows)]
pub use acl::add_deny_read_ace;
#[cfg(windows)]
pub use acl::add_deny_write_ace;

#[cfg(windows)]
pub use acl::allow_null_device;
#[cfg(windows)]
pub use acl::ensure_allow_mask_aces;
#[cfg(windows)]
pub use acl::ensure_allow_mask_aces_with_inheritance;
#[cfg(windows)]
pub use acl::ensure_allow_write_aces;
#[cfg(windows)]
pub use acl::fetch_dacl_handle;
#[cfg(windows)]
pub use acl::path_mask_allows;
#[cfg(windows)]
pub use audit::apply_world_writable_scan_and_denies_for_permissions;
#[cfg(windows)]
pub use cap::load_or_create_cap_sids;
#[cfg(windows)]
pub use cap::workspace_cap_sid_for_cwd;
#[cfg(windows)]
pub use cap::workspace_write_cap_sid_for_root;
#[cfg(windows)]
pub use cap::workspace_write_root_contains_path;
#[cfg(windows)]
pub use cap::workspace_write_root_overlaps_path;
#[cfg(windows)]
pub use conpty::ConptyInstance;
#[cfg(windows)]
pub use conpty::spawn_conpty_process_as_user;
#[cfg(windows)]
pub use deny_read_acl::apply_deny_read_acls;
#[cfg(windows)]
pub use deny_read_acl::plan_deny_read_acl_paths;
pub use deny_read_resolver::resolve_windows_deny_read_paths;
#[cfg(windows)]
pub use deny_read_state::sync_persistent_deny_read_acls;
#[cfg(windows)]
pub use desktop::LaunchDesktop;
#[cfg(windows)]
pub use dpapi::protect as dpapi_protect;
#[cfg(windows)]
pub use dpapi::unprotect as dpapi_unprotect;
#[cfg(windows)]
pub use elevated_impl::ElevatedSandboxProfileCaptureRequest;
#[cfg(windows)]
pub use elevated_impl::run_windows_sandbox_capture_for_permission_profile as run_windows_sandbox_capture_for_permission_profile_elevated;
#[cfg(windows)]
pub use helper_materialization::resolve_current_exe_for_launch;
#[cfg(windows)]
pub use helper_materialization::resolve_exe_for_launch;
#[cfg(windows)]
pub use hide_users::hide_current_user_profile_dir;
#[cfg(windows)]
pub use hide_users::hide_newly_created_users;
#[cfg(windows)]
pub use identity::require_logon_sandbox_creds;
#[cfg(windows)]
pub use identity::sandbox_setup_is_complete;
#[cfg(windows)]
pub use ipc_framed::ErrorPayload;
#[cfg(windows)]
pub use ipc_framed::ErrorStage;
#[cfg(windows)]
pub use ipc_framed::ExitPayload;
#[cfg(windows)]
pub use ipc_framed::FramedMessage;
#[cfg(windows)]
pub use ipc_framed::IPC_PROTOCOL_VERSION;
#[cfg(windows)]
pub use ipc_framed::Message;
#[cfg(windows)]
pub use ipc_framed::OutputPayload;
#[cfg(windows)]
pub use ipc_framed::OutputStream;
#[cfg(windows)]
pub use ipc_framed::ResizePayload;
#[cfg(windows)]
pub use ipc_framed::SpawnReady;
#[cfg(windows)]
pub use ipc_framed::SpawnRequest;
#[cfg(windows)]
pub use ipc_framed::decode_bytes;
#[cfg(windows)]
pub use ipc_framed::encode_bytes;
#[cfg(windows)]
pub use ipc_framed::read_frame;
#[cfg(windows)]
pub use ipc_framed::write_frame;
#[cfg(windows)]
pub use logging::current_log_file_path;
#[cfg(windows)]
pub use logging::current_log_file_path_for_devo_home;
#[cfg(windows)]
pub use logging::log_file_path_for_utc_date;
#[cfg(windows)]
pub use logging::log_note;
#[cfg(windows)]
pub use logging::log_writer;
#[cfg(windows)]
pub use path_normalization::canonicalize_path;
#[cfg(windows)]
pub use process::ConsoleMode;
#[cfg(windows)]
pub use process::PipeSpawnHandles;
#[cfg(windows)]
pub use process::StderrMode;
#[cfg(windows)]
pub use process::StdinMode;
#[cfg(windows)]
pub use process::create_process_as_user;
#[cfg(windows)]
pub use process::read_handle_loop;
#[cfg(windows)]
pub use process::spawn_process_with_pipes;
#[cfg(windows)]
pub use resolved_permissions::ResolvedWindowsSandboxPermissions;
#[cfg(windows)]
pub use resolved_permissions::WindowsSandboxTokenMode;
#[cfg(windows)]
pub use resolved_permissions::token_mode_for_permission_profile;
#[cfg(windows)]
pub use setup::SETUP_VERSION;
#[cfg(windows)]
pub use setup::SandboxSetupRequest;
#[cfg(windows)]
pub use setup::SetupRootOverrides;
#[cfg(windows)]
pub use setup::run_elevated_provisioning_setup;
#[cfg(windows)]
pub use setup::run_elevated_setup;
#[cfg(windows)]
pub use setup::run_setup_refresh;
#[cfg(windows)]
pub use setup::run_setup_refresh_with_extra_read_roots;
#[cfg(windows)]
pub use setup::sandbox_bin_dir;
#[cfg(windows)]
pub use setup::sandbox_dir;
#[cfg(windows)]
pub use setup::sandbox_secrets_dir;
#[cfg(windows)]
pub use setup_error::SetupErrorCode;
#[cfg(windows)]
pub use setup_error::SetupErrorReport;
#[cfg(windows)]
pub use setup_error::SetupFailure;
#[cfg(windows)]
pub use setup_error::extract_failure as extract_setup_failure;
#[cfg(windows)]
pub use setup_error::sanitize_setup_metric_tag_value;
#[cfg(windows)]
pub use setup_error::setup_error_path;
#[cfg(windows)]
pub use setup_error::write_setup_error_report;
#[cfg(windows)]
pub use stdio_bridge::forward_sandbox_session_stdio;
#[cfg(windows)]
#[doc(hidden)]
pub use token::LocalSid;
#[cfg(windows)]
pub use token::convert_string_sid_to_sid;
#[cfg(windows)]
pub use token::create_readonly_token_with_cap_from;
#[cfg(windows)]
pub use token::create_readonly_token_with_caps_and_user_from;
#[cfg(windows)]
pub use token::create_readonly_token_with_caps_from;
#[cfg(windows)]
pub use token::create_workspace_write_token_with_caps_and_user_from;
#[cfg(windows)]
pub use token::create_workspace_write_token_with_caps_from;
#[cfg(windows)]
pub use token::get_current_token_for_restriction;
#[cfg(windows)]
pub use unified_exec::WindowsSandboxSessionRequest;
#[cfg(windows)]
pub use unified_exec::spawn_windows_sandbox_session_elevated_for_permission_profile;
#[cfg(windows)]
pub use unified_exec::spawn_windows_sandbox_session_for_level;
#[cfg(windows)]
pub use unified_exec::spawn_windows_sandbox_session_legacy;
#[cfg(windows)]
pub use wfp::install_wfp_filters_for_account;
#[cfg(windows)]
pub use wfp_setup::install_wfp_filters;
#[cfg(windows)]
#[cfg(windows)]
#[cfg(windows)]
#[cfg(windows)]
#[cfg(windows)]
pub use winutil::quote_windows_arg;
#[cfg(windows)]
pub use winutil::string_from_sid_bytes;
#[cfg(windows)]
pub use winutil::to_wide;
#[cfg(windows)]
pub use workspace_acl::is_command_cwd_root;
#[cfg(windows)]
pub use wrapper::DEVO_WINDOWS_SANDBOX_ARG1;
#[cfg(windows)]
pub use wrapper::create_windows_sandbox_command_args_for_permission_profile;
#[cfg(windows)]
pub use wrapper::run_windows_sandbox_wrapper_main;

#[cfg(not(windows))]
#[cfg(not(windows))]
#[cfg(not(windows))]
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn off_profile_never_wraps() {
        assert!(!should_wrap_profile(Some("off")));
        assert!(!should_wrap_profile(None));
    }

    #[test]
    fn prepare_none_on_non_windows() {
        let launch = prepare_windows_sandbox_launch(&WindowsSandboxRequest {
            command: "echo hi".to_string(),
            shell_program: "cmd.exe".to_string(),
            shell_args: vec!["/C".to_string()],
            cwd: PathBuf::from("."),
            readable_roots: vec![],
            writable_roots: vec![],
            deny_read: vec![],
            restrict_network: false,
        })
        .expect("prepare");
        #[cfg(not(windows))]
        assert_eq!(launch, None);
    }
}
