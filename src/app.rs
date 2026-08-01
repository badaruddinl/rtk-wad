#[cfg(test)]
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use crate::diagnostics::trace;
#[cfg(test)]
use crate::execution::planner::execution_plan_for_provider_candidate;
#[cfg(test)]
use crate::execution::planner::provider_adapter;
use crate::execution::planner::{
    configured_wsl_backend, execution_plan_for_explicit_provider_candidate,
    first_compatible_provider_plan, is_shell_operator_command, provider_execution_config,
    static_windows_execution_plan,
};
#[cfg(test)]
use crate::setup::has_complete_go_provider;
use crate::setup::setup_command;
use crate::wsl::valid_installation_id;
use crate::{
    PRODUCT_COMMAND, adapters, agent, bridge, cli, cli_exit, config, dispatcher, lifecycle,
    metrics, paths, process, providers, routing, self_update,
};

#[cfg(test)]
use crate::planning::classify_project_path;
#[cfg(test)]
use crate::planning::provider_environment_policy;
#[cfg(test)]
use crate::planning::windows_cwd_for_invocation;
#[cfg(test)]
use adapters::rtk::adapter_contract_id;
#[cfg(test)]
use adapters::rtk::command_surface_report;
#[cfg(test)]
use adapters::rtk::{CommandSurface, command_surface};
#[cfg(test)]
use adapters::windows::apply_command_spec;
#[cfg(test)]
use bridge::decode_wsl_bridge_fields;
use bridge::wsl_bridge_request;
use cli_exit::CliExit as ExitCode;
#[cfg(test)]
use config::ExecutableProfile;
use config::{Config, ExecutionEnvironment, GitMode, InvocationOrigin, Route};
#[cfg(test)]
use config::{DEFAULT_DISTRO, DEFAULT_WSL1_DISTRO, WslBackend};
#[cfg(test)]
use config::{OutputAdapterPreference, PolicyObjective};
use metrics::{TokenTotals, XuvaMetrics};
use paths::windows_path_to_wsl_path;
#[cfg(test)]
use providers::cache::{
    PROVIDER_CACHE_SCHEMA_VERSION, PROVIDER_CACHE_TTL_SECONDS, cache_entry_is_fresh,
    discovery_context_signature, unix_seconds,
};
use providers::commands::{is_safe_provider_tool_name, provider_command, provider_scan_command};
use providers::discovery::{decode_wsl_output, installed_wsl_distributions};
#[cfg(test)]
use providers::discovery::{
    is_eligible_wsl_distro, is_windows_launchable_path, parse_wsl_binary_identity,
    parse_wsl_distributions, select_windows_executable, version_probe_arguments,
};
use providers::dispatch::{
    ProviderDispatchDecision, explicit_executable_plan, provider_dispatch_decision,
};
#[cfg(test)]
use providers::dispatch::{
    is_dispatchable_provider_tool, provider_dispatch_decision_from_resolution,
    windows_tool_is_usable,
};
use providers::mapping::mapped_windows_project_path;
#[cfg(test)]
use providers::mapping::{
    windows_mapping_arguments_with_user, windows_project_path_with,
    wsl_mapping_arguments_with_user, wsl_project_path_with,
};
#[cfg(test)]
use providers::model::{
    AdapterKind, InspectionLevel, ProbeStatus, ProjectLocation, ProjectLocationKind,
    ProviderCacheEntry, ProviderCandidate, ProviderHost, ProviderResolution, WindowsToolProbe,
    WslToolProbe,
};
#[cfg(test)]
use providers::probe::verified_wsl_executable_path;
use providers::resolution::resolve_tool_provider;
#[cfg(test)]
use providers::resolution::{
    requires_raw_posix_provider, resolve_tool_provider_from_discovery_with_user,
    windows_provider_has_compatible_semantics,
};

const ADAPTER_INFO_ARGUMENT: &str = "--adapter-info";
#[cfg(test)]
const VERSION_ARGUMENT: &str = "--version";
const POLICY_ARGUMENT: &str = "policy";
const CALIBRATION_ARGUMENT: &str = "calibration";
const RESOLVE_ARGUMENT: &str = "resolve";
const DOCTOR_ARGUMENT: &str = "doctor";
const WHICH_ARGUMENT: &str = "which";
const SCAN_ARGUMENT: &str = "scan";
const PROVIDER_ARGUMENT: &str = "provider";
const SURFACE_ARGUMENT: &str = "surface";
const SETUP_ARGUMENT: &str = "setup";
const AGENT_ARGUMENT: &str = "agent";
const HELP_ARGUMENT: &str = "--help";
const SELF_UPDATE_ARGUMENT: &str = "self-update";
const CANCEL_SCRIPT: &str = include_str!("scripts/cancel.sh");
const CANCEL_PROBE_SCRIPT: &str = include_str!("scripts/cancel_probe.sh");
const LAUNCH_SCRIPT: &str = include_str!("scripts/launch.sh");
const WSL1_MARKER_VALIDATOR_SCRIPT: &str = include_str!("scripts/wsl1_marker_validator.sh");
const WSL1_LAUNCH_SCRIPT: &str = include_str!("scripts/wsl1_launch.sh");
const PLAN_LAUNCH_SCRIPT: &str = include_str!("scripts/plan_launch.sh");
#[cfg(test)]
fn distro_version_from_list(output: &str, distro: &str) -> Option<u8> {
    output.lines().find_map(|line| {
        let trimmed = line.trim().trim_start_matches('*').trim_start();
        let remainder = trimmed.strip_prefix(distro)?;
        if remainder.is_empty() || !remainder.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }
        remainder.split_whitespace().last()?.parse::<u8>().ok()
    })
}

#[cfg(test)]
use routing::RoutePolicyFile;
use routing::calibration::{
    load as load_calibration, print as print_calibration, record as record_calibration,
};
#[cfg(test)]
use routing::decision::{
    auto_route, auto_route_with_context, is_adapter_only_rtk_command, is_rtk_meta_command,
};
use routing::decision::{
    auto_route_for_environment, command_family, is_verified_read_only_git, is_wsl_path,
    route_policy_key,
};
use routing::policy::{import as import_route_policy, load as load_route_policy};
#[cfg(test)]
use routing::{ROUTE_POLICY_SCHEMA_VERSION, RoutePolicyEvidence, calibration_signature};
use routing::{adaptive_context_signature, calibration_plan, policy_context_report};

use cli::{
    is_verbose_version_command, is_version_command, parse_options, print_command_surface,
    print_verbose_version,
};
#[cfg(test)]
use self_update::{latest_release_from_ls_remote, parsed_stable_version, stable_release_is_newer};

fn git_uses_wsl_directory(arguments: &[OsString]) -> bool {
    arguments.windows(2).any(|pair| {
        (pair[0] == "-C" || pair[0] == "--git-dir" || pair[0] == "--work-tree")
            && is_wsl_path(&pair[1])
    })
}

fn should_use_native_git(
    arguments: &[OsString],
    config: &Config,
    current_directory: Option<&str>,
) -> bool {
    if arguments.first().is_none_or(|argument| argument != "git")
        || git_uses_wsl_directory(arguments)
    {
        return false;
    }
    match config.git_mode {
        GitMode::Native => true,
        GitMode::Wsl => false,
        GitMode::Auto => {
            config.cwd.is_none()
                && current_directory
                    .and_then(windows_path_to_wsl_path)
                    .is_some()
        }
    }
}

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

fn test_ready_wsl_path() -> Option<String> {
    env::var("XUVA_WSL_TEST_READY_FILE")
        .ok()
        .and_then(|path| windows_path_to_wsl_path(&path))
}

fn test_wsl1_attestation_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL1_ATTESTATION_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 5)
        .unwrap_or(0)
}

fn test_wsl1_marker_path() -> String {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return String::new();
    }
    env::var("XUVA_TEST_WSL1_MARKER_PATH")
        .ok()
        .filter(|path| {
            path.starts_with('/')
                && !path.contains(['\0', '\r', '\n'])
                && path.split('/').all(|part| !matches!(part, "." | ".."))
        })
        .unwrap_or_default()
}

fn test_wsl2_launch_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL2_LAUNCH_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 5)
        .unwrap_or(0)
}

fn test_completion_status_override() -> Option<u8> {
    (env::var("XUVA_TEST_MODE").as_deref() == Ok("1"))
        .then(|| {
            env::var("XUVA_TEST_COMPLETION_STATUS_OVERRIDE")
                .ok()
                .and_then(|value| value.parse::<u8>().ok())
        })
        .flatten()
}

fn test_kill_wsl1_proxy_after_permit() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL1_PROXY_AFTER_PERMIT").as_deref() == Ok("1")
}

fn test_ready_file_exists() -> bool {
    env::var_os("XUVA_WSL_TEST_READY_FILE").is_some_and(|path| PathBuf::from(path).is_file())
}

fn test_kill_wsl2_proxy_during_cancellation() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL2_PROXY_DURING_CANCEL").as_deref() == Ok("1")
}

fn test_defer_wsl2_proxy_reap_until_cleanup() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_DEFER_WSL2_PROXY_REAP_UNTIL_CLEANUP").as_deref() == Ok("1")
}

#[cfg(test)]
fn rtk_arguments(arguments: Vec<OsString>, config: &Config, cancel_nonce: &str) -> Vec<OsString> {
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

fn rtk_arguments_with_metrics(
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
fn wsl1_rtk_arguments(arguments: Vec<OsString>, config: &Config) -> Vec<OsString> {
    wsl1_rtk_arguments_with_metrics(
        arguments,
        config,
        None,
        "/tmp/xuva-test.attestation",
        "/tmp/xuva-test.permit",
        "/tmp/xuva-test.completion",
    )
}

fn wsl1_rtk_arguments_with_metrics(
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

fn wsl_environment_assignments(
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
struct WslLaunchMetadata<'a> {
    cancel_nonce: Option<&'a str>,
    metrics_db_path: Option<&'a str>,
    attestation_path: Option<&'a str>,
    permit_path: Option<&'a str>,
    completion_path: Option<&'a str>,
}

fn plan_wsl_arguments_with_metrics(
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

#[cfg(target_os = "windows")]
fn cancellation_nonce() -> std::io::Result<String> {
    use std::ffi::c_void;

    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32)
        -> i32;
    }

    let mut bytes = [0_u8; 16];
    // SAFETY: the null algorithm handle requests the system-preferred RNG and
    // `bytes` is a valid writable buffer for the exact supplied length.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(std::io::Error::other(format!(
            "Windows secure random generation failed with NTSTATUS 0x{:08x}",
            status as u32
        )));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(target_os = "windows"))]
fn cancellation_nonce() -> std::io::Result<String> {
    use std::io::Read;

    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cancel_arguments(config: &Config, token: &str, signal: &str) -> Vec<OsString> {
    let mut command = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        command.extend([OsString::from("-u"), OsString::from(user)]);
    }
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(CANCEL_SCRIPT),
        OsString::from("xuva-cancel"),
        OsString::from(token),
        OsString::from(signal),
    ]);
    command
}

#[cfg(target_os = "windows")]
mod console {
    use std::sync::atomic::{AtomicBool, Ordering};

    static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handler(event: u32) -> i32 {
        if event == 0 || event == 1 {
            CANCEL_REQUESTED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }

    pub fn install() -> bool {
        unsafe { SetConsoleCtrlHandler(Some(handler), 1) != 0 }
    }

    pub fn uninstall() {
        unsafe { SetConsoleCtrlHandler(Some(handler), 0) };
    }

    pub fn requested() -> bool {
        CANCEL_REQUESTED.load(Ordering::SeqCst)
    }
}

#[cfg(not(target_os = "windows"))]
mod console {
    pub fn install() -> bool {
        true
    }
    pub fn uninstall() {}
    pub fn requested() -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
mod windows_lock {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x0000_0080;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const MUTEX_NAME: &str = r"Local\xuva-wsl1-global-lock";

    unsafe extern "system" {
        fn CreateMutexW(
            mutex_attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn ReleaseMutex(handle: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub struct Guard {
        handle: *mut c_void,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            super::trace(format!(
                "releasing WSL1 mutex in process {}",
                std::process::id()
            ));
            unsafe {
                ReleaseMutex(self.handle);
                CloseHandle(self.handle);
            }
        }
    }

    pub fn acquire(wait_seconds: &str) -> Result<Guard, String> {
        let name = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err("unable to create the WSL1 Windows mutex".to_owned());
        }
        let seconds = wait_seconds
            .parse::<u64>()
            .map_err(|_| "invalid WSL1 Windows mutex timeout".to_owned())?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if super::console::requested() {
                unsafe { CloseHandle(handle) };
                return Err("cancelled while waiting for the WSL1 Windows mutex".to_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                unsafe { CloseHandle(handle) };
                return Err(format!(
                    "timed out waiting for the WSL1 Windows mutex after {wait_seconds} seconds"
                ));
            }
            let milliseconds = u32::try_from(remaining.as_millis().min(50)).unwrap_or(50);
            let result = unsafe { WaitForSingleObject(handle, milliseconds) };
            match result {
                WAIT_OBJECT_0 | WAIT_ABANDONED => {
                    super::trace(format!(
                        "acquired WSL1 mutex in process {}",
                        std::process::id()
                    ));
                    return Ok(Guard { handle });
                }
                WAIT_TIMEOUT => {}
                _ => {
                    unsafe { CloseHandle(handle) };
                    return Err("unable to wait for the WSL1 Windows mutex".to_owned());
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod windows_lock {
    pub struct Guard;

    pub fn acquire(_wait_seconds: &str) -> Result<Guard, String> {
        Ok(Guard)
    }
}

fn send_linux_signal(config: &Config, token: &str, signal: &str) -> std::io::Result<bool> {
    let mut command = Command::new("wsl.exe");
    command.args(cancel_arguments(config, token, signal));
    let output = process::run_bounded(
        &mut command,
        None,
        Duration::from_secs(1),
        process::PROBE_OUTPUT_LIMIT,
    )?;
    Ok(output.status.success())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxProcessGroupState {
    Alive,
    Gone,
    TokenUnavailable,
}

fn linux_process_group_state(
    config: &Config,
    token: &str,
) -> std::io::Result<LinuxProcessGroupState> {
    let mut arguments = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        arguments.extend([OsString::from("-u"), OsString::from(user)]);
    }
    arguments.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(CANCEL_PROBE_SCRIPT),
        OsString::from("xuva-cancel-probe"),
        OsString::from(token),
    ]);
    let mut command = Command::new("wsl.exe");
    command.args(arguments);
    let output = process::run_bounded(
        &mut command,
        None,
        Duration::from_secs(1),
        process::PROBE_OUTPUT_LIMIT,
    )?;
    match output.status.code() {
        Some(0) => Ok(LinuxProcessGroupState::Alive),
        Some(1) => Ok(LinuxProcessGroupState::Gone),
        Some(4) => Ok(LinuxProcessGroupState::TokenUnavailable),
        code => Err(std::io::Error::other(format!(
            "unable to verify the Linux process group (probe exit {code:?}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

fn dedicated_wsl1_installation_id_for(distro: &str) -> Option<String> {
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

fn require_wsl1_version(config: &Config) -> std::io::Result<()> {
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

struct LaunchPermitGuard {
    attestation_windows_path: PathBuf,
    attestation_wsl_path: String,
    permit_windows_path: PathBuf,
    permit_wsl_path: String,
    completion_windows_path: PathBuf,
    completion_wsl_path: String,
    expected_value: Option<String>,
}

impl LaunchPermitGuard {
    fn new(label: &str, expected_value: String) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, Some(expected_value))
    }

    fn new_unbound(label: &str) -> std::io::Result<Self> {
        Self::new_with_expected_value(label, None)
    }

    fn new_with_expected_value(
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

    fn attested_value(&self) -> std::io::Result<Option<String>> {
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

    fn is_attested(&self) -> std::io::Result<bool> {
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

    fn authorize(&self) -> std::io::Result<()> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit permit value",
            )
        })?;
        self.authorize_value(expected_value)
    }

    fn authorize_value(&self, expected_value: &str) -> std::io::Result<()> {
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

    fn completion_status(&self) -> std::io::Result<Option<i32>> {
        let expected_value = self.expected_value.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an unbound WSL launch guard requires an explicit completion identity",
            )
        })?;
        self.completion_status_for(expected_value)
    }

    fn completion_status_for(&self, expected_value: &str) -> std::io::Result<Option<i32>> {
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

fn verify_proxy_completion_status(
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

fn verify_pre_authorization_proxy_status(proxy_status: ExitStatus) -> std::io::Result<ExitStatus> {
    if proxy_status.success() {
        Err(std::io::Error::other(
            "WSL1 proxy exited successfully before launch authorization; the target was not executed",
        ))
    } else {
        Ok(proxy_status)
    }
}

fn revalidate_dedicated_wsl1_installation(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match dedicated_wsl1_installation_id_for(&config.distro) {
            Some(actual) if actual == expected_installation_id => return Ok(()),
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the WSL1 dedicated-runtime identity changed after child launch",
                ));
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unable to revalidate the WSL1 dedicated-runtime identity before termination",
                ));
            }
        }
    }
}

fn running_wsl_distributions() -> std::io::Result<Vec<String>> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--running", "--quiet"]);
    let output = process::run_probe(&mut command)?;
    if !output.status.success() || output.stdout_truncated {
        return Err(std::io::Error::other(
            "unable to inspect running WSL distributions",
        ));
    }
    Ok(decode_wsl_output(&output.stdout)
        .replace('\0', "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn wait_for_wsl_distro_to_stop_within(distro: &str, timeout: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let running = running_wsl_distributions()?;
        if !running.iter().any(|candidate| candidate == distro) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("WSL distro {distro} remained running after termination"),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_wsl_distro_to_stop(distro: &str) -> std::io::Result<()> {
    wait_for_wsl_distro_to_stop_within(distro, Duration::from_secs(5))
}

fn terminate_dedicated_wsl1_distro(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    revalidate_dedicated_wsl1_installation(config, expected_installation_id)?;
    trace(format!(
        "terminating dedicated WSL1 distro {} installation {} after cancellation",
        config.distro, expected_installation_id
    ));
    let mut command = Command::new("wsl.exe");
    command.args(["--terminate", &config.distro]);
    match process::run_probe(&mut command) {
        Ok(output) if output.status.success() => wait_for_wsl_distro_to_stop(&config.distro),
        Ok(output) => {
            trace(format!(
                "WSL1 terminate returned {}: {}{}",
                output.status,
                decode_wsl_output(&output.stdout).trim(),
                decode_wsl_output(&output.stderr).trim()
            ));
            Err(std::io::Error::other("WSL1 terminate command failed"))
        }
        Err(error) => Err(error),
    }
}

fn stop_cancelled_wsl1_child(
    child: &mut Child,
    config: &Config,
    expected_installation_id: Option<&str>,
) -> std::io::Result<ExitStatus> {
    // Stop the Windows proxy first so it cannot outlive XUVA. The Linux-side
    // command is still blocked on the identity-bound permit at this point.
    let _ = child.kill();
    let termination = expected_installation_id
        .map(|installation_id| terminate_dedicated_wsl1_distro(config, installation_id));
    let proxy_deadline = Instant::now() + Duration::from_secs(3);
    let proxy_status = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }
        if Instant::now() >= proxy_deadline {
            break Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the Windows WSL1 proxy remained alive after cancellation",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };

    match termination {
        Some(Err(error)) => {
            // An identity change means XUVA must not terminate a potentially
            // unrelated distro. The unpermitted child self-expires instead,
            // and XUVA proves that the runtime stopped before returning the
            // failure.
            let stopped =
                wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12));
            return Err(match stopped {
                Ok(()) => error,
                Err(stop_error) => std::io::Error::other(format!(
                    "{error}; the untrusted WSL1 runtime also failed to stop: {stop_error}"
                )),
            });
        }
        None => {
            wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12)).map_err(
                |stop_error| {
                    std::io::Error::other(format!(
                        "the unpermitted WSL1 runtime failed to stop after proxy cancellation: {stop_error}"
                    ))
                },
            )?;
        }
        Some(Ok(())) => {}
    }
    proxy_status
}

fn wait_for_wsl1_child(
    mut child: Child,
    config: &Config,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    let mut authorized = false;
    let mut accepted_installation_id = None;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    loop {
        let cancellation_requested = console::requested();
        if cancellation_requested {
            match (
                accepted_installation_id.as_deref(),
                launch_guard.attested_value(),
            ) {
                (Some(installation_id), _) => {
                    return stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                }
                (None, Ok(Some(installation_id))) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id);
                    return stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                }
                (None, Ok(Some(_))) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the cancelled WSL1 child attested an invalid installation ID",
                    ));
                }
                (None, Err(error)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
                (None, Ok(None)) if started.elapsed() < Duration::from_secs(10) => {
                    // The target remains blocked because no permit is ever
                    // published. Keep the proxy alive long enough for the
                    // root-owned dedicated marker to be attested, then use
                    // that exact identity for safe distro termination.
                }
                (None, Ok(None)) => {
                    let cleanup = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(match cleanup {
                        Ok(_) => std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "cancelled WSL1 child never attested a dedicated-runtime identity",
                        ),
                        Err(cleanup_error) => cleanup_error,
                    });
                }
            }
        }
        if !authorized && !cancellation_requested {
            match launch_guard.attested_value() {
                Ok(Some(installation_id)) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id.clone());
                    if let Err(error) = launch_guard.authorize_value(&installation_id) {
                        let cleanup = stop_cancelled_wsl1_child(
                            &mut child,
                            config,
                            accepted_installation_id.as_deref(),
                        );
                        return Err(match cleanup {
                            Ok(_) => error,
                            Err(cleanup_error) => std::io::Error::other(format!(
                                "{error}; WSL1 authorization cleanup failed: {cleanup_error}"
                            )),
                        });
                    }
                    authorized = true;
                    trace("authorized an identity-matched WSL1 child");
                }
                Ok(Some(_)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the WSL1 child attested an invalid dedicated-runtime installation ID",
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
            }
        }
        if authorized
            && !test_proxy_killed
            && test_kill_wsl1_proxy_after_permit()
            && test_ready_file_exists()
        {
            // The test-only ready boundary is published immediately before
            // target launch. Give the target one scheduler turn so the
            // contract exercises a proxy failure after execution has begun.
            thread::sleep(Duration::from_millis(100));
            trace("test hook terminated the WSL1 Windows proxy after launch permit");
            let _ = child.kill();
            test_proxy_killed = true;
        }
        if proxy_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    trace(format!("WSL1 Windows proxy exited with {status}"));
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    let cleanup = stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 child status cleanup failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if let Some(status) = proxy_status {
            if !authorized {
                return verify_pre_authorization_proxy_status(status);
            }
            let installation_id = accepted_installation_id.as_deref().ok_or_else(|| {
                std::io::Error::other(
                    "an authorized WSL1 child has no accepted dedicated-runtime identity",
                )
            })?;
            match launch_guard.completion_status_for(installation_id) {
                Ok(Some(attested_status)) => {
                    match verify_proxy_completion_status(status, attested_status) {
                        Ok(status) => return Ok(status),
                        Err(error) => {
                            let cleanup = stop_cancelled_wsl1_child(
                                &mut child,
                                config,
                                Some(installation_id),
                            );
                            return Err(match cleanup {
                                Ok(_) => error,
                                Err(cleanup_error) => std::io::Error::other(format!(
                                    "{error}; WSL1 mismatch recovery failed: {cleanup_error}"
                                )),
                            });
                        }
                    }
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    let error = std::io::Error::other(
                        "WSL1 proxy exited after launch permit without a completion attestation",
                    );
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 incomplete-launch recovery failed: {cleanup_error}"
                        )),
                    });
                }
                Err(error) => {
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 invalid-completion recovery failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if !authorized && started.elapsed() >= Duration::from_secs(10) {
            let _ =
                stop_cancelled_wsl1_child(&mut child, config, accepted_installation_id.as_deref());
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "WSL1 child did not attest its dedicated-runtime identity within 10 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_wsl_child(
    mut child: Child,
    config: &Config,
    token: &str,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let launched_at = Instant::now();
    let mut authorized = false;
    let mut pending_error: Option<std::io::Error> = None;
    let mut cancellation_started: Option<Instant> = None;
    let mut interrupt_sent = false;
    let mut terminate_sent = false;
    let mut kill_sent = false;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    let mut test_proxy_reap_deferred = false;
    let test_defer_proxy_reap = test_defer_wsl2_proxy_reap_until_cleanup();
    if test_defer_proxy_reap {
        trace("test hook armed deferred WSL2 proxy reap");
    }
    loop {
        if console::requested() {
            cancellation_started.get_or_insert_with(Instant::now);
        }
        let defer_proxy_reap = test_proxy_killed && test_defer_proxy_reap;
        if defer_proxy_reap && !test_proxy_reap_deferred {
            trace("test hook deferred WSL2 proxy reap until Linux cleanup");
            test_proxy_reap_deferred = true;
        }
        if proxy_status.is_none()
            && !defer_proxy_reap
            && let Some(status) = child.try_wait()?
        {
            trace(format!("WSL2 Windows proxy exited with {status}"));
            proxy_status = Some(status);
            proxy_exited_at = Some(Instant::now());
        }
        if !authorized && cancellation_started.is_none() && proxy_status.is_none() {
            match launch_guard.is_attested() {
                Ok(true) => match launch_guard.authorize() {
                    Ok(()) => {
                        authorized = true;
                        trace("authorized a cancellation-ready WSL2 child");
                    }
                    Err(error) => {
                        pending_error = Some(error);
                        cancellation_started = Some(Instant::now());
                    }
                },
                Ok(false) if launched_at.elapsed() < Duration::from_secs(10) => {}
                Ok(false) => {
                    pending_error = Some(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "WSL2 child did not attest its private cancellation token within 10 seconds",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(status) = proxy_status.as_ref()
            && cancellation_started.is_none()
            && pending_error.is_none()
        {
            match launch_guard.completion_status() {
                Ok(Some(attested_status)) => {
                    return verify_proxy_completion_status(*status, attested_status);
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    pending_error = Some(std::io::Error::other(
                        "WSL2 proxy exited before the Linux launcher attested complete process-group cleanup",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(started) = cancellation_started {
            let elapsed = started.elapsed();
            if test_kill_wsl2_proxy_during_cancellation()
                && !test_proxy_killed
                && proxy_status.is_none()
            {
                trace("test hook terminated the WSL2 Windows proxy during cancellation");
                let _ = child.kill();
                test_proxy_killed = true;
            }
            if !interrupt_sent && send_linux_signal(config, token, "INT").unwrap_or(false) {
                trace("sent SIGINT to the isolated Linux process group");
                interrupt_sent = true;
            }
            if elapsed >= Duration::from_millis(1_500)
                && !terminate_sent
                && send_linux_signal(config, token, "TERM").unwrap_or(false)
            {
                trace("escalated cancellation to SIGTERM inside Linux");
                terminate_sent = true;
            }
            if elapsed >= Duration::from_secs(3)
                && !kill_sent
                && send_linux_signal(config, token, "KILL").unwrap_or(false)
            {
                trace("escalated cancellation to SIGKILL inside Linux");
                kill_sent = true;
            }
            let completion = match launch_guard.completion_status() {
                Ok(status) => status,
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let group_state = match linux_process_group_state(config, token) {
                Ok(state) => Some(state),
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let cleanup_proven = matches!(group_state, Some(LinuxProcessGroupState::Gone))
                || matches!(
                    (group_state, completion),
                    (Some(LinuxProcessGroupState::TokenUnavailable), Some(_))
                );
            if cleanup_proven {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                if let Some(error) = pending_error {
                    return Err(error);
                }
                return Ok(status);
            }
            if elapsed >= Duration::from_secs(4) && proxy_status.is_none() {
                let _ = child.kill();
                if let Some(status) = child.try_wait()? {
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
            }
            if elapsed >= Duration::from_secs(15) {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                trace(format!(
                    "reaped WSL2 Windows proxy with {status} after failed cleanup proof"
                ));
                let cleanup_error = std::io::Error::other(match group_state {
                    Some(LinuxProcessGroupState::Alive) => {
                        "Linux process group survived SIGINT, SIGTERM, and SIGKILL escalation"
                    }
                    Some(LinuxProcessGroupState::TokenUnavailable) => {
                        "WSL2 cancellation token disappeared without a completion attestation"
                    }
                    Some(LinuxProcessGroupState::Gone) => {
                        "WSL2 cleanup completed without a reapable Windows proxy"
                    }
                    None => "unable to prove WSL2 process-group cleanup after proxy exit",
                });
                return Err(match pending_error {
                    Some(error) => std::io::Error::other(format!(
                        "{error}; cancellation finalization failed: {cleanup_error}"
                    )),
                    None => cleanup_error,
                });
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn print_adapter_info(config: &Config) {
    println!("adapter={PRODUCT_COMMAND}");
    println!("command={PRODUCT_COMMAND}");
    println!("profile={}", config.profile.as_str());
    println!("route_preference={}", config.route_preference.as_str());
    println!("environment={}", config.environment.as_str());
    println!("policy_objective={}", config.policy_objective.as_str());
    println!(
        "environment_allowlist={}",
        if config.environment_allowlist.is_empty() {
            "none".to_owned()
        } else {
            config.environment_allowlist.join(",")
        }
    );
    println!("environment_boolean_feature_gates=automatic");
    println!("native_rtk_path={}", config.native_rtk_path);
    println!(
        "metrics={}",
        if config.metrics_enabled {
            "local-aggregate-only"
        } else {
            "off"
        }
    );
}

fn begin_invocation_metrics(
    config: &Config,
    adapter: &dispatcher::OutputAdapter,
) -> Option<XuvaMetrics> {
    if !config.metrics_enabled {
        return None;
    }
    let result = if matches!(adapter, dispatcher::OutputAdapter::Raw) {
        XuvaMetrics::begin_unmeasured()
    } else {
        XuvaMetrics::begin()
    };
    match result {
        Ok(metrics) => Some(metrics),
        Err(error) => {
            eprintln!("xuva: metrics disabled for this invocation: {error}");
            None
        }
    }
}

fn run_native_rtk(
    arguments: &[OsString],
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    adapters::windows::run_rtk_at(&config.native_rtk_path, arguments, None, metrics)
}

fn execution_route(route: &dispatcher::RouteCandidate) -> Route {
    match route {
        dispatcher::RouteCandidate::Windows { .. } => Route::Raw,
        dispatcher::RouteCandidate::Wsl1 { .. } => Route::Wsl1,
        dispatcher::RouteCandidate::Wsl2 { .. } => Route::Wsl2,
    }
}

fn run_execution_plan(
    plan: &dispatcher::ExecutionPlan,
    config: &Config,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let rtk_arguments = || {
        let mut forwarded = Vec::with_capacity(plan.request.arguments.len() + 1);
        forwarded.push(plan.request.executable.clone());
        forwarded.extend(plan.request.arguments.iter().cloned());
        forwarded
    };
    match (&plan.candidate, &plan.adapter) {
        (
            dispatcher::RouteCandidate::Windows { executable, .. },
            dispatcher::OutputAdapter::Raw,
        ) => adapters::windows::run_plan(executable, &plan.request),
        (
            dispatcher::RouteCandidate::Windows { .. },
            dispatcher::OutputAdapter::Rtk { executable },
        ) => adapters::windows::run_rtk_plan(executable, &rtk_arguments(), &plan.request, metrics),
        (
            dispatcher::RouteCandidate::Wsl1 { .. } | dispatcher::RouteCandidate::Wsl2 { .. },
            adapter,
        ) => {
            let selected = provider_execution_config(config, &plan.candidate, adapter)?;
            let forwarded = match adapter {
                dispatcher::OutputAdapter::Raw => plan.request.arguments.clone(),
                dispatcher::OutputAdapter::Rtk { .. } => rtk_arguments(),
            };
            let raw_executable = match &plan.candidate {
                dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
                dispatcher::RouteCandidate::Windows { .. } => {
                    unreachable!("WSL arm has a WSL candidate")
                }
            };
            let executable = match adapter {
                dispatcher::OutputAdapter::Raw => raw_executable,
                dispatcher::OutputAdapter::Rtk { executable } => executable,
            };
            let measured = matches!(adapter, dispatcher::OutputAdapter::Rtk { .. })
                .then_some(metrics)
                .flatten();
            run_wsl_execution_plan(
                executable,
                &forwarded,
                &plan.request.environment,
                &selected,
                execution_route(&plan.candidate),
                measured,
            )
        }
    }
}

fn provider_exec_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(2).and_then(|argument| argument.to_str()) else {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let separator = arguments.iter().position(|argument| argument == "--");
    let Some(separator) = separator else {
        eprintln!("xuva: provider execution requires `--` before tool arguments");
        return ExitCode::FAILURE;
    };
    if separator < 3 {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    let options = &arguments[3..separator];
    let candidate_index = match options {
        [] => None,
        [flag, index] if flag == "--candidate" => index.to_string_lossy().parse::<usize>().ok(),
        _ => None,
    };
    if !options.is_empty() && candidate_index.is_none() {
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    // Execution is explicit and must not reuse a provider identity discovered
    // under a previous RTK path or tool installation state.
    let resolution = resolve_tool_provider(tool, config, true);
    let forwarded = &arguments[separator + 1..];
    let (index, candidate, plan) = if let Some(index) = candidate_index {
        let Some(candidate) = resolution.candidates.get(index) else {
            eprintln!("xuva: provider candidate {index} does not exist; run `xuva resolve {tool}`");
            return ExitCode::FAILURE;
        };
        if !candidate.usable {
            eprintln!(
                "xuva: provider candidate {index} is not verified: {}",
                candidate.reason
            );
            return ExitCode::from(127);
        }
        match execution_plan_for_explicit_provider_candidate(tool, forwarded, config, candidate) {
            Ok(plan) => (index, candidate, plan),
            Err(error) => {
                eprintln!(
                    "xuva: provider candidate {index} cannot produce an execution plan: {error}"
                );
                return ExitCode::from(127);
            }
        }
    } else {
        let selected =
            first_compatible_provider_plan(tool, forwarded, config, &resolution.candidates);
        let Some(selected) = selected else {
            eprintln!(
                "xuva: no verified provider supports the requested output adapter and command semantics; run `xuva resolve {tool}` for details"
            );
            return ExitCode::from(127);
        };
        selected
    };
    let route = execution_route(&plan.candidate);
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let metrics = begin_invocation_metrics(config, &plan.adapter);
    let result = run_execution_plan(&plan, config, metrics.as_ref());
    if needs_console_handler {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    if let Some(metrics) = metrics {
        let command_family = format!(
            "provider:{}:{}",
            candidate.host.as_str(),
            plan.adapter.as_str()
        );
        if let Err(error) = metrics.finish(
            route.as_str(),
            &command_family,
            started.elapsed(),
            exit_code,
        ) {
            eprintln!("xuva: metrics were not recorded: {error}");
        }
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!("xuva: unable to start provider candidate {index}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_wsl_route(
    arguments: Vec<OsString>,
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 child was spawned",
            ));
        }
        trace("starting identity-gated WSL1 child while holding the global mutex");
        let mut process = adapters::wsl1::process(wsl1_rtk_arguments_with_metrics(
            arguments,
            config,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ));
        let result = process.spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 child completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        adapters::wsl2::process(rtk_arguments_with_metrics(
            arguments,
            config,
            &token,
            metrics_path.as_deref(),
            &launch_guard.attestation_wsl_path,
            &launch_guard.permit_wsl_path,
            &launch_guard.completion_wsl_path,
        ))
        .spawn()
        .and_then(|child| {
            trace("spawned cancellation-gated WSL2 proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}

fn run_wsl_execution_plan(
    executable: &OsString,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        let path = metrics.scratch_windows_path().to_string_lossy();
        mapped_windows_project_path(&config.distro, config.user.as_deref(), &path)
            .or_else(|| windows_path_to_wsl_path(&path))
    });
    if route == Route::Wsl1 {
        let lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        require_wsl1_version(config)?;
        let launch_guard = LaunchPermitGuard::new_unbound("wsl1")?;
        if console::requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before the WSL1 plan was spawned",
            ));
        }
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: None,
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
            },
        )?;
        trace("starting identity-gated WSL1 plan while holding the global mutex");
        let result = adapters::wsl1::process(command).spawn().and_then(|child| {
            trace("spawned identity-gated WSL1 plan proxy");
            wait_for_wsl1_child(child, config, &launch_guard)
        });
        trace(format!(
            "identity-gated WSL1 plan completed with {result:?}"
        ));
        drop(lock);
        result
    } else {
        let token = cancellation_nonce()?;
        let launch_guard = LaunchPermitGuard::new("wsl2", token.clone())?;
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            WslLaunchMetadata {
                cancel_nonce: Some(&token),
                metrics_db_path: metrics_path.as_deref(),
                attestation_path: Some(&launch_guard.attestation_wsl_path),
                permit_path: Some(&launch_guard.permit_wsl_path),
                completion_path: Some(&launch_guard.completion_wsl_path),
            },
        )?;
        adapters::wsl2::process(command).spawn().and_then(|child| {
            trace("spawned cancellation-gated WSL2 plan proxy");
            wait_for_wsl_child(child, config, &token, &launch_guard)
        })
    }
}

fn run_cli(arguments: Vec<OsString>, config: &Config) -> ExitCode {
    if is_verbose_version_command(&arguments) {
        print_verbose_version();
        return ExitCode::SUCCESS;
    }
    if is_version_command(&arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some(HELP_ARGUMENT | "help" | "-h"))
    {
        cli::print_help();
        return ExitCode::SUCCESS;
    }
    if let Some(result) = lifecycle::command(&arguments) {
        return result;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SELF_UPDATE_ARGUMENT)
    {
        return self_update::command(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == AGENT_ARGUMENT)
    {
        return agent::command(&arguments, &config.native_rtk_path);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SURFACE_ARGUMENT)
    {
        return print_command_surface(&arguments);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == PROVIDER_ARGUMENT)
    {
        if arguments.get(1).is_some_and(|argument| argument == "exec") {
            return provider_exec_command(&arguments, config);
        }
        eprintln!("xuva: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == RESOLVE_ARGUMENT)
    {
        return provider_command(&arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == WHICH_ARGUMENT)
    {
        let mut resolve_arguments = arguments.clone();
        resolve_arguments[0] = OsString::from(RESOLVE_ARGUMENT);
        return provider_command(&resolve_arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return provider_command(&arguments, config, true);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SCAN_ARGUMENT)
    {
        return provider_scan_command(&arguments, config);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SETUP_ARGUMENT)
    {
        return setup_command(&arguments, config);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == POLICY_ARGUMENT)
    {
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "context")
            && arguments.len() == 2
        {
            return match serde_json::to_string_pretty(&policy_context_report(config)) {
                Ok(rendered) => {
                    println!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: unable to render policy context: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            match load_route_policy() {
                Some(policy) => match serde_json::to_string_pretty(&policy) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("xuva: unable to render route policy: {error}");
                        return ExitCode::FAILURE;
                    }
                },
                None => println!("No local route policy is installed."),
            }
            return ExitCode::SUCCESS;
        }
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "import")
            && arguments.len() == 3
        {
            return match import_route_policy(Path::new(&arguments[2]), config) {
                Ok(()) => {
                    println!("Imported local XUVA route policy.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("xuva: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!(
            "{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} policy [show|context] | policy import <evidence.json>"
        );
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == CALIBRATION_ARGUMENT)
    {
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            return match print_calibration(config.policy_objective) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("xuva: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!("{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} calibration [show]");
        return ExitCode::FAILURE;
    }
    if arguments.len() == 1 && arguments[0] == ADAPTER_INFO_ARGUMENT {
        print_adapter_info(config);
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && (arguments[0] == "gain" || arguments[0] == "stats") {
        return match XuvaMetrics::print_gain() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xuva: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let (arguments, requested_route, environment, explain) =
        match parse_options(arguments, config.route_preference, config.environment) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::FAILURE;
            }
        };
    if arguments.is_empty() {
        eprintln!(
            "{PRODUCT_COMMAND}: no command supplied; run `{PRODUCT_COMMAND} --help` for usage"
        );
        return ExitCode::FAILURE;
    }
    if is_shell_operator_command(&arguments) {
        eprintln!(
            "xuva: `{}` is shell syntax, not an executable; let PowerShell, cmd, or a POSIX shell own the pipeline and invoke XUVA only for command argv",
            arguments[0].to_string_lossy()
        );
        return ExitCode::from(2);
    }
    let mut invocation_config = config.clone();
    invocation_config.environment = environment;
    let current_directory = env::current_dir().ok();
    let same_host_raw_fast_path = matches!(
        invocation_config.invocation_origin,
        InvocationOrigin::Windows
    ) && !explain
        && (requested_route == Route::Raw
            || (requested_route == Route::Auto
                && !is_verified_read_only_git(&arguments)
                && should_use_native_git(
                    &arguments,
                    &invocation_config,
                    current_directory.as_deref().and_then(|path| path.to_str()),
                )));
    if same_host_raw_fast_path {
        return match adapters::windows::run(&arguments) {
            Ok(status) => ExitCode::from_status(status),
            Err(error) => {
                eprintln!("xuva: unable to start Windows raw command: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let started = Instant::now();
    let policy_eligible = requested_route == Route::Auto && route_policy_key(&arguments).is_some();
    let calibration_eligible =
        requested_route == Route::Auto && routing::is_calibration_candidate(&arguments);
    let adaptive_context = if policy_eligible || calibration_eligible {
        adaptive_context_signature(&invocation_config)
    } else {
        String::new()
    };
    let policy = policy_eligible.then(load_route_policy).flatten();
    let (initial_route, initial_reason) = if requested_route == Route::Auto {
        auto_route_for_environment(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            Some(&adaptive_context),
            environment,
            invocation_config.policy_objective,
        )
    } else {
        (requested_route, "explicit route preference")
    };
    let mut route = initial_route;
    let mut reason = initial_reason.to_owned();
    let calibration = if calibration_eligible {
        let calibration_state = match load_calibration() {
            Ok(state) => Some(state),
            Err(error) => {
                eprintln!("xuva: local calibration state is unavailable: {error}");
                None
            }
        };
        match calibration_plan(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            calibration_state.as_ref(),
            &adaptive_context,
            invocation_config.policy_objective,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("xuva: local calibration is unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    if let Some(plan) = &calibration {
        route = plan.route;
        reason = plan.reason.to_owned();
    }
    let selected_config = configured_wsl_backend(&invocation_config, route);
    let mut provider_missing = None;
    let mut execution_plan = None;
    let mut fallback_execution_plans = Vec::new();
    let mut selected_adapter = match route {
        Route::Raw => dispatcher::OutputAdapter::Raw,
        Route::NativeRtk | Route::Wsl1 | Route::Wsl2 | Route::Auto => {
            dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(&selected_config.native_rtk_path),
            }
        }
    };
    let explicit_plan = match explicit_executable_plan(&arguments, &invocation_config) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("xuva: {error}");
            return ExitCode::from(127);
        }
    };
    if let Some((plan, explicit_reason)) = explicit_plan {
        route = execution_route(&plan.candidate);
        selected_adapter = dispatcher::OutputAdapter::Raw;
        execution_plan = Some(plan);
        reason = explicit_reason;
    } else if requested_route == Route::Auto && environment == ExecutionEnvironment::Adaptive {
        trace(format!(
            "adaptive provider planning for {}",
            command_family(&arguments)
        ));
        match provider_dispatch_decision(&arguments, &invocation_config, route) {
            ProviderDispatchDecision::KeepStaticRoute => {}
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason: provider_reason,
            } => {
                route = execution_route(&plan.candidate);
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(*plan);
                fallback_execution_plans = fallbacks;
                reason = provider_reason;
            }
            ProviderDispatchDecision::Missing {
                reason: missing_reason,
            } => {
                provider_missing = Some(missing_reason.clone());
                reason = missing_reason;
            }
        }
    }
    if execution_plan.is_none()
        && matches!(
            invocation_config.invocation_origin,
            InvocationOrigin::Wsl { .. }
        )
        && matches!(route, Route::Raw | Route::NativeRtk)
    {
        match static_windows_execution_plan(&arguments, &invocation_config, route) {
            Ok(plan) => {
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(plan);
                reason = "WSL-origin Windows route requires an isolated execution plan".to_owned();
            }
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::from(127);
            }
        }
    }
    if explain {
        println!("route={}", route.as_str());
        println!("output_adapter={}", selected_adapter.as_str());
        println!("reason={reason}");
        println!("command_family={}", command_family(&arguments));
        if let Some(plan) = &execution_plan {
            let provider = match &plan.candidate {
                dispatcher::RouteCandidate::Windows { executable, .. }
                | dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
            };
            println!("provider={}", provider.to_string_lossy());
        }
        return if provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        };
    }
    if let Some(reason) = provider_missing {
        eprintln!("xuva: {reason}");
        return ExitCode::from(127);
    }
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    let mut console_installed = false;
    if needs_console_handler && !console::install() {
        eprintln!("xuva: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    } else if needs_console_handler {
        console_installed = true;
    }
    let metrics = begin_invocation_metrics(&invocation_config, &selected_adapter);
    let mut executed_route = route;
    let result = if let Some(plan) = execution_plan.as_ref() {
        let mut result = run_execution_plan(plan, &invocation_config, metrics.as_ref());
        for fallback in &fallback_execution_plans {
            if !result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                break;
            }
            let fallback_route = execution_route(&fallback.candidate);
            trace(format!(
                "selected provider executable was unavailable before child start; retrying {} candidate",
                fallback_route.as_str()
            ));
            if matches!(fallback_route, Route::Wsl1 | Route::Wsl2) && !console_installed {
                if !console::install() {
                    eprintln!(
                        "xuva: unable to register the Windows console cancellation handler for provider fallback"
                    );
                    return ExitCode::FAILURE;
                }
                console_installed = true;
            }
            executed_route = fallback_route;
            result = run_execution_plan(fallback, &invocation_config, metrics.as_ref());
        }
        result
    } else {
        match route {
            Route::Raw => adapters::windows::run(&arguments),
            Route::NativeRtk => {
                match run_native_rtk(&arguments, &selected_config, metrics.as_ref()) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && requested_route == Route::Auto
                            && environment == ExecutionEnvironment::Adaptive =>
                    {
                        trace(
                            "native RTK was not found; falling back to isolated WSL1 before any child started",
                        );
                        if !console_installed {
                            if !console::install() {
                                eprintln!(
                                    "xuva: unable to register the Windows console cancellation handler for WSL fallback"
                                );
                                return ExitCode::FAILURE;
                            }
                            console_installed = true;
                        }
                        executed_route = Route::Wsl1;
                        let fallback_config =
                            configured_wsl_backend(&invocation_config, Route::Wsl1);
                        run_wsl_route(
                            arguments.clone(),
                            &fallback_config,
                            Route::Wsl1,
                            metrics.as_ref(),
                        )
                    }
                    result => result,
                }
            }
            Route::Wsl1 | Route::Wsl2 => {
                run_wsl_route(arguments.clone(), &selected_config, route, metrics.as_ref())
            }
            Route::Auto => unreachable!("auto route is resolved before execution"),
        }
    };
    if console_installed {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    let elapsed = started.elapsed();
    let totals = if let Some(metrics) = metrics {
        match metrics.finish(
            executed_route.as_str(),
            command_family(&arguments),
            elapsed,
            exit_code,
        ) {
            Ok(totals) => totals,
            Err(error) => {
                eprintln!("xuva: metrics were not recorded: {error}");
                TokenTotals::default()
            }
        }
    } else {
        TokenTotals::default()
    };
    if invocation_config.metrics_enabled
        && let Some(plan) = &calibration
        && let Err(error) = record_calibration(plan, executed_route, elapsed, exit_code, totals)
    {
        eprintln!("xuva: local calibration was not recorded: {error}");
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from_status(status),
        Err(error) => {
            eprintln!(
                "xuva: unable to start {} route: {error}",
                executed_route.as_str()
            );
            ExitCode::FAILURE
        }
    }
}

fn main_exit() -> ExitCode {
    let original_arguments: Vec<OsString> = env::args_os().skip(1).collect();
    // This is intentionally before bridge decoding and environment parsing:
    // a local version query must remain instant even when WSL is unavailable
    // or a caller has an invalid dispatcher configuration.
    if is_verbose_version_command(&original_arguments) {
        print_verbose_version();
        return ExitCode::SUCCESS;
    }
    if is_version_command(&original_arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // Lifecycle recovery must remain available even when a stale environment
    // contains invalid routing configuration. WSL bridge requests are decoded
    // below and receive the same handling inside `run_cli`.
    if let Some(result) = lifecycle::command(&original_arguments) {
        return result;
    }
    let bridge = match wsl_bridge_request(&original_arguments) {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("xuva: invalid WSL bridge payload: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("xuva: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    let arguments = if let Some(bridge) = bridge {
        config.invocation_origin = InvocationOrigin::Wsl {
            distro: bridge.distro.clone(),
        };
        config.distro = bridge.distro;
        config.user = Some(bridge.origin_user);
        config.cwd = Some(bridge.cwd);
        config.bridge_windows_cwd = bridge.windows_cwd;
        config.extra_path = bridge.extra_path;
        config.output_adapter = bridge.output_adapter;
        bridge.arguments
    } else {
        original_arguments
    };
    run_cli(arguments, &config)
}

pub fn run_from_env() -> ! {
    main_exit().terminate();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config::from_lookup(|_| None).expect("default config is valid")
    }

    #[test]
    fn version_commands_are_owned_by_the_dispatcher() {
        for argument in [VERSION_ARGUMENT, "version", "-V"] {
            assert!(
                is_version_command(&[OsString::from(argument)]),
                "{argument}"
            );
        }
        assert!(is_verbose_version_command(&[
            OsString::from("--version"),
            OsString::from("--verbose")
        ]));
        assert!(!is_version_command(&[
            OsString::from("go"),
            OsString::from("version")
        ]));
    }

    #[test]
    fn forwards_special_characters_as_distinct_arguments() {
        let arguments = rtk_arguments(
            vec![
                OsString::from("run"),
                OsString::from("semi;and&dollar$HOME"),
                OsString::from("C:\\Program Files\\Example"),
            ],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );

        assert!(arguments.contains(&OsString::from("--exec")));
        assert!(arguments.contains(&OsString::from(LAUNCH_SCRIPT)));
        assert!(arguments.contains(&OsString::from("semi;and&dollar$HOME")));
        assert!(arguments.contains(&OsString::from("C:\\Program Files\\Example")));
    }

    #[test]
    fn wsl_bridge_payload_preserves_literal_utf8_argv_without_shell_parsing() {
        let fields = decode_wsl_bridge_fields("Z28AcnVuAHNwYWNlICYgJGRvbGxhclzmvKLlrZcA")
            .expect("valid base64 payload decodes");
        assert_eq!(
            fields,
            vec![
                "go".to_owned(),
                "run".to_owned(),
                "space & $dollar\\\u{6f22}\u{5b57}".to_owned(),
            ]
        );
        assert!(decode_wsl_bridge_fields("Z28=").is_err());
        assert!(decode_wsl_bridge_fields("not base64!").is_err());
    }

    #[test]
    fn wsl_bridge_request_carries_context_and_arguments_without_environment() {
        let request = wsl_bridge_request(&[OsString::from(
            "--wsl-bridge=djMAVWJ1bnR1AGJhZGFyAC9tbnQvZC9maXh0dXJlAEQ6XGZpeHR1cmUAL3RtcC9maXh0dXJlAHJhdwAtLWV4cGxhaW4tcm91dGUAZ28AcnVuAHgA",
        )])
        .expect("bridge payload is valid")
        .expect("argument selects the bridge");
        assert_eq!(request.distro, "Ubuntu");
        assert_eq!(request.origin_user, "badar");
        assert_eq!(request.cwd, "/mnt/d/fixture");
        assert_eq!(request.windows_cwd.as_deref(), Some(r"D:\fixture"));
        assert_eq!(request.extra_path.as_deref(), Some("/tmp/fixture"));
        assert_eq!(request.output_adapter, OutputAdapterPreference::Raw);
        assert_eq!(
            request.arguments,
            vec![
                OsString::from("--explain-route"),
                OsString::from("go"),
                OsString::from("run"),
                OsString::from("x"),
            ]
        );
    }

    #[test]
    fn wsl_plan_launcher_forwards_environment_as_structured_assignments() {
        let config = default_config();
        let arguments = plan_wsl_arguments_with_metrics(
            &OsString::from("/tmp/go"),
            &[OsString::from("run"), OsString::from("$literal & text")],
            &[(
                OsString::from("P7_OVERLAY"),
                OsString::from("value with spaces"),
            )],
            &config,
            Route::Wsl2,
            WslLaunchMetadata {
                cancel_nonce: Some("0123456789abcdef0123456789abcdef"),
                metrics_db_path: None,
                attestation_path: Some("/tmp/xuva-test.attestation"),
                permit_path: Some("/tmp/xuva-test.permit"),
                completion_path: Some("/tmp/xuva-test.completion"),
            },
        )
        .expect("WSL plan arguments are valid");
        let executable = arguments
            .iter()
            .position(|argument| argument == "/tmp/go")
            .expect("plan includes executable");
        let overlay = arguments
            .iter()
            .position(|argument| argument == "P7_OVERLAY=value with spaces")
            .expect("plan includes environment overlay");
        let user_argument = arguments
            .iter()
            .position(|argument| argument == "$literal & text")
            .expect("plan includes literal user argument");
        assert!(arguments.contains(&OsString::from(PLAN_LAUNCH_SCRIPT)));
        assert!(overlay < executable && executable < user_argument);
        assert!(
            wsl_environment_assignments(&[(
                OsString::from("INVALID-NAME"),
                OsString::from("value"),
            )])
            .is_err()
        );
    }

    #[test]
    fn execution_plan_applies_command_environment_and_cwd_to_windows_processes() {
        let request = dispatcher::CommandSpec {
            executable: OsString::from("fixture.exe"),
            arguments: vec![OsString::from("space value"), OsString::from("$literal")],
            cwd: Some(PathBuf::from(r"E:\work")),
            environment: vec![(OsString::from("P7_OVERLAY"), OsString::from("enabled"))],
            environment_policy: dispatcher::EnvironmentPolicy::Isolated,
            interactive: true,
        };
        let mut command = Command::new("fixture.exe");
        apply_command_spec(&mut command, &request);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![&OsString::from("space value"), &OsString::from("$literal")]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new(r"E:\work")));
        assert!(command.get_envs().any(|(key, value)| {
            key == "P7_OVERLAY" && value == Some(std::ffi::OsStr::new("enabled"))
        }));
    }

    #[test]
    fn explicit_wsl1_route_uses_the_windows_mutex_and_supervised_process_group() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        let command = wsl1_rtk_arguments(
            vec![
                OsString::from("proxy"),
                OsString::from("/usr/bin/printf"),
                OsString::from("%s"),
                OsString::from("space & $HOME"),
            ],
            &config,
        );
        let strings = command
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            strings
                .iter()
                .any(|value| value.contains("publish_completion"))
        );
        assert!(
            strings
                .iter()
                .any(|value| value.contains("/usr/bin/env -i"))
        );
        assert!(!strings.iter().any(|value| value.contains("/usr/bin/flock")));
        assert!(strings.iter().any(|value| value == "/usr/bin/setsid"));
        assert_eq!(
            strings.last().map(|value| value.as_ref()),
            Some("space & $HOME")
        );
    }

    #[test]
    fn every_wsl1_launch_surface_uses_the_same_strict_marker_validator() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        let rtk_arguments = wsl1_rtk_arguments_with_metrics(
            vec![OsString::from("smart")],
            &config,
            None,
            "/tmp/xuva-test.attestation",
            "/tmp/xuva-test.permit",
            "/tmp/xuva-test.completion",
        );
        let plan_arguments = plan_wsl_arguments_with_metrics(
            &OsString::from("/usr/bin/printf"),
            &[OsString::from("%s"), OsString::from("fixture")],
            &[],
            &config,
            Route::Wsl1,
            WslLaunchMetadata {
                cancel_nonce: None,
                metrics_db_path: None,
                attestation_path: Some("/tmp/xuva-test.attestation"),
                permit_path: Some("/tmp/xuva-test.permit"),
                completion_path: Some("/tmp/xuva-test.completion"),
            },
        )
        .expect("WSL1 plan arguments are valid");

        for arguments in [&rtk_arguments, &plan_arguments] {
            assert_eq!(
                arguments
                    .iter()
                    .filter(|argument| argument.as_os_str() == WSL1_MARKER_VALIDATOR_SCRIPT)
                    .count(),
                1,
                "each WSL1 launch must receive the canonical marker validator exactly once"
            );
            assert!(
                arguments
                    .iter()
                    .any(|argument| argument.as_os_str() == WSL1_LAUNCH_SCRIPT)
            );
        }
        assert!(!WSL1_LAUNCH_SCRIPT.contains("marker=/etc/xuva-dedicated-wsl1"));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("stat -Lc '%u:%a'"));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("!= \"0:444\""));
        assert!(WSL1_MARKER_VALIDATOR_SCRIPT.contains("grep -c '^installation_id='"));
    }

    #[test]
    fn wsl1_proxy_cannot_report_success_before_target_authorization() {
        let success = Command::new("cmd.exe")
            .args(["/d", "/c", "exit 0"])
            .status()
            .expect("successful proxy fixture starts");
        let rejected = verify_pre_authorization_proxy_status(success)
            .expect_err("pre-authorization success must not impersonate target success");
        assert!(rejected.to_string().contains("target was not executed"));

        let failure = Command::new("cmd.exe")
            .args(["/d", "/c", "exit 126"])
            .status()
            .expect("failed proxy fixture starts");
        assert_eq!(
            verify_pre_authorization_proxy_status(failure)
                .expect("launcher failure remains observable")
                .code(),
            Some(126)
        );
    }

    #[test]
    fn stats_remains_a_compatibility_alias() {
        let arguments = rtk_arguments(
            vec![OsString::from("stats")],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );
        assert_eq!(arguments.last(), Some(&OsString::from("gain")));
    }

    #[test]
    fn maps_windows_drive_paths_for_wsl_current_directory() {
        assert_eq!(
            windows_path_to_wsl_path(r"D:\projects\rtk-wsl"),
            Some("/mnt/d/projects/rtk-wsl".to_owned())
        );
        assert_eq!(
            windows_path_to_wsl_path(r"F:\path with spaces\漢字"),
            Some("/mnt/f/path with spaces/漢字".to_owned())
        );
        assert_eq!(
            windows_path_to_wsl_path(r"\\?\E:\projects\rtk-wsl"),
            Some("/mnt/e/projects/rtk-wsl".to_owned())
        );
        assert_eq!(windows_path_to_wsl_path(r"\\server\share"), None);
    }

    #[test]
    fn defaults_to_the_selected_wsl_users_home() {
        let arguments = rtk_arguments(
            vec![OsString::from("help")],
            &default_config(),
            "0123456789abcdef0123456789abcdef",
        );

        assert!(arguments.contains(&OsString::from("")));
        assert!(arguments.iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("rtk_path=\"$HOME/.local/bin/rtk\"")
        }));
        assert!(!arguments.contains(&OsString::from("-u")));
    }

    #[test]
    fn validates_configuration_without_ambient_user_defaults() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            "XUVA_WSL_USER" => Some("alex".to_owned()),
            "XUVA_WSL_RTK_PATH" => Some("/opt/rtk/bin/rtk".to_owned()),
            "XUVA_WSL_CWD" => Some("/work/custom-mount".to_owned()),
            "XUVA_WSL_EXTRA_PATH" => Some("/opt/fixture-bin:/work/tools".to_owned()),
            _ => None,
        })
        .expect("portable config is valid");

        let arguments = rtk_arguments(
            vec![OsString::from("help")],
            &config,
            "0123456789abcdef0123456789abcdef",
        );
        assert!(arguments.windows(2).any(|pair| pair == ["-u", "alex"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--cd", "/work/custom-mount"])
        );
        assert!(arguments.contains(&OsString::from("/opt/rtk/bin/rtk")));
        assert!(arguments.contains(&OsString::from("/opt/fixture-bin:/work/tools")));
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_configuration() {
        let invalid_wait = Config::from_lookup(|name| match name {
            "XUVA_WSL_LOCK_WAIT_SECONDS" => Some("0".to_owned()),
            _ => None,
        });
        assert!(invalid_wait.is_err());

        let relative_path = Config::from_lookup(|name| match name {
            "XUVA_WSL_RTK_PATH" => Some("bin/rtk".to_owned()),
            _ => None,
        });
        assert!(relative_path.is_err());

        let invalid_extra_path = Config::from_lookup(|name| match name {
            "XUVA_WSL_EXTRA_PATH" => Some("relative:/opt/tools".to_owned()),
            _ => None,
        });
        assert!(invalid_extra_path.is_err());

        let invalid_objective = Config::from_lookup(|name| match name {
            "XUVA_POLICY_OBJECTIVE" => Some("fastest-ish".to_owned()),
            _ => None,
        });
        assert!(invalid_objective.is_err());
    }

    #[test]
    fn cancellation_uses_a_separate_structured_wsl_command() {
        let arguments = cancel_arguments(
            &default_config(),
            "0123456789abcdef0123456789abcdef",
            "TERM",
        );
        assert!(arguments.contains(&OsString::from(CANCEL_SCRIPT)));
        assert!(arguments.contains(&OsString::from("0123456789abcdef0123456789abcdef")));
        assert!(
            !arguments
                .iter()
                .any(|argument| { argument.to_string_lossy().starts_with("/tmp/xuva-") })
        );
        assert!(arguments.contains(&OsString::from("TERM")));
    }

    #[test]
    fn launch_permit_requires_the_exact_attested_identity_and_cleans_up() {
        let expected = "0123456789abcdef0123456789abcdef".to_owned();
        let (attestation, permit, completion);
        {
            let guard =
                LaunchPermitGuard::new("unit", expected.clone()).expect("launch guard is created");
            attestation = guard.attestation_windows_path.clone();
            permit = guard.permit_windows_path.clone();
            completion = guard.completion_windows_path.clone();
            let mut staging = attestation.as_os_str().to_os_string();
            staging.push(".staging");
            fs::write(PathBuf::from(staging), &expected).expect("staged attestation is written");
            assert!(
                !guard
                    .is_attested()
                    .expect("an unpublished attestation remains invisible")
            );
            fs::write(&attestation, "ffffffffffffffffffffffffffffffff")
                .expect("mismatched attestation is written");
            let mismatch = guard
                .is_attested()
                .expect_err("mismatched launch identity is rejected");
            assert_eq!(mismatch.kind(), std::io::ErrorKind::PermissionDenied);

            fs::write(&attestation, &expected).expect("matching attestation is written");
            assert!(guard.is_attested().expect("attestation is readable"));
            guard.authorize().expect("matching launch is authorized");
            assert_eq!(
                fs::read_to_string(&permit).expect("permit is readable"),
                expected
            );
            fs::write(&completion, format!("{expected}:37"))
                .expect("matching completion is written");
            assert_eq!(
                guard.completion_status().expect("completion is valid"),
                Some(37)
            );
            fs::write(&completion, format!("{expected}:999"))
                .expect("out-of-range completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("out-of-range completion is rejected")
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
            fs::write(&completion, "ffffffffffffffffffffffffffffffff:37")
                .expect("mismatched completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("mismatched completion identity is rejected")
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
            fs::write(&completion, "not-a-completion").expect("malformed completion is written");
            assert_eq!(
                guard
                    .completion_status()
                    .expect_err("malformed completion is rejected")
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        assert!(!attestation.exists());
        assert!(!permit.exists());
        assert!(!completion.exists());
    }

    #[test]
    fn unbound_launch_permit_requires_explicit_identity_acceptance() {
        let installation_id = "01234567-89ab-cdef-0123-456789abcdef";
        let guard =
            LaunchPermitGuard::new_unbound("unit-unbound").expect("unbound guard is created");
        assert_eq!(
            guard
                .is_attested()
                .expect_err("unbound attestation cannot be accepted implicitly")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            guard
                .authorize()
                .expect_err("unbound permit cannot publish an implicit identity")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        fs::write(&guard.attestation_windows_path, installation_id)
            .expect("dedicated identity attestation is written");
        assert_eq!(
            guard
                .attested_value()
                .expect("attestation is readable")
                .as_deref(),
            Some(installation_id)
        );
        guard
            .authorize_value(installation_id)
            .expect("explicitly accepted dedicated identity is authorized");
        assert_eq!(
            fs::read_to_string(&guard.permit_windows_path).expect("permit is readable"),
            installation_id
        );
    }

    #[test]
    fn wsl2_launchers_only_remove_proven_dead_cancellation_tokens() {
        for script in [LAUNCH_SCRIPT, PLAN_LAUNCH_SCRIPT] {
            assert!(!script.contains("-mmin"));
            assert!(script.contains("/bin/kill -0 -- \"-$stale_worker\""));
            assert!(script.contains("group_has_other_members"));
            assert!(script.contains("publish_completion"));
            assert_eq!(
                script.matches("stat_fields=${stat_value##*) }").count(),
                2,
                "both process-group scans must parse after the final comm delimiter"
            );
            assert!(
                !script.contains("stat_fields=${stat_value#*) }"),
                "shortest-prefix /proc stat parsing can misread comm containing `) `"
            );

            let finish = script
                .split_once("finish() {")
                .expect("launcher has a finish trap")
                .1
                .split_once("trap finish EXIT")
                .expect("launcher finish trap is installed")
                .0;
            let failed_quiescence_exit = finish
                .find("exit 125")
                .expect("failed quiescence exits without attestation");
            let cleanup = finish
                .find("\n    cleanup\n")
                .expect("successful quiescence removes its token");
            let completion = finish
                .find("\n    publish_completion ")
                .expect("successful quiescence publishes completion");
            assert!(
                failed_quiescence_exit < cleanup && cleanup < completion,
                "cleanup and completion must remain unreachable after quiescence failure"
            );
            assert!(
                !finish[..failed_quiescence_exit].contains("publish_completion"),
                "failed quiescence must not publish cleanup proof"
            );
        }
    }

    #[test]
    fn routes_windows_worktree_git_to_native_git_by_default() {
        assert!(should_use_native_git(
            &[OsString::from("git"), OsString::from("status")],
            &default_config(),
            Some(r"E:\luthfi\project\flowpeek"),
        ));
    }

    #[test]
    fn keeps_explicit_wsl_git_paths_and_wsl_mode_in_wsl() {
        assert!(!should_use_native_git(
            &[
                OsString::from("git"),
                OsString::from("-C"),
                OsString::from("/mnt/e/project"),
                OsString::from("status")
            ],
            &default_config(),
            Some(r"E:\luthfi\project\flowpeek"),
        ));
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_GIT_MODE" => Some("wsl".to_owned()),
            _ => None,
        })
        .expect("WSL Git mode is valid");
        assert!(!should_use_native_git(
            &[OsString::from("git"), OsString::from("status")],
            &config,
            Some(r"E:\luthfi\project\flowpeek"),
        ));
    }

    #[test]
    fn validates_git_mode() {
        let invalid = Config::from_lookup(|name| match name {
            "XUVA_WSL_GIT_MODE" => Some("other".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn explicit_wsl1_backend_selects_the_isolated_distro_without_affecting_default_xuva() {
        let default = default_config();
        assert_eq!(default.backend, WslBackend::Auto);
        assert_eq!(default.distro, DEFAULT_DISTRO);

        let wsl1 = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        assert_eq!(wsl1.backend, WslBackend::Wsl1);
        assert_eq!(wsl1.distro, DEFAULT_WSL1_DISTRO);
    }

    #[test]
    fn explicit_backend_and_distro_select_the_xuva_wsl_provider() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("wsl2".to_owned()),
            "XUVA_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            _ => None,
        })
        .expect("explicit backend configuration is valid");
        assert_eq!(config.backend, WslBackend::Wsl2);
        assert_eq!(config.distro, "Ubuntu-24.04");

        let invalid = Config::from_lookup(|name| match name {
            "XUVA_WSL_BACKEND" => Some("legacy".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn canonical_xuva_configuration_is_adaptive_by_default() {
        let xuva = default_config();
        assert_eq!(xuva.profile, ExecutableProfile::Xuva);
        assert_eq!(xuva.backend, WslBackend::Auto);
        assert_eq!(xuva.route_preference, Route::Auto);
        assert!(xuva.metrics_enabled);

        let metrics_off =
            Config::from_lookup(|name| (name == "XUVA_METRICS").then(|| "off".to_owned()))
                .expect("metrics can be disabled explicitly");
        assert!(!metrics_off.metrics_enabled);
        assert!(
            Config::from_lookup(|name| { (name == "XUVA_METRICS").then(|| "remote".to_owned()) })
                .is_err()
        );
    }

    #[test]
    fn embedded_command_surface_is_complete_and_non_overlapping() {
        let report = command_surface_report();
        assert_eq!(report.schema_version, 2);
        assert_eq!(report.adapter.name, "rtk");
        assert_eq!(report.adapter.version, "0.43.0");
        assert_eq!(report.adapter.protocol_version, 1);
        assert_eq!(report.upstream_command_count, 69);
        let names = report
            .commands
            .iter()
            .map(|row| row.command.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), report.upstream_command_count);
        assert!(
            report
                .commands
                .iter()
                .all(|row| row.classification != CommandSurface::Unknown)
        );
        assert_eq!(command_surface("git"), CommandSurface::NativeStructured);
        assert_eq!(command_surface("go"), CommandSurface::RawNative);
        assert_eq!(command_surface("proxy"), CommandSurface::Wsl1Conservative);
        assert_eq!(command_surface("gain"), CommandSurface::CoreInternal);
    }

    #[test]
    fn adapter_only_rtk_commands_never_enter_generic_provider_resolution() {
        let config = default_config();
        for command in ["smart", "proxy", "rewrite", "hook"] {
            assert!(is_adapter_only_rtk_command(command));
            let arguments = [OsString::from(command), OsString::from("literal-argument")];
            assert!(
                matches!(
                    provider_dispatch_decision(&arguments, &config, Route::Wsl1),
                    ProviderDispatchDecision::KeepStaticRoute
                ),
                "{command} must remain an adapter-owned RTK command"
            );
        }
        assert!(is_rtk_meta_command("wc"));
        assert!(requires_raw_posix_provider("wc"));
        assert!(!is_adapter_only_rtk_command("wc"));
    }

    #[test]
    fn xuva_auto_route_keeps_mutations_raw_and_read_only_commands_structured() {
        let mutation = vec![
            OsString::from("git"),
            OsString::from("commit"),
            OsString::from("-m"),
        ];
        assert_eq!(auto_route(&mutation, Some(r"E:\work"), None).0, Route::Raw);

        let clone = vec![
            OsString::from("git"),
            OsString::from("clone"),
            OsString::from("https://example.invalid/repo"),
        ];
        assert_eq!(auto_route(&clone, Some(r"E:\work"), None).0, Route::Raw);

        let read_only = vec![
            OsString::from("git"),
            OsString::from("log"),
            OsString::from("-1"),
        ];
        assert_eq!(
            auto_route(&read_only, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        let cargo = vec![
            OsString::from("cargo"),
            OsString::from("check"),
            OsString::from("--version"),
        ];
        assert_eq!(
            auto_route(&cargo, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        assert_eq!(
            auto_route(&[OsString::from("npm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("npx")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("pnpm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("go")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("dotnet")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("dart")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_route(&[OsString::from("flutter")], Some(r"E:\work"), None).0,
            Route::Raw
        );

        let literal = vec![
            OsString::from("proxy"),
            OsString::from("/usr/bin/printf"),
            OsString::from("$HOME; &"),
        ];
        assert_eq!(auto_route(&literal, Some(r"E:\work"), None).0, Route::Wsl1);
    }

    #[test]
    fn policy_uses_measured_savings_without_permitting_git_mutations() {
        let context = adaptive_context_signature(&default_config());
        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: context.clone(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "git:status".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 20.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "cargo:check".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "npm:run-list".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "go:test-all".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
            ],
        };
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("git"), OsString::from("status")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("rg"), OsString::from("needle")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("cargo"), OsString::from("check")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_with_context(
                &[OsString::from("npm"), OsString::from("run")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_with_context(
                &[
                    OsString::from("go"),
                    OsString::from("test"),
                    OsString::from("./...")
                ],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context),
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route(
                &[OsString::from("go"), OsString::from("test")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route(
                &[
                    OsString::from("npm"),
                    OsString::from("run"),
                    OsString::from("test")
                ],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route(
                &[
                    OsString::from("git"),
                    OsString::from("clone"),
                    OsString::from("url")
                ],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
    }

    #[test]
    fn adaptive_evidence_is_bound_to_manifest_and_local_adapter_context() {
        let default = default_config();
        let context = adaptive_context_signature(&default);
        let mut different = default.clone();
        different.native_rtk_path = r"C:\tools\other-rtk.exe".to_owned();
        assert_ne!(context, adaptive_context_signature(&different));

        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: context.clone(),
            evidence: vec![RoutePolicyEvidence {
                key: "rg".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 20.0,
                token_savings_percent: 0.0,
                sample_count: 5,
            }],
        };
        assert_eq!(
            policy.route_for("rg", &context, PolicyObjective::Balanced),
            Some(Route::Raw)
        );
        assert_eq!(
            policy.route_for("rg", "0123456789abcdef", PolicyObjective::Balanced),
            None
        );
    }

    #[test]
    fn xuva_route_options_are_explicit_and_validate_values() {
        let (arguments, route, environment, explain) = parse_options(
            vec![
                OsString::from("--route"),
                OsString::from("native-rtk"),
                OsString::from("--explain-route"),
                OsString::from("rg"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("route options are valid");
        assert_eq!(route, Route::NativeRtk);
        assert_eq!(environment, ExecutionEnvironment::Adaptive);
        assert!(explain);
        assert_eq!(arguments, vec![OsString::from("rg")]);
        assert!(
            parse_options(
                vec![OsString::from("--route"), OsString::from("unsafe")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );

        let (arguments, route, environment, explain) = parse_options(
            vec![
                OsString::from("--environment"),
                OsString::from("windows-only"),
                OsString::from("pytest"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("windows-only option is valid");
        assert_eq!(arguments, vec![OsString::from("pytest")]);
        assert_eq!(route, Route::Auto);
        assert_eq!(environment, ExecutionEnvironment::WindowsOnly);
        assert!(!explain);
        assert!(
            parse_options(
                vec![OsString::from("--environment"), OsString::from("hybrid")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );
    }

    #[test]
    fn windows_only_routes_external_commands_raw_and_keeps_rtk_meta_native() {
        assert_eq!(
            auto_route_for_environment(
                &[OsString::from("pytest"), OsString::from("-q")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_route_for_environment(
                &[OsString::from("init"), OsString::from("-g")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_route_for_environment(
                &[
                    OsString::from("git"),
                    OsString::from("commit"),
                    OsString::from("-m"),
                    OsString::from("x")
                ],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
                PolicyObjective::Balanced,
            )
            .0,
            Route::Raw
        );
    }

    #[test]
    fn decodes_and_parses_redirected_wsl_distribution_output() {
        let text = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
        let utf16 = text
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        let decoded = decode_wsl_output(&utf16);
        assert_eq!(distro_version_from_list(&decoded, "Ubuntu"), Some(2));
        assert_eq!(
            distro_version_from_list(&decoded, "Ubuntu-RTK-WSL1"),
            Some(1)
        );
        assert_eq!(
            distro_version_from_list(&decoded, "Custom WSL One"),
            Some(1)
        );
        assert_eq!(distro_version_from_list(&decoded, "missing"), None);
    }

    #[test]
    fn provider_discovery_parses_wsl_distro_names_and_versions() {
        let output = "  NAME                   STATE           VERSION\r\n* Ubuntu                  Running         2\r\n  Ubuntu-RTK-WSL1         Stopped         1\r\n  Custom WSL One          Stopped         1\r\n";
        assert_eq!(
            parse_wsl_distributions(output),
            vec![
                ("Ubuntu".to_owned(), Some(2)),
                ("Ubuntu-RTK-WSL1".to_owned(), Some(1)),
                ("Custom WSL One".to_owned(), Some(1)),
            ]
        );
        assert!(!is_eligible_wsl_distro("docker-desktop"));
        assert!(!is_eligible_wsl_distro("docker-desktop-data"));
        assert!(is_eligible_wsl_distro("Ubuntu-24.04"));
    }

    #[test]
    fn provider_discovery_classifies_windows_and_wsl_project_paths() {
        let windows = classify_project_path(r"E:\luthfi\project\rtk-wsl");
        assert_eq!(windows.kind, ProjectLocationKind::Windows);
        assert_eq!(windows.distro, None);

        let wsl = classify_project_path(r"\\wsl.localhost\Ubuntu-24.04\home\luthfi\project");
        assert_eq!(wsl.kind, ProjectLocationKind::Wsl);
        assert_eq!(wsl.distro.as_deref(), Some("Ubuntu-24.04"));
        assert_eq!(wsl.path, "/home/luthfi/project");
    }

    #[test]
    fn windows_provider_discovery_recognizes_native_launchable_extensions() {
        assert!(is_windows_launchable_path(r"C:\tools\go.exe"));
        assert!(is_windows_launchable_path(r"C:\tools\npm.cmd"));
        assert!(is_windows_launchable_path(r"C:\tools\gradle.bat"));
        assert!(is_windows_launchable_path(r"C:\tools\legacy.com"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm.ps1"));
        assert_eq!(
            select_windows_executable(vec![
                r"C:\tools\npm".to_owned(),
                r"C:\tools\npm.cmd".to_owned(),
                r"C:\tools\npm.ps1".to_owned(),
            ]),
            Some(r"C:\tools\npm.cmd".to_owned())
        );
        assert_eq!(
            select_windows_executable(vec![
                r"C:\tools\script.ps1".to_owned(),
                r"C:\tools\script.py".to_owned(),
            ]),
            None
        );
    }

    #[test]
    fn provider_cache_uses_a_bounded_freshness_window() {
        let entry = ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 100,
            inspection_level: InspectionLevel::Version,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: None,
                native_rtk: None,
                executable_version: None,
                version_probe_status: ProbeStatus::NotRequested,
                executable_capabilities: Vec::new(),
                executable_identity: None,
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: Vec::new(),
        };
        assert!(cache_entry_is_fresh(
            &entry,
            100 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
        assert!(
            !cache_entry_is_fresh(&entry, 100, "changed-path-or-git-revision", true),
            "a changed discovery fingerprint invalidates even a new entry"
        );
        assert!(!cache_entry_is_fresh(
            &entry,
            101 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
        let mut identity_only = entry.clone();
        identity_only.inspection_level = InspectionLevel::Identity;
        assert!(cache_entry_is_fresh(&identity_only, 100, "fixture", false));
        assert!(
            !cache_entry_is_fresh(&identity_only, 100, "fixture", true),
            "doctor/version verification must upgrade an identity-only cache entry"
        );
    }

    #[test]
    fn version_probe_registry_never_executes_unknown_tools() {
        assert_eq!(version_probe_arguments("git"), Some(&["--version"][..]));
        assert_eq!(version_probe_arguments("go"), Some(&["version"][..]));
        assert_eq!(version_probe_arguments("user-defined-tool"), None);
    }

    #[test]
    fn explicit_windows_executable_paths_bypass_provider_discovery() {
        let fixture = env::temp_dir().join(format!(
            "xuva-explicit-path-{}-{}.cmd",
            std::process::id(),
            unix_seconds()
        ));
        fs::write(&fixture, "@exit /b 0\r\n").expect("explicit fixture is written");
        let arguments = vec![
            fixture.clone().into_os_string(),
            OsString::from("literal argument"),
        ];
        let (plan, reason) = explicit_executable_plan(&arguments, &default_config())
            .expect("explicit path is valid")
            .expect("explicit path creates a plan");
        assert!(matches!(
            plan.candidate,
            dispatcher::RouteCandidate::Windows { .. }
        ));
        assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
        assert_eq!(
            plan.request.arguments,
            vec![OsString::from("literal argument")]
        );
        assert!(reason.contains("explicit Windows"));
        fs::remove_file(fixture).expect("explicit fixture is removed");
    }

    #[test]
    fn provider_cache_fingerprint_changes_with_wsl_extra_path() {
        let default = default_config();
        let configured = Config::from_lookup(|name| match name {
            "XUVA_WSL_EXTRA_PATH" => Some("/tmp/xuva-go/bin".to_owned()),
            _ => None,
        })
        .expect("extra path configuration is valid");
        assert_ne!(
            discovery_context_signature(&default, false),
            discovery_context_signature(&configured, false),
            "changing the executable search overlay must invalidate discovery"
        );
    }

    #[test]
    fn provider_resolution_requires_a_verified_cross_host_project_mapping() {
        let probe = WslToolProbe {
            distro: "Ubuntu".to_owned(),
            user: None,
            wsl_version: Some(2),
            dedicated: false,
            installation_id: None,
            executable: Some("/usr/bin/go".to_owned()),
            rtk: Some("/home/test/.local/bin/rtk".to_owned()),
            executable_version: None,
            version_probe_status: ProbeStatus::NotRequested,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert_eq!(
            wsl_project_path_with(
                &windows_project,
                &probe,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, r"E:\work");
                    None
                },
                |_, _| true,
            ),
            None
        );
        assert_eq!(
            wsl_project_path_with(
                &windows_project,
                &probe,
                |_, _| Some("/mnt/e/work".to_owned()),
                |distro, path| distro == "Ubuntu" && path == "/mnt/e/work",
            ),
            Some("/mnt/e/work".to_owned())
        );

        let same_wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert_eq!(
            wsl_project_path_with(
                &same_wsl_project,
                &probe,
                |_, _| None,
                |distro, path| distro == "Ubuntu" && path == "/home/test/work",
            ),
            Some("/home/test/work".to_owned())
        );

        assert_eq!(
            wsl_project_path_with(&same_wsl_project, &probe, |_, _| None, |_, _| false,),
            None
        );

        let bridged_other_distro_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/mnt/host/d/work".to_owned(),
            distro: Some("docker-desktop".to_owned()),
            windows_path: Some(r"D:\work".to_owned()),
        };
        assert_eq!(
            wsl_project_path_with(
                &bridged_other_distro_project,
                &probe,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, r"D:\work");
                    Some("/mnt/d/work".to_owned())
                },
                |distro, path| distro == "Ubuntu" && path == "/mnt/d/work",
            ),
            Some("/mnt/d/work".to_owned()),
            "a WSL-origin bridge may cross distros only through a verified Windows-mounted path"
        );

        let mapping =
            wsl_mapping_arguments_with_user("Ubuntu", None, r"E:\work with spaces\$literal");
        assert_eq!(
            mapping,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-a"),
                OsString::from(r"E:\work with spaces\$literal"),
            ]
        );
        assert_eq!(
            wsl_mapping_arguments_with_user("Ubuntu", Some("luthfi"), r"E:\work"),
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("-u"),
                OsString::from("luthfi"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-a"),
                OsString::from(r"E:\work"),
            ]
        );
    }

    #[test]
    fn provider_resolution_verifies_wsl_to_windows_project_mappings() {
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work with spaces\漢字".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert_eq!(
            windows_project_path_with(
                &windows_project,
                |_, _| None,
                |path| { path == r"E:\work with spaces\漢字" }
            ),
            Some(r"E:\work with spaces\漢字".to_owned())
        );

        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/luthfi/work with spaces/漢字".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, "/home/luthfi/work with spaces/漢字");
                    Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
                },
                |path| path.contains("work with spaces"),
            ),
            Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work with spaces\漢字".to_owned())
        );
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |_, _| Some(r"\\wsl.localhost\Other\home\luthfi\work".to_owned()),
                |_| true,
            ),
            None,
            "a mapped UNC path must name the source WSL distribution"
        );
        assert_eq!(
            windows_project_path_with(
                &wsl_project,
                |_, _| Some(r"\\wsl.localhost\Ubuntu\home\luthfi\work".to_owned()),
                |_| false,
            ),
            None,
            "a path that Windows cannot read is never executable"
        );

        let arguments = windows_mapping_arguments_with_user(
            "Ubuntu",
            None,
            "/home/luthfi/work with spaces/$literal",
        );
        assert_eq!(
            arguments,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-w"),
                OsString::from("-a"),
                OsString::from("/home/luthfi/work with spaces/$literal"),
            ]
        );
        assert_eq!(
            windows_mapping_arguments_with_user("Ubuntu", Some("luthfi"), "/home/luthfi/work"),
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu"),
                OsString::from("-u"),
                OsString::from("luthfi"),
                OsString::from("--exec"),
                OsString::from("wslpath"),
                OsString::from("-w"),
                OsString::from("-a"),
                OsString::from("/home/luthfi/work"),
            ]
        );
    }

    #[test]
    fn provider_aware_go_routing_uses_only_a_complete_verified_wsl_candidate() {
        let config = default_config();
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                distro: Some("Ubuntu-22.04".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture: a verified WSL provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter.as_str(), "raw");
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref distro, ref cwd, .. }
                        if distro == "Ubuntu-22.04" && cwd == Path::new("/mnt/e/work")
                ));
                assert!(reason.contains("verified project path"));
            }
            _ => panic!("expected verified WSL provider selection"),
        }
    }

    #[test]
    fn provider_aware_go_routing_runs_a_wsl_only_go_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Go exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };
        assert!(
            has_complete_go_provider(&resolution),
            "a verified WSL raw Go binary is ready and must not trigger setup"
        );
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/usr/local/go/bin/go")
                ));
                assert!(reason.contains("raw output adapter"));
            }
            _ => panic!("expected the WSL-only raw Go provider"),
        }
    }

    #[test]
    fn generic_windows_executable_overrides_an_unavailable_legacy_wsl_route() {
        let project_path = env::current_dir()
            .expect("test project directory exists")
            .to_string_lossy()
            .to_string();
        let resolution = resolve_tool_provider_from_discovery_with_user(
            "nvm",
            ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: project_path.clone(),
                distro: None,
                windows_path: None,
            },
            ProviderCacheEntry {
                tool: "nvm".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\\Users\\test\\AppData\\Local\\nvm\\nvm.exe".to_owned()),
                    native_rtk: None,
                    executable_version: Some("1.2.2".to_owned()),
                    version_probe_status: ProbeStatus::Success,
                    executable_capabilities: vec!["version".to_owned()],
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: vec![WslToolProbe {
                    distro: "Ubuntu-RTK-WSL1".to_owned(),
                    user: None,
                    wsl_version: Some(1),
                    dedicated: true,
                    installation_id: Some("00000000-0000-0000-0000-000000000001".to_owned()),
                    executable: None,
                    rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    rtk_identity: None,
                }],
            },
            "miss",
            None,
        );

        assert_eq!(resolution.candidates.len(), 1);
        assert_eq!(resolution.availability.wsl[0].executable, None);
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("nvm"), OsString::from("ls")],
            &default_config(),
            Route::Wsl1,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { .. }
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(fallbacks.is_empty());
            }
            _ => {
                panic!("an unavailable WSL1 provider must not block generic Windows raw execution")
            }
        }
    }

    #[test]
    fn provider_planning_retains_the_next_eligible_route_for_pre_start_fallback() {
        let raw_config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let candidate = |distro: &str, version, executable: &str| ProviderCandidate {
            host: if version == 1 {
                ProviderHost::Wsl1
            } else {
                ProviderHost::Wsl2
            },
            adapters: vec![AdapterKind::Raw],
            distro: Some(distro.to_owned()),
            wsl_version: Some(version),
            executable: executable.to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![
                candidate("Ubuntu-RTK-WSL1", 1, "/opt/go-wsl1/bin/go"),
                candidate("Ubuntu", 2, "/opt/go-wsl2/bin/go"),
            ],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &raw_config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl1);
                assert_eq!(fallbacks.len(), 1);
                assert_eq!(execution_route(&fallbacks[0].candidate), Route::Wsl2);
            }
            _ => panic!("the usable WSL2 candidate must remain available as fallback"),
        }
    }

    #[test]
    fn generic_dispatcher_routes_a_wsl_only_cargo_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "XUVA_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/home/test/.cargo/bin/cargo".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/home/test/.cargo/bin/cargo")
                ));
                assert!(reason.contains("cargo discovery"));
            }
            _ => panic!("expected the WSL-only raw Cargo provider"),
        }
    }

    #[test]
    fn generic_dispatcher_falls_back_to_verified_windows_raw_when_rtk_is_absent() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Users\test\.cargo\bin\cargo.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                host: ProviderHost::Windows,
                adapters: vec![AdapterKind::Raw],
                distro: None,
                wsl_version: None,
                executable: r"C:\Users\test\.cargo\bin\cargo.exe".to_owned(),
                rtk: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists on Windows without RTK".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &default_config(),
            Route::NativeRtk,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { ref executable, .. }
                        if executable == &OsString::from(r"C:\Users\test\.cargo\bin\cargo.exe")
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(reason.contains("on Windows"));
            }
            _ => panic!("expected Windows raw fallback when native RTK is absent"),
        }
    }

    #[test]
    fn generic_dispatcher_discovers_every_safe_executable_name() {
        for tool in [
            "go",
            "cargo",
            "rustc",
            "node",
            "nvm",
            "npm",
            "pnpm",
            "python",
            "python3",
            "pytest",
            "java",
            "gradle",
            "mvn",
            "dotnet",
            "git",
            "tool.name",
            "cargo-next",
        ] {
            assert!(
                is_dispatchable_provider_tool(&[OsString::from(tool)]),
                "{tool}"
            );
        }
        assert!(!is_dispatchable_provider_tool(&[OsString::from("cmd /c")]));
        assert!(!is_dispatchable_provider_tool(&[OsString::from("go;exit")]));
    }

    #[test]
    fn execution_plan_rejects_inconsistent_provider_host_metadata() {
        let candidate = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let error = execution_plan_for_provider_candidate(
            "go",
            &[OsString::from("version")],
            &default_config(),
            &candidate,
        )
        .expect_err("host and WSL metadata must not contradict each other");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn requested_rtk_adapter_does_not_silently_downgrade_to_raw() {
        let candidate = ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        assert!(provider_adapter(&candidate, OutputAdapterPreference::Rtk).is_err());

        let adapter_only = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Rtk],
            distro: None,
            wsl_version: None,
            executable: r"C:\tools\rtk.exe".to_owned(),
            rtk: Some(r"C:\tools\rtk.exe".to_owned()),
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        assert!(provider_adapter(&adapter_only, OutputAdapterPreference::Raw).is_err());
        assert!(matches!(
            provider_adapter(&adapter_only, OutputAdapterPreference::Auto),
            Ok(dispatcher::OutputAdapter::Rtk { .. })
        ));
    }

    #[test]
    fn provider_aware_go_routing_reports_missing_without_an_install_action() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &default_config(),
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::Missing { reason } => {
                assert!(reason.contains("does not execute shell builtins implicitly"));
                assert!(reason.contains("doctor go"));
            }
            _ => panic!("expected a missing-provider diagnostic"),
        }
    }

    #[test]
    fn cached_windows_go_skips_cross_host_resolution_when_it_is_sufficient() {
        let windows = WindowsToolProbe {
            executable: Some(r"C:\Program Files\Go\bin\go.exe".to_owned()),
            native_rtk: None,
            executable_version: None,
            version_probe_status: ProbeStatus::NotRequested,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            native_rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert!(windows_tool_is_usable(
            "go",
            &windows_project,
            Route::Raw,
            &windows
        ));
        assert!(!windows_tool_is_usable(
            "go",
            &windows_project,
            Route::NativeRtk,
            &windows
        ));
        assert!(
            !windows_tool_is_usable("go", &windows_project, Route::Wsl1, &windows),
            "a conservative WSL fallback must not suppress Windows provider resolution"
        );
        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert!(!windows_tool_is_usable(
            "go",
            &wsl_project,
            Route::Raw,
            &windows
        ));
        assert!(
            !windows_tool_is_usable("find", &windows_project, Route::Raw, &windows),
            "Windows find.exe must never satisfy POSIX find semantics"
        );
        let mut structured = windows.clone();
        structured.native_rtk = Some(r"C:\Tools\rtk.exe".to_owned());
        assert!(windows_tool_is_usable(
            "go",
            &windows_project,
            Route::NativeRtk,
            &structured
        ));
        assert!(
            windows_tool_is_usable("find", &windows_project, Route::NativeRtk, &structured),
            "Windows RTK find is a structured adapter, not raw find.exe"
        );
    }

    #[test]
    fn cross_host_isolation_uses_origin_identity_and_preserves_unc_cwd() {
        let mut config = default_config();
        config.invocation_origin = InvocationOrigin::Wsl {
            distro: "Ubuntu".to_owned(),
        };
        config.cwd = Some("/home/test/project".to_owned());
        config.bridge_windows_cwd = Some(r"\\wsl.localhost\Ubuntu\home\test\project".to_owned());
        let candidate = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Tools\tool.exe".to_owned(),
            rtk: None,
            project_path: config.bridge_windows_cwd.clone(),
            usable: true,
            reason: "fixture".to_owned(),
        };

        assert_eq!(
            provider_environment_policy(&config, &candidate),
            dispatcher::EnvironmentPolicy::Isolated
        );
        assert_eq!(
            windows_cwd_for_invocation(&config).expect("UNC mapping is usable"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\test\project")
        );

        config.invocation_origin = InvocationOrigin::Windows;
        assert_eq!(
            provider_environment_policy(&config, &candidate),
            dispatcher::EnvironmentPolicy::Inherit
        );
    }

    #[test]
    fn explicit_provider_selection_skips_adapter_incompatible_candidates() {
        let mut config = default_config();
        config.output_adapter = OutputAdapterPreference::Rtk;
        let candidates = vec![
            ProviderCandidate {
                host: ProviderHost::Windows,
                adapters: vec![AdapterKind::Raw],
                distro: None,
                wsl_version: None,
                executable: r"C:\Tools\tool.exe".to_owned(),
                rtk: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "raw-only Windows fixture".to_owned(),
            },
            ProviderCandidate {
                host: ProviderHost::Wsl2,
                adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/bin/tool".to_owned(),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "RTK-capable WSL fixture".to_owned(),
            },
        ];

        let (index, candidate, plan) =
            first_compatible_provider_plan("tool", &[], &config, &candidates)
                .expect("a compatible provider exists");
        assert_eq!(index, 1);
        assert_eq!(candidate.host, ProviderHost::Wsl2);
        assert!(matches!(
            plan.adapter,
            dispatcher::OutputAdapter::Rtk { .. }
        ));
    }

    #[test]
    fn policy_objective_is_part_of_the_local_evidence_context() {
        let balanced = default_config();
        let mut latency = balanced.clone();
        latency.policy_objective = PolicyObjective::Latency;
        assert_ne!(
            adaptive_context_signature(&balanced),
            adaptive_context_signature(&latency)
        );
    }

    #[test]
    fn local_calibration_signature_does_not_expose_command_text() {
        let arguments = vec![OsString::from("rg"), OsString::from("sensitive value")];
        let signature = calibration_signature(&arguments, Some(r"E:\work"));
        assert_eq!(signature.len(), 16);
        assert!(!signature.contains("sensitive"));
        assert_ne!(
            signature,
            calibration_signature(
                &[OsString::from("rg"), OsString::from("other")],
                Some(r"E:\work")
            )
        );
    }

    #[test]
    fn provider_registry_accepts_safe_generic_tool_names_only() {
        for tool in ["git", "python3", "cargo-next", "tool.name", "go"] {
            assert!(
                is_safe_provider_tool_name(tool),
                "{tool} should be accepted"
            );
        }
        for tool in ["", "../tool", "tool/path", "tool;echo", "tool name", "工具"] {
            assert!(
                !is_safe_provider_tool_name(tool),
                "{tool} should be rejected"
            );
        }
    }

    #[test]
    fn provider_registry_parses_wsl_binary_identity_without_retaining_command_output() {
        let identity = parse_wsl_binary_identity(
            Some("/usr/local/bin/rtk".to_owned()),
            Some("2291200:1721880000".to_owned()),
        )
        .expect("valid stat identity is parsed");
        assert_eq!(identity.path, "/usr/local/bin/rtk");
        assert_eq!(identity.size_bytes, 2_291_200);
        assert_eq!(identity.modified_unix_seconds, 1_721_880_000);
        assert!(
            parse_wsl_binary_identity(Some("/bin/tool".to_owned()), Some("bad".to_owned()))
                .is_none()
        );
    }

    #[test]
    fn provider_fingerprint_separates_partial_and_complete_wsl_inventory() {
        let config = default_config();
        assert_ne!(
            discovery_context_signature(&config, false),
            discovery_context_signature(&config, true),
            "a Windows-only cache must not satisfy a complete WSL inventory request"
        );
    }

    #[test]
    fn wsl_provider_probe_rejects_shell_builtins_as_executables() {
        assert_eq!(verified_wsl_executable_path("read".to_owned()), None);
        assert_eq!(
            verified_wsl_executable_path("/usr/bin/find".to_owned()),
            Some("/usr/bin/find".to_owned())
        );
    }

    #[test]
    fn posix_command_families_do_not_collide_with_windows_system_tools() {
        for tool in ["find", "head", "tail", "grep", "tree"] {
            assert!(
                !windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
                "{tool}"
            );
            assert!(
                windows_provider_has_compatible_semantics(tool, AdapterKind::Rtk),
                "{tool}"
            );
        }
        for tool in ["find", "head", "tail", "tree"] {
            assert!(requires_raw_posix_provider(tool), "{tool}");
        }
        assert!(!requires_raw_posix_provider("grep"));
        for tool in ["git", "cargo", "python3"] {
            assert!(
                windows_provider_has_compatible_semantics(tool, AdapterKind::Raw),
                "{tool}"
            );
        }
    }

    #[test]
    fn execution_plans_translate_only_cross_host_absolute_path_arguments() {
        let windows = ProviderCandidate {
            host: ProviderHost::Windows,
            adapters: vec![AdapterKind::Raw],
            distro: None,
            wsl_version: None,
            executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
            rtk: None,
            project_path: Some(r"E:\work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let plan = execution_plan_for_provider_candidate(
            "git",
            &[
                OsString::from("-C"),
                OsString::from("/mnt/e/work"),
                OsString::from("status"),
                OsString::from("literal && value"),
            ],
            &default_config(),
            &windows,
        )
        .expect("Windows plan is valid");
        assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
        assert_eq!(
            plan.request.arguments[3],
            OsString::from("literal && value"),
            "non-path argv remains byte-for-byte structured"
        );

        let wsl = ProviderCandidate {
            host: ProviderHost::Wsl2,
            adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/bin/rtk".to_owned(),
            rtk: Some("/usr/local/bin/rtk".to_owned()),
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let plan = execution_plan_for_provider_candidate(
            "read",
            &[OsString::from(r"E:\work\Cargo.toml")],
            &default_config(),
            &wsl,
        )
        .expect("WSL plan is valid");
        assert_eq!(
            plan.request.arguments,
            vec![OsString::from("/mnt/e/work/Cargo.toml")]
        );
    }

    #[test]
    fn windows_git_mutations_have_no_wsl_execution_fallback() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "git".to_owned(),
            cache: "hit",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "git".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Program Files\Git\cmd\git.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![
                ProviderCandidate {
                    host: ProviderHost::Windows,
                    adapters: vec![AdapterKind::Raw],
                    distro: None,
                    wsl_version: None,
                    executable: r"C:\Program Files\Git\cmd\git.exe".to_owned(),
                    rtk: None,
                    project_path: Some(r"E:\work".to_owned()),
                    usable: true,
                    reason: "fixture".to_owned(),
                },
                ProviderCandidate {
                    host: ProviderHost::Wsl2,
                    adapters: vec![AdapterKind::Raw, AdapterKind::Rtk],
                    distro: Some("Ubuntu".to_owned()),
                    wsl_version: Some(2),
                    executable: "/usr/bin/git".to_owned(),
                    rtk: Some("/usr/local/bin/rtk".to_owned()),
                    project_path: Some("/mnt/e/work".to_owned()),
                    usable: true,
                    reason: "fixture".to_owned(),
                },
            ],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled",
        };
        match provider_dispatch_decision_from_resolution(
            &[
                OsString::from("git"),
                OsString::from("-C"),
                OsString::from("/mnt/e/work"),
                OsString::from("push"),
                OsString::from("origin"),
                OsString::from("HEAD"),
            ],
            &default_config(),
            Route::Wsl1,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason,
            } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { .. }
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(fallbacks.is_empty());
                assert!(reason.contains("Windows DNS"));
                assert_eq!(plan.request.arguments[1], OsString::from(r"E:\work"));
            }
            _ => panic!("Windows Git mutation must produce a native-only plan"),
        }
    }

    #[test]
    fn shell_operator_and_update_check_ux_are_owned_by_xuva() {
        assert!(is_shell_operator_command(&[OsString::from("&&")]));
        assert!(!is_shell_operator_command(&[
            OsString::from("rg"),
            OsString::from("literal && value")
        ]));
        let tags = "a refs/tags/v0.3.0\nb refs/tags/not-semver\nc refs/tags/v0.4.1\n";
        assert_eq!(
            latest_release_from_ls_remote(tags).as_deref(),
            Some("v0.4.1")
        );
        assert_eq!(parsed_stable_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parsed_stable_version("v1.2.3-rc1"), None);
        assert!(stable_release_is_newer("v1.2.3", "1.2.3-beta.1"));
        assert!(stable_release_is_newer("v1.2.4", "1.2.3-beta.1"));
        assert!(!stable_release_is_newer("v1.2.2", "1.2.3-beta.1"));
        assert!(!stable_release_is_newer("v1.2.3", "1.2.3"));
    }
}
