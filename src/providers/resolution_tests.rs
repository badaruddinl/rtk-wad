use crate::test_support::*;

#[test]
fn provider_resolution_requires_a_verified_cross_host_project_mapping() {
    let probe = WslToolProbe {
        distro: "Ubuntu".to_owned(),
        user: None,
        wsl_version: Some(2),
        dedicated: false,
        installation_id: None,
        executable: Some("/usr/bin/go".to_owned()),
        rtk: Some("/home/test/.local/bin/rtk".to_owned()),
        executable_version: None,
        version_probe_status: ProbeStatus::NotRequested,
        executable_capabilities: Vec::new(),
        executable_identity: None,
        rtk_identity: None,
    };
    let windows_project = ProjectLocation {
        kind: ProjectLocationKind::Windows,
        path: r"E:\work".to_owned(),
        distro: None,
        windows_path: None,
    };
    assert_eq!(
        wsl_project_path_with(
            &windows_project,
            &probe,
            |distro, path| {
                assert_eq!(distro, "Ubuntu");
                assert_eq!(path, r"E:\work");
                None
            },
            |_, _| true,
        ),
        None
    );
    assert_eq!(
        wsl_project_path_with(
            &windows_project,
            &probe,
            |_, _| Some("/mnt/e/work".to_owned()),
            |distro, path| distro == "Ubuntu" && path == "/mnt/e/work",
        ),
        Some("/mnt/e/work".to_owned())
    );

    let same_wsl_project = ProjectLocation {
        kind: ProjectLocationKind::Wsl,
        path: "/home/test/work".to_owned(),
        distro: Some("Ubuntu".to_owned()),
        windows_path: None,
    };
    assert_eq!(
        wsl_project_path_with(
            &same_wsl_project,
            &probe,
            |_, _| None,
            |distro, path| distro == "Ubuntu" && path == "/home/test/work",
        ),
        Some("/home/test/work".to_owned())
    );

    assert_eq!(
        wsl_project_path_with(&same_wsl_project, &probe, |_, _| None, |_, _| false,),
        None
    );

    let bridged_other_distro_project = ProjectLocation {
        kind: ProjectLocationKind::Wsl,
        path: "/mnt/host/d/work".to_owned(),
        distro: Some("docker-desktop".to_owned()),
        windows_path: Some(r"D:\work".to_owned()),
    };
    assert_eq!(
        wsl_project_path_with(
            &bridged_other_distro_project,
            &probe,
            |distro, path| {
                assert_eq!(distro, "Ubuntu");
                assert_eq!(path, r"D:\work");
                Some("/mnt/d/work".to_owned())
            },
            |distro, path| distro == "Ubuntu" && path == "/mnt/d/work",
        ),
        Some("/mnt/d/work".to_owned()),
        "a WSL-origin bridge may cross distros only through a verified Windows-mounted path"
    );

    let mapping = wsl_mapping_arguments_with_user("Ubuntu", None, r"E:\work with spaces\$literal");
    assert_eq!(
        mapping,
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu"),
            OsString::from("--exec"),
            OsString::from("wslpath"),
            OsString::from("-a"),
            OsString::from(r"E:\work with spaces\$literal"),
        ]
    );
    assert_eq!(
        wsl_mapping_arguments_with_user("Ubuntu", Some("luthfi"), r"E:\work"),
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu"),
            OsString::from("-u"),
            OsString::from("luthfi"),
            OsString::from("--exec"),
            OsString::from("wslpath"),
            OsString::from("-a"),
            OsString::from(r"E:\work"),
        ]
    );
}

#[test]
fn provider_resolution_verifies_wsl_to_windows_project_mappings() {
    let windows_project = ProjectLocation {
        kind: ProjectLocationKind::Windows,
        path: r"E:\work with spaces\漢字".to_owned(),
        distro: None,
        windows_path: None,
    };
    assert_eq!(
        windows_project_path_with(
            &windows_project,
            |_, _| None,
            |path| { path == r"E:\work with spaces\漢字" }
        ),
        Some(r"E:\work with spaces\漢字".to_owned())
    );

    let wsl_project = ProjectLocation {
        kind: ProjectLocationKind::Wsl,
        path: "/home/luthfi/work with spaces/漢字".to_owned(),
        distro: Some("Ubuntu".to_owned()),
        windows_path: None,
    };
    assert_eq!(
        windows_project_path_with(
            &wsl_project,
            |distro, path| {
                assert_eq!(distro, "Ubuntu");
                assert_eq!(path, "/home/luthfi/work with spaces/漢字");
                Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
            },
            |path| path.contains("work with spaces"),
        ),
        Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
    );
    assert_eq!(
        windows_project_path_with(
            &wsl_project,
            |_, _| Some(r"\\wsl.localhost\Other\home\luthfi\work".to_owned()),
            |_| true,
        ),
        None,
        "a mapped UNC path must name the source WSL distribution"
    );
    assert_eq!(
        windows_project_path_with(
            &wsl_project,
            |_, _| Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work".to_owned()),
            |_| false,
        ),
        None,
        "a path that Windows cannot read is never executable"
    );

    let arguments = windows_mapping_arguments_with_user(
        "Ubuntu",
        None,
        "/home/luthfi/work with spaces/$literal",
    );
    assert_eq!(
        arguments,
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu"),
            OsString::from("--exec"),
            OsString::from("wslpath"),
            OsString::from("-w"),
            OsString::from("-a"),
            OsString::from("/home/luthfi/work with spaces/$literal"),
        ]
    );
    assert_eq!(
        windows_mapping_arguments_with_user("Ubuntu", Some("luthfi"), "/home/luthfi/work"),
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu"),
            OsString::from("-u"),
            OsString::from("luthfi"),
            OsString::from("--exec"),
            OsString::from("wslpath"),
            OsString::from("-w"),
            OsString::from("-a"),
            OsString::from("/home/luthfi/work"),
        ]
    );
}

#[test]
fn provider_aware_go_routing_uses_only_a_complete_verified_wsl_candidate() {
    let config = default_config();
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "go".to_owned(),
        cache: "miss",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        },
        availability: ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 1,
            inspection_level: InspectionLevel::Identity,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: None,
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
        candidates: vec![ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu-22.04".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            executable_identity: Some(fixture_binary_identity("/usr/local/go/bin/go")),
            rtk: Some("/usr/local/bin/rtk".to_owned()),
            rtk_identity: Some(fixture_binary_identity("/usr/local/bin/rtk")),
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        }],
        recommended: Some(0),
        diagnosis: "fixture: a verified WSL provider is available".to_owned(),
        install: "disabled_in_pd1",
    };
    match provider_dispatch_decision_from_resolution(
        &[OsString::from("go"), OsString::from("version")],
        &config,
        Route::Raw,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
            assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
            assert_eq!(plan.adapter.as_str(), "raw");
            assert!(matches!(
                plan.candidate,
                dispatcher::RouteCandidate::Wsl2 { ref distro, ref cwd, .. }
                    if distro == "Ubuntu-22.04" && cwd == Path::new("/mnt/e/work")
            ));
            assert!(reason.contains("verified project path"));
        }
        _ => panic!("expected verified WSL provider selection"),
    }
}

#[test]
fn provider_aware_go_routing_runs_a_wsl_only_go_binary_without_rtk() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
        _ => None,
    })
    .expect("raw adapter configuration is valid");
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "go".to_owned(),
        cache: "miss",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        },
        availability: ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 1,
            inspection_level: InspectionLevel::Identity,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: None,
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
        candidates: vec![ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            executable_identity: Some(fixture_binary_identity("/usr/local/go/bin/go")),
            rtk: None,
            rtk_identity: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture: Go exists only in WSL".to_owned(),
        }],
        recommended: Some(0),
        diagnosis: "fixture".to_owned(),
        install: "disabled_in_p7",
    };
    assert!(
        has_complete_go_provider(&resolution),
        "a verified WSL raw Go binary is ready and must not trigger setup"
    );
    match provider_dispatch_decision_from_resolution(
        &[OsString::from("go"), OsString::from("version")],
        &config,
        Route::Raw,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
            assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
            assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
            assert!(matches!(
                plan.candidate,
                dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                    if executable == &OsString::from("/usr/local/go/bin/go")
            ));
            assert!(reason.contains("raw output adapter"));
        }
        _ => panic!("expected the WSL-only raw Go provider"),
    }
}

#[test]
fn generic_windows_executable_overrides_an_unavailable_legacy_wsl_route() {
    let project_path = env::current_dir()
        .expect("test project directory exists")
        .to_string_lossy()
        .to_string();
    let resolution = resolve_tool_provider_from_discovery_with_user(
        "nvm",
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: project_path.clone(),
            distro: None,
            windows_path: None,
        },
        ProviderCacheEntry {
            tool: "nvm".to_owned(),
            observed_unix_seconds: 1,
            inspection_level: InspectionLevel::Identity,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: Some(r"C:\\Users\\test\\AppData\\Local\\nvm\\nvm.exe".to_owned()),
                native_rtk: None,
                executable_version: Some("1.2.2".to_owned()),
                version_probe_status: ProbeStatus::Success,
                executable_capabilities: vec!["version".to_owned()],
                executable_identity: Some(fixture_binary_identity(
                    r"C:\\Users\\test\\AppData\\Local\\nvm\\nvm.exe",
                )),
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: vec![WslToolProbe {
                distro: "Ubuntu-RTK-WSL1".to_owned(),
                user: None,
                wsl_version: Some(1),
                dedicated: true,
                installation_id: Some("00000000-0000-0000-0000-000000000001".to_owned()),
                executable: None,
                rtk: None,
                executable_version: None,
                version_probe_status: ProbeStatus::NotRequested,
                executable_capabilities: Vec::new(),
                executable_identity: None,
                rtk_identity: None,
            }],
        },
        "miss",
        None,
    );

    assert_eq!(resolution.candidates.len(), 1);
    assert_eq!(resolution.availability.wsl[0].executable, None);
    match provider_dispatch_decision_from_resolution(
        &[OsString::from("nvm"), OsString::from("ls")],
        &default_config(),
        Route::Wsl1,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan {
            plan, fallbacks, ..
        } => {
            assert!(matches!(
                plan.candidate,
                dispatcher::RouteCandidate::Windows { .. }
            ));
            assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
            assert!(fallbacks.is_empty());
        }
        _ => {
            panic!("an unavailable WSL1 provider must not block generic Windows raw execution")
        }
    }
}
