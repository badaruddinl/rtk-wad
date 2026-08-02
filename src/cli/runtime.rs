use std::env;
use std::ffi::OsString;

use crate::cli;
use crate::cli::info::print_adapter_info;
use crate::cli::{
    is_verbose_version_command, is_version_command, print_command_surface, print_verbose_version,
};
use crate::cli_exit::CliExit as ExitCode;
use crate::config::Config;
use crate::execution::provider_command::provider_exec_command;
use crate::metrics::XuvaMetrics;
use crate::providers::commands::{provider_command, provider_scan_command};
use crate::setup::setup_command;
use crate::{PRODUCT_COMMAND, agent, lifecycle, self_update};

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
    if let Some(exit) = dispatch_builtin(&arguments, config) {
        return exit;
    }
    let request = match crate::cli::invocation::parse(arguments, config) {
        Ok(request) => request,
        Err(error) => return report_error(error.message, error.exit),
    };
    if let Some(exit) = crate::cli::execution::direct_windows_fast_path(&request) {
        return exit;
    }
    let mut resolved = crate::cli::planning::resolve_policy(request);
    if crate::cli::planning::optimistic_windows_raw(&resolved) {
        match crate::cli::execution::optimistic_windows_raw(&resolved) {
            Ok(Some(exit)) => return exit,
            Ok(None) => {}
            Err(error) => return report_error(error.message, error.exit),
        }
    }
    crate::cli::planning::complete_calibration(&mut resolved);
    let planned = match crate::cli::planning::build_execution_plan(resolved) {
        Ok(planned) => planned,
        Err(error) => return report_error(error.message, error.exit),
    };
    if planned.request.explain {
        return planned.print_explanation();
    }
    crate::cli::execution::execute(planned)
}

fn report_error(message: String, exit: ExitCode) -> ExitCode {
    eprintln!("{message}");
    exit
}

fn dispatch_builtin(arguments: &[OsString], config: &Config) -> Option<ExitCode> {
    if is_verbose_version_command(arguments) {
        print_verbose_version();
        return Some(ExitCode::SUCCESS);
    }
    if is_version_command(arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return Some(ExitCode::SUCCESS);
    }
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some(HELP_ARGUMENT | "help" | "-h"))
    {
        cli::print_help();
        return Some(ExitCode::SUCCESS);
    }
    if let Some(result) = lifecycle::command(arguments) {
        return Some(result);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SELF_UPDATE_ARGUMENT)
    {
        return Some(self_update::command(arguments));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == AGENT_ARGUMENT)
    {
        return Some(agent::command(arguments, &config.native_rtk_path));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SURFACE_ARGUMENT)
    {
        return Some(print_command_surface(arguments));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == PROVIDER_ARGUMENT)
    {
        if arguments.get(1).is_some_and(|argument| argument == "exec") {
            return Some(provider_exec_command(arguments, config));
        }
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return Some(ExitCode::FAILURE);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == RESOLVE_ARGUMENT)
    {
        return Some(provider_command(arguments, config, false));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == WHICH_ARGUMENT)
    {
        let mut resolve_arguments = arguments.to_vec();
        resolve_arguments[0] = OsString::from(RESOLVE_ARGUMENT);
        return Some(provider_command(&resolve_arguments, config, false));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return Some(provider_command(arguments, config, true));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SCAN_ARGUMENT)
    {
        return Some(provider_scan_command(arguments, config));
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SETUP_ARGUMENT)
    {
        return Some(setup_command(arguments, config));
    }
    if let Some(result) = crate::cli::metrics_command::command(arguments) {
        return Some(result);
    }
    if let Some(result) = crate::cli::policy_command::command(arguments, config) {
        return Some(result);
    }
    if let Some(result) = crate::cli::policy_command::calibration(arguments, config) {
        return Some(result);
    }
    if arguments.len() == 1 && arguments[0] == ADAPTER_INFO_ARGUMENT {
        print_adapter_info(config);
        return Some(ExitCode::SUCCESS);
    }
    if arguments.len() == 1 && (arguments[0] == "gain" || arguments[0] == "stats") {
        return Some(match XuvaMetrics::print_gain() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xuva: {error}");
                ExitCode::FAILURE
            }
        });
    }
    None
}
