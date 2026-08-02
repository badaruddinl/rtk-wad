use std::io;

use crate::adapters;
use crate::cli::invocation::{InvocationError, InvocationRequest};
use crate::cli::planning::{PlannedInvocation, PolicyResolvedInvocation};
use crate::cli_exit::CliExit as ExitCode;
use crate::command::CommandAccess;
use crate::config::{ExecutionEnvironment, InvocationOrigin, Route};
use crate::diagnostics::trace;
use crate::execution::planner::configured_wsl_backend;
use crate::execution::runner::{
    begin_invocation_metrics, execution_route, run_execution_plan, run_native_rtk, run_wsl_route,
};
use crate::metrics::TokenTotals;
use crate::routing::calibration::record as record_calibration;
use crate::routing::decision::should_use_native_git;
use crate::wsl::cancellation::console;

pub(crate) fn direct_windows_fast_path(request: &InvocationRequest) -> Option<ExitCode> {
    let read_only_git = request
        .command
        .git
        .as_ref()
        .is_some_and(|git| git.access == CommandAccess::ReadOnly);
    let eligible = matches!(request.config.invocation_origin, InvocationOrigin::Windows)
        && !request.explain
        && (request.requested_route == Route::Raw
            || (request.requested_route == Route::Auto
                && !read_only_git
                && should_use_native_git(
                    &request.arguments,
                    &request.config,
                    request.current_directory_str(),
                )));
    eligible.then(|| match adapters::windows::run(&request.arguments) {
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!("xuva: unable to start Windows raw command: {error}");
            ExitCode::FAILURE
        }
    })
}

pub(crate) fn optimistic_windows_raw(
    resolved: &PolicyResolvedInvocation,
) -> Result<Option<ExitCode>, InvocationError> {
    match adapters::windows::run(&resolved.request.arguments) {
        Ok(status) => Ok(Some(ExitCode::from_status(status))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            trace("native command was not found; continuing with provider resolution");
            Ok(None)
        }
        Err(error) => Err(InvocationError {
            message: format!("xuva: unable to start Windows raw command: {error}"),
            exit: ExitCode::FAILURE,
        }),
    }
}

pub(crate) fn execute(planned: PlannedInvocation) -> ExitCode {
    if let Some(reason) = &planned.provider_missing {
        eprintln!("xuva: {reason}");
        return ExitCode::from(127);
    }
    let mut console_handler = ConsoleHandler::default();
    if matches!(planned.route, Route::Wsl1 | Route::Wsl2)
        && let Err(exit) = console_handler
            .ensure("xuva: unable to register the Windows console cancellation handler")
    {
        return exit;
    }
    let metrics = begin_invocation_metrics(&planned.request.config, &planned.selected_adapter);
    let mut executed_route = planned.route;
    let result = if let Some(plan) = planned.execution_plan.as_ref() {
        let mut result = run_execution_plan(plan, &planned.request.config, metrics.as_ref());
        for fallback in &planned.fallback_execution_plans {
            if !result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            {
                break;
            }
            let fallback_route = execution_route(&fallback.candidate);
            trace(format!(
                "selected provider executable was unavailable before child start; retrying {} candidate",
                fallback_route.as_str()
            ));
            if matches!(fallback_route, Route::Wsl1 | Route::Wsl2)
                && let Err(exit) = console_handler.ensure(
                    "xuva: unable to register the Windows console cancellation handler for provider fallback",
                )
            {
                return exit;
            }
            executed_route = fallback_route;
            result = run_execution_plan(fallback, &planned.request.config, metrics.as_ref());
        }
        result
    } else {
        match planned.route {
            Route::Raw => adapters::windows::run(&planned.request.arguments),
            Route::NativeRtk => match run_native_rtk(
                &planned.request.arguments,
                &planned.selected_config,
                metrics.as_ref(),
            ) {
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && planned.request.requested_route == Route::Auto
                        && planned.request.environment == ExecutionEnvironment::Adaptive =>
                {
                    trace(
                        "native RTK was not found; falling back to isolated WSL1 before any child started",
                    );
                    if let Err(exit) = console_handler.ensure(
                        "xuva: unable to register the Windows console cancellation handler for WSL fallback",
                    ) {
                        return exit;
                    }
                    executed_route = Route::Wsl1;
                    let fallback_config =
                        configured_wsl_backend(&planned.request.config, Route::Wsl1);
                    run_wsl_route(
                        planned.request.arguments.clone(),
                        &fallback_config,
                        Route::Wsl1,
                        metrics.as_ref(),
                    )
                }
                result => result,
            },
            Route::Wsl1 | Route::Wsl2 => run_wsl_route(
                planned.request.arguments.clone(),
                &planned.selected_config,
                planned.route,
                metrics.as_ref(),
            ),
            Route::Auto => unreachable!("auto route is resolved before execution"),
        }
    };
    drop(console_handler);
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    let elapsed = planned.started.elapsed();
    let totals = if let Some(metrics) = metrics {
        match metrics.finish(
            executed_route.as_str(),
            planned.request.command.family(&planned.request.arguments),
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
    if planned.request.config.metrics_enabled
        && let Some(plan) = &planned.calibration
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

#[derive(Default)]
struct ConsoleHandler {
    installed: bool,
}

impl ConsoleHandler {
    fn ensure(&mut self, failure_message: &str) -> Result<(), ExitCode> {
        if self.installed {
            return Ok(());
        }
        if !console::install() {
            eprintln!("{failure_message}");
            return Err(ExitCode::FAILURE);
        }
        self.installed = true;
        Ok(())
    }
}

impl Drop for ConsoleHandler {
    fn drop(&mut self) {
        if self.installed {
            console::uninstall();
        }
    }
}
