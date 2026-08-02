use std::ffi::OsString;

use crate::adapters::rtk::{CommandSurface, command_surface};
use crate::command::{CommandAccess, classify};
use crate::config::{Config, ExecutionEnvironment, GitMode, PolicyObjective, Route};
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

pub(crate) fn is_verified_read_only_git(arguments: &[OsString]) -> bool {
    classify(arguments)
        .git
        .is_some_and(|git| git.access == CommandAccess::ReadOnly)
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
        "git" => classify(arguments)
            .git
            .and_then(|git| git.subcommand)
            .map(|subcommand| format!("git:{subcommand}")),
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
    if let Some(route) = authorized_policy_route(arguments, policy, context_signature, objective) {
        return (
            route,
            if route == Route::Raw {
                "local benchmark policy selected lower-latency raw execution"
            } else {
                "local benchmark policy selected token-saving native RTK"
            },
        );
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

pub(crate) fn authorized_policy_route(
    arguments: &[OsString],
    policy: Option<&RoutePolicyFile>,
    context_signature: Option<&str>,
    objective: PolicyObjective,
) -> Option<Route> {
    let key = route_policy_key(arguments)?;
    let route = policy?.route_for(&key, context_signature?, objective)?;
    let permitted = command_family(arguments) == "rg"
        || is_verified_read_only_git(arguments)
        || is_verified_cargo_operation(arguments)
        || is_verified_npm_run_list_operation(arguments)
        || is_verified_go_test_all_operation(arguments);
    (permitted && matches!(route, Route::Raw | Route::NativeRtk)).then_some(route)
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

pub(crate) fn git_uses_wsl_directory(arguments: &[OsString]) -> bool {
    classify(arguments)
        .git
        .is_some_and(|git| git.uses_wsl_directory)
}

pub(crate) fn should_use_native_git(
    arguments: &[OsString],
    config: &Config,
    current_directory: Option<&str>,
) -> bool {
    if arguments.first().is_none_or(|argument| argument != "git")
        || git_uses_wsl_directory(arguments)
    {
        return false;
    }
    match config.git_mode {
        GitMode::Native => true,
        GitMode::Wsl => false,
        GitMode::Auto => {
            config.cwd.is_none()
                && current_directory
                    .and_then(windows_path_to_wsl_path)
                    .is_some()
        }
    }
}
