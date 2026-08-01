use std::ffi::OsString;
use std::path::Path;

use crate::PRODUCT_COMMAND;
use crate::cli_exit::CliExit as ExitCode;
use crate::config::Config;
use crate::routing::calibration::print as print_calibration;
use crate::routing::policy::{import as import_route_policy, load as load_route_policy};
use crate::routing::policy_context_report;

const POLICY_ARGUMENT: &str = "policy";
const CALIBRATION_ARGUMENT: &str = "calibration";

pub(crate) fn command(arguments: &[OsString], config: &Config) -> Option<ExitCode> {
    if !arguments
        .first()
        .is_some_and(|argument| argument == POLICY_ARGUMENT)
    {
        return None;
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "context")
        && arguments.len() == 2
    {
        return Some(
            match serde_json::to_string_pretty(&policy_context_report(config)) {
                Ok(rendered) => {
                    println!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: unable to render policy context: {error}");
                    ExitCode::FAILURE
                }
            },
        );
    }
    if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
        match load_route_policy() {
            Some(policy) => match serde_json::to_string_pretty(&policy) {
                Ok(rendered) => println!("{rendered}"),
                Err(error) => {
                    eprintln!("xuva: unable to render route policy: {error}");
                    return Some(ExitCode::FAILURE);
                }
            },
            None => println!("No local route policy is installed."),
        }
        return Some(ExitCode::SUCCESS);
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "import")
        && arguments.len() == 3
    {
        return Some(
            match import_route_policy(Path::new(&arguments[2]), config) {
                Ok(()) => {
                    println!("Imported local XUVA route policy.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: {error}");
                    ExitCode::FAILURE
                }
            },
        );
    }
    eprintln!(
        "{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} policy [show|context] | policy import <evidence.json>"
    );
    Some(ExitCode::FAILURE)
}

pub(crate) fn calibration(arguments: &[OsString], config: &Config) -> Option<ExitCode> {
    if !arguments
        .first()
        .is_some_and(|argument| argument == CALIBRATION_ARGUMENT)
    {
        return None;
    }
    if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
        return Some(match print_calibration(config.policy_objective) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xuva: {error}");
                ExitCode::FAILURE
            }
        });
    }
    eprintln!("{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} calibration [show]");
    Some(ExitCode::FAILURE)
}
