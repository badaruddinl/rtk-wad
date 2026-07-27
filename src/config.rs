//! Configuration and route preferences shared by discovery and execution.
//!
//! Keeping environment parsing here makes the hot command path explicit and
//! prevents provider code from depending on ambient process variables.

use std::env;

pub(crate) const DEFAULT_DISTRO: &str = "Ubuntu";
pub(crate) const DEFAULT_WSL1_DISTRO: &str = "Ubuntu-RTK-WSL1";
const DEFAULT_LOCK_PATH: &str = "/tmp/rtk-wad.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const DEFAULT_GIT_MODE: &str = "auto";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitMode {
    Auto,
    Wsl,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WslBackend {
    Auto,
    Wsl1,
    Wsl2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutableProfile {
    Wad,
}

impl ExecutableProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Wad => "xuva",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Auto,
    Raw,
    NativeRtk,
    Wsl1,
    Wsl2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionEnvironment {
    Adaptive,
    WindowsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputAdapterPreference {
    Auto,
    Raw,
    Rtk,
}

impl OutputAdapterPreference {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Raw),
            "rtk" => Ok(Self::Rtk),
            _ => Err("RTK_WAD_OUTPUT_ADAPTER must be auto, raw, or rtk".to_owned()),
        }
    }
}

impl ExecutionEnvironment {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::WindowsOnly => "windows-only",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "adaptive" => Ok(Self::Adaptive),
            "windows-only" => Ok(Self::WindowsOnly),
            _ => Err("environment must be adaptive or windows-only".to_owned()),
        }
    }
}

impl Route {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Raw => "raw",
            Self::NativeRtk => "native-rtk",
            Self::Wsl1 => "wsl1",
            Self::Wsl2 => "wsl2",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Raw),
            "native-rtk" => Ok(Self::NativeRtk),
            "wsl1" => Ok(Self::Wsl1),
            "wsl2" => Ok(Self::Wsl2),
            _ => Err("route must be auto, raw, native-rtk, wsl1, or wsl2".to_owned()),
        }
    }
}

impl WslBackend {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wsl1 => "wsl1",
            Self::Wsl2 => "wsl2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) profile: ExecutableProfile,
    pub(crate) backend: WslBackend,
    pub(crate) distro: String,
    pub(crate) user: Option<String>,
    pub(crate) rtk_path: Option<String>,
    pub(crate) lock_path: String,
    pub(crate) lock_wait: String,
    pub(crate) cwd: Option<String>,
    pub(crate) bridge_windows_cwd: Option<String>,
    pub(crate) git_mode: GitMode,
    pub(crate) extra_path: Option<String>,
    pub(crate) wad_route: Route,
    pub(crate) environment: ExecutionEnvironment,
    pub(crate) native_rtk_path: String,
    pub(crate) output_adapter: OutputAdapterPreference,
}

impl Config {
    pub(crate) fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let profile = ExecutableProfile::Wad;
        let backend = match lookup("RTK_WSL_BACKEND")
            .unwrap_or_else(|| WslBackend::Auto.as_str().to_owned())
            .as_str()
        {
            "auto" => WslBackend::Auto,
            "wsl1" => WslBackend::Wsl1,
            "wsl2" => WslBackend::Wsl2,
            _ => return Err("RTK_WSL_BACKEND must be auto, wsl1, or wsl2".to_owned()),
        };
        let default_distro = match backend {
            WslBackend::Wsl1 => DEFAULT_WSL1_DISTRO,
            WslBackend::Auto | WslBackend::Wsl2 => DEFAULT_DISTRO,
        };
        let distro = required_setting(&lookup, "RTK_WSL_DISTRO", default_distro)?;
        let user = optional_setting(&lookup, "RTK_WSL_USER")?;
        let rtk_path = optional_absolute_path(&lookup, "RTK_WSL_RTK_PATH")?;
        let lock_path = required_absolute_path(&lookup, "RTK_WSL_LOCK_PATH", DEFAULT_LOCK_PATH)?;
        let lock_wait = required_setting(
            &lookup,
            "RTK_WSL_LOCK_WAIT_SECONDS",
            DEFAULT_LOCK_WAIT_SECONDS,
        )?;
        let cwd = optional_absolute_path(&lookup, "RTK_WSL_CWD")?;
        let git_mode =
            match required_setting(&lookup, "RTK_WSL_GIT_MODE", DEFAULT_GIT_MODE)?.as_str() {
                "auto" => GitMode::Auto,
                "wsl" => GitMode::Wsl,
                "native" => GitMode::Native,
                _ => return Err("RTK_WSL_GIT_MODE must be auto, wsl, or native".to_owned()),
            };
        let extra_path = optional_linux_path_list(&lookup, "RTK_WSL_EXTRA_PATH")?;
        let wad_route = match lookup("RTK_WAD_ROUTE") {
            Some(value) if value.trim().is_empty() => {
                return Err("RTK_WAD_ROUTE must not be empty when set".to_owned());
            }
            Some(value) => {
                Route::parse(&value).map_err(|error| format!("RTK_WAD_ROUTE {error}"))?
            }
            None => Route::Auto,
        };
        let environment = match lookup("RTK_WAD_ENVIRONMENT") {
            Some(value) if value.trim().is_empty() => {
                return Err("RTK_WAD_ENVIRONMENT must not be empty when set".to_owned());
            }
            Some(value) => ExecutionEnvironment::parse(&value)
                .map_err(|error| format!("RTK_WAD_ENVIRONMENT {error}"))?,
            None => ExecutionEnvironment::Adaptive,
        };
        let native_rtk_path = lookup("RTK_WAD_NATIVE_RTK_PATH")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "rtk.exe".to_owned());
        let output_adapter = match lookup("RTK_WAD_OUTPUT_ADAPTER") {
            Some(value) if value.trim().is_empty() => {
                return Err("RTK_WAD_OUTPUT_ADAPTER must not be empty when set".to_owned());
            }
            Some(value) => OutputAdapterPreference::parse(&value)?,
            None => OutputAdapterPreference::Auto,
        };

        if lock_wait
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
        {
            return Err("RTK_WSL_LOCK_WAIT_SECONDS must be a positive integer".to_owned());
        }

        Ok(Self {
            profile,
            backend,
            distro,
            user,
            rtk_path,
            lock_path,
            lock_wait,
            cwd,
            bridge_windows_cwd: None,
            git_mode,
            extra_path,
            wad_route,
            environment,
            native_rtk_path,
            output_adapter,
        })
    }
}

fn required_setting(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: &str,
) -> Result<String, String> {
    match lookup(name) {
        Some(value) if value.trim().is_empty() => Err(format!("{name} must not be empty")),
        Some(value) => Ok(value),
        None => Ok(default.to_owned()),
    }
}

fn optional_setting(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<String>, String> {
    match lookup(name) {
        Some(value) if value.trim().is_empty() => Err(format!("{name} must not be empty when set")),
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn optional_absolute_path(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = optional_setting(lookup, name)?;
    if value.as_deref().is_some_and(|path| !path.starts_with('/')) {
        return Err(format!("{name} must be an absolute Linux path"));
    }
    Ok(value)
}

fn required_absolute_path(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: &str,
) -> Result<String, String> {
    let value = required_setting(lookup, name, default)?;
    if !value.starts_with('/') {
        return Err(format!("{name} must be an absolute Linux path"));
    }
    Ok(value)
}

fn optional_linux_path_list(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = optional_setting(lookup, name)?;
    if let Some(value) = &value
        && value
            .split(':')
            .any(|entry| entry.is_empty() || !entry.starts_with('/'))
    {
        return Err(format!(
            "{name} must be a colon-separated list of absolute Linux paths"
        ));
    }
    Ok(value)
}
