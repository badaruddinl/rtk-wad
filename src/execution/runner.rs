use std::ffi::OsString;
use std::process::ExitStatus;

use crate::config::{Config, Route};
use crate::diagnostics::trace;
use crate::execution::planner::provider_execution_config;
use crate::metrics::XuvaMetrics;
use crate::paths::windows_path_to_wsl_path;
use crate::providers::discovery::windows_binary_identity;
use crate::providers::mapping::mapped_windows_project_path;
use crate::providers::model::BinaryIdentity;
use crate::wsl::arguments::{
    WslLaunchMetadata, plan_wsl_arguments_with_metrics, rtk_arguments_with_metrics,
    wsl1_rtk_arguments_with_metrics,
};
use crate::wsl::authorization::{LaunchPermitGuard, require_wsl1_version};
use crate::wsl::cancellation::{cancellation_nonce, console, windows_lock};
use crate::wsl::supervisor::{wait_for_wsl_child, wait_for_wsl1_child};
use crate::{adapters, dispatcher};

pub(crate) fn begin_invocation_metrics(
    config: &Config,
    adapter: &dispatcher::OutputAdapter,
) -> Option<XuvaMetrics> {
    if !config.metrics_enabled {
        return None;
    }
    let result = if matches!(adapter, dispatcher::OutputAdapter::Raw) {
        XuvaMetrics::begin_unmeasured()
    } else {
        XuvaMetrics::begin()
    };
    match result {
        Ok(metrics) => Some(metrics),
        Err(error) => {
            eprintln!("xuva: metrics disabled for this invocation: {error}");
            None
        }
    }
}

pub(crate) fn run_native_rtk(
    arguments: &[OsString],
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    adapters::windows::run_rtk_at(&config.native_rtk_path, arguments, None, metrics)
}

pub(crate) fn execution_route(route: &dispatcher::RouteCandidate) -> Route {
    match route {
        dispatcher::RouteCandidate::Windows { .. } => Route::Raw,
        dispatcher::RouteCandidate::Wsl1 { .. } => Route::Wsl1,
        dispatcher::RouteCandidate::Wsl2 { .. } => Route::Wsl2,
    }
}

pub(crate) fn run_execution_plan(
    plan: &dispatcher::ExecutionPlan,
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    if let dispatcher::RouteCandidate::Windows { executable, .. } = &plan.candidate
        && let Some(expected) = plan.expected_identity.as_ref()
    {
        let selected = match &plan.adapter {
            dispatcher::OutputAdapter::Raw => executable,
            dispatcher::OutputAdapter::Rtk { executable } => executable,
        };
        validate_windows_binary_identity(selected, expected)?;
    }
    let rtk_arguments = || {
        let mut forwarded = Vec::with_capacity(plan.request.arguments.len() + 1);
        forwarded.push(plan.request.executable.clone());
        forwarded.extend(plan.request.arguments.iter().cloned());
        forwarded
    };
    match (&plan.candidate, &plan.adapter) {
        (
            dispatcher::RouteCandidate::Windows { executable, .. },
            dispatcher::OutputAdapter::Raw,
        ) => adapters::windows::run_plan(executable, &plan.request),
        (
            dispatcher::RouteCandidate::Windows { .. },
            dispatcher::OutputAdapter::Rtk { executable },
        ) => adapters::windows::run_rtk_plan(executable, &rtk_arguments(), &plan.request, metrics),
        (
            dispatcher::RouteCandidate::Wsl1 { .. } | dispatcher::RouteCandidate::Wsl2 { .. },
            adapter,
        ) => {
            let selected = provider_execution_config(config, &plan.candidate, adapter)?;
            let forwarded = match adapter {
                dispatcher::OutputAdapter::Raw => plan.request.arguments.clone(),
                dispatcher::OutputAdapter::Rtk { .. } => rtk_arguments(),
            };
            let raw_executable = match &plan.candidate {
                dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
                dispatcher::RouteCandidate::Windows { .. } => {
                    unreachable!("WSL arm has a WSL candidate")
                }
            };
            let executable = match adapter {
                dispatcher::OutputAdapter::Raw => raw_executable,
                dispatcher::OutputAdapter::Rtk { executable } => executable,
            };
            let measured = matches!(adapter, dispatcher::OutputAdapter::Rtk { .. })
                .then_some(metrics)
                .flatten();
            run_wsl_execution_plan(
                executable,
                plan.expected_identity.as_ref().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "WSL execution plan has no captured provider identity",
                    )
                })?,
                &forwarded,
                &plan.request.environment,
                &selected,
                execution_route(&plan.candidate),
                measured,
            )
        }
    }
}

pub(crate) fn validate_windows_binary_identity(
    executable: &OsString,
    expected: &BinaryIdentity,
) -> std::io::Result<()> {
    if executable != &OsString::from(&expected.path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "planned Windows executable does not match its captured identity path",
        ));
    }
    let actual = windows_binary_identity(&expected.path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "provider executable disappeared before launch: {}",
                expected.path
            ),
        )
    })?;
    if &actual != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "provider executable identity changed before launch: {}",
                expected.path
            ),
        ));
    }
    Ok(())
}

pub(crate) fn run_wsl_route(
    arguments: Vec<OsString>,
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 child was spawned",
            ));
        }
        trace("starting identity-gated WSL1 child while holding the global mutex");
        let mut process = adapters::wsl1::process(wsl1_rtk_arguments_with_metrics(
            arguments,
            config,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ));
        let result = process.spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 child completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        adapters::wsl2::process(rtk_arguments_with_metrics(
            arguments,
            config,
            &token,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ))
        .spawn()
        .and_then(|child| {
            trace("spawned cancellation-gated WSL2 proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}

pub(crate) fn run_wsl_execution_plan(
    executable: &OsString,
    expected_identity: &BinaryIdentity,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 plan was spawned",
            ));
        }
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: None,
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
                expected_identity: Some(expected_identity),
            },
        )?;
        trace("starting identity-gated WSL1 plan while holding the global mutex");
        let result = adapters::wsl1::process(command).spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 plan proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 plan completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: Some(&token),
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
                expected_identity: Some(expected_identity),
            },
        )?;
        adapters::wsl2::process(command).spawn().and_then(|child| {
            trace("spawned cancellation-gated WSL2 plan proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}
