//! Hardened persistence primitives for local XUVA state.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

static STATE_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_json_atomic<T: Serialize>(
    destination: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("unable to encode {label}: {error}"))?;
    write_bytes_atomic(destination, &bytes, label)
}

fn write_bytes_atomic(destination: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("unable to determine {label} directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("unable to create {label} directory: {error}"))?;
    reject_unsafe_destination(destination, label)?;

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

    if destination.exists() {
        let backup = unique_sibling(destination, "replaced");
        fs::rename(destination, &backup)
            .map_err(|error| format!("unable to prepare {label} replacement: {error}"))?;
        if let Err(error) = fs::rename(&temporary, destination) {
            let restore = fs::rename(&backup, destination);
            return Err(match restore {
                Ok(()) => format!("unable to activate {label}: {error}"),
                Err(restore_error) => format!(
                    "unable to activate {label}: {error}; previous state restore failed: {restore_error}"
                ),
            });
        }
        pending.keep = true;
        fs::remove_file(&backup)
            .map_err(|error| format!("unable to remove replaced {label}: {error}"))?;
    } else {
        fs::rename(&temporary, destination)
            .map_err(|error| format!("unable to activate {label}: {error}"))?;
        pending.keep = true;
    }

    sync_parent(parent);
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

fn unique_sibling(destination: &Path, role: &str) -> PathBuf {
    let sequence = STATE_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    destination.with_file_name(format!(".{name}.{}.{sequence}.{role}", std::process::id()))
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
        set_private_permissions(&file, label)?;
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
fn set_private_permissions(file: &File, label: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("unable to secure pending {label}: {error}"))
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &File, _label: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) {}

#[cfg(test)]
mod tests {
    use super::write_json_atomic;
    use std::fs;
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
}
