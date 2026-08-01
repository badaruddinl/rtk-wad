use std::ffi::OsString;
use std::time::Instant;

use crate::cli_exit::CliExit as ExitCode;
use crate::config::{Config, Route};
use crate::execution::planner::{
    execution_plan_for_explicit_provider_candidate, first_compatible_provider_plan,
};
use crate::execution::runner::{begin_invocation_metrics, execution_route, run_execution_plan};
use crate::providers::commands::is_safe_provider_tool_name;
use crate::providers::resolution::resolve_tool_provider;
use crate::wsl::cancellation::console;

pub(crate) fn provider_exec_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(2).and_then(|argument| argument.to_str()) else {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let separator = arguments.iter().position(|argument| argument == "--");
    let Some(separator) = separator else {
        eprintln!("xuva: provider execution requires `--` before tool arguments");
        return ExitCode::FAILURE;
    };
    if separator < 3 {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    let options = &arguments[3..separator];
    let candidate_index = match options {
        [] => None,
        [flag, index] if flag == "--candidate" => index.to_string_lossy().parse::<usize>().ok(),
        _ => None,
    };
    if !options.is_empty() && candidate_index.is_none() {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    // Execution is explicit and must not reuse a provider identity discovered
    // under a previous RTK path or tool installation state.
    let resolution = resolve_tool_provider(tool, config, true);
    let forwarded = &arguments[separator + 1..];
    let (index, candidate, plan) = if let Some(index) = candidate_index {
        let Some(candidate) = resolution.candidates.get(index) else {
            eprintln!("xuva: provider candidate {index} does not exist; run `xuva resolve {tool}`");
            return ExitCode::FAILURE;
        };
        if !candidate.usable {
            eprintln!(
                "xuva: provider candidate {index} is not verified: {}",
                candidate.reason
            );
            return ExitCode::from(127);
        }
        match execution_plan_for_explicit_provider_candidate(tool, forwarded, config, candidate) {
            Ok(plan) => (index, candidate, plan),
            Err(error) => {
                eprintln!(
                    "xuva: provider candidate {index} cannot produce an execution plan: {error}"
                );
                return ExitCode::from(127);
            }
        }
    } else {
        let selected =
            first_compatible_provider_plan(tool, forwarded, config, &resolution.candidates);
        let Some(selected) = selected else {
            eprintln!(
                "xuva: no verified provider supports the requested output adapter and command semantics; run `xuva resolve {tool}` for details"
            );
            return ExitCode::from(127);
        };
        selected
    };
    let route = execution_route(&plan.candidate);
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let metrics = begin_invocation_metrics(config, &plan.adapter);
    let result = run_execution_plan(&plan, config, metrics.as_ref());
    if needs_console_handler {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    if let Some(metrics) = metrics {
        let command_family = format!(
            "provider:{}:{}",
            candidate.host.as_str(),
            plan.adapter.as_str()
        );
        if let Err(error) = metrics.finish(
            route.as_str(),
            &command_family,
            started.elapsed(),
            exit_code,
        ) {
            eprintln!("xuva: metrics were not recorded: {error}");
        }
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!("xuva: unable to start provider candidate {index}: {error}");
            ExitCode::FAILURE
        }
    }
}
