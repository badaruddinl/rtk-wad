use std::env;
use std::ffi::OsString;

use crate::config::{Config, Route};
use crate::paths::windows_path_to_wsl_path;
use crate::wsl::test_hooks::{
    test_completion_status_override, test_ready_wsl_path, test_wsl1_attestation_delay_seconds,
    test_wsl1_marker_path, test_wsl2_launch_delay_seconds,
};

pub(crate) const LAUNCH_SCRIPT: &str = include_str!("../scripts/launch.sh");
pub(crate) const WSL1_MARKER_VALIDATOR_SCRIPT: &str =
    include_str!("../scripts/wsl1_marker_validator.sh");
pub(crate) const WSL1_LAUNCH_SCRIPT: &str = include_str!("../scripts/wsl1_launch.sh");
pub(crate) const PLAN_LAUNCH_SCRIPT: &str = include_str!("../scripts/plan_launch.sh");

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

#[cfg(test)]
pub(crate) fn rtk_arguments(
    arguments: Vec<OsString>,
    config: &Config,
    cancel_nonce: &str,
) -> Vec<OsString> {
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

pub(crate) fn rtk_arguments_with_metrics(
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
pub(crate) fn wsl1_rtk_arguments(arguments: Vec<OsString>, config: &Config) -> Vec<OsString> {
    wsl1_rtk_arguments_with_metrics(
        arguments,
        config,
        None,
        "/tmp/xuva-test.attestation",
        "/tmp/xuva-test.permit",
        "/tmp/xuva-test.completion",
    )
}

pub(crate) fn wsl1_rtk_arguments_with_metrics(
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

pub(crate) fn wsl_environment_assignments(
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
pub(crate) struct WslLaunchMetadata<'a> {
    pub(crate) cancel_nonce: Option<&'a str>,
    pub(crate) metrics_db_path: Option<&'a str>,
    pub(crate) attestation_path: Option<&'a str>,
    pub(crate) permit_path: Option<&'a str>,
    pub(crate) completion_path: Option<&'a str>,
}

pub(crate) fn plan_wsl_arguments_with_metrics(
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
