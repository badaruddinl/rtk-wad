use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const DEFAULT_DISTRO: &str = "Ubuntu";
const DEFAULT_WSL1_DISTRO: &str = "Ubuntu-RTK-WSL1";
const DEFAULT_LOCK_PATH: &str = "/tmp/rtk-wsl.lock";
const DEFAULT_LOCK_WAIT_SECONDS: &str = "120";
const DEFAULT_GIT_MODE: &str = "auto";
const BRIDGE_INFO_ARGUMENT: &str = "--bridge-info";
const ADAPTER_INFO_ARGUMENT: &str = "--adapter-info";
const EXPLAIN_ROUTE_ARGUMENT: &str = "--explain-route";
const POLICY_ARGUMENT: &str = "policy";
const CALIBRATION_ARGUMENT: &str = "calibration";
const RESOLVE_ARGUMENT: &str = "resolve";
const DOCTOR_ARGUMENT: &str = "doctor";
const PROVIDER_ARGUMENT: &str = "provider";
const SETUP_ARGUMENT: &str = "setup";
const PROVIDER_CACHE_SCHEMA_VERSION: u32 = 2;
const PROVIDER_CACHE_TTL_SECONDS: u64 = 300;
const CALIBRATION_SCHEMA_VERSION: u32 = 1;
const CALIBRATION_MAX_SAMPLES: usize = 5;
const CANCEL_SCRIPT: &str = r#"
if [ -r "$1" ]; then
    worker=$(cat "$1")
    case "$worker" in
        *[!0-9]*|'') exit 1 ;;
    esac
    /bin/kill -INT -- "-$worker"
fi
"#;
const LAUNCH_SCRIPT: &str = r#"
lock_wait=$1
lock_path=$2
rtk_path=$3
cancel_token=$4
metrics_db_path=$5
extra_path=$6
ready_file=$7
shift 7

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-}
if [ -n "$extra_path" ]; then
    path_prefix="$extra_path:"
else
    path_prefix=""
fi
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
if [ -n "$ready_file" ]; then
    printf 'ready' > "$ready_file"
fi
/usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    RTK_DB_PATH="$metrics_db_path" \
    "$rtk_path" "$@"
status=$?
exit "$status"
"#;
const WSL1_LAUNCH_SCRIPT: &str = r#"
rtk_path=$1
metrics_db_path=$2
extra_path=$3
ready_file=$4
shift 4

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-}
if [ -n "$extra_path" ]; then
    path_prefix="$extra_path:"
else
    path_prefix=""
fi
if [ -n "$ready_file" ]; then
    printf 'ready' > "$ready_file"
fi
exec /usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    RTK_DB_PATH="$metrics_db_path" \
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutableProfile {
    Legacy,
    Wsl1,
    Wad,
}

impl ExecutableProfile {
    fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "rtk-wsl",
            Self::Wsl1 => "rtk-wsl1",
            Self::Wad => "rtk-wad",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Auto,
    Raw,
    NativeRtk,
    Wsl1,
    Wsl2,
}

impl Route {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Raw => "raw",
            Self::NativeRtk => "native-rtk",
            Self::Wsl1 => "wsl1",
            Self::Wsl2 => "wsl2",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Raw),
            "native-rtk" => Ok(Self::NativeRtk),
            "wsl1" => Ok(Self::Wsl1),
            "wsl2" => Ok(Self::Wsl2),
            _ => Err("route must be auto, raw, native-rtk, wsl1, or wsl2".to_owned()),
        }
    }
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
    profile: ExecutableProfile,
    backend: WslBackend,
    distro: String,
    user: Option<String>,
    rtk_path: Option<String>,
    lock_path: String,
    lock_wait: String,
    cwd: Option<String>,
    git_mode: GitMode,
    extra_path: Option<String>,
    wad_route: Route,
    native_rtk_path: String,
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
        let profile = match executable {
            Some(name)
                if name.eq_ignore_ascii_case("rtk-wsl1")
                    || name.eq_ignore_ascii_case("rtk-wsl1.exe") =>
            {
                ExecutableProfile::Wsl1
            }
            Some(name)
                if name.eq_ignore_ascii_case("rtk-wad")
                    || name.eq_ignore_ascii_case("rtk-wad.exe") =>
            {
                ExecutableProfile::Wad
            }
            _ => ExecutableProfile::Legacy,
        };
        let executable_backend = if profile == ExecutableProfile::Wsl1 {
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
        let extra_path = optional_linux_path_list(&lookup, "RTK_WSL_EXTRA_PATH")?;
        let wad_route = match lookup("RTK_WAD_ROUTE") {
            Some(value) if value.trim().is_empty() => {
                return Err("RTK_WAD_ROUTE must not be empty when set".to_owned());
            }
            Some(value) => {
                Route::parse(&value).map_err(|error| format!("RTK_WAD_ROUTE {error}"))?
            }
            None => Route::Auto,
        };
        let native_rtk_path = lookup("RTK_WAD_NATIVE_RTK_PATH")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "rtk.exe".to_owned());

        if lock_wait
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
        {
            return Err("RTK_WSL_LOCK_WAIT_SECONDS must be a positive integer".to_owned());
        }

        Ok(Self {
            profile,
            backend,
            distro,
            user,
            rtk_path,
            lock_path,
            lock_wait,
            cwd,
            git_mode,
            extra_path,
            wad_route,
            native_rtk_path,
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

fn optional_linux_path_list(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = optional_setting(lookup, name)?;
    if let Some(value) = &value
        && value
            .split(':')
            .any(|entry| entry.is_empty() || !entry.starts_with('/'))
    {
        return Err(format!(
            "{name} must be a colon-separated list of absolute Linux paths"
        ));
    }
    Ok(value)
}

#[derive(Debug, Default, Clone, Copy)]
struct TokenTotals {
    commands: i64,
    input_tokens: i64,
    output_tokens: i64,
    saved_tokens: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RoutePolicyFile {
    schema_version: u32,
    evidence: Vec<RoutePolicyEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoutePolicyEvidence {
    key: String,
    raw_median_ms: f64,
    candidate_median_ms: f64,
    token_savings_percent: f64,
    sample_count: u32,
}

impl RoutePolicyFile {
    fn route_for(&self, key: &str) -> Option<Route> {
        let evidence = self.evidence.iter().find(|evidence| evidence.key == key)?;
        if self.schema_version != 1 || evidence.sample_count < 5 {
            return None;
        }
        if evidence.token_savings_percent >= 25.0 {
            Some(Route::NativeRtk)
        } else if evidence.raw_median_ms <= evidence.candidate_median_ms {
            Some(Route::Raw)
        } else {
            Some(Route::NativeRtk)
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CalibrationFile {
    schema_version: u32,
    entries: Vec<CalibrationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalibrationEntry {
    signature: String,
    key: String,
    raw_samples_ms: Vec<f64>,
    native_samples_ms: Vec<f64>,
    native_input_tokens: i64,
    native_saved_tokens: i64,
}

#[derive(Debug, Clone)]
struct CalibrationPlan {
    signature: String,
    key: String,
    route: Route,
    reason: &'static str,
}

impl CalibrationEntry {
    fn token_savings_percent(&self) -> f64 {
        if self.native_input_tokens > 0 {
            (self.native_saved_tokens as f64 / self.native_input_tokens as f64) * 100.0
        } else {
            0.0
        }
    }

    fn selected_route(&self) -> Route {
        select_adaptive_route(
            median(&self.raw_samples_ms),
            median(&self.native_samples_ms),
            self.token_savings_percent(),
        )
    }

    fn phase(&self) -> &'static str {
        if self.raw_samples_ms.is_empty() || self.native_samples_ms.len() < 2 {
            "candidate"
        } else if self.raw_samples_ms.len() < 2 {
            "provisional"
        } else {
            "stable"
        }
    }
}

fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Some(sorted[middle])
    }
}

fn select_adaptive_route(
    raw_median_ms: Option<f64>,
    native_median_ms: Option<f64>,
    token_savings_percent: f64,
) -> Route {
    if token_savings_percent >= 25.0 {
        Route::NativeRtk
    } else if raw_median_ms
        .zip(native_median_ms)
        .is_some_and(|(raw, native)| raw <= native)
    {
        Route::Raw
    } else {
        Route::NativeRtk
    }
}

fn wad_data_root() -> PathBuf {
    env::var_os("RTK_WAD_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|root| root.join("rtk-wad"))
        })
        .unwrap_or_else(env::temp_dir)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProjectLocationKind {
    Windows,
    Wsl,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ProjectLocation {
    kind: ProjectLocationKind,
    path: String,
    distro: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct WindowsToolProbe {
    executable: Option<String>,
    native_rtk: Option<String>,
    #[serde(default)]
    executable_identity: Option<BinaryIdentity>,
    #[serde(default)]
    native_rtk_identity: Option<BinaryIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct WslToolProbe {
    distro: String,
    wsl_version: Option<u8>,
    executable: Option<String>,
    rtk: Option<String>,
    #[serde(default)]
    executable_identity: Option<BinaryIdentity>,
    #[serde(default)]
    rtk_identity: Option<BinaryIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct BinaryIdentity {
    path: String,
    size_bytes: u64,
    modified_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ProviderCacheEntry {
    tool: String,
    observed_unix_seconds: u64,
    windows: WindowsToolProbe,
    #[serde(default)]
    wsl_probe_complete: bool,
    wsl: Vec<WslToolProbe>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ProviderCacheFile {
    schema_version: u32,
    entries: Vec<ProviderCacheEntry>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProviderKind {
    WindowsRaw,
    WindowsRtk,
    WslRaw,
    WslRtk,
}

impl ProviderKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::WindowsRaw => "windows-raw",
            Self::WindowsRtk => "windows-rtk",
            Self::WslRaw => "wsl-raw",
            Self::WslRtk => "wsl-rtk",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ProviderCandidate {
    kind: ProviderKind,
    distro: Option<String>,
    wsl_version: Option<u8>,
    executable: String,
    rtk: Option<String>,
    project_path: Option<String>,
    usable: bool,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct ProviderResolution {
    schema_version: u32,
    tool: String,
    cache: &'static str,
    project: ProjectLocation,
    availability: ProviderCacheEntry,
    candidates: Vec<ProviderCandidate>,
    recommended: Option<usize>,
    install: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct SetupPlan {
    schema_version: u32,
    tool: String,
    mode: &'static str,
    status: &'static str,
    reason: String,
    proposed_provider: Option<&'static str>,
    proposed_command: Option<Vec<String>>,
    verification_command: Vec<String>,
    apply: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct SetupTransaction {
    schema_version: u32,
    tool: String,
    status: String,
    observed_unix_seconds: u64,
    command: Option<Vec<String>>,
    detail: String,
}

fn provider_cache_path() -> PathBuf {
    wad_data_root().join("provider-cache-v2.json")
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn load_provider_cache() -> ProviderCacheFile {
    fs::read_to_string(provider_cache_path())
        .ok()
        .and_then(|contents| serde_json::from_str::<ProviderCacheFile>(&contents).ok())
        .filter(|cache| cache.schema_version == PROVIDER_CACHE_SCHEMA_VERSION)
        .unwrap_or(ProviderCacheFile {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            entries: Vec::new(),
        })
}

fn save_provider_cache(cache: &ProviderCacheFile) -> Result<(), String> {
    let root = wad_data_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("unable to create provider cache directory: {error}"))?;
    let target = provider_cache_path();
    let temporary = root.join(format!("provider-cache-{}.pending", std::process::id()));
    let contents = serde_json::to_vec_pretty(cache)
        .map_err(|error| format!("unable to encode provider cache: {error}"))?;
    fs::write(&temporary, contents)
        .map_err(|error| format!("unable to write provider cache: {error}"))?;
    if target.exists() {
        let _ = fs::remove_file(&target);
    }
    fs::rename(&temporary, &target)
        .map_err(|error| format!("unable to finalize provider cache: {error}"))
}

fn cache_entry_is_fresh(entry: &ProviderCacheEntry, now: u64) -> bool {
    now.saturating_sub(entry.observed_unix_seconds) <= PROVIDER_CACHE_TTL_SECONDS
}

fn first_output_line(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn first_windows_executable(name: &str) -> Option<String> {
    Command::new("where.exe")
        .arg(name)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| first_output_line(&output.stdout))
}

fn configured_windows_executable(path: &str) -> Option<String> {
    Path::new(path)
        .is_file()
        .then(|| path.to_owned())
        .or_else(|| first_windows_executable(path))
}

fn windows_binary_identity(path: &str) -> Option<BinaryIdentity> {
    let metadata = fs::metadata(path).ok()?;
    let modified_unix_seconds = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(BinaryIdentity {
        path: path.to_owned(),
        size_bytes: metadata.len(),
        modified_unix_seconds,
    })
}

fn parse_wsl_binary_identity(
    path: Option<String>,
    identity: Option<String>,
) -> Option<BinaryIdentity> {
    let path = path?;
    let (size_bytes, modified_unix_seconds) = identity?
        .split_once(':')
        .and_then(|(size, modified)| Some((size.parse().ok()?, modified.parse().ok()?)))?;
    Some(BinaryIdentity {
        path,
        size_bytes,
        modified_unix_seconds,
    })
}

fn parse_wsl_distributions(output: &str) -> Vec<(String, Option<u8>)> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line
                .trim()
                .trim_start_matches('*')
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.len() < 3 || fields[0].eq_ignore_ascii_case("name") {
                return None;
            }
            let version = fields.last()?.parse::<u8>().ok();
            let name = fields[..fields.len() - 2].join(" ");
            (!name.is_empty()).then_some((name, version))
        })
        .collect()
}

fn is_eligible_wsl_distro(distro: &str) -> bool {
    !matches!(
        distro.to_ascii_lowercase().as_str(),
        "docker-desktop" | "docker-desktop-data"
    )
}

fn installed_wsl_distributions() -> Vec<(String, Option<u8>)> {
    Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            parse_wsl_distributions(&decode_wsl_output(&output.stdout))
                .into_iter()
                .filter(|(distro, _)| is_eligible_wsl_distro(distro))
                .collect()
        })
        .unwrap_or_default()
}

fn probe_wsl_tool(distro: &str, wsl_version: Option<u8>, tool: &str) -> WslToolProbe {
    let script = "tool_path=$(command -v \"$1\" 2>/dev/null || true); rtk_path=$(command -v rtk 2>/dev/null || true); tool_identity=$(stat -Lc '%s:%Y' -- \"$tool_path\" 2>/dev/null || true); rtk_identity=$(stat -Lc '%s:%Y' -- \"$rtk_path\" 2>/dev/null || true); printf '%s\\n%s\\n%s\\n%s\\n' \"$tool_path\" \"$rtk_path\" \"$tool_identity\" \"$rtk_identity\"";
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            distro,
            "--exec",
            "sh",
            "-c",
            script,
            "rtk-wad-provider-probe",
            tool,
        ])
        .output();
    let (executable, rtk, executable_identity, rtk_identity) = output
        .ok()
        .filter(|result| result.status.success())
        .map(|result| {
            let rendered = decode_wsl_output(&result.stdout);
            let mut lines = rendered.lines().map(str::trim).map(str::to_owned);
            let executable = lines.next().filter(|line| !line.is_empty());
            let rtk = lines.next().filter(|line| !line.is_empty());
            let executable_identity = lines.next().filter(|line| !line.is_empty());
            let rtk_identity = lines.next().filter(|line| !line.is_empty());
            (
                executable.clone(),
                rtk.clone(),
                parse_wsl_binary_identity(executable, executable_identity),
                parse_wsl_binary_identity(rtk, rtk_identity),
            )
        })
        .unwrap_or((None, None, None, None));
    WslToolProbe {
        distro: distro.to_owned(),
        wsl_version,
        executable,
        rtk,
        executable_identity,
        rtk_identity,
    }
}

fn classify_project_path(path: &str) -> ProjectLocation {
    let normalized = path.replace('/', "\\");
    let lowered = normalized.to_ascii_lowercase();
    for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
        if lowered.starts_with(prefix) {
            let original_remainder = &normalized[prefix.len()..];
            let mut parts = original_remainder.splitn(2, '\\');
            if let Some(distro) = parts.next().filter(|value| !value.is_empty()) {
                let linux_path =
                    format!("/{}", parts.next().unwrap_or_default().replace('\\', "/"));
                return ProjectLocation {
                    kind: ProjectLocationKind::Wsl,
                    path: linux_path,
                    distro: Some(distro.to_owned()),
                };
            }
        }
    }
    if windows_path_to_wsl_path(path).is_some() {
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: path.to_owned(),
            distro: None,
        }
    } else {
        ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: path.to_owned(),
            distro: None,
        }
    }
}

fn current_project_location(config: &Config) -> ProjectLocation {
    if let Some(cwd) = &config.cwd {
        return ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: cwd.clone(),
            distro: Some(config.distro.clone()),
        };
    }
    env::current_dir()
        .map(|path| classify_project_path(&path.to_string_lossy()))
        .unwrap_or(ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: String::new(),
            distro: None,
        })
}

fn discover_tool(tool: &str, config: &Config, include_wsl: bool) -> ProviderCacheEntry {
    let executable = if tool == "go" { "go.exe" } else { tool };
    let windows_executable = first_windows_executable(executable);
    let native_rtk = configured_windows_executable(&config.native_rtk_path);
    let windows = WindowsToolProbe {
        executable_identity: windows_executable
            .as_deref()
            .and_then(windows_binary_identity),
        native_rtk_identity: native_rtk.as_deref().and_then(windows_binary_identity),
        executable: windows_executable,
        native_rtk,
    };
    let wsl = include_wsl
        .then(installed_wsl_distributions)
        .unwrap_or_default()
        .into_iter()
        .map(|(distro, version)| probe_wsl_tool(&distro, version, tool))
        .collect();
    ProviderCacheEntry {
        tool: tool.to_owned(),
        observed_unix_seconds: unix_seconds(),
        windows,
        wsl_probe_complete: include_wsl,
        wsl,
    }
}

fn cached_or_discovered_tool(
    tool: &str,
    config: &Config,
    refresh: bool,
    require_wsl: bool,
) -> (ProviderCacheEntry, &'static str) {
    let now = unix_seconds();
    let mut cache = load_provider_cache();
    if !refresh
        && let Some(entry) = cache.entries.iter().find(|entry| {
            entry.tool == tool
                && cache_entry_is_fresh(entry, now)
                && (!require_wsl || entry.wsl_probe_complete)
        })
    {
        return (entry.clone(), "hit");
    }
    let discovered = discover_tool(tool, config, require_wsl);
    cache.entries.retain(|entry| entry.tool != tool);
    cache.entries.push(discovered.clone());
    if let Err(error) = save_provider_cache(&cache) {
        trace(format!("provider cache was not saved: {error}"));
    }
    (discovered, "miss")
}

fn wsl_exec_prefix(distro: &str, user: Option<&str>) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("-d"), OsString::from(distro)];
    if let Some(user) = user {
        arguments.extend([OsString::from("-u"), OsString::from(user)]);
    }
    arguments.push(OsString::from("--exec"));
    arguments
}

fn wsl_mapping_arguments_with_user(
    distro: &str,
    user: Option<&str>,
    windows_path: &str,
) -> Vec<OsString> {
    let mut arguments = wsl_exec_prefix(distro, user);
    arguments.extend([
        OsString::from("wslpath"),
        OsString::from("-a"),
        OsString::from(windows_path),
    ]);
    arguments
}

fn mapped_windows_project_path(
    distro: &str,
    user: Option<&str>,
    windows_path: &str,
) -> Option<String> {
    Command::new("wsl.exe")
        .args(wsl_mapping_arguments_with_user(distro, user, windows_path))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| first_output_line(&output.stdout))
        .filter(|path| path.starts_with('/'))
}

fn windows_mapping_arguments_with_user(
    distro: &str,
    user: Option<&str>,
    linux_path: &str,
) -> Vec<OsString> {
    let mut arguments = wsl_exec_prefix(distro, user);
    arguments.extend([
        OsString::from("wslpath"),
        OsString::from("-w"),
        OsString::from("-a"),
        OsString::from(linux_path),
    ]);
    arguments
}

fn mapped_wsl_project_path(distro: &str, user: Option<&str>, linux_path: &str) -> Option<String> {
    Command::new("wsl.exe")
        .args(windows_mapping_arguments_with_user(
            distro, user, linux_path,
        ))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| first_output_line(&output.stdout))
}

fn wsl_directory_exists(distro: &str, user: Option<&str>, path: &str) -> bool {
    Command::new("wsl.exe")
        .args({
            let mut arguments = wsl_exec_prefix(distro, user);
            arguments.extend([
                OsString::from("test"),
                OsString::from("-d"),
                OsString::from(path),
            ]);
            arguments
        })
        .status()
        .is_ok_and(|status| status.success())
}

fn is_windows_project_path_for_distro(path: &str, expected_distro: Option<&str>) -> bool {
    match classify_project_path(path) {
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            ..
        } => true,
        ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            distro: Some(distro),
            ..
        } => expected_distro.is_some_and(|expected| distro.eq_ignore_ascii_case(expected)),
        ProjectLocation {
            kind: ProjectLocationKind::Wsl | ProjectLocationKind::Unknown,
            ..
        } => false,
    }
}

fn wsl_project_path_with(
    project: &ProjectLocation,
    probe: &WslToolProbe,
    map_windows_path: impl FnOnce(&str, &str) -> Option<String>,
    directory_exists: impl FnOnce(&str, &str) -> bool,
) -> Option<String> {
    let path = match project.kind {
        ProjectLocationKind::Windows => map_windows_path(&probe.distro, &project.path),
        ProjectLocationKind::Wsl if project.distro.as_deref() == Some(probe.distro.as_str()) => {
            Some(project.path.clone())
        }
        ProjectLocationKind::Wsl | ProjectLocationKind::Unknown => None,
    }?;
    (path.starts_with('/') && directory_exists(&probe.distro, &path)).then_some(path)
}

fn wsl_project_path(
    project: &ProjectLocation,
    probe: &WslToolProbe,
    user: Option<&str>,
) -> Option<String> {
    wsl_project_path_with(
        project,
        probe,
        |distro, path| mapped_windows_project_path(distro, user, path),
        |distro, path| wsl_directory_exists(distro, user, path),
    )
}

fn windows_project_path_with(
    project: &ProjectLocation,
    map_wsl_path: impl FnOnce(&str, &str) -> Option<String>,
    directory_exists: impl FnOnce(&str) -> bool,
) -> Option<String> {
    let path = match project.kind {
        ProjectLocationKind::Windows => Some(project.path.clone()),
        ProjectLocationKind::Wsl => project
            .distro
            .as_deref()
            .and_then(|distro| map_wsl_path(distro, &project.path)),
        ProjectLocationKind::Unknown => None,
    }?;
    let expected_distro = (project.kind == ProjectLocationKind::Wsl)
        .then_some(project.distro.as_deref())
        .flatten();
    (is_windows_project_path_for_distro(&path, expected_distro) && directory_exists(&path))
        .then_some(path)
}

fn windows_project_path(project: &ProjectLocation, user: Option<&str>) -> Option<String> {
    windows_project_path_with(
        project,
        |distro, path| mapped_wsl_project_path(distro, user, path),
        |path| Path::new(path).is_dir(),
    )
}

fn resolve_tool_provider(tool: &str, config: &Config, refresh: bool) -> ProviderResolution {
    let project = current_project_location(config);
    let (discovery, cache) = cached_or_discovered_tool(tool, config, refresh, true);
    resolve_tool_provider_from_discovery_with_user(
        tool,
        project,
        discovery,
        cache,
        config.user.as_deref(),
    )
}

fn resolve_tool_provider_from_discovery_with_user(
    tool: &str,
    project: ProjectLocation,
    discovery: ProviderCacheEntry,
    cache: &'static str,
    user: Option<&str>,
) -> ProviderResolution {
    let availability = discovery.clone();
    let mut candidates = Vec::new();
    if let Some(executable) = discovery.windows.executable {
        let project_path = windows_project_path(&project, user);
        let usable = project_path.is_some();
        candidates.push(ProviderCandidate {
            kind: if discovery.windows.native_rtk.is_some() {
                ProviderKind::WindowsRtk
            } else {
                ProviderKind::WindowsRaw
            },
            distro: None,
            wsl_version: None,
            executable,
            rtk: discovery.windows.native_rtk,
            project_path,
            usable,
            reason: if usable {
                if project.kind == ProjectLocationKind::Wsl {
                    "Windows toolchain and WSL-to-Windows project mapping are verified; generic execution remains diagnostic until P14".to_owned()
                } else {
                    "native Windows toolchain and project directory are available".to_owned()
                }
            } else {
                "provider is present but its project directory is not verified for Windows execution"
                    .to_owned()
            },
        });
    }
    for probe in discovery.wsl {
        if let Some(executable) = &probe.executable {
            let project_path = wsl_project_path(&project, &probe, user);
            let usable = project_path.is_some();
            candidates.push(ProviderCandidate {
                kind: if probe.rtk.is_some() {
                    ProviderKind::WslRtk
                } else {
                    ProviderKind::WslRaw
                },
                distro: Some(probe.distro),
                wsl_version: probe.wsl_version,
                executable: executable.clone(),
                rtk: probe.rtk,
                project_path,
                usable,
                reason: if usable {
                    "WSL toolchain and project path mapping are available".to_owned()
                } else if project.kind == ProjectLocationKind::Windows {
                    "provider is present but Windows-to-WSL project mapping failed".to_owned()
                } else {
                    "provider is present but its project path mapping is not yet verified"
                        .to_owned()
                },
            });
        }
    }
    let recommended = candidates.iter().position(|candidate| candidate.usable);
    ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: tool.to_owned(),
        cache,
        project,
        availability,
        candidates,
        recommended,
        install: "disabled_in_p12",
    }
}

enum GoProviderDecision {
    KeepStaticRoute,
    UseWsl {
        route: Route,
        config: Box<Config>,
        reason: String,
    },
    Missing {
        reason: String,
    },
}

fn is_go_command(arguments: &[OsString]) -> bool {
    arguments.first().is_some_and(|argument| argument == "go")
}

fn wsl_route_for_version(version: Option<u8>) -> Option<Route> {
    match version {
        Some(1) => Some(Route::Wsl1),
        Some(2) => Some(Route::Wsl2),
        _ => None,
    }
}

fn windows_go_is_usable(
    project: &ProjectLocation,
    static_route: Route,
    windows: &WindowsToolProbe,
) -> bool {
    project.kind != ProjectLocationKind::Wsl
        && windows.executable.is_some()
        && (static_route != Route::NativeRtk || windows.native_rtk.is_some())
}

fn go_provider_decision(
    arguments: &[OsString],
    config: &Config,
    static_route: Route,
) -> GoProviderDecision {
    if !is_go_command(arguments) || has_wsl_path(arguments) {
        return GoProviderDecision::KeepStaticRoute;
    }
    let project = current_project_location(config);
    let (discovery, _cache) = cached_or_discovered_tool("go", config, false, false);
    if windows_go_is_usable(&project, static_route, &discovery.windows) {
        return GoProviderDecision::KeepStaticRoute;
    }
    go_provider_decision_from_resolution(
        config,
        static_route,
        resolve_tool_provider("go", config, false),
    )
}

fn go_provider_decision_from_resolution(
    config: &Config,
    static_route: Route,
    resolution: ProviderResolution,
) -> GoProviderDecision {
    let windows_is_usable = windows_go_is_usable(
        &resolution.project,
        static_route,
        &resolution.availability.windows,
    );
    if windows_is_usable {
        return GoProviderDecision::KeepStaticRoute;
    }
    let Some(candidate) = resolution.candidates.iter().find(|candidate| {
        candidate.usable
            && candidate.kind == ProviderKind::WslRtk
            && wsl_route_for_version(candidate.wsl_version).is_some()
    }) else {
        return GoProviderDecision::Missing {
            reason: "Go is unavailable in the safe Windows and WSL providers; run `rtk-wad doctor go` for details. Installation is disabled in PD3.".to_owned(),
        };
    };
    let route = wsl_route_for_version(candidate.wsl_version)
        .expect("eligible WSL provider has a supported WSL version");
    let mut selected = config.clone();
    selected.backend = if route == Route::Wsl1 {
        WslBackend::Wsl1
    } else {
        WslBackend::Wsl2
    };
    selected.distro = candidate
        .distro
        .clone()
        .expect("WSL provider candidate has a distro");
    selected.cwd = candidate.project_path.clone();
    if selected.rtk_path.is_none() {
        selected.rtk_path = candidate.rtk.clone();
    }
    GoProviderDecision::UseWsl {
        route,
        config: Box::new(selected),
        reason: format!(
            "on-demand Go discovery selected {} in WSL {} with a verified project path",
            candidate.kind.as_str(),
            candidate.distro.as_deref().unwrap_or_default()
        ),
    }
}

fn print_provider_resolution(
    resolution: &ProviderResolution,
    json: bool,
    doctor: bool,
) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(resolution) {
            Ok(rendered) => {
                println!("{rendered}");
                if doctor && resolution.recommended.is_none() {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(error) => {
                eprintln!("rtk-wad: unable to render provider resolution: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("tool={}", resolution.tool);
    println!("cache={}", resolution.cache);
    println!("project_kind={:?}", resolution.project.kind);
    println!("project_path={}", resolution.project.path);
    if let Some(distro) = &resolution.project.distro {
        println!("project_distro={distro}");
    }
    println!(
        "windows_{}_path={}",
        resolution.tool,
        resolution
            .availability
            .windows
            .executable
            .as_deref()
            .unwrap_or("missing")
    );
    println!(
        "windows_rtk_path={}",
        resolution
            .availability
            .windows
            .native_rtk
            .as_deref()
            .unwrap_or("missing")
    );
    println!(
        "windows_{}_identity={}",
        resolution.tool,
        binary_identity_display(resolution.availability.windows.executable_identity.as_ref())
    );
    println!(
        "windows_rtk_identity={}",
        binary_identity_display(resolution.availability.windows.native_rtk_identity.as_ref())
    );
    for probe in &resolution.availability.wsl {
        println!(
            "inspected_distro={};wsl_version={};{}_path={};{}_identity={};rtk_path={};rtk_identity={}",
            probe.distro,
            probe
                .wsl_version
                .map_or_else(|| "unknown".to_owned(), |version| version.to_string()),
            resolution.tool,
            probe.executable.as_deref().unwrap_or("missing"),
            resolution.tool,
            binary_identity_display(probe.executable_identity.as_ref()),
            probe.rtk.as_deref().unwrap_or("missing"),
            binary_identity_display(probe.rtk_identity.as_ref())
        );
    }
    if resolution.candidates.is_empty() {
        println!("recommended=none");
        println!("install={}", resolution.install);
        return if doctor {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    for (index, candidate) in resolution.candidates.iter().enumerate() {
        println!(
            "candidate_{index}={:?};distro={};usable={};executable={};reason={}",
            candidate.kind,
            candidate.distro.as_deref().unwrap_or("windows"),
            candidate.usable,
            candidate.executable,
            candidate.reason
        );
        if let Some(project_path) = &candidate.project_path {
            println!("candidate_{index}_project_path={project_path}");
        }
    }
    println!(
        "recommended={}",
        resolution
            .recommended
            .map_or_else(|| "none".to_owned(), |index| index.to_string())
    );
    println!("install={}", resolution.install);
    if doctor && resolution.recommended.is_none() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn binary_identity_display(identity: Option<&BinaryIdentity>) -> String {
    identity.map_or_else(
        || "missing".to_owned(),
        |identity| {
            format!(
                "{}:{}:{}",
                identity.path, identity.size_bytes, identity.modified_unix_seconds
            )
        },
    )
}

fn is_safe_provider_tool_name(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 128
        && tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn provider_command(arguments: &[OsString], config: &Config, doctor: bool) -> ExitCode {
    let Some(tool) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        eprintln!(
            "rtk-wad: usage: {} <tool> [--json] [--refresh]",
            if doctor {
                DOCTOR_ARGUMENT
            } else {
                RESOLVE_ARGUMENT
            }
        );
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) || arguments.len() > 4 {
        eprintln!("rtk-wad: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let json = arguments
        .iter()
        .skip(2)
        .any(|argument| argument == "--json");
    let refresh = arguments
        .iter()
        .skip(2)
        .any(|argument| argument == "--refresh");
    if arguments
        .iter()
        .skip(2)
        .any(|argument| argument != "--json" && argument != "--refresh")
    {
        eprintln!(
            "rtk-wad: usage: {} <tool> [--json] [--refresh]",
            if doctor {
                DOCTOR_ARGUMENT
            } else {
                RESOLVE_ARGUMENT
            }
        );
        return ExitCode::FAILURE;
    }
    print_provider_resolution(&resolve_tool_provider(tool, config, refresh), json, doctor)
}

fn has_complete_go_provider(resolution: &ProviderResolution) -> bool {
    if resolution.project.kind != ProjectLocationKind::Wsl
        && resolution.availability.windows.executable.is_some()
    {
        return true;
    }
    resolution.candidates.iter().any(|candidate| {
        candidate.usable
            && candidate.kind == ProviderKind::WslRtk
            && wsl_route_for_version(candidate.wsl_version).is_some()
    })
}

fn setup_go_plan_from_resolution(
    resolution: &ProviderResolution,
    winget_available: bool,
) -> SetupPlan {
    let verification_command = vec![
        "rtk-wad".to_owned(),
        "doctor".to_owned(),
        "go".to_owned(),
        "--refresh".to_owned(),
    ];
    if has_complete_go_provider(resolution) {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "ready",
            reason: "a complete existing Go provider is already available; no setup is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    if resolution.project.kind == ProjectLocationKind::Windows
        && resolution.availability.windows.native_rtk.is_some()
        && winget_available
    {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "planned",
            reason: "Windows Go is absent while native RTK is already available".to_owned(),
            proposed_provider: Some("windows-winget"),
            proposed_command: Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ]),
            verification_command,
            apply: "unavailable_in_pd4",
        };
    }
    let reason = if resolution.project.kind == ProjectLocationKind::Wsl {
        "no complete provider is available for this WSL project; PD4 will not install a Windows toolchain across hosts".to_owned()
    } else if resolution.availability.windows.native_rtk.is_none() {
        "Windows Go setup is blocked because a verified native RTK provider is also required and is not available".to_owned()
    } else {
        "Windows Go setup is blocked because winget is unavailable; no alternate installer is selected automatically".to_owned()
    };
    SetupPlan {
        schema_version: 1,
        tool: "go".to_owned(),
        mode: "plan-only",
        status: "blocked",
        reason,
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_in_pd4",
    }
}

fn print_setup_plan(plan: &SetupPlan, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(plan) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rtk-wad: unable to render setup plan: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("tool={}", plan.tool);
    println!("mode={}", plan.mode);
    println!("status={}", plan.status);
    println!("reason={}", plan.reason);
    if let Some(provider) = plan.proposed_provider {
        println!("proposed_provider={provider}");
    }
    if let Some(command) = &plan.proposed_command {
        println!("proposed_command={}", command.join(" "));
    }
    println!(
        "verification_command={}",
        plan.verification_command.join(" ")
    );
    println!("apply={}", plan.apply);
    ExitCode::SUCCESS
}

fn setup_transaction_path() -> PathBuf {
    wad_data_root().join("setup-transaction-v1.json")
}

fn load_setup_transaction() -> Option<SetupTransaction> {
    fs::read_to_string(setup_transaction_path())
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

fn write_setup_transaction(transaction: &SetupTransaction) -> Result<(), String> {
    let destination = setup_transaction_path();
    let encoded = serde_json::to_string_pretty(transaction)
        .map_err(|error| format!("unable to encode setup transaction: {error}"))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create setup transaction directory: {error}"))?;
    }
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write setup transaction: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate setup transaction: {error}"))
}

fn print_setup_transaction(transaction: Option<&SetupTransaction>, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(&transaction) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rtk-wad: unable to render setup transaction: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match transaction {
        Some(transaction) => {
            println!("tool={}", transaction.tool);
            println!("status={}", transaction.status);
            println!(
                "observed_unix_seconds={}",
                transaction.observed_unix_seconds
            );
            println!("detail={}", transaction.detail);
            if let Some(command) = &transaction.command {
                println!("command={}", command.join(" "));
            }
        }
        None => println!("No local setup transaction is recorded."),
    }
    ExitCode::SUCCESS
}

fn record_setup_transaction(
    status: &str,
    command: Option<Vec<String>>,
    detail: impl Into<String>,
) -> Result<SetupTransaction, String> {
    let transaction = SetupTransaction {
        schema_version: 1,
        tool: "go".to_owned(),
        status: status.to_owned(),
        observed_unix_seconds: unix_seconds(),
        command,
        detail: detail.into(),
    };
    write_setup_transaction(&transaction)?;
    Ok(transaction)
}

fn setup_recovery_outcome(has_complete_provider: bool) -> (&'static str, &'static str) {
    if has_complete_provider {
        (
            "recovered_verified",
            "fresh provider discovery found a complete Go provider; no installer was replayed",
        )
    } else {
        (
            "recovery_required",
            "fresh provider discovery is still incomplete; no installer was replayed and manual review is required",
        )
    }
}

fn recover_setup_transaction(config: &Config, json: bool) -> ExitCode {
    let Some(previous) = load_setup_transaction() else {
        return print_setup_transaction(None, json);
    };
    let resolution = resolve_tool_provider("go", config, true);
    let (status, detail) = setup_recovery_outcome(has_complete_go_provider(&resolution));
    let recovered = match record_setup_transaction(status, previous.command, detail) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("rtk-wad: {error}");
            return ExitCode::FAILURE;
        }
    };
    print_setup_transaction(Some(&recovered), json)
}

fn apply_setup_plan(plan: &SetupPlan, config: &Config, json: bool) -> ExitCode {
    if plan.status == "ready" {
        return print_setup_plan(plan, json);
    }
    let Some(command) = plan.proposed_command.clone() else {
        eprintln!("rtk-wad: setup is blocked; no installer is selected automatically");
        return ExitCode::FAILURE;
    };
    if let Err(error) = record_setup_transaction(
        "running",
        Some(command.clone()),
        "installer started after explicit --apply --confirm",
    ) {
        eprintln!("rtk-wad: {error}");
        return ExitCode::FAILURE;
    }
    let mut installer = Command::new(&command[0]);
    installer.args(&command[1..]);
    let status = match installer.status() {
        Ok(status) => status,
        Err(error) => {
            let detail = format!("installer could not start: {error}");
            let _ = record_setup_transaction("failed", Some(command), &detail);
            eprintln!("rtk-wad: {detail}");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        let detail = format!("installer exited with {status}");
        let _ = record_setup_transaction("failed", Some(command), &detail);
        eprintln!(
            "rtk-wad: {detail}; run `rtk-wad setup go --recover` to re-discover without replaying it"
        );
        return ExitCode::FAILURE;
    }
    let resolution = resolve_tool_provider("go", config, true);
    if has_complete_go_provider(&resolution) {
        let transaction = match record_setup_transaction(
            "verified",
            Some(command),
            "installer completed and fresh provider discovery found a complete Go provider",
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                eprintln!("rtk-wad: {error}");
                return ExitCode::FAILURE;
            }
        };
        return print_setup_transaction(Some(&transaction), json);
    }
    let detail = "installer completed but fresh provider discovery is incomplete; reopen the shell if PATH changed, then run `rtk-wad setup go --recover`";
    let _ = record_setup_transaction("verification_required", Some(command), detail);
    eprintln!("rtk-wad: {detail}");
    ExitCode::FAILURE
}

fn setup_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        eprintln!(
            "rtk-wad: usage: setup go [--json] [--refresh] [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    };
    if tool != "go" {
        eprintln!("rtk-wad: setup currently supports only the exact tool name `go`");
        return ExitCode::FAILURE;
    }
    let flags: Vec<&str> = match arguments
        .iter()
        .skip(2)
        .map(|argument| argument.to_str())
        .collect()
    {
        Some(flags) => flags,
        None => {
            eprintln!("rtk-wad: setup options must be valid Unicode");
            return ExitCode::FAILURE;
        }
    };
    let valid = [
        "--json",
        "--refresh",
        "--status",
        "--recover",
        "--apply",
        "--confirm",
    ];
    if flags.iter().any(|flag| !valid.contains(flag)) {
        eprintln!(
            "rtk-wad: usage: setup go [--json] [--refresh] [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    let json = flags.contains(&"--json");
    let refresh = flags.contains(&"--refresh");
    let status = flags.contains(&"--status");
    let recover = flags.contains(&"--recover");
    let apply = flags.contains(&"--apply");
    let confirm = flags.contains(&"--confirm");
    if [status, recover, apply]
        .into_iter()
        .filter(|selected| *selected)
        .count()
        > 1
        || (confirm && !apply)
        || (status && refresh)
    {
        eprintln!(
            "rtk-wad: usage: setup go [--json] [--refresh] [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    if status {
        return print_setup_transaction(load_setup_transaction().as_ref(), json);
    }
    if recover {
        return recover_setup_transaction(config, json);
    }
    let resolution = resolve_tool_provider(tool, config, refresh || apply);
    let mut plan =
        setup_go_plan_from_resolution(&resolution, first_windows_executable("winget").is_some());
    if plan.status == "planned" {
        plan.apply = "requires_apply_and_confirm";
    }
    if !apply {
        return print_setup_plan(&plan, json);
    }
    if !confirm {
        eprintln!(
            "rtk-wad: review the plan above; re-run with `rtk-wad setup go --apply --confirm` to start the installer"
        );
        let _ = print_setup_plan(&plan, json);
        return ExitCode::from(2);
    }
    apply_setup_plan(&plan, config, json)
}

fn wad_policy_path() -> PathBuf {
    env::var_os("RTK_WAD_POLICY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| wad_data_root().join("route-policy-v1.json"))
}

fn load_route_policy() -> Option<RoutePolicyFile> {
    let path = wad_policy_path();
    let contents = fs::read_to_string(path).ok()?;
    let policy = serde_json::from_str(&contents).ok()?;
    validate_route_policy(&policy).ok()?;
    Some(policy)
}

fn validate_route_policy(policy: &RoutePolicyFile) -> Result<(), String> {
    if policy.schema_version != 1 || policy.evidence.is_empty() {
        return Err("policy evidence must use schema_version 1 and contain evidence".to_owned());
    }
    let mut keys = HashSet::new();
    for evidence in &policy.evidence {
        if evidence.key.trim().is_empty()
            || evidence.sample_count == 0
            || !evidence.raw_median_ms.is_finite()
            || !evidence.candidate_median_ms.is_finite()
            || !evidence.token_savings_percent.is_finite()
            || evidence.raw_median_ms < 0.0
            || evidence.candidate_median_ms < 0.0
        {
            return Err("policy evidence contains an invalid measurement".to_owned());
        }
        if !keys.insert(&evidence.key) {
            return Err(format!(
                "policy evidence contains duplicate key {}",
                evidence.key
            ));
        }
    }
    Ok(())
}

fn merge_route_policy(
    existing: Option<RoutePolicyFile>,
    incoming: RoutePolicyFile,
) -> RoutePolicyFile {
    let mut evidence = existing.map_or_else(Vec::new, |policy| policy.evidence);
    for next in incoming.evidence {
        if let Some(index) = evidence.iter().position(|current| current.key == next.key) {
            evidence[index] = next;
        } else {
            evidence.push(next);
        }
    }
    evidence.sort_by(|left, right| left.key.cmp(&right.key));
    RoutePolicyFile {
        schema_version: 1,
        evidence,
    }
}

fn import_route_policy(source: &Path) -> Result<(), String> {
    let contents = fs::read_to_string(source)
        .map_err(|error| format!("unable to read policy evidence: {error}"))?;
    let incoming: RoutePolicyFile = serde_json::from_str(&contents)
        .map_err(|error| format!("invalid policy evidence: {error}"))?;
    validate_route_policy(&incoming)?;
    let destination = wad_policy_path();
    let existing = if destination.exists() {
        let contents = fs::read_to_string(&destination)
            .map_err(|error| format!("unable to read existing route policy: {error}"))?;
        let policy = serde_json::from_str(&contents)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        validate_route_policy(&policy)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        Some(policy)
    } else {
        None
    };
    let merged = merge_route_policy(existing, incoming);
    let encoded = serde_json::to_string_pretty(&merged)
        .map_err(|error| format!("unable to encode merged route policy: {error}"))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create policy directory: {error}"))?;
    }
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write policy evidence: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate policy evidence: {error}"))
}

fn calibration_path() -> PathBuf {
    wad_data_root().join("calibration-v1.json")
}

fn validate_calibration(file: &CalibrationFile) -> Result<(), String> {
    if file.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }
    let mut signatures = HashSet::new();
    for entry in &file.entries {
        if entry.signature.len() != 16
            || entry.key.trim().is_empty()
            || entry.native_input_tokens < 0
            || entry.native_saved_tokens < 0
            || !entry
                .raw_samples_ms
                .iter()
                .chain(&entry.native_samples_ms)
                .all(|sample| sample.is_finite() && *sample >= 0.0)
            || !signatures.insert(&entry.signature)
        {
            return Err("calibration state contains invalid local evidence".to_owned());
        }
    }
    Ok(())
}

fn load_calibration() -> Result<CalibrationFile, String> {
    let path = calibration_path();
    if !path.exists() {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("unable to read local calibration state: {error}"))?;
    let file: CalibrationFile = serde_json::from_str(&contents)
        .map_err(|error| format!("local calibration state is invalid: {error}"))?;
    validate_calibration(&file)?;
    Ok(file)
}

fn save_calibration(file: &CalibrationFile) -> Result<(), String> {
    validate_calibration(file)?;
    let destination = calibration_path();
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("unable to create calibration directory: {error}"))?;
    }
    let encoded = serde_json::to_string_pretty(file)
        .map_err(|error| format!("unable to encode local calibration state: {error}"))?;
    let temporary = destination.with_extension(format!("{}.new", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("unable to write local calibration state: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("unable to activate local calibration state: {error}"))
}

fn calibration_key(arguments: &[OsString]) -> Option<&'static str> {
    match wad_command_family(arguments) {
        "git" if is_verified_read_only_git(arguments) => Some("git:read-only"),
        "rg" => Some("rg"),
        "npm" if is_verified_npm_run_list_operation(arguments) => Some("npm:run-list"),
        "go" if is_verified_go_test_all_operation(arguments) => Some("go:test-all"),
        _ => None,
    }
}

fn calibration_signature(arguments: &[OsString], current_directory: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut append = |value: &str| {
        for byte in value.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    append(current_directory);
    for argument in arguments {
        append(&argument.to_string_lossy());
    }
    format!("{hash:016x}")
}

fn calibration_plan(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
) -> Result<Option<CalibrationPlan>, String> {
    let Some(current_directory) = current_directory else {
        return Ok(None);
    };
    if has_wsl_path(arguments)
        || windows_path_to_wsl_path(current_directory).is_none()
        || calibration_key(arguments).is_none()
    {
        return Ok(None);
    }
    let key = calibration_key(arguments).expect("calibration key was checked");
    if route_policy_key(arguments)
        .as_deref()
        .and_then(|policy_key| policy.and_then(|policy| policy.route_for(policy_key)))
        .is_some()
    {
        return Ok(None);
    }
    let signature = calibration_signature(arguments, current_directory);
    let state = load_calibration()?;
    let entry = state
        .entries
        .iter()
        .find(|entry| entry.signature == signature);
    let (route, reason) = calibration_route_for(entry);
    Ok(Some(CalibrationPlan {
        signature,
        key: key.to_owned(),
        route,
        reason,
    }))
}

fn calibration_route_for(entry: Option<&CalibrationEntry>) -> (Route, &'static str) {
    let (route, reason) = match entry {
        None => (
            Route::NativeRtk,
            "local calibration candidate: first safe observation uses native RTK",
        ),
        Some(entry) if entry.raw_samples_ms.is_empty() => (
            Route::Raw,
            "local calibration candidate: second safe observation uses raw execution",
        ),
        Some(entry) if entry.native_samples_ms.len() < 2 => (
            Route::NativeRtk,
            "local calibration candidate: third safe observation confirms native RTK",
        ),
        Some(entry) if entry.raw_samples_ms.len() < 2 => {
            let selected = entry.selected_route();
            if entry.raw_samples_ms.len() == 1 && entry.native_samples_ms.len() == 2 {
                (
                    selected,
                    "local calibration provisional choice; validating with one further natural invocation",
                )
            } else {
                (
                    Route::Raw,
                    "local calibration validation samples raw execution before marking a stable route",
                )
            }
        }
        Some(entry) => {
            let selected = entry.selected_route();
            (
                selected,
                if selected == Route::Raw {
                    "local calibration selected stable lower-latency raw execution"
                } else {
                    "local calibration selected stable token-saving native RTK"
                },
            )
        }
    };
    (route, reason)
}

fn cap_samples(samples: &mut Vec<f64>) {
    if samples.len() > CALIBRATION_MAX_SAMPLES {
        let excess = samples.len() - CALIBRATION_MAX_SAMPLES;
        samples.drain(0..excess);
    }
}

fn record_calibration(
    plan: &CalibrationPlan,
    executed_route: Route,
    elapsed: Duration,
    exit_code: i32,
    totals: TokenTotals,
) -> Result<(), String> {
    if exit_code != 0 || !matches!(executed_route, Route::Raw | Route::NativeRtk) {
        return Ok(());
    }
    let mut state = load_calibration()?;
    let entry = match state
        .entries
        .iter_mut()
        .find(|entry| entry.signature == plan.signature)
    {
        Some(entry) => entry,
        None => {
            state.entries.push(CalibrationEntry {
                signature: plan.signature.clone(),
                key: plan.key.clone(),
                raw_samples_ms: Vec::new(),
                native_samples_ms: Vec::new(),
                native_input_tokens: 0,
                native_saved_tokens: 0,
            });
            state.entries.last_mut().expect("entry was just appended")
        }
    };
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    match executed_route {
        Route::Raw => {
            entry.raw_samples_ms.push(elapsed_ms);
            cap_samples(&mut entry.raw_samples_ms);
        }
        Route::NativeRtk => {
            entry.native_samples_ms.push(elapsed_ms);
            cap_samples(&mut entry.native_samples_ms);
            entry.native_input_tokens = entry
                .native_input_tokens
                .saturating_add(totals.input_tokens);
            entry.native_saved_tokens = entry
                .native_saved_tokens
                .saturating_add(totals.saved_tokens);
        }
        Route::Wsl1 | Route::Wsl2 | Route::Auto => unreachable!("route was filtered above"),
    }
    save_calibration(&state)
}

fn print_calibration() -> Result<(), String> {
    let state = load_calibration()?;
    if state.entries.is_empty() {
        println!("No local adaptive calibration evidence is recorded.");
        return Ok(());
    }
    println!("RTK-WAD Local Adaptive Calibration");
    println!();
    for entry in &state.entries {
        let route = entry.selected_route();
        println!("key={}", entry.key);
        println!("signature={}", entry.signature);
        println!("phase={}", entry.phase());
        println!("route={}", route.as_str());
        println!("raw_samples={}", entry.raw_samples_ms.len());
        println!("native_samples={}", entry.native_samples_ms.len());
        println!(
            "native_token_savings_percent={:.1}",
            entry.token_savings_percent()
        );
        println!();
    }
    Ok(())
}

struct WadMetrics {
    ledger_path: PathBuf,
    scratch_path: PathBuf,
}

impl WadMetrics {
    fn begin() -> Result<Self, String> {
        Self::begin_with_tracker(true)
    }

    fn begin_unmeasured() -> Result<Self, String> {
        Self::begin_with_tracker(false)
    }

    fn begin_with_tracker(with_tracker: bool) -> Result<Self, String> {
        let root = wad_data_root();
        let scratch_directory = root.join("scratch");
        fs::create_dir_all(&scratch_directory)
            .map_err(|error| format!("unable to create local metrics directory: {error}"))?;
        cleanup_stale_scratch(&scratch_directory);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let scratch_path = scratch_directory.join(format!("{}-{nonce}.sqlite", std::process::id()));
        if with_tracker {
            let tracker_template = root.join("tracker-template.sqlite");
            if !tracker_template.exists() {
                initialize_tracker_template(&tracker_template)?;
            }
            fs::copy(&tracker_template, &scratch_path)
                .map_err(|error| format!("unable to prepare temporary RTK metrics: {error}"))?;
        }
        let ledger_path = root.join("metrics-v1.sqlite");
        let metrics = Self {
            ledger_path,
            scratch_path,
        };
        if !metrics.ledger_path.exists() {
            metrics.initialize_ledger()?;
        }
        Ok(metrics)
    }

    fn initialize_ledger(&self) -> Result<(), String> {
        let connection = Connection::open(&self.ledger_path)
            .map_err(|error| format!("unable to open local metrics ledger: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
                 CREATE TABLE IF NOT EXISTS invocations (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    route TEXT NOT NULL,
                    command_family TEXT NOT NULL,
                    commands INTEGER NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    saved_tokens INTEGER NOT NULL,
                    elapsed_ms INTEGER NOT NULL,
                    exit_code INTEGER NOT NULL,
                    measured INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_invocations_timestamp ON invocations(timestamp);",
            )
            .map_err(|error| format!("unable to initialize local metrics ledger: {error}"))
    }

    fn scratch_windows_path(&self) -> &Path {
        &self.scratch_path
    }

    fn finish(
        self,
        route: Route,
        command_family: &str,
        elapsed: Duration,
        exit_code: i32,
    ) -> Result<TokenTotals, String> {
        let totals = read_upstream_totals(&self.scratch_path)?;
        let measured = i64::from(totals.commands > 0);
        let connection = Connection::open(&self.ledger_path)
            .map_err(|error| format!("unable to reopen local metrics ledger: {error}"))?;
        connection
            .execute(
                "INSERT INTO invocations (timestamp, route, command_family, commands, input_tokens, output_tokens, saved_tokens, elapsed_ms, exit_code, measured)
                 VALUES (datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    route.as_str(),
                    command_family,
                    totals.commands,
                    totals.input_tokens,
                    totals.output_tokens,
                    totals.saved_tokens,
                    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
                    exit_code,
                    measured,
                ],
            )
            .map_err(|error| format!("unable to record local metrics: {error}"))?;
        remove_scratch_database(&self.scratch_path);
        Ok(totals)
    }

    fn print_gain() -> Result<(), String> {
        let root = wad_data_root();
        let ledger_path = root.join("metrics-v1.sqlite");
        if !ledger_path.exists() {
            println!("RTK-WAD Token Savings\n\nNo measured commands yet.");
            return Ok(());
        }
        let connection = Connection::open(&ledger_path)
            .map_err(|error| format!("unable to open local metrics ledger: {error}"))?;
        let totals = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(commands), 0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(saved_tokens), 0), COALESCE(SUM(measured), 0) FROM invocations",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?)),
            )
            .map_err(|error| format!("unable to read local metrics ledger: {error}"))?;
        let savings = if totals.2 > 0 {
            (totals.4 as f64 / totals.2 as f64) * 100.0
        } else {
            0.0
        };
        println!("RTK-WAD Token Savings");
        println!();
        println!("Invocations: {} ({} measured by RTK)", totals.0, totals.5);
        println!("Commands optimized: {}", totals.1);
        println!("Input tokens: {}", totals.2);
        println!("Output tokens: {}", totals.3);
        println!("Tokens saved: {} ({savings:.1}%)", totals.4);
        println!();
        println!("By route:");
        let mut statement = connection
            .prepare("SELECT route, COUNT(*), COALESCE(SUM(saved_tokens), 0) FROM invocations GROUP BY route ORDER BY saved_tokens DESC, route")
            .map_err(|error| format!("unable to prepare local metrics summary: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| format!("unable to read local metrics summary: {error}"))?;
        for row in rows {
            let (route, count, saved) =
                row.map_err(|error| format!("unable to decode local metrics summary: {error}"))?;
            println!("  {route}: {count} invocation(s), {saved} tokens saved");
        }
        Ok(())
    }
}

fn initialize_tracker_template(path: &Path) -> Result<(), String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("unable to create RTK metrics template: {error}"))?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                original_cmd TEXT NOT NULL,
                rtk_cmd TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL,
                exec_time_ms INTEGER DEFAULT 0,
                project_path TEXT DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp);
             CREATE INDEX IF NOT EXISTS idx_project_path_timestamp ON commands(project_path, timestamp);
             CREATE TABLE IF NOT EXISTS parse_failures (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                raw_command TEXT NOT NULL,
                error_message TEXT NOT NULL,
                fallback_succeeded INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_pf_timestamp ON parse_failures(timestamp);",
        )
        .map_err(|error| format!("unable to initialize RTK metrics template: {error}"))?;
    Ok(())
}

fn read_upstream_totals(path: &Path) -> Result<TokenTotals, String> {
    if !path.exists() {
        return Ok(TokenTotals::default());
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("unable to read temporary RTK metrics: {error}"))?;
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'commands'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("unable to inspect temporary RTK metrics: {error}"))?;
    if exists == 0 {
        return Ok(TokenTotals::default());
    }
    connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(saved_tokens), 0) FROM commands",
            [],
            |row| {
                Ok(TokenTotals {
                    commands: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    saved_tokens: row.get(3)?,
                })
            },
        )
        .map_err(|error| format!("unable to aggregate temporary RTK metrics: {error}"))
}

fn remove_scratch_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
        let _ = fs::remove_file(candidate);
    }
}

fn cleanup_stale_scratch(directory: &Path) {
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(24 * 60 * 60));
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let remove = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .zip(cutoff)
            .is_some_and(|(modified, cutoff)| modified < cutoff);
        if remove {
            let _ = fs::remove_file(entry.path());
        }
    }
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

fn test_ready_wsl_path() -> Option<String> {
    env::var("RTK_WSL_TEST_READY_FILE")
        .ok()
        .and_then(|path| windows_path_to_wsl_path(&path))
}

fn rtk_arguments(arguments: Vec<OsString>, config: &Config, cancel_token: &str) -> Vec<OsString> {
    rtk_arguments_with_metrics(arguments, config, cancel_token, None)
}

fn rtk_arguments_with_metrics(
    arguments: Vec<OsString>,
    config: &Config,
    cancel_token: &str,
    metrics_db_path: Option<&str>,
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
        OsString::from("rtk-wsl"),
        OsString::from(&config.lock_wait),
        OsString::from(&config.lock_path),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
        OsString::from(cancel_token),
        OsString::from(metrics_db_path.unwrap_or("")),
        OsString::from(config.extra_path.as_deref().unwrap_or("")),
        OsString::from(test_ready_wsl_path().unwrap_or_default()),
    ]);
    command.extend(forwarded);
    command
}

fn wsl1_rtk_arguments(arguments: Vec<OsString>, config: &Config) -> Vec<OsString> {
    wsl1_rtk_arguments_with_metrics(arguments, config, None)
}

fn wsl1_rtk_arguments_with_metrics(
    arguments: Vec<OsString>,
    config: &Config,
    metrics_db_path: Option<&str>,
) -> Vec<OsString> {
    let forwarded = forwarded_rtk_arguments(arguments);
    let mut command = wsl_launch_prefix(config);
    command.extend([
        OsString::from("--exec"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(WSL1_LAUNCH_SCRIPT),
        OsString::from("rtk-wsl1"),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
        OsString::from(metrics_db_path.unwrap_or("")),
        OsString::from(config.extra_path.as_deref().unwrap_or("")),
        OsString::from(test_ready_wsl_path().unwrap_or_default()),
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

fn request_linux_interrupt(config: &Config, token: &str) -> std::io::Result<Child> {
    Command::new("wsl.exe")
        .args(cancel_arguments(config, token))
        .spawn()
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
    let mut last_interrupt = None;
    let mut cancellation_started = None;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if console::requested() {
            cancellation_started.get_or_insert_with(Instant::now);
        }
        if let Some(started) = cancellation_started
            && started.elapsed() >= Duration::from_secs(4)
        {
            trace(
                "WSL proxy exceeded the cancellation deadline; terminating only the proxy process",
            );
            child.kill()?;
            return child.wait();
        }
        if cancellation_started.is_some()
            && last_interrupt
                .is_none_or(|previous: Instant| previous.elapsed() >= Duration::from_secs(1))
        {
            match request_linux_interrupt(config, token) {
                Ok(_interrupt_helper) => {
                    trace("forwarded Ctrl+C to the isolated Linux process group");
                }
                Err(error) => trace(format!("unable to start Linux interrupt helper: {error}")),
            }
            last_interrupt = Some(Instant::now());
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
    wsl_process(arguments)
}

fn wsl_process(arguments: Vec<OsString>) -> Command {
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

fn wad_command_family(arguments: &[OsString]) -> &str {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .unwrap_or("unknown")
}

fn has_wsl_path(arguments: &[OsString]) -> bool {
    arguments.iter().any(is_wsl_path)
}

fn git_subcommand(arguments: &[OsString]) -> Option<&str> {
    let mut skip_value = false;
    for argument in arguments.iter().skip(1) {
        let value = argument.to_str()?;
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(value, "-C" | "--git-dir" | "--work-tree" | "-c") {
            skip_value = true;
            continue;
        }
        if value.starts_with('-') {
            continue;
        }
        return Some(value);
    }
    None
}

fn is_verified_read_only_git(arguments: &[OsString]) -> bool {
    matches!(
        git_subcommand(arguments),
        Some("status" | "log" | "show" | "diff" | "rev-parse" | "ls-files" | "grep")
    )
}

fn is_verified_cargo_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments.get(1).and_then(|argument| argument.to_str()),
        Some("check" | "test" | "clippy")
    )
}

fn is_verified_npm_run_list_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand] if program == "npm" && subcommand == "run"
    )
}

fn is_verified_go_test_all_operation(arguments: &[OsString]) -> bool {
    matches!(
        arguments,
        [program, subcommand, selector]
            if program == "go" && subcommand == "test" && selector == "./..."
    )
}

fn route_policy_key(arguments: &[OsString]) -> Option<String> {
    match wad_command_family(arguments) {
        "git" => git_subcommand(arguments).map(|subcommand| format!("git:{subcommand}")),
        "rg" => Some("rg".to_owned()),
        "cargo" => arguments
            .get(1)
            .and_then(|subcommand| subcommand.to_str())
            .map(|subcommand| format!("cargo:{subcommand}")),
        "npm" if is_verified_npm_run_list_operation(arguments) => Some("npm:run-list".to_owned()),
        "go" if is_verified_go_test_all_operation(arguments) => Some("go:test-all".to_owned()),
        _ => None,
    }
}

fn auto_wad_route(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
) -> (Route, &'static str) {
    if has_wsl_path(arguments)
        || current_directory.is_some_and(|directory| windows_path_to_wsl_path(directory).is_none())
    {
        return (
            Route::Wsl1,
            "Linux path or WSL working directory requires Linux execution",
        );
    }
    let policy_key = route_policy_key(arguments);
    if let Some((_key, route)) = policy_key.as_deref().and_then(|key| {
        policy
            .and_then(|policy| policy.route_for(key))
            .map(|route| (key, route))
    }) {
        let permitted = match route {
            Route::Raw => {
                wad_command_family(arguments) == "rg"
                    || is_verified_read_only_git(arguments)
                    || is_verified_cargo_operation(arguments)
                    || is_verified_npm_run_list_operation(arguments)
                    || is_verified_go_test_all_operation(arguments)
            }
            Route::NativeRtk => {
                wad_command_family(arguments) == "rg"
                    || is_verified_read_only_git(arguments)
                    || is_verified_cargo_operation(arguments)
                    || is_verified_npm_run_list_operation(arguments)
                    || is_verified_go_test_all_operation(arguments)
            }
            Route::Wsl1 | Route::Wsl2 | Route::Auto => false,
        };
        if permitted {
            return (
                route,
                if route == Route::Raw {
                    "local benchmark policy selected lower-latency raw execution"
                } else {
                    "local benchmark policy selected token-saving native RTK"
                },
            );
        }
    }
    match wad_command_family(arguments) {
        "npm" | "npx" | "pnpm" | "go" | "dotnet" | "dart" | "flutter" => (
            Route::Raw,
            "validated Windows toolchain fallback avoids an unavailable WSL toolchain",
        ),
        "git" if is_verified_read_only_git(arguments) => (
            Route::NativeRtk,
            "structured native RTK Git adapter is safe for read-only Git",
        ),
        "git" => (
            Route::Raw,
            "Git command is not in the verified read-only allowlist; execute once with native Git",
        ),
        "rg" | "grep" | "find" | "ls" | "tree" | "read" | "files" | "diff" | "cargo" => (
            Route::NativeRtk,
            "structured native RTK adapter avoids the Windows shell parser",
        ),
        _ => (
            Route::Wsl1,
            "no verified native adapter contract; use isolated Linux RTK",
        ),
    }
}

fn configured_wsl_backend(config: &Config, route: Route) -> Config {
    let mut selected = config.clone();
    match route {
        Route::Wsl1 => {
            selected.backend = WslBackend::Wsl1;
            if selected.distro == DEFAULT_DISTRO {
                selected.distro = DEFAULT_WSL1_DISTRO.to_owned();
            }
        }
        Route::Wsl2 => {
            selected.backend = WslBackend::Wsl2;
            if selected.distro == DEFAULT_WSL1_DISTRO {
                selected.distro = DEFAULT_DISTRO.to_owned();
            }
        }
        Route::Auto | Route::Raw | Route::NativeRtk => {}
    }
    selected
}

fn print_adapter_info(config: &Config) {
    println!("adapter=rtk-wad");
    println!("profile={}", config.profile.as_str());
    println!("route_preference={}", config.wad_route.as_str());
    println!("native_rtk_path={}", config.native_rtk_path);
    println!("metrics=local-aggregate-only");
    println!("compatibility_aliases=rtk-wsl,rtk-wsl1");
}

fn run_native_rtk(
    arguments: &[OsString],
    config: &Config,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    run_native_rtk_at(&config.native_rtk_path, arguments, None, metrics)
}

fn run_native_rtk_at(
    executable: &str,
    arguments: &[OsString],
    current_directory: Option<&str>,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new(executable);
    command.args(arguments);
    if let Some(current_directory) = current_directory {
        command.current_dir(current_directory);
    }
    if let Some(metrics) = metrics {
        command.env("RTK_DB_PATH", metrics.scratch_windows_path());
    }
    command.spawn().and_then(|mut child| child.wait())
}

fn run_raw(arguments: &[OsString]) -> std::io::Result<ExitStatus> {
    let Some(program) = arguments.first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a raw route needs a command",
        ));
    };
    let executable = match program.to_str() {
        Some("git") => OsString::from("git.exe"),
        Some("npm") => OsString::from("npm.cmd"),
        Some("npx") => OsString::from("npx.cmd"),
        Some("pnpm") => OsString::from("pnpm.cmd"),
        Some("dart") => OsString::from("dart.bat"),
        Some("flutter") => OsString::from("flutter.bat"),
        _ => program.clone(),
    };
    run_raw_at(&executable, &arguments[1..], None)
}

fn run_raw_at(
    executable: &OsString,
    arguments: &[OsString],
    current_directory: Option<&str>,
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new(executable);
    command.args(arguments);
    if let Some(current_directory) = current_directory {
        command.current_dir(current_directory);
    }
    command.spawn().and_then(|mut child| child.wait())
}

fn has_foreign_absolute_path(arguments: &[OsString], candidate: &ProviderCandidate) -> bool {
    match candidate.kind {
        ProviderKind::WindowsRaw | ProviderKind::WindowsRtk => arguments.iter().any(is_wsl_path),
        ProviderKind::WslRaw | ProviderKind::WslRtk => arguments.iter().any(|argument| {
            argument
                .to_str()
                .and_then(windows_path_to_wsl_path)
                .is_some()
        }),
    }
}

fn provider_execution_route(candidate: &ProviderCandidate) -> Option<Route> {
    match candidate.kind {
        ProviderKind::WindowsRaw => Some(Route::Raw),
        ProviderKind::WindowsRtk => Some(Route::NativeRtk),
        ProviderKind::WslRaw | ProviderKind::WslRtk => wsl_route_for_version(candidate.wsl_version),
    }
}

fn provider_execution_config(config: &Config, candidate: &ProviderCandidate) -> Option<Config> {
    let route = provider_execution_route(candidate)?;
    let mut selected = configured_wsl_backend(config, route);
    if matches!(candidate.kind, ProviderKind::WslRaw | ProviderKind::WslRtk) {
        selected.distro = candidate.distro.clone()?;
        selected.cwd = candidate.project_path.clone();
        selected.rtk_path = Some(match candidate.kind {
            ProviderKind::WslRaw => candidate.executable.clone(),
            ProviderKind::WslRtk => candidate.rtk.clone()?,
            ProviderKind::WindowsRaw | ProviderKind::WindowsRtk => unreachable!(),
        });
    }
    Some(selected)
}

fn run_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let project_path = candidate.project_path.as_deref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider candidate has no verified project directory",
        )
    })?;
    match candidate.kind {
        ProviderKind::WindowsRaw => run_raw_at(
            &OsString::from(&candidate.executable),
            arguments,
            Some(project_path),
        ),
        ProviderKind::WindowsRtk => {
            let mut forwarded = Vec::with_capacity(arguments.len() + 1);
            forwarded.push(OsString::from(tool));
            forwarded.extend(arguments.iter().cloned());
            run_native_rtk_at(
                candidate.rtk.as_deref().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Windows RTK candidate has no RTK executable",
                    )
                })?,
                &forwarded,
                Some(project_path),
                metrics,
            )
        }
        ProviderKind::WslRaw | ProviderKind::WslRtk => {
            let selected = provider_execution_config(config, candidate).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL provider has no supported WSL version or project directory",
                )
            })?;
            let forwarded = if candidate.kind == ProviderKind::WslRtk {
                let mut forwarded = Vec::with_capacity(arguments.len() + 1);
                forwarded.push(OsString::from(tool));
                forwarded.extend(arguments.iter().cloned());
                forwarded
            } else {
                arguments.to_vec()
            };
            let route = provider_execution_route(candidate)
                .expect("WSL provider execution requires a supported WSL route");
            run_wsl_route(
                forwarded,
                &selected,
                route,
                (candidate.kind == ProviderKind::WslRtk)
                    .then_some(metrics)
                    .flatten(),
            )
        }
    }
}

fn provider_exec_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(2).and_then(|argument| argument.to_str()) else {
        eprintln!("rtk-wad: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("rtk-wad: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let separator = arguments.iter().position(|argument| argument == "--");
    let Some(separator) = separator else {
        eprintln!("rtk-wad: provider execution requires `--` before tool arguments");
        return ExitCode::FAILURE;
    };
    if separator < 3 {
        eprintln!("rtk-wad: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    let options = &arguments[3..separator];
    let candidate_index = match options {
        [] => None,
        [flag, index] if flag == "--candidate" => index.to_string_lossy().parse::<usize>().ok(),
        _ => None,
    };
    if !options.is_empty() && candidate_index.is_none() {
        eprintln!("rtk-wad: usage: provider exec <tool> [--candidate <index>] -- <args...>");
        return ExitCode::FAILURE;
    }
    // Execution is explicit and must not reuse a provider identity discovered
    // under a previous RTK path or tool installation state.
    let resolution = resolve_tool_provider(tool, config, true);
    let index = candidate_index.or(resolution.recommended);
    let Some(index) = index else {
        eprintln!(
            "rtk-wad: no verified provider is available; run `rtk-wad doctor {tool}` for details"
        );
        return ExitCode::from(127);
    };
    let Some(candidate) = resolution.candidates.get(index) else {
        eprintln!(
            "rtk-wad: provider candidate {index} does not exist; run `rtk-wad resolve {tool}`"
        );
        return ExitCode::FAILURE;
    };
    if !candidate.usable {
        eprintln!(
            "rtk-wad: provider candidate {index} is not verified: {}",
            candidate.reason
        );
        return ExitCode::from(127);
    }
    let forwarded = &arguments[separator + 1..];
    if has_foreign_absolute_path(forwarded, candidate) {
        eprintln!(
            "rtk-wad: provider execution does not translate foreign absolute arguments; run from the verified project directory with relative paths"
        );
        return ExitCode::FAILURE;
    }
    let Some(route) = provider_execution_route(candidate) else {
        eprintln!("rtk-wad: provider candidate {index} has an unsupported WSL version");
        return ExitCode::FAILURE;
    };
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    if needs_console_handler && !console::install() {
        eprintln!("rtk-wad: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let metrics = match if matches!(
        candidate.kind,
        ProviderKind::WindowsRaw | ProviderKind::WslRaw
    ) {
        WadMetrics::begin_unmeasured()
    } else {
        WadMetrics::begin()
    } {
        Ok(metrics) => Some(metrics),
        Err(error) => {
            eprintln!("rtk-wad: metrics disabled for this invocation: {error}");
            None
        }
    };
    let result = run_provider_candidate(tool, forwarded, config, candidate, metrics.as_ref());
    if needs_console_handler {
        console::uninstall();
    }
    let exit_code = result
        .as_ref()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1);
    if let Some(metrics) = metrics {
        let command_family = format!("provider:{}", candidate.kind.as_str());
        if let Err(error) = metrics.finish(route, &command_family, started.elapsed(), exit_code) {
            eprintln!("rtk-wad: metrics were not recorded: {error}");
        }
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("rtk-wad: unable to start provider candidate {index}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_wsl_route(
    arguments: Vec<OsString>,
    config: &Config,
    route: Route,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        windows_path_to_wsl_path(&metrics.scratch_windows_path().to_string_lossy())
    });
    if route == Route::Wsl1 {
        let _lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        wsl1_process(wsl1_rtk_arguments_with_metrics(
            arguments,
            config,
            metrics_path.as_deref(),
        ))
        .spawn()
        .and_then(|child| wait_for_wsl1_child(child, config))
    } else {
        let token = cancel_token();
        wsl_process(rtk_arguments_with_metrics(
            arguments,
            config,
            &token,
            metrics_path.as_deref(),
        ))
        .spawn()
        .and_then(|child| wait_for_wsl_child(child, config, &token))
    }
}

fn parse_wad_options(
    mut arguments: Vec<OsString>,
    configured: Route,
) -> Result<(Vec<OsString>, Route, bool), String> {
    let mut route = configured;
    let mut explain = false;
    loop {
        match arguments.first().and_then(|argument| argument.to_str()) {
            Some("--route") => {
                if arguments.len() < 2 {
                    return Err("--route requires auto, raw, native-rtk, wsl1, or wsl2".to_owned());
                }
                route = Route::parse(&arguments[1].to_string_lossy())?;
                arguments.drain(0..2);
            }
            Some(EXPLAIN_ROUTE_ARGUMENT) => {
                explain = true;
                arguments.remove(0);
            }
            _ => return Ok((arguments, route, explain)),
        }
    }
}

fn wad_main(arguments: Vec<OsString>, config: &Config) -> ExitCode {
    if arguments
        .first()
        .is_some_and(|argument| argument == PROVIDER_ARGUMENT)
    {
        if arguments.get(1).is_some_and(|argument| argument == "exec") {
            return provider_exec_command(&arguments, config);
        }
        eprintln!("rtk-wad: usage: provider exec <tool> [--candidate <index>] -- <args...>");
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
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return provider_command(&arguments, config, true);
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
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            match load_route_policy() {
                Some(policy) => match serde_json::to_string_pretty(&policy) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("rtk-wad: unable to render route policy: {error}");
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
            return match import_route_policy(Path::new(&arguments[2])) {
                Ok(()) => {
                    println!("Imported local RTK-WAD route policy.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rtk-wad: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!("rtk-wad: usage: rtk-wad policy [show] | policy import <evidence.json>");
        return ExitCode::FAILURE;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == CALIBRATION_ARGUMENT)
    {
        if arguments.len() == 1 || arguments.get(1).is_some_and(|argument| argument == "show") {
            return match print_calibration() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("rtk-wad: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!("rtk-wad: usage: rtk-wad calibration [show]");
        return ExitCode::FAILURE;
    }
    if arguments.len() == 1 && arguments[0] == ADAPTER_INFO_ARGUMENT {
        print_adapter_info(config);
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && (arguments[0] == "gain" || arguments[0] == "stats") {
        return match WadMetrics::print_gain() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("rtk-wad: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let (arguments, requested_route, explain) = match parse_wad_options(arguments, config.wad_route)
    {
        Ok(options) => options,
        Err(error) => {
            eprintln!("rtk-wad: {error}");
            return ExitCode::FAILURE;
        }
    };
    let current_directory = env::current_dir().ok();
    let started = Instant::now();
    let policy = load_route_policy();
    let (initial_route, initial_reason) = if requested_route == Route::Auto {
        auto_wad_route(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
        )
    } else {
        (requested_route, "explicit route preference")
    };
    let mut route = initial_route;
    let mut reason = initial_reason.to_owned();
    let calibration = if requested_route == Route::Auto {
        match calibration_plan(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("rtk-wad: local calibration is unavailable: {error}");
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
    let mut selected_config = configured_wsl_backend(config, route);
    let mut provider_missing = None;
    if requested_route == Route::Auto {
        match go_provider_decision(&arguments, config, route) {
            GoProviderDecision::KeepStaticRoute => {}
            GoProviderDecision::UseWsl {
                route: provider_route,
                config: provider_config,
                reason: provider_reason,
            } => {
                route = provider_route;
                selected_config = *provider_config;
                reason = provider_reason;
            }
            GoProviderDecision::Missing {
                reason: missing_reason,
            } => {
                provider_missing = Some(missing_reason.clone());
                reason = missing_reason;
            }
        }
    }
    if explain {
        println!("route={}", route.as_str());
        println!("reason={reason}");
        println!("command_family={}", wad_command_family(&arguments));
        return if provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        };
    }
    if arguments.is_empty() {
        eprintln!("rtk-wad: no command supplied; use rtk-wad --adapter-info for configuration");
        return ExitCode::FAILURE;
    }
    if let Some(reason) = provider_missing {
        eprintln!("rtk-wad: {reason}");
        return ExitCode::from(127);
    }
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    let mut console_installed = false;
    if needs_console_handler && !console::install() {
        eprintln!("rtk-wad: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    } else if needs_console_handler {
        console_installed = true;
    }
    let metrics = match if route == Route::Raw {
        WadMetrics::begin_unmeasured()
    } else {
        WadMetrics::begin()
    } {
        Ok(metrics) => Some(metrics),
        Err(error) => {
            eprintln!("rtk-wad: metrics disabled for this invocation: {error}");
            None
        }
    };
    let mut executed_route = route;
    let result = match route {
        Route::Raw => run_raw(&arguments),
        Route::NativeRtk => match run_native_rtk(&arguments, &selected_config, metrics.as_ref()) {
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && requested_route == Route::Auto =>
            {
                trace(
                    "native RTK was not found; falling back to isolated WSL1 before any child started",
                );
                if !console_installed {
                    if !console::install() {
                        eprintln!(
                            "rtk-wad: unable to register the Windows console cancellation handler for WSL fallback"
                        );
                        return ExitCode::FAILURE;
                    }
                    console_installed = true;
                }
                executed_route = Route::Wsl1;
                let fallback_config = configured_wsl_backend(config, Route::Wsl1);
                run_wsl_route(
                    arguments.clone(),
                    &fallback_config,
                    Route::Wsl1,
                    metrics.as_ref(),
                )
            }
            result => result,
        },
        Route::Wsl1 | Route::Wsl2 => {
            run_wsl_route(arguments.clone(), &selected_config, route, metrics.as_ref())
        }
        Route::Auto => unreachable!("auto route is resolved before execution"),
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
            executed_route,
            wad_command_family(&arguments),
            elapsed,
            exit_code,
        ) {
            Ok(totals) => totals,
            Err(error) => {
                eprintln!("rtk-wad: metrics were not recorded: {error}");
                TokenTotals::default()
            }
        }
    } else {
        TokenTotals::default()
    };
    if let Some(plan) = &calibration
        && let Err(error) = record_calibration(plan, executed_route, elapsed, exit_code, totals)
    {
        eprintln!("rtk-wad: local calibration was not recorded: {error}");
    }
    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!(
                "rtk-wad: unable to start {} route: {error}",
                executed_route.as_str()
            );
            ExitCode::FAILURE
        }
    }
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
    if config.profile == ExecutableProfile::Wad {
        return wad_main(arguments, &config);
    }
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
        wsl_process(rtk_arguments(arguments, &config, &token))
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
            "RTK_WSL_EXTRA_PATH" => Some("/opt/fixture-bin:/work/tools".to_owned()),
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
        assert!(arguments.contains(&OsString::from("/opt/fixture-bin:/work/tools")));
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

        let invalid_extra_path = Config::from_lookup(|name| match name {
            "RTK_WSL_EXTRA_PATH" => Some("relative:/opt/tools".to_owned()),
            _ => None,
        });
        assert!(invalid_extra_path.is_err());
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
    fn wad_alias_selects_adaptive_profile_without_changing_legacy_defaults() {
        let wad = Config::from_lookup_with_executable(|_| None, Some("rtk-wad.exe"))
            .expect("WAD alias configuration is valid");
        assert_eq!(wad.profile, ExecutableProfile::Wad);
        assert_eq!(wad.backend, WslBackend::Auto);
        assert_eq!(wad.wad_route, Route::Auto);

        let legacy = Config::from_lookup_with_executable(|_| None, Some("rtk-wsl.exe"))
            .expect("legacy configuration is valid");
        assert_eq!(legacy.profile, ExecutableProfile::Legacy);
    }

    #[test]
    fn wad_auto_route_keeps_mutations_raw_and_read_only_commands_structured() {
        let mutation = vec![
            OsString::from("git"),
            OsString::from("commit"),
            OsString::from("-m"),
        ];
        assert_eq!(
            auto_wad_route(&mutation, Some(r"E:\work"), None).0,
            Route::Raw
        );

        let clone = vec![
            OsString::from("git"),
            OsString::from("clone"),
            OsString::from("https://example.invalid/repo"),
        ];
        assert_eq!(auto_wad_route(&clone, Some(r"E:\work"), None).0, Route::Raw);

        let read_only = vec![
            OsString::from("git"),
            OsString::from("log"),
            OsString::from("-1"),
        ];
        assert_eq!(
            auto_wad_route(&read_only, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        let cargo = vec![
            OsString::from("cargo"),
            OsString::from("check"),
            OsString::from("--version"),
        ];
        assert_eq!(
            auto_wad_route(&cargo, Some(r"E:\work"), None).0,
            Route::NativeRtk
        );

        assert_eq!(
            auto_wad_route(&[OsString::from("npm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("npx")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("pnpm")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("go")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("dotnet")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("dart")], Some(r"E:\work"), None).0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(&[OsString::from("flutter")], Some(r"E:\work"), None).0,
            Route::Raw
        );

        let literal = vec![
            OsString::from("proxy"),
            OsString::from("/usr/bin/printf"),
            OsString::from("$HOME; &"),
        ];
        assert_eq!(
            auto_wad_route(&literal, Some(r"E:\work"), None).0,
            Route::Wsl1
        );
    }

    #[test]
    fn policy_uses_measured_savings_without_permitting_git_mutations() {
        let policy = RoutePolicyFile {
            schema_version: 1,
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
            auto_wad_route(
                &[OsString::from("git"), OsString::from("status")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(
                &[OsString::from("rg"), OsString::from("needle")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route(
                &[OsString::from("cargo"), OsString::from("check")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(
                &[OsString::from("npm"), OsString::from("run")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route(
                &[
                    OsString::from("go"),
                    OsString::from("test"),
                    OsString::from("./...")
                ],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route(
                &[OsString::from("go"), OsString::from("test")],
                Some(r"E:\work"),
                Some(&policy)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route(
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
            auto_wad_route(
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
    fn policy_import_merge_preserves_other_evidence_and_replaces_same_key() {
        let existing = RoutePolicyFile {
            schema_version: 1,
            evidence: vec![
                RoutePolicyEvidence {
                    key: "cargo:check".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 20.0,
                    token_savings_percent: 1.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 20.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
            ],
        };
        let incoming = RoutePolicyFile {
            schema_version: 1,
            evidence: vec![
                RoutePolicyEvidence {
                    key: "npm:run-list".to_owned(),
                    raw_median_ms: 30.0,
                    candidate_median_ms: 40.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 5.0,
                    candidate_median_ms: 10.0,
                    token_savings_percent: 90.0,
                    sample_count: 5,
                },
            ],
        };
        let merged = merge_route_policy(Some(existing), incoming);
        assert_eq!(
            merged
                .evidence
                .iter()
                .map(|evidence| evidence.key.as_str())
                .collect::<Vec<_>>(),
            vec!["cargo:check", "npm:run-list", "rg"]
        );
        let rg = merged
            .evidence
            .iter()
            .find(|evidence| evidence.key == "rg")
            .expect("new measurement replaces rg");
        assert_eq!(rg.token_savings_percent, 90.0);
        assert_eq!(merged.route_for("cargo:check"), Some(Route::Raw));
        assert_eq!(merged.route_for("npm:run-list"), Some(Route::Raw));
    }

    #[test]
    fn wad_route_options_are_explicit_and_validate_values() {
        let (arguments, route, explain) = parse_wad_options(
            vec![
                OsString::from("--route"),
                OsString::from("native-rtk"),
                OsString::from("--explain-route"),
                OsString::from("rg"),
            ],
            Route::Auto,
        )
        .expect("route options are valid");
        assert_eq!(route, Route::NativeRtk);
        assert!(explain);
        assert_eq!(arguments, vec![OsString::from("rg")]);
        assert!(
            parse_wad_options(
                vec![OsString::from("--route"), OsString::from("unsafe")],
                Route::Auto
            )
            .is_err()
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
    fn provider_cache_uses_a_bounded_freshness_window() {
        let entry = ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 100,
            windows: WindowsToolProbe {
                executable: None,
                native_rtk: None,
                executable_identity: None,
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: Vec::new(),
        };
        assert!(cache_entry_is_fresh(
            &entry,
            100 + PROVIDER_CACHE_TTL_SECONDS
        ));
        assert!(!cache_entry_is_fresh(
            &entry,
            101 + PROVIDER_CACHE_TTL_SECONDS
        ));
    }

    #[test]
    fn provider_resolution_requires_a_verified_cross_host_project_mapping() {
        let probe = WslToolProbe {
            distro: "Ubuntu".to_owned(),
            wsl_version: Some(2),
            executable: Some("/usr/bin/go".to_owned()),
            rtk: Some("/home/test/.local/bin/rtk".to_owned()),
            executable_identity: None,
            rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
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
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                kind: ProviderKind::WslRtk,
                distro: Some("Ubuntu-22.04".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: Some("/usr/local/bin/rtk".to_owned()),
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture".to_owned(),
            }],
            recommended: Some(0),
            install: "disabled_in_pd1",
        };
        match go_provider_decision_from_resolution(&config, Route::Raw, resolution) {
            GoProviderDecision::UseWsl {
                route,
                config,
                reason,
            } => {
                assert_eq!(route, Route::Wsl2);
                assert_eq!(config.distro, "Ubuntu-22.04");
                assert_eq!(config.cwd.as_deref(), Some("/mnt/e/work"));
                assert_eq!(config.rtk_path.as_deref(), Some("/usr/local/bin/rtk"));
                assert!(reason.contains("verified project path"));
            }
            _ => panic!("expected verified WSL provider selection"),
        }
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
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            install: "disabled_in_pd1",
        };
        match go_provider_decision_from_resolution(&default_config(), Route::Raw, resolution) {
            GoProviderDecision::Missing { reason } => {
                assert!(reason.contains("Installation is disabled in PD3"));
                assert!(reason.contains("doctor go"));
            }
            _ => panic!("expected a missing-provider diagnostic"),
        }
    }

    #[test]
    fn setup_plan_proposes_only_a_reviewable_windows_go_command() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: Some(r"C:\tools\rtk.exe".to_owned()),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            install: "disabled_in_pd1",
        };
        let plan = setup_go_plan_from_resolution(&resolution, true);
        assert_eq!(plan.status, "planned");
        assert_eq!(plan.proposed_provider, Some("windows-winget"));
        assert_eq!(plan.apply, "unavailable_in_pd4");
        assert_eq!(
            plan.proposed_command,
            Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ])
        );
    }

    #[test]
    fn setup_plan_never_selects_an_installer_when_a_provider_is_ready_or_blocked() {
        let ready = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Go\bin\go.exe".to_owned()),
                    native_rtk: None,
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            install: "disabled_in_pd1",
        };
        let ready_plan = setup_go_plan_from_resolution(&ready, false);
        assert_eq!(ready_plan.status, "ready");
        assert_eq!(ready_plan.proposed_command, None);
        assert_eq!(ready_plan.apply, "not_needed");

        let blocked = ProviderResolution {
            availability: ProviderCacheEntry {
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                ..ready.availability.clone()
            },
            ..ready
        };
        let blocked_plan = setup_go_plan_from_resolution(&blocked, true);
        assert_eq!(blocked_plan.status, "blocked");
        assert_eq!(blocked_plan.proposed_command, None);
        assert_eq!(blocked_plan.apply, "unavailable_in_pd4");
    }

    #[test]
    fn setup_recovery_never_replays_an_installer() {
        let (verified_status, verified_detail) = setup_recovery_outcome(true);
        assert_eq!(verified_status, "recovered_verified");
        assert!(verified_detail.contains("no installer was replayed"));

        let (required_status, required_detail) = setup_recovery_outcome(false);
        assert_eq!(required_status, "recovery_required");
        assert!(required_detail.contains("no installer was replayed"));
    }

    #[test]
    fn cached_windows_go_skips_cross_host_resolution_when_it_is_sufficient() {
        let windows = WindowsToolProbe {
            executable: Some(r"C:\Program Files\Go\bin\go.exe".to_owned()),
            native_rtk: None,
            executable_identity: None,
            native_rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
        };
        assert!(windows_go_is_usable(&windows_project, Route::Raw, &windows));
        assert!(!windows_go_is_usable(
            &windows_project,
            Route::NativeRtk,
            &windows
        ));
        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
        };
        assert!(!windows_go_is_usable(&wsl_project, Route::Raw, &windows));
    }

    #[test]
    fn local_calibration_bootstraps_then_requires_validation_before_stable() {
        assert_eq!(calibration_route_for(None).0, Route::NativeRtk);

        let mut entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            raw_samples_ms: Vec::new(),
            native_samples_ms: vec![30.0],
            native_input_tokens: 100,
            native_saved_tokens: 0,
        };
        assert_eq!(calibration_route_for(Some(&entry)).0, Route::Raw);

        entry.raw_samples_ms.push(10.0);
        entry.native_samples_ms.push(30.0);
        assert_eq!(entry.phase(), "provisional");
        assert_eq!(calibration_route_for(Some(&entry)).0, Route::Raw);

        entry.raw_samples_ms.push(10.0);
        assert_eq!(entry.phase(), "stable");
        assert_eq!(calibration_route_for(Some(&entry)).0, Route::Raw);
    }

    #[test]
    fn local_calibration_prioritizes_measured_token_savings() {
        let entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            raw_samples_ms: vec![10.0, 11.0],
            native_samples_ms: vec![30.0, 31.0],
            native_input_tokens: 100,
            native_saved_tokens: 25,
        };
        assert_eq!(entry.phase(), "stable");
        assert_eq!(entry.selected_route(), Route::NativeRtk);
        assert_eq!(median(&[1.0, 3.0]), Some(2.0));
    }

    #[test]
    fn local_calibration_is_limited_to_safe_command_contracts() {
        assert_eq!(
            calibration_key(&[OsString::from("git"), OsString::from("status")]),
            Some("git:read-only")
        );
        assert_eq!(
            calibration_key(&[OsString::from("rg"), OsString::from("needle")]),
            Some("rg")
        );
        assert_eq!(
            calibration_key(&[
                OsString::from("go"),
                OsString::from("test"),
                OsString::from("./...")
            ]),
            Some("go:test-all")
        );
        assert_eq!(
            calibration_key(&[OsString::from("cargo"), OsString::from("test")]),
            None
        );
        assert_eq!(
            calibration_key(&[OsString::from("git"), OsString::from("commit")]),
            None
        );
    }

    #[test]
    fn local_calibration_signature_does_not_expose_command_text() {
        let arguments = vec![OsString::from("rg"), OsString::from("sensitive value")];
        let signature = calibration_signature(&arguments, r"E:\work");
        assert_eq!(signature.len(), 16);
        assert!(!signature.contains("sensitive"));
        assert_ne!(
            signature,
            calibration_signature(&[OsString::from("rg"), OsString::from("other")], r"E:\work")
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
}
