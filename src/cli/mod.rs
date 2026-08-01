use serde::{Deserialize, Serialize};
use std::ffi::OsString;

use crate::PRODUCT_COMMAND;
use crate::adapters::rtk::command_surface_report;
use crate::cli_exit::CliExit as ExitCode;

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

fn print_usage_error(detail: &str, usage: &str) {
    eprintln!("xuva: {detail}");
    eprintln!("  Usage: {usage}");
    eprintln!("  Try: xuva --help");
}
