//! Cross-environment path validation and mapping helpers.

use std::ffi::OsString;
use std::path::Path;

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

/// Converts a standard WSL drive mount path back into a drive-qualified
/// Windows path. Other Linux paths deliberately remain unmapped.
pub(crate) fn wsl_path_to_windows_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 7
        || !path.starts_with("/mnt/")
        || !bytes[5].is_ascii_alphabetic()
        || bytes[6] != b'/'
    {
        return None;
    }
    let remainder = path[7..].replace('/', "\\");
    Some(format!(
        "{}:\\{}",
        (bytes[5] as char).to_ascii_uppercase(),
        remainder
    ))
}

#[cfg(test)]
fn translate_path(value: &str, windows: bool) -> Option<String> {
    if windows {
        wsl_path_to_windows_path(value)
    } else {
        windows_path_to_wsl_path(value)
    }
}

fn translated_path_is_concrete(original: &str, translated: &str) -> bool {
    let absolute_contract = Path::new(original).is_absolute()
        || original.starts_with('/')
        || original.starts_with(r"\\");
    absolute_contract && (Path::new(original).exists() || Path::new(translated).exists())
}

fn flag_takes_path(tool: &str, flag: &str) -> bool {
    match tool {
        "git" => matches!(flag, "-C" | "--git-dir" | "--work-tree"),
        "go" => matches!(flag, "-C" | "-modfile" | "-overlay" | "-o"),
        "cargo" | "rustc" => matches!(
            flag,
            "--manifest-path" | "--target-dir" | "--out-dir" | "-o"
        ),
        "rg" | "fd" => matches!(flag, "--ignore-file"),
        _ => false,
    }
}

fn embedded_path_prefix(tool: &str, argument: &str) -> Option<&'static str> {
    let prefixes: &[&str] = match tool {
        "git" => &["--git-dir=", "--work-tree="],
        "go" => &["-modfile=", "-overlay=", "-o="],
        "cargo" | "rustc" => &["--manifest-path=", "--target-dir=", "--out-dir="],
        "rg" | "fd" => &["--ignore-file="],
        _ => &[],
    };
    prefixes
        .iter()
        .copied()
        .find(|prefix| argument.starts_with(prefix))
}

/// Translates only argv positions whose command contract identifies them as
/// paths. Generic arguments are left untouched unless an exact standalone path
/// exists on either side of the mapping.
#[cfg(test)]
pub(crate) fn translate_arguments_for_provider(
    tool: &str,
    arguments: &[OsString],
    windows: bool,
) -> Vec<OsString> {
    translate_arguments_with(tool, arguments, |value| translate_path(value, windows))
}

pub(crate) fn translate_arguments_with(
    tool: &str,
    arguments: &[OsString],
    mut translate: impl FnMut(&str) -> Option<String>,
) -> Vec<OsString> {
    let mut translated = Vec::with_capacity(arguments.len());
    let mut previous_path_flag = false;
    let mut git_pathspecs = false;
    for argument in arguments {
        let Some(value) = argument.to_str() else {
            translated.push(argument.clone());
            previous_path_flag = false;
            continue;
        };
        if tool == "git" && value == "--" {
            git_pathspecs = true;
            translated.push(argument.clone());
            previous_path_flag = false;
            continue;
        }
        let contracted_path =
            previous_path_flag || git_pathspecs || (tool == "read" && !value.starts_with('-'));
        if contracted_path {
            translated.push(
                translate(value)
                    .map(OsString::from)
                    .unwrap_or_else(|| argument.clone()),
            );
            previous_path_flag = false;
            continue;
        }
        if let Some(prefix) = embedded_path_prefix(tool, value) {
            let path = &value[prefix.len()..];
            translated.push(
                translate(path)
                    .map(|path| OsString::from(format!("{prefix}{path}")))
                    .unwrap_or_else(|| argument.clone()),
            );
            previous_path_flag = false;
            continue;
        }
        if let Some(path) = value.strip_prefix('@')
            && matches!(tool, "cargo" | "rustc" | "go")
            && let Some(mapped) = translate(path)
        {
            translated.push(OsString::from(format!("@{mapped}")));
            previous_path_flag = false;
            continue;
        }
        if let Some(mapped) = translate(value)
            && translated_path_is_concrete(value, &mapped)
        {
            translated.push(OsString::from(mapped));
        } else {
            translated.push(argument.clone());
        }
        previous_path_flag = flag_takes_path(tool, value);
    }
    translated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_only_typed_or_concrete_path_arguments() {
        let translated = translate_arguments_for_provider(
            "cargo",
            &[
                OsString::from("--manifest-path=C:\\work\\Cargo.toml"),
                OsString::from("C:\\literal\\that\\does-not-exist"),
                OsString::from("@C:\\work\\args.rsp"),
            ],
            false,
        );
        assert_eq!(
            translated[0],
            OsString::from("--manifest-path=/mnt/c/work/Cargo.toml")
        );
        assert_eq!(
            translated[1],
            OsString::from("C:\\literal\\that\\does-not-exist")
        );
        assert_eq!(translated[2], OsString::from("@/mnt/c/work/args.rsp"));
    }

    #[test]
    fn typed_argument_translation_uses_the_provider_mount_mapping() {
        let translated = translate_arguments_with(
            "cargo",
            &[
                OsString::from("--manifest-path=/windows/c/work/Cargo.toml"),
                OsString::from("@/windows/c/work/args.rsp"),
                OsString::from("/windows/c/not-a-typed-value"),
            ],
            |value| {
                value
                    .strip_prefix("/windows/c/")
                    .map(|suffix| format!(r"C:\{}", suffix.replace('/', r"\")))
            },
        );
        assert_eq!(
            translated,
            [
                OsString::from(r"--manifest-path=C:\work\Cargo.toml"),
                OsString::from(r"@C:\work\args.rsp"),
                OsString::from("/windows/c/not-a-typed-value"),
            ]
        );
    }

    #[test]
    fn translates_git_pathspecs_but_not_revision_like_data() {
        let translated = translate_arguments_for_provider(
            "git",
            &[
                OsString::from("show"),
                OsString::from("C:\\looks\\like\\data"),
                OsString::from("--"),
                OsString::from("C:\\work\\file.txt"),
            ],
            false,
        );
        assert_eq!(translated[1], OsString::from("C:\\looks\\like\\data"));
        assert_eq!(translated[3], OsString::from("/mnt/c/work/file.txt"));
    }
}
