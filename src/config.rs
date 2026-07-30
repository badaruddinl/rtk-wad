//! Configuration and route preferences shared by discovery and execution.
//!
//! Keeping environment parsing here makes the hot command path explicit and
//! prevents provider code from depending on ambient process variables.

use std::env;

pub(crate) const DEFAULT_DISTRO: &str = "Ubuntu";
pub(crate) const DEFAULT_WSL1_DISTRO: &str = "Ubuntu-RTK-WSL1";
const DEFAULT_LOCK_PATH: &str = "/tmp/xuva.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const DEFAULT_GIT_MODE: &str = "auto";
const DEFAULT_POLICY_OBJECTIVE: &str = "balanced";

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
    Xuva,
}

impl ExecutableProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Xuva => "xuva",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InvocationOrigin {
    Windows,
    Wsl { distro: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputAdapterPreference {
    Auto,
    Raw,
    Rtk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyObjective {
    Latency,
    Balanced,
    Tokens,
}

impl PolicyObjective {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Latency => "latency",
            Self::Balanced => "balanced",
            Self::Tokens => "tokens",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "latency" => Ok(Self::Latency),
            "balanced" => Ok(Self::Balanced),
            "tokens" => Ok(Self::Tokens),
            _ => Err("XUVA_POLICY_OBJECTIVE must be latency, balanced, or tokens".to_owned()),
        }
    }
}

impl OutputAdapterPreference {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Raw),
            "rtk" => Ok(Self::Rtk),
            _ => Err("XUVA_OUTPUT_ADAPTER must be auto, raw, or rtk".to_owned()),
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
    pub(crate) invocation_origin: InvocationOrigin,
    pub(crate) git_mode: GitMode,
    pub(crate) extra_path: Option<String>,
    pub(crate) route_preference: Route,
    pub(crate) environment: ExecutionEnvironment,
    pub(crate) native_rtk_path: String,
    pub(crate) output_adapter: OutputAdapterPreference,
    pub(crate) environment_allowlist: Vec<String>,
    pub(crate) metrics_enabled: bool,
    pub(crate) policy_objective: PolicyObjective,
}

impl Config {
    pub(crate) fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let profile = ExecutableProfile::Xuva;
        let backend = match lookup("XUVA_WSL_BACKEND")
            .unwrap_or_else(|| WslBackend::Auto.as_str().to_owned())
            .as_str()
        {
            "auto" => WslBackend::Auto,
            "wsl1" => WslBackend::Wsl1,
            "wsl2" => WslBackend::Wsl2,
            _ => return Err("XUVA_WSL_BACKEND must be auto, wsl1, or wsl2".to_owned()),
        };
        let default_distro = match backend {
            WslBackend::Wsl1 => DEFAULT_WSL1_DISTRO,
            WslBackend::Auto | WslBackend::Wsl2 => DEFAULT_DISTRO,
        };
        let distro = required_setting(&lookup, "XUVA_WSL_DISTRO", default_distro)?;
        let user = optional_setting(&lookup, "XUVA_WSL_USER")?;
        if let Some(user) = &user {
            validate_wsl_user(user)?;
        }
        let rtk_path = optional_absolute_path(&lookup, "XUVA_WSL_RTK_PATH")?;
        let lock_path = required_absolute_path(&lookup, "XUVA_WSL_LOCK_PATH", DEFAULT_LOCK_PATH)?;
        let lock_wait = required_setting(
            &lookup,
            "XUVA_WSL_LOCK_WAIT_SECONDS",
            DEFAULT_LOCK_WAIT_SECONDS,
        )?;
        let cwd = optional_absolute_path(&lookup, "XUVA_WSL_CWD")?;
        let git_mode =
            match required_setting(&lookup, "XUVA_WSL_GIT_MODE", DEFAULT_GIT_MODE)?.as_str() {
                "auto" => GitMode::Auto,
                "wsl" => GitMode::Wsl,
                "native" => GitMode::Native,
                _ => return Err("XUVA_WSL_GIT_MODE must be auto, wsl, or native".to_owned()),
            };
        let extra_path = optional_linux_path_list(&lookup, "XUVA_WSL_EXTRA_PATH")?;
        let route_preference = match lookup("XUVA_ROUTE") {
            Some(value) if value.trim().is_empty() => {
                return Err("XUVA_ROUTE must not be empty when set".to_owned());
            }
            Some(value) => Route::parse(&value).map_err(|error| format!("XUVA_ROUTE {error}"))?,
            None => Route::Auto,
        };
        let environment = match lookup("XUVA_ENVIRONMENT") {
            Some(value) if value.trim().is_empty() => {
                return Err("XUVA_ENVIRONMENT must not be empty when set".to_owned());
            }
            Some(value) => ExecutionEnvironment::parse(&value)
                .map_err(|error| format!("XUVA_ENVIRONMENT {error}"))?,
            None => ExecutionEnvironment::Adaptive,
        };
        let native_rtk_path = lookup("XUVA_NATIVE_RTK_PATH")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "rtk.exe".to_owned());
        let output_adapter = match lookup("XUVA_OUTPUT_ADAPTER") {
            Some(value) if value.trim().is_empty() => {
                return Err("XUVA_OUTPUT_ADAPTER must not be empty when set".to_owned());
            }
            Some(value) => OutputAdapterPreference::parse(&value)?,
            None => OutputAdapterPreference::Auto,
        };
        let environment_allowlist =
            parse_environment_allowlist(lookup("XUVA_ENV_ALLOWLIST").as_deref())?;
        let metrics_enabled = match lookup("XUVA_METRICS").as_deref() {
            None | Some("local") | Some("on") => true,
            Some("off") => false,
            Some(_) => return Err("XUVA_METRICS must be local, on, or off".to_owned()),
        };
        let policy_objective = PolicyObjective::parse(
            &lookup("XUVA_POLICY_OBJECTIVE").unwrap_or_else(|| DEFAULT_POLICY_OBJECTIVE.to_owned()),
        )?;

        if lock_wait
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
        {
            return Err("XUVA_WSL_LOCK_WAIT_SECONDS must be a positive integer".to_owned());
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
            invocation_origin: InvocationOrigin::Windows,
            git_mode,
            extra_path,
            route_preference,
            environment,
            native_rtk_path,
            output_adapter,
            environment_allowlist,
            metrics_enabled,
            policy_objective,
        })
    }
}

fn is_posix_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
            b'0'..=b'9' => index > 0,
            _ => false,
        })
}

pub(crate) fn is_sensitive_environment_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let sensitive_component = upper.split('_').any(|component| {
        matches!(
            component,
            "TOKEN"
                | "SECRET"
                | "PASSWORD"
                | "PASSWD"
                | "CREDENTIAL"
                | "CREDENTIALS"
                | "COOKIE"
                | "AUTH"
        )
    });
    sensitive_component
        || ["TOKEN", "PRIVATE_KEY", "ACCESS_KEY"]
            .iter()
            .any(|marker| upper.contains(marker))
}

fn parse_environment_allowlist(value: Option<&str>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for name in value.split(',').map(str::trim) {
        if !is_posix_environment_name(name) {
            return Err(
                "XUVA_ENV_ALLOWLIST must be a comma-separated list of POSIX environment names"
                    .to_owned(),
            );
        }
        if is_sensitive_environment_name(name) {
            return Err(format!(
                "XUVA_ENV_ALLOWLIST refuses credential-like variable `{name}`"
            ));
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
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

pub(crate) fn validate_wsl_user(value: &str) -> Result<(), String> {
    let mut bytes = value.bytes();
    let first = bytes
        .next()
        .ok_or_else(|| "WSL user must not be empty".to_owned())?;
    if !matches!(first, b'a'..=b'z' | b'_')
        || value.len() > 32
        || !bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        return Err(
            "WSL user must be a lowercase POSIX account name of at most 32 characters".to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn validate_linux_path_list(value: &str, name: &str) -> Result<(), String> {
    if value.split(':').any(|entry| {
        !entry.starts_with('/')
            || entry.ends_with('/')
            || entry[1..]
                .split('/')
                .any(|component| matches!(component, "" | "." | ".."))
    }) {
        return Err(format!(
            "{name} must be a colon-separated list of normalized absolute Linux paths"
        ));
    }
    Ok(())
}

fn optional_linux_path_list(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = optional_setting(lookup, name)?;
    if let Some(value) = &value {
        validate_linux_path_list(value, name)?;
    }
    Ok(value)
}
