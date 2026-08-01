use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use crate::config::Config;
use crate::paths::windows_path_to_wsl_path;
use crate::process;
use crate::providers::discovery::{decode_wsl_output, installed_wsl_distributions};
use crate::wsl::arguments::WSL1_MARKER_VALIDATOR_SCRIPT;
use crate::wsl::cancellation::cancellation_nonce;
use crate::wsl::valid_installation_id;

pub(crate) fn dedicated_wsl1_installation_id_for(distro: &str) -> Option<String> {
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

pub(crate) fn require_wsl1_version(config: &Config) -> std::io::Result<()> {
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

pub(crate) struct LaunchPermitGuard {
    pub(crate) attestation_windows_path: PathBuf,
    pub(crate) attestation_wsl_path: String,
    pub(crate) permit_windows_path: PathBuf,
    pub(crate) permit_wsl_path: String,
    pub(crate) completion_windows_path: PathBuf,
    pub(crate) completion_wsl_path: String,
    pub(crate) expected_value: Option<String>,
}

impl LaunchPermitGuard {
    pub(crate) fn new(label: &str, expected_value: String) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, Some(expected_value))
    }

    pub(crate) fn new_unbound(label: &str) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, None)
    }

    pub(crate) fn new_with_expected_value(
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

    pub(crate) fn attested_value(&self) -> std::io::Result<Option<String>> {
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

    pub(crate) fn is_attested(&self) -> std::io::Result<bool> {
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

    pub(crate) fn authorize(&self) -> std::io::Result<()> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit permit value",
            )
        })?;
        self.authorize_value(expected_value)
    }

    pub(crate) fn authorize_value(&self, expected_value: &str) -> std::io::Result<()> {
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

    pub(crate) fn completion_status(&self) -> std::io::Result<Option<i32>> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit completion identity",
            )
        })?;
        self.completion_status_for(expected_value)
    }

    pub(crate) fn completion_status_for(
        &self,
        expected_value: &str,
    ) -> std::io::Result<Option<i32>> {
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

pub(crate) fn verify_proxy_completion_status(
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

pub(crate) fn verify_pre_authorization_proxy_status(
    proxy_status: ExitStatus,
) -> std::io::Result<ExitStatus> {
    if proxy_status.success() {
        Err(std::io::Error::other(
            "WSL1 proxy exited successfully before launch authorization; the target was not executed",
        ))
    } else {
        Ok(proxy_status)
    }
}
