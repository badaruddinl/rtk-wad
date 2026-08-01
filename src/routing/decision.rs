use std::ffi::OsString;

use crate::adapters::rtk::{CommandSurface, command_surface};
use crate::config::{ExecutionEnvironment, PolicyObjective, Route};
use crate::paths::windows_path_to_wsl_path;
use crate::providers::resolution::requires_raw_posix_provider;
use crate::routing::RoutePolicyFile;

pub(crate) fn command_family(arguments: &[OsString]) -> &str {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .unwrap_or("unknown")
}

pub(crate) fn is_wsl_path(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

pub(crate) fn has_wsl_path(arguments: &[OsString]) -> bool {
    arguments.iter().any(is_wsl_path)
}

pub(crate) fn git_subcommand(arguments: &[OsString]) -> Option<&str> {
    let mut skip_value = false;
    for argument in arguments.iter().skip(1) {
        let value = argument.to_str()?;
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(value, "-C" | "--git-dir" | "--work-tree" | "-c") {
            skip_value = true;
            continue;
        }
        if value.starts_with('-') {
            continue;
        }
        return Some(value);
    }
    None
}

pub(crate) fn is_verified_read_only_git(arguments: &[OsString]) -> bool {
    if matches!(
        arguments,
        [program, option]
            if program == "git"
                && matches!(option.to_str(), Some("--version" | "-v" | "--help" | "-h"))
    ) {
        return true;
    }
    matches!(
        git_subcommand(arguments),
        Some("status" | "log" | "show" | "diff" | "rev-parse" | "ls-files" | "grep")
    )
}

pub(crate) fn is_verified_cargo_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments.get(1).and_then(|argument| argument.to_str()),
        Some("check" | "test" | "clippy")
    )
}

pub(crate) fn is_verified_npm_run_list_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand] if program == "npm" && subcommand == "run"
    )
}

pub(crate) fn is_verified_go_test_all_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand, selector]
            if program == "go" && subcommand == "test" && selector == "./..."
    )
}

pub(crate) fn route_policy_key(arguments: &[OsString]) -> Option<String> {
    match command_family(arguments) {
        "git" => git_subcommand(arguments).map(|subcommand| format!("git:{subcommand}")),
        "rg" => Some("rg".to_owned()),
        "cargo" => arguments
            .get(1)
            .and_then(|subcommand| subcommand.to_str())
            .map(|subcommand| format!("cargo:{subcommand}")),
        "npm" if is_verified_npm_run_list_operation(arguments) => Some("npm:run-list".to_owned()),
        "go" if is_verified_go_test_all_operation(arguments) => Some("go:test-all".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn auto_route(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
) -> (Route, &'static str) {
    auto_route_with_context(
        arguments,
        current_directory,
        policy,
        None,
        PolicyObjective::Balanced,
    )
}

pub(crate) fn auto_route_with_context(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: Option<&str>,
    objective: PolicyObjective,
) -> (Route, &'static str) {
    if has_wsl_path(arguments)
        || current_directory.is_some_and(|directory| windows_path_to_wsl_path(directory).is_none())
    {
        return (
            Route::Wsl1,
            "Linux path or WSL working directory requires Linux execution",
        );
    }
    let policy_key = route_policy_key(arguments);
    if let Some((_key, route)) = policy_key.as_deref().and_then(|key| {
        context_signature
            .and_then(|context| policy.and_then(|policy| policy.route_for(key, context, objective)))
            .map(|route| (key, route))
    }) {
        let permitted = match route {
            Route::Raw => {
                command_family(arguments) == "rg"
                    || is_verified_read_only_git(arguments)
                    || is_verified_cargo_operation(arguments)
                    || is_verified_npm_run_list_operation(arguments)
                    || is_verified_go_test_all_operation(arguments)
            }
            Route::NativeRtk => {
                command_family(arguments) == "rg"
                    || is_verified_read_only_git(arguments)
                    || is_verified_cargo_operation(arguments)
                    || is_verified_npm_run_list_operation(arguments)
                    || is_verified_go_test_all_operation(arguments)
            }
            Route::Wsl1 | Route::Wsl2 | Route::Auto => false,
        };
        if permitted {
            return (
                route,
                if route == Route::Raw {
                    "local benchmark policy selected lower-latency raw execution"
                } else {
                    "local benchmark policy selected token-saving native RTK"
                },
            );
        }
    }
    match command_surface(command_family(arguments)) {
        CommandSurface::RawNative => (
            Route::Raw,
            "command manifest selects the validated Windows raw provider",
        ),
        CommandSurface::NativeStructured if command_family(arguments) == "git" => {
            if is_verified_read_only_git(arguments) {
                (
                    Route::NativeRtk,
                    "command manifest permits structured native RTK for read-only Git",
                )
            } else {
                (
                    Route::Raw,
                    "Git mutation uses native Git for NTFS object writes, Windows credentials, and Windows DNS",
                )
            }
        }
        CommandSurface::NativeStructured => (
            Route::NativeRtk,
            "command manifest selects the structured native RTK adapter",
        ),
        CommandSurface::Wsl1Conservative => (
            Route::Wsl1,
            "command manifest retains the conservative isolated Linux RTK contract",
        ),
        CommandSurface::CoreInternal => (
            Route::Wsl1,
            "RTK command is internal to XUVA only when invoked through its dedicated interface",
        ),
        CommandSurface::Unknown => match command_family(arguments) {
            "dart" | "flutter" => (
                Route::Raw,
                "XUVA-owned Windows SDK shim executes once without an RTK adapter",
            ),
            _ => (
                Route::Wsl1,
                "unknown command has no manifest contract; use isolated Linux RTK",
            ),
        },
    }
}

pub(crate) fn is_rtk_meta_command(command: &str) -> bool {
    matches!(
        command,
        "smart"
            | "err"
            | "test"
            | "json"
            | "deps"
            | "env"
            | "log"
            | "summary"
            | "init"
            | "wget"
            | "wc"
            | "cc-economics"
            | "config"
            | "discover"
            | "session"
            | "telemetry"
            | "learn"
            | "run"
            | "proxy"
            | "pipe"
            | "trust"
            | "untrust"
            | "verify"
            | "hook-audit"
            | "rewrite"
            | "hook"
    )
}

pub(crate) fn is_adapter_only_rtk_command(command: &str) -> bool {
    is_rtk_meta_command(command) && !requires_raw_posix_provider(command)
}

pub(crate) fn auto_route_for_environment(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: Option<&str>,
    environment: ExecutionEnvironment,
    objective: PolicyObjective,
) -> (Route, &'static str) {
    if environment == ExecutionEnvironment::Adaptive {
        return auto_route_with_context(
            arguments,
            current_directory,
            policy,
            context_signature,
            objective,
        );
    }

    let command = command_family(arguments);
    if is_rtk_meta_command(command) || command_surface(command) == CommandSurface::CoreInternal {
        return (
            Route::NativeRtk,
            "windows-only environment requires native RTK for an RTK meta command",
        );
    }
    match command_surface(command) {
        CommandSurface::NativeStructured
            if command == "git" && !is_verified_read_only_git(arguments) =>
        {
            (
                Route::Raw,
                "windows-only environment executes Git mutation once with native Git",
            )
        }
        CommandSurface::NativeStructured => (
            Route::NativeRtk,
            "windows-only environment selects the structured native RTK adapter",
        ),
        CommandSurface::RawNative | CommandSurface::Wsl1Conservative | CommandSurface::Unknown => (
            Route::Raw,
            "windows-only environment disables automatic WSL routing and uses the native command",
        ),
        CommandSurface::CoreInternal => unreachable!("XUVA core commands were handled above"),
    }
}
