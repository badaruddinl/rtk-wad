use crate::providers::model::BinaryIdentity;
use crate::test_support::*;

#[test]
fn wsl_plan_launcher_forwards_environment_as_structured_assignments() {
    let config = default_config();
    let identity = BinaryIdentity {
        path: "/tmp/go".to_owned(),
        file_key: "1:2".to_owned(),
        size_bytes: 3,
        modified_stamp: "fixture".to_owned(),
    };
    let arguments = plan_wsl_arguments_with_metrics(
        &OsString::from("/tmp/go"),
        &[OsString::from("run"), OsString::from("$literal & text")],
        &[(
            OsString::from("P7_OVERLAY"),
            OsString::from("value with spaces"),
        )],
        &config,
        Route::Wsl2,
        WslLaunchMetadata {
            cancel_nonce: Some("0123456789abcdef0123456789abcdef"),
            metrics_db_path: None,
            attestation_path: Some("/tmp/xuva-test.attestation"),
            permit_path: Some("/tmp/xuva-test.permit"),
            completion_path: Some("/tmp/xuva-test.completion"),
            expected_identity: Some(&identity),
        },
    )
    .expect("WSL plan arguments are valid");
    let executable = arguments
        .iter()
        .position(|argument| argument == "/tmp/go")
        .expect("plan includes executable");
    let overlay = arguments
        .iter()
        .position(|argument| argument == "P7_OVERLAY=value with spaces")
        .expect("plan includes environment overlay");
    let user_argument = arguments
        .iter()
        .position(|argument| argument == "$literal & text")
        .expect("plan includes literal user argument");
    assert!(arguments.contains(&OsString::from(PLAN_LAUNCH_SCRIPT)));
    assert!(arguments.contains(&OsString::from("1:2")));
    assert!(arguments.contains(&OsString::from("fixture")));
    assert!(PLAN_LAUNCH_SCRIPT.contains("stat -Lc '%d:%i|%s|%y'"));
    assert!(PLAN_LAUNCH_SCRIPT.contains("identity changed before launch"));
    assert!(
        overlay < executable && executable < user_argument,
        "env must receive overlays before the identity-verified executable"
    );
    assert!(
        wsl_environment_assignments(&[(OsString::from("INVALID-NAME"), OsString::from("value"),)])
            .is_err()
    );
}

#[test]
fn execution_plan_applies_command_environment_and_cwd_to_windows_processes() {
    let request = dispatcher::CommandSpec {
        executable: OsString::from("fixture.exe"),
        arguments: vec![OsString::from("space value"), OsString::from("$literal")],
        cwd: Some(PathBuf::from(r"E:\work")),
        environment: vec![(OsString::from("P7_OVERLAY"), OsString::from("enabled"))],
        environment_policy: dispatcher::EnvironmentPolicy::Isolated,
        interactive: true,
    };
    let mut command = Command::new("fixture.exe");
    apply_command_spec(&mut command, &request);
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![&OsString::from("space value"), &OsString::from("$literal")]
    );
    assert_eq!(command.get_current_dir(), Some(Path::new(r"E:\work")));
    assert!(command.get_envs().any(|(key, value)| {
        key == "P7_OVERLAY" && value == Some(std::ffi::OsStr::new("enabled"))
    }));
}

#[test]
fn explicit_wsl1_route_uses_the_windows_mutex_and_supervised_process_group() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
        _ => None,
    })
    .expect("explicit WSL1 configuration is valid");
    let command = wsl1_rtk_arguments(
        vec![
            OsString::from("proxy"),
            OsString::from("/usr/bin/printf"),
            OsString::from("%s"),
            OsString::from("space & $HOME"),
        ],
        &config,
    );
    let strings = command
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();

    assert!(
        strings
            .iter()
            .any(|value| value.contains("publish_completion"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("/usr/bin/env -i"))
    );
    assert!(!strings.iter().any(|value| value.contains("/usr/bin/flock")));
    assert!(strings.iter().any(|value| value == "/usr/bin/setsid"));
    assert_eq!(
        strings.last().map(|value| value.as_ref()),
        Some("space & $HOME")
    );
}

#[test]
fn every_wsl1_launch_surface_uses_the_same_strict_marker_validator() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
        _ => None,
    })
    .expect("explicit WSL1 configuration is valid");
    let rtk_arguments = wsl1_rtk_arguments_with_metrics(
        vec![OsString::from("smart")],
        &config,
        None,
        "/tmp/xuva-test.attestation",
        "/tmp/xuva-test.permit",
        "/tmp/xuva-test.completion",
    );
    let identity = BinaryIdentity {
        path: "/usr/bin/printf".to_owned(),
        file_key: "1:2".to_owned(),
        size_bytes: 3,
        modified_stamp: "fixture".to_owned(),
    };
    let plan_arguments = plan_wsl_arguments_with_metrics(
        &OsString::from("/usr/bin/printf"),
        &[OsString::from("%s"), OsString::from("fixture")],
        &[],
        &config,
        Route::Wsl1,
        WslLaunchMetadata {
            cancel_nonce: None,
            metrics_db_path: None,
            attestation_path: Some("/tmp/xuva-test.attestation"),
            permit_path: Some("/tmp/xuva-test.permit"),
            completion_path: Some("/tmp/xuva-test.completion"),
            expected_identity: Some(&identity),
        },
    )
    .expect("WSL1 plan arguments are valid");

    for arguments in [&rtk_arguments, &plan_arguments] {
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.as_os_str() == WSL1_MARKER_VALIDATOR_SCRIPT)
                .count(),
            1,
            "each WSL1 launch must receive the canonical marker validator exactly once"
        );
        assert!(
            arguments
                .iter()
                .any(|argument| argument.as_os_str() == WSL1_LAUNCH_SCRIPT)
        );
    }
    assert!(!WSL1_LAUNCH_SCRIPT.contains("marker=/etc/xuva-dedicated-wsl1"));
    assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("stat -Lc '%u:%a'"));
    assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("!= \"0:444\""));
    assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("grep -c '^installation_id='"));
}

#[test]
fn wsl1_proxy_cannot_report_success_before_target_authorization() {
    let success = Command::new("cmd.exe")
        .args(["/d", "/c", "exit 0"])
        .status()
        .expect("successful proxy fixture starts");
    let rejected = verify_pre_authorization_proxy_status(success)
        .expect_err("pre-authorization success must not impersonate target success");
    assert!(rejected.to_string().contains("target was not executed"));

    let failure = Command::new("cmd.exe")
        .args(["/d", "/c", "exit 126"])
        .status()
        .expect("failed proxy fixture starts");
    assert_eq!(
        verify_pre_authorization_proxy_status(failure)
            .expect("launcher failure remains observable")
            .code(),
        Some(126)
    );
}

#[test]
fn stats_remains_a_compatibility_alias() {
    let arguments = rtk_arguments(
        vec![OsString::from("stats")],
        &default_config(),
        "0123456789abcdef0123456789abcdef",
    );
    assert_eq!(arguments.last(), Some(&OsString::from("gain")));
}

#[test]
fn maps_windows_drive_paths_for_wsl_current_directory() {
    assert_eq!(
        windows_path_to_wsl_path(r"D:\projects\rtk-wsl"),
        Some("/mnt/d/projects/rtk-wsl".to_owned())
    );
    assert_eq!(
        windows_path_to_wsl_path(r"F:\path with spaces\漢字"),
        Some("/mnt/f/path with spaces/漢字".to_owned())
    );
    assert_eq!(
        windows_path_to_wsl_path(r"\\?\E:\projects\rtk-wsl"),
        Some("/mnt/e/projects/rtk-wsl".to_owned())
    );
    assert_eq!(windows_path_to_wsl_path(r"\\server\share"), None);
}

#[test]
fn defaults_to_the_selected_wsl_users_home() {
    let arguments = rtk_arguments(
        vec![OsString::from("help")],
        &default_config(),
        "0123456789abcdef0123456789abcdef",
    );

    assert!(arguments.contains(&OsString::from("")));
    assert!(arguments.iter().any(|argument| {
        argument
            .to_string_lossy()
            .contains("rtk_path=\"$HOME/.local/bin/rtk\"")
    }));
    assert!(!arguments.contains(&OsString::from("-u")));
}

#[test]
fn validates_configuration_without_ambient_user_defaults() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
        "XUVA_WSL_USER" => Some("alex".to_owned()),
        "XUVA_WSL_RTK_PATH" => Some("/opt/rtk/bin/rtk".to_owned()),
        "XUVA_WSL_CWD" => Some("/work/custom-mount".to_owned()),
        "XUVA_WSL_EXTRA_PATH" => Some("/opt/fixture-bin:/work/tools".to_owned()),
        _ => None,
    })
    .expect("portable config is valid");

    let arguments = rtk_arguments(
        vec![OsString::from("help")],
        &config,
        "0123456789abcdef0123456789abcdef",
    );
    assert!(arguments.windows(2).any(|pair| pair == ["-u", "alex"]));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--cd", "/work/custom-mount"])
    );
    assert!(arguments.contains(&OsString::from("/opt/rtk/bin/rtk")));
    assert!(arguments.contains(&OsString::from("/opt/fixture-bin:/work/tools")));
}

#[test]
fn rejects_unsafe_or_ambiguous_configuration() {
    let invalid_wait = Config::from_lookup(|name| match name {
        "XUVA_WSL_LOCK_WAIT_SECONDS" => Some("0".to_owned()),
        _ => None,
    });
    assert!(invalid_wait.is_err());

    let relative_path = Config::from_lookup(|name| match name {
        "XUVA_WSL_RTK_PATH" => Some("bin/rtk".to_owned()),
        _ => None,
    });
    assert!(relative_path.is_err());

    let invalid_extra_path = Config::from_lookup(|name| match name {
        "XUVA_WSL_EXTRA_PATH" => Some("relative:/opt/tools".to_owned()),
        _ => None,
    });
    assert!(invalid_extra_path.is_err());

    let invalid_objective = Config::from_lookup(|name| match name {
        "XUVA_POLICY_OBJECTIVE" => Some("fastest-ish".to_owned()),
        _ => None,
    });
    assert!(invalid_objective.is_err());
}

#[test]
fn cancellation_uses_a_separate_structured_wsl_command() {
    let arguments = cancel_arguments(
        &default_config(),
        "0123456789abcdef0123456789abcdef",
        "TERM",
    );
    assert!(arguments.contains(&OsString::from(CANCEL_SCRIPT)));
    assert!(arguments.contains(&OsString::from("0123456789abcdef0123456789abcdef")));
    assert!(
        !arguments
            .iter()
            .any(|argument| { argument.to_string_lossy().starts_with("/tmp/xuva-") })
    );
    assert!(arguments.contains(&OsString::from("TERM")));
}

#[test]
fn launch_permit_requires_the_exact_attested_identity_and_cleans_up() {
    let expected = "0123456789abcdef0123456789abcdef".to_owned();
    let (attestation, permit, completion);
    {
        let guard =
            LaunchPermitGuard::new("unit", expected.clone()).expect("launch guard is created");
        attestation = guard.attestation_windows_path.clone();
        permit = guard.permit_windows_path.clone();
        completion = guard.completion_windows_path.clone();
        let mut staging = attestation.as_os_str().to_os_string();
        staging.push(".staging");
        fs::write(PathBuf::from(staging), &expected).expect("staged attestation is written");
        assert!(
            !guard
                .is_attested()
                .expect("an unpublished attestation remains invisible")
        );
        fs::write(&attestation, "ffffffffffffffffffffffffffffffff")
            .expect("mismatched attestation is written");
        let mismatch = guard
            .is_attested()
            .expect_err("mismatched launch identity is rejected");
        assert_eq!(mismatch.kind(), std::io::ErrorKind::PermissionDenied);

        fs::write(&attestation, &expected).expect("matching attestation is written");
        assert!(guard.is_attested().expect("attestation is readable"));
        guard.authorize().expect("matching launch is authorized");
        assert_eq!(
            fs::read_to_string(&permit).expect("permit is readable"),
            expected
        );
        fs::write(&completion, format!("{expected}:37")).expect("matching completion is written");
        assert_eq!(
            guard.completion_status().expect("completion is valid"),
            Some(37)
        );
        fs::write(&completion, format!("{expected}:999"))
            .expect("out-of-range completion is written");
        assert_eq!(
            guard
                .completion_status()
                .expect_err("out-of-range completion is rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        fs::write(&completion, "ffffffffffffffffffffffffffffffff:37")
            .expect("mismatched completion is written");
        assert_eq!(
            guard
                .completion_status()
                .expect_err("mismatched completion identity is rejected")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        fs::write(&completion, "not-a-completion").expect("malformed completion is written");
        assert_eq!(
            guard
                .completion_status()
                .expect_err("malformed completion is rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    assert!(!attestation.exists());
    assert!(!permit.exists());
    assert!(!completion.exists());
}

#[test]
fn unbound_launch_permit_requires_explicit_identity_acceptance() {
    let installation_id = "01234567-89ab-cdef-0123-456789abcdef";
    let guard = LaunchPermitGuard::new_unbound("unit-unbound").expect("unbound guard is created");
    assert_eq!(
        guard
            .is_attested()
            .expect_err("unbound attestation cannot be accepted implicitly")
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        guard
            .authorize()
            .expect_err("unbound permit cannot publish an implicit identity")
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    fs::write(&guard.attestation_windows_path, installation_id)
        .expect("dedicated identity attestation is written");
    assert_eq!(
        guard
            .attested_value()
            .expect("attestation is readable")
            .as_deref(),
        Some(installation_id)
    );
    guard
        .authorize_value(installation_id)
        .expect("explicitly accepted dedicated identity is authorized");
    assert_eq!(
        fs::read_to_string(&guard.permit_windows_path).expect("permit is readable"),
        installation_id
    );
}

#[test]
fn wsl2_launchers_only_remove_proven_dead_cancellation_tokens() {
    for script in [LAUNCH_SCRIPT, PLAN_LAUNCH_SCRIPT] {
        assert!(!script.contains("-mmin"));
        assert!(script.contains("/bin/kill -0 -- \"-$stale_worker\""));
        assert!(script.contains("group_has_other_members"));
        assert!(script.contains("publish_completion"));
        assert_eq!(
            script.matches("stat_fields=${stat_value##*) }").count(),
            2,
            "both process-group scans must parse after the final comm delimiter"
        );
        assert!(
            !script.contains("stat_fields=${stat_value#*) }"),
            "shortest-prefix /proc stat parsing can misread comm containing `) `"
        );

        let finish = script
            .split_once("finish() {")
            .expect("launcher has a finish trap")
            .1
            .split_once("trap finish EXIT")
            .expect("launcher finish trap is installed")
            .0;
        let failed_quiescence_exit = finish
            .find("exit 125")
            .expect("failed quiescence exits without attestation");
        let cleanup = finish
            .find("\n    cleanup\n")
            .expect("successful quiescence removes its token");
        let completion = finish
            .find("\n    publish_completion ")
            .expect("successful quiescence publishes completion");
        assert!(
            failed_quiescence_exit < cleanup && cleanup < completion,
            "cleanup and completion must remain unreachable after quiescence failure"
        );
        assert!(
            !finish[..failed_quiescence_exit].contains("publish_completion"),
            "failed quiescence must not publish cleanup proof"
        );
    }
}
