use crate::test_support::*;

#[test]
fn routes_windows_worktree_git_to_native_git_by_default() {
    assert!(should_use_native_git(
        &[OsString::from("git"), OsString::from("status")],
        &default_config(),
        Some(r"E:\luthfi\project\flowpeek"),
    ));
}

#[test]
fn keeps_explicit_wsl_git_paths_and_wsl_mode_in_wsl() {
    assert!(!should_use_native_git(
        &[
            OsString::from("git"),
            OsString::from("-C"),
            OsString::from("/mnt/e/project"),
            OsString::from("status")
        ],
        &default_config(),
        Some(r"E:\luthfi\project\flowpeek"),
    ));
    let config = Config::from_lookup(|name| match name {
        "XUVA_WSL_GIT_MODE" => Some("wsl".to_owned()),
        _ => None,
    })
    .expect("WSL Git mode is valid");
    assert!(!should_use_native_git(
        &[OsString::from("git"), OsString::from("status")],
        &config,
        Some(r"E:\luthfi\project\flowpeek"),
    ));
}

#[test]
fn validates_git_mode() {
    let invalid = Config::from_lookup(|name| match name {
        "XUVA_WSL_GIT_MODE" => Some("other".to_owned()),
        _ => None,
    });
    assert!(invalid.is_err());
}

#[test]
fn explicit_wsl1_backend_selects_the_isolated_distro_without_affecting_default_xuva() {
    let default = default_config();
    assert_eq!(default.backend, WslBackend::Auto);
    assert_eq!(default.distro, DEFAULT_DISTRO);

    let wsl1 = Config::from_lookup(|name| match name {
        "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
        _ => None,
    })
    .expect("explicit WSL1 configuration is valid");
    assert_eq!(wsl1.backend, WslBackend::Wsl1);
    assert_eq!(wsl1.distro, DEFAULT_WSL1_DISTRO);
}

#[test]
fn explicit_backend_and_distro_select_the_xuva_wsl_provider() {
    let config = Config::from_lookup(|name| match name {
        "XUVA_WSL_BACKEND" => Some("wsl2".to_owned()),
        "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
        _ => None,
    })
    .expect("explicit backend configuration is valid");
    assert_eq!(config.backend, WslBackend::Wsl2);
    assert_eq!(config.distro, "Ubuntu-24.04");

    let invalid = Config::from_lookup(|name| match name {
        "XUVA_WSL_BACKEND" => Some("legacy".to_owned()),
        _ => None,
    });
    assert!(invalid.is_err());
}

#[test]
fn canonical_xuva_configuration_is_adaptive_by_default() {
    let xuva = default_config();
    assert_eq!(xuva.profile, ExecutableProfile::Xuva);
    assert_eq!(xuva.backend, WslBackend::Auto);
    assert_eq!(xuva.route_preference, Route::Auto);
    assert!(!xuva.metrics_enabled);

    for enabled in ["local", "on"] {
        let metrics =
            Config::from_lookup(|name| (name == "XUVA_METRICS").then(|| enabled.to_owned()))
                .expect("metrics can be enabled explicitly");
        assert!(metrics.metrics_enabled);
    }
    assert!(
        Config::from_lookup(|name| { (name == "XUVA_METRICS").then(|| "remote".to_owned()) })
            .is_err()
    );
}

#[test]
fn embedded_command_surface_is_complete_and_non_overlapping() {
    let report = command_surface_report();
    assert_eq!(report.schema_version, 2);
    assert_eq!(report.adapter.name, "rtk");
    assert_eq!(report.adapter.version, "0.43.0");
    assert_eq!(report.adapter.protocol_version, 1);
    assert_eq!(report.upstream_command_count, 69);
    let names = report
        .commands
        .iter()
        .map(|row| row.command.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(names.len(), report.upstream_command_count);
    assert!(
        report
            .commands
            .iter()
            .all(|row| row.classification != CommandSurface::Unknown)
    );
    assert_eq!(command_surface("git"), CommandSurface::NativeStructured);
    assert_eq!(command_surface("go"), CommandSurface::RawNative);
    assert_eq!(command_surface("proxy"), CommandSurface::Wsl1Conservative);
    assert_eq!(command_surface("gain"), CommandSurface::CoreInternal);
}

#[test]
fn adapter_only_rtk_commands_never_enter_generic_provider_resolution() {
    let config = default_config();
    for command in ["smart", "proxy", "rewrite", "hook"] {
        assert!(is_adapter_only_rtk_command(command));
        let arguments = [OsString::from(command), OsString::from("literal-argument")];
        assert!(
            matches!(
                provider_dispatch_decision(&arguments, &config, Route::Wsl1),
                ProviderDispatchDecision::KeepStaticRoute
            ),
            "{command} must remain an adapter-owned RTK command"
        );
    }
    assert!(is_rtk_meta_command("wc"));
    assert!(requires_raw_posix_provider("wc"));
    assert!(!is_adapter_only_rtk_command("wc"));
}

#[test]
fn xuva_auto_route_keeps_mutations_raw_and_read_only_commands_structured() {
    let mutation = vec![
        OsString::from("git"),
        OsString::from("commit"),
        OsString::from("-m"),
    ];
    assert_eq!(auto_route(&mutation, Some(r"E:\work"), None).0, Route::Raw);

    let clone = vec![
        OsString::from("git"),
        OsString::from("clone"),
        OsString::from("https://example.invalid/repo"),
    ];
    assert_eq!(auto_route(&clone, Some(r"E:\work"), None).0, Route::Raw);

    let read_only = vec![
        OsString::from("git"),
        OsString::from("log"),
        OsString::from("-1"),
    ];
    assert_eq!(
        auto_route(&read_only, Some(r"E:\work"), None).0,
        Route::NativeRtk
    );

    let cargo = vec![
        OsString::from("cargo"),
        OsString::from("check"),
        OsString::from("--version"),
    ];
    assert_eq!(
        auto_route(&cargo, Some(r"E:\work"), None).0,
        Route::NativeRtk
    );

    assert_eq!(
        auto_route(&[OsString::from("npm")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("npx")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("pnpm")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("go")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("dotnet")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("dart")], Some(r"E:\work"), None).0,
        Route::Raw
    );
    assert_eq!(
        auto_route(&[OsString::from("flutter")], Some(r"E:\work"), None).0,
        Route::Raw
    );

    let literal = vec![
        OsString::from("proxy"),
        OsString::from("/usr/bin/printf"),
        OsString::from("$HOME; &"),
    ];
    assert_eq!(auto_route(&literal, Some(r"E:\work"), None).0, Route::Wsl1);
}

#[test]
fn path_shaped_patterns_do_not_force_linux_execution() {
    let slash_pattern = vec![
        OsString::from("rg"),
        OsString::from("/api/"),
        OsString::from("src"),
    ];
    assert_eq!(
        auto_route(&slash_pattern, Some(r"E:\work"), None).0,
        Route::NativeRtk
    );

    let revision = vec![
        OsString::from("git"),
        OsString::from("show"),
        OsString::from("/release/"),
    ];
    assert_eq!(
        auto_route(&revision, Some(r"E:\work"), None).0,
        Route::NativeRtk
    );
}

#[test]
fn policy_uses_measured_savings_without_permitting_git_mutations() {
    let context = adaptive_context_signature(&default_config());
    let policy = RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: adapter_contract_id(),
        context_signature: context.clone(),
        evidence: vec![
            RoutePolicyEvidence {
                key: "git:status".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 20.0,
                token_savings_percent: 0.0,
                sample_count: 5,
            },
            RoutePolicyEvidence {
                key: "rg".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 30.0,
                token_savings_percent: 80.0,
                sample_count: 5,
            },
            RoutePolicyEvidence {
                key: "cargo:check".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 30.0,
                token_savings_percent: 0.0,
                sample_count: 5,
            },
            RoutePolicyEvidence {
                key: "npm:run-list".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 30.0,
                token_savings_percent: 80.0,
                sample_count: 5,
            },
            RoutePolicyEvidence {
                key: "go:test-all".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 30.0,
                token_savings_percent: 80.0,
                sample_count: 5,
            },
        ],
    };
    assert_eq!(
        auto_route_with_context(
            &[OsString::from("git"), OsString::from("status")],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::Raw
    );
    assert_eq!(
        auto_route_with_context(
            &[
                OsString::from("rg"),
                OsString::from("needle"),
                OsString::from("/mnt/e/work")
            ],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::Wsl1,
        "Linux paths retain precedence over an otherwise valid Windows policy"
    );
    assert_eq!(
        auto_route_with_context(
            &[OsString::from("rg"), OsString::from("needle")],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::NativeRtk
    );
    assert_eq!(
        auto_route_with_context(
            &[OsString::from("cargo"), OsString::from("check")],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::Raw
    );
    assert_eq!(
        auto_route_with_context(
            &[OsString::from("npm"), OsString::from("run")],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::NativeRtk
    );
    assert_eq!(
        auto_route_with_context(
            &[
                OsString::from("go"),
                OsString::from("test"),
                OsString::from("./...")
            ],
            Some(r"E:\work"),
            Some(&policy),
            Some(&context),
            PolicyObjective::Balanced,
        )
        .0,
        Route::NativeRtk
    );
    assert_eq!(
        auto_route(
            &[OsString::from("go"), OsString::from("test")],
            Some(r"E:\work"),
            Some(&policy)
        )
        .0,
        Route::Raw
    );
    assert_eq!(
        auto_route(
            &[
                OsString::from("npm"),
                OsString::from("run"),
                OsString::from("test")
            ],
            Some(r"E:\work"),
            Some(&policy)
        )
        .0,
        Route::Raw
    );
    assert_eq!(
        auto_route(
            &[
                OsString::from("git"),
                OsString::from("clone"),
                OsString::from("url")
            ],
            Some(r"E:\work"),
            Some(&policy)
        )
        .0,
        Route::Raw
    );

    let mutation_policy = RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: adapter_contract_id(),
        context_signature: context.clone(),
        evidence: vec![RoutePolicyEvidence {
            key: "git:commit".to_owned(),
            raw_median_ms: 1.0,
            candidate_median_ms: 100.0,
            token_savings_percent: 0.0,
            sample_count: 5,
        }],
    };
    assert_eq!(
        authorized_policy_route(
            &[OsString::from("git"), OsString::from("commit")],
            Some(&mutation_policy),
            Some(&context),
            PolicyObjective::Balanced,
        ),
        None,
        "benchmark evidence cannot authorize a Git mutation fast path"
    );
}

#[test]
fn adaptive_evidence_is_bound_to_manifest_and_local_adapter_context() {
    let default = default_config();
    let context = adaptive_context_signature(&default);
    let mut different = default.clone();
    different.native_rtk_path = r"C:\tools\other-rtk.exe".to_owned();
    assert_ne!(context, adaptive_context_signature(&different));

    let policy = RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: adapter_contract_id(),
        context_signature: context.clone(),
        evidence: vec![RoutePolicyEvidence {
            key: "rg".to_owned(),
            raw_median_ms: 10.0,
            candidate_median_ms: 20.0,
            token_savings_percent: 0.0,
            sample_count: 5,
        }],
    };
    assert_eq!(
        policy.route_for("rg", &context, PolicyObjective::Balanced),
        Some(Route::Raw)
    );
    assert_eq!(
        policy.route_for("rg", "0123456789abcdef", PolicyObjective::Balanced),
        None
    );
}

#[test]
fn xuva_route_options_are_explicit_and_validate_values() {
    let (arguments, route, environment, explain) = parse_options(
        vec![
            OsString::from("--route"),
            OsString::from("native-rtk"),
            OsString::from("--explain-route"),
            OsString::from("rg"),
        ],
        Route::Auto,
        ExecutionEnvironment::Adaptive,
    )
    .expect("route options are valid");
    assert_eq!(route, Route::NativeRtk);
    assert_eq!(environment, ExecutionEnvironment::Adaptive);
    assert!(explain);
    assert_eq!(arguments, vec![OsString::from("rg")]);
    assert!(
        parse_options(
            vec![OsString::from("--route"), OsString::from("unsafe")],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .is_err()
    );

    let (arguments, route, environment, explain) = parse_options(
        vec![
            OsString::from("--environment"),
            OsString::from("windows-only"),
            OsString::from("pytest"),
        ],
        Route::Auto,
        ExecutionEnvironment::Adaptive,
    )
    .expect("windows-only option is valid");
    assert_eq!(arguments, vec![OsString::from("pytest")]);
    assert_eq!(route, Route::Auto);
    assert_eq!(environment, ExecutionEnvironment::WindowsOnly);
    assert!(!explain);
    assert!(
        parse_options(
            vec![OsString::from("--environment"), OsString::from("hybrid")],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .is_err()
    );
}

#[test]
fn windows_only_routes_external_commands_raw_and_keeps_rtk_meta_native() {
    assert_eq!(
        auto_route_for_environment(
            &[OsString::from("pytest"), OsString::from("-q")],
            Some(r"E:\work"),
            None,
            None,
            ExecutionEnvironment::WindowsOnly,
            PolicyObjective::Balanced,
        )
        .0,
        Route::Raw
    );
    assert_eq!(
        auto_route_for_environment(
            &[OsString::from("init"), OsString::from("-g")],
            Some(r"E:\work"),
            None,
            None,
            ExecutionEnvironment::WindowsOnly,
            PolicyObjective::Balanced,
        )
        .0,
        Route::NativeRtk
    );
    assert_eq!(
        auto_route_for_environment(
            &[
                OsString::from("git"),
                OsString::from("commit"),
                OsString::from("-m"),
                OsString::from("x")
            ],
            Some(r"E:\work"),
            None,
            None,
            ExecutionEnvironment::WindowsOnly,
            PolicyObjective::Balanced,
        )
        .0,
        Route::Raw
    );
}
