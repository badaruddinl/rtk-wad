use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::adapters::rtk::{CommandSurface, adapter_version_is_compatible, command_surface};
use crate::config::Config;
use crate::diagnostics::trace;
use crate::process;
use crate::wsl::{exec_prefix as wsl_exec_prefix, valid_installation_id};

use super::cache::{
    cache_entry_is_fresh, discovery_context_signature, load_provider_cache, unix_seconds,
    update_provider_cache,
};
use super::discovery::{
    VersionProbe, configured_windows_executable, decode_wsl_output, first_windows_executable,
    installed_wsl_distributions, parse_wsl_binary_identity, tool_version, version_capabilities,
    windows_binary_identity,
};
use super::model::{
    InspectionLevel, ProbeStatus, ProviderCacheEntry, WindowsToolProbe, WslToolProbe,
};

const WSL1_MARKER_VALIDATOR_SCRIPT: &str = include_str!("../scripts/wsl1_marker_validator.sh");

pub(crate) fn verified_wsl_executable_path(path: String) -> Option<String> {
    path.starts_with('/').then_some(path)
}

pub(crate) fn probe_wsl_tool(
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
        "tool_identity=$(stat -Lc '%d|%i|%s|%y' -- \"$tool_path\" 2>/dev/null || true); ",
        "rtk_identity=$(stat -Lc '%d|%i|%s|%y' -- \"$rtk_path\" 2>/dev/null || true); ",
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
            let rtk = rtk.filter(|path| {
                adapter_version_is_compatible(
                    tool_version("rtk", path, Some((distro, user)))
                        .version
                        .as_deref(),
                )
            });
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

pub(crate) fn discover_tool(
    tool: &str,
    config: &Config,
    include_wsl: bool,
    inspect_versions: bool,
) -> ProviderCacheEntry {
    let executable = if tool == "go" { "go.exe" } else { tool };
    let windows_executable = first_windows_executable(executable);
    let native_rtk = configured_windows_executable(&config.native_rtk_path).filter(|path| {
        adapter_version_is_compatible(tool_version("rtk", path, None).version.as_deref())
    });
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

pub(crate) fn complete_wsl_discovery(
    tool: &str,
    config: &Config,
    mut discovered: ProviderCacheEntry,
    inspect_versions: bool,
) -> ProviderCacheEntry {
    let mut distros = installed_wsl_distributions();
    distros.sort_by_key(|(distro, version)| {
        if distro == &config.distro {
            0
        } else if *version == Some(2) {
            1
        } else {
            2
        }
    });
    for (distro, version) in &distros {
        if discovered.wsl.iter().any(|probe| probe.distro == *distro) {
            continue;
        }
        discovered.wsl.push(probe_wsl_tool(
            distro,
            *version,
            config.user.as_deref(),
            tool,
            config.extra_path.as_deref(),
            inspect_versions,
        ));
    }
    discovered.observed_unix_seconds = unix_seconds();
    discovered.inspection_level = if inspect_versions {
        InspectionLevel::Version
    } else {
        discovered.inspection_level
    };
    discovered.context_signature = discovery_context_signature(config, true);
    discovered.wsl_probe_complete = distros
        .iter()
        .all(|(distro, _)| discovered.wsl.iter().any(|probe| probe.distro == *distro));
    discovered
}

pub(crate) fn complete_cached_wsl_discovery(
    tool: &str,
    config: &Config,
    discovered: ProviderCacheEntry,
    inspect_versions: bool,
) -> (ProviderCacheEntry, &'static str) {
    if discovered.wsl_probe_complete {
        return (discovered, "hit");
    }
    let completed = complete_wsl_discovery(tool, config, discovered, inspect_versions);
    if let Err(error) = update_provider_cache(&completed) {
        trace(format!("completed provider cache was not saved: {error}"));
    }
    (completed, "miss")
}

pub(crate) fn cached_or_discovered_tool(
    tool: &str,
    config: &Config,
    refresh: bool,
    require_wsl: bool,
    validate_versions: bool,
) -> (ProviderCacheEntry, &'static str) {
    let now = unix_seconds();
    let context_signature = discovery_context_signature(config, require_wsl);
    let cache = load_provider_cache();
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
    if let Err(error) = update_provider_cache(&discovered) {
        trace(format!("provider cache was not saved: {error}"));
    }
    (discovered, "miss")
}
