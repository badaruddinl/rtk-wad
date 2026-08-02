use std::env;
use std::path::PathBuf;

use crate::paths::windows_path_to_wsl_path;

pub(crate) fn test_ready_wsl_path() -> Option<String> {
    env::var("XUVA_WSL_TEST_READY_FILE")
        .ok()
        .and_then(|path| windows_path_to_wsl_path(&path))
}

pub(crate) fn test_wsl1_attestation_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL1_ATTESTATION_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 5)
        .unwrap_or(0)
}

pub(crate) fn test_wsl1_marker_path() -> String {
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

pub(crate) fn test_wsl2_launch_delay_seconds() -> u8 {
    if env::var("XUVA_TEST_MODE").as_deref() != Ok("1") {
        return 0;
    }
    env::var("XUVA_TEST_WSL2_LAUNCH_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 15)
        .unwrap_or(0)
}

pub(crate) fn test_completion_status_override() -> Option<u8> {
    (env::var("XUVA_TEST_MODE").as_deref() == Ok("1"))
        .then(|| {
            env::var("XUVA_TEST_COMPLETION_STATUS_OVERRIDE")
                .ok()
                .and_then(|value| value.parse::<u8>().ok())
        })
        .flatten()
}

pub(crate) fn test_kill_wsl1_proxy_after_permit() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL1_PROXY_AFTER_PERMIT").as_deref() == Ok("1")
}

pub(crate) fn test_ready_file_exists() -> bool {
    env::var_os("XUVA_WSL_TEST_READY_FILE").is_some_and(|path| PathBuf::from(path).is_file())
}

pub(crate) fn test_kill_wsl2_proxy_during_cancellation() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_KILL_WSL2_PROXY_DURING_CANCEL").as_deref() == Ok("1")
}

pub(crate) fn test_defer_wsl2_proxy_reap_until_cleanup() -> bool {
    env::var("XUVA_TEST_MODE").as_deref() == Ok("1")
        && env::var("XUVA_TEST_DEFER_WSL2_PROXY_REAP_UNTIL_CLEANUP").as_deref() == Ok("1")
}
