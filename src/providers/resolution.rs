use crate::PRODUCT_COMMAND;
use crate::adapters::rtk::{CommandSurface, command_surface};
use crate::config::Config;
use crate::planning::current_project_location;

use super::cache::PROVIDER_CACHE_SCHEMA_VERSION;
use super::mapping::{windows_project_path, wsl_project_path};
use super::model::{
    AdapterKind, ProjectLocation, ProjectLocationKind, ProviderCacheEntry, ProviderCandidate,
    ProviderHost, ProviderResolution, WindowsToolProbe,
};
use super::probe::cached_or_discovered_tool;

pub(crate) fn resolve_tool_provider(
    tool: &str,
    config: &Config,
    refresh: bool,
) -> ProviderResolution {
    resolve_tool_provider_with_inspection(tool, config, refresh, true)
}

pub(crate) fn resolve_tool_provider_with_inspection(
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

pub(crate) fn resolve_tool_provider_from_discovery_with_user(
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

pub(crate) fn windows_provider_has_compatible_semantics(tool: &str, adapter: AdapterKind) -> bool {
    match adapter {
        AdapterKind::Raw => !matches!(
            tool,
            "awk" | "cat" | "find" | "grep" | "head" | "ls" | "sed" | "tail" | "tree" | "wc"
        ),
        AdapterKind::Rtk => true,
    }
}

pub(crate) fn windows_probe_has_compatible_provider(
    tool: &str,
    windows: &WindowsToolProbe,
) -> bool {
    (windows.executable.is_some()
        && windows_provider_has_compatible_semantics(tool, AdapterKind::Raw))
        || (windows.native_rtk.is_some()
            && windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk))
}

pub(crate) fn requires_raw_posix_provider(tool: &str) -> bool {
    matches!(
        tool,
        "awk" | "cat" | "find" | "head" | "ls" | "sed" | "tail" | "tree" | "wc"
    )
}
