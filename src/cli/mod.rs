use serde::{Deserialize, Serialize};
use std::ffi::OsString;

use crate::PRODUCT_COMMAND;
use crate::adapters::rtk::command_surface_report;
use crate::cli_exit::CliExit as ExitCode;
use crate::config::{ExecutionEnvironment, Route};
use crate::providers::cache::PROVIDER_CACHE_SCHEMA_VERSION;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct SetupPlan {
    pub(crate) schema_version: u32,
    pub(crate) tool: String,
    pub(crate) mode: &'static str,
    pub(crate) status: &'static str,
    pub(crate) reason: String,
    pub(crate) proposed_provider: Option<&'static str>,
    pub(crate) proposed_command: Option<Vec<String>>,
    pub(crate) verification_command: Vec<String>,
    pub(crate) apply: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SetupTransaction {
    pub(crate) schema_version: u32,
    pub(crate) tool: String,
    pub(crate) status: String,
    pub(crate) observed_unix_seconds: u64,
    pub(crate) command: Option<Vec<String>>,
    pub(crate) detail: String,
}

pub(crate) fn print_command_surface(arguments: &[OsString]) -> ExitCode {
    if arguments.len() > 2
        || arguments
            .get(1)
            .is_some_and(|argument| argument != "--json")
    {
        print_usage_error("Invalid surface options.", "xuva surface [--json]");
        return ExitCode::FAILURE;
    }
    let report = command_surface_report();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--json")
    {
        return match serde_json::to_string_pretty(&report) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("xuva: unable to render command surface: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("Command surface");
    println!(
        "  Adapter: {} {}",
        report.adapter.name, report.adapter.version
    );
    println!("  Protocol: {}", report.adapter.protocol_version);
    println!("  Commands: {}", report.upstream_command_count);
    println!();
    println!("Commands");
    for row in report.commands {
        println!(
            "  {:<18} {:<14} {}",
            row.command,
            row.classification.as_str(),
            row.default_route
        );
    }
    ExitCode::SUCCESS
}

pub(crate) fn print_help() {
    println!(
        "{PRODUCT_COMMAND} {} — fast, explainable command routing",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("Usage");
    println!("  xuva [--explain-route] <command> [<argv>...]");
    println!();
    println!("Start here");
    println!("  xuva <command> [<argv>...]");
    println!("    Run a command through the verified provider and route.");
    println!("  xuva --explain-route <command> [<argv>...]");
    println!("    Explain the selected provider and route before execution.");
    println!();
    println!("Diagnostics");
    println!("  xuva doctor <tool> [--json] [--refresh]");
    println!("    Verify providers and show an actionable diagnosis.");
    println!("  xuva surface [--json]");
    println!("    List the embedded adapter command contract.");
    println!("  xuva self-update --check");
    println!("    Check for a newer stable release without installing it.");
    println!();
    println!("Lifecycle");
    println!("  xuva install --status");
    println!("  xuva install --recover");
    println!("  xuva rollback");
    println!("  xuva uninstall [--remove-from-path]");
    println!();
    println!("Safety");
    println!("  Shell operators stay with the invoking shell.");
    println!("  XUVA preserves argv and never rebuilds a pipeline.");
}

pub(crate) fn parse_options(
    mut arguments: Vec<OsString>,
    configured: Route,
    configured_environment: ExecutionEnvironment,
) -> Result<(Vec<OsString>, Route, ExecutionEnvironment, bool), String> {
    let mut route = configured;
    let mut environment = configured_environment;
    let mut explain = false;
    loop {
        match arguments.first().and_then(|argument| argument.to_str()) {
            Some("--route") => {
                if arguments.len() < 2 {
                    return Err("--route requires auto, raw, native-rtk, wsl1, or wsl2".to_owned());
                }
                route = Route::parse(&arguments[1].to_string_lossy())?;
                arguments.drain(0..2);
            }
            Some("--environment") => {
                if arguments.len() < 2 {
                    return Err("--environment requires adaptive or windows-only".to_owned());
                }
                environment = ExecutionEnvironment::parse(&arguments[1].to_string_lossy())?;
                arguments.drain(0..2);
            }
            Some("--explain-route") => {
                explain = true;
                arguments.remove(0);
            }
            _ => return Ok((arguments, route, environment, explain)),
        }
    }
}

pub(crate) fn is_version_command(arguments: &[OsString]) -> bool {
    arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--version" | "version" | "-V"))
}

pub(crate) fn is_verbose_version_command(arguments: &[OsString]) -> bool {
    arguments == [OsString::from("--version"), OsString::from("--verbose")]
}

pub(crate) fn print_verbose_version() {
    println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
    println!("commit={}", env!("XUVA_BUILD_COMMIT"));
    println!("target={}", env!("XUVA_BUILD_TARGET"));
    println!("profile={}", env!("XUVA_BUILD_PROFILE"));
    println!("provenance={}", env!("XUVA_BUILD_PROVENANCE"));
    println!("provider_cache_schema={PROVIDER_CACHE_SCHEMA_VERSION}");
}

fn print_usage_error(detail: &str, usage: &str) {
    eprintln!("xuva: {detail}");
    eprintln!("  Usage: {usage}");
    eprintln!("  Try: xuva --help");
}

#[cfg(test)]
mod tests {
    use super::{is_verbose_version_command, is_version_command, parse_options};
    use crate::config::{ExecutionEnvironment, Route};
    use std::ffi::OsString;

    #[test]
    fn options_are_consumed_without_rebuilding_command_arguments() {
        let command = OsString::from("rg");
        let literal = OsString::from("value with spaces");
        let (arguments, route, environment, explain) = parse_options(
            vec![
                OsString::from("--route"),
                OsString::from("raw"),
                OsString::from("--environment"),
                OsString::from("windows-only"),
                OsString::from("--explain-route"),
                command.clone(),
                literal.clone(),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("options are valid");
        assert_eq!(arguments, [command, literal]);
        assert_eq!(route, Route::Raw);
        assert_eq!(environment, ExecutionEnvironment::WindowsOnly);
        assert!(explain);
    }

    #[test]
    fn version_forms_are_bounded_and_explicit() {
        for value in ["--version", "version", "-V"] {
            assert!(is_version_command(&[OsString::from(value)]));
        }
        assert!(is_verbose_version_command(&[
            OsString::from("--version"),
            OsString::from("--verbose"),
        ]));
        assert!(!is_version_command(&[
            OsString::from("--version"),
            OsString::from("extra"),
        ]));
    }
}
pub(crate) mod info;
pub(crate) mod policy_command;
pub(crate) mod runtime;
#[cfg(test)]
mod runtime_tests;
