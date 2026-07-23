use std::env;
use std::ffi::OsString;
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::thread;
use std::time::Duration;

const DEFAULT_DISTRO: &str = "Ubuntu";
const DEFAULT_LOCK_PATH: &str = "/tmp/rtk-wsl.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const CANCEL_SCRIPT: &str = r#"
if [ -r "$1" ]; then
    worker=$(cat "$1")
    case "$worker" in
        *[!0-9]*|'') exit 1 ;;
    esac
    kill -INT -- "-$worker"
fi
"#;
const LAUNCH_SCRIPT: &str = r#"
lock_wait=$1
lock_path=$2
rtk_path=$3
cancel_token=$4
shift 4

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-}
cleanup() { rm -f "$cancel_token"; }
trap cleanup EXIT
/usr/bin/setsid /usr/bin/flock -w "$lock_wait" "$lock_path" /usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    "$rtk_path" "$@" &
worker=$!
printf '%s' "$worker" > "$cancel_token"
wait "$worker"
exit $?
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    distro: String,
    user: Option<String>,
    rtk_path: Option<String>,
    lock_path: String,
    lock_wait: String,
    cwd: Option<String>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let distro = required_setting(&lookup, "RTK_WSL_DISTRO", DEFAULT_DISTRO)?;
        let user = optional_setting(&lookup, "RTK_WSL_USER")?;
        let rtk_path = optional_absolute_path(&lookup, "RTK_WSL_RTK_PATH")?;
        let lock_path = required_absolute_path(&lookup, "RTK_WSL_LOCK_PATH", DEFAULT_LOCK_PATH)?;
        let lock_wait = required_setting(
            &lookup,
            "RTK_WSL_LOCK_WAIT_SECONDS",
            DEFAULT_LOCK_WAIT_SECONDS,
        )?;
        let cwd = optional_absolute_path(&lookup, "RTK_WSL_CWD")?;

        if lock_wait
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
        {
            return Err("RTK_WSL_LOCK_WAIT_SECONDS must be a positive integer".to_owned());
        }

        Ok(Self {
            distro,
            user,
            rtk_path,
            lock_path,
            lock_wait,
            cwd,
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

fn rtk_arguments(arguments: Vec<OsString>, config: &Config, cancel_token: &str) -> Vec<OsString> {
    let mut forwarded = arguments;

    if forwarded
        .first()
        .is_some_and(|argument| argument == "stats")
    {
        forwarded[0] = OsString::from("gain");
    }

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
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(LAUNCH_SCRIPT),
        OsString::from("rtk-wsl"),
        OsString::from(&config.lock_wait),
        OsString::from(&config.lock_path),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
        OsString::from(cancel_token),
    ]);
    command.extend(forwarded);
    command
}

fn cancel_token() -> String {
    format!("/tmp/rtk-wsl-{}.cancel", std::process::id())
}

fn cancel_arguments(config: &Config, token: &str) -> Vec<OsString> {
    let mut command = vec![OsString::from("-d"), OsString::from(&config.distro)];
    if let Some(user) = &config.user {
        command.extend([OsString::from("-u"), OsString::from(user)]);
    }
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(CANCEL_SCRIPT),
        OsString::from("rtk-wsl-cancel"),
        OsString::from(token),
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

fn request_linux_interrupt(config: &Config, token: &str) {
    let _ = Command::new("wsl.exe")
        .args(cancel_arguments(config, token))
        .status();
}

fn wait_for_child(mut child: Child, config: &Config, token: &str) -> std::io::Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if console::requested() {
            request_linux_interrupt(config, token);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("rtk-wsl: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !console::install() {
        eprintln!("rtk-wsl: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let token = cancel_token();
    let result = Command::new("wsl.exe")
        .args(rtk_arguments(arguments, &config, &token))
        .spawn()
        .and_then(|child| wait_for_child(child, &config, &token));
    console::uninstall();
    match result {
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

    fn default_config() -> Config {
        Config::from_lookup(|_| None).expect("default config is valid")
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
            "/tmp/test.cancel",
        );

        assert!(arguments.contains(&OsString::from("--exec")));
        assert!(arguments.contains(&OsString::from(LAUNCH_SCRIPT)));
        assert!(arguments.contains(&OsString::from("semi;and&dollar$HOME")));
        assert!(arguments.contains(&OsString::from("C:\\Program Files\\Example")));
    }

    #[test]
    fn stats_remains_a_compatibility_alias() {
        let arguments = rtk_arguments(
            vec![OsString::from("stats")],
            &default_config(),
            "/tmp/test.cancel",
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
        assert_eq!(windows_path_to_wsl_path(r"\\server\share"), None);
    }

    #[test]
    fn defaults_to_the_selected_wsl_users_home() {
        let arguments = rtk_arguments(
            vec![OsString::from("help")],
            &default_config(),
            "/tmp/test.cancel",
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
            "RTK_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            "RTK_WSL_USER" => Some("alex".to_owned()),
            "RTK_WSL_RTK_PATH" => Some("/opt/rtk/bin/rtk".to_owned()),
            "RTK_WSL_CWD" => Some("/work/custom-mount".to_owned()),
            _ => None,
        })
        .expect("portable config is valid");

        let arguments = rtk_arguments(vec![OsString::from("help")], &config, "/tmp/test.cancel");
        assert!(arguments.windows(2).any(|pair| pair == ["-u", "alex"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--cd", "/work/custom-mount"])
        );
        assert!(arguments.contains(&OsString::from("/opt/rtk/bin/rtk")));
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_configuration() {
        let invalid_wait = Config::from_lookup(|name| match name {
            "RTK_WSL_LOCK_WAIT_SECONDS" => Some("0".to_owned()),
            _ => None,
        });
        assert!(invalid_wait.is_err());

        let relative_path = Config::from_lookup(|name| match name {
            "RTK_WSL_RTK_PATH" => Some("bin/rtk".to_owned()),
            _ => None,
        });
        assert!(relative_path.is_err());
    }

    #[test]
    fn cancellation_uses_a_separate_structured_wsl_command() {
        let arguments = cancel_arguments(&default_config(), "/tmp/rtk-wsl-42.cancel");
        assert!(arguments.contains(&OsString::from(CANCEL_SCRIPT)));
        assert!(arguments.contains(&OsString::from("/tmp/rtk-wsl-42.cancel")));
    }
}
