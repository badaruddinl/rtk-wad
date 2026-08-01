use std::ffi::OsString;
#[cfg(not(target_os = "windows"))]
use std::fs;
use std::process::Command;
use std::time::Duration;

use crate::config::Config;
use crate::diagnostics::trace;
use crate::process;

pub(crate) const CANCEL_SCRIPT: &str = include_str!("../scripts/cancel.sh");
pub(crate) const CANCEL_PROBE_SCRIPT: &str = include_str!("../scripts/cancel_probe.sh");

#[cfg(target_os = "windows")]
pub(crate) fn cancellation_nonce() -> std::io::Result<String> {
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
pub(crate) fn cancellation_nonce() -> std::io::Result<String> {
    use std::io::Read;

    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn cancel_arguments(config: &Config, token: &str, signal: &str) -> Vec<OsString> {
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
pub(crate) mod console {
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
pub(crate) mod console {
    pub fn install() -> bool {
        true
    }
    pub fn uninstall() {}
    pub fn requested() -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
pub(crate) mod windows_lock {
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
pub(crate) mod windows_lock {
    pub struct Guard;

    pub fn acquire(_wait_seconds: &str) -> Result<Guard, String> {
        Ok(Guard)
    }
}

pub(crate) fn send_linux_signal(
    config: &Config,
    token: &str,
    signal: &str,
) -> std::io::Result<bool> {
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
pub(crate) enum LinuxProcessGroupState {
    Alive,
    Gone,
    TokenUnavailable,
}

pub(crate) fn linux_process_group_state(
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
