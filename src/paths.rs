//! Cross-environment path validation and mapping helpers.

/// Converts a drive-qualified Windows path into its standard WSL mount path.
/// UNC paths intentionally return `None`: their compatibility is determined by
/// the provider/project mapper rather than guessed here.
pub(crate) fn windows_path_to_wsl_path(path: &str) -> Option<String> {
    let replaced = path.replace('\\', "/");
    let normalized = replaced.strip_prefix("//?/").unwrap_or(&replaced);
    let bytes = normalized.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || bytes[2] != b'/' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    Some(format!(
        "/mnt/{}/{}",
        (bytes[0] as char).to_ascii_lowercase(),
        &normalized[3..]
    ))
}
