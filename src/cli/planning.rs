use std::ffi::OsString;
use std::time::Instant;

use crate::cli::invocation::InvocationRequest;
use crate::cli_exit::CliExit as ExitCode;
use crate::config::{Config, ExecutionEnvironment, InvocationOrigin, Route};
use crate::diagnostics::trace;
use crate::dispatcher;
use crate::execution::planner::{configured_wsl_backend, static_windows_execution_plan};
use crate::execution::runner::execution_route;
use crate::providers::dispatch::{
    ProviderDispatchDecision, explicit_executable_plan, provider_dispatch_decision,
};
use crate::routing::calibration::load as load_calibration;
use crate::routing::decision::{
    authorized_policy_route, auto_route_for_environment, route_policy_key,
};
use crate::routing::policy::load as load_route_policy;
use crate::routing::{self, CalibrationPlan, RoutePolicyFile, adaptive_context_signature};

pub(crate) struct PolicyResolvedInvocation {
    pub(crate) request: InvocationRequest,
    pub(crate) started: Instant,
    pub(crate) route: Route,
    pub(crate) reason: String,
    pub(crate) policy_route: Option<Route>,
    pub(crate) policy_eligible: bool,
    pub(crate) policy: Option<RoutePolicyFile>,
    pub(crate) adaptive_context: String,
    pub(crate) calibration_eligible: bool,
    pub(crate) calibration: Option<CalibrationPlan>,
    calibration_resolved: bool,
}

pub(crate) struct PlannedInvocation {
    pub(crate) request: InvocationRequest,
    pub(crate) started: Instant,
    pub(crate) route: Route,
    pub(crate) reason: String,
    pub(crate) selected_config: Config,
    pub(crate) selected_adapter: dispatcher::OutputAdapter,
    pub(crate) execution_plan: Option<dispatcher::ExecutionPlan>,
    pub(crate) fallback_execution_plans: Vec<dispatcher::ExecutionPlan>,
    pub(crate) provider_missing: Option<String>,
    pub(crate) calibration: Option<CalibrationPlan>,
}

#[derive(Clone, Debug)]
pub(crate) struct PlanningError {
    pub(crate) message: String,
    pub(crate) exit: ExitCode,
}

pub(crate) fn resolve_policy(request: InvocationRequest) -> PolicyResolvedInvocation {
    let started = Instant::now();
    let policy_eligible =
        request.requested_route == Route::Auto && route_policy_key(&request.arguments).is_some();
    let calibration_eligible = request.config.calibration_enabled
        && request.requested_route == Route::Auto
        && routing::is_calibration_candidate(&request.arguments);
    let adaptive_context = if policy_eligible || calibration_eligible {
        adaptive_context_signature(&request.config)
    } else {
        String::new()
    };
    let policy = policy_eligible.then(load_route_policy).flatten();
    let policy_route = authorized_policy_route(
        &request.arguments,
        policy.as_ref(),
        Some(&adaptive_context),
        request.config.policy_objective,
    );
    let (route, reason) = if request.requested_route == Route::Auto {
        auto_route_for_environment(
            &request.arguments,
            request.current_directory_str(),
            policy.as_ref(),
            Some(&adaptive_context),
            request.environment,
            request.config.policy_objective,
        )
    } else {
        (request.requested_route, "explicit route preference")
    };
    PolicyResolvedInvocation {
        request,
        started,
        route,
        reason: reason.to_owned(),
        policy_route,
        policy_eligible,
        policy,
        adaptive_context,
        calibration_eligible,
        calibration: None,
        calibration_resolved: false,
    }
}

pub(crate) fn optimistic_windows_raw(resolved: &PolicyResolvedInvocation) -> bool {
    matches!(
        resolved.request.config.invocation_origin,
        InvocationOrigin::Windows
    ) && !resolved.request.explain
        && resolved.request.requested_route == Route::Auto
        && resolved.route == Route::Raw
        && ((!resolved.policy_eligible && !resolved.calibration_eligible)
            || resolved.policy_route == Some(Route::Raw))
}

pub(crate) fn complete_calibration(resolved: &mut PolicyResolvedInvocation) {
    if resolved.calibration_resolved {
        return;
    }
    resolved.calibration_resolved = true;
    if !resolved.calibration_eligible || resolved.policy_route.is_some() {
        return;
    }
    let calibration_state = match load_calibration() {
        Ok(state) => Some(state),
        Err(error) => {
            eprintln!("xuva: local calibration state is unavailable: {error}");
            None
        }
    };
    resolved.calibration = match routing::calibration_plan(
        &resolved.request.arguments,
        resolved.request.current_directory_str(),
        resolved.policy.as_ref(),
        calibration_state.as_ref(),
        &resolved.adaptive_context,
        resolved.request.config.policy_objective,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("xuva: local calibration is unavailable: {error}");
            None
        }
    };
    if let Some(plan) = &resolved.calibration {
        resolved.route = plan.route;
        resolved.reason = plan.reason.to_owned();
    }
}

pub(crate) fn build_execution_plan(
    mut resolved: PolicyResolvedInvocation,
) -> Result<PlannedInvocation, PlanningError> {
    complete_calibration(&mut resolved);
    let mut route = resolved.route;
    let mut reason = resolved.reason;
    let selected_config = configured_wsl_backend(&resolved.request.config, route);
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
    let explicit_plan =
        explicit_executable_plan(&resolved.request.arguments, &resolved.request.config).map_err(
            |message| PlanningError {
                message: format!("xuva: {message}"),
                exit: ExitCode::from(127),
            },
        )?;
    if let Some((plan, explicit_reason)) = explicit_plan {
        route = execution_route(&plan.candidate);
        selected_adapter = dispatcher::OutputAdapter::Raw;
        execution_plan = Some(plan);
        reason = explicit_reason;
    } else if resolved.request.requested_route == Route::Auto
        && resolved.request.environment == ExecutionEnvironment::Adaptive
    {
        trace(format!(
            "adaptive provider planning for {}",
            resolved.request.command.family(&resolved.request.arguments)
        ));
        match provider_dispatch_decision(
            &resolved.request.arguments,
            &resolved.request.config,
            route,
        ) {
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
            resolved.request.config.invocation_origin,
            InvocationOrigin::Wsl { .. }
        )
        && matches!(route, Route::Raw | Route::NativeRtk)
    {
        let plan = static_windows_execution_plan(
            &resolved.request.arguments,
            &resolved.request.config,
            route,
        )
        .map_err(|message| PlanningError {
            message: format!("xuva: {message}"),
            exit: ExitCode::from(127),
        })?;
        selected_adapter = plan.adapter.clone();
        execution_plan = Some(plan);
        reason = "WSL-origin Windows route requires an isolated execution plan".to_owned();
    }
    Ok(PlannedInvocation {
        request: resolved.request,
        started: resolved.started,
        route,
        reason,
        selected_config,
        selected_adapter,
        execution_plan,
        fallback_execution_plans,
        provider_missing,
        calibration: resolved.calibration,
    })
}

impl PlannedInvocation {
    pub(crate) fn print_explanation(&self) -> ExitCode {
        println!("route={}", self.route.as_str());
        println!("output_adapter={}", self.selected_adapter.as_str());
        println!("reason={}", self.reason);
        println!(
            "command_family={}",
            self.request.command.family(&self.request.arguments)
        );
        if let Some(plan) = &self.execution_plan {
            let provider = match &plan.candidate {
                dispatcher::RouteCandidate::Windows { executable, .. }
                | dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
            };
            println!("provider={}", provider.to_string_lossy());
        }
        if self.provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::invocation;

    #[test]
    fn explicit_raw_invocation_can_be_planned_without_execution() {
        let config = Config::from_lookup(|_| None).expect("default config is valid");
        let request = invocation::parse(
            vec![
                OsString::from("--route"),
                OsString::from("raw"),
                OsString::from("git"),
                OsString::from("status"),
            ],
            &config,
        )
        .expect("invocation parses");
        let resolved = resolve_policy(request);
        assert_eq!(resolved.route, Route::Raw);
        let planned = build_execution_plan(resolved).expect("invocation plans");
        assert_eq!(planned.route, Route::Raw);
        assert!(planned.execution_plan.is_none());
        assert!(planned.fallback_execution_plans.is_empty());
        assert!(planned.provider_missing.is_none());
    }

    #[test]
    fn disabled_calibration_never_loads_or_plans_local_evidence() {
        let config =
            Config::from_lookup(|name| (name == "XUVA_CALIBRATION").then(|| "off".to_owned()))
                .expect("calibration can be disabled");
        let request = invocation::parse(
            vec![
                OsString::from("git"),
                OsString::from("status"),
                OsString::from("--short"),
            ],
            &config,
        )
        .expect("invocation parses");

        let mut resolved = resolve_policy(request);
        assert!(!resolved.calibration_eligible);
        complete_calibration(&mut resolved);
        assert!(resolved.calibration.is_none());
    }
}
