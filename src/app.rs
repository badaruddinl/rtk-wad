use std::env;
use std::ffi::OsString;

use crate::cli::runtime::run_cli;
use crate::{PRODUCT_COMMAND, bridge, cli, cli_exit, config, lifecycle};

use bridge::wsl_bridge_request;
use cli::{is_verbose_version_command, is_version_command, print_verbose_version};
use cli_exit::CliExit as ExitCode;
use config::{Config, InvocationOrigin};

fn main_exit() -> ExitCode {
    let original_arguments: Vec<OsString> = env::args_os().skip(1).collect();
    // This is intentionally before bridge decoding and environment parsing:
    // a local version query must remain instant even when WSL is unavailable
    // or a caller has an invalid dispatcher configuration.
    if is_verbose_version_command(&original_arguments) {
        print_verbose_version();
        return ExitCode::SUCCESS;
    }
    if is_version_command(&original_arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // Lifecycle recovery must remain available even when a stale environment
    // contains invalid routing configuration. WSL bridge requests are decoded
    // below and receive the same handling inside `run_cli`.
    if let Some(result) = lifecycle::command(&original_arguments) {
        return result;
    }
    let bridge = match wsl_bridge_request(&original_arguments) {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("xuva: invalid WSL bridge payload: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("xuva: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    let arguments = if let Some(bridge) = bridge {
        config.invocation_origin = InvocationOrigin::Wsl {
            distro: bridge.distro.clone(),
        };
        config.distro = bridge.distro;
        config.user = Some(bridge.origin_user);
        config.cwd = Some(bridge.cwd);
        config.bridge_windows_cwd = bridge.windows_cwd;
        config.extra_path = bridge.extra_path;
        config.output_adapter = bridge.output_adapter;
        bridge.arguments
    } else {
        original_arguments
    };
    run_cli(arguments, &config)
}

pub fn run_from_env() -> ! {
    main_exit().terminate();
}
