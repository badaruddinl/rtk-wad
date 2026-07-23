use std::env;
use std::ffi::OsString;
use std::process::{Command, ExitCode};

const DEFAULT_DISTRO: &str = "Ubuntu";
const DEFAULT_LOCK_PATH: &str = "/tmp/rtk-wsl.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const LAUNCH_SCRIPT: &str = r#"
lock_wait=$1
lock_path=$2
rtk_path=$3
shift 3

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-$(id -un)}
exec /usr/bin/flock -w "$lock_wait" "$lock_path" /usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    "$rtk_path" "$@"
"#;

fn setting(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn windows_path_to_wsl_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
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

fn rtk_arguments(arguments: Vec<OsString>) -> Vec<OsString> {
    let distro = setting("RTK_WSL_DISTRO", DEFAULT_DISTRO);
    let user = env::var("RTK_WSL_USER").ok();
    let rtk_path = env::var("RTK_WSL_RTK_PATH").unwrap_or_default();
    let lock_path = setting("RTK_WSL_LOCK_PATH", DEFAULT_LOCK_PATH);
    let lock_wait = setting("RTK_WSL_LOCK_WAIT_SECONDS", DEFAULT_LOCK_WAIT_SECONDS);
    let mut forwarded = arguments;

    if forwarded
        .first()
        .is_some_and(|argument| argument == "stats")
    {
        forwarded[0] = OsString::from("gain");
    }

    let mut command = vec![OsString::from("-d"), OsString::from(distro)];
    if let Some(user) = user {
        command.extend([OsString::from("-u"), OsString::from(user)]);
    }
    if let Ok(current_directory) = env::current_dir()
        && let Some(wsl_directory) = windows_path_to_wsl_path(&current_directory.to_string_lossy())
    {
        command.extend([OsString::from("--cd"), OsString::from(wsl_directory)]);
    }
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(LAUNCH_SCRIPT),
        OsString::from("rtk-wsl"),
        OsString::from(lock_wait),
        OsString::from(lock_path),
        OsString::from(rtk_path),
    ]);
    command.extend(forwarded);
    command
}

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect();
    match Command::new("wsl.exe")
        .args(rtk_arguments(arguments))
        .status()
    {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("rtk-wsl: unable to start wsl.exe: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_special_characters_as_distinct_arguments() {
        let arguments = rtk_arguments(vec![
            OsString::from("run"),
            OsString::from("semi;and&dollar$HOME"),
            OsString::from("C:\\Program Files\\Example"),
        ]);

        assert!(arguments.contains(&OsString::from("--exec")));
        assert!(arguments.contains(&OsString::from(LAUNCH_SCRIPT)));
        assert!(arguments.contains(&OsString::from("semi;and&dollar$HOME")));
        assert!(arguments.contains(&OsString::from("C:\\Program Files\\Example")));
    }

    #[test]
    fn stats_remains_a_compatibility_alias() {
        let arguments = rtk_arguments(vec![OsString::from("stats")]);
        assert_eq!(arguments.last(), Some(&OsString::from("gain")));
    }

    #[test]
    fn maps_windows_drive_paths_for_wsl_current_directory() {
        assert_eq!(
            windows_path_to_wsl_path(r"D:\projects\rtk-wsl"),
            Some("/mnt/d/projects/rtk-wsl".to_owned())
        );
        assert_eq!(windows_path_to_wsl_path(r"\\server\share"), None);
    }

    #[test]
    fn defaults_to_the_selected_wsl_users_home() {
        let arguments = rtk_arguments(vec![OsString::from("help")]);

        assert!(arguments.contains(&OsString::from("")));
        assert!(arguments.iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("rtk_path=\"$HOME/.local/bin/rtk\"")
        }));
        assert!(!arguments.contains(&OsString::from("-u")));
    }
}
