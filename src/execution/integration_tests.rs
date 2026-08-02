use crate::test_support::*;

#[test]
fn windows_provider_identity_change_is_rejected_before_launch() {
    let path = env::temp_dir().join(format!("xuva-provider-identity-{}.bin", std::process::id()));
    fs::write(&path, b"first").expect("identity fixture is written");
    let path_text = path.to_string_lossy().into_owned();
    let expected = windows_binary_identity(&path_text).expect("fixture identity is captured");

    validate_windows_binary_identity(&OsString::from(&path_text), &expected)
        .expect("unchanged identity is accepted");
    fs::write(&path, b"replacement-content").expect("identity fixture is replaced");
    let error = validate_windows_binary_identity(&OsString::from(&path_text), &expected)
        .expect_err("changed identity must be rejected before launch");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

    let _ = fs::remove_file(path);
}

#[test]
fn cross_host_isolation_uses_origin_identity_and_preserves_unc_cwd() {
    let mut config = default_config();
    config.invocation_origin = InvocationOrigin::Wsl {
        distro: "Ubuntu".to_owned(),
    };
    config.cwd = Some("/home/test/project".to_owned());
    config.bridge_windows_cwd = Some(r"\\wsl.localhost\Ubuntu\home\test\project".to_owned());
    let candidate = ProviderCandidate {
        host: ProviderHost::Windows,
        adapters: vec![AdapterKind::Raw],
        distro: None,
        wsl_version: None,
        executable: r"C:\Tools\tool.exe".to_owned(),
        executable_identity: Some(fixture_binary_identity(r"C:\Tools\tool.exe")),
        rtk: None,
        rtk_identity: None,
        project_path: config.bridge_windows_cwd.clone(),
        usable: true,
        reason: "fixture".to_owned(),
    };

    assert_eq!(
        provider_environment_policy(&config, &candidate),
        dispatcher::EnvironmentPolicy::Isolated
    );
    assert_eq!(
        windows_cwd_for_invocation(&config).expect("UNC mapping is usable"),
        PathBuf::from(r"\\wsl.localhost\Ubuntu\home\test\project")
    );

    config.invocation_origin = InvocationOrigin::Windows;
    assert_eq!(
        provider_environment_policy(&config, &candidate),
        dispatcher::EnvironmentPolicy::Inherit
    );
}

#[test]
fn explicit_provider_selection_skips_adapter_incompatible_candidates() {
    let mut config = default_config();
    config.output_adapter = OutputAdapterPreference::Rtk;
    let candidates = vec![
        ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Tools\tool.exe".to_owned(),
            executable_identity: Some(fixture_binary_identity(r"C:\Tools\tool.exe")),
            rtk: None,
            rtk_identity: None,
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "raw-only Windows fixture".to_owned(),
        },
        ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/bin/tool".to_owned(),
            executable_identity: Some(fixture_binary_identity("/usr/bin/tool")),
            rtk: Some("/usr/local/bin/rtk".to_owned()),
            rtk_identity: Some(fixture_binary_identity("/usr/local/bin/rtk")),
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "RTK-capable WSL fixture".to_owned(),
        },
    ];

    let (index, candidate, plan) =
        first_compatible_provider_plan("tool", &[], &config, &candidates)
            .expect("a compatible provider exists");
    assert_eq!(index, 1);
    assert_eq!(candidate.host, ProviderHost::Wsl2);
    assert!(matches!(
        plan.adapter,
        dispatcher::OutputAdapter::Rtk { .. }
    ));
}

#[test]
fn policy_objective_is_part_of_the_local_evidence_context() {
    let balanced = default_config();
    let mut latency = balanced.clone();
    latency.policy_objective = PolicyObjective::Latency;
    assert_ne!(
        adaptive_context_signature(&balanced),
        adaptive_context_signature(&latency)
    );
}

#[test]
fn local_calibration_signature_does_not_expose_command_text() {
    let arguments = vec![OsString::from("rg"), OsString::from("sensitive value")];
    let signature = calibration_signature(&arguments, Some(r"E:\work"));
    assert_eq!(signature.len(), 16);
    assert!(!signature.contains("sensitive"));
    assert_ne!(
        signature,
        calibration_signature(
            &[OsString::from("rg"), OsString::from("other")],
            Some(r"E:\work")
        )
    );
}

#[test]
fn provider_registry_accepts_safe_generic_tool_names_only() {
    for tool in ["git", "python3", "cargo-next", "tool.name", "go"] {
        assert!(
            is_safe_provider_tool_name(tool),
            "{tool} should be accepted"
        );
    }
    for tool in ["", "../tool", "tool/path", "tool;echo", "tool name", "工具"] {
        assert!(
            !is_safe_provider_tool_name(tool),
            "{tool} should be rejected"
        );
    }
}

#[test]
fn provider_registry_parses_wsl_binary_identity_without_retaining_command_output() {
    let identity = parse_wsl_binary_identity(
        Some("/usr/local/bin/rtk".to_owned()),
        Some("8|42|2291200|2024-07-25 00:00:00.000000000 +0000".to_owned()),
    )
    .expect("valid stat identity is parsed");
    assert_eq!(identity.path, "/usr/local/bin/rtk");
    assert_eq!(identity.file_key, "8:42");
    assert_eq!(identity.size_bytes, 2_291_200);
    assert_eq!(
        identity.modified_stamp,
        "2024-07-25 00:00:00.000000000 +0000"
    );
    assert!(
        parse_wsl_binary_identity(Some("/bin/tool".to_owned()), Some("bad".to_owned())).is_none()
    );
}

#[test]
fn provider_fingerprint_separates_partial_and_complete_wsl_inventory() {
    let config = default_config();
    assert_ne!(
        discovery_context_signature(&config, false),
        discovery_context_signature(&config, true),
        "a Windows-only cache must not satisfy a complete WSL inventory request"
    );
}

#[test]
fn wsl_provider_probe_rejects_shell_builtins_as_executables() {
    assert_eq!(verified_wsl_executable_path("read".to_owned()), None);
    assert_eq!(
        verified_wsl_executable_path("/usr/bin/find".to_owned()),
        Some("/usr/bin/find".to_owned())
    );
}

#[test]
fn posix_command_families_do_not_collide_with_windows_system_tools() {
    for tool in ["find", "head", "tail", "grep", "tree"] {
        assert!(
            !windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
            "{tool}"
        );
        assert!(
            windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk),
            "{tool}"
        );
    }
    for tool in ["find", "head", "tail", "tree"] {
        assert!(requires_raw_posix_provider(tool), "{tool}");
    }
    assert!(!requires_raw_posix_provider("grep"));
    for tool in ["git", "cargo", "python3"] {
        assert!(
            windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
            "{tool}"
        );
    }
}

#[test]
fn execution_plans_translate_only_cross_host_absolute_path_arguments() {
    let windows = ProviderCandidate {
        host: ProviderHost::Windows,
        adapters: vec![AdapterKind::Raw],
        distro: None,
        wsl_version: None,
        executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
        executable_identity: Some(fixture_binary_identity(r"C:\Program Files\Git\cmd\git.exe")),
        rtk: None,
        rtk_identity: None,
        project_path: Some(r"E:\work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    let plan = execution_plan_for_provider_candidate(
        "git",
        &[
            OsString::from("-C"),
            OsString::from("/mnt/e/work"),
            OsString::from("status"),
            OsString::from("literal && value"),
        ],
        &default_config(),
        &windows,
    )
    .expect("Windows plan is valid");
    assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
    assert_eq!(
        plan.request.arguments[3],
        OsString::from("literal && value"),
        "non-path argv remains byte-for-byte structured"
    );

    let wsl = ProviderCandidate {
        host: ProviderHost::Wsl2,
        adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
        distro: Some("Ubuntu".to_owned()),
        wsl_version: Some(2),
        executable: "/usr/local/bin/rtk".to_owned(),
        executable_identity: Some(fixture_binary_identity("/usr/local/bin/rtk")),
        rtk: Some("/usr/local/bin/rtk".to_owned()),
        rtk_identity: Some(fixture_binary_identity("/usr/local/bin/rtk")),
        project_path: Some("/mnt/e/work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    let plan = execution_plan_for_provider_candidate(
        "read",
        &[OsString::from(r"E:\work\Cargo.toml")],
        &default_config(),
        &wsl,
    )
    .expect("WSL plan is valid");
    assert_eq!(
        plan.request.arguments,
        vec![OsString::from("/mnt/e/work/Cargo.toml")]
    );
}

#[test]
fn windows_git_mutations_have_no_wsl_execution_fallback() {
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "git".to_owned(),
        cache: "hit",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        },
        availability: ProviderCacheEntry {
            tool: "git".to_owned(),
            observed_unix_seconds: 1,
            inspection_level: InspectionLevel::Identity,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: Some(r"C:\Program Files\Git\cmd\git.exe".to_owned()),
                native_rtk: None,
                executable_version: None,
                version_probe_status: ProbeStatus::NotRequested,
                executable_capabilities: Vec::new(),
                executable_identity: None,
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: Vec::new(),
        },
        candidates: vec![
            ProviderCandidate {
                host: ProviderHost::Windows,
                adapters: vec![AdapterKind::Raw],
                distro: None,
                wsl_version: None,
                executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
                executable_identity: Some(fixture_binary_identity(
                    r"C:\Program Files\Git\cmd\git.exe",
                )),
                rtk: None,
                rtk_identity: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "fixture".to_owned(),
            },
            ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/bin/git".to_owned(),
                executable_identity: Some(fixture_binary_identity("/usr/bin/git")),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                rtk_identity: Some(fixture_binary_identity("/usr/local/bin/rtk")),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture".to_owned(),
            },
        ],
        recommended: Some(0),
        diagnosis: "fixture".to_owned(),
        install: "disabled",
    };
    match provider_dispatch_decision_from_resolution(
        &[
            OsString::from("git"),
            OsString::from("-C"),
            OsString::from("/mnt/e/work"),
            OsString::from("push"),
            OsString::from("origin"),
            OsString::from("HEAD"),
        ],
        &default_config(),
        Route::Wsl1,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan {
            plan,
            fallbacks,
            reason,
        } => {
            assert!(matches!(
                plan.candidate,
                dispatcher::RouteCandidate::Windows { .. }
            ));
            assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
            assert!(fallbacks.is_empty());
            assert!(reason.contains("Windows DNS"));
            assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
        }
        _ => panic!("Windows Git mutation must produce a native-only plan"),
    }
}
