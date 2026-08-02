use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;

use crate::PRODUCT_COMMAND;
use crate::cli_exit::CliExit as ExitCode;
use crate::config::Config;

use super::discovery::{installed_wsl_distributions, is_windows_launchable_path};
use super::model::{BinaryIdentity, ProviderResolution};
use super::resolution::{resolve_tool_provider, resolve_tool_provider_with_inspection};

const DOCTOR_ARGUMENT: &str = "doctor";
const RESOLVE_ARGUMENT: &str = "resolve";

pub(crate) fn print_provider_resolution(
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

pub(crate) fn binary_identity_display(identity: Option<&BinaryIdentity>) -> String {
    identity.map_or_else(
        || "missing".to_owned(),
        |identity| {
            format!(
                "{}:{}:{}:{}",
                identity.path, identity.file_key, identity.size_bytes, identity.modified_stamp
            )
        },
    )
}

pub(crate) fn is_safe_provider_tool_name(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 128
        && tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(crate) fn windows_path_tool_names() -> Vec<String> {
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

pub(crate) fn provider_command(arguments: &[OsString], config: &Config, doctor: bool) -> ExitCode {
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

pub(crate) fn provider_scan_command(arguments: &[OsString], config: &Config) -> ExitCode {
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
