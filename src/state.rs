//! Hardened persistence primitives for local XUVA state.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

static STATE_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const STATE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const STATE_LOCK_RETRY: Duration = Duration::from_millis(10);

pub(crate) fn secure_private_path(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("unable to inspect {label} permissions: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("refusing to secure symlinked {label}"));
    }
    set_private_path_permissions(path, metadata.is_dir())
        .map_err(|error| format!("unable to secure {label}: {error}"))
}

pub(crate) fn write_json_atomic<T: Serialize>(
    destination: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    with_state_lock(destination, label, || {
        write_json_atomic_unlocked(destination, value, label)
    })
}

pub(crate) fn update_json_atomic<T, Load, Mutate, Validate>(
    destination: &Path,
    label: &str,
    load: Load,
    mutate: Mutate,
    validate: Validate,
) -> Result<(), String>
where
    T: Serialize,
    Load: FnOnce(&Path) -> Result<T, String>,
    Mutate: FnOnce(&mut T) -> Result<(), String>,
    Validate: FnOnce(&T) -> Result<(), String>,
{
    with_state_lock(destination, label, || {
        let mut value = load(destination)?;
        mutate(&mut value)?;
        validate(&value)?;
        write_json_atomic_unlocked(destination, &value, label)
    })
}

fn write_json_atomic_unlocked<T: Serialize>(
    destination: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("unable to encode {label}: {error}"))?;
    write_bytes_atomic_unlocked(destination, &bytes, label)
}

fn with_state_lock<T>(
    destination: &Path,
    label: &str,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let parent = prepare_destination(destination, label)?;
    let lock_path = sibling_with_suffix(destination, "lock");
    reject_unsafe_destination(&lock_path, &format!("{label} lock"))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("unable to open {label} lock: {error}"))?;
    set_private_permissions(&lock, &lock_path, &format!("{label} lock"))?;
    acquire_lock(&lock, label)?;
    recover_and_cleanup_orphaned_siblings(destination, label)?;
    let result = action();
    drop(lock);
    sync_parent(&parent);
    result
}

fn acquire_lock(file: &File, label: &str) -> Result<(), String> {
    let deadline = Instant::now() + STATE_LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "timed out waiting for another XUVA process to finish writing {label}"
                    ));
                }
                thread::sleep(STATE_LOCK_RETRY);
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(format!("unable to lock {label}: {error}"));
            }
        }
    }
}

fn prepare_destination(destination: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("unable to determine {label} directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("unable to create {label} directory: {error}"))?;
    reject_unsafe_destination(destination, label)?;
    Ok(parent.to_owned())
}

fn write_bytes_atomic_unlocked(
    destination: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<(), String> {
    let parent = prepare_destination(destination, label)?;
    let temporary = unique_sibling(destination, "pending");
    let mut pending = PendingFile::create(&temporary, label)?;
    let file = pending
        .file
        .as_mut()
        .expect("pending state file remains open");
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("unable to write {label}: {error}"))?;
    drop(pending.file.take());

    replace_file(&temporary, destination, label)?;
    pending.keep = true;
    sync_parent(&parent);
    Ok(())
}

fn reject_unsafe_destination(destination: &Path, label: &str) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(destination) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("refusing to replace symlinked {label}"));
    }
    if !metadata.is_file() {
        return Err(format!("refusing to replace non-file {label}"));
    }
    Ok(())
}

fn sibling_with_suffix(destination: &Path, suffix: &str) -> PathBuf {
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    destination.with_file_name(format!(".{name}.{suffix}"))
}

fn unique_sibling(destination: &Path, role: &str) -> PathBuf {
    let sequence = STATE_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    destination.with_file_name(format!(".{name}.{}.{sequence}.{role}", std::process::id()))
}

fn recover_and_cleanup_orphaned_siblings(destination: &Path, label: &str) -> Result<(), String> {
    let Some(parent) = destination.parent() else {
        return Ok(());
    };
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let prefix = format!(".{name}.");
    let Ok(entries) = fs::read_dir(parent) else {
        return Ok(());
    };
    let mut pending = Vec::new();
    let mut replaced = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let candidate = file_name.to_string_lossy();
        if candidate.starts_with(&prefix) && candidate.ends_with(".pending") {
            pending.push(entry.path());
        } else if candidate.starts_with(&prefix) && candidate.ends_with(".replaced") {
            replaced.push(entry.path());
        }
    }
    if !destination.exists() && !replaced.is_empty() {
        replaced.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        let recovery = replaced.pop().expect("replacement list is non-empty");
        fs::rename(&recovery, destination)
            .map_err(|error| format!("unable to recover interrupted {label}: {error}"))?;
    }
    for orphan in pending.into_iter().chain(replaced) {
        let _ = fs::remove_file(orphan);
    }
    Ok(())
}

struct PendingFile {
    file: Option<File>,
    path: PathBuf,
    keep: bool,
}

impl PendingFile {
    fn create(path: &Path, label: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|error| format!("unable to create pending {label}: {error}"))?;
        set_private_permissions(&file, path, label)?;
        Ok(Self {
            file: Some(file),
            path: path.to_owned(),
            keep: false,
        })
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn replace_file(temporary: &Path, destination: &Path, label: &str) -> Result<(), String> {
    fs::rename(temporary, destination)
        .map_err(|error| format!("unable to activate {label}: {error}"))
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path, label: &str) -> Result<(), String> {
    windows::replace_file(temporary, destination)
        .map_err(|error| format!("unable to activate {label}: {error}"))
}

#[cfg(unix)]
fn set_private_permissions(file: &File, _path: &Path, label: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("unable to secure {label}: {error}"))
}

#[cfg(unix)]
fn set_private_path_permissions(path: &Path, directory: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
}

#[cfg(windows)]
fn set_private_permissions(_file: &File, path: &Path, label: &str) -> Result<(), String> {
    windows::set_private_permissions(path, false)
        .map_err(|error| format!("unable to secure {label}: {error}"))
}

#[cfg(windows)]
fn set_private_path_permissions(path: &Path, directory: bool) -> std::io::Result<()> {
    windows::set_private_permissions(path, directory)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(windows)]
fn sync_parent(_parent: &Path) {}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const SDDL_REVISION_1: u32 = 1;
    const TOKEN_QUERY: u32 = 0x0000_0008;
    const TOKEN_USER_CLASS: u32 = 1;
    const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;

    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetVolumePathNameW(
            file_name: *const u16,
            volume_path: *mut u16,
            buffer_length: u32,
        ) -> i32;
        fn GetVolumeInformationW(
            root_path: *const u16,
            volume_name: *mut u16,
            volume_name_size: u32,
            serial_number: *mut u32,
            maximum_component_length: *mut u32,
            file_system_flags: *mut u32,
            file_system_name: *mut u16,
            file_system_name_size: u32,
        ) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor: *const u16,
            revision: u32,
            converted: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *mut c_void) -> i32;
        fn OpenProcessToken(
            process: *mut c_void,
            desired_access: u32,
            token: *mut *mut c_void,
        ) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            information_class: u32,
            information: *mut c_void,
            information_length: u32,
            return_length: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, string_sid: *mut *mut u16) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
        let temporary_wide = wide(temporary);
        let destination_wide = wide(destination);
        if destination.exists() {
            let backup = super::unique_sibling(destination, "replaced");
            let backup_wide = wide(&backup);
            // SAFETY: all pointers reference live, null-terminated UTF-16 buffers for the call.
            let replaced = unsafe {
                ReplaceFileW(
                    destination_wide.as_ptr(),
                    temporary_wide.as_ptr(),
                    backup_wide.as_ptr(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if replaced == 0 {
                if !destination.exists() && backup.exists() {
                    let _ = fs_rename_without_replacement(&backup, destination);
                }
                return Err(io::Error::last_os_error());
            }
            let _ = std::fs::remove_file(backup);
            return Ok(());
        }

        // SAFETY: both pointers reference live, null-terminated UTF-16 buffers for the call.
        let moved = unsafe {
            MoveFileExW(
                temporary_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn fs_rename_without_replacement(source: &Path, destination: &Path) -> io::Result<()> {
        let source_wide = wide(source);
        let destination_wide = wide(destination);
        // SAFETY: both pointers reference live, null-terminated UTF-16 buffers for the call.
        let moved = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), 0) };
        if moved == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn set_private_permissions(path: &Path, directory: bool) -> io::Result<()> {
        if !volume_supports_persistent_acls(path)? {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the state volume does not support persistent Windows ACLs",
            ));
        }
        let user_sid = current_user_sid_string()?;
        let inheritance = if directory { "OICI" } else { "" };
        let descriptor_text: Vec<u16> = format!(
            "D:P(A;{inheritance};FA;;;{user_sid})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)\0"
        )
        .encode_utf16()
        .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: the input is a valid null-terminated SDDL string and output points to storage.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor_text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }

        let path_wide = wide(path);
        // SAFETY: path is null-terminated and descriptor was allocated by the conversion API.
        let secured = unsafe {
            SetFileSecurityW(
                path_wide.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        };
        // SAFETY: descriptor was allocated by LocalAlloc inside the conversion API.
        unsafe {
            LocalFree(descriptor);
        }
        if secured == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn volume_supports_persistent_acls(path: &Path) -> io::Result<bool> {
        let path_wide = wide(path);
        let mut volume_path = vec![0_u16; 32_768];
        // SAFETY: input and output buffers are live and sized as declared for the call.
        let resolved = unsafe {
            GetVolumePathNameW(
                path_wide.as_ptr(),
                volume_path.as_mut_ptr(),
                volume_path.len() as u32,
            )
        };
        if resolved == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut flags = 0_u32;
        // SAFETY: volume_path is null-terminated by GetVolumePathNameW; unused outputs are null.
        let inspected = unsafe {
            GetVolumeInformationW(
                volume_path.as_ptr(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut flags,
                ptr::null_mut(),
                0,
            )
        };
        if inspected == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(flags & FILE_PERSISTENT_ACLS != 0)
        }
    }

    fn current_user_sid_string() -> io::Result<String> {
        let mut token = ptr::null_mut();
        // SAFETY: pseudo process handle is always valid and token points to writable storage.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if opened == 0 {
            return Err(io::Error::last_os_error());
        }

        let result = (|| {
            let mut required = 0_u32;
            // SAFETY: a zero-sized probe with a null buffer is the documented sizing operation.
            unsafe {
                GetTokenInformation(token, TOKEN_USER_CLASS, ptr::null_mut(), 0, &mut required);
            }
            if required == 0 {
                return Err(io::Error::last_os_error());
            }
            let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
            let mut buffer = vec![0_usize; words];
            // SAFETY: the aligned buffer is at least `required` bytes and remains live for SID use.
            let loaded = unsafe {
                GetTokenInformation(
                    token,
                    TOKEN_USER_CLASS,
                    buffer.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            };
            if loaded == 0 {
                return Err(io::Error::last_os_error());
            }
            let token_user = buffer.as_ptr().cast::<SidAndAttributes>();
            // SAFETY: a successful TokenUser query starts with a valid SID_AND_ATTRIBUTES value.
            let sid = unsafe { (*token_user).sid };
            let mut sid_text = ptr::null_mut();
            // SAFETY: sid points inside the live token information buffer; output is writable.
            let converted = unsafe { ConvertSidToStringSidW(sid, &mut sid_text) };
            if converted == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut length = 0;
            // SAFETY: ConvertSidToStringSidW returns a valid null-terminated UTF-16 string.
            unsafe {
                while *sid_text.add(length) != 0 {
                    length += 1;
                }
            }
            // SAFETY: the measured range belongs to the allocated SID string.
            let text = unsafe { std::slice::from_raw_parts(sid_text, length) };
            let decoded = String::from_utf16(text)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid user SID"));
            // SAFETY: sid_text was allocated by LocalAlloc inside ConvertSidToStringSidW.
            unsafe {
                LocalFree(sid_text.cast());
            }
            decoded
        })();

        // SAFETY: token is a real handle returned by OpenProcessToken.
        unsafe {
            CloseHandle(token);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{sibling_with_suffix, update_json_atomic, write_json_atomic};
    use std::fs;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "xuva-state-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is valid")
                .as_nanos()
        ))
    }

    #[test]
    fn atomic_json_state_replaces_complete_documents() {
        let directory = fixture_directory();
        let target = directory.join("state.json");
        write_json_atomic(&target, &vec!["first"], "test state").expect("first write");
        write_json_atomic(&target, &vec!["second"], "test state").expect("replacement");
        let value: Vec<String> = serde_json::from_slice(&fs::read(&target).expect("state exists"))
            .expect("complete JSON");
        assert_eq!(value, ["second"]);
        fs::remove_dir_all(directory).expect("fixture cleanup");
    }

    #[test]
    fn locked_updates_do_not_lose_concurrent_mutations() {
        let directory = fixture_directory();
        let target = directory.join("counter.json");
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let target = target.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..20 {
                        update_json_atomic(
                            &target,
                            "test counter",
                            |path| {
                                if path.exists() {
                                    serde_json::from_slice(
                                        &fs::read(path).map_err(|e| e.to_string())?,
                                    )
                                    .map_err(|e| e.to_string())
                                } else {
                                    Ok(0_u64)
                                }
                            },
                            |value| {
                                *value += 1;
                                Ok(())
                            },
                            |_| Ok(()),
                        )
                        .expect("locked increment");
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker completed");
        }
        let value: u64 = serde_json::from_slice(&fs::read(&target).expect("counter exists"))
            .expect("counter JSON");
        assert_eq!(value, 160);
        fs::remove_dir_all(directory).expect("fixture cleanup");
    }

    #[test]
    fn next_locked_write_removes_crash_orphans() {
        let directory = fixture_directory();
        fs::create_dir_all(&directory).expect("fixture directory");
        let target = directory.join("state.json");
        let pending = directory.join(".state.json.999.1.pending");
        let replaced = directory.join(".state.json.999.2.replaced");
        fs::write(&pending, b"pending").expect("pending orphan");
        fs::write(&replaced, b"41").expect("replacement orphan");
        update_json_atomic(
            &target,
            "test state",
            |path| {
                serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            },
            |value: &mut u64| {
                *value += 1;
                Ok(())
            },
            |_| Ok(()),
        )
        .expect("recovered state update");
        assert!(!pending.exists());
        assert!(!replaced.exists());
        assert!(sibling_with_suffix(&target, "lock").exists());
        let value: u64 =
            serde_json::from_slice(&fs::read(&target).expect("state exists")).expect("state JSON");
        assert_eq!(value, 42);
        fs::remove_dir_all(directory).expect("fixture cleanup");
    }

    #[test]
    fn process_update_worker() {
        let Some(target) = std::env::var_os("XUVA_STATE_PROCESS_TEST_PATH") else {
            return;
        };
        let iterations = std::env::var("XUVA_STATE_PROCESS_TEST_ITERATIONS")
            .expect("iteration count")
            .parse::<u64>()
            .expect("numeric iteration count");
        for _ in 0..iterations {
            update_json_atomic(
                std::path::Path::new(&target),
                "cross-process test counter",
                |path| {
                    if path.exists() {
                        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())
                    } else {
                        Ok(0_u64)
                    }
                },
                |value| {
                    *value += 1;
                    Ok(())
                },
                |_| Ok(()),
            )
            .expect("cross-process increment");
        }
    }

    #[test]
    fn locked_updates_do_not_lose_cross_process_mutations() {
        let directory = fixture_directory();
        let target = directory.join("process-counter.json");
        let test_binary = std::env::current_exe().expect("test binary");
        let mut children = Vec::new();
        for _ in 0..4 {
            children.push(
                Command::new(&test_binary)
                    .arg("--exact")
                    .arg("state::tests::process_update_worker")
                    .arg("--test-threads=1")
                    .env("XUVA_STATE_PROCESS_TEST_PATH", &target)
                    .env("XUVA_STATE_PROCESS_TEST_ITERATIONS", "20")
                    .spawn()
                    .expect("worker process"),
            );
        }
        for mut child in children {
            assert!(child.wait().expect("worker status").success());
        }
        let value: u64 = serde_json::from_slice(&fs::read(&target).expect("counter exists"))
            .expect("counter JSON");
        assert_eq!(value, 80);
        fs::remove_dir_all(directory).expect("fixture cleanup");
    }
}
