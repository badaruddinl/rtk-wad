use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{Config, InvocationOrigin, OutputAdapterPreference, Route};
use crate::diagnostics::trace;
use crate::execution::environment::forwarded_environment;
use crate::execution::planner::execution_plan_for_provider_candidate;
use crate::planning::{current_project_location, windows_cwd_for_invocation};
use crate::providers::commands::is_safe_provider_tool_name;
use crate::providers::discovery::{installed_wsl_distributions, is_windows_launchable_path};
use crate::providers::mapping::{
    mapped_windows_project_path, mapped_wsl_project_path, translate_arguments_to_windows,
    translate_arguments_to_wsl,
};
use crate::providers::model::{
    AdapterKind, ProjectLocation, ProjectLocationKind, ProviderCandidate, ProviderResolution,
    WindowsToolProbe,
};
use crate::providers::probe::cached_or_discovered_tool;
use crate::providers::resolution::{
    requires_raw_posix_provider, resolve_tool_provider_from_discovery_with_user,
    windows_probe_has_compatible_provider, windows_provider_has_compatible_semantics,
};
use crate::routing::decision::{is_adapter_only_rtk_command, is_verified_read_only_git};
use crate::wsl::exec_prefix as wsl_exec_prefix;
use crate::{PRODUCT_COMMAND, dispatcher, process};

pub(crate) enum ProviderDispatchDecision {
    KeepStaticRoute,
    UsePlan {
        plan: Box<dispatcher::ExecutionPlan>,
        fallbacks: Vec<dispatcher::ExecutionPlan>,
        reason: String,
    },
    Missing {
        reason: String,
    },
}

pub(crate) fn is_dispatchable_provider_tool(arguments: &[OsString]) -> bool {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(is_safe_provider_tool_name)
}

pub(crate) fn looks_like_explicit_executable(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with(".\\")
        || value.starts_with("..\\")
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'\\' | b'/'))
}

pub(crate) fn wsl_executable_exists(distro: &str, user: Option<&str>, path: &str) -> bool {
    let mut command = Command::new("wsl.exe");
    let mut arguments = wsl_exec_prefix(distro, user);
    arguments.extend([
        OsString::from("test"),
        OsString::from("-f"),
        OsString::from(path),
        OsString::from("-a"),
        OsString::from("-x"),
        OsString::from(path),
    ]);
    command.args(arguments);
    process::run_probe(&mut command).is_ok_and(|output| output.status.success())
}

pub(crate) fn explicit_executable_plan(
    arguments: &[OsString],
    config: &Config,
) -> Result<Option<(dispatcher::ExecutionPlan, String)>, String> {
    let Some(value) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Ok(None);
    };
    if !looks_like_explicit_executable(value) {
        return Ok(None);
    }
    let tool_arguments = &arguments[1..];
    let windows_path = value.contains('\\')
        || (value.len() >= 3 && value.as_bytes()[1] == b':')
        || is_windows_launchable_path(value);
    if windows_path {
        let mapped_value = if value.starts_with('/') {
            mapped_wsl_project_path(&config.distro, config.user.as_deref(), value).ok_or_else(
                || {
                    format!(
                        "explicit Windows executable `{value}` could not be mapped by the originating WSL user"
                    )
                },
            )?
        } else {
            value.to_owned()
        };
        let resolved = fs::canonicalize(&mapped_value).map_err(|error| {
            format!("explicit Windows executable `{value}` is unavailable: {error}")
        })?;
        if !resolved.is_file() || !is_windows_launchable_path(&resolved.to_string_lossy()) {
            return Err(format!(
                "explicit Windows executable `{value}` is not a launchable .exe, .com, .cmd, or .bat file"
            ));
        }
        let cwd = Some(windows_cwd_for_invocation(config)?);
        let executable = resolved.into_os_string();
        let tool = Path::new(&executable)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let request = dispatcher::CommandSpec {
            executable: executable.clone(),
            arguments: translate_arguments_to_windows(tool, tool_arguments, config),
            cwd: cwd.clone(),
            environment: forwarded_environment(config),
            environment_policy: if matches!(config.invocation_origin, InvocationOrigin::Wsl { .. })
            {
                dispatcher::EnvironmentPolicy::Isolated
            } else {
                dispatcher::EnvironmentPolicy::Inherit
            },
            interactive: false,
        };
        return Ok(Some((
            dispatcher::ExecutionPlan {
                request,
                candidate: dispatcher::RouteCandidate::Windows { executable, cwd },
                adapter: dispatcher::OutputAdapter::Raw,
                explanation: vec![dispatcher::DecisionReason(
                    "explicit Windows executable path pins the Windows host".to_owned(),
                )],
            },
            "explicit Windows executable path".to_owned(),
        )));
    }

    let executable = if value.starts_with('/') {
        value.to_owned()
    } else {
        let cwd = config.cwd.as_deref().ok_or_else(|| {
            "relative Linux executable paths require a WSL-origin working directory".to_owned()
        })?;
        format!("{}/{}", cwd.trim_end_matches('/'), value)
    };
    if !wsl_executable_exists(&config.distro, config.user.as_deref(), &executable) {
        return Err(format!(
            "explicit WSL executable `{executable}` is missing or not executable in {}",
            config.distro
        ));
    }
    let cwd = config
        .cwd
        .clone()
        .or_else(|| {
            env::current_dir().ok().and_then(|path| {
                mapped_windows_project_path(
                    &config.distro,
                    config.user.as_deref(),
                    &path.to_string_lossy(),
                )
            })
        })
        .ok_or_else(|| "unable to map the current directory into WSL".to_owned())?;
    let tool = executable.rsplit('/').next().unwrap_or_default();
    let request = dispatcher::CommandSpec {
        executable: OsString::from(&executable),
        arguments: translate_arguments_to_wsl(tool, tool_arguments, config, &config.distro),
        cwd: Some(PathBuf::from(&cwd)),
        environment: forwarded_environment(config),
        environment_policy: dispatcher::EnvironmentPolicy::Isolated,
        interactive: false,
    };
    let candidate = match installed_wsl_distributions()
        .into_iter()
        .find(|(distro, _)| distro == &config.distro)
        .and_then(|(_, version)| version)
    {
        Some(1) => dispatcher::RouteCandidate::Wsl1 {
            distro: config.distro.clone(),
            executable: OsString::from(&executable),
            cwd: PathBuf::from(&cwd),
        },
        _ => dispatcher::RouteCandidate::Wsl2 {
            distro: config.distro.clone(),
            executable: OsString::from(&executable),
            cwd: PathBuf::from(&cwd),
        },
    };
    Ok(Some((
        dispatcher::ExecutionPlan {
            request,
            candidate,
            adapter: dispatcher::OutputAdapter::Raw,
            explanation: vec![dispatcher::DecisionReason(
                "explicit Linux executable path pins the configured WSL host".to_owned(),
            )],
        },
        "explicit WSL executable path".to_owned(),
    )))
}

pub(crate) fn windows_tool_is_usable(
    tool: &str,
    project: &ProjectLocation,
    static_route: Route,
    windows: &WindowsToolProbe,
) -> bool {
    project.kind != ProjectLocationKind::Wsl
        && match static_route {
            Route::Raw => {
                windows.executable.is_some()
                    && windows_provider_has_compatible_semantics(tool, AdapterKind::Raw)
            }
            Route::NativeRtk => {
                windows.native_rtk.is_some()
                    && windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk)
            }
            // WSL routes are legacy route suggestions, not a reason to skip
            // generic candidate resolution when a Windows tool is verified.
            Route::Wsl1 | Route::Wsl2 | Route::Auto => false,
        }
}

pub(crate) fn provider_dispatch_decision(
    arguments: &[OsString],
    config: &Config,
    static_route: Route,
) -> ProviderDispatchDecision {
    if !is_dispatchable_provider_tool(arguments) {
        return ProviderDispatchDecision::KeepStaticRoute;
    }
    let tool = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .expect("dispatchable provider tools have a safe Unicode name");
    trace(format!(
        "provider dispatch evaluating {tool} (adapter_only={}, raw_posix={})",
        is_adapter_only_rtk_command(tool),
        requires_raw_posix_provider(tool)
    ));
    // Adapter-only RTK commands are subcommands, not external executables.
    // Keep this boundary distinct from the broader RTK command surface:
    // `wc`, for example, is recognized by RTK but deliberately resolves a raw
    // POSIX executable to preserve GNU/POSIX argv semantics.
    if is_adapter_only_rtk_command(tool) {
        return ProviderDispatchDecision::KeepStaticRoute;
    }
    let project = current_project_location(config);
    // A Windows project always probes its native executable first. This keeps
    // an unknown command such as `code`, `nvm`, or a user tool out of WSL when
    // Windows already owns it. Only a missing native candidate expands to the
    // WSL inventory. A WSL project still needs the complete inventory first so
    // its same-distro provider keeps precedence over a compatible Windows one.
    let (windows_discovery, windows_cache) =
        cached_or_discovered_tool(tool, config, false, false, false);
    if project.kind != ProjectLocationKind::Wsl
        && windows_probe_has_compatible_provider(tool, &windows_discovery.windows)
    {
        return provider_dispatch_decision_from_resolution(
            arguments,
            config,
            static_route,
            resolve_tool_provider_from_discovery_with_user(
                tool,
                project,
                windows_discovery,
                windows_cache,
                config.user.as_deref(),
            ),
        );
    }
    let (discovery, cache) = cached_or_discovered_tool(tool, config, false, true, false);
    let resolution = resolve_tool_provider_from_discovery_with_user(
        tool,
        project,
        discovery,
        cache,
        config.user.as_deref(),
    );
    provider_dispatch_decision_from_resolution(arguments, config, static_route, resolution)
}

pub(crate) fn provider_dispatch_decision_from_resolution(
    arguments: &[OsString],
    config: &Config,
    static_route: Route,
    resolution: ProviderResolution,
) -> ProviderDispatchDecision {
    let windows_is_usable = windows_tool_is_usable(
        arguments
            .first()
            .and_then(|argument| argument.to_str())
            .unwrap_or_default(),
        &resolution.project,
        static_route,
        &resolution.availability.windows,
    );
    if windows_is_usable {
        return ProviderDispatchDecision::KeepStaticRoute;
    }
    let Some((tool, tool_arguments)) = arguments.split_first() else {
        return ProviderDispatchDecision::Missing {
            reason: "Provider execution request has no executable".to_owned(),
        };
    };
    let Some(tool) = tool.to_str() else {
        return ProviderDispatchDecision::Missing {
            reason: "Provider executable name is not valid Unicode".to_owned(),
        };
    };
    let pin_windows_git = resolution.project.kind == ProjectLocationKind::Windows
        && tool == "git"
        && !is_verified_read_only_git(arguments);
    let eligible = |candidate: &&ProviderCandidate| {
        candidate.usable
            && candidate.has_consistent_location()
            && (!pin_windows_git || candidate.is_windows())
            && (config.output_adapter != OutputAdapterPreference::Rtk
                || candidate.supports_adapter(AdapterKind::Rtk))
    };
    let preferred_wsl = resolution.project.distro.as_deref();
    let ordered_candidates: Vec<&ProviderCandidate> = if resolution.project.kind
        == ProjectLocationKind::Wsl
    {
        resolution
            .candidates
            .iter()
            .filter(|candidate| eligible(candidate) && candidate.distro.as_deref() == preferred_wsl)
            .chain(resolution.candidates.iter().filter(|candidate| {
                eligible(candidate)
                    && candidate.is_wsl()
                    && candidate.distro.as_deref() != preferred_wsl
            }))
            .chain(
                resolution
                    .candidates
                    .iter()
                    .filter(|candidate| eligible(candidate) && candidate.is_windows()),
            )
            .collect()
    } else {
        resolution.candidates.iter().filter(eligible).collect()
    };
    let mut planning_errors = Vec::new();
    let mut planned_candidates = Vec::new();
    for candidate in ordered_candidates {
        match execution_plan_for_provider_candidate(tool, tool_arguments, config, candidate) {
            Ok(plan) => planned_candidates.push((candidate.clone(), plan)),
            Err(error) => planning_errors.push(format!("{}: {error}", candidate.executable)),
        }
    }
    let Some((candidate, plan)) = planned_candidates.first().cloned() else {
        let detail = if planning_errors.is_empty() {
            "no compatible candidate was discovered".to_owned()
        } else {
            planning_errors.join("; ")
        };
        return ProviderDispatchDecision::Missing {
            reason: format!(
                "command `{}` was not found in verified Windows or WSL providers ({detail}); run `{PRODUCT_COMMAND} doctor {}` for provider evidence or `{PRODUCT_COMMAND} --help` for dispatcher syntax. XUVA does not execute shell builtins implicitly.",
                resolution.tool, resolution.tool
            ),
        };
    };
    let fallbacks = planned_candidates
        .into_iter()
        .skip(1)
        .map(|(_, plan)| plan)
        .collect();
    let adapter_name = plan.adapter.as_str();
    let location = if candidate.is_windows() {
        "Windows".to_owned()
    } else {
        format!(
            "WSL {}",
            candidate.distro.as_deref().unwrap_or("unknown-distro")
        )
    };
    let reason = if resolution.tool == "git" && candidate.is_windows() {
        format!(
            "native Git on Windows owns the NTFS worktree, object writes, credentials, and Windows DNS; selected {} with {} output",
            candidate.host.as_str(),
            adapter_name,
        )
    } else {
        format!(
            "on-demand {} discovery selected {} on {} with a verified project path and {} output adapter",
            resolution.tool,
            candidate.host.as_str(),
            location,
            adapter_name,
        )
    };
    ProviderDispatchDecision::UsePlan {
        plan: Box::new(plan),
        fallbacks,
        reason,
    }
}
