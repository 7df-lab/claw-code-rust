use super::*;
use pretty_assertions::assert_eq;

fn temp_workspace(tag: &str, toml_body: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let workspace =
        std::env::temp_dir().join(format!("devo-wrap-{tag}-{}-{nanos}", std::process::id()));
    let devo = workspace.join(".devo");
    std::fs::create_dir_all(&devo).expect("create sandbox config directory");
    std::fs::write(devo.join("sandbox.toml"), toml_body).expect("write sandbox config");
    workspace
}

fn resolved_profile(deny: &[&str], restrict_network: bool) -> SandboxProfile {
    SandboxProfile {
        name: "test".to_string(),
        read_only: vec![],
        read_write: vec![],
        deny: deny.iter().map(PathBuf::from).collect(),
        default_read: true,
        restrict_network,
    }
}

#[test]
fn none_and_off_profiles_never_wrap() {
    let workspace = Path::new("/tmp");
    let logger = SandboxLogger::new();
    for profile in [None, Some("off"), Some("none")] {
        for mode in [WrapMode::PtyOnly, WrapMode::PipeComposed] {
            assert_eq!(
                wrap_command_for_profile(profile, workspace, mode, &logger)
                    .expect("off/None profiles are not errors"),
                SandboxWrap::None,
                "profile {profile:?} in mode {mode:?} must not wrap"
            );
        }
    }
    assert!(
        logger.take_events().is_empty(),
        "off/None profiles must not record events"
    );
}

#[test]
fn undefined_custom_profile_is_an_error() {
    let workspace = temp_workspace("missing", "");
    let error = wrap_command_for_profile(
        Some("devo-test-missing-profile-xyz"),
        &workspace,
        WrapMode::PipeComposed,
        &SandboxLogger::new(),
    )
    .expect_err("an unresolvable profile name must fail, not silently unwrap");
    assert!(
        error.to_string().contains("not found"),
        "unexpected error: {error:#}"
    );
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
#[cfg(target_os = "macos")]
fn macos_pipe_and_pty_wrap_via_sandbox_exec() {
    let workspace = temp_workspace(
        "macos",
        "[profiles.wrapdeny]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n",
    );
    for mode in [WrapMode::PipeComposed, WrapMode::PtyOnly] {
        match wrap_command_for_profile(Some("wrapdeny"), &workspace, mode, &SandboxLogger::new())
            .expect("valid profile resolves")
        {
            SandboxWrap::Wrapped(wrapped) => {
                assert_eq!(wrapped.program, "/usr/bin/sandbox-exec");
                assert_eq!(wrapped.prefix_args.len(), 2, "{wrapped:?}");
                assert_eq!(wrapped.prefix_args[0], "-p");
                let sbpl = &wrapped.prefix_args[1];
                assert!(sbpl.contains("(deny default)"), "{sbpl}");
                assert!(sbpl.contains("(allow pseudo-tty)"), "{sbpl}");
                assert!(sbpl.contains("(deny file-read*"), "{sbpl}");
                assert_eq!(wrapped.placeholder_dir, None);
                assert!(wrapped.helper_enforces);
            }
            SandboxWrap::None => assert!(
                !Path::new("/usr/bin/sandbox-exec").is_file(),
                "sandbox-exec exists but the {mode:?} wrap was declined"
            ),
        }
    }
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn macos_wrap_without_launcher_records_not_enforced() {
    let logger = SandboxLogger::new();
    let wrap = macos_wrap(
        &ProfileName::Workspace,
        &resolved_profile(&["secret.txt"], false),
        Path::new("/tmp"),
        /*sandbox_exec_available*/ false,
        WrapMode::PtyOnly,
        &logger,
        false,
    )
    .expect("a missing launcher is a warn-and-release, not an error");

    assert_eq!(wrap, SandboxWrap::None);
    let events = logger.take_events();
    assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
    let event = &events[0];
    assert!(matches!(
        event.event_type,
        crate::types::SandboxEventType::NotEnforced
    ));
    assert_eq!(event.profile, "workspace");
    assert_eq!(event.mode.as_deref(), Some("PtyOnly"));
    assert_eq!(event.launcher.as_deref(), Some("sandbox-exec"));
    assert_eq!(event.enforced, Some(false));
}

#[test]
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn macos_wrap_success_records_profile_applied() {
    if !Path::new("/usr/bin/sandbox-exec").is_file() {
        eprintln!("skipping: sandbox-exec not available on this machine");
        return;
    }
    let workspace = temp_workspace(
        "macoslog",
        "[profiles.wraplog]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n",
    );
    let profile: ProfileName = "wraplog".parse().expect("valid custom profile name");
    let config = load_sandbox_config(&workspace).expect("load sandbox config");
    let resolved = profile
        .resolve_profile(&workspace, &config)
        .expect("custom profile resolves");
    let logger = SandboxLogger::new();

    let wrap = macos_wrap(
        &profile,
        &resolved,
        &workspace,
        /*sandbox_exec_available*/ true,
        WrapMode::PtyOnly,
        &logger,
        false,
    )
    .expect("wrap construction must not fail");

    assert!(matches!(&wrap, SandboxWrap::Wrapped(_)), "{wrap:?}");
    let events = logger.take_events();
    assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
    let event = &events[0];
    assert!(matches!(
        event.event_type,
        crate::types::SandboxEventType::ProfileApplied
    ));
    assert_eq!(event.profile, "wraplog");
    assert_eq!(event.mode.as_deref(), Some("PtyOnly"));
    assert_eq!(event.launcher.as_deref(), Some("/usr/bin/sandbox-exec"));
    assert_eq!(event.enforced, Some(true));
    assert_eq!(
        event.deny_paths.as_deref(),
        Some(&["secret.txt".to_string()][..])
    );
    let SandboxWrap::Wrapped(wrapped) = wrap else {
        panic!("macOS wrapper must enforce through sandbox-exec");
    };
    assert!(wrapped.helper_enforces);
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
#[cfg(target_os = "linux")]
fn linux_wrap_without_bwrap_records_not_enforced() {
    let logger = SandboxLogger::new();
    let wrap = linux_wrap(
        &ProfileName::Workspace,
        &SandboxConfig::default(),
        &resolved_profile(&["secret.txt"], false),
        Path::new("/tmp"),
        WrapMode::PipeComposed,
        LauncherAvailability {
            sandbox_exec: false,
            bwrap: false,
        },
        &logger,
    )
    .expect("a missing bwrap is a warn-and-release, not an error");

    assert_eq!(wrap, SandboxWrap::None);
    let events = logger.take_events();
    assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
    let event = &events[0];
    assert!(matches!(
        event.event_type,
        crate::types::SandboxEventType::NotEnforced
    ));
    assert_eq!(event.profile, "workspace");
    assert_eq!(event.mode.as_deref(), Some("PipeComposed"));
    assert_eq!(event.launcher.as_deref(), Some("bwrap"));
    assert_eq!(event.enforced, Some(false));
}

#[test]
fn launcher_override_values() {
    assert_eq!(launcher_override(None), LauncherOverride::Auto);
    assert_eq!(launcher_override(Some("auto")), LauncherOverride::Auto);
    assert_eq!(launcher_override(Some("none")), LauncherOverride::None);
    assert_eq!(launcher_override(Some("bwrap")), LauncherOverride::Bwrap);
    assert_eq!(
        launcher_override(Some("sandbox-exec")),
        LauncherOverride::SandboxExec
    );
    assert_eq!(launcher_override(Some("garbage")), LauncherOverride::Auto);
}

#[test]
#[cfg(target_os = "macos")]
fn macos_never_applies_seatbelt_in_child() {
    assert!(!SandboxWrap::None.requires_child_apply());
    assert!(
        !SandboxWrap::Wrapped(WrappedCommand {
            program: "/usr/bin/sandbox-exec".to_string(),
            prefix_args: vec![],
            placeholder_dir: None,
            helper_enforces: true,
        })
        .requires_child_apply()
    );
}

#[test]
#[cfg(target_os = "linux")]
fn linux_direct_spawn_still_applies_landlock_in_child() {
    assert!(SandboxWrap::None.requires_child_apply());
}

#[test]
fn linux_wrap_adds_enforcement_only_for_deny_or_network_in_pipe_mode() {
    let deny_profile = resolved_profile(&["secret.txt"], false);
    let net_profile = resolved_profile(&[], true);
    let plain_profile = resolved_profile(&[], false);

    assert!(linux_wrap_adds_enforcement(
        &deny_profile,
        WrapMode::PipeComposed
    ));
    assert!(linux_wrap_adds_enforcement(
        &net_profile,
        WrapMode::PipeComposed
    ));
    assert!(!linux_wrap_adds_enforcement(
        &plain_profile,
        WrapMode::PipeComposed
    ));
    for profile in [&deny_profile, &net_profile, &plain_profile] {
        assert!(
            linux_wrap_adds_enforcement(profile, WrapMode::PtyOnly),
            "PTY wraps always carry the full policy"
        );
    }
}

#[test]
fn placeholder_dir_name_guard_rejects_other_paths() {
    assert!(is_placeholder_dir_name(Path::new(
        "/home/u/.devo/bwrap-placeholder.abc123"
    )));
    assert!(!is_placeholder_dir_name(Path::new("/home/u/.devo")));
    assert!(!is_placeholder_dir_name(Path::new("/")));
    assert!(!is_placeholder_dir_name(Path::new(
        "/home/u/.devo/bwrap-placeholder"
    )));
}

#[test]
fn remove_placeholder_dir_refuses_foreign_directories() {
    let root = std::env::temp_dir().join(format!(
        "devo-wrap-guard-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("keep")).expect("create foreign directory");
    remove_placeholder_dir(&root.join("keep"));
    assert!(root.join("keep").is_dir(), "foreign directory must survive");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn janitor_removes_only_stale_placeholder_dirs() {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("devo-janitor-{}-{nanos}", std::process::id()));
    let placeholder = root.join("bwrap-placeholder.test01");
    std::fs::create_dir_all(&placeholder).expect("create placeholder directory");
    std::fs::write(placeholder.join("sandbox-blocked-0"), "x").expect("write placeholder file");
    std::fs::create_dir_all(root.join("keep")).expect("create foreign directory");

    // Young placeholders survive a normal sweep.
    cleanup_stale_placeholder_dirs_in(&root, SystemTime::now());
    assert!(placeholder.is_dir(), "young placeholder must survive");

    // A clock far in the future makes everything look stale: the
    // placeholder goes, the foreign directory stays.
    let far_future = SystemTime::now() + Duration::from_secs(72 * 60 * 60);
    cleanup_stale_placeholder_dirs_in(&root, far_future);
    assert!(!placeholder.exists(), "stale placeholder must be removed");
    assert!(root.join("keep").is_dir(), "foreign directory must survive");
    let _ = std::fs::remove_dir_all(&root);
}
