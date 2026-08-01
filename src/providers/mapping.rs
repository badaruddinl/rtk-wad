use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::config::{Config, InvocationOrigin};
use crate::paths::{translate_arguments_with, windows_path_to_wsl_path, wsl_path_to_windows_path};
use crate::planning::classify_project_path;
use crate::process;
use crate::wsl::exec_prefix as wsl_exec_prefix;

use super::discovery::first_output_line;
use super::model::{ProjectLocation, ProjectLocationKind, WslToolProbe};

pub(crate) fn wsl_mapping_arguments_with_user(
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

pub(crate) fn mapped_windows_project_path(
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

pub(crate) fn windows_mapping_arguments_with_user(
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

pub(crate) fn mapped_wsl_project_path(
    distro: &str,
    user: Option<&str>,
    linux_path: &str,
) -> Option<String> {
    let mut command = Command::new("wsl.exe");
    command.args(windows_mapping_arguments_with_user(
        distro, user, linux_path,
    ));
    process::run_probe(&mut command)
        .ok()
        .filter(|output| output.status.success() && !output.stdout_truncated)
        .and_then(|output| first_output_line(&output.stdout))
}

pub(crate) fn translate_arguments_to_windows(
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

pub(crate) fn translate_arguments_to_wsl(
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

pub(crate) fn wsl_directory_exists(distro: &str, user: Option<&str>, path: &str) -> bool {
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

pub(crate) fn is_windows_project_path_for_distro(
    path: &str,
    expected_distro: Option<&str>,
) -> bool {
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

pub(crate) fn wsl_project_path_with(
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

pub(crate) fn wsl_project_path(
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

pub(crate) fn windows_project_path_with(
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

pub(crate) fn windows_project_path(
    project: &ProjectLocation,
    user: Option<&str>,
) -> Option<String> {
    windows_project_path_with(
        project,
        |distro, path| mapped_wsl_project_path(distro, user, path),
        |path| Path::new(path).is_dir(),
    )
}
