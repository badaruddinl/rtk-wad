use std::env;
use std::ffi::OsString;
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::thread;
use std::time::Duration;

const DEFAULT_DISTRO: &str = "Ubuntu";
const DEFAULT_WSL1_DISTRO: &str = "Ubuntu-RTK-WSL1";
const DEFAULT_LOCK_PATH: &str = "/tmp/rtk-wsl.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const DEFAULT_GIT_MODE: &str = "auto";
const BRIDGE_INFO_ARGUMENT: &str = "--bridge-info";
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
trap "cleanup; exit 130" INT TERM
trap cleanup EXIT
printf '%s' "$$" > "$cancel_token"
exec 9>"$lock_path"
remaining=$((lock_wait * 10))
while ! /usr/bin/flock -n 9; do
    if [ "$remaining" -le 0 ]; then
        printf 'rtk-wsl: timed out waiting for lock %s\n' "$lock_path" >&2
        exit 1
    fi
    remaining=$((remaining - 1))
    /bin/sleep 0.1
done
/usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    "$rtk_path" "$@"
status=$?
exit "$status"
"#;
const WSL1_LAUNCH_SCRIPT: &str = r#"
rtk_path=$1
shift

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-}
exec /usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    "$rtk_path" "$@"
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitMode {
    Auto,
    Wsl,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WslBackend {
    Auto,
    Wsl1,
    Wsl2,
}

impl WslBackend {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wsl1 => "wsl1",
            Self::Wsl2 => "wsl2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    backend: WslBackend,
    distro: String,
    user: Option<String>,
    rtk_path: Option<String>,
    lock_path: String,
    lock_wait: String,
    cwd: Option<String>,
    git_mode: GitMode,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let executable = env::current_exe().ok().and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        Self::from_lookup_with_executable(|name| env::var(name).ok(), executable.as_deref())
    }

    #[cfg(test)]
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        Self::from_lookup_with_executable(lookup, None)
    }

    fn from_lookup_with_executable(
        lookup: impl Fn(&str) -> Option<String>,
        executable: Option<&str>,
    ) -> Result<Self, String> {
        let executable_backend = if executable.is_some_and(|name| {
            name.eq_ignore_ascii_case("rtk-wsl1") || name.eq_ignore_ascii_case("rtk-wsl1.exe")
        }) {
            WslBackend::Wsl1
        } else {
            WslBackend::Auto
        };
        let backend = match lookup("RTK_WSL_BACKEND")
            .unwrap_or_else(|| executable_backend.as_str().to_owned())
            .as_str()
        {
            "auto" => WslBackend::Auto,
            "wsl1" => WslBackend::Wsl1,
            "wsl2" => WslBackend::Wsl2,
            _ => return Err("RTK_WSL_BACKEND must be auto, wsl1, or wsl2".to_owned()),
        };
        let default_distro = match backend {
            WslBackend::Wsl1 => DEFAULT_WSL1_DISTRO,
            WslBackend::Auto | WslBackend::Wsl2 => DEFAULT_DISTRO,
        };
        let distro = required_setting(&lookup, "RTK_WSL_DISTRO", default_distro)?;
        let user = optional_setting(&lookup, "RTK_WSL_USER")?;
        let rtk_path = optional_absolute_path(&lookup, "RTK_WSL_RTK_PATH")?;
        let lock_path = required_absolute_path(&lookup, "RTK_WSL_LOCK_PATH", DEFAULT_LOCK_PATH)?;
        let lock_wait = required_setting(
            &lookup,
            "RTK_WSL_LOCK_WAIT_SECONDS",
            DEFAULT_LOCK_WAIT_SECONDS,
        )?;
        let cwd = optional_absolute_path(&lookup, "RTK_WSL_CWD")?;
        let git_mode =
            match required_setting(&lookup, "RTK_WSL_GIT_MODE", DEFAULT_GIT_MODE)?.as_str() {
                "auto" => GitMode::Auto,
                "wsl" => GitMode::Wsl,
                "native" => GitMode::Native,
                _ => return Err("RTK_WSL_GIT_MODE must be auto, wsl, or native".to_owned()),
            };

        if lock_wait
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
        {
            return Err("RTK_WSL_LOCK_WAIT_SECONDS must be a positive integer".to_owned());
        }

        Ok(Self {
            backend,
            distro,
            user,
            rtk_path,
            lock_path,
            lock_wait,
            cwd,
            git_mode,
        })
    }
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.chunks_exact(2).any(|pair| pair[1] == 0) {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

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

fn bridge_info(config: &Config) -> ExitCode {
    let output = match Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "rtk-wsl: unable to inspect WSL distributions: {}",
                decode_wsl_output(&output.stderr).trim()
            );
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("rtk-wsl: unable to start wsl.exe for bridge diagnostics: {error}");
            return ExitCode::FAILURE;
        }
    };
    let list = decode_wsl_output(&output.stdout);
    let version = distro_version_from_list(&list, &config.distro);
    println!("bridge=rtk-wsl");
    println!("backend={}", config.backend.as_str());
    println!("distro={}", config.distro);
    println!(
        "detected_wsl_version={}",
        version.map_or_else(|| "missing".to_owned(), |value| value.to_string())
    );
    println!(
        "git_mode={}",
        match config.git_mode {
            GitMode::Auto => "auto",
            GitMode::Wsl => "wsl",
            GitMode::Native => "native",
        }
    );

    let expected = match config.backend {
        WslBackend::Auto => return version.map_or(ExitCode::FAILURE, |_| ExitCode::SUCCESS),
        WslBackend::Wsl1 => 1,
        WslBackend::Wsl2 => 2,
    };
    match version {
        Some(actual) if actual == expected => ExitCode::SUCCESS,
        Some(actual) => {
            eprintln!(
                "rtk-wsl: configured {} backend requires WSL {}, but {} is WSL {}",
                config.backend.as_str(),
                expected,
                config.distro,
                actual
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!(
                "rtk-wsl: configured distro {} is not registered",
                config.distro
            );
            ExitCode::FAILURE
        }
    }
}

fn trace(message: impl AsRef<str>) {
    if env::var("RTK_WSL_TRACE").as_deref() == Ok("1") {
        eprintln!("rtk-wsl: trace: {}", message.as_ref());
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

fn is_wsl_path(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

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

fn rtk_arguments(arguments: Vec<OsString>, config: &Config, cancel_token: &str) -> Vec<OsString> {
    let forwarded = forwarded_rtk_arguments(arguments);
    let mut command = wsl_launch_prefix(config);
    command.extend([
        OsString::from("--exec"),
        OsString::from("/usr/bin/setsid"),
        OsString::from("-w"),
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

fn wsl1_rtk_arguments(arguments: Vec<OsString>, config: &Config) -> Vec<OsString> {
    let forwarded = forwarded_rtk_arguments(arguments);
    let mut command = wsl_launch_prefix(config);
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(WSL1_LAUNCH_SCRIPT),
        OsString::from("rtk-wsl1"),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
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

#[cfg(target_os = "windows")]
mod windows_lock {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x0000_0080;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const MUTEX_NAME: &str = r"Local\rtk-wsl-wsl1-global-lock";

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
                WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Guard { handle }),
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

fn request_linux_interrupt(config: &Config, token: &str) {
    let _ = Command::new("wsl.exe")
        .args(cancel_arguments(config, token))
        .status();
}

fn terminate_dedicated_wsl1_distro(config: &Config) {
    trace(format!(
        "terminating dedicated WSL1 distro {} after cancellation",
        config.distro
    ));
    match Command::new("wsl.exe")
        .args(["--terminate", &config.distro])
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => trace(format!(
            "WSL1 terminate returned {}: {}{}",
            output.status,
            decode_wsl_output(&output.stdout).trim(),
            decode_wsl_output(&output.stderr).trim()
        )),
        Err(error) => trace(format!("unable to start WSL1 terminate command: {error}")),
    }
}

fn wait_for_wsl_child(
    mut child: Child,
    config: &Config,
    token: &str,
) -> std::io::Result<ExitStatus> {
    let mut interrupted = false;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if console::requested() && !interrupted {
            request_linux_interrupt(config, token);
            interrupted = true;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_wsl1_child(mut child: Child, config: &Config) -> std::io::Result<ExitStatus> {
    let mut interrupted = false;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if console::requested() && !interrupted {
            let _ = child.kill();
            terminate_dedicated_wsl1_distro(config);
            interrupted = true;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wsl1_process(arguments: Vec<OsString>) -> Command {
    let mut command = Command::new("wsl.exe");
    command.args(arguments);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    command
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("rtk-wsl: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    if arguments.len() == 1 && arguments[0] == BRIDGE_INFO_ARGUMENT {
        return bridge_info(&config);
    }
    let current_directory = env::current_dir().ok();
    let use_native_git = should_use_native_git(
        &arguments,
        &config,
        current_directory.as_deref().and_then(|path| path.to_str()),
    );
    let use_native_wsl1_bridge = !use_native_git && config.backend == WslBackend::Wsl1;
    if !use_native_git && !console::install() {
        eprintln!("rtk-wsl: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let _wsl1_lock = if use_native_wsl1_bridge {
        trace("waiting for the Windows WSL1 mutex");
        match windows_lock::acquire(&config.lock_wait) {
            Ok(guard) => {
                trace("acquired the Windows WSL1 mutex");
                Some(guard)
            }
            Err(error) => {
                eprintln!("rtk-wsl: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let token = cancel_token();
    let result = if use_native_git {
        Command::new("git.exe")
            .args(arguments.iter().skip(1))
            .spawn()
            .and_then(|mut child| child.wait())
    } else if use_native_wsl1_bridge {
        wsl1_process(wsl1_rtk_arguments(arguments, &config))
            .spawn()
            .and_then(|child| {
                trace(format!("started WSL1 wsl.exe process {}", child.id()));
                let status = wait_for_wsl1_child(child, &config);
                trace("WSL1 wsl.exe process exited");
                status
            })
    } else {
        Command::new("wsl.exe")
            .args(rtk_arguments(arguments, &config, &token))
            .spawn()
            .and_then(|child| wait_for_wsl_child(child, &config, &token))
    };
    if !use_native_git {
        console::uninstall();
    }
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
    fn wsl1_launch_uses_the_windows_mutex_without_redundant_linux_locking() {
        let config = Config::from_lookup_with_executable(|_| None, Some("rtk-wsl1.exe")).unwrap();
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
                .any(|value| value.contains("exec /usr/bin/env"))
        );
        assert!(!strings.iter().any(|value| value.contains("/usr/bin/flock")));
        assert!(!strings.iter().any(|value| value == "/usr/bin/setsid"));
        assert_eq!(
            strings.last().map(|value| value.as_ref()),
            Some("space & $HOME")
        );
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
            "RTK_WSL_GIT_MODE" => Some("wsl".to_owned()),
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
            "RTK_WSL_GIT_MODE" => Some("other".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn wsl1_alias_selects_the_isolated_distro_without_affecting_the_default_bridge() {
        let default = default_config();
        assert_eq!(default.backend, WslBackend::Auto);
        assert_eq!(default.distro, DEFAULT_DISTRO);

        let wsl1 = Config::from_lookup_with_executable(|_| None, Some("rtk-wsl1.exe"))
            .expect("WSL1 alias configuration is valid");
        assert_eq!(wsl1.backend, WslBackend::Wsl1);
        assert_eq!(wsl1.distro, DEFAULT_WSL1_DISTRO);
    }

    #[test]
    fn explicit_backend_and_distro_override_alias_defaults() {
        let config = Config::from_lookup_with_executable(
            |name| match name {
                "RTK_WSL_BACKEND" => Some("wsl2".to_owned()),
                "RTK_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
                _ => None,
            },
            Some("rtk-wsl1.exe"),
        )
        .expect("explicit backend configuration is valid");
        assert_eq!(config.backend, WslBackend::Wsl2);
        assert_eq!(config.distro, "Ubuntu-24.04");

        let invalid = Config::from_lookup(|name| match name {
            "RTK_WSL_BACKEND" => Some("legacy".to_owned()),
            _ => None,
        });
        assert!(invalid.is_err());
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
}
