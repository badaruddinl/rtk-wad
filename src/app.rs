use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    PRODUCT_COMMAND, adapters, agent, bridge, cli, cli_exit, config, dispatcher, lifecycle,
    metrics, paths, planning, process, providers, routing,
};

#[cfg(test)]
use adapters::rtk::command_surface_report;
use adapters::rtk::{CommandSurface, adapter_contract_id, command_surface};
#[cfg(test)]
use adapters::windows::apply_command_spec;
#[cfg(test)]
use bridge::decode_wsl_bridge_fields;
use bridge::wsl_bridge_request;
use cli_exit::CliExit as ExitCode;
#[cfg(test)]
use config::ExecutableProfile;
use config::{
    Config, DEFAULT_DISTRO, DEFAULT_WSL1_DISTRO, ExecutionEnvironment, GitMode, InvocationOrigin,
    OutputAdapterPreference, PolicyObjective, Route, WslBackend, is_sensitive_environment_name,
};
use metrics::{TokenTotals, XuvaMetrics, xuva_data_root};
use paths::{translate_arguments_with, windows_path_to_wsl_path, wsl_path_to_windows_path};
use planning::{
    classify_project_path, current_project_location, provider_environment_policy,
    windows_cwd_for_invocation,
};
#[cfg(test)]
use providers::cache::PROVIDER_CACHE_TTL_SECONDS;
use providers::cache::{
    PROVIDER_CACHE_SCHEMA_VERSION, cache_entry_is_fresh, discovery_context_signature,
    load_provider_cache, save_provider_cache, unix_seconds,
};
use providers::discovery::{
    VersionProbe, configured_windows_executable, decode_wsl_output, first_output_line,
    first_windows_executable, installed_wsl_distributions, is_windows_launchable_path,
    parse_wsl_binary_identity, tool_version, version_capabilities, windows_binary_identity,
};
#[cfg(test)]
use providers::discovery::{
    is_eligible_wsl_distro, parse_wsl_distributions, select_windows_executable,
    version_probe_arguments,
};
use providers::model::{
    AdapterKind, BinaryIdentity, InspectionLevel, ProbeStatus, ProjectLocation,
    ProjectLocationKind, ProviderCacheEntry, ProviderCandidate, ProviderHost, ProviderResolution,
    WindowsToolProbe, WslToolProbe,
};

const ADAPTER_INFO_ARGUMENT: &str = "--adapter-info";
const VERSION_ARGUMENT: &str = "--version";
const EXPLAIN_ROUTE_ARGUMENT: &str = "--explain-route";
const POLICY_ARGUMENT: &str = "policy";
const CALIBRATION_ARGUMENT: &str = "calibration";
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
const RELEASE_TAGS_URL: &str = "https://github.com/badsleepyday/xuva.git";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const CANCEL_SCRIPT: &str = include_str!("scripts/cancel.sh");
const CANCEL_PROBE_SCRIPT: &str = include_str!("scripts/cancel_probe.sh");
const LAUNCH_SCRIPT: &str = include_str!("scripts/launch.sh");
const WSL1_MARKER_VALIDATOR_SCRIPT: &str = include_str!("scripts/wsl1_marker_validator.sh");
const WSL1_LAUNCH_SCRIPT: &str = include_str!("scripts/wsl1_launch.sh");
const PLAN_LAUNCH_SCRIPT: &str = include_str!("scripts/plan_launch.sh");
#[cfg(test)]
fn distro_version_from_list(output: &str, distro: &str) -> Option<u8> {
    output.lines().find_map(|line| {
        let trimmed = line.trim().trim_start_matches('*').trim_start();
        let remainder = trimmed.strip_prefix(distro)?;
        if remainder.is_empty() || !remainder.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }
        remainder.split_whitespace().last()?.parse::<u8>().ok()
    })
}

fn trace(message: impl AsRef<str>) {
    if env::var("XUVA_WSL_TRACE").as_deref() == Ok("1") {
        eprintln!("xuva: trace: {}", message.as_ref());
    }
}

use routing::{
    CALIBRATION_MAX_SAMPLES, CALIBRATION_SCHEMA_VERSION, CalibrationEntry, CalibrationFile,
    CalibrationPlan, NativeCalibrationSample, PolicyContextReport, ROUTE_POLICY_SCHEMA_VERSION,
    RoutePolicyEvidence, RoutePolicyFile, adaptive_context_signature, calibration_plan,
    calibration_signature, median, policy_context_report, select_adaptive_route,
};

use cli::{SetupPlan, SetupTransaction, print_command_surface};

fn probe_wsl_tool(
    distro: &str,
    wsl_version: Option<u8>,
    user: Option<&str>,
    tool: &str,
    extra_path: Option<&str>,
    inspect_version: bool,
) -> WslToolProbe {
    let script = concat!(
        "if [ -n \"$2\" ]; then PATH=\"$2:$PATH\"; fi; ",
        "tool_path=$(command -v \"$1\" 2>/dev/null || true); ",
        "case \"$tool_path\" in /*) [ -f \"$tool_path\" ] && [ -x \"$tool_path\" ] || tool_path= ;; *) tool_path= ;; esac; ",
        "rtk_path=$(command -v rtk 2>/dev/null || true); ",
        "case \"$rtk_path\" in /*) [ -f \"$rtk_path\" ] && [ -x \"$rtk_path\" ] || rtk_path= ;; *) rtk_path= ;; esac; ",
        "tool_identity=$(stat -Lc '%s:%Y' -- \"$tool_path\" 2>/dev/null || true); ",
        "rtk_identity=$(stat -Lc '%s:%Y' -- \"$rtk_path\" 2>/dev/null || true); ",
        "installation_id=; ",
        "if [ \"$3\" = 1 ]; then installation_id=$(/bin/sh -c \"$4\") || installation_id=; fi; ",
        "printf '%s\\n%s\\n%s\\n%s\\n%s\\n' \"$tool_path\" \"$rtk_path\" \"$tool_identity\" \"$rtk_identity\" \"$installation_id\"",
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let output = loop {
        let mut command = Command::new("wsl.exe");
        command
            .args(wsl_exec_prefix(distro, user))
            .args(["sh", "-c", script, "xuva-provider-probe", tool])
            .arg(extra_path.unwrap_or_default())
            .arg(wsl_version.map_or_else(String::new, |version| version.to_string()))
            .arg(WSL1_MARKER_VALIDATOR_SCRIPT);
        let output = process::run_probe(&mut command);
        let should_retry = wsl_version == Some(1)
            && output
                .as_ref()
                .map_or(true, |result| !result.status.success())
            && Instant::now() < deadline;
        if !should_retry {
            break output;
        }
        // Store-backed WSL1 can briefly reject the first new session after a
        // deliberate distro termination. This is a bounded, read-only
        // discovery retry; no user command or adapter is ever replayed.
        thread::sleep(Duration::from_millis(100));
    };
    let (executable, rtk, executable_identity, rtk_identity, installation_id) = output
        .ok()
        .filter(|result| result.status.success())
        .map(|result| {
            let rendered = decode_wsl_output(&result.stdout);
            let mut lines = rendered.lines().map(str::trim).map(str::to_owned);
            let executable = lines.next().and_then(verified_wsl_executable_path);
            let rtk = lines.next().and_then(verified_wsl_executable_path);
            let executable_identity = lines.next().filter(|line| !line.is_empty());
            let rtk_identity = lines.next().filter(|line| !line.is_empty());
            let installation_id = lines
                .next()
                .filter(|installation_id| valid_installation_id(installation_id));
            (
                executable.clone(),
                rtk.clone(),
                parse_wsl_binary_identity(executable, executable_identity),
                parse_wsl_binary_identity(rtk, rtk_identity),
                installation_id,
            )
        })
        .unwrap_or((None, None, None, None, None));
    let version_probe = if inspect_version {
        executable.as_deref().map_or(
            VersionProbe {
                version: None,
                status: ProbeStatus::Failed,
            },
            |path| tool_version(tool, path, Some((distro, user))),
        )
    } else {
        VersionProbe {
            version: None,
            status: ProbeStatus::NotRequested,
        }
    };
    let executable_version = version_probe.version;
    WslToolProbe {
        distro: distro.to_owned(),
        user: user.map(str::to_owned),
        wsl_version,
        dedicated: installation_id.is_some(),
        installation_id,
        executable_capabilities: version_capabilities(&executable_version),
        executable_version,
        version_probe_status: version_probe.status,
        executable,
        rtk,
        executable_identity,
        rtk_identity,
    }
}

fn discover_tool(
    tool: &str,
    config: &Config,
    include_wsl: bool,
    inspect_versions: bool,
) -> ProviderCacheEntry {
    let executable = if tool == "go" { "go.exe" } else { tool };
    let windows_executable = first_windows_executable(executable);
    let native_rtk = configured_windows_executable(&config.native_rtk_path);
    let version_probe = if inspect_versions {
        windows_executable.as_deref().map_or(
            VersionProbe {
                version: None,
                status: ProbeStatus::Failed,
            },
            |path| tool_version(tool, path, None),
        )
    } else {
        VersionProbe {
            version: None,
            status: ProbeStatus::NotRequested,
        }
    };
    let executable_version = version_probe.version;
    let windows = WindowsToolProbe {
        executable_capabilities: version_capabilities(&executable_version),
        executable_version,
        version_probe_status: version_probe.status,
        executable_identity: windows_executable
            .as_deref()
            .and_then(windows_binary_identity),
        native_rtk_identity: native_rtk.as_deref().and_then(windows_binary_identity),
        executable: windows_executable,
        native_rtk,
    };
    let mut distros = include_wsl
        .then(installed_wsl_distributions)
        .unwrap_or_default();
    distros.sort_by_key(|(distro, version)| {
        if distro == &config.distro {
            0
        } else if *version == Some(2) {
            1
        } else {
            2
        }
    });
    let distro_count = distros.len();
    let mut wsl = Vec::new();
    for (distro, version) in distros {
        let probe = probe_wsl_tool(
            &distro,
            version,
            config.user.as_deref(),
            tool,
            config.extra_path.as_deref(),
            inspect_versions,
        );
        let sufficient = probe.executable.is_some()
            || (command_surface(tool) == CommandSurface::NativeStructured && probe.rtk.is_some());
        wsl.push(probe);
        if sufficient && !inspect_versions {
            break;
        }
    }
    let wsl_probe_complete = include_wsl && wsl.len() == distro_count;
    ProviderCacheEntry {
        tool: tool.to_owned(),
        observed_unix_seconds: unix_seconds(),
        inspection_level: if inspect_versions {
            InspectionLevel::Version
        } else {
            InspectionLevel::Identity
        },
        context_signature: discovery_context_signature(config, include_wsl),
        windows,
        wsl_probe_complete,
        wsl,
    }
}

fn cached_or_discovered_tool(
    tool: &str,
    config: &Config,
    refresh: bool,
    require_wsl: bool,
    validate_versions: bool,
) -> (ProviderCacheEntry, &'static str) {
    let now = unix_seconds();
    let context_signature = discovery_context_signature(config, require_wsl);
    let mut cache = load_provider_cache();
    if !refresh
        && let Some(entry) = cache.entries.iter().find(|entry| {
            entry.tool == tool
                && cache_entry_is_fresh(entry, now, &context_signature, validate_versions)
                && (!require_wsl
                    || entry.wsl_probe_complete
                    || (!validate_versions && !entry.wsl.is_empty()))
        })
    {
        return (entry.clone(), "hit");
    }
    let discovered = discover_tool(tool, config, require_wsl, validate_versions);
    cache.entries.retain(|entry| entry.tool != tool);
    cache.entries.push(discovered.clone());
    if let Err(error) = save_provider_cache(&cache) {
        trace(format!("provider cache was not saved: {error}"));
    }
    (discovered, "miss")
}

fn wsl_exec_prefix(distro: &str, user: Option<&str>) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("-d"), OsString::from(distro)];
    if let Some(user) = user {
        arguments.extend([OsString::from("-u"), OsString::from(user)]);
    }
    arguments.push(OsString::from("--exec"));
    arguments
}

fn wsl_mapping_arguments_with_user(
    distro: &str,
    user: Option<&str>,
    windows_path: &str,
) -> Vec<OsString> {
    let mut arguments = wsl_exec_prefix(distro, user);
    arguments.extend([
        OsString::from("wslpath"),
        OsString::from("-a"),
        OsString::from(windows_path),
    ]);
    arguments
}

fn mapped_windows_project_path(
    distro: &str,
    user: Option<&str>,
    windows_path: &str,
) -> Option<String> {
    let mut command = Command::new("wsl.exe");
    command.args(wsl_mapping_arguments_with_user(distro, user, windows_path));
    process::run_probe(&mut command)
        .ok()
        .filter(|output| output.status.success() && !output.stdout_truncated)
        .and_then(|output| first_output_line(&output.stdout))
        .filter(|path| path.starts_with('/'))
}

fn windows_mapping_arguments_with_user(
    distro: &str,
    user: Option<&str>,
    linux_path: &str,
) -> Vec<OsString> {
    let mut arguments = wsl_exec_prefix(distro, user);
    arguments.extend([
        OsString::from("wslpath"),
        OsString::from("-w"),
        OsString::from("-a"),
        OsString::from(linux_path),
    ]);
    arguments
}

fn mapped_wsl_project_path(distro: &str, user: Option<&str>, linux_path: &str) -> Option<String> {
    let mut command = Command::new("wsl.exe");
    command.args(windows_mapping_arguments_with_user(
        distro, user, linux_path,
    ));
    process::run_probe(&mut command)
        .ok()
        .filter(|output| output.status.success() && !output.stdout_truncated)
        .and_then(|output| first_output_line(&output.stdout))
}

fn translate_arguments_to_windows(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
) -> Vec<OsString> {
    translate_arguments_with(tool, arguments, |value| {
        if value.starts_with('/')
            && matches!(config.invocation_origin, InvocationOrigin::Wsl { .. })
        {
            mapped_wsl_project_path(&config.distro, config.user.as_deref(), value)
                .or_else(|| wsl_path_to_windows_path(value))
        } else {
            wsl_path_to_windows_path(value)
        }
    })
}

fn translate_arguments_to_wsl(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    target_distro: &str,
) -> Vec<OsString> {
    translate_arguments_with(tool, arguments, |value| {
        if value.starts_with('/') {
            let InvocationOrigin::Wsl {
                distro: origin_distro,
            } = &config.invocation_origin
            else {
                return None;
            };
            if origin_distro == target_distro {
                return None;
            }
            let windows = mapped_wsl_project_path(origin_distro, config.user.as_deref(), value)?;
            mapped_windows_project_path(target_distro, config.user.as_deref(), &windows)
        } else {
            mapped_windows_project_path(target_distro, config.user.as_deref(), value)
                .or_else(|| windows_path_to_wsl_path(value))
        }
    })
}

fn wsl_directory_exists(distro: &str, user: Option<&str>, path: &str) -> bool {
    let mut command = Command::new("wsl.exe");
    command.args({
        let mut arguments = wsl_exec_prefix(distro, user);
        arguments.extend([
            OsString::from("test"),
            OsString::from("-d"),
            OsString::from(path),
        ]);
        arguments
    });
    process::run_probe(&mut command).is_ok_and(|output| output.status.success())
}

fn is_windows_project_path_for_distro(path: &str, expected_distro: Option<&str>) -> bool {
    match classify_project_path(path) {
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            ..
        } => true,
        ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            distro: Some(distro),
            ..
        } => expected_distro.is_some_and(|expected| distro.eq_ignore_ascii_case(expected)),
        ProjectLocation {
            kind: ProjectLocationKind::Wsl | ProjectLocationKind::Unknown,
            ..
        } => false,
    }
}

fn wsl_project_path_with(
    project: &ProjectLocation,
    probe: &WslToolProbe,
    map_windows_path: impl FnOnce(&str, &str) -> Option<String>,
    directory_exists: impl FnOnce(&str, &str) -> bool,
) -> Option<String> {
    let path = match project.kind {
        ProjectLocationKind::Windows => map_windows_path(&probe.distro, &project.path),
        ProjectLocationKind::Wsl if project.distro.as_deref() == Some(probe.distro.as_str()) => {
            Some(project.path.clone())
        }
        ProjectLocationKind::Wsl => project
            .windows_path
            .as_deref()
            .and_then(|path| map_windows_path(&probe.distro, path)),
        ProjectLocationKind::Unknown => None,
    }?;
    (path.starts_with('/') && directory_exists(&probe.distro, &path)).then_some(path)
}

fn wsl_project_path(
    project: &ProjectLocation,
    probe: &WslToolProbe,
    user: Option<&str>,
) -> Option<String> {
    if project.kind == ProjectLocationKind::Windows
        && let Some(path) = windows_path_to_wsl_path(&project.path)
        && wsl_directory_exists(&probe.distro, user, &path)
    {
        return Some(path);
    }
    wsl_project_path_with(
        project,
        probe,
        |distro, path| mapped_windows_project_path(distro, user, path),
        |distro, path| wsl_directory_exists(distro, user, path),
    )
}

fn windows_project_path_with(
    project: &ProjectLocation,
    map_wsl_path: impl FnOnce(&str, &str) -> Option<String>,
    directory_exists: impl FnOnce(&str) -> bool,
) -> Option<String> {
    let path = match project.kind {
        ProjectLocationKind::Windows => Some(project.path.clone()),
        ProjectLocationKind::Wsl => project.windows_path.clone().or_else(|| {
            project
                .distro
                .as_deref()
                .and_then(|distro| map_wsl_path(distro, &project.path))
        }),
        ProjectLocationKind::Unknown => None,
    }?;
    let expected_distro = (project.kind == ProjectLocationKind::Wsl)
        .then_some(project.distro.as_deref())
        .flatten();
    (is_windows_project_path_for_distro(&path, expected_distro) && directory_exists(&path))
        .then_some(path)
}

fn windows_project_path(project: &ProjectLocation, user: Option<&str>) -> Option<String> {
    windows_project_path_with(
        project,
        |distro, path| mapped_wsl_project_path(distro, user, path),
        |path| Path::new(path).is_dir(),
    )
}

fn resolve_tool_provider(tool: &str, config: &Config, refresh: bool) -> ProviderResolution {
    resolve_tool_provider_with_inspection(tool, config, refresh, true)
}

fn resolve_tool_provider_with_inspection(
    tool: &str,
    config: &Config,
    refresh: bool,
    inspect_versions: bool,
) -> ProviderResolution {
    let project = current_project_location(config);
    let (discovery, cache) =
        cached_or_discovered_tool(tool, config, refresh, true, inspect_versions);
    resolve_tool_provider_from_discovery_with_user(
        tool,
        project,
        discovery,
        cache,
        config.user.as_deref(),
    )
}

fn resolve_tool_provider_from_discovery_with_user(
    tool: &str,
    project: ProjectLocation,
    discovery: ProviderCacheEntry,
    cache: &'static str,
    user: Option<&str>,
) -> ProviderResolution {
    let availability = discovery.clone();
    let mut candidates = Vec::new();
    let windows_rtk_fallback = (command_surface(tool) == CommandSurface::NativeStructured
        && tool != "git")
        .then(|| discovery.windows.native_rtk.clone())
        .flatten();
    let windows_raw_available = discovery.windows.executable.is_some()
        && windows_provider_has_compatible_semantics(tool, AdapterKind::Raw);
    if let Some(executable) = discovery
        .windows
        .executable
        .clone()
        .or(windows_rtk_fallback)
    {
        let project_path = windows_project_path(&project, user);
        let compatible = windows_probe_has_compatible_provider(tool, &discovery.windows);
        let usable = project_path.is_some() && compatible;
        candidates.push(ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: [
                windows_raw_available.then_some(AdapterKind::Raw),
                (discovery.windows.native_rtk.is_some()
                    && windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk))
                .then_some(AdapterKind::Rtk),
            ]
            .into_iter()
            .flatten()
            .collect(),
            distro: None,
            wsl_version: None,
            executable,
            rtk: discovery.windows.native_rtk.clone(),
            project_path,
            usable,
            reason: if !compatible {
                format!(
                    "Windows `{tool}` has incompatible command semantics; XUVA requires the POSIX provider for this command family"
                )
            } else if usable {
                if project.kind == ProjectLocationKind::Wsl {
                    "Windows toolchain and WSL-to-Windows project mapping are verified; generic execution remains diagnostic until P14".to_owned()
                } else {
                    "native Windows toolchain and project directory are available".to_owned()
                }
            } else {
                "provider is present but its project directory is not verified for Windows execution"
                    .to_owned()
            },
        });
    }
    for probe in discovery.wsl {
        let host = match probe.wsl_version {
            Some(1) => ProviderHost::Wsl1,
            Some(2) => ProviderHost::Wsl2,
            _ => continue,
        };
        let raw_available = probe.executable.is_some();
        let rtk_fallback = (command_surface(tool) == CommandSurface::NativeStructured)
            .then(|| probe.rtk.clone())
            .flatten();
        if let Some(executable) = probe.executable.clone().or(rtk_fallback) {
            let project_path = wsl_project_path(&project, &probe, user);
            let dedicated_runtime = host != ProviderHost::Wsl1 || probe.dedicated;
            let usable = project_path.is_some() && dedicated_runtime;
            candidates.push(ProviderCandidate {
                host,
                adapters: [
                    raw_available.then_some(AdapterKind::Raw),
                    probe.rtk.is_some().then_some(AdapterKind::Rtk),
                ]
                .into_iter()
                .flatten()
                .collect(),
                distro: Some(probe.distro),
                wsl_version: probe.wsl_version,
                executable,
                rtk: probe.rtk,
                project_path,
                usable,
                reason: if host == ProviderHost::Wsl1 && !probe.dedicated {
                    "WSL1 provider is not a verified XUVA-dedicated runtime".to_owned()
                } else if usable {
                    "WSL toolchain and project path mapping are available".to_owned()
                } else if project.kind == ProjectLocationKind::Windows {
                    "provider is present but Windows-to-WSL project mapping failed".to_owned()
                } else {
                    "provider is present but its project path mapping is not yet verified"
                        .to_owned()
                },
            });
        }
    }
    let recommended = candidates.iter().position(|candidate| candidate.usable);
    let diagnosis = recommended.map_or_else(
        || format!(
            "no verified provider is available for {}; run `{PRODUCT_COMMAND} setup {tool}` for a non-installing setup diagnosis",
            tool
        ),
        |index| format!(
            "candidate {index} is verified; run `{PRODUCT_COMMAND} provider exec {tool} -- <args...>` to execute it explicitly"
        ),
    );
    ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: tool.to_owned(),
        cache,
        project,
        availability,
        candidates,
        recommended,
        diagnosis,
        install: "disabled_in_p12",
    }
}

fn verified_wsl_executable_path(path: String) -> Option<String> {
    path.starts_with('/').then_some(path)
}

fn windows_provider_has_compatible_semantics(tool: &str, adapter: AdapterKind) -> bool {
    match adapter {
        AdapterKind::Raw => !matches!(
            tool,
            "awk" | "cat" | "find" | "grep" | "head" | "ls" | "sed" | "tail" | "tree" | "wc"
        ),
        AdapterKind::Rtk => true,
    }
}

fn windows_probe_has_compatible_provider(tool: &str, windows: &WindowsToolProbe) -> bool {
    (windows.executable.is_some()
        && windows_provider_has_compatible_semantics(tool, AdapterKind::Raw))
        || (windows.native_rtk.is_some()
            && windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk))
}

fn requires_raw_posix_provider(tool: &str) -> bool {
    matches!(
        tool,
        "awk" | "cat" | "find" | "head" | "ls" | "sed" | "tail" | "tree" | "wc"
    )
}

enum ProviderDispatchDecision {
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

fn is_dispatchable_provider_tool(arguments: &[OsString]) -> bool {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(is_safe_provider_tool_name)
}

fn looks_like_explicit_executable(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with(".\\")
        || value.starts_with("..\\")
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'\\' | b'/'))
}

fn wsl_executable_exists(distro: &str, user: Option<&str>, path: &str) -> bool {
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

fn explicit_executable_plan(
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

fn windows_tool_is_usable(
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

fn provider_dispatch_decision(
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

fn provider_dispatch_decision_from_resolution(
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

fn print_provider_resolution(
    resolution: &ProviderResolution,
    json: bool,
    doctor: bool,
) -> ExitCode {
    if json {
        let mut report = match serde_json::to_value(resolution) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("xuva: unable to render provider resolution: {error}");
                return ExitCode::FAILURE;
            }
        };
        if resolution.tool == "git"
            && let Some(object) = report.as_object_mut()
        {
            object.insert(
                "routing_health".to_owned(),
                serde_json::json!({
                    "ntfs_mutations": "windows-native-git",
                    "network_mutations": "windows-native-git",
                    "wsl_fallback": "read-only and pre-start failures only"
                }),
            );
        }
        return match serde_json::to_string_pretty(&report) {
            Ok(rendered) => {
                println!("{rendered}");
                if doctor && resolution.recommended.is_none() {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(error) => {
                eprintln!("xuva: unable to render provider resolution: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("tool={}", resolution.tool);
    println!("cache={}", resolution.cache);
    println!("project_kind={:?}", resolution.project.kind);
    println!("project_path={}", resolution.project.path);
    if resolution.tool == "git" {
        println!("git_ntfs_mutations=windows-native-git");
        println!("git_network_mutations=windows-native-git");
        println!("git_wsl_fallback=read-only-and-pre-start-only");
    }
    if let Some(distro) = &resolution.project.distro {
        println!("project_distro={distro}");
    }
    println!(
        "windows_{}_path={}",
        resolution.tool,
        resolution
            .availability
            .windows
            .executable
            .as_deref()
            .unwrap_or("missing")
    );
    println!(
        "windows_rtk_path={}",
        resolution
            .availability
            .windows
            .native_rtk
            .as_deref()
            .unwrap_or("missing")
    );
    println!(
        "windows_{}_identity={};version={};probe_status={:?};capabilities={}",
        resolution.tool,
        binary_identity_display(resolution.availability.windows.executable_identity.as_ref()),
        resolution
            .availability
            .windows
            .executable_version
            .as_deref()
            .unwrap_or("unknown"),
        resolution.availability.windows.version_probe_status,
        resolution
            .availability
            .windows
            .executable_capabilities
            .join(","),
    );
    println!(
        "windows_rtk_identity={}",
        binary_identity_display(resolution.availability.windows.native_rtk_identity.as_ref())
    );
    for probe in &resolution.availability.wsl {
        println!(
            "inspected_distro={};user={};wsl_version={};dedicated={};installation_id={};{}_path={};{}_identity={};version={};probe_status={:?};capabilities={};rtk_path={};rtk_identity={}",
            probe.distro,
            probe.user.as_deref().unwrap_or("default"),
            probe
                .wsl_version
                .map_or_else(|| "unknown".to_owned(), |version| version.to_string()),
            probe.dedicated,
            probe.installation_id.as_deref().unwrap_or("none"),
            resolution.tool,
            probe.executable.as_deref().unwrap_or("missing"),
            resolution.tool,
            binary_identity_display(probe.executable_identity.as_ref()),
            probe.executable_version.as_deref().unwrap_or("unknown"),
            probe.version_probe_status,
            probe.executable_capabilities.join(","),
            probe.rtk.as_deref().unwrap_or("missing"),
            binary_identity_display(probe.rtk_identity.as_ref())
        );
    }
    if resolution.candidates.is_empty() {
        println!("recommended=none");
        if doctor {
            println!("diagnosis={}", resolution.diagnosis);
        }
        println!("install={}", resolution.install);
        return if doctor {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    for (index, candidate) in resolution.candidates.iter().enumerate() {
        println!(
            "candidate_{index}={:?};adapters={:?};distro={};usable={};executable={};reason={}",
            candidate.host,
            candidate.adapters,
            candidate.distro.as_deref().unwrap_or("windows"),
            candidate.usable,
            candidate.executable,
            candidate.reason
        );
        if let Some(project_path) = &candidate.project_path {
            println!("candidate_{index}_project_path={project_path}");
        }
    }
    println!(
        "recommended={}",
        resolution
            .recommended
            .map_or_else(|| "none".to_owned(), |index| index.to_string())
    );
    if doctor {
        println!("diagnosis={}", resolution.diagnosis);
    }
    println!("install={}", resolution.install);
    if doctor && resolution.recommended.is_none() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn binary_identity_display(identity: Option<&BinaryIdentity>) -> String {
    identity.map_or_else(
        || "missing".to_owned(),
        |identity| {
            format!(
                "{}:{}:{}",
                identity.path, identity.size_bytes, identity.modified_unix_seconds
            )
        },
    )
}

fn is_safe_provider_tool_name(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 128
        && tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn windows_path_tool_names() -> Vec<String> {
    let mut tools = HashSet::new();
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path) {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_windows_launchable_path(&path.to_string_lossy()) {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|name| name.to_str())
                && is_safe_provider_tool_name(name)
            {
                tools.insert(name.to_ascii_lowercase());
            }
        }
    }
    let mut tools: Vec<_> = tools.into_iter().collect();
    tools.sort_unstable();
    tools
}

fn provider_command(arguments: &[OsString], config: &Config, doctor: bool) -> ExitCode {
    let Some(tool) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        eprintln!(
            "xuva: usage: {} <tool> [--json] [--refresh]",
            if doctor {
                DOCTOR_ARGUMENT
            } else {
                RESOLVE_ARGUMENT
            }
        );
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) || arguments.len() > 4 {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let json = arguments
        .iter()
        .skip(2)
        .any(|argument| argument == "--json");
    let refresh = arguments
        .iter()
        .skip(2)
        .any(|argument| argument == "--refresh");
    if arguments
        .iter()
        .skip(2)
        .any(|argument| argument != "--json" && argument != "--refresh")
    {
        eprintln!(
            "xuva: usage: {} <tool> [--json] [--refresh]",
            if doctor {
                DOCTOR_ARGUMENT
            } else {
                RESOLVE_ARGUMENT
            }
        );
        return ExitCode::FAILURE;
    }
    print_provider_resolution(
        &resolve_tool_provider_with_inspection(tool, config, refresh, doctor || refresh),
        json,
        doctor,
    )
}

fn provider_scan_command(arguments: &[OsString], config: &Config) -> ExitCode {
    if arguments.len() == 1 {
        let windows_tools = windows_path_tool_names();
        let wsl_distros = installed_wsl_distributions()
            .into_iter()
            .map(|(distro, version)| format!("{distro}:{}", version.unwrap_or_default()))
            .collect::<Vec<_>>();
        println!("scan=complete; windows_tools={}", windows_tools.len());
        println!(
            "wsl_distros={}",
            if wsl_distros.is_empty() {
                "none".to_owned()
            } else {
                wsl_distros.join(",")
            }
        );
        println!(
            "provider_cache=on-demand; use `{PRODUCT_COMMAND} scan <tool>...` to refresh named providers"
        );
        return ExitCode::SUCCESS;
    }

    let requested_tools: Vec<&str> = {
        let mut tools = Vec::new();
        for argument in arguments.iter().skip(1) {
            let Some(tool) = argument
                .to_str()
                .filter(|tool| is_safe_provider_tool_name(tool))
            else {
                eprintln!(
                    "xuva: usage: scan [<tool>...]; tool names must contain only ASCII letters, digits, '.', '_', or '-'"
                );
                return ExitCode::FAILURE;
            };
            if !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        tools
    };

    for tool in &requested_tools {
        let resolution = resolve_tool_provider(tool, config, true);
        let recommended = resolution
            .recommended
            .and_then(|index| resolution.candidates.get(index))
            .map_or("missing", |candidate| candidate.host.as_str());
        println!(
            "tool={tool}; cache={}; candidates={}; recommended={recommended}",
            resolution.cache,
            resolution.candidates.len()
        );
    }
    println!("scan=complete; tools={}", requested_tools.len());
    ExitCode::SUCCESS
}

fn has_complete_go_provider(resolution: &ProviderResolution) -> bool {
    if resolution.project.kind != ProjectLocationKind::Wsl
        && resolution.availability.windows.executable.is_some()
    {
        return true;
    }
    resolution.candidates.iter().any(|candidate| {
        candidate.usable && candidate.is_wsl() && candidate.has_consistent_location()
    })
}

fn setup_go_plan_from_resolution(
    resolution: &ProviderResolution,
    winget_available: bool,
) -> SetupPlan {
    let verification_command = vec![
        "xuva".to_owned(),
        "doctor".to_owned(),
        "go".to_owned(),
        "--refresh".to_owned(),
    ];
    if has_complete_go_provider(resolution) {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "ready",
            reason: "a complete existing Go provider is already available; no setup is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    if resolution.project.kind == ProjectLocationKind::Windows
        && resolution.availability.windows.native_rtk.is_some()
        && winget_available
    {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "planned",
            reason: "Windows Go is absent while native RTK is already available".to_owned(),
            proposed_provider: Some("windows-winget"),
            proposed_command: Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ]),
            verification_command,
            apply: "unavailable_in_pd4",
        };
    }
    let reason = if resolution.project.kind == ProjectLocationKind::Wsl {
        "no complete provider is available for this WSL project; PD4 will not install a Windows toolchain across hosts".to_owned()
    } else if resolution.availability.windows.native_rtk.is_none() {
        "Windows Go setup is blocked because a verified native RTK provider is also required and is not available".to_owned()
    } else {
        "Windows Go setup is blocked because winget is unavailable; no alternate installer is selected automatically".to_owned()
    };
    SetupPlan {
        schema_version: 1,
        tool: "go".to_owned(),
        mode: "plan-only",
        status: "blocked",
        reason,
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_in_pd4",
    }
}

fn setup_generic_plan_from_resolution(resolution: &ProviderResolution) -> SetupPlan {
    let verification_command = vec![
        "xuva".to_owned(),
        "doctor".to_owned(),
        resolution.tool.clone(),
        "--refresh".to_owned(),
    ];
    if resolution.recommended.is_some() {
        return SetupPlan {
            schema_version: 1,
            tool: resolution.tool.clone(),
            mode: "diagnostic-only",
            status: "ready",
            reason: "a verified existing provider is available; no setup action is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    SetupPlan {
        schema_version: 1,
        tool: resolution.tool.clone(),
        mode: "diagnostic-only",
        status: "blocked",
        reason: format!(
            "{}; XUVA will not guess an installer, package manager, or dependency chain for a generic tool",
            resolution.diagnosis
        ),
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_for_generic_tool",
    }
}

fn print_setup_plan(plan: &SetupPlan, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(plan) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("xuva: unable to render setup plan: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("tool={}", plan.tool);
    println!("mode={}", plan.mode);
    println!("status={}", plan.status);
    println!("reason={}", plan.reason);
    if let Some(provider) = plan.proposed_provider {
        println!("proposed_provider={provider}");
    }
    if let Some(command) = &plan.proposed_command {
        println!("proposed_command={}", command.join(" "));
    }
    println!(
        "verification_command={}",
        plan.verification_command.join(" ")
    );
    println!("apply={}", plan.apply);
    ExitCode::SUCCESS
}

fn setup_transaction_path() -> PathBuf {
    xuva_data_root().join("setup-transaction-v1.json")
}

fn load_setup_transaction() -> Option<SetupTransaction> {
    fs::read_to_string(setup_transaction_path())
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

fn write_setup_transaction(transaction: &SetupTransaction) -> Result<(), String> {
    let destination = setup_transaction_path();
    let encoded = serde_json::to_string_pretty(transaction)
        .map_err(|error| format!("unable to encode setup transaction: {error}"))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create setup transaction directory: {error}"))?;
    }
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write setup transaction: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate setup transaction: {error}"))
}

fn print_setup_transaction(transaction: Option<&SetupTransaction>, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(&transaction) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("xuva: unable to render setup transaction: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match transaction {
        Some(transaction) => {
            println!("tool={}", transaction.tool);
            println!("status={}", transaction.status);
            println!(
                "observed_unix_seconds={}",
                transaction.observed_unix_seconds
            );
            println!("detail={}", transaction.detail);
            if let Some(command) = &transaction.command {
                println!("command={}", command.join(" "));
            }
        }
        None => println!("No local setup transaction is recorded."),
    }
    ExitCode::SUCCESS
}

fn record_setup_transaction(
    status: &str,
    command: Option<Vec<String>>,
    detail: impl Into<String>,
) -> Result<SetupTransaction, String> {
    let transaction = SetupTransaction {
        schema_version: 1,
        tool: "go".to_owned(),
        status: status.to_owned(),
        observed_unix_seconds: unix_seconds(),
        command,
        detail: detail.into(),
    };
    write_setup_transaction(&transaction)?;
    Ok(transaction)
}

fn setup_recovery_outcome(has_complete_provider: bool) -> (&'static str, &'static str) {
    if has_complete_provider {
        (
            "recovered_verified",
            "fresh provider discovery found a complete Go provider; no installer was replayed",
        )
    } else {
        (
            "recovery_required",
            "fresh provider discovery is still incomplete; no installer was replayed and manual review is required",
        )
    }
}

fn recover_setup_transaction(config: &Config, json: bool) -> ExitCode {
    let Some(previous) = load_setup_transaction() else {
        return print_setup_transaction(None, json);
    };
    let resolution = resolve_tool_provider("go", config, true);
    let (status, detail) = setup_recovery_outcome(has_complete_go_provider(&resolution));
    let recovered = match record_setup_transaction(status, previous.command, detail) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("xuva: {error}");
            return ExitCode::FAILURE;
        }
    };
    print_setup_transaction(Some(&recovered), json)
}

fn apply_setup_plan(plan: &SetupPlan, config: &Config, json: bool) -> ExitCode {
    if plan.status == "ready" {
        return print_setup_plan(plan, json);
    }
    let Some(command) = plan.proposed_command.clone() else {
        eprintln!("xuva: setup is blocked; no installer is selected automatically");
        return ExitCode::FAILURE;
    };
    if let Err(error) = record_setup_transaction(
        "running",
        Some(command.clone()),
        "installer started after explicit --apply --confirm",
    ) {
        eprintln!("xuva: {error}");
        return ExitCode::FAILURE;
    }
    let mut installer = Command::new(&command[0]);
    installer.args(&command[1..]);
    let status = match installer.status() {
        Ok(status) => status,
        Err(error) => {
            let detail = format!("installer could not start: {error}");
            let _ = record_setup_transaction("failed", Some(command), &detail);
            eprintln!("xuva: {detail}");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        let detail = format!("installer exited with {status}");
        let _ = record_setup_transaction("failed", Some(command), &detail);
        eprintln!(
            "xuva: {detail}; run `xuva setup go --recover` to re-discover without replaying it"
        );
        return ExitCode::FAILURE;
    }
    let resolution = resolve_tool_provider("go", config, true);
    if has_complete_go_provider(&resolution) {
        let transaction = match record_setup_transaction(
            "verified",
            Some(command),
            "installer completed and fresh provider discovery found a complete Go provider",
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::FAILURE;
            }
        };
        return print_setup_transaction(Some(&transaction), json);
    }
    let detail = "installer completed but fresh provider discovery is incomplete; reopen the shell if PATH changed, then run `xuva setup go --recover`";
    let _ = record_setup_transaction("verification_required", Some(command), detail);
    eprintln!("xuva: {detail}");
    ExitCode::FAILURE
}

fn setup_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        eprintln!(
            "xuva: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let flags: Vec<&str> = match arguments
        .iter()
        .skip(2)
        .map(|argument| argument.to_str())
        .collect()
    {
        Some(flags) => flags,
        None => {
            eprintln!("xuva: setup options must be valid Unicode");
            return ExitCode::FAILURE;
        }
    };
    let valid = [
        "--json",
        "--refresh",
        "--status",
        "--recover",
        "--apply",
        "--confirm",
    ];
    if flags.iter().any(|flag| !valid.contains(flag)) {
        eprintln!(
            "xuva: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    let json = flags.contains(&"--json");
    let refresh = flags.contains(&"--refresh");
    let status = flags.contains(&"--status");
    let recover = flags.contains(&"--recover");
    let apply = flags.contains(&"--apply");
    let confirm = flags.contains(&"--confirm");
    if tool != "go" {
        if status || recover || apply || confirm {
            eprintln!(
                "xuva: generic setup is diagnostic-only; `--apply`, `--confirm`, `--status`, and `--recover` are available only for the explicit Go transaction"
            );
            return ExitCode::FAILURE;
        }
        let resolution = resolve_tool_provider(tool, config, refresh);
        return print_setup_plan(&setup_generic_plan_from_resolution(&resolution), json);
    }
    if [status, recover, apply]
        .into_iter()
        .filter(|selected| *selected)
        .count()
        > 1
        || (confirm && !apply)
        || (status && refresh)
    {
        eprintln!(
            "xuva: usage: setup go [--json] [--refresh] [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    if status {
        return print_setup_transaction(load_setup_transaction().as_ref(), json);
    }
    if recover {
        return recover_setup_transaction(config, json);
    }
    let resolution = resolve_tool_provider(tool, config, refresh || apply);
    let mut plan =
        setup_go_plan_from_resolution(&resolution, first_windows_executable("winget").is_some());
    if plan.status == "planned" {
        plan.apply = "requires_apply_and_confirm";
    }
    if !apply {
        return print_setup_plan(&plan, json);
    }
    if !confirm {
        eprintln!(
            "xuva: review the plan above; re-run with `xuva setup go --apply --confirm` to start the installer"
        );
        let _ = print_setup_plan(&plan, json);
        return ExitCode::from(2);
    }
    apply_setup_plan(&plan, config, json)
}

fn policy_path() -> PathBuf {
    env::var_os("XUVA_POLICY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| xuva_data_root().join("route-policy-v2.json"))
}

fn load_route_policy() -> Option<RoutePolicyFile> {
    let path = policy_path();
    let contents = fs::read_to_string(path).ok()?;
    let policy = serde_json::from_str(&contents).ok()?;
    validate_route_policy(&policy).ok()?;
    Some(policy)
}

fn validate_route_policy(policy: &RoutePolicyFile) -> Result<(), String> {
    if policy.schema_version != ROUTE_POLICY_SCHEMA_VERSION
        || policy.manifest_version != adapter_contract_id()
        || policy.context_signature.len() != 16
        || policy.evidence.is_empty()
    {
        return Err("policy evidence must use the current schema, manifest, context, and non-empty evidence".to_owned());
    }
    let mut keys = HashSet::new();
    for evidence in &policy.evidence {
        if evidence.key.trim().is_empty()
            || evidence.sample_count == 0
            || !evidence.raw_median_ms.is_finite()
            || !evidence.candidate_median_ms.is_finite()
            || !evidence.token_savings_percent.is_finite()
            || evidence.raw_median_ms < 0.0
            || evidence.candidate_median_ms < 0.0
        {
            return Err("policy evidence contains an invalid measurement".to_owned());
        }
        if !keys.insert(&evidence.key) {
            return Err(format!(
                "policy evidence contains duplicate key {}",
                evidence.key
            ));
        }
    }
    Ok(())
}

fn merge_route_policy(
    existing: Option<RoutePolicyFile>,
    incoming: RoutePolicyFile,
) -> RoutePolicyFile {
    let RoutePolicyFile {
        manifest_version,
        context_signature,
        evidence: incoming_evidence,
        ..
    } = incoming;
    let mut evidence = existing.map_or_else(Vec::new, |policy| policy.evidence);
    for next in incoming_evidence {
        if let Some(index) = evidence.iter().position(|current| current.key == next.key) {
            evidence[index] = next;
        } else {
            evidence.push(next);
        }
    }
    evidence.sort_by(|left, right| left.key.cmp(&right.key));
    RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version,
        context_signature,
        evidence,
    }
}

fn import_route_policy(source: &Path, config: &Config) -> Result<(), String> {
    let contents = fs::read_to_string(source)
        .map_err(|error| format!("unable to read policy evidence: {error}"))?;
    let incoming: RoutePolicyFile = serde_json::from_str(&contents)
        .map_err(|error| format!("invalid policy evidence: {error}"))?;
    validate_route_policy(&incoming)?;
    let expected_context = adaptive_context_signature(config);
    if incoming.context_signature != expected_context {
        return Err("policy evidence was measured for a different local adapter context; run `xuva policy context` and re-benchmark".to_owned());
    }
    let destination = policy_path();
    let existing = if destination.exists() {
        let contents = fs::read_to_string(&destination)
            .map_err(|error| format!("unable to read existing route policy: {error}"))?;
        let policy = serde_json::from_str(&contents)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        validate_route_policy(&policy)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        if policy.context_signature != incoming.context_signature {
            return Err("existing policy belongs to a different local adapter context; remove or relocate it before importing new evidence".to_owned());
        }
        Some(policy)
    } else {
        None
    };
    let merged = merge_route_policy(existing, incoming);
    let encoded = serde_json::to_string_pretty(&merged)
        .map_err(|error| format!("unable to encode merged route policy: {error}"))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create policy directory: {error}"))?;
    }
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write policy evidence: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate policy evidence: {error}"))
}

fn calibration_path() -> PathBuf {
    xuva_data_root().join("calibration-v3.json")
}

fn validate_calibration(file: &CalibrationFile) -> Result<(), String> {
    if file.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }
    let mut signatures = HashSet::new();
    for entry in &file.entries {
        if entry.signature.len() != 16
            || entry.key.trim().is_empty()
            || entry.manifest_version != adapter_contract_id()
            || entry.context_signature.len() != 16
            || !entry
                .raw_samples_ms
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0)
            || !entry.native_samples.iter().all(|sample| {
                sample.elapsed_ms.is_finite()
                    && sample.elapsed_ms >= 0.0
                    && sample.input_tokens >= 0
                    && sample.saved_tokens >= 0
                    && sample.saved_tokens <= sample.input_tokens
            })
            || !signatures.insert(&entry.signature)
        {
            return Err("calibration state contains invalid local evidence".to_owned());
        }
    }
    Ok(())
}

fn calibration_for_current_contract(mut file: CalibrationFile) -> Result<CalibrationFile, String> {
    if file.schema_version < CALIBRATION_SCHEMA_VERSION {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
    if file.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }

    // Adapter upgrades intentionally invalidate measurements made against the
    // previous contract. They are stale evidence, not corrupt user state.
    file.entries
        .retain(|entry| entry.manifest_version == adapter_contract_id());
    validate_calibration(&file)?;
    Ok(file)
}

fn load_calibration() -> Result<CalibrationFile, String> {
    let path = calibration_path();
    if !path.exists() {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("unable to read local calibration state: {error}"))?;
    let file: CalibrationFile = serde_json::from_str(&contents)
        .map_err(|error| format!("local calibration state is invalid: {error}"))?;
    calibration_for_current_contract(file)
}

fn save_calibration(file: &CalibrationFile) -> Result<(), String> {
    validate_calibration(file)?;
    let destination = calibration_path();
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create calibration directory: {error}"))?;
    }
    let encoded = serde_json::to_string_pretty(file)
        .map_err(|error| format!("unable to encode local calibration state: {error}"))?;
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write local calibration state: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate local calibration state: {error}"))
}

fn calibration_key(arguments: &[OsString]) -> Option<&'static str> {
    match command_family(arguments) {
        "git" if is_verified_read_only_git(arguments) => Some("git:read-only"),
        "rg" => Some("rg"),
        "npm" if is_verified_npm_run_list_operation(arguments) => Some("npm:run-list"),
        "go" if is_verified_go_test_all_operation(arguments) => Some("go:test-all"),
        _ => None,
    }
}

fn calibration_entry_matches(
    entry: &CalibrationEntry,
    signature: &str,
    context_signature: &str,
) -> bool {
    entry.signature == signature
        && entry.manifest_version == adapter_contract_id()
        && entry.context_signature == context_signature
}

fn calibration_route_for(
    entry: Option<&CalibrationEntry>,
    objective: PolicyObjective,
) -> (Route, &'static str) {
    let (route, reason) = match entry {
        None => (
            Route::NativeRtk,
            "local calibration candidate: first safe observation uses native RTK",
        ),
        Some(entry) if entry.raw_samples_ms.is_empty() => (
            Route::Raw,
            "local calibration candidate: second safe observation uses raw execution",
        ),
        Some(entry) if entry.native_samples.len() < 2 => (
            Route::NativeRtk,
            "local calibration candidate: third safe observation confirms native RTK",
        ),
        Some(entry) if entry.raw_samples_ms.len() < 2 => {
            let selected = entry.selected_route(objective);
            if entry.raw_samples_ms.len() == 1 && entry.native_samples.len() == 2 {
                (
                    selected,
                    "local calibration provisional choice; validating with one further natural invocation",
                )
            } else {
                (
                    Route::Raw,
                    "local calibration validation samples raw execution before marking a stable route",
                )
            }
        }
        Some(entry) => {
            let selected = entry.selected_route(objective);
            (
                selected,
                if selected == Route::Raw {
                    "local calibration selected stable lower-latency raw execution"
                } else {
                    "local calibration selected stable token-saving native RTK"
                },
            )
        }
    };
    (route, reason)
}

fn cap_samples<T>(samples: &mut Vec<T>) {
    if samples.len() > CALIBRATION_MAX_SAMPLES {
        let excess = samples.len() - CALIBRATION_MAX_SAMPLES;
        samples.drain(0..excess);
    }
}

fn record_calibration(
    plan: &CalibrationPlan,
    executed_route: Route,
    elapsed: Duration,
    exit_code: i32,
    totals: TokenTotals,
) -> Result<(), String> {
    if exit_code != 0 || !matches!(executed_route, Route::Raw | Route::NativeRtk) {
        return Ok(());
    }
    let mut state = load_calibration()?;
    let entry = match state
        .entries
        .iter()
        .position(|entry| entry.signature == plan.signature)
    {
        Some(index)
            if state.entries[index].manifest_version == plan.manifest_version
                && state.entries[index].context_signature == plan.context_signature =>
        {
            &mut state.entries[index]
        }
        Some(index) => {
            state.entries[index] = CalibrationEntry {
                signature: plan.signature.clone(),
                key: plan.key.clone(),
                manifest_version: plan.manifest_version.clone(),
                context_signature: plan.context_signature.clone(),
                raw_samples_ms: Vec::new(),
                native_samples: Vec::new(),
            };
            &mut state.entries[index]
        }
        None => {
            state.entries.push(CalibrationEntry {
                signature: plan.signature.clone(),
                key: plan.key.clone(),
                manifest_version: plan.manifest_version.clone(),
                context_signature: plan.context_signature.clone(),
                raw_samples_ms: Vec::new(),
                native_samples: Vec::new(),
            });
            state.entries.last_mut().expect("entry was just appended")
        }
    };
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    match executed_route {
        Route::Raw => {
            entry.raw_samples_ms.push(elapsed_ms);
            cap_samples(&mut entry.raw_samples_ms);
        }
        Route::NativeRtk => {
            entry.native_samples.push(NativeCalibrationSample {
                elapsed_ms,
                input_tokens: totals.input_tokens,
                saved_tokens: totals.saved_tokens,
            });
            cap_samples(&mut entry.native_samples);
        }
        Route::Wsl1 | Route::Wsl2 | Route::Auto => unreachable!("route was filtered above"),
    }
    save_calibration(&state)
}

fn print_calibration(objective: PolicyObjective) -> Result<(), String> {
    let state = load_calibration()?;
    if state.entries.is_empty() {
        println!("No local adaptive calibration evidence is recorded.");
        return Ok(());
    }
    println!("XUVA Local Adaptive Calibration");
    println!();
    for entry in &state.entries {
        let route = entry.selected_route(objective);
        println!("key={}", entry.key);
        println!("signature={}", entry.signature);
        println!("phase={}", entry.phase());
        println!("route={}", route.as_str());
        println!("raw_samples={}", entry.raw_samples_ms.len());
        println!("native_samples={}", entry.native_samples.len());
        println!(
            "native_token_savings_percent={:.1}",
            entry.token_savings_percent()
        );
        println!();
    }
    Ok(())
}

fn is_wsl_path(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

fn git_uses_wsl_directory(arguments: &[OsString]) -> bool {
    arguments.windows(2).any(|pair| {
        (pair[0] == "-C" || pair[0] == "--git-dir" || pair[0] == "--work-tree")
            && is_wsl_path(&pair[1])
    })
}

fn should_use_native_git(
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

fn forwarded_rtk_arguments(arguments: Vec<OsString>) -> Vec<OsString> {
    let mut forwarded = arguments;
    if forwarded
        .first()
        .is_some_and(|argument| argument == "stats")
    {
        forwarded[0] = OsString::from("gain");
    }
    forwarded
}

fn wsl_launch_prefix(config: &Config) -> Vec<OsString> {
    let mut command = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        command.extend([OsString::from("-u"), OsString::from(user)]);
    }
    let working_directory = config.cwd.clone().or_else(|| {
        env::current_dir().ok().and_then(|current_directory| {
            windows_path_to_wsl_path(&current_directory.to_string_lossy())
        })
    });
    if let Some(wsl_directory) = working_directory {
        command.extend([OsString::from("--cd"), OsString::from(wsl_directory)]);
    }
    command
}

fn test_ready_wsl_path() -> Option<String> {
    env::var("XUVA_WSL_TEST_READY_FILE")
        .ok()
        .and_then(|path| windows_path_to_wsl_path(&path))
}

fn test_wsl1_attestation_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL1_ATTESTATION_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 5)
        .unwrap_or(0)
}

fn test_wsl1_marker_path() -> String {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return String::new();
    }
    env::var("XUVA_TEST_WSL1_MARKER_PATH")
        .ok()
        .filter(|path| {
            path.starts_with('/')
                && !path.contains(['\0', '\r', '\n'])
                && path.split('/').all(|part| !matches!(part, "." | ".."))
        })
        .unwrap_or_default()
}

fn test_wsl2_launch_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL2_LAUNCH_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 5)
        .unwrap_or(0)
}

fn test_completion_status_override() -> Option<u8> {
    (env::var("XUVA_TEST_MODE").as_deref() == Ok("1"))
        .then(|| {
            env::var("XUVA_TEST_COMPLETION_STATUS_OVERRIDE")
                .ok()
                .and_then(|value| value.parse::<u8>().ok())
        })
        .flatten()
}

fn test_kill_wsl1_proxy_after_permit() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL1_PROXY_AFTER_PERMIT").as_deref() == Ok("1")
}

fn test_ready_file_exists() -> bool {
    env::var_os("XUVA_WSL_TEST_READY_FILE").is_some_and(|path| PathBuf::from(path).is_file())
}

fn test_kill_wsl2_proxy_during_cancellation() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL2_PROXY_DURING_CANCEL").as_deref() == Ok("1")
}

fn test_defer_wsl2_proxy_reap_until_cleanup() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_DEFER_WSL2_PROXY_REAP_UNTIL_CLEANUP").as_deref() == Ok("1")
}

#[cfg(test)]
fn rtk_arguments(arguments: Vec<OsString>, config: &Config, cancel_nonce: &str) -> Vec<OsString> {
    rtk_arguments_with_metrics(
        arguments,
        config,
        cancel_nonce,
        None,
        "/tmp/xuva-test.attestation",
        "/tmp/xuva-test.permit",
        "/tmp/xuva-test.completion",
    )
}

fn rtk_arguments_with_metrics(
    arguments: Vec<OsString>,
    config: &Config,
    cancel_nonce: &str,
    metrics_db_path: Option<&str>,
    attestation_path: &str,
    permit_path: &str,
    completion_path: &str,
) -> Vec<OsString> {
    let forwarded = forwarded_rtk_arguments(arguments);
    let mut command = wsl_launch_prefix(config);
    command.extend([
        OsString::from("--exec"),
        OsString::from("/usr/bin/setsid"),
        OsString::from("-w"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(LAUNCH_SCRIPT),
        OsString::from("xuva"),
        OsString::from(&config.lock_wait),
        OsString::from(&config.lock_path),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
        OsString::from(cancel_nonce),
        OsString::from(metrics_db_path.unwrap_or("")),
        OsString::from(config.extra_path.as_deref().unwrap_or("")),
        OsString::from(test_ready_wsl_path().unwrap_or_default()),
        OsString::from(attestation_path),
        OsString::from(permit_path),
        OsString::from(completion_path),
        OsString::from(test_wsl2_launch_delay_seconds().to_string()),
        OsString::from(
            test_completion_status_override().map_or_else(String::new, |value| value.to_string()),
        ),
    ]);
    command.extend(forwarded);
    command
}

#[cfg(test)]
fn wsl1_rtk_arguments(arguments: Vec<OsString>, config: &Config) -> Vec<OsString> {
    wsl1_rtk_arguments_with_metrics(
        arguments,
        config,
        None,
        "/tmp/xuva-test.attestation",
        "/tmp/xuva-test.permit",
        "/tmp/xuva-test.completion",
    )
}

fn wsl1_rtk_arguments_with_metrics(
    arguments: Vec<OsString>,
    config: &Config,
    metrics_db_path: Option<&str>,
    attestation_path: &str,
    permit_path: &str,
    completion_path: &str,
) -> Vec<OsString> {
    let forwarded = forwarded_rtk_arguments(arguments);
    let mut command = wsl_launch_prefix(config);
    command.extend([
        OsString::from("--exec"),
        OsString::from("/usr/bin/setsid"),
        OsString::from("-w"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(WSL1_LAUNCH_SCRIPT),
        OsString::from("xuva-wsl1"),
        OsString::from(metrics_db_path.unwrap_or("")),
        OsString::from(config.extra_path.as_deref().unwrap_or("")),
        OsString::from(test_ready_wsl_path().unwrap_or_default()),
        OsString::from(attestation_path),
        OsString::from(permit_path),
        OsString::from(completion_path),
        OsString::from(test_wsl1_attestation_delay_seconds().to_string()),
        OsString::from(WSL1_MARKER_VALIDATOR_SCRIPT),
        OsString::from(
            test_completion_status_override().map_or_else(String::new, |value| value.to_string()),
        ),
        OsString::from(test_wsl1_marker_path()),
        OsString::from(config.rtk_path.as_deref().unwrap_or("@default-rtk@")),
    ]);
    command.extend(forwarded);
    command
}

fn wsl_environment_assignments(
    environment: &[(OsString, OsString)],
) -> Result<Vec<OsString>, std::io::Error> {
    environment
        .iter()
        .map(|(key, value)| {
            let key = key.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable names must be valid Unicode",
                )
            })?;
            let value = value.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable values must be valid Unicode",
                )
            })?;
            let valid_name = key.bytes().enumerate().all(|(index, byte)| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
                b'0'..=b'9' => index > 0,
                _ => false,
            });
            if key.is_empty() || !valid_name {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable names must use POSIX identifier syntax",
                ));
            }
            Ok(OsString::from(format!("{key}={value}")))
        })
        .collect()
}

#[derive(Clone, Copy)]
struct WslLaunchMetadata<'a> {
    cancel_nonce: Option<&'a str>,
    metrics_db_path: Option<&'a str>,
    attestation_path: Option<&'a str>,
    permit_path: Option<&'a str>,
    completion_path: Option<&'a str>,
}

fn plan_wsl_arguments_with_metrics(
    executable: &OsString,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    metadata: WslLaunchMetadata<'_>,
) -> Result<Vec<OsString>, std::io::Error> {
    let environment = wsl_environment_assignments(environment)?;
    let mut command = wsl_launch_prefix(config);
    match route {
        Route::Wsl1 => {
            let attestation_path = metadata.attestation_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL1 execution plans require a dedicated-runtime attestation path",
                )
            })?;
            let permit_path = metadata.permit_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL1 execution plans require a parent launch-permit path",
                )
            })?;
            let completion_path = metadata.completion_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL1 execution plans require a completion-attestation path",
                )
            })?;
            command.extend([
                OsString::from("--exec"),
                OsString::from("/usr/bin/setsid"),
                OsString::from("-w"),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(WSL1_LAUNCH_SCRIPT),
                OsString::from("xuva-wsl1-plan"),
                OsString::from(metadata.metrics_db_path.unwrap_or("")),
                OsString::from(config.extra_path.as_deref().unwrap_or("")),
                OsString::from(test_ready_wsl_path().unwrap_or_default()),
                OsString::from(attestation_path),
                OsString::from(permit_path),
                OsString::from(completion_path),
                OsString::from(test_wsl1_attestation_delay_seconds().to_string()),
                OsString::from(WSL1_MARKER_VALIDATOR_SCRIPT),
                OsString::from(
                    test_completion_status_override()
                        .map_or_else(String::new, |value| value.to_string()),
                ),
                OsString::from(test_wsl1_marker_path()),
                OsString::new(),
            ]);
        }
        Route::Wsl2 => {
            let cancel_nonce = metadata.cancel_nonce.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL2 execution plans require a cancellation token",
                )
            })?;
            let attestation_path = metadata.attestation_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL2 execution plans require a cancellation-token attestation path",
                )
            })?;
            let permit_path = metadata.permit_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL2 execution plans require a parent launch-permit path",
                )
            })?;
            let completion_path = metadata.completion_path.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL2 execution plans require a completion-attestation path",
                )
            })?;
            command.extend([
                OsString::from("--exec"),
                OsString::from("/usr/bin/setsid"),
                OsString::from("-w"),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(PLAN_LAUNCH_SCRIPT),
                OsString::from("xuva-plan"),
                OsString::from(&config.lock_wait),
                OsString::from(&config.lock_path),
                OsString::from(cancel_nonce),
                OsString::from(metadata.metrics_db_path.unwrap_or("")),
                OsString::from(config.extra_path.as_deref().unwrap_or("")),
                OsString::from(test_ready_wsl_path().unwrap_or_default()),
                OsString::from(attestation_path),
                OsString::from(permit_path),
                OsString::from(completion_path),
                OsString::from(test_wsl2_launch_delay_seconds().to_string()),
                OsString::from(
                    test_completion_status_override()
                        .map_or_else(String::new, |value| value.to_string()),
                ),
            ]);
        }
        Route::Auto | Route::Raw | Route::NativeRtk => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "only WSL routes can execute a WSL plan",
            ));
        }
    }
    command.extend(environment);
    command.push(executable.clone());
    command.extend(arguments.iter().cloned());
    Ok(command)
}

#[cfg(target_os = "windows")]
fn cancellation_nonce() -> std::io::Result<String> {
    use std::ffi::c_void;

    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32)
        -> i32;
    }

    let mut bytes = [0_u8; 16];
    // SAFETY: the null algorithm handle requests the system-preferred RNG and
    // `bytes` is a valid writable buffer for the exact supplied length.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(std::io::Error::other(format!(
            "Windows secure random generation failed with NTSTATUS 0x{:08x}",
            status as u32
        )));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(target_os = "windows"))]
fn cancellation_nonce() -> std::io::Result<String> {
    use std::io::Read;

    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cancel_arguments(config: &Config, token: &str, signal: &str) -> Vec<OsString> {
    let mut command = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        command.extend([OsString::from("-u"), OsString::from(user)]);
    }
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(CANCEL_SCRIPT),
        OsString::from("xuva-cancel"),
        OsString::from(token),
        OsString::from(signal),
    ]);
    command
}

#[cfg(target_os = "windows")]
mod console {
    use std::sync::atomic::{AtomicBool, Ordering};

    static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handler(event: u32) -> i32 {
        if event == 0 || event == 1 {
            CANCEL_REQUESTED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }

    pub fn install() -> bool {
        unsafe { SetConsoleCtrlHandler(Some(handler), 1) != 0 }
    }

    pub fn uninstall() {
        unsafe { SetConsoleCtrlHandler(Some(handler), 0) };
    }

    pub fn requested() -> bool {
        CANCEL_REQUESTED.load(Ordering::SeqCst)
    }
}

#[cfg(not(target_os = "windows"))]
mod console {
    pub fn install() -> bool {
        true
    }
    pub fn uninstall() {}
    pub fn requested() -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
mod windows_lock {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x0000_0080;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const MUTEX_NAME: &str = r"Local\xuva-wsl1-global-lock";

    unsafe extern "system" {
        fn CreateMutexW(
            mutex_attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn ReleaseMutex(handle: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub struct Guard {
        handle: *mut c_void,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            super::trace(format!(
                "releasing WSL1 mutex in process {}",
                std::process::id()
            ));
            unsafe {
                ReleaseMutex(self.handle);
                CloseHandle(self.handle);
            }
        }
    }

    pub fn acquire(wait_seconds: &str) -> Result<Guard, String> {
        let name = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err("unable to create the WSL1 Windows mutex".to_owned());
        }
        let seconds = wait_seconds
            .parse::<u64>()
            .map_err(|_| "invalid WSL1 Windows mutex timeout".to_owned())?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if super::console::requested() {
                unsafe { CloseHandle(handle) };
                return Err("cancelled while waiting for the WSL1 Windows mutex".to_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                unsafe { CloseHandle(handle) };
                return Err(format!(
                    "timed out waiting for the WSL1 Windows mutex after {wait_seconds} seconds"
                ));
            }
            let milliseconds = u32::try_from(remaining.as_millis().min(50)).unwrap_or(50);
            let result = unsafe { WaitForSingleObject(handle, milliseconds) };
            match result {
                WAIT_OBJECT_0 | WAIT_ABANDONED => {
                    super::trace(format!(
                        "acquired WSL1 mutex in process {}",
                        std::process::id()
                    ));
                    return Ok(Guard { handle });
                }
                WAIT_TIMEOUT => {}
                _ => {
                    unsafe { CloseHandle(handle) };
                    return Err("unable to wait for the WSL1 Windows mutex".to_owned());
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod windows_lock {
    pub struct Guard;

    pub fn acquire(_wait_seconds: &str) -> Result<Guard, String> {
        Ok(Guard)
    }
}

fn send_linux_signal(config: &Config, token: &str, signal: &str) -> std::io::Result<bool> {
    let mut command = Command::new("wsl.exe");
    command.args(cancel_arguments(config, token, signal));
    let output = process::run_bounded(
        &mut command,
        None,
        Duration::from_secs(1),
        process::PROBE_OUTPUT_LIMIT,
    )?;
    Ok(output.status.success())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxProcessGroupState {
    Alive,
    Gone,
    TokenUnavailable,
}

fn linux_process_group_state(
    config: &Config,
    token: &str,
) -> std::io::Result<LinuxProcessGroupState> {
    let mut arguments = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        arguments.extend([OsString::from("-u"), OsString::from(user)]);
    }
    arguments.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(CANCEL_PROBE_SCRIPT),
        OsString::from("xuva-cancel-probe"),
        OsString::from(token),
    ]);
    let mut command = Command::new("wsl.exe");
    command.args(arguments);
    let output = process::run_bounded(
        &mut command,
        None,
        Duration::from_secs(1),
        process::PROBE_OUTPUT_LIMIT,
    )?;
    match output.status.code() {
        Some(0) => Ok(LinuxProcessGroupState::Alive),
        Some(1) => Ok(LinuxProcessGroupState::Gone),
        Some(4) => Ok(LinuxProcessGroupState::TokenUnavailable),
        code => Err(std::io::Error::other(format!(
            "unable to verify the Linux process group (probe exit {code:?}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

fn dedicated_wsl1_installation_id_for(distro: &str) -> Option<String> {
    let version = installed_wsl_distributions()
        .into_iter()
        .find_map(|(candidate, version)| (candidate == distro).then_some(version))
        .flatten();
    if version != Some(1) {
        return None;
    }
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        distro,
        "-u",
        "root",
        "--exec",
        "/bin/sh",
        "-c",
        WSL1_MARKER_VALIDATOR_SCRIPT,
    ]);
    let output = process::run_probe(&mut command).ok()?;
    if !output.status.success() || output.stdout_truncated {
        return None;
    }
    let rendered = decode_wsl_output(&output.stdout);
    let installation_id = rendered.trim();
    valid_installation_id(installation_id).then(|| installation_id.to_owned())
}

fn valid_installation_id(installation_id: &str) -> bool {
    installation_id.len() == 36
        && installation_id.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
}

fn require_wsl1_version(config: &Config) -> std::io::Result<()> {
    let version = installed_wsl_distributions()
        .into_iter()
        .find_map(|(distro, version)| (distro == config.distro).then_some(version))
        .flatten();
    (version == Some(1)).then_some(()).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "WSL1 route requires a version-1 distro; refusing to manage {}",
                config.distro
            ),
        )
    })
}

struct LaunchPermitGuard {
    attestation_windows_path: PathBuf,
    attestation_wsl_path: String,
    permit_windows_path: PathBuf,
    permit_wsl_path: String,
    completion_windows_path: PathBuf,
    completion_wsl_path: String,
    expected_value: Option<String>,
}

impl LaunchPermitGuard {
    fn new(label: &str, expected_value: String) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, Some(expected_value))
    }

    fn new_unbound(label: &str) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, None)
    }

    fn new_with_expected_value(
        label: &str,
        expected_value: Option<String>,
    ) -> std::io::Result<Self> {
        let nonce = cancellation_nonce()?;
        let root = env::temp_dir();
        let attestation_windows_path = root.join(format!(
            "xuva-{label}-attestation-{}-{nonce}.txt",
            std::process::id()
        ));
        let permit_windows_path = root.join(format!(
            "xuva-{label}-permit-{}-{nonce}.txt",
            std::process::id()
        ));
        let completion_windows_path = root.join(format!(
            "xuva-{label}-completion-{}-{nonce}.txt",
            std::process::id()
        ));
        let _ = fs::remove_file(&attestation_windows_path);
        let _ = fs::remove_file(&permit_windows_path);
        let _ = fs::remove_file(&completion_windows_path);
        let attestation_wsl_path = windows_path_to_wsl_path(
            &attestation_windows_path.to_string_lossy(),
        )
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the Windows temporary directory cannot be mapped into the dedicated WSL1 runtime",
            )
        })?;
        let permit_wsl_path = windows_path_to_wsl_path(&permit_windows_path.to_string_lossy())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the Windows temporary directory cannot carry a WSL1 launch permit",
                )
            })?;
        let completion_wsl_path = windows_path_to_wsl_path(
            &completion_windows_path.to_string_lossy(),
        )
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the Windows temporary directory cannot carry a WSL completion attestation",
            )
        })?;
        Ok(Self {
            attestation_windows_path,
            attestation_wsl_path,
            permit_windows_path,
            permit_wsl_path,
            completion_windows_path,
            completion_wsl_path,
            expected_value,
        })
    }

    fn attested_value(&self) -> std::io::Result<Option<String>> {
        let value = match fs::read_to_string(&self.attestation_windows_path) {
            Ok(value) => value.trim().to_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("unable to read the WSL child attestation: {error}"),
                ));
            }
        };
        Ok(Some(value))
    }

    fn is_attested(&self) -> std::io::Result<bool> {
        let Some(expected_value) = self.expected_value.as_deref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires explicit attestation acceptance",
            ));
        };
        let Some(value) = self.attested_value()? else {
            return Ok(false);
        };
        if value != expected_value {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the WSL child attested a different launch identity",
            ));
        }
        Ok(true)
    }

    fn authorize(&self) -> std::io::Result<()> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit permit value",
            )
        })?;
        self.authorize_value(expected_value)
    }

    fn authorize_value(&self, expected_value: &str) -> std::io::Result<()> {
        let temporary = self.permit_windows_path.with_extension("tmp");
        let _ = fs::remove_file(&temporary);
        fs::write(&temporary, expected_value.as_bytes()).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("unable to prepare the WSL1 parent launch permit: {error}"),
            )
        })?;
        fs::rename(&temporary, &self.permit_windows_path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            std::io::Error::new(
                error.kind(),
                format!("unable to publish the WSL1 parent launch permit: {error}"),
            )
        })
    }

    fn completion_status(&self) -> std::io::Result<Option<i32>> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit completion identity",
            )
        })?;
        self.completion_status_for(expected_value)
    }

    fn completion_status_for(&self, expected_value: &str) -> std::io::Result<Option<i32>> {
        let completion = match fs::read_to_string(&self.completion_windows_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("unable to read the WSL child completion attestation: {error}"),
                ));
            }
        };
        let (identity, status) = completion.trim().split_once(':').ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the WSL child completion attestation is malformed",
            )
        })?;
        if identity != expected_value {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the WSL child completed under a different launch identity",
            ));
        }
        let status = status.parse::<i32>().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the WSL child completion attestation has an invalid exit status",
            )
        })?;
        if !(0..=255).contains(&status) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the WSL child completion attestation exit status is out of range",
            ));
        }
        Ok(Some(status))
    }
}

impl Drop for LaunchPermitGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.attestation_windows_path);
        let mut attestation_staging = self.attestation_windows_path.as_os_str().to_os_string();
        attestation_staging.push(".staging");
        let _ = fs::remove_file(PathBuf::from(attestation_staging));
        let _ = fs::remove_file(&self.permit_windows_path);
        let _ = fs::remove_file(self.permit_windows_path.with_extension("tmp"));
        let _ = fs::remove_file(&self.completion_windows_path);
        let mut completion_staging = self.completion_windows_path.as_os_str().to_os_string();
        completion_staging.push(".staging");
        let _ = fs::remove_file(PathBuf::from(completion_staging));
    }
}

fn verify_proxy_completion_status(
    proxy_status: ExitStatus,
    attested_status: i32,
) -> std::io::Result<ExitStatus> {
    if proxy_status.code() == Some(attested_status) {
        Ok(proxy_status)
    } else {
        Err(std::io::Error::other(format!(
            "WSL completion status {attested_status} differs from proxy status {:?}",
            proxy_status.code()
        )))
    }
}

fn verify_pre_authorization_proxy_status(proxy_status: ExitStatus) -> std::io::Result<ExitStatus> {
    if proxy_status.success() {
        Err(std::io::Error::other(
            "WSL1 proxy exited successfully before launch authorization; the target was not executed",
        ))
    } else {
        Ok(proxy_status)
    }
}

fn revalidate_dedicated_wsl1_installation(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match dedicated_wsl1_installation_id_for(&config.distro) {
            Some(actual) if actual == expected_installation_id => return Ok(()),
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the WSL1 dedicated-runtime identity changed after child launch",
                ));
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unable to revalidate the WSL1 dedicated-runtime identity before termination",
                ));
            }
        }
    }
}

fn running_wsl_distributions() -> std::io::Result<Vec<String>> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--running", "--quiet"]);
    let output = process::run_probe(&mut command)?;
    if !output.status.success() || output.stdout_truncated {
        return Err(std::io::Error::other(
            "unable to inspect running WSL distributions",
        ));
    }
    Ok(decode_wsl_output(&output.stdout)
        .replace('\0', "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn wait_for_wsl_distro_to_stop_within(distro: &str, timeout: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let running = running_wsl_distributions()?;
        if !running.iter().any(|candidate| candidate == distro) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("WSL distro {distro} remained running after termination"),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_wsl_distro_to_stop(distro: &str) -> std::io::Result<()> {
    wait_for_wsl_distro_to_stop_within(distro, Duration::from_secs(5))
}

fn terminate_dedicated_wsl1_distro(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    revalidate_dedicated_wsl1_installation(config, expected_installation_id)?;
    trace(format!(
        "terminating dedicated WSL1 distro {} installation {} after cancellation",
        config.distro, expected_installation_id
    ));
    let mut command = Command::new("wsl.exe");
    command.args(["--terminate", &config.distro]);
    match process::run_probe(&mut command) {
        Ok(output) if output.status.success() => wait_for_wsl_distro_to_stop(&config.distro),
        Ok(output) => {
            trace(format!(
                "WSL1 terminate returned {}: {}{}",
                output.status,
                decode_wsl_output(&output.stdout).trim(),
                decode_wsl_output(&output.stderr).trim()
            ));
            Err(std::io::Error::other("WSL1 terminate command failed"))
        }
        Err(error) => Err(error),
    }
}

fn stop_cancelled_wsl1_child(
    child: &mut Child,
    config: &Config,
    expected_installation_id: Option<&str>,
) -> std::io::Result<ExitStatus> {
    // Stop the Windows proxy first so it cannot outlive XUVA. The Linux-side
    // command is still blocked on the identity-bound permit at this point.
    let _ = child.kill();
    let termination = expected_installation_id
        .map(|installation_id| terminate_dedicated_wsl1_distro(config, installation_id));
    let proxy_deadline = Instant::now() + Duration::from_secs(3);
    let proxy_status = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }
        if Instant::now() >= proxy_deadline {
            break Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the Windows WSL1 proxy remained alive after cancellation",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };

    match termination {
        Some(Err(error)) => {
            // An identity change means XUVA must not terminate a potentially
            // unrelated distro. The unpermitted child self-expires instead,
            // and XUVA proves that the runtime stopped before returning the
            // failure.
            let stopped =
                wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12));
            return Err(match stopped {
                Ok(()) => error,
                Err(stop_error) => std::io::Error::other(format!(
                    "{error}; the untrusted WSL1 runtime also failed to stop: {stop_error}"
                )),
            });
        }
        None => {
            wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12)).map_err(
                |stop_error| {
                    std::io::Error::other(format!(
                        "the unpermitted WSL1 runtime failed to stop after proxy cancellation: {stop_error}"
                    ))
                },
            )?;
        }
        Some(Ok(())) => {}
    }
    proxy_status
}

fn wait_for_wsl1_child(
    mut child: Child,
    config: &Config,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    let mut authorized = false;
    let mut accepted_installation_id = None;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    loop {
        let cancellation_requested = console::requested();
        if cancellation_requested {
            match (
                accepted_installation_id.as_deref(),
                launch_guard.attested_value(),
            ) {
                (Some(installation_id), _) => {
                    return stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                }
                (None, Ok(Some(installation_id))) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id);
                    return stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                }
                (None, Ok(Some(_))) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the cancelled WSL1 child attested an invalid installation ID",
                    ));
                }
                (None, Err(error)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
                (None, Ok(None)) if started.elapsed() < Duration::from_secs(10) => {
                    // The target remains blocked because no permit is ever
                    // published. Keep the proxy alive long enough for the
                    // root-owned dedicated marker to be attested, then use
                    // that exact identity for safe distro termination.
                }
                (None, Ok(None)) => {
                    let cleanup = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(match cleanup {
                        Ok(_) => std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "cancelled WSL1 child never attested a dedicated-runtime identity",
                        ),
                        Err(cleanup_error) => cleanup_error,
                    });
                }
            }
        }
        if !authorized && !cancellation_requested {
            match launch_guard.attested_value() {
                Ok(Some(installation_id)) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id.clone());
                    if let Err(error) = launch_guard.authorize_value(&installation_id) {
                        let cleanup = stop_cancelled_wsl1_child(
                            &mut child,
                            config,
                            accepted_installation_id.as_deref(),
                        );
                        return Err(match cleanup {
                            Ok(_) => error,
                            Err(cleanup_error) => std::io::Error::other(format!(
                                "{error}; WSL1 authorization cleanup failed: {cleanup_error}"
                            )),
                        });
                    }
                    authorized = true;
                    trace("authorized an identity-matched WSL1 child");
                }
                Ok(Some(_)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the WSL1 child attested an invalid dedicated-runtime installation ID",
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
            }
        }
        if authorized
            && !test_proxy_killed
            && test_kill_wsl1_proxy_after_permit()
            && test_ready_file_exists()
        {
            // The test-only ready boundary is published immediately before
            // target launch. Give the target one scheduler turn so the
            // contract exercises a proxy failure after execution has begun.
            thread::sleep(Duration::from_millis(100));
            trace("test hook terminated the WSL1 Windows proxy after launch permit");
            let _ = child.kill();
            test_proxy_killed = true;
        }
        if proxy_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    trace(format!("WSL1 Windows proxy exited with {status}"));
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    let cleanup = stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 child status cleanup failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if let Some(status) = proxy_status {
            if !authorized {
                return verify_pre_authorization_proxy_status(status);
            }
            let installation_id = accepted_installation_id.as_deref().ok_or_else(|| {
                std::io::Error::other(
                    "an authorized WSL1 child has no accepted dedicated-runtime identity",
                )
            })?;
            match launch_guard.completion_status_for(installation_id) {
                Ok(Some(attested_status)) => {
                    match verify_proxy_completion_status(status, attested_status) {
                        Ok(status) => return Ok(status),
                        Err(error) => {
                            let cleanup = stop_cancelled_wsl1_child(
                                &mut child,
                                config,
                                Some(installation_id),
                            );
                            return Err(match cleanup {
                                Ok(_) => error,
                                Err(cleanup_error) => std::io::Error::other(format!(
                                    "{error}; WSL1 mismatch recovery failed: {cleanup_error}"
                                )),
                            });
                        }
                    }
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    let error = std::io::Error::other(
                        "WSL1 proxy exited after launch permit without a completion attestation",
                    );
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 incomplete-launch recovery failed: {cleanup_error}"
                        )),
                    });
                }
                Err(error) => {
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 invalid-completion recovery failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if !authorized && started.elapsed() >= Duration::from_secs(10) {
            let _ =
                stop_cancelled_wsl1_child(&mut child, config, accepted_installation_id.as_deref());
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "WSL1 child did not attest its dedicated-runtime identity within 10 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_wsl_child(
    mut child: Child,
    config: &Config,
    token: &str,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let launched_at = Instant::now();
    let mut authorized = false;
    let mut pending_error: Option<std::io::Error> = None;
    let mut cancellation_started: Option<Instant> = None;
    let mut interrupt_sent = false;
    let mut terminate_sent = false;
    let mut kill_sent = false;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    let mut test_proxy_reap_deferred = false;
    let test_defer_proxy_reap = test_defer_wsl2_proxy_reap_until_cleanup();
    if test_defer_proxy_reap {
        trace("test hook armed deferred WSL2 proxy reap");
    }
    loop {
        if console::requested() {
            cancellation_started.get_or_insert_with(Instant::now);
        }
        let defer_proxy_reap = test_proxy_killed && test_defer_proxy_reap;
        if defer_proxy_reap && !test_proxy_reap_deferred {
            trace("test hook deferred WSL2 proxy reap until Linux cleanup");
            test_proxy_reap_deferred = true;
        }
        if proxy_status.is_none()
            && !defer_proxy_reap
            && let Some(status) = child.try_wait()?
        {
            trace(format!("WSL2 Windows proxy exited with {status}"));
            proxy_status = Some(status);
            proxy_exited_at = Some(Instant::now());
        }
        if !authorized && cancellation_started.is_none() && proxy_status.is_none() {
            match launch_guard.is_attested() {
                Ok(true) => match launch_guard.authorize() {
                    Ok(()) => {
                        authorized = true;
                        trace("authorized a cancellation-ready WSL2 child");
                    }
                    Err(error) => {
                        pending_error = Some(error);
                        cancellation_started = Some(Instant::now());
                    }
                },
                Ok(false) if launched_at.elapsed() < Duration::from_secs(10) => {}
                Ok(false) => {
                    pending_error = Some(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "WSL2 child did not attest its private cancellation token within 10 seconds",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(status) = proxy_status.as_ref()
            && cancellation_started.is_none()
            && pending_error.is_none()
        {
            match launch_guard.completion_status() {
                Ok(Some(attested_status)) => {
                    return verify_proxy_completion_status(*status, attested_status);
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    pending_error = Some(std::io::Error::other(
                        "WSL2 proxy exited before the Linux launcher attested complete process-group cleanup",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(started) = cancellation_started {
            let elapsed = started.elapsed();
            if test_kill_wsl2_proxy_during_cancellation()
                && !test_proxy_killed
                && proxy_status.is_none()
            {
                trace("test hook terminated the WSL2 Windows proxy during cancellation");
                let _ = child.kill();
                test_proxy_killed = true;
            }
            if !interrupt_sent && send_linux_signal(config, token, "INT").unwrap_or(false) {
                trace("sent SIGINT to the isolated Linux process group");
                interrupt_sent = true;
            }
            if elapsed >= Duration::from_millis(1_500)
                && !terminate_sent
                && send_linux_signal(config, token, "TERM").unwrap_or(false)
            {
                trace("escalated cancellation to SIGTERM inside Linux");
                terminate_sent = true;
            }
            if elapsed >= Duration::from_secs(3)
                && !kill_sent
                && send_linux_signal(config, token, "KILL").unwrap_or(false)
            {
                trace("escalated cancellation to SIGKILL inside Linux");
                kill_sent = true;
            }
            let completion = match launch_guard.completion_status() {
                Ok(status) => status,
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let group_state = match linux_process_group_state(config, token) {
                Ok(state) => Some(state),
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let cleanup_proven = matches!(group_state, Some(LinuxProcessGroupState::Gone))
                || matches!(
                    (group_state, completion),
                    (Some(LinuxProcessGroupState::TokenUnavailable), Some(_))
                );
            if cleanup_proven {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                if let Some(error) = pending_error {
                    return Err(error);
                }
                return Ok(status);
            }
            if elapsed >= Duration::from_secs(4) && proxy_status.is_none() {
                let _ = child.kill();
                if let Some(status) = child.try_wait()? {
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
            }
            if elapsed >= Duration::from_secs(15) {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                trace(format!(
                    "reaped WSL2 Windows proxy with {status} after failed cleanup proof"
                ));
                let cleanup_error = std::io::Error::other(match group_state {
                    Some(LinuxProcessGroupState::Alive) => {
                        "Linux process group survived SIGINT, SIGTERM, and SIGKILL escalation"
                    }
                    Some(LinuxProcessGroupState::TokenUnavailable) => {
                        "WSL2 cancellation token disappeared without a completion attestation"
                    }
                    Some(LinuxProcessGroupState::Gone) => {
                        "WSL2 cleanup completed without a reapable Windows proxy"
                    }
                    None => "unable to prove WSL2 process-group cleanup after proxy exit",
                });
                return Err(match pending_error {
                    Some(error) => std::io::Error::other(format!(
                        "{error}; cancellation finalization failed: {cleanup_error}"
                    )),
                    None => cleanup_error,
                });
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn command_family(arguments: &[OsString]) -> &str {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .unwrap_or("unknown")
}

fn has_wsl_path(arguments: &[OsString]) -> bool {
    arguments.iter().any(is_wsl_path)
}

fn git_subcommand(arguments: &[OsString]) -> Option<&str> {
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

fn is_verified_read_only_git(arguments: &[OsString]) -> bool {
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

fn is_verified_cargo_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments.get(1).and_then(|argument| argument.to_str()),
        Some("check" | "test" | "clippy")
    )
}

fn is_verified_npm_run_list_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand] if program == "npm" && subcommand == "run"
    )
}

fn is_verified_go_test_all_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand, selector]
            if program == "go" && subcommand == "test" && selector == "./..."
    )
}

fn route_policy_key(arguments: &[OsString]) -> Option<String> {
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
fn auto_route(
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

fn auto_route_with_context(
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

fn is_rtk_meta_command(command: &str) -> bool {
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

fn is_adapter_only_rtk_command(command: &str) -> bool {
    is_rtk_meta_command(command) && !requires_raw_posix_provider(command)
}

fn auto_route_for_environment(
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

fn configured_wsl_backend(config: &Config, route: Route) -> Config {
    let mut selected = config.clone();
    match route {
        Route::Wsl1 => {
            selected.backend = WslBackend::Wsl1;
            if config.backend != WslBackend::Wsl1 && selected.distro == DEFAULT_DISTRO {
                selected.distro = DEFAULT_WSL1_DISTRO.to_owned();
            }
        }
        Route::Wsl2 => {
            selected.backend = WslBackend::Wsl2;
            if config.backend != WslBackend::Wsl2 && selected.distro == DEFAULT_WSL1_DISTRO {
                selected.distro = DEFAULT_DISTRO.to_owned();
            }
        }
        Route::Auto | Route::Raw | Route::NativeRtk => {}
    }
    selected
}

fn print_adapter_info(config: &Config) {
    println!("adapter={PRODUCT_COMMAND}");
    println!("command={PRODUCT_COMMAND}");
    println!("profile={}", config.profile.as_str());
    println!("route_preference={}", config.route_preference.as_str());
    println!("environment={}", config.environment.as_str());
    println!("policy_objective={}", config.policy_objective.as_str());
    println!(
        "environment_allowlist={}",
        if config.environment_allowlist.is_empty() {
            "none".to_owned()
        } else {
            config.environment_allowlist.join(",")
        }
    );
    println!("environment_boolean_feature_gates=automatic");
    println!("native_rtk_path={}", config.native_rtk_path);
    println!(
        "metrics={}",
        if config.metrics_enabled {
            "local-aggregate-only"
        } else {
            "off"
        }
    );
}

fn begin_invocation_metrics(
    config: &Config,
    adapter: &dispatcher::OutputAdapter,
) -> Option<XuvaMetrics> {
    if !config.metrics_enabled {
        return None;
    }
    let result = if matches!(adapter, dispatcher::OutputAdapter::Raw) {
        XuvaMetrics::begin_unmeasured()
    } else {
        XuvaMetrics::begin()
    };
    match result {
        Ok(metrics) => Some(metrics),
        Err(error) => {
            eprintln!("xuva: metrics disabled for this invocation: {error}");
            None
        }
    }
}

fn static_windows_execution_plan(
    arguments: &[OsString],
    config: &Config,
    route: Route,
) -> Result<dispatcher::ExecutionPlan, String> {
    let tool = arguments
        .first()
        .ok_or_else(|| "a Windows execution plan needs a command".to_owned())?;
    let tool_name = tool
        .to_str()
        .ok_or_else(|| "cross-host command names must be valid Unicode".to_owned())?;
    let cwd = windows_cwd_for_invocation(config)?;
    let raw_executable = adapters::windows::raw_executable(tool);
    let (candidate_executable, adapter) = match route {
        Route::Raw => (raw_executable, dispatcher::OutputAdapter::Raw),
        Route::NativeRtk => {
            let executable = OsString::from(&config.native_rtk_path);
            (
                executable.clone(),
                dispatcher::OutputAdapter::Rtk { executable },
            )
        }
        Route::Auto | Route::Wsl1 | Route::Wsl2 => {
            return Err("only Windows raw/native routes can use a static Windows plan".to_owned());
        }
    };
    Ok(dispatcher::ExecutionPlan {
        request: dispatcher::CommandSpec {
            executable: tool.clone(),
            arguments: translate_arguments_to_windows(tool_name, &arguments[1..], config),
            cwd: Some(cwd.clone()),
            environment: forwarded_environment(config),
            environment_policy: dispatcher::EnvironmentPolicy::Isolated,
            interactive: false,
        },
        candidate: dispatcher::RouteCandidate::Windows {
            executable: candidate_executable,
            cwd: Some(cwd),
        },
        adapter,
        explanation: vec![dispatcher::DecisionReason(
            "WSL-origin Windows execution uses an isolated structured plan".to_owned(),
        )],
    })
}

fn run_native_rtk(
    arguments: &[OsString],
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    adapters::windows::run_rtk_at(&config.native_rtk_path, arguments, None, metrics)
}

fn execution_route(route: &dispatcher::RouteCandidate) -> Route {
    match route {
        dispatcher::RouteCandidate::Windows { .. } => Route::Raw,
        dispatcher::RouteCandidate::Wsl1 { .. } => Route::Wsl1,
        dispatcher::RouteCandidate::Wsl2 { .. } => Route::Wsl2,
    }
}

fn provider_adapter(
    candidate: &ProviderCandidate,
    preference: OutputAdapterPreference,
) -> Result<dispatcher::OutputAdapter, std::io::Error> {
    match (preference, candidate.rtk.as_deref()) {
        (OutputAdapterPreference::Raw, _) if candidate.supports_adapter(AdapterKind::Raw) => {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Raw, _) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "raw output was requested but this provider has no raw tool executable",
        )),
        (OutputAdapterPreference::Auto, None) if candidate.supports_adapter(AdapterKind::Raw) => {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Auto, None) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "this provider has neither a raw executable nor a usable output adapter",
        )),
        (OutputAdapterPreference::Auto | OutputAdapterPreference::Rtk, Some(executable))
            if candidate.supports_adapter(AdapterKind::Rtk) =>
        {
            Ok(dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(executable),
            })
        }
        (OutputAdapterPreference::Rtk, _) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "RTK output adapter was requested but this provider has no RTK executable",
        )),
        (OutputAdapterPreference::Auto, Some(_))
            if candidate.supports_adapter(AdapterKind::Raw) =>
        {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Auto, Some(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the configured adapter is not usable and no raw tool executable exists",
        )),
    }
}

fn provider_execution_config(
    config: &Config,
    route: &dispatcher::RouteCandidate,
    adapter: &dispatcher::OutputAdapter,
) -> Result<Config, std::io::Error> {
    let (wsl_route, distro, cwd, raw_executable) = match route {
        dispatcher::RouteCandidate::Wsl1 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl1, distro, cwd, executable),
        dispatcher::RouteCandidate::Wsl2 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl2, distro, cwd, executable),
        dispatcher::RouteCandidate::Windows { .. } => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a Windows execution plan has no WSL transport configuration",
            ));
        }
    };
    let mut selected = configured_wsl_backend(config, wsl_route);
    selected.distro = distro.clone();
    selected.cwd = Some(cwd.to_string_lossy().into_owned());
    selected.rtk_path = Some(match adapter {
        dispatcher::OutputAdapter::Raw => raw_executable.to_string_lossy().into_owned(),
        dispatcher::OutputAdapter::Rtk { executable } => executable.to_string_lossy().into_owned(),
    });
    Ok(selected)
}

fn execution_plan_for_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
) -> Result<dispatcher::ExecutionPlan, std::io::Error> {
    let cwd = candidate.project_path.as_deref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider candidate has no verified project directory",
        )
    })?;
    if !candidate.has_consistent_location() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "provider host, distribution, and WSL version are inconsistent",
        ));
    }
    let raw_required = (tool == "git" && candidate.is_windows())
        || requires_raw_posix_provider(tool)
        || (config.output_adapter == OutputAdapterPreference::Auto
            && matches!(
                command_surface(tool),
                CommandSurface::RawNative | CommandSurface::Unknown
            ));
    let adapter = if raw_required {
        if !candidate.supports_adapter(AdapterKind::Raw) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "this command requires raw semantics but the provider has no raw tool executable",
            ));
        }
        // Git for Windows owns NTFS worktrees, object creation, credentials,
        // and Windows network configuration. Keep it raw so an output adapter
        // cannot turn a successful native operation into a failed WSL one.
        dispatcher::OutputAdapter::Raw
    } else {
        provider_adapter(candidate, config.output_adapter)?
    };
    let translated_arguments = match candidate.host {
        ProviderHost::Windows => translate_arguments_to_windows(tool, arguments, config),
        ProviderHost::Wsl1 | ProviderHost::Wsl2 => translate_arguments_to_wsl(
            tool,
            arguments,
            config,
            candidate
                .distro
                .as_deref()
                .expect("consistent WSL candidates have a distro"),
        ),
    };
    let request = dispatcher::CommandSpec {
        executable: OsString::from(tool),
        arguments: translated_arguments,
        cwd: Some(PathBuf::from(cwd)),
        environment: forwarded_environment(config),
        environment_policy: provider_environment_policy(config, candidate),
        interactive: false,
    };
    let route = match candidate.host {
        ProviderHost::Windows => dispatcher::RouteCandidate::Windows {
            executable: OsString::from(&candidate.executable),
            cwd: Some(PathBuf::from(cwd)),
        },
        ProviderHost::Wsl1 => dispatcher::RouteCandidate::Wsl1 {
            distro: candidate
                .distro
                .clone()
                .expect("consistent WSL1 candidates have a distro"),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
        ProviderHost::Wsl2 => dispatcher::RouteCandidate::Wsl2 {
            distro: candidate
                .distro
                .clone()
                .expect("consistent WSL2 candidates have a distro"),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
    };
    Ok(dispatcher::ExecutionPlan {
        request,
        candidate: route,
        adapter,
        explanation: vec![dispatcher::DecisionReason(candidate.reason.clone())],
    })
}

fn execution_plan_for_explicit_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
) -> Result<dispatcher::ExecutionPlan, std::io::Error> {
    let mut explicit = config.clone();
    if explicit.output_adapter == OutputAdapterPreference::Auto && candidate.rtk.is_some() {
        explicit.output_adapter = OutputAdapterPreference::Rtk;
    }
    execution_plan_for_provider_candidate(tool, arguments, &explicit, candidate)
}

fn first_compatible_provider_plan<'a>(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidates: &'a [ProviderCandidate],
) -> Option<(usize, &'a ProviderCandidate, dispatcher::ExecutionPlan)> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.usable && candidate.has_consistent_location())
        .find_map(|(index, candidate)| {
            execution_plan_for_explicit_provider_candidate(tool, arguments, config, candidate)
                .ok()
                .map(|plan| (index, candidate, plan))
        })
}

fn is_shell_operator_command(arguments: &[OsString]) -> bool {
    matches!(
        arguments.first().and_then(|argument| argument.to_str()),
        Some("|" | "||" | "&&" | ";" | "<" | ">" | ">>")
    )
}

fn forwarded_environment(config: &Config) -> Vec<(OsString, OsString)> {
    const SAFE_DEFAULTS: &[&str] = &[
        "CI",
        "COLORTERM",
        "FORCE_COLOR",
        "NO_COLOR",
        "RUST_BACKTRACE",
        "TERM",
    ];
    let explicitly_allowed: HashSet<&str> = config
        .environment_allowlist
        .iter()
        .map(String::as_str)
        .collect();
    env::vars_os()
        .filter(|(name, value)| {
            should_forward_environment(
                name.to_str().unwrap_or_default(),
                value.to_str(),
                &explicitly_allowed,
                SAFE_DEFAULTS,
            )
        })
        .collect()
}

fn should_forward_environment(
    name: &str,
    value: Option<&str>,
    explicitly_allowed: &HashSet<&str>,
    safe_defaults: &[&str],
) -> bool {
    if is_sensitive_environment_name(name) {
        return false;
    }
    let automatic_feature_gate = matches!(value, Some("0" | "1")) && name.contains("_RUN_");
    safe_defaults.contains(&name) || explicitly_allowed.contains(name) || automatic_feature_gate
}

fn run_execution_plan(
    plan: &dispatcher::ExecutionPlan,
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let rtk_arguments = || {
        let mut forwarded = Vec::with_capacity(plan.request.arguments.len() + 1);
        forwarded.push(plan.request.executable.clone());
        forwarded.extend(plan.request.arguments.iter().cloned());
        forwarded
    };
    match (&plan.candidate, &plan.adapter) {
        (
            dispatcher::RouteCandidate::Windows { executable, .. },
            dispatcher::OutputAdapter::Raw,
        ) => adapters::windows::run_plan(executable, &plan.request),
        (
            dispatcher::RouteCandidate::Windows { .. },
            dispatcher::OutputAdapter::Rtk { executable },
        ) => adapters::windows::run_rtk_plan(executable, &rtk_arguments(), &plan.request, metrics),
        (
            dispatcher::RouteCandidate::Wsl1 { .. } | dispatcher::RouteCandidate::Wsl2 { .. },
            adapter,
        ) => {
            let selected = provider_execution_config(config, &plan.candidate, adapter)?;
            let forwarded = match adapter {
                dispatcher::OutputAdapter::Raw => plan.request.arguments.clone(),
                dispatcher::OutputAdapter::Rtk { .. } => rtk_arguments(),
            };
            let raw_executable = match &plan.candidate {
                dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
                dispatcher::RouteCandidate::Windows { .. } => {
                    unreachable!("WSL arm has a WSL candidate")
                }
            };
            let executable = match adapter {
                dispatcher::OutputAdapter::Raw => raw_executable,
                dispatcher::OutputAdapter::Rtk { executable } => executable,
            };
            let measured = matches!(adapter, dispatcher::OutputAdapter::Rtk { .. })
                .then_some(metrics)
                .flatten();
            run_wsl_execution_plan(
                executable,
                &forwarded,
                &plan.request.environment,
                &selected,
                execution_route(&plan.candidate),
                measured,
            )
        }
    }
}

fn provider_exec_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(2).and_then(|argument| argument.to_str()) else {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let separator = arguments.iter().position(|argument| argument == "--");
    let Some(separator) = separator else {
        eprintln!("xuva: provider execution requires `--` before tool arguments");
        return ExitCode::FAILURE;
    };
    if separator < 3 {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    let options = &arguments[3..separator];
    let candidate_index = match options {
        [] => None,
        [flag, index] if flag == "--candidate" => index.to_string_lossy().parse::<usize>().ok(),
        _ => None,
    };
    if !options.is_empty() && candidate_index.is_none() {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    // Execution is explicit and must not reuse a provider identity discovered
    // under a previous RTK path or tool installation state.
    let resolution = resolve_tool_provider(tool, config, true);
    let forwarded = &arguments[separator + 1..];
    let (index, candidate, plan) = if let Some(index) = candidate_index {
        let Some(candidate) = resolution.candidates.get(index) else {
            eprintln!("xuva: provider candidate {index} does not exist; run `xuva resolve {tool}`");
            return ExitCode::FAILURE;
        };
        if !candidate.usable {
            eprintln!(
                "xuva: provider candidate {index} is not verified: {}",
                candidate.reason
            );
            return ExitCode::from(127);
        }
        match execution_plan_for_explicit_provider_candidate(tool, forwarded, config, candidate) {
            Ok(plan) => (index, candidate, plan),
            Err(error) => {
                eprintln!(
                    "xuva: provider candidate {index} cannot produce an execution plan: {error}"
                );
                return ExitCode::from(127);
            }
        }
    } else {
        let selected =
            first_compatible_provider_plan(tool, forwarded, config, &resolution.candidates);
        let Some(selected) = selected else {
            eprintln!(
                "xuva: no verified provider supports the requested output adapter and command semantics; run `xuva resolve {tool}` for details"
            );
            return ExitCode::from(127);
        };
        selected
    };
    let route = execution_route(&plan.candidate);
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let metrics = begin_invocation_metrics(config, &plan.adapter);
    let result = run_execution_plan(&plan, config, metrics.as_ref());
    if needs_console_handler {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    if let Some(metrics) = metrics {
        let command_family = format!(
            "provider:{}:{}",
            candidate.host.as_str(),
            plan.adapter.as_str()
        );
        if let Err(error) = metrics.finish(
            route.as_str(),
            &command_family,
            started.elapsed(),
            exit_code,
        ) {
            eprintln!("xuva: metrics were not recorded: {error}");
        }
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!("xuva: unable to start provider candidate {index}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_wsl_route(
    arguments: Vec<OsString>,
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 child was spawned",
            ));
        }
        trace("starting identity-gated WSL1 child while holding the global mutex");
        let mut process = adapters::wsl1::process(wsl1_rtk_arguments_with_metrics(
            arguments,
            config,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ));
        let result = process.spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 child completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        adapters::wsl2::process(rtk_arguments_with_metrics(
            arguments,
            config,
            &token,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ))
        .spawn()
        .and_then(|child| {
            trace("spawned cancellation-gated WSL2 proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}

fn run_wsl_execution_plan(
    executable: &OsString,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 plan was spawned",
            ));
        }
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: None,
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
            },
        )?;
        trace("starting identity-gated WSL1 plan while holding the global mutex");
        let result = adapters::wsl1::process(command).spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 plan proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 plan completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: Some(&token),
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
            },
        )?;
        adapters::wsl2::process(command).spawn().and_then(|child| {
            trace("spawned cancellation-gated WSL2 plan proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}

fn parse_options(
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
            Some(EXPLAIN_ROUTE_ARGUMENT) => {
                explain = true;
                arguments.remove(0);
            }
            _ => return Ok((arguments, route, environment, explain)),
        }
    }
}

fn is_version_command(arguments: &[OsString]) -> bool {
    arguments.len() == 1
        && matches!(
            arguments[0].to_str(),
            Some(VERSION_ARGUMENT | "version" | "-V")
        )
}

fn is_verbose_version_command(arguments: &[OsString]) -> bool {
    arguments
        == [
            OsString::from(VERSION_ARGUMENT),
            OsString::from("--verbose"),
        ]
}

fn print_verbose_version() {
    println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
    println!("commit={}", env!("XUVA_BUILD_COMMIT"));
    println!("target={}", env!("XUVA_BUILD_TARGET"));
    println!("profile={}", env!("XUVA_BUILD_PROFILE"));
    println!("provenance={}", env!("XUVA_BUILD_PROVENANCE"));
    println!("provider_cache_schema={PROVIDER_CACHE_SCHEMA_VERSION}");
}

fn print_help() {
    println!("XUVA {}", env!("CARGO_PKG_VERSION"));
    println!("usage: xuva [--explain-route] <command> [<argv>...]");
    println!();
    println!("diagnostics:");
    println!("  xuva --explain-route <command> [<argv>...]");
    println!("  xuva doctor <tool> [--json] [--refresh]");
    println!("  xuva self-update --check");
    println!("  xuva surface");
    println!();
    println!("lifecycle:");
    println!("  xuva install --status");
    println!("  xuva install --recover");
    println!("  xuva rollback");
    println!("  xuva uninstall [--remove-from-path]");
    println!();
    println!(
        "Shell operators are owned by the invoking shell. XUVA preserves argv and never rebuilds a pipeline."
    );
}

fn parsed_version(value: &str) -> Option<((u64, u64, u64), bool)> {
    let value = value.trim().trim_start_matches('v');
    let (core, prerelease) = match value.split_once('-') {
        Some((core, suffix)) if !suffix.trim().is_empty() => (core, true),
        Some(_) => return None,
        None => (value, false),
    };
    let mut fields = core.split('.');
    let major = fields.next()?.parse().ok()?;
    let minor = fields.next()?.parse().ok()?;
    let patch = fields.next()?.parse().ok()?;
    fields
        .next()
        .is_none()
        .then_some(((major, minor, patch), prerelease))
}

fn parsed_stable_version(value: &str) -> Option<(u64, u64, u64)> {
    parsed_version(value)
        .filter(|(_, prerelease)| !prerelease)
        .map(|(version, _)| version)
}

fn stable_release_is_newer(latest: &str, current: &str) -> bool {
    parsed_stable_version(latest)
        .zip(parsed_version(current))
        .is_some_and(|(latest, (current, prerelease))| {
            latest > current || (latest == current && prerelease)
        })
}

fn latest_release_from_ls_remote(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/tags/"))
        .filter_map(|tag| parsed_stable_version(tag).map(|version| (version, tag)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag.to_owned())
}

fn native_git_output_with_timeout(
    arguments: &[&str],
    timeout: Duration,
) -> std::io::Result<(ExitStatus, String, String)> {
    let mut command = Command::new("git.exe");
    command.args(arguments);
    let output = process::run_bounded(&mut command, None, timeout, process::PROBE_OUTPUT_LIMIT)?;
    if output.stdout_truncated || output.stderr_truncated {
        return Err(std::io::Error::other(
            "native Git release check exceeded the output limit",
        ));
    }
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn self_update_command(arguments: &[OsString]) -> ExitCode {
    if arguments == [OsString::from(SELF_UPDATE_ARGUMENT)] {
        println!("current={}", env!("CARGO_PKG_VERSION"));
        println!("status=manual-update-required");
        println!(
            "action=download a verified release or run scripts/install.ps1 from a trusted XUVA checkout"
        );
        println!("check=xuva self-update --check");
        return ExitCode::SUCCESS;
    }
    if arguments
        != [
            OsString::from(SELF_UPDATE_ARGUMENT),
            OsString::from("--check"),
        ]
    {
        eprintln!("xuva: usage: self-update [--check]");
        return ExitCode::FAILURE;
    }
    let current = env!("CARGO_PKG_VERSION");
    let result = native_git_output_with_timeout(
        &["ls-remote", "--tags", "--refs", RELEASE_TAGS_URL],
        UPDATE_CHECK_TIMEOUT,
    );
    let (status, stdout, stderr) = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!(
                "xuva: update check unavailable via native Git: {error}; verify Git for Windows and Windows DNS, then retry"
            );
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Git for Windows could not query the release tags");
        eprintln!(
            "xuva: update check failed via native Git: {detail}; verify Windows DNS, proxy, and Git credentials, then retry"
        );
        return ExitCode::FAILURE;
    }
    let Some(latest) = latest_release_from_ls_remote(&stdout) else {
        eprintln!("xuva: update check returned no stable vMAJOR.MINOR.PATCH release tags");
        return ExitCode::FAILURE;
    };
    let update_available = stable_release_is_newer(&latest, current);
    println!("current={current}");
    println!("latest={}", latest.trim_start_matches('v'));
    println!(
        "status={}",
        if update_available {
            "update-available"
        } else {
            "up-to-date"
        }
    );
    println!("route=windows-native-git");
    ExitCode::SUCCESS
}

fn run_cli(arguments: Vec<OsString>, config: &Config) -> ExitCode {
    if is_verbose_version_command(&arguments) {
        print_verbose_version();
        return ExitCode::SUCCESS;
    }
    if is_version_command(&arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some(HELP_ARGUMENT | "help" | "-h"))
    {
        print_help();
        return ExitCode::SUCCESS;
    }
    if let Some(result) = lifecycle::command(&arguments) {
        return result;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SELF_UPDATE_ARGUMENT)
    {
        return self_update_command(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == AGENT_ARGUMENT)
    {
        return agent::command(&arguments, &config.native_rtk_path);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SURFACE_ARGUMENT)
    {
        return print_command_surface(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == PROVIDER_ARGUMENT)
    {
        if arguments.get(1).is_some_and(|argument| argument == "exec") {
            return provider_exec_command(&arguments, config);
        }
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == RESOLVE_ARGUMENT)
    {
        return provider_command(&arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == WHICH_ARGUMENT)
    {
        let mut resolve_arguments = arguments.clone();
        resolve_arguments[0] = OsString::from(RESOLVE_ARGUMENT);
        return provider_command(&resolve_arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return provider_command(&arguments, config, true);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SCAN_ARGUMENT)
    {
        return provider_scan_command(&arguments, config);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SETUP_ARGUMENT)
    {
        return setup_command(&arguments, config);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == POLICY_ARGUMENT)
    {
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "context")
            && arguments.len() == 2
        {
            return match serde_json::to_string_pretty(&policy_context_report(config)) {
                Ok(rendered) => {
                    println!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: unable to render policy context: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            match load_route_policy() {
                Some(policy) => match serde_json::to_string_pretty(&policy) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("xuva: unable to render route policy: {error}");
                        return ExitCode::FAILURE;
                    }
                },
                None => println!("No local route policy is installed."),
            }
            return ExitCode::SUCCESS;
        }
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "import")
            && arguments.len() == 3
        {
            return match import_route_policy(Path::new(&arguments[2]), config) {
                Ok(()) => {
                    println!("Imported local XUVA route policy.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!(
            "{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} policy [show|context] | policy import <evidence.json>"
        );
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == CALIBRATION_ARGUMENT)
    {
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            return match print_calibration(config.policy_objective) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("xuva: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!("{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} calibration [show]");
        return ExitCode::FAILURE;
    }
    if arguments.len() == 1 && arguments[0] == ADAPTER_INFO_ARGUMENT {
        print_adapter_info(config);
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && (arguments[0] == "gain" || arguments[0] == "stats") {
        return match XuvaMetrics::print_gain() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xuva: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let (arguments, requested_route, environment, explain) =
        match parse_options(arguments, config.route_preference, config.environment) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::FAILURE;
            }
        };
    if arguments.is_empty() {
        eprintln!(
            "{PRODUCT_COMMAND}: no command supplied; run `{PRODUCT_COMMAND} --help` for usage"
        );
        return ExitCode::FAILURE;
    }
    if is_shell_operator_command(&arguments) {
        eprintln!(
            "xuva: `{}` is shell syntax, not an executable; let PowerShell, cmd, or a POSIX shell own the pipeline and invoke XUVA only for command argv",
            arguments[0].to_string_lossy()
        );
        return ExitCode::from(2);
    }
    let mut invocation_config = config.clone();
    invocation_config.environment = environment;
    let current_directory = env::current_dir().ok();
    let same_host_raw_fast_path = matches!(
        invocation_config.invocation_origin,
        InvocationOrigin::Windows
    ) && !explain
        && (requested_route == Route::Raw
            || (requested_route == Route::Auto
                && !is_verified_read_only_git(&arguments)
                && should_use_native_git(
                    &arguments,
                    &invocation_config,
                    current_directory.as_deref().and_then(|path| path.to_str()),
                )));
    if same_host_raw_fast_path {
        return match adapters::windows::run(&arguments) {
            Ok(status) => ExitCode::from_status(status),
            Err(error) => {
                eprintln!("xuva: unable to start Windows raw command: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let started = Instant::now();
    let adaptive_context = adaptive_context_signature(&invocation_config);
    let policy = load_route_policy();
    let (initial_route, initial_reason) = if requested_route == Route::Auto {
        auto_route_for_environment(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            Some(&adaptive_context),
            environment,
            invocation_config.policy_objective,
        )
    } else {
        (requested_route, "explicit route preference")
    };
    let mut route = initial_route;
    let mut reason = initial_reason.to_owned();
    let calibration = if requested_route == Route::Auto {
        match calibration_plan(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            &adaptive_context,
            invocation_config.policy_objective,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("xuva: local calibration is unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    if let Some(plan) = &calibration {
        route = plan.route;
        reason = plan.reason.to_owned();
    }
    let selected_config = configured_wsl_backend(&invocation_config, route);
    let mut provider_missing = None;
    let mut execution_plan = None;
    let mut fallback_execution_plans = Vec::new();
    let mut selected_adapter = match route {
        Route::Raw => dispatcher::OutputAdapter::Raw,
        Route::NativeRtk | Route::Wsl1 | Route::Wsl2 | Route::Auto => {
            dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(&selected_config.native_rtk_path),
            }
        }
    };
    let explicit_plan = match explicit_executable_plan(&arguments, &invocation_config) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("xuva: {error}");
            return ExitCode::from(127);
        }
    };
    if let Some((plan, explicit_reason)) = explicit_plan {
        route = execution_route(&plan.candidate);
        selected_adapter = dispatcher::OutputAdapter::Raw;
        execution_plan = Some(plan);
        reason = explicit_reason;
    } else if requested_route == Route::Auto && environment == ExecutionEnvironment::Adaptive {
        trace(format!(
            "adaptive provider planning for {}",
            command_family(&arguments)
        ));
        match provider_dispatch_decision(&arguments, &invocation_config, route) {
            ProviderDispatchDecision::KeepStaticRoute => {}
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason: provider_reason,
            } => {
                route = execution_route(&plan.candidate);
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(*plan);
                fallback_execution_plans = fallbacks;
                reason = provider_reason;
            }
            ProviderDispatchDecision::Missing {
                reason: missing_reason,
            } => {
                provider_missing = Some(missing_reason.clone());
                reason = missing_reason;
            }
        }
    }
    if execution_plan.is_none()
        && matches!(
            invocation_config.invocation_origin,
            InvocationOrigin::Wsl { .. }
        )
        && matches!(route, Route::Raw | Route::NativeRtk)
    {
        match static_windows_execution_plan(&arguments, &invocation_config, route) {
            Ok(plan) => {
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(plan);
                reason = "WSL-origin Windows route requires an isolated execution plan".to_owned();
            }
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::from(127);
            }
        }
    }
    if explain {
        println!("route={}", route.as_str());
        println!("output_adapter={}", selected_adapter.as_str());
        println!("reason={reason}");
        println!("command_family={}", command_family(&arguments));
        if let Some(plan) = &execution_plan {
            let provider = match &plan.candidate {
                dispatcher::RouteCandidate::Windows { executable, .. }
                | dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
            };
            println!("provider={}", provider.to_string_lossy());
        }
        return if provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        };
    }
    if let Some(reason) = provider_missing {
        eprintln!("xuva: {reason}");
        return ExitCode::from(127);
    }
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    let mut console_installed = false;
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    } else if needs_console_handler {
        console_installed = true;
    }
    let metrics = begin_invocation_metrics(&invocation_config, &selected_adapter);
    let mut executed_route = route;
    let result = if let Some(plan) = execution_plan.as_ref() {
        let mut result = run_execution_plan(plan, &invocation_config, metrics.as_ref());
        for fallback in &fallback_execution_plans {
            if !result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                break;
            }
            let fallback_route = execution_route(&fallback.candidate);
            trace(format!(
                "selected provider executable was unavailable before child start; retrying {} candidate",
                fallback_route.as_str()
            ));
            if matches!(fallback_route, Route::Wsl1 | Route::Wsl2) && !console_installed {
                if !console::install() {
                    eprintln!(
                        "xuva: unable to register the Windows console cancellation handler for provider fallback"
                    );
                    return ExitCode::FAILURE;
                }
                console_installed = true;
            }
            executed_route = fallback_route;
            result = run_execution_plan(fallback, &invocation_config, metrics.as_ref());
        }
        result
    } else {
        match route {
            Route::Raw => adapters::windows::run(&arguments),
            Route::NativeRtk => {
                match run_native_rtk(&arguments, &selected_config, metrics.as_ref()) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && requested_route == Route::Auto
                            && environment == ExecutionEnvironment::Adaptive =>
                    {
                        trace(
                            "native RTK was not found; falling back to isolated WSL1 before any child started",
                        );
                        if !console_installed {
                            if !console::install() {
                                eprintln!(
                                    "xuva: unable to register the Windows console cancellation handler for WSL fallback"
                                );
                                return ExitCode::FAILURE;
                            }
                            console_installed = true;
                        }
                        executed_route = Route::Wsl1;
                        let fallback_config =
                            configured_wsl_backend(&invocation_config, Route::Wsl1);
                        run_wsl_route(
                            arguments.clone(),
                            &fallback_config,
                            Route::Wsl1,
                            metrics.as_ref(),
                        )
                    }
                    result => result,
                }
            }
            Route::Wsl1 | Route::Wsl2 => {
                run_wsl_route(arguments.clone(), &selected_config, route, metrics.as_ref())
            }
            Route::Auto => unreachable!("auto route is resolved before execution"),
        }
    };
    if console_installed {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    let elapsed = started.elapsed();
    let totals = if let Some(metrics) = metrics {
        match metrics.finish(
            executed_route.as_str(),
            command_family(&arguments),
            elapsed,
            exit_code,
        ) {
            Ok(totals) => totals,
            Err(error) => {
                eprintln!("xuva: metrics were not recorded: {error}");
                TokenTotals::default()
            }
        }
    } else {
        TokenTotals::default()
    };
    if invocation_config.metrics_enabled
        && let Some(plan) = &calibration
        && let Err(error) = record_calibration(plan, executed_route, elapsed, exit_code, totals)
    {
        eprintln!("xuva: local calibration was not recorded: {error}");
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!(
                "xuva: unable to start {} route: {error}",
                executed_route.as_str()
            );
            ExitCode::FAILURE
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config::from_lookup(|_| None).expect("default config is valid")
    }

    #[test]
    fn version_commands_are_owned_by_the_dispatcher() {
        for argument in [VERSION_ARGUMENT, "version", "-V"] {
            assert!(
                is_version_command(&[OsString::from(argument)]),
                "{argument}"
            );
        }
        assert!(is_verbose_version_command(&[
            OsString::from("--version"),
            OsString::from("--verbose")
        ]));
        assert!(!is_version_command(&[
            OsString::from("go"),
            OsString::from("version")
        ]));
    }

    #[test]
    fn forwards_special_characters_as_distinct_arguments() {
        let arguments = rtk_arguments(
            vec![
                OsString::from("run"),
                OsString::from("semi;and&dollar$HOME"),
                OsString::from("C:\\Program Files\\Example"),
            ],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );

        assert!(arguments.contains(&OsString::from("--exec")));
        assert!(arguments.contains(&OsString::from(LAUNCH_SCRIPT)));
        assert!(arguments.contains(&OsString::from("semi;and&dollar$HOME")));
        assert!(arguments.contains(&OsString::from("C:\\Program Files\\Example")));
    }

    #[test]
    fn wsl_bridge_payload_preserves_literal_utf8_argv_without_shell_parsing() {
        let fields = decode_wsl_bridge_fields("Z28AcnVuAHNwYWNlICYgJGRvbGxhclzmvKLlrZcA")
            .expect("valid base64 payload decodes");
        assert_eq!(
            fields,
            vec![
                "go".to_owned(),
                "run".to_owned(),
                "space & $dollar\\\u{6f22}\u{5b57}".to_owned(),
            ]
        );
        assert!(decode_wsl_bridge_fields("Z28=").is_err());
        assert!(decode_wsl_bridge_fields("not base64!").is_err());
    }

    #[test]
    fn wsl_bridge_request_carries_context_and_arguments_without_environment() {
        let request = wsl_bridge_request(&[OsString::from(
            "--wsl-bridge=djMAVWJ1bnR1AGJhZGFyAC9tbnQvZC9maXh0dXJlAEQ6XGZpeHR1cmUAL3RtcC9maXh0dXJlAHJhdwAtLWV4cGxhaW4tcm91dGUAZ28AcnVuAHgA",
        )])
        .expect("bridge payload is valid")
        .expect("argument selects the bridge");
        assert_eq!(request.distro, "Ubuntu");
        assert_eq!(request.origin_user, "badar");
        assert_eq!(request.cwd, "/mnt/d/fixture");
        assert_eq!(request.windows_cwd.as_deref(), Some(r"D:\fixture"));
        assert_eq!(request.extra_path.as_deref(), Some("/tmp/fixture"));
        assert_eq!(request.output_adapter, OutputAdapterPreference::Raw);
        assert_eq!(
            request.arguments,
            vec![
                OsString::from("--explain-route"),
                OsString::from("go"),
                OsString::from("run"),
                OsString::from("x"),
            ]
        );
    }

    #[test]
    fn wsl_plan_launcher_forwards_environment_as_structured_assignments() {
        let config = default_config();
        let arguments = plan_wsl_arguments_with_metrics(
            &OsString::from("/tmp/go"),
            &[OsString::from("run"), OsString::from("$literal & text")],
            &[(
                OsString::from("P7_OVERLAY"),
                OsString::from("value with spaces"),
            )],
            &config,
            Route::Wsl2,
            WslLaunchMetadata {
                cancel_nonce: Some("0123456789abcdef0123456789abcdef"),
                metrics_db_path: None,
                attestation_path: Some("/tmp/xuva-test.attestation"),
                permit_path: Some("/tmp/xuva-test.permit"),
                completion_path: Some("/tmp/xuva-test.completion"),
            },
        )
        .expect("WSL plan arguments are valid");
        let executable = arguments
            .iter()
            .position(|argument| argument == "/tmp/go")
            .expect("plan includes executable");
        let overlay = arguments
            .iter()
            .position(|argument| argument == "P7_OVERLAY=value with spaces")
            .expect("plan includes environment overlay");
        let user_argument = arguments
            .iter()
            .position(|argument| argument == "$literal & text")
            .expect("plan includes literal user argument");
        assert!(arguments.contains(&OsString::from(PLAN_LAUNCH_SCRIPT)));
        assert!(overlay < executable && executable < user_argument);
        assert!(
            wsl_environment_assignments(&[(
                OsString::from("INVALID-NAME"),
                OsString::from("value"),
            )])
            .is_err()
        );
    }

    #[test]
    fn execution_plan_applies_command_environment_and_cwd_to_windows_processes() {
        let request = dispatcher::CommandSpec {
            executable: OsString::from("fixture.exe"),
            arguments: vec![OsString::from("space value"), OsString::from("$literal")],
            cwd: Some(PathBuf::from(r"E:\work")),
            environment: vec![(OsString::from("P7_OVERLAY"), OsString::from("enabled"))],
            environment_policy: dispatcher::EnvironmentPolicy::Isolated,
            interactive: true,
        };
        let mut command = Command::new("fixture.exe");
        apply_command_spec(&mut command, &request);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![&OsString::from("space value"), &OsString::from("$literal")]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new(r"E:\work")));
        assert!(command.get_envs().any(|(key, value)| {
            key == "P7_OVERLAY" && value == Some(std::ffi::OsStr::new("enabled"))
        }));
    }

    #[test]
    fn explicit_wsl1_route_uses_the_windows_mutex_and_supervised_process_group() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        let command = wsl1_rtk_arguments(
            vec![
                OsString::from("proxy"),
                OsString::from("/usr/bin/printf"),
                OsString::from("%s"),
                OsString::from("space & $HOME"),
            ],
            &config,
        );
        let strings = command
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            strings
                .iter()
                .any(|value| value.contains("publish_completion"))
        );
        assert!(
            strings
                .iter()
                .any(|value| value.contains("/usr/bin/env -i"))
        );
        assert!(!strings.iter().any(|value| value.contains("/usr/bin/flock")));
        assert!(strings.iter().any(|value| value == "/usr/bin/setsid"));
        assert_eq!(
            strings.last().map(|value| value.as_ref()),
            Some("space & $HOME")
        );
    }

    #[test]
    fn every_wsl1_launch_surface_uses_the_same_strict_marker_validator() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        let rtk_arguments = wsl1_rtk_arguments_with_metrics(
            vec![OsString::from("smart")],
            &config,
            None,
            "/tmp/xuva-test.attestation",
            "/tmp/xuva-test.permit",
            "/tmp/xuva-test.completion",
        );
        let plan_arguments = plan_wsl_arguments_with_metrics(
            &OsString::from("/usr/bin/printf"),
            &[OsString::from("%s"), OsString::from("fixture")],
            &[],
            &config,
            Route::Wsl1,
            WslLaunchMetadata {
                cancel_nonce: None,
                metrics_db_path: None,
                attestation_path: Some("/tmp/xuva-test.attestation"),
                permit_path: Some("/tmp/xuva-test.permit"),
                completion_path: Some("/tmp/xuva-test.completion"),
            },
        )
        .expect("WSL1 plan arguments are valid");

        for arguments in [&rtk_arguments, &plan_arguments] {
            assert_eq!(
                arguments
                    .iter()
                    .filter(|argument| argument.as_os_str() == WSL1_MARKER_VALIDATOR_SCRIPT)
                    .count(),
                1,
                "each WSL1 launch must receive the canonical marker validator exactly once"
            );
            assert!(
                arguments
                    .iter()
                    .any(|argument| argument.as_os_str() == WSL1_LAUNCH_SCRIPT)
            );
        }
        assert!(!WSL1_LAUNCH_SCRIPT.contains("marker=/etc/xuva-dedicated-wsl1"));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("stat -Lc '%u:%a'"));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("!= \"0:444\""));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("grep -c '^installation_id='"));
    }

    #[test]
    fn wsl1_proxy_cannot_report_success_before_target_authorization() {
        let success = Command::new("cmd.exe")
            .args(["/d", "/c", "exit 0"])
            .status()
            .expect("successful proxy fixture starts");
        let rejected = verify_pre_authorization_proxy_status(success)
            .expect_err("pre-authorization success must not impersonate target success");
        assert!(rejected.to_string().contains("target was not executed"));

        let failure = Command::new("cmd.exe")
            .args(["/d", "/c", "exit 126"])
            .status()
            .expect("failed proxy fixture starts");
        assert_eq!(
            verify_pre_authorization_proxy_status(failure)
                .expect("launcher failure remains observable")
                .code(),
            Some(126)
        );
    }

    #[test]
    fn stats_remains_a_compatibility_alias() {
        let arguments = rtk_arguments(
            vec![OsString::from("stats")],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );
        assert_eq!(arguments.last(), Some(&OsString::from("gain")));
    }

    #[test]
    fn maps_windows_drive_paths_for_wsl_current_directory() {
        assert_eq!(
            windows_path_to_wsl_path(r"D:\projects\rtk-wsl"),
            Some("/mnt/d/projects/rtk-wsl".to_owned())
        );
        assert_eq!(
            windows_path_to_wsl_path(r"F:\path with spaces\漢字"),
            Some("/mnt/f/path with spaces/漢字".to_owned())
        );
        assert_eq!(
            windows_path_to_wsl_path(r"\\?\E:\projects\rtk-wsl"),
            Some("/mnt/e/projects/rtk-wsl".to_owned())
        );
        assert_eq!(windows_path_to_wsl_path(r"\\server\share"), None);
    }

    #[test]
    fn defaults_to_the_selected_wsl_users_home() {
        let arguments = rtk_arguments(
            vec![OsString::from("help")],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );

        assert!(arguments.contains(&OsString::from("")));
        assert!(arguments.iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("rtk_path=\"$HOME/.local/bin/rtk\"")
        }));
        assert!(!arguments.contains(&OsString::from("-u")));
    }

    #[test]
    fn validates_configuration_without_ambient_user_defaults() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            "XUVA_WSL_USER" => Some("alex".to_owned()),
            "XUVA_WSL_RTK_PATH" => Some("/opt/rtk/bin/rtk".to_owned()),
            "XUVA_WSL_CWD" => Some("/work/custom-mount".to_owned()),
            "XUVA_WSL_EXTRA_PATH" => Some("/opt/fixture-bin:/work/tools".to_owned()),
            _ => None,
        })
        .expect("portable config is valid");

        let arguments = rtk_arguments(
            vec![OsString::from("help")],
            &config,
            "0123456789abcdef0123456789abcdef",
        );
        assert!(arguments.windows(2).any(|pair| pair == ["-u", "alex"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--cd", "/work/custom-mount"])
        );
        assert!(arguments.contains(&OsString::from("/opt/rtk/bin/rtk")));
        assert!(arguments.contains(&OsString::from("/opt/fixture-bin:/work/tools")));
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_configuration() {
        let invalid_wait = Config::from_lookup(|name| match name {
            "XUVA_WSL_LOCK_WAIT_SECONDS" => Some("0".to_owned()),
            _ => None,
        });
        assert!(invalid_wait.is_err());

        let relative_path = Config::from_lookup(|name| match name {
            "XUVA_WSL_RTK_PATH" => Some("bin/rtk".to_owned()),
            _ => None,
        });
        assert!(relative_path.is_err());

        let invalid_extra_path = Config::from_lookup(|name| match name {
            "XUVA_WSL_EXTRA_PATH" => Some("relative:/opt/tools".to_owned()),
            _ => None,
        });
        assert!(invalid_extra_path.is_err());

        let invalid_objective = Config::from_lookup(|name| match name {
            "XUVA_POLICY_OBJECTIVE" => Some("fastest-ish".to_owned()),
            _ => None,
        });
        assert!(invalid_objective.is_err());
    }

    #[test]
    fn cancellation_uses_a_separate_structured_wsl_command() {
        let arguments = cancel_arguments(
            &default_config(),
            "0123456789abcdef0123456789abcdef",
            "TERM",
        );
        assert!(arguments.contains(&OsString::from(CANCEL_SCRIPT)));
        assert!(arguments.contains(&OsString::from("0123456789abcdef0123456789abcdef")));
        assert!(
            !arguments
                .iter()
                .any(|argument| { argument.to_string_lossy().starts_with("/tmp/xuva-") })
        );
        assert!(arguments.contains(&OsString::from("TERM")));
    }

    #[test]
    fn launch_permit_requires_the_exact_attested_identity_and_cleans_up() {
        let expected = "0123456789abcdef0123456789abcdef".to_owned();
        let (attestation, permit, completion);
        {
            let guard =
                LaunchPermitGuard::new("unit", expected.clone()).expect("launch guard is created");
            attestation = guard.attestation_windows_path.clone();
            permit = guard.permit_windows_path.clone();
            completion = guard.completion_windows_path.clone();
            let mut staging = attestation.as_os_str().to_os_string();
            staging.push(".staging");
            fs::write(PathBuf::from(staging), &expected).expect("staged attestation is written");
            assert!(
                !guard
                    .is_attested()
                    .expect("an unpublished attestation remains invisible")
            );
            fs::write(&attestation, "ffffffffffffffffffffffffffffffff")
                .expect("mismatched attestation is written");
            let mismatch = guard
                .is_attested()
                .expect_err("mismatched launch identity is rejected");
            assert_eq!(mismatch.kind(), std::io::ErrorKind::PermissionDenied);

            fs::write(&attestation, &expected).expect("matching attestation is written");
            assert!(guard.is_attested().expect("attestation is readable"));
            guard.authorize().expect("matching launch is authorized");
            assert_eq!(
                fs::read_to_string(&permit).expect("permit is readable"),
                expected
            );
            fs::write(&completion, format!("{expected}:37"))
                .expect("matching completion is written");
            assert_eq!(
                guard.completion_status().expect("completion is valid"),
                Some(37)
            );
            fs::write(&completion, format!("{expected}:999"))
                .expect("out-of-range completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("out-of-range completion is rejected")
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
            fs::write(&completion, "ffffffffffffffffffffffffffffffff:37")
                .expect("mismatched completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("mismatched completion identity is rejected")
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
            fs::write(&completion, "not-a-completion").expect("malformed completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("malformed completion is rejected")
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        assert!(!attestation.exists());
        assert!(!permit.exists());
        assert!(!completion.exists());
    }

    #[test]
    fn unbound_launch_permit_requires_explicit_identity_acceptance() {
        let installation_id = "01234567-89ab-cdef-0123-456789abcdef";
        let guard =
            LaunchPermitGuard::new_unbound("unit-unbound").expect("unbound guard is created");
        assert_eq!(
            guard
                .is_attested()
                .expect_err("unbound attestation cannot be accepted implicitly")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            guard
                .authorize()
                .expect_err("unbound permit cannot publish an implicit identity")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        fs::write(&guard.attestation_windows_path, installation_id)
            .expect("dedicated identity attestation is written");
        assert_eq!(
            guard
                .attested_value()
                .expect("attestation is readable")
                .as_deref(),
            Some(installation_id)
        );
        guard
            .authorize_value(installation_id)
            .expect("explicitly accepted dedicated identity is authorized");
        assert_eq!(
            fs::read_to_string(&guard.permit_windows_path).expect("permit is readable"),
            installation_id
        );
    }

    #[test]
    fn wsl2_launchers_only_remove_proven_dead_cancellation_tokens() {
        for script in [LAUNCH_SCRIPT, PLAN_LAUNCH_SCRIPT] {
            assert!(!script.contains("-mmin"));
            assert!(script.contains("/bin/kill -0 -- \"-$stale_worker\""));
            assert!(script.contains("group_has_other_members"));
            assert!(script.contains("publish_completion"));
            assert_eq!(
                script.matches("stat_fields=${stat_value##*) }").count(),
                2,
                "both process-group scans must parse after the final comm delimiter"
            );
            assert!(
                !script.contains("stat_fields=${stat_value#*) }"),
                "shortest-prefix /proc stat parsing can misread comm containing `) `"
            );

            let finish = script
                .split_once("finish() {")
                .expect("launcher has a finish trap")
                .1
                .split_once("trap finish EXIT")
                .expect("launcher finish trap is installed")
                .0;
            let failed_quiescence_exit = finish
                .find("exit 125")
                .expect("failed quiescence exits without attestation");
            let cleanup = finish
                .find("\n    cleanup\n")
                .expect("successful quiescence removes its token");
            let completion = finish
                .find("\n    publish_completion ")
                .expect("successful quiescence publishes completion");
            assert!(
                failed_quiescence_exit < cleanup && cleanup < completion,
                "cleanup and completion must remain unreachable after quiescence failure"
            );
            assert!(
                !finish[..failed_quiescence_exit].contains("publish_completion"),
                "failed quiescence must not publish cleanup proof"
            );
        }
    }

    #[test]
    fn routes_windows_worktree_git_to_native_git_by_default() {
        assert!(should_use_native_git(
            &[OsString::from("git"), OsString::from("status")],
            &default_config(),
            Some(r"E:\luthfi\project\flowpeek"),
        ));
    }

    #[test]
    fn keeps_explicit_wsl_git_paths_and_wsl_mode_in_wsl() {
        assert!(!should_use_native_git(
            &[
                OsString::from("git"),
                OsString::from("-C"),
                OsString::from("/mnt/e/project"),
                OsString::from("status")
            ],
            &default_config(),
            Some(r"E:\luthfi\project\flowpeek"),
        ));
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_GIT_MODE" => Some("wsl".to_owned()),
            _ => None,
        })
        .expect("WSL Git mode is valid");
        assert!(!should_use_native_git(
            &[OsString::from("git"), OsString::from("status")],
            &config,
            Some(r"E:\luthfi\project\flowpeek"),
        ));
    }

    #[test]
    fn validates_git_mode() {
        let invalid = Config::from_lookup(|name| match name {
            "XUVA_WSL_GIT_MODE" => Some("other".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn explicit_wsl1_backend_selects_the_isolated_distro_without_affecting_default_xuva() {
        let default = default_config();
        assert_eq!(default.backend, WslBackend::Auto);
        assert_eq!(default.distro, DEFAULT_DISTRO);

        let wsl1 = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        assert_eq!(wsl1.backend, WslBackend::Wsl1);
        assert_eq!(wsl1.distro, DEFAULT_WSL1_DISTRO);
    }

    #[test]
    fn explicit_backend_and_distro_select_the_xuva_wsl_provider() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl2".to_owned()),
            "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            _ => None,
        })
        .expect("explicit backend configuration is valid");
        assert_eq!(config.backend, WslBackend::Wsl2);
        assert_eq!(config.distro, "Ubuntu-24.04");

        let invalid = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("legacy".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn canonical_xuva_configuration_is_adaptive_by_default() {
        let xuva = default_config();
        assert_eq!(xuva.profile, ExecutableProfile::Xuva);
        assert_eq!(xuva.backend, WslBackend::Auto);
        assert_eq!(xuva.route_preference, Route::Auto);
        assert!(xuva.metrics_enabled);

        let metrics_off =
            Config::from_lookup(|name| (name == "XUVA_METRICS").then(|| "off".to_owned()))
                .expect("metrics can be disabled explicitly");
        assert!(!metrics_off.metrics_enabled);
        assert!(
            Config::from_lookup(|name| { (name == "XUVA_METRICS").then(|| "remote".to_owned()) })
                .is_err()
        );
    }

    #[test]
    fn embedded_command_surface_is_complete_and_non_overlapping() {
        let report = command_surface_report();
        assert_eq!(report.schema_version, 2);
        assert_eq!(report.adapter.name, "rtk");
        assert_eq!(report.adapter.version, "0.43.0");
        assert_eq!(report.adapter.protocol_version, 1);
        assert_eq!(report.upstream_command_count, 69);
        let names = report
            .commands
            .iter()
            .map(|row| row.command.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), report.upstream_command_count);
        assert!(
            report
                .commands
                .iter()
                .all(|row| row.classification != CommandSurface::Unknown)
        );
        assert_eq!(command_surface("git"), CommandSurface::NativeStructured);
        assert_eq!(command_surface("go"), CommandSurface::RawNative);
        assert_eq!(command_surface("proxy"), CommandSurface::Wsl1Conservative);
        assert_eq!(command_surface("gain"), CommandSurface::CoreInternal);
    }

    #[test]
    fn adapter_only_rtk_commands_never_enter_generic_provider_resolution() {
        let config = default_config();
        for command in ["smart", "proxy", "rewrite", "hook"] {
            assert!(is_adapter_only_rtk_command(command));
            let arguments = [OsString::from(command), OsString::from("literal-argument")];
            assert!(
                matches!(
                    provider_dispatch_decision(&arguments, &config, Route::Wsl1),
                    ProviderDispatchDecision::KeepStaticRoute
                ),
                "{command} must remain an adapter-owned RTK command"
            );
        }
        assert!(is_rtk_meta_command("wc"));
        assert!(requires_raw_posix_provider("wc"));
        assert!(!is_adapter_only_rtk_command("wc"));
    }

    #[test]
    fn xuva_auto_route_keeps_mutations_raw_and_read_only_commands_structured() {
        let mutation = vec![
            OsString::from("git"),
            OsString::from("commit"),
            OsString::from("-m"),
        ];
        assert_eq!(auto_route(&mutation, Some(r"E:\work"), None).0, Route::Raw);

        let clone = vec![
            OsString::from("git"),
            OsString::from("clone"),
            OsString::from("https://example.invalid/repo"),
        ];
        assert_eq!(auto_route(&clone, Some(r"E:\work"), None).0, Route::Raw);

        let read_only = vec![
            OsString::from("git"),
            OsString::from("log"),
            OsString::from("-1"),
        ];
        assert_eq!(
            auto_route(&read_only, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        let cargo = vec![
            OsString::from("cargo"),
            OsString::from("check"),
            OsString::from("--version"),
        ];
        assert_eq!(
            auto_route(&cargo, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        assert_eq!(
            auto_route(&[OsString::from("npm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("npx")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("pnpm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("go")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("dotnet")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("dart")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("flutter")], Some(r"E:\work"), None).0,
            Route::Raw
        );

        let literal = vec![
            OsString::from("proxy"),
            OsString::from("/usr/bin/printf"),
            OsString::from("$HOME; &"),
        ];
        assert_eq!(auto_route(&literal, Some(r"E:\work"), None).0, Route::Wsl1);
    }

    #[test]
    fn policy_uses_measured_savings_without_permitting_git_mutations() {
        let context = adaptive_context_signature(&default_config());
        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: context.clone(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "git:status".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 20.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "cargo:check".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "npm:run-list".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "go:test-all".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
            ],
        };
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("git"), OsString::from("status")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("rg"), OsString::from("needle")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("cargo"), OsString::from("check")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("npm"), OsString::from("run")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_with_context(
                &[
                    OsString::from("go"),
                    OsString::from("test"),
                    OsString::from("./...")
                ],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route(
                &[OsString::from("go"), OsString::from("test")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route(
                &[
                    OsString::from("npm"),
                    OsString::from("run"),
                    OsString::from("test")
                ],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route(
                &[
                    OsString::from("git"),
                    OsString::from("clone"),
                    OsString::from("url")
                ],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
    }

    #[test]
    fn policy_import_merge_preserves_other_evidence_and_replaces_same_key() {
        let existing = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "cargo:check".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 20.0,
                    token_savings_percent: 1.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 20.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
            ],
        };
        let incoming = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "npm:run-list".to_owned(),
                    raw_median_ms: 30.0,
                    candidate_median_ms: 40.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 5.0,
                    candidate_median_ms: 10.0,
                    token_savings_percent: 90.0,
                    sample_count: 5,
                },
            ],
        };
        let merged = merge_route_policy(Some(existing), incoming);
        assert_eq!(
            merged
                .evidence
                .iter()
                .map(|evidence| evidence.key.as_str())
                .collect::<Vec<_>>(),
            vec!["cargo:check", "npm:run-list", "rg"]
        );
        let rg = merged
            .evidence
            .iter()
            .find(|evidence| evidence.key == "rg")
            .expect("new measurement replaces rg");
        assert_eq!(rg.token_savings_percent, 90.0);
        assert_eq!(
            merged.route_for("cargo:check", "0123456789abcdef", PolicyObjective::Balanced,),
            Some(Route::Raw)
        );
        assert_eq!(
            merged.route_for(
                "npm:run-list",
                "0123456789abcdef",
                PolicyObjective::Balanced,
            ),
            Some(Route::Raw)
        );
    }

    #[test]
    fn adaptive_evidence_is_bound_to_manifest_and_local_adapter_context() {
        let default = default_config();
        let context = adaptive_context_signature(&default);
        let mut different = default.clone();
        different.native_rtk_path = r"C:\tools\other-rtk.exe".to_owned();
        assert_ne!(context, adaptive_context_signature(&different));

        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: context.clone(),
            evidence: vec![RoutePolicyEvidence {
                key: "rg".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 20.0,
                token_savings_percent: 0.0,
                sample_count: 5,
            }],
        };
        assert_eq!(
            policy.route_for("rg", &context, PolicyObjective::Balanced),
            Some(Route::Raw)
        );
        assert_eq!(
            policy.route_for("rg", "0123456789abcdef", PolicyObjective::Balanced),
            None
        );

        let entry = CalibrationEntry {
            signature: "fedcba9876543210".to_owned(),
            key: "rg".to_owned(),
            manifest_version: adapter_contract_id(),
            context_signature: context.clone(),
            raw_samples_ms: vec![1.0],
            native_samples: vec![NativeCalibrationSample {
                elapsed_ms: 2.0,
                input_tokens: 0,
                saved_tokens: 0,
            }],
        };
        assert!(calibration_entry_matches(
            &entry,
            "fedcba9876543210",
            &context
        ));
        assert!(!calibration_entry_matches(
            &entry,
            "fedcba9876543210",
            "0123456789abcdef"
        ));
    }

    #[test]
    fn xuva_route_options_are_explicit_and_validate_values() {
        let (arguments, route, environment, explain) = parse_options(
            vec![
                OsString::from("--route"),
                OsString::from("native-rtk"),
                OsString::from("--explain-route"),
                OsString::from("rg"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("route options are valid");
        assert_eq!(route, Route::NativeRtk);
        assert_eq!(environment, ExecutionEnvironment::Adaptive);
        assert!(explain);
        assert_eq!(arguments, vec![OsString::from("rg")]);
        assert!(
            parse_options(
                vec![OsString::from("--route"), OsString::from("unsafe")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );

        let (arguments, route, environment, explain) = parse_options(
            vec![
                OsString::from("--environment"),
                OsString::from("windows-only"),
                OsString::from("pytest"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("windows-only option is valid");
        assert_eq!(arguments, vec![OsString::from("pytest")]);
        assert_eq!(route, Route::Auto);
        assert_eq!(environment, ExecutionEnvironment::WindowsOnly);
        assert!(!explain);
        assert!(
            parse_options(
                vec![OsString::from("--environment"), OsString::from("hybrid")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );
    }

    #[test]
    fn windows_only_routes_external_commands_raw_and_keeps_rtk_meta_native() {
        assert_eq!(
            auto_route_for_environment(
                &[OsString::from("pytest"), OsString::from("-q")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_for_environment(
                &[OsString::from("init"), OsString::from("-g")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_for_environment(
                &[
                    OsString::from("git"),
                    OsString::from("commit"),
                    OsString::from("-m"),
                    OsString::from("x")
                ],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
    }

    #[test]
    fn decodes_and_parses_redirected_wsl_distribution_output() {
        let text = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
        let utf16 = text
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        let decoded = decode_wsl_output(&utf16);
        assert_eq!(distro_version_from_list(&decoded, "Ubuntu"), Some(2));
        assert_eq!(
            distro_version_from_list(&decoded, "Ubuntu-RTK-WSL1"),
            Some(1)
        );
        assert_eq!(
            distro_version_from_list(&decoded, "Custom WSL One"),
            Some(1)
        );
        assert_eq!(distro_version_from_list(&decoded, "missing"), None);
    }

    #[test]
    fn provider_discovery_parses_wsl_distro_names_and_versions() {
        let output = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
        assert_eq!(
            parse_wsl_distributions(output),
            vec![
                ("Ubuntu".to_owned(), Some(2)),
                ("Ubuntu-RTK-WSL1".to_owned(), Some(1)),
                ("Custom WSL One".to_owned(), Some(1)),
            ]
        );
        assert!(!is_eligible_wsl_distro("docker-desktop"));
        assert!(!is_eligible_wsl_distro("docker-desktop-data"));
        assert!(is_eligible_wsl_distro("Ubuntu-24.04"));
    }

    #[test]
    fn provider_discovery_classifies_windows_and_wsl_project_paths() {
        let windows = classify_project_path(r"E:\luthfi\project\rtk-wsl");
        assert_eq!(windows.kind, ProjectLocationKind::Windows);
        assert_eq!(windows.distro, None);

        let wsl = classify_project_path(r"\\wsl.localhost\Ubuntu-24.04\home\luthfi\project");
        assert_eq!(wsl.kind, ProjectLocationKind::Wsl);
        assert_eq!(wsl.distro.as_deref(), Some("Ubuntu-24.04"));
        assert_eq!(wsl.path, "/home/luthfi/project");
    }

    #[test]
    fn windows_provider_discovery_recognizes_native_launchable_extensions() {
        assert!(is_windows_launchable_path(r"C:\tools\go.exe"));
        assert!(is_windows_launchable_path(r"C:\tools\npm.cmd"));
        assert!(is_windows_launchable_path(r"C:\tools\gradle.bat"));
        assert!(is_windows_launchable_path(r"C:\tools\legacy.com"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm.ps1"));
        assert_eq!(
            select_windows_executable(vec![
                r"C:\tools\npm".to_owned(),
                r"C:\tools\npm.cmd".to_owned(),
                r"C:\tools\npm.ps1".to_owned(),
            ]),
            Some(r"C:\tools\npm.cmd".to_owned())
        );
        assert_eq!(
            select_windows_executable(vec![
                r"C:\tools\script.ps1".to_owned(),
                r"C:\tools\script.py".to_owned(),
            ]),
            None
        );
    }

    #[test]
    fn provider_cache_uses_a_bounded_freshness_window() {
        let entry = ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 100,
            inspection_level: InspectionLevel::Version,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: None,
                native_rtk: None,
                executable_version: None,
                version_probe_status: ProbeStatus::NotRequested,
                executable_capabilities: Vec::new(),
                executable_identity: None,
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: Vec::new(),
        };
        assert!(cache_entry_is_fresh(
            &entry,
            100 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
        assert!(
            !cache_entry_is_fresh(&entry, 100, "changed-path-or-git-revision", true),
            "a changed discovery fingerprint invalidates even a new entry"
        );
        assert!(!cache_entry_is_fresh(
            &entry,
            101 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
        let mut identity_only = entry.clone();
        identity_only.inspection_level = InspectionLevel::Identity;
        assert!(cache_entry_is_fresh(&identity_only, 100, "fixture", false));
        assert!(
            !cache_entry_is_fresh(&identity_only, 100, "fixture", true),
            "doctor/version verification must upgrade an identity-only cache entry"
        );
    }

    #[test]
    fn version_probe_registry_never_executes_unknown_tools() {
        assert_eq!(version_probe_arguments("git"), Some(&["--version"][..]));
        assert_eq!(version_probe_arguments("go"), Some(&["version"][..]));
        assert_eq!(version_probe_arguments("user-defined-tool"), None);
    }

    #[test]
    fn explicit_windows_executable_paths_bypass_provider_discovery() {
        let fixture = env::temp_dir().join(format!(
            "xuva-explicit-path-{}-{}.cmd",
            std::process::id(),
            unix_seconds()
        ));
        fs::write(&fixture, "@exit /b 0\r\n").expect("explicit fixture is written");
        let arguments = vec![
            fixture.clone().into_os_string(),
            OsString::from("literal argument"),
        ];
        let (plan, reason) = explicit_executable_plan(&arguments, &default_config())
            .expect("explicit path is valid")
            .expect("explicit path creates a plan");
        assert!(matches!(
            plan.candidate,
            dispatcher::RouteCandidate::Windows { .. }
        ));
        assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
        assert_eq!(
            plan.request.arguments,
            vec![OsString::from("literal argument")]
        );
        assert!(reason.contains("explicit Windows"));
        fs::remove_file(fixture).expect("explicit fixture is removed");
    }

    #[test]
    fn provider_cache_fingerprint_changes_with_wsl_extra_path() {
        let default = default_config();
        let configured = Config::from_lookup(|name| match name {
            "XUVA_WSL_EXTRA_PATH" => Some("/tmp/xuva-go/bin".to_owned()),
            _ => None,
        })
        .expect("extra path configuration is valid");
        assert_ne!(
            discovery_context_signature(&default, false),
            discovery_context_signature(&configured, false),
            "changing the executable search overlay must invalidate discovery"
        );
    }

    #[test]
    fn provider_resolution_requires_a_verified_cross_host_project_mapping() {
        let probe = WslToolProbe {
            distro: "Ubuntu".to_owned(),
            user: None,
            wsl_version: Some(2),
            dedicated: false,
            installation_id: None,
            executable: Some("/usr/bin/go".to_owned()),
            rtk: Some("/home/test/.local/bin/rtk".to_owned()),
            executable_version: None,
            version_probe_status: ProbeStatus::NotRequested,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert_eq!(
            wsl_project_path_with(
                &windows_project,
                &probe,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, r"E:\work");
                    None
                },
                |_, _| true,
            ),
            None
        );
        assert_eq!(
            wsl_project_path_with(
                &windows_project,
                &probe,
                |_, _| Some("/mnt/e/work".to_owned()),
                |distro, path| distro == "Ubuntu" && path == "/mnt/e/work",
            ),
            Some("/mnt/e/work".to_owned())
        );

        let same_wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert_eq!(
            wsl_project_path_with(
                &same_wsl_project,
                &probe,
                |_, _| None,
                |distro, path| distro == "Ubuntu" && path == "/home/test/work",
            ),
            Some("/home/test/work".to_owned())
        );

        assert_eq!(
            wsl_project_path_with(&same_wsl_project, &probe, |_, _| None, |_, _| false,),
            None
        );

        let bridged_other_distro_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/mnt/host/d/work".to_owned(),
            distro: Some("docker-desktop".to_owned()),
            windows_path: Some(r"D:\work".to_owned()),
        };
        assert_eq!(
            wsl_project_path_with(
                &bridged_other_distro_project,
                &probe,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, r"D:\work");
                    Some("/mnt/d/work".to_owned())
                },
                |distro, path| distro == "Ubuntu" && path == "/mnt/d/work",
            ),
            Some("/mnt/d/work".to_owned()),
            "a WSL-origin bridge may cross distros only through a verified Windows-mounted path"
        );

        let mapping =
            wsl_mapping_arguments_with_user("Ubuntu", None, r"E:\work with spaces\$literal");
        assert_eq!(
            mapping,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-a"),
                OsString::from(r"E:\work with spaces\$literal"),
            ]
        );
        assert_eq!(
            wsl_mapping_arguments_with_user("Ubuntu", Some("luthfi"), r"E:\work"),
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("-u"),
                OsString::from("luthfi"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-a"),
                OsString::from(r"E:\work"),
            ]
        );
    }

    #[test]
    fn provider_resolution_verifies_wsl_to_windows_project_mappings() {
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work with spaces\漢字".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert_eq!(
            windows_project_path_with(
                &windows_project,
                |_, _| None,
                |path| { path == r"E:\work with spaces\漢字" }
            ),
            Some(r"E:\work with spaces\漢字".to_owned())
        );

        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/luthfi/work with spaces/漢字".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, "/home/luthfi/work with spaces/漢字");
                    Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
                },
                |path| path.contains("work with spaces"),
            ),
            Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
        );
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |_, _| Some(r"\\wsl.localhost\Other\home\luthfi\work".to_owned()),
                |_| true,
            ),
            None,
            "a mapped UNC path must name the source WSL distribution"
        );
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |_, _| Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work".to_owned()),
                |_| false,
            ),
            None,
            "a path that Windows cannot read is never executable"
        );

        let arguments = windows_mapping_arguments_with_user(
            "Ubuntu",
            None,
            "/home/luthfi/work with spaces/$literal",
        );
        assert_eq!(
            arguments,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-w"),
                OsString::from("-a"),
                OsString::from("/home/luthfi/work with spaces/$literal"),
            ]
        );
        assert_eq!(
            windows_mapping_arguments_with_user("Ubuntu", Some("luthfi"), "/home/luthfi/work"),
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("-u"),
                OsString::from("luthfi"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-w"),
                OsString::from("-a"),
                OsString::from("/home/luthfi/work"),
            ]
        );
    }

    #[test]
    fn provider_aware_go_routing_uses_only_a_complete_verified_wsl_candidate() {
        let config = default_config();
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                distro: Some("Ubuntu-22.04".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture: a verified WSL provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter.as_str(), "raw");
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref distro, ref cwd, .. }
                        if distro == "Ubuntu-22.04" && cwd == Path::new("/mnt/e/work")
                ));
                assert!(reason.contains("verified project path"));
            }
            _ => panic!("expected verified WSL provider selection"),
        }
    }

    #[test]
    fn provider_aware_go_routing_runs_a_wsl_only_go_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Go exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };
        assert!(
            has_complete_go_provider(&resolution),
            "a verified WSL raw Go binary is ready and must not trigger setup"
        );
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/usr/local/go/bin/go")
                ));
                assert!(reason.contains("raw output adapter"));
            }
            _ => panic!("expected the WSL-only raw Go provider"),
        }
    }

    #[test]
    fn generic_windows_executable_overrides_an_unavailable_legacy_wsl_route() {
        let project_path = env::current_dir()
            .expect("test project directory exists")
            .to_string_lossy()
            .to_string();
        let resolution = resolve_tool_provider_from_discovery_with_user(
            "nvm",
            ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: project_path.clone(),
                distro: None,
                windows_path: None,
            },
            ProviderCacheEntry {
                tool: "nvm".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\\Users\\test\\AppData\\Local\\nvm\\nvm.exe".to_owned()),
                    native_rtk: None,
                    executable_version: Some("1.2.2".to_owned()),
                    version_probe_status: ProbeStatus::Success,
                    executable_capabilities: vec!["version".to_owned()],
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: vec![WslToolProbe {
                    distro: "Ubuntu-RTK-WSL1".to_owned(),
                    user: None,
                    wsl_version: Some(1),
                    dedicated: true,
                    installation_id: Some("00000000-0000-0000-0000-000000000001".to_owned()),
                    executable: None,
                    rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    rtk_identity: None,
                }],
            },
            "miss",
            None,
        );

        assert_eq!(resolution.candidates.len(), 1);
        assert_eq!(resolution.availability.wsl[0].executable, None);
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("nvm"), OsString::from("ls")],
            &default_config(),
            Route::Wsl1,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { .. }
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(fallbacks.is_empty());
            }
            _ => {
                panic!("an unavailable WSL1 provider must not block generic Windows raw execution")
            }
        }
    }

    #[test]
    fn provider_planning_retains_the_next_eligible_route_for_pre_start_fallback() {
        let raw_config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let candidate = |distro: &str, version, executable: &str| ProviderCandidate {
            host: if version == 1 {
                ProviderHost::Wsl1
            } else {
                ProviderHost::Wsl2
            },
            adapters: vec![AdapterKind::Raw],
            distro: Some(distro.to_owned()),
            wsl_version: Some(version),
            executable: executable.to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![
                candidate("Ubuntu-RTK-WSL1", 1, "/opt/go-wsl1/bin/go"),
                candidate("Ubuntu", 2, "/opt/go-wsl2/bin/go"),
            ],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &raw_config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl1);
                assert_eq!(fallbacks.len(), 1);
                assert_eq!(execution_route(&fallbacks[0].candidate), Route::Wsl2);
            }
            _ => panic!("the usable WSL2 candidate must remain available as fallback"),
        }
    }

    #[test]
    fn generic_dispatcher_routes_a_wsl_only_cargo_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/home/test/.cargo/bin/cargo".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/home/test/.cargo/bin/cargo")
                ));
                assert!(reason.contains("cargo discovery"));
            }
            _ => panic!("expected the WSL-only raw Cargo provider"),
        }
    }

    #[test]
    fn generic_dispatcher_falls_back_to_verified_windows_raw_when_rtk_is_absent() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Users\test\.cargo\bin\cargo.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Windows,
                adapters: vec![AdapterKind::Raw],
                distro: None,
                wsl_version: None,
                executable: r"C:\Users\test\.cargo\bin\cargo.exe".to_owned(),
                rtk: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists on Windows without RTK".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &default_config(),
            Route::NativeRtk,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { ref executable, .. }
                        if executable == &OsString::from(r"C:\Users\test\.cargo\bin\cargo.exe")
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(reason.contains("on Windows"));
            }
            _ => panic!("expected Windows raw fallback when native RTK is absent"),
        }
    }

    #[test]
    fn generic_dispatcher_discovers_every_safe_executable_name() {
        for tool in [
            "go",
            "cargo",
            "rustc",
            "node",
            "nvm",
            "npm",
            "pnpm",
            "python",
            "python3",
            "pytest",
            "java",
            "gradle",
            "mvn",
            "dotnet",
            "git",
            "tool.name",
            "cargo-next",
        ] {
            assert!(
                is_dispatchable_provider_tool(&[OsString::from(tool)]),
                "{tool}"
            );
        }
        assert!(!is_dispatchable_provider_tool(&[OsString::from("cmd /c")]));
        assert!(!is_dispatchable_provider_tool(&[OsString::from("go;exit")]));
    }

    #[test]
    fn execution_plan_rejects_inconsistent_provider_host_metadata() {
        let candidate = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let error = execution_plan_for_provider_candidate(
            "go",
            &[OsString::from("version")],
            &default_config(),
            &candidate,
        )
        .expect_err("host and WSL metadata must not contradict each other");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn requested_rtk_adapter_does_not_silently_downgrade_to_raw() {
        let candidate = ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        assert!(provider_adapter(&candidate, OutputAdapterPreference::Rtk).is_err());

        let adapter_only = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Rtk],
            distro: None,
            wsl_version: None,
            executable: r"C:\tools\rtk.exe".to_owned(),
            rtk: Some(r"C:\tools\rtk.exe".to_owned()),
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        assert!(provider_adapter(&adapter_only, OutputAdapterPreference::Raw).is_err());
        assert!(matches!(
            provider_adapter(&adapter_only, OutputAdapterPreference::Auto),
            Ok(dispatcher::OutputAdapter::Rtk { .. })
        ));
    }

    #[test]
    fn provider_aware_go_routing_reports_missing_without_an_install_action() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &default_config(),
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::Missing { reason } => {
                assert!(reason.contains("does not execute shell builtins implicitly"));
                assert!(reason.contains("doctor go"));
            }
            _ => panic!("expected a missing-provider diagnostic"),
        }
    }

    #[test]
    fn setup_plan_proposes_only_a_reviewable_windows_go_command() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: Some(r"C:\tools\rtk.exe".to_owned()),
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        let plan = setup_go_plan_from_resolution(&resolution, true);
        assert_eq!(plan.status, "planned");
        assert_eq!(plan.proposed_provider, Some("windows-winget"));
        assert_eq!(plan.apply, "unavailable_in_pd4");
        assert_eq!(
            plan.proposed_command,
            Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ])
        );
    }

    #[test]
    fn setup_plan_never_selects_an_installer_when_a_provider_is_ready_or_blocked() {
        let ready = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Go\bin\go.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: Windows Go is available".to_owned(),
            install: "disabled_in_pd1",
        };
        let ready_plan = setup_go_plan_from_resolution(&ready, false);
        assert_eq!(ready_plan.status, "ready");
        assert_eq!(ready_plan.proposed_command, None);
        assert_eq!(ready_plan.apply, "not_needed");

        let blocked = ProviderResolution {
            availability: ProviderCacheEntry {
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                ..ready.availability.clone()
            },
            ..ready
        };
        let blocked_plan = setup_go_plan_from_resolution(&blocked, true);
        assert_eq!(blocked_plan.status, "blocked");
        assert_eq!(blocked_plan.proposed_command, None);
        assert_eq!(blocked_plan.apply, "unavailable_in_pd4");
    }

    #[test]
    fn setup_recovery_never_replays_an_installer() {
        let (verified_status, verified_detail) = setup_recovery_outcome(true);
        assert_eq!(verified_status, "recovered_verified");
        assert!(verified_detail.contains("no installer was replayed"));

        let (required_status, required_detail) = setup_recovery_outcome(false);
        assert_eq!(required_status, "recovery_required");
        assert!(required_detail.contains("no installer was replayed"));
    }

    #[test]
    fn cached_windows_go_skips_cross_host_resolution_when_it_is_sufficient() {
        let windows = WindowsToolProbe {
            executable: Some(r"C:\Program Files\Go\bin\go.exe".to_owned()),
            native_rtk: None,
            executable_version: None,
            version_probe_status: ProbeStatus::NotRequested,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            native_rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert!(windows_tool_is_usable(
            "go",
            &windows_project,
            Route::Raw,
            &windows
        ));
        assert!(!windows_tool_is_usable(
            "go",
            &windows_project,
            Route::NativeRtk,
            &windows
        ));
        assert!(
            !windows_tool_is_usable("go", &windows_project, Route::Wsl1, &windows),
            "a conservative WSL fallback must not suppress Windows provider resolution"
        );
        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert!(!windows_tool_is_usable(
            "go",
            &wsl_project,
            Route::Raw,
            &windows
        ));
        assert!(
            !windows_tool_is_usable("find", &windows_project, Route::Raw, &windows),
            "Windows find.exe must never satisfy POSIX find semantics"
        );
        let mut structured = windows.clone();
        structured.native_rtk = Some(r"C:\Tools\rtk.exe".to_owned());
        assert!(windows_tool_is_usable(
            "go",
            &windows_project,
            Route::NativeRtk,
            &structured
        ));
        assert!(
            windows_tool_is_usable("find", &windows_project, Route::NativeRtk, &structured),
            "Windows RTK find is a structured adapter, not raw find.exe"
        );
    }

    #[test]
    fn cross_host_isolation_uses_origin_identity_and_preserves_unc_cwd() {
        let mut config = default_config();
        config.invocation_origin = InvocationOrigin::Wsl {
            distro: "Ubuntu".to_owned(),
        };
        config.cwd = Some("/home/test/project".to_owned());
        config.bridge_windows_cwd = Some(r"\\wsl.localhost\Ubuntu\home\test\project".to_owned());
        let candidate = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Tools\tool.exe".to_owned(),
            rtk: None,
            project_path: config.bridge_windows_cwd.clone(),
            usable: true,
            reason: "fixture".to_owned(),
        };

        assert_eq!(
            provider_environment_policy(&config, &candidate),
            dispatcher::EnvironmentPolicy::Isolated
        );
        assert_eq!(
            windows_cwd_for_invocation(&config).expect("UNC mapping is usable"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\test\project")
        );

        config.invocation_origin = InvocationOrigin::Windows;
        assert_eq!(
            provider_environment_policy(&config, &candidate),
            dispatcher::EnvironmentPolicy::Inherit
        );
    }

    #[test]
    fn explicit_provider_selection_skips_adapter_incompatible_candidates() {
        let mut config = default_config();
        config.output_adapter = OutputAdapterPreference::Rtk;
        let candidates = vec![
            ProviderCandidate {
                host: ProviderHost::Windows,
                adapters: vec![AdapterKind::Raw],
                distro: None,
                wsl_version: None,
                executable: r"C:\Tools\tool.exe".to_owned(),
                rtk: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "raw-only Windows fixture".to_owned(),
            },
            ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/bin/tool".to_owned(),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "RTK-capable WSL fixture".to_owned(),
            },
        ];

        let (index, candidate, plan) =
            first_compatible_provider_plan("tool", &[], &config, &candidates)
                .expect("a compatible provider exists");
        assert_eq!(index, 1);
        assert_eq!(candidate.host, ProviderHost::Wsl2);
        assert!(matches!(
            plan.adapter,
            dispatcher::OutputAdapter::Rtk { .. }
        ));
    }

    #[test]
    fn local_calibration_bootstraps_then_requires_validation_before_stable() {
        assert_eq!(
            calibration_route_for(None, PolicyObjective::Balanced).0,
            Route::NativeRtk
        );

        let mut entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            raw_samples_ms: Vec::new(),
            native_samples: vec![NativeCalibrationSample {
                elapsed_ms: 30.0,
                input_tokens: 100,
                saved_tokens: 0,
            }],
        };
        assert_eq!(
            calibration_route_for(Some(&entry), PolicyObjective::Balanced).0,
            Route::Raw
        );

        entry.raw_samples_ms.push(10.0);
        entry.native_samples.push(NativeCalibrationSample {
            elapsed_ms: 30.0,
            input_tokens: 100,
            saved_tokens: 0,
        });
        assert_eq!(entry.phase(), "provisional");
        assert_eq!(
            calibration_route_for(Some(&entry), PolicyObjective::Balanced).0,
            Route::Raw
        );

        entry.raw_samples_ms.push(10.0);
        assert_eq!(entry.phase(), "stable");
        assert_eq!(
            calibration_route_for(Some(&entry), PolicyObjective::Balanced).0,
            Route::Raw
        );
    }

    #[test]
    fn local_calibration_discards_stale_adapter_contract_without_failing() {
        let stale = CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: vec![CalibrationEntry {
                signature: "0123456789abcdef".to_owned(),
                key: "rg".to_owned(),
                manifest_version: "wad:0.42.0:protocol-1".to_owned(),
                context_signature: "fedcba9876543210".to_owned(),
                raw_samples_ms: vec![1.0],
                native_samples: vec![NativeCalibrationSample {
                    elapsed_ms: 2.0,
                    input_tokens: 10,
                    saved_tokens: 5,
                }],
            }],
        };

        let migrated =
            calibration_for_current_contract(stale).expect("stale evidence is safely ignored");
        assert_eq!(migrated.schema_version, CALIBRATION_SCHEMA_VERSION);
        assert!(migrated.entries.is_empty());
    }

    #[test]
    fn local_calibration_prioritizes_measured_token_savings() {
        let entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            raw_samples_ms: vec![10.0, 11.0],
            native_samples: vec![
                NativeCalibrationSample {
                    elapsed_ms: 30.0,
                    input_tokens: 50,
                    saved_tokens: 10,
                },
                NativeCalibrationSample {
                    elapsed_ms: 31.0,
                    input_tokens: 50,
                    saved_tokens: 15,
                },
            ],
        };
        assert_eq!(entry.phase(), "stable");
        assert_eq!(
            entry.selected_route(PolicyObjective::Balanced),
            Route::NativeRtk
        );
        assert_eq!(entry.selected_route(PolicyObjective::Latency), Route::Raw);
        assert_eq!(
            select_adaptive_route(Some(10.0), Some(30.0), 1.0, PolicyObjective::Tokens,),
            Route::NativeRtk
        );
        assert_eq!(median(&[1.0, 3.0]), Some(2.0));
    }

    #[test]
    fn policy_objective_is_part_of_the_local_evidence_context() {
        let balanced = default_config();
        let mut latency = balanced.clone();
        latency.policy_objective = PolicyObjective::Latency;
        assert_ne!(
            adaptive_context_signature(&balanced),
            adaptive_context_signature(&latency)
        );
    }

    #[test]
    fn local_calibration_is_limited_to_safe_command_contracts() {
        assert_eq!(
            calibration_key(&[OsString::from("git"), OsString::from("status")]),
            Some("git:read-only")
        );
        assert_eq!(
            calibration_key(&[OsString::from("rg"), OsString::from("needle")]),
            Some("rg")
        );
        assert_eq!(
            calibration_key(&[
                OsString::from("go"),
                OsString::from("test"),
                OsString::from("./...")
            ]),
            Some("go:test-all")
        );
        assert_eq!(
            calibration_key(&[OsString::from("cargo"), OsString::from("test")]),
            None
        );
        assert_eq!(
            calibration_key(&[OsString::from("git"), OsString::from("commit")]),
            None
        );
    }

    #[test]
    fn local_calibration_signature_does_not_expose_command_text() {
        let arguments = vec![OsString::from("rg"), OsString::from("sensitive value")];
        let signature = calibration_signature(&arguments, Some(r"E:\work"));
        assert_eq!(signature.len(), 16);
        assert!(!signature.contains("sensitive"));
        assert_ne!(
            signature,
            calibration_signature(
                &[OsString::from("rg"), OsString::from("other")],
                Some(r"E:\work")
            )
        );
    }

    #[test]
    fn provider_registry_accepts_safe_generic_tool_names_only() {
        for tool in ["git", "python3", "cargo-next", "tool.name", "go"] {
            assert!(
                is_safe_provider_tool_name(tool),
                "{tool} should be accepted"
            );
        }
        for tool in ["", "../tool", "tool/path", "tool;echo", "tool name", "工具"] {
            assert!(
                !is_safe_provider_tool_name(tool),
                "{tool} should be rejected"
            );
        }
    }

    #[test]
    fn provider_registry_parses_wsl_binary_identity_without_retaining_command_output() {
        let identity = parse_wsl_binary_identity(
            Some("/usr/local/bin/rtk".to_owned()),
            Some("2291200:1721880000".to_owned()),
        )
        .expect("valid stat identity is parsed");
        assert_eq!(identity.path, "/usr/local/bin/rtk");
        assert_eq!(identity.size_bytes, 2_291_200);
        assert_eq!(identity.modified_unix_seconds, 1_721_880_000);
        assert!(
            parse_wsl_binary_identity(Some("/bin/tool".to_owned()), Some("bad".to_owned()))
                .is_none()
        );
    }

    #[test]
    fn provider_fingerprint_separates_partial_and_complete_wsl_inventory() {
        let config = default_config();
        assert_ne!(
            discovery_context_signature(&config, false),
            discovery_context_signature(&config, true),
            "a Windows-only cache must not satisfy a complete WSL inventory request"
        );
    }

    #[test]
    fn wsl_provider_probe_rejects_shell_builtins_as_executables() {
        assert_eq!(verified_wsl_executable_path("read".to_owned()), None);
        assert_eq!(
            verified_wsl_executable_path("/usr/bin/find".to_owned()),
            Some("/usr/bin/find".to_owned())
        );
    }

    #[test]
    fn posix_command_families_do_not_collide_with_windows_system_tools() {
        for tool in ["find", "head", "tail", "grep", "tree"] {
            assert!(
                !windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
                "{tool}"
            );
            assert!(
                windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk),
                "{tool}"
            );
        }
        for tool in ["find", "head", "tail", "tree"] {
            assert!(requires_raw_posix_provider(tool), "{tool}");
        }
        assert!(!requires_raw_posix_provider("grep"));
        for tool in ["git", "cargo", "python3"] {
            assert!(
                windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
                "{tool}"
            );
        }
    }

    #[test]
    fn execution_plans_translate_only_cross_host_absolute_path_arguments() {
        let windows = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
            rtk: None,
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let plan = execution_plan_for_provider_candidate(
            "git",
            &[
                OsString::from("-C"),
                OsString::from("/mnt/e/work"),
                OsString::from("status"),
                OsString::from("literal && value"),
            ],
            &default_config(),
            &windows,
        )
        .expect("Windows plan is valid");
        assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
        assert_eq!(
            plan.request.arguments[3],
            OsString::from("literal && value"),
            "non-path argv remains byte-for-byte structured"
        );

        let wsl = ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/bin/rtk".to_owned(),
            rtk: Some("/usr/local/bin/rtk".to_owned()),
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let plan = execution_plan_for_provider_candidate(
            "read",
            &[OsString::from(r"E:\work\Cargo.toml")],
            &default_config(),
            &wsl,
        )
        .expect("WSL plan is valid");
        assert_eq!(
            plan.request.arguments,
            vec![OsString::from("/mnt/e/work/Cargo.toml")]
        );
    }

    #[test]
    fn environment_forwarding_is_allowlisted_and_secret_averse() {
        let explicit = HashSet::from(["PROJECT_MODE", "GIT_AUTHOR_NAME"]);
        let defaults = ["CI"];
        assert!(should_forward_environment(
            "XPDE_RUN_TRAINING_E2E",
            Some("1"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "PROJECT_MODE",
            Some("training"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "CI",
            Some("true"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "GIT_AUTHOR_NAME",
            Some("XUVA Contract"),
            &explicit,
            &defaults
        ));
        assert!(!should_forward_environment(
            "PROJECT_RUN_MODE",
            Some("training"),
            &explicit,
            &defaults
        ));
        assert!(!should_forward_environment(
            "PROJECT_SECRET_TOKEN",
            Some("1"),
            &HashSet::from(["PROJECT_SECRET_TOKEN"]),
            &defaults
        ));
        assert!(
            Config::from_lookup(
                |name| (name == "XUVA_ENV_ALLOWLIST").then(|| "SAFE_FLAG,API_TOKEN".to_owned())
            )
            .is_err()
        );
    }

    #[test]
    fn windows_git_mutations_have_no_wsl_execution_fallback() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "git".to_owned(),
            cache: "hit",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "git".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Program Files\Git\cmd\git.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![
                ProviderCandidate {
                    host: ProviderHost::Windows,
                    adapters: vec![AdapterKind::Raw],
                    distro: None,
                    wsl_version: None,
                    executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
                    rtk: None,
                    project_path: Some(r"E:\work".to_owned()),
                    usable: true,
                    reason: "fixture".to_owned(),
                },
                ProviderCandidate {
                    host: ProviderHost::Wsl2,
                    adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                    distro: Some("Ubuntu".to_owned()),
                    wsl_version: Some(2),
                    executable: "/usr/bin/git".to_owned(),
                    rtk: Some("/usr/local/bin/rtk".to_owned()),
                    project_path: Some("/mnt/e/work".to_owned()),
                    usable: true,
                    reason: "fixture".to_owned(),
                },
            ],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled",
        };
        match provider_dispatch_decision_from_resolution(
            &[
                OsString::from("git"),
                OsString::from("-C"),
                OsString::from("/mnt/e/work"),
                OsString::from("push"),
                OsString::from("origin"),
                OsString::from("HEAD"),
            ],
            &default_config(),
            Route::Wsl1,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason,
            } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { .. }
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(fallbacks.is_empty());
                assert!(reason.contains("Windows DNS"));
                assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
            }
            _ => panic!("Windows Git mutation must produce a native-only plan"),
        }
    }

    #[test]
    fn shell_operator_and_update_check_ux_are_owned_by_xuva() {
        assert!(is_shell_operator_command(&[OsString::from("&&")]));
        assert!(!is_shell_operator_command(&[
            OsString::from("rg"),
            OsString::from("literal && value")
        ]));
        let tags = "a refs/tags/v0.3.0\nb refs/tags/not-semver\nc refs/tags/v0.4.1\n";
        assert_eq!(
            latest_release_from_ls_remote(tags).as_deref(),
            Some("v0.4.1")
        );
        assert_eq!(parsed_stable_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parsed_stable_version("v1.2.3-rc1"), None);
        assert!(stable_release_is_newer("v1.2.3", "1.2.3-beta.1"));
        assert!(stable_release_is_newer("v1.2.4", "1.2.3-beta.1"));
        assert!(!stable_release_is_newer("v1.2.2", "1.2.3-beta.1"));
        assert!(!stable_release_is_newer("v1.2.3", "1.2.3"));
    }
}
