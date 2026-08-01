use crate::test_support::*;

#[test]
fn decodes_and_parses_redirected_wsl_distribution_output() {
    let text = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
    let utf16 = text
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

    let decoded = decode_wsl_output(&utf16);
    assert_eq!(distro_version_from_list(&decoded, "Ubuntu"), Some(2));
    assert_eq!(
        distro_version_from_list(&decoded, "Ubuntu-RTK-WSL1"),
        Some(1)
    );
    assert_eq!(
        distro_version_from_list(&decoded, "Custom WSL One"),
        Some(1)
    );
    assert_eq!(distro_version_from_list(&decoded, "missing"), None);
}

#[test]
fn provider_discovery_parses_wsl_distro_names_and_versions() {
    let output = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
    assert_eq!(
        parse_wsl_distributions(output),
        vec![
            ("Ubuntu".to_owned(), Some(2)),
            ("Ubuntu-RTK-WSL1".to_owned(), Some(1)),
            ("Custom WSL One".to_owned(), Some(1)),
        ]
    );
    assert!(!is_eligible_wsl_distro("docker-desktop"));
    assert!(!is_eligible_wsl_distro("docker-desktop-data"));
    assert!(is_eligible_wsl_distro("Ubuntu-24.04"));
}

#[test]
fn provider_discovery_classifies_windows_and_wsl_project_paths() {
    let windows = classify_project_path(r"E:\luthfi\project\rtk-wsl");
    assert_eq!(windows.kind, ProjectLocationKind::Windows);
    assert_eq!(windows.distro, None);

    let wsl = classify_project_path(r"\\wsl.localhost\Ubuntu-24.04\home\luthfi\project");
    assert_eq!(wsl.kind, ProjectLocationKind::Wsl);
    assert_eq!(wsl.distro.as_deref(), Some("Ubuntu-24.04"));
    assert_eq!(wsl.path, "/home/luthfi/project");
}

#[test]
fn windows_provider_discovery_recognizes_native_launchable_extensions() {
    assert!(is_windows_launchable_path(r"C:\tools\go.exe"));
    assert!(is_windows_launchable_path(r"C:\tools\npm.cmd"));
    assert!(is_windows_launchable_path(r"C:\tools\gradle.bat"));
    assert!(is_windows_launchable_path(r"C:\tools\legacy.com"));
    assert!(!is_windows_launchable_path(r"C:\tools\npm"));
    assert!(!is_windows_launchable_path(r"C:\tools\npm.ps1"));
    assert_eq!(
        select_windows_executable(vec![
            r"C:\tools\npm".to_owned(),
            r"C:\tools\npm.cmd".to_owned(),
            r"C:\tools\npm.ps1".to_owned(),
        ]),
        Some(r"C:\tools\npm.cmd".to_owned())
    );
    assert_eq!(
        select_windows_executable(vec![
            r"C:\tools\script.ps1".to_owned(),
            r"C:\tools\script.py".to_owned(),
        ]),
        None
    );
}

#[test]
fn provider_cache_uses_a_bounded_freshness_window() {
    let entry = ProviderCacheEntry {
        tool: "go".to_owned(),
        observed_unix_seconds: 100,
        inspection_level: InspectionLevel::Version,
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
    };
    assert!(cache_entry_is_fresh(
        &entry,
        100 + PROVIDER_CACHE_TTL_SECONDS,
        "fixture",
        true
    ));
    assert!(
        !cache_entry_is_fresh(&entry, 100, "changed-path-or-git-revision", true),
        "a changed discovery fingerprint invalidates even a new entry"
    );
    assert!(!cache_entry_is_fresh(
        &entry,
        101 + PROVIDER_CACHE_TTL_SECONDS,
        "fixture",
        true
    ));
    let mut identity_only = entry.clone();
    identity_only.inspection_level = InspectionLevel::Identity;
    assert!(cache_entry_is_fresh(&identity_only, 100, "fixture", false));
    assert!(
        !cache_entry_is_fresh(&identity_only, 100, "fixture", true),
        "doctor/version verification must upgrade an identity-only cache entry"
    );
}

#[test]
fn version_probe_registry_never_executes_unknown_tools() {
    assert_eq!(version_probe_arguments("git"), Some(&["--version"][..]));
    assert_eq!(version_probe_arguments("go"), Some(&["version"][..]));
    assert_eq!(version_probe_arguments("user-defined-tool"), None);
}

#[test]
fn explicit_windows_executable_paths_bypass_provider_discovery() {
    let fixture = env::temp_dir().join(format!(
        "xuva-explicit-path-{}-{}.cmd",
        std::process::id(),
        unix_seconds()
    ));
    fs::write(&fixture, "@exit /b 0\r\n").expect("explicit fixture is written");
    let arguments = vec![
        fixture.clone().into_os_string(),
        OsString::from("literal argument"),
    ];
    let (plan, reason) = explicit_executable_plan(&arguments, &default_config())
        .expect("explicit path is valid")
        .expect("explicit path creates a plan");
    assert!(matches!(
        plan.candidate,
        dispatcher::RouteCandidate::Windows { .. }
    ));
    assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
    assert_eq!(
        plan.request.arguments,
        vec![OsString::from("literal argument")]
    );
    assert!(reason.contains("explicit Windows"));
    fs::remove_file(fixture).expect("explicit fixture is removed");
}

#[test]
fn provider_cache_fingerprint_changes_with_wsl_extra_path() {
    let default = default_config();
    let configured = Config::from_lookup(|name| match name {
        "XUVA_WSL_EXTRA_PATH" => Some("/tmp/xuva-go/bin".to_owned()),
        _ => None,
    })
    .expect("extra path configuration is valid");
    assert_ne!(
        discovery_context_signature(&default, false),
        discovery_context_signature(&configured, false),
        "changing the executable search overlay must invalidate discovery"
    );
}
