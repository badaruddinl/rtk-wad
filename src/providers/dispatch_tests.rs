use crate::test_support::*;

#[test]
fn provider_planning_retains_the_next_eligible_route_for_pre_start_fallback() {
    let raw_config = Config::from_lookup(|name| match name {
        "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
        _ => None,
    })
    .expect("raw adapter configuration is valid");
    let candidate = |distro: &str, version, executable: &str| ProviderCandidate {
        host: if version == 1 {
            ProviderHost::Wsl1
        } else {
            ProviderHost::Wsl2
        },
        adapters: vec![AdapterKind::Raw],
        distro: Some(distro.to_owned()),
        wsl_version: Some(version),
        executable: executable.to_owned(),
        rtk: None,
        project_path: Some("/mnt/e/work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "go".to_owned(),
        cache: "miss",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\\work".to_owned(),
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
        candidates: vec![
            candidate("Ubuntu-RTK-WSL1", 1, "/opt/go-wsl1/bin/go"),
            candidate("Ubuntu", 2, "/opt/go-wsl2/bin/go"),
        ],
        recommended: Some(0),
        diagnosis: "fixture".to_owned(),
        install: "disabled_in_p7",
    };

    match provider_dispatch_decision_from_resolution(
        &[OsString::from("go"), OsString::from("version")],
        &raw_config,
        Route::Raw,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan {
            plan, fallbacks, ..
        } => {
            assert_eq!(execution_route(&plan.candidate), Route::Wsl1);
            assert_eq!(fallbacks.len(), 1);
            assert_eq!(execution_route(&fallbacks[0].candidate), Route::Wsl2);
        }
        _ => panic!("the usable WSL2 candidate must remain available as fallback"),
    }
}

#[test]
fn generic_dispatcher_routes_a_wsl_only_cargo_binary_without_rtk() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
        _ => None,
    })
    .expect("raw adapter configuration is valid");
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "cargo".to_owned(),
        cache: "miss",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        },
        availability: ProviderCacheEntry {
            tool: "cargo".to_owned(),
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
            executable: "/home/test/.cargo/bin/cargo".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture: Cargo exists only in WSL".to_owned(),
        }],
        recommended: Some(0),
        diagnosis: "fixture".to_owned(),
        install: "disabled_in_p7",
    };

    match provider_dispatch_decision_from_resolution(
        &[OsString::from("cargo"), OsString::from("--version")],
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
                    if executable == &OsString::from("/home/test/.cargo/bin/cargo")
            ));
            assert!(reason.contains("cargo discovery"));
        }
        _ => panic!("expected the WSL-only raw Cargo provider"),
    }
}

#[test]
fn generic_dispatcher_falls_back_to_verified_windows_raw_when_rtk_is_absent() {
    let resolution = ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: "cargo".to_owned(),
        cache: "miss",
        project: ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        },
        availability: ProviderCacheEntry {
            tool: "cargo".to_owned(),
            observed_unix_seconds: 1,
            inspection_level: InspectionLevel::Identity,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: Some(r"C:\Users\test\.cargo\bin\cargo.exe".to_owned()),
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
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Users\test\.cargo\bin\cargo.exe".to_owned(),
            rtk: None,
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "fixture: Cargo exists on Windows without RTK".to_owned(),
        }],
        recommended: Some(0),
        diagnosis: "fixture".to_owned(),
        install: "disabled_in_p7",
    };

    match provider_dispatch_decision_from_resolution(
        &[OsString::from("cargo"), OsString::from("--version")],
        &default_config(),
        Route::NativeRtk,
        resolution,
    ) {
        ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
            assert!(matches!(
                plan.candidate,
                dispatcher::RouteCandidate::Windows { ref executable, .. }
                    if executable == &OsString::from(r"C:\Users\test\.cargo\bin\cargo.exe")
            ));
            assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
            assert!(reason.contains("on Windows"));
        }
        _ => panic!("expected Windows raw fallback when native RTK is absent"),
    }
}

#[test]
fn generic_dispatcher_discovers_every_safe_executable_name() {
    for tool in [
        "go",
        "cargo",
        "rustc",
        "node",
        "nvm",
        "npm",
        "pnpm",
        "python",
        "python3",
        "pytest",
        "java",
        "gradle",
        "mvn",
        "dotnet",
        "git",
        "tool.name",
        "cargo-next",
    ] {
        assert!(
            is_dispatchable_provider_tool(&[OsString::from(tool)]),
            "{tool}"
        );
    }
    assert!(!is_dispatchable_provider_tool(&[OsString::from("cmd /c")]));
    assert!(!is_dispatchable_provider_tool(&[OsString::from("go;exit")]));
}

#[test]
fn execution_plan_rejects_inconsistent_provider_host_metadata() {
    let candidate = ProviderCandidate {
        host: ProviderHost::Windows,
        adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
        distro: Some("Ubuntu".to_owned()),
        wsl_version: Some(2),
        executable: "/usr/local/go/bin/go".to_owned(),
        rtk: None,
        project_path: Some("/mnt/e/work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    let error = execution_plan_for_provider_candidate(
        "go",
        &[OsString::from("version")],
        &default_config(),
        &candidate,
    )
    .expect_err("host and WSL metadata must not contradict each other");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn requested_rtk_adapter_does_not_silently_downgrade_to_raw() {
    let candidate = ProviderCandidate {
        host: ProviderHost::Wsl2,
        adapters: vec![AdapterKind::Raw],
        distro: Some("Ubuntu".to_owned()),
        wsl_version: Some(2),
        executable: "/usr/local/go/bin/go".to_owned(),
        rtk: None,
        project_path: Some("/mnt/e/work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    assert!(provider_adapter(&candidate, OutputAdapterPreference::Rtk).is_err());

    let adapter_only = ProviderCandidate {
        host: ProviderHost::Windows,
        adapters: vec![AdapterKind::Rtk],
        distro: None,
        wsl_version: None,
        executable: r"C:\tools\rtk.exe".to_owned(),
        rtk: Some(r"C:\tools\rtk.exe".to_owned()),
        project_path: Some(r"E:\work".to_owned()),
        usable: true,
        reason: "fixture".to_owned(),
    };
    assert!(provider_adapter(&adapter_only, OutputAdapterPreference::Raw).is_err());
    assert!(matches!(
        provider_adapter(&adapter_only, OutputAdapterPreference::Auto),
        Ok(dispatcher::OutputAdapter::Rtk { .. })
    ));
}

#[test]
fn provider_aware_go_routing_reports_missing_without_an_install_action() {
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
        candidates: Vec::new(),
        recommended: None,
        diagnosis: "fixture: no provider is available".to_owned(),
        install: "disabled_in_pd1",
    };
    match provider_dispatch_decision_from_resolution(
        &[OsString::from("go"), OsString::from("version")],
        &default_config(),
        Route::Raw,
        resolution,
    ) {
        ProviderDispatchDecision::Missing { reason } => {
            assert!(reason.contains("does not execute shell builtins implicitly"));
            assert!(reason.contains("doctor go"));
        }
        _ => panic!("expected a missing-provider diagnostic"),
    }
}

#[test]
fn cached_windows_go_skips_cross_host_resolution_when_it_is_sufficient() {
    let windows = WindowsToolProbe {
        executable: Some(r"C:\Program Files\Go\bin\go.exe".to_owned()),
        native_rtk: None,
        executable_version: None,
        version_probe_status: ProbeStatus::NotRequested,
        executable_capabilities: Vec::new(),
        executable_identity: None,
        native_rtk_identity: None,
    };
    let windows_project = ProjectLocation {
        kind: ProjectLocationKind::Windows,
        path: r"E:\work".to_owned(),
        distro: None,
        windows_path: None,
    };
    assert!(windows_tool_is_usable(
        "go",
        &windows_project,
        Route::Raw,
        &windows
    ));
    assert!(!windows_tool_is_usable(
        "go",
        &windows_project,
        Route::NativeRtk,
        &windows
    ));
    assert!(
        !windows_tool_is_usable("go", &windows_project, Route::Wsl1, &windows),
        "a conservative WSL fallback must not suppress Windows provider resolution"
    );
    let wsl_project = ProjectLocation {
        kind: ProjectLocationKind::Wsl,
        path: "/home/test/work".to_owned(),
        distro: Some("Ubuntu".to_owned()),
        windows_path: None,
    };
    assert!(!windows_tool_is_usable(
        "go",
        &wsl_project,
        Route::Raw,
        &windows
    ));
    assert!(
        !windows_tool_is_usable("find", &windows_project, Route::Raw, &windows),
        "Windows find.exe must never satisfy POSIX find semantics"
    );
    let mut structured = windows.clone();
    structured.native_rtk = Some(r"C:\Tools\rtk.exe".to_owned());
    assert!(windows_tool_is_usable(
        "go",
        &windows_project,
        Route::NativeRtk,
        &structured
    ));
    assert!(
        windows_tool_is_usable("find", &windows_project, Route::NativeRtk, &structured),
        "Windows RTK find is a structured adapter, not raw find.exe"
    );
}
