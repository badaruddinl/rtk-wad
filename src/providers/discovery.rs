use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::UNIX_EPOCH;

use crate::process;
use crate::providers::model::*;

pub(crate) fn is_windows_launchable_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "exe" | "com" | "cmd" | "bat"
            )
        })
}

pub(crate) fn configured_windows_executable(path: &str) -> Option<String> {
    Path::new(path)
        .is_file()
        .then(|| path.to_owned())
        .or_else(|| first_windows_executable(path))
}

pub(crate) fn first_windows_executable(path: &str) -> Option<String> {
    let mut command = Command::new("where.exe");
    command.arg(path);
    process::run_probe(&mut command)
        .ok()
        .filter(|output| output.status.success() && !output.stdout_truncated)
        .and_then(|output| {
            let rendered = String::from_utf8_lossy(&output.stdout);
            let candidates = rendered
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            select_windows_executable(candidates)
        })
}

pub(crate) fn select_windows_executable(candidates: Vec<String>) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| is_windows_launchable_path(candidate))
        .cloned()
}

pub(crate) fn windows_binary_identity(path: &str) -> Option<BinaryIdentity> {
    let metadata = fs::metadata(path).ok()?;
    let modified_unix_seconds = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(BinaryIdentity {
        path: path.to_owned(),
        size_bytes: metadata.len(),
        modified_unix_seconds,
    })
}

pub(crate) fn version_probe_arguments(tool: &str) -> Option<&'static [&'static str]> {
    match tool.to_ascii_lowercase().as_str() {
        "go" => Some(&["version"]),
        "git" | "cargo" | "rustc" | "rustup" | "python" | "python3" | "node" | "npm" | "npx"
        | "pnpm" | "dart" | "flutter" | "rtk" | "rg" | "fd" | "jq" => Some(&["--version"]),
        _ => None,
    }
}

pub(crate) struct VersionProbe {
    pub(crate) version: Option<String>,
    pub(crate) status: ProbeStatus,
}

fn wsl_exec_prefix(distro: &str, user: Option<&str>) -> Vec<OsString> {
    let mut args = vec![OsString::from("-d"), OsString::from(distro)];
    if let Some(user) = user {
        args.extend([OsString::from("-u"), OsString::from(user)]);
    }
    args.push(OsString::from("--exec"));
    args
}

pub(crate) fn first_output_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

pub(crate) fn tool_version(
    tool: &str,
    executable: &str,
    wsl_context: Option<(&str, Option<&str>)>,
) -> VersionProbe {
    let Some(arguments) = version_probe_arguments(tool) else {
        return VersionProbe {
            version: None,
            status: ProbeStatus::NotSupported,
        };
    };
    let mut command = match wsl_context {
        Some((distro, user)) => {
            let mut command = Command::new("wsl.exe");
            command.args(wsl_exec_prefix(distro, user));
            command.arg(executable);
            command
        }
        None => Command::new(executable),
    };
    command.args(arguments);
    let output = match process::run_probe(&mut command) {
        Ok(output) => output,
        Err(error) => {
            return VersionProbe {
                version: None,
                status: if error.kind() == std::io::ErrorKind::TimedOut {
                    ProbeStatus::Timeout
                } else {
                    ProbeStatus::Failed
                },
            };
        }
    };
    if output.stdout_truncated || output.stderr_truncated {
        return VersionProbe {
            version: None,
            status: ProbeStatus::OutputLimit,
        };
    }
    if !output.status.success() {
        return VersionProbe {
            version: None,
            status: ProbeStatus::Failed,
        };
    }
    let version = first_output_line(&output.stdout).or_else(|| first_output_line(&output.stderr));
    VersionProbe {
        status: if version.is_some() {
            ProbeStatus::Success
        } else {
            ProbeStatus::Failed
        },
        version,
    }
}

pub(crate) fn version_capabilities(version: &Option<String>) -> Vec<String> {
    version
        .as_ref()
        .map(|_| vec!["version".to_owned()])
        .unwrap_or_default()
}

pub(crate) fn parse_wsl_binary_identity(
    path: Option<String>,
    identity: Option<String>,
) -> Option<BinaryIdentity> {
    let path = path?;
    let (size_bytes, modified_unix_seconds) = identity?
        .split_once(':')
        .and_then(|(size, modified)| Some((size.parse().ok()?, modified.parse().ok()?)))?;
    Some(BinaryIdentity {
        path,
        size_bytes,
        modified_unix_seconds,
    })
}

pub(crate) fn parse_wsl_distributions(output: &str) -> Vec<(String, Option<u8>)> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line
                .trim()
                .trim_start_matches('*')
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.len() < 3 || fields[0].eq_ignore_ascii_case("name") {
                return None;
            }
            let version = fields.last()?.parse::<u8>().ok();
            let name = fields[..fields.len() - 2].join(" ");
            (!name.is_empty()).then_some((name, version))
        })
        .collect()
}

pub(crate) fn is_eligible_wsl_distro(distro: &str) -> bool {
    !matches!(
        distro.to_ascii_lowercase().as_str(),
        "docker-desktop" | "docker-desktop-data"
    )
}

pub(crate) fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.chunks_exact(2).any(|pair| pair[1] == 0) {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub(crate) fn installed_wsl_distributions() -> Vec<(String, Option<u8>)> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--verbose"]);
    process::run_probe(&mut command)
        .ok()
        .filter(|output| output.status.success() && !output.stdout_truncated)
        .map(|output| {
            parse_wsl_distributions(&decode_wsl_output(&output.stdout))
                .into_iter()
                .filter(|(distro, _)| is_eligible_wsl_distro(distro))
                .collect()
        })
        .unwrap_or_default()
}
