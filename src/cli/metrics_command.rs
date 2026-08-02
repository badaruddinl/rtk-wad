use std::ffi::OsString;

use crate::cli_exit::CliExit as ExitCode;
use crate::metrics::{self, XuvaMetrics};

pub(crate) fn command(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first().and_then(|value| value.to_str()) != Some("metrics") {
        return None;
    }
    Some(match arguments.get(1).and_then(|value| value.to_str()) {
        None | Some("status") if arguments.len() <= 2 => print_status(),
        Some("purge") if arguments.len() == 2 => purge(),
        Some("help" | "--help" | "-h") if arguments.len() == 2 => {
            print_help();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("xuva: invalid metrics command");
            eprintln!("usage: xuva metrics <status|purge>");
            ExitCode::FAILURE
        }
    })
}

fn print_status() -> ExitCode {
    println!("XUVA Local Metrics");
    println!();
    println!("  Collection: off by default; enable with XUVA_METRICS=on");
    println!("  Stored data: route, command family, token totals, duration, and exit code");
    println!("  Never stored: command arguments, project paths, parse input, or error text");
    println!("  Retention: newest 10,000 aggregate invocation records");
    println!("  Directory: {}", metrics::xuva_data_root().display());
    println!();
    match XuvaMetrics::print_gain() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xuva: {error}");
            ExitCode::FAILURE
        }
    }
}

fn purge() -> ExitCode {
    match XuvaMetrics::purge() {
        Ok(removed) => {
            println!("XUVA local metrics purged.");
            println!("  Removed files: {removed}");
            println!("  State directory: {}", metrics::xuva_data_root().display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("xuva: unable to purge local metrics: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("Usage");
    println!("  xuva metrics status");
    println!("    Show the local-only privacy contract and aggregate totals.");
    println!("  xuva metrics purge");
    println!("    Delete the local aggregate ledger and temporary metrics files.");
}
