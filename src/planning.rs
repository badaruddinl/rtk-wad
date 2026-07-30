//! Pure host/origin planning helpers.
//!
//! Discovery owns inventory collection; this module owns the safety boundary
//! between the invocation host and the selected provider.

use std::env;
use std::path::PathBuf;

use crate::config::{Config, InvocationOrigin};
use crate::dispatcher::EnvironmentPolicy;
use crate::paths::windows_path_to_wsl_path;
use crate::providers::model::{
    ProjectLocation, ProjectLocationKind, ProviderCandidate, ProviderHost,
};

pub(crate) fn classify_project_path(path: &str) -> ProjectLocation {
    let normalized = path.replace('/', "\\");
    let lowered = normalized.to_ascii_lowercase();
    for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
        if lowered.starts_with(prefix) {
            let original_remainder = &normalized[prefix.len()..];
            let mut parts = original_remainder.splitn(2, '\\');
            if let Some(distro) = parts.next().filter(|value| !value.is_empty()) {
                return ProjectLocation {
                    kind: ProjectLocationKind::Wsl,
                    path: format!("/{}", parts.next().unwrap_or_default().replace('\\', "/")),
                    distro: Some(distro.to_owned()),
                    windows_path: None,
                };
            }
        }
    }
    if windows_path_to_wsl_path(path).is_some() {
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: path.to_owned(),
            distro: None,
            windows_path: None,
        }
    } else {
        ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: path.to_owned(),
            distro: None,
            windows_path: None,
        }
    }
}

pub(crate) fn current_project_location(config: &Config) -> ProjectLocation {
    if let Some(cwd) = &config.cwd {
        return ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: cwd.clone(),
            distro: Some(config.distro.clone()),
            windows_path: config.bridge_windows_cwd.clone(),
        };
    }
    env::current_dir()
        .map(|path| classify_project_path(&path.to_string_lossy()))
        .unwrap_or(ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: String::new(),
            distro: None,
            windows_path: None,
        })
}

fn provider_matches_invocation_origin(config: &Config, candidate: &ProviderCandidate) -> bool {
    match (&config.invocation_origin, candidate.host) {
        (InvocationOrigin::Windows, ProviderHost::Windows) => true,
        (InvocationOrigin::Wsl { distro: origin }, ProviderHost::Wsl1 | ProviderHost::Wsl2) => {
            candidate.distro.as_deref() == Some(origin.as_str())
        }
        _ => false,
    }
}

pub(crate) fn provider_environment_policy(
    config: &Config,
    candidate: &ProviderCandidate,
) -> EnvironmentPolicy {
    if provider_matches_invocation_origin(config, candidate) {
        EnvironmentPolicy::Inherit
    } else {
        EnvironmentPolicy::Isolated
    }
}

pub(crate) fn windows_cwd_for_invocation(config: &Config) -> Result<PathBuf, String> {
    match &config.invocation_origin {
        InvocationOrigin::Windows => env::current_dir()
            .map_err(|error| format!("unable to determine the Windows current directory: {error}")),
        InvocationOrigin::Wsl { distro } => config
            .bridge_windows_cwd
            .as_deref()
            .filter(|path| {
                matches!(
                    classify_project_path(path),
                    ProjectLocation {
                        kind: ProjectLocationKind::Windows | ProjectLocationKind::Wsl,
                        ..
                    }
                )
            })
            .map(PathBuf::from)
            .ok_or_else(|| {
                format!(
                    "the WSL origin `{distro}` has no verified Windows/UNC mapping for its current directory"
                )
            }),
    }
}
