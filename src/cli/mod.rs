use std::ffi::OsString;
use serde::{Deserialize, Serialize};

use crate::adapters::rtk::{command_surface_report};
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
        eprintln!("xuva: usage: surface [--json]");
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
    println!(
        "{} {} protocol {} command surface: {} adapter command families",
        report.adapter.name,
        report.adapter.version,
        report.adapter.protocol_version,
        report.upstream_command_count
    );
    for row in report.commands {
        println!(
            "{}\t{}\t{}",
            row.command,
            row.classification.as_str(),
            row.default_route
        );
    }
    ExitCode::SUCCESS
}
