use std::env;
use std::ffi::OsString;
use std::time::Instant;

use crate::cli;
use crate::cli::info::print_adapter_info;
use crate::cli::{
    is_verbose_version_command, is_version_command, parse_options, print_command_surface,
    print_verbose_version,
};
use crate::cli_exit::CliExit as ExitCode;
use crate::config::{Config, ExecutionEnvironment, InvocationOrigin, Route};
use crate::diagnostics::trace;
use crate::execution::planner::{
    configured_wsl_backend, is_shell_operator_command, static_windows_execution_plan,
};
use crate::execution::provider_command::provider_exec_command;
use crate::execution::runner::{
    begin_invocation_metrics, execution_route, run_execution_plan, run_native_rtk, run_wsl_route,
};
use crate::metrics::{TokenTotals, XuvaMetrics};
use crate::providers::commands::{provider_command, provider_scan_command};
use crate::providers::dispatch::{
    ProviderDispatchDecision, explicit_executable_plan, provider_dispatch_decision,
};
use crate::routing::calibration::{load as load_calibration, record as record_calibration};
use crate::routing::decision::{
    auto_route_for_environment, command_family, is_verified_read_only_git, route_policy_key,
    should_use_native_git,
};
use crate::routing::policy::load as load_route_policy;
use crate::routing::{adaptive_context_signature, calibration_plan};
use crate::setup::setup_command;
use crate::wsl::cancellation::console;
use crate::{PRODUCT_COMMAND, adapters, agent, dispatcher, lifecycle, routing, self_update};

const ADAPTER_INFO_ARGUMENT: &str = "--adapter-info";
const RESOLVE_ARGUMENT: &str = "resolve";
const DOCTOR_ARGUMENT: &str = "doctor";
const WHICH_ARGUMENT: &str = "which";
const SCAN_ARGUMENT: &str = "scan";
const PROVIDER_ARGUMENT: &str = "provider";
const SURFACE_ARGUMENT: &str = "surface";
const SETUP_ARGUMENT: &str = "setup";
const AGENT_ARGUMENT: &str = "agent";
const HELP_ARGUMENT: &str = "--help";
const SELF_UPDATE_ARGUMENT: &str = "self-update";

pub(crate) fn run_cli(arguments: Vec<OsString>, config: &Config) -> ExitCode {
    if is_verbose_version_command(&arguments) {
        print_verbose_version();
        return ExitCode::SUCCESS;
    }
    if is_version_command(&arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some(HELP_ARGUMENT | "help" | "-h"))
    {
        cli::print_help();
        return ExitCode::SUCCESS;
    }
    if let Some(result) = lifecycle::command(&arguments) {
        return result;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SELF_UPDATE_ARGUMENT)
    {
        return self_update::command(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == AGENT_ARGUMENT)
    {
        return agent::command(&arguments, &config.native_rtk_path);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SURFACE_ARGUMENT)
    {
        return print_command_surface(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == PROVIDER_ARGUMENT)
    {
        if arguments.get(1).is_some_and(|argument| argument == "exec") {
            return provider_exec_command(&arguments, config);
        }
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == RESOLVE_ARGUMENT)
    {
        return provider_command(&arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == WHICH_ARGUMENT)
    {
        let mut resolve_arguments = arguments.clone();
        resolve_arguments[0] = OsString::from(RESOLVE_ARGUMENT);
        return provider_command(&resolve_arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return provider_command(&arguments, config, true);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SCAN_ARGUMENT)
    {
        return provider_scan_command(&arguments, config);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SETUP_ARGUMENT)
    {
        return setup_command(&arguments, config);
    }
    if let Some(result) = crate::cli::policy_command::command(&arguments, config) {
        return result;
    }
    if let Some(result) = crate::cli::policy_command::calibration(&arguments, config) {
        return result;
    }
    if arguments.len() == 1 && arguments[0] == ADAPTER_INFO_ARGUMENT {
        print_adapter_info(config);
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && (arguments[0] == "gain" || arguments[0] == "stats") {
        return match XuvaMetrics::print_gain() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xuva: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let (arguments, requested_route, environment, explain) =
        match parse_options(arguments, config.route_preference, config.environment) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::FAILURE;
            }
        };
    if arguments.is_empty() {
        eprintln!(
            "{PRODUCT_COMMAND}: no command supplied; run `{PRODUCT_COMMAND} --help` for usage"
        );
        return ExitCode::FAILURE;
    }
    if is_shell_operator_command(&arguments) {
        eprintln!(
            "xuva: `{}` is shell syntax, not an executable; let PowerShell, cmd, or a POSIX shell own the pipeline and invoke XUVA only for command argv",
            arguments[0].to_string_lossy()
        );
        return ExitCode::from(2);
    }
    let mut invocation_config = config.clone();
    invocation_config.environment = environment;
    let current_directory = env::current_dir().ok();
    let same_host_raw_fast_path = matches!(
        invocation_config.invocation_origin,
        InvocationOrigin::Windows
    ) && !explain
        && (requested_route == Route::Raw
            || (requested_route == Route::Auto
                && !is_verified_read_only_git(&arguments)
                && should_use_native_git(
                    &arguments,
                    &invocation_config,
                    current_directory.as_deref().and_then(|path| path.to_str()),
                )));
    if same_host_raw_fast_path {
        return match adapters::windows::run(&arguments) {
            Ok(status) => ExitCode::from_status(status),
            Err(error) => {
                eprintln!("xuva: unable to start Windows raw command: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let started = Instant::now();
    let policy_eligible = requested_route == Route::Auto && route_policy_key(&arguments).is_some();
    let calibration_eligible =
        requested_route == Route::Auto && routing::is_calibration_candidate(&arguments);
    let adaptive_context = if policy_eligible || calibration_eligible {
        adaptive_context_signature(&invocation_config)
    } else {
        String::new()
    };
    let policy = policy_eligible.then(load_route_policy).flatten();
    let (initial_route, initial_reason) = if requested_route == Route::Auto {
        auto_route_for_environment(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            Some(&adaptive_context),
            environment,
            invocation_config.policy_objective,
        )
    } else {
        (requested_route, "explicit route preference")
    };
    let optimistic_same_host_raw = matches!(
        invocation_config.invocation_origin,
        InvocationOrigin::Windows
    ) && !explain
        && requested_route == Route::Auto
        && initial_route == Route::Raw
        && !policy_eligible
        && !calibration_eligible;
    if optimistic_same_host_raw {
        match adapters::windows::run(&arguments) {
            Ok(status) => return ExitCode::from_status(status),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                trace("native command was not found; continuing with provider resolution");
            }
            Err(error) => {
                eprintln!("xuva: unable to start Windows raw command: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    let mut route = initial_route;
    let mut reason = initial_reason.to_owned();
    let calibration = if calibration_eligible {
        let calibration_state = match load_calibration() {
            Ok(state) => Some(state),
            Err(error) => {
                eprintln!("xuva: local calibration state is unavailable: {error}");
                None
            }
        };
        match calibration_plan(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            calibration_state.as_ref(),
            &adaptive_context,
            invocation_config.policy_objective,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("xuva: local calibration is unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    if let Some(plan) = &calibration {
        route = plan.route;
        reason = plan.reason.to_owned();
    }
    let selected_config = configured_wsl_backend(&invocation_config, route);
    let mut provider_missing = None;
    let mut execution_plan = None;
    let mut fallback_execution_plans = Vec::new();
    let mut selected_adapter = match route {
        Route::Raw => dispatcher::OutputAdapter::Raw,
        Route::NativeRtk | Route::Wsl1 | Route::Wsl2 | Route::Auto => {
            dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(&selected_config.native_rtk_path),
            }
        }
    };
    let explicit_plan = match explicit_executable_plan(&arguments, &invocation_config) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("xuva: {error}");
            return ExitCode::from(127);
        }
    };
    if let Some((plan, explicit_reason)) = explicit_plan {
        route = execution_route(&plan.candidate);
        selected_adapter = dispatcher::OutputAdapter::Raw;
        execution_plan = Some(plan);
        reason = explicit_reason;
    } else if requested_route == Route::Auto && environment == ExecutionEnvironment::Adaptive {
        trace(format!(
            "adaptive provider planning for {}",
            command_family(&arguments)
        ));
        match provider_dispatch_decision(&arguments, &invocation_config, route) {
            ProviderDispatchDecision::KeepStaticRoute => {}
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason: provider_reason,
            } => {
                route = execution_route(&plan.candidate);
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(*plan);
                fallback_execution_plans = fallbacks;
                reason = provider_reason;
            }
            ProviderDispatchDecision::Missing {
                reason: missing_reason,
            } => {
                provider_missing = Some(missing_reason.clone());
                reason = missing_reason;
            }
        }
    }
    if execution_plan.is_none()
        && matches!(
            invocation_config.invocation_origin,
            InvocationOrigin::Wsl { .. }
        )
        && matches!(route, Route::Raw | Route::NativeRtk)
    {
        match static_windows_execution_plan(&arguments, &invocation_config, route) {
            Ok(plan) => {
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(plan);
                reason = "WSL-origin Windows route requires an isolated execution plan".to_owned();
            }
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::from(127);
            }
        }
    }
    if explain {
        println!("route={}", route.as_str());
        println!("output_adapter={}", selected_adapter.as_str());
        println!("reason={reason}");
        println!("command_family={}", command_family(&arguments));
        if let Some(plan) = &execution_plan {
            let provider = match &plan.candidate {
                dispatcher::RouteCandidate::Windows { executable, .. }
                | dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
            };
            println!("provider={}", provider.to_string_lossy());
        }
        return if provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        };
    }
    if let Some(reason) = provider_missing {
        eprintln!("xuva: {reason}");
        return ExitCode::from(127);
    }
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    let mut console_installed = false;
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    } else if needs_console_handler {
        console_installed = true;
    }
    let metrics = begin_invocation_metrics(&invocation_config, &selected_adapter);
    let mut executed_route = route;
    let result = if let Some(plan) = execution_plan.as_ref() {
        let mut result = run_execution_plan(plan, &invocation_config, metrics.as_ref());
        for fallback in &fallback_execution_plans {
            if !result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                break;
            }
            let fallback_route = execution_route(&fallback.candidate);
            trace(format!(
                "selected provider executable was unavailable before child start; retrying {} candidate",
                fallback_route.as_str()
            ));
            if matches!(fallback_route, Route::Wsl1 | Route::Wsl2) && !console_installed {
                if !console::install() {
                    eprintln!(
                        "xuva: unable to register the Windows console cancellation handler for provider fallback"
                    );
                    return ExitCode::FAILURE;
                }
                console_installed = true;
            }
            executed_route = fallback_route;
            result = run_execution_plan(fallback, &invocation_config, metrics.as_ref());
        }
        result
    } else {
        match route {
            Route::Raw => adapters::windows::run(&arguments),
            Route::NativeRtk => {
                match run_native_rtk(&arguments, &selected_config, metrics.as_ref()) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && requested_route == Route::Auto
                            && environment == ExecutionEnvironment::Adaptive =>
                    {
                        trace(
                            "native RTK was not found; falling back to isolated WSL1 before any child started",
                        );
                        if !console_installed {
                            if !console::install() {
                                eprintln!(
                                    "xuva: unable to register the Windows console cancellation handler for WSL fallback"
                                );
                                return ExitCode::FAILURE;
                            }
                            console_installed = true;
                        }
                        executed_route = Route::Wsl1;
                        let fallback_config =
                            configured_wsl_backend(&invocation_config, Route::Wsl1);
                        run_wsl_route(
                            arguments.clone(),
                            &fallback_config,
                            Route::Wsl1,
                            metrics.as_ref(),
                        )
                    }
                    result => result,
                }
            }
            Route::Wsl1 | Route::Wsl2 => {
                run_wsl_route(arguments.clone(), &selected_config, route, metrics.as_ref())
            }
            Route::Auto => unreachable!("auto route is resolved before execution"),
        }
    };
    if console_installed {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    let elapsed = started.elapsed();
    let totals = if let Some(metrics) = metrics {
        match metrics.finish(
            executed_route.as_str(),
            command_family(&arguments),
            elapsed,
            exit_code,
        ) {
            Ok(totals) => totals,
            Err(error) => {
                eprintln!("xuva: metrics were not recorded: {error}");
                TokenTotals::default()
            }
        }
    } else {
        TokenTotals::default()
    };
    if invocation_config.metrics_enabled
        && let Some(plan) = &calibration
        && let Err(error) = record_calibration(plan, executed_route, elapsed, exit_code, totals)
    {
        eprintln!("xuva: local calibration was not recorded: {error}");
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!(
                "xuva: unable to start {} route: {error}",
                executed_route.as_str()
            );
            ExitCode::FAILURE
        }
    }
}
