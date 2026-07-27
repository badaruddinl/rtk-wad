use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

mod adapters;
mod agent;
mod bridge;
mod command_surface;
mod config;
mod dispatcher;
mod metrics;
mod paths;

pub(crate) const PRODUCT_NAME: &str = "XUVA";
pub(crate) const PRODUCT_COMMAND: &str = "xuva";
pub(crate) const LEGACY_COMMAND: &str = "rtk-wad";

#[cfg(test)]
use adapters::windows::apply_command_spec;
#[cfg(test)]
use bridge::decode_wsl_bridge_fields;
use bridge::wsl_bridge_request;
use command_surface::{CommandSurface, command_manifest, command_surface, command_surface_report};
use config::{
    Config, DEFAULT_DISTRO, DEFAULT_WSL1_DISTRO, ExecutionEnvironment, OutputAdapterPreference,
    Route, WslBackend,
};
#[cfg(test)]
use config::{ExecutableProfile, GitMode};
use metrics::{TokenTotals, WadMetrics, wad_data_root};
use paths::windows_path_to_wsl_path;

const ADAPTER_INFO_ARGUMENT: &str = "--adapter-info";
const VERSION_ARGUMENT: &str = "--version";
const EXPLAIN_ROUTE_ARGUMENT: &str = "--explain-route";
const POLICY_ARGUMENT: &str = "policy";
const CALIBRATION_ARGUMENT: &str = "calibration";
const RESOLVE_ARGUMENT: &str = "resolve";
const DOCTOR_ARGUMENT: &str = "doctor";
const WHICH_ARGUMENT: &str = "which";
const SCAN_ARGUMENT: &str = "scan";
const PROVIDER_ARGUMENT: &str = "provider";
const SURFACE_ARGUMENT: &str = "surface";
const SETUP_ARGUMENT: &str = "setup";
const AGENT_ARGUMENT: &str = "agent";
const PROVIDER_CACHE_SCHEMA_VERSION: u32 = 3;
const PROVIDER_CACHE_TTL_SECONDS: u64 = 300;
const ROUTE_POLICY_SCHEMA_VERSION: u32 = 2;
const CALIBRATION_SCHEMA_VERSION: u32 = 2;
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
        printf 'rtk-wad: timed out waiting for lock %s\n' "$lock_path" >&2
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
const PLAN_LAUNCH_SCRIPT: &str = r#"
lock_wait=$1
lock_path=$2
cancel_token=$3
metrics_db_path=$4
extra_path=$5
ready_file=$6
shift 6

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
        printf 'rtk-wad: timed out waiting for lock %s\n' "$lock_path" >&2
        exit 1
    fi
    remaining=$((remaining - 1))
    /bin/sleep 0.1
done
if [ -n "$ready_file" ]; then
    printf 'ready' > "$ready_file"
fi
# Remaining argv is deliberately: KEY=VALUE overlays, executable, then user
# argv. `env` consumes assignments only until the executable; no shell parses
# a user command string.
exec /usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    RTK_DB_PATH="$metrics_db_path" \
    "$@"
"#;
const WSL1_PLAN_LAUNCH_SCRIPT: &str = r#"
metrics_db_path=$1
extra_path=$2
ready_file=$3
shift 3

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
    "$@"
"#;

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

#[cfg(test)]
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

fn trace(message: impl AsRef<str>) {
    if env::var("RTK_WSL_TRACE").as_deref() == Ok("1") {
        eprintln!("rtk-wad: trace: {}", message.as_ref());
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RoutePolicyFile {
    schema_version: u32,
    #[serde(default)]
    manifest_version: String,
    #[serde(default)]
    context_signature: String,
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

#[derive(Serialize)]
struct PolicyContextReport {
    schema_version: u32,
    manifest_version: String,
    context_signature: String,
}

fn policy_context_report(config: &Config) -> PolicyContextReport {
    PolicyContextReport {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: command_manifest().upstream_rtk_version.clone(),
        context_signature: adaptive_context_signature(config),
    }
}

impl RoutePolicyFile {
    fn route_for(&self, key: &str, context_signature: &str) -> Option<Route> {
        let evidence = self.evidence.iter().find(|evidence| evidence.key == key)?;
        if self.schema_version != ROUTE_POLICY_SCHEMA_VERSION
            || self.manifest_version != command_manifest().upstream_rtk_version
            || self.context_signature != context_signature
            || evidence.sample_count < 5
        {
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
    #[serde(default)]
    manifest_version: String,
    #[serde(default)]
    context_signature: String,
    raw_samples_ms: Vec<f64>,
    native_samples_ms: Vec<f64>,
    native_input_tokens: i64,
    native_saved_tokens: i64,
}

#[derive(Debug, Clone)]
struct CalibrationPlan {
    signature: String,
    key: String,
    manifest_version: String,
    context_signature: String,
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
    windows_path: Option<String>,
}

fn print_command_surface(arguments: &[OsString]) -> ExitCode {
    if arguments.len() > 2
        || arguments
            .get(1)
            .is_some_and(|argument| argument != "--json")
    {
        eprintln!("rtk-wad: usage: surface [--json]");
        return ExitCode::FAILURE;
    }
    let report = command_surface_report();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--json")
    {
        return match serde_json::to_string_pretty(&report) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rtk-wad: unable to render command surface: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!(
        "RTK {} command surface: {} upstream command families",
        report.upstream_rtk_version, report.upstream_command_count
    );
    for row in report.commands {
        println!(
            "{}\t{}\t{}",
            row.command,
            row.classification.as_str(),
            row.default_route
        );
    }
    ExitCode::SUCCESS
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct WindowsToolProbe {
    executable: Option<String>,
    native_rtk: Option<String>,
    #[serde(default)]
    executable_version: Option<String>,
    #[serde(default)]
    executable_capabilities: Vec<String>,
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
    executable_version: Option<String>,
    #[serde(default)]
    executable_capabilities: Vec<String>,
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
    #[serde(default)]
    context_signature: String,
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
    diagnosis: String,
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
    wad_data_root().join("provider-cache-v3.json")
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

fn cache_entry_is_fresh(
    entry: &ProviderCacheEntry,
    now: u64,
    context_signature: &str,
    validate_versions: bool,
) -> bool {
    now.saturating_sub(entry.observed_unix_seconds) <= PROVIDER_CACHE_TTL_SECONDS
        && entry.context_signature == context_signature
        && binary_identity_is_current(entry.windows.executable_identity.as_ref())
        && binary_identity_is_current(entry.windows.native_rtk_identity.as_ref())
        && (!validate_versions
            || cached_tool_version_is_current(
                entry.windows.executable.as_deref(),
                entry.windows.executable_version.as_deref(),
                None,
            ))
        && entry.wsl.iter().all(wsl_probe_identities_are_current)
        && (!validate_versions || entry.wsl.iter().all(wsl_probe_version_is_current))
}

fn binary_identity_is_current(identity: Option<&BinaryIdentity>) -> bool {
    identity
        .is_none_or(|identity| windows_binary_identity(&identity.path).as_ref() == Some(identity))
}

fn wsl_binary_identity_is_current(distro: &str, identity: Option<&BinaryIdentity>) -> bool {
    identity.is_none_or(|identity| {
        let output = Command::new("wsl.exe")
            .args([
                "-d",
                distro,
                "--exec",
                "stat",
                "-Lc",
                "%s:%Y",
                &identity.path,
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| first_output_line(&output.stdout));
        parse_wsl_binary_identity(Some(identity.path.clone()), output).as_ref() == Some(identity)
    })
}

fn wsl_probe_identities_are_current(probe: &WslToolProbe) -> bool {
    wsl_binary_identity_is_current(&probe.distro, probe.executable_identity.as_ref())
        && wsl_binary_identity_is_current(&probe.distro, probe.rtk_identity.as_ref())
}

fn cached_tool_version_is_current(
    executable: Option<&str>,
    cached_version: Option<&str>,
    wsl_distro: Option<&str>,
) -> bool {
    match executable {
        None => cached_version.is_none(),
        Some(executable) => tool_version(executable, wsl_distro).as_deref() == cached_version,
    }
}

fn wsl_probe_version_is_current(probe: &WslToolProbe) -> bool {
    cached_tool_version_is_current(
        probe.executable.as_deref(),
        probe.executable_version.as_deref(),
        Some(&probe.distro),
    )
}

fn discovery_context_signature(config: &Config, require_wsl: bool) -> String {
    let path_value = env::var_os("PATH").unwrap_or_default();
    let path_ext_value = env::var_os("PATHEXT").unwrap_or_default();
    let path = path_value.to_string_lossy();
    let path_ext = path_ext_value.to_string_lossy();
    let configured = format!(
        "{}:{}:{}:{}:{}",
        config.distro,
        config.user.as_deref().unwrap_or_default(),
        config.native_rtk_path,
        config.extra_path.as_deref().unwrap_or_default(),
        require_wsl,
    );
    let distros = if require_wsl {
        installed_wsl_distributions()
            .into_iter()
            .map(|(name, version)| format!("{name}:{}", version.unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        String::new()
    };
    let git_revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| first_output_line(&output.stdout))
        .unwrap_or_default();
    stable_signature(&[&path, &path_ext, &configured, &distros, &git_revision])
}

fn stable_signature(parts: &[&str]) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0xff)) {
            state ^= u64::from(byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{state:016x}")
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
        .and_then(|output| {
            let rendered = String::from_utf8_lossy(&output.stdout);
            let candidates = rendered
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            select_windows_executable(candidates)
        })
}

fn select_windows_executable(candidates: Vec<String>) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| is_windows_launchable_path(candidate))
        .or_else(|| candidates.first())
        .cloned()
}

fn is_windows_launchable_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "exe" | "com" | "cmd" | "bat"
            )
        })
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

fn tool_version(executable: &str, wsl_distro: Option<&str>) -> Option<String> {
    ["--version", "version"].into_iter().find_map(|argument| {
        let output = match wsl_distro {
            Some(distro) => Command::new("wsl.exe")
                .args(["-d", distro, "--exec", executable, argument])
                .output()
                .ok(),
            None => Command::new(executable).arg(argument).output().ok(),
        }?;
        output
            .status
            .success()
            .then(|| first_output_line(&output.stdout))
            .flatten()
    })
}

fn version_capabilities(version: &Option<String>) -> Vec<String> {
    version
        .as_ref()
        .map(|_| vec!["version".to_owned()])
        .unwrap_or_default()
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

fn probe_wsl_tool(
    distro: &str,
    wsl_version: Option<u8>,
    tool: &str,
    extra_path: Option<&str>,
) -> WslToolProbe {
    let script = "if [ -n \"$2\" ]; then PATH=\"$2:$PATH\"; fi; tool_path=$(command -v \"$1\" 2>/dev/null || true); rtk_path=$(command -v rtk 2>/dev/null || true); tool_identity=$(stat -Lc '%s:%Y' -- \"$tool_path\" 2>/dev/null || true); rtk_identity=$(stat -Lc '%s:%Y' -- \"$rtk_path\" 2>/dev/null || true); printf '%s\\n%s\\n%s\\n%s\\n' \"$tool_path\" \"$rtk_path\" \"$tool_identity\" \"$rtk_identity\"";
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
        .arg(extra_path.unwrap_or_default())
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
    let executable_version = executable
        .as_deref()
        .and_then(|path| tool_version(path, Some(distro)));
    WslToolProbe {
        distro: distro.to_owned(),
        wsl_version,
        executable_capabilities: version_capabilities(&executable_version),
        executable_version,
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
                    windows_path: None,
                };
            }
        }
    }
    if windows_path_to_wsl_path(path).is_some() {
        ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: path.to_owned(),
            distro: None,
            windows_path: None,
        }
    } else {
        ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: path.to_owned(),
            distro: None,
            windows_path: None,
        }
    }
}

fn current_project_location(config: &Config) -> ProjectLocation {
    if let Some(cwd) = &config.cwd {
        return ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: cwd.clone(),
            distro: Some(config.distro.clone()),
            windows_path: config.bridge_windows_cwd.clone(),
        };
    }
    env::current_dir()
        .map(|path| classify_project_path(&path.to_string_lossy()))
        .unwrap_or(ProjectLocation {
            kind: ProjectLocationKind::Unknown,
            path: String::new(),
            distro: None,
            windows_path: None,
        })
}

fn discover_tool(
    tool: &str,
    config: &Config,
    include_wsl: bool,
    inspect_versions: bool,
) -> ProviderCacheEntry {
    let executable = if tool == "go" { "go.exe" } else { tool };
    let windows_executable = first_windows_executable(executable);
    let native_rtk = configured_windows_executable(&config.native_rtk_path);
    let executable_version = inspect_versions
        .then(|| {
            windows_executable
                .as_deref()
                .and_then(|path| tool_version(path, None))
        })
        .flatten();
    let windows = WindowsToolProbe {
        executable_capabilities: version_capabilities(&executable_version),
        executable_version,
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
        .map(|(distro, version)| {
            probe_wsl_tool(&distro, version, tool, config.extra_path.as_deref())
        })
        .collect();
    ProviderCacheEntry {
        tool: tool.to_owned(),
        observed_unix_seconds: unix_seconds(),
        context_signature: discovery_context_signature(config, include_wsl),
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
    validate_versions: bool,
) -> (ProviderCacheEntry, &'static str) {
    let now = unix_seconds();
    let context_signature = discovery_context_signature(config, require_wsl);
    let mut cache = load_provider_cache();
    if !refresh
        && let Some(entry) = cache.entries.iter().find(|entry| {
            entry.tool == tool
                && cache_entry_is_fresh(entry, now, &context_signature, validate_versions)
                && (!require_wsl || entry.wsl_probe_complete)
        })
    {
        return (entry.clone(), "hit");
    }
    let discovered = discover_tool(tool, config, require_wsl, validate_versions);
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
        ProjectLocationKind::Wsl => project
            .windows_path
            .as_deref()
            .and_then(|path| map_windows_path(&probe.distro, path)),
        ProjectLocationKind::Unknown => None,
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
        ProjectLocationKind::Wsl => project.windows_path.clone().or_else(|| {
            project
                .distro
                .as_deref()
                .and_then(|distro| map_wsl_path(distro, &project.path))
        }),
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
    let (discovery, cache) = cached_or_discovered_tool(tool, config, refresh, true, true);
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
    let diagnosis = recommended.map_or_else(
        || format!(
            "no verified provider is available for {}; run `{PRODUCT_COMMAND} setup {tool}` for a non-installing setup diagnosis",
            tool
        ),
        |index| format!(
            "candidate {index} is verified; run `{PRODUCT_COMMAND} provider exec {tool} -- <args...>` to execute it explicitly"
        ),
    );
    ProviderResolution {
        schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
        tool: tool.to_owned(),
        cache,
        project,
        availability,
        candidates,
        recommended,
        diagnosis,
        install: "disabled_in_p12",
    }
}

enum ProviderDispatchDecision {
    KeepStaticRoute,
    UsePlan {
        plan: Box<dispatcher::ExecutionPlan>,
        fallbacks: Vec<dispatcher::ExecutionPlan>,
        reason: String,
    },
    Missing {
        reason: String,
    },
}

fn is_dispatchable_provider_tool(arguments: &[OsString]) -> bool {
    arguments
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(is_safe_provider_tool_name)
}

fn wsl_route_for_version(version: Option<u8>) -> Option<Route> {
    match version {
        Some(1) => Some(Route::Wsl1),
        Some(2) => Some(Route::Wsl2),
        _ => None,
    }
}

fn windows_tool_is_usable(
    project: &ProjectLocation,
    static_route: Route,
    windows: &WindowsToolProbe,
) -> bool {
    project.kind != ProjectLocationKind::Wsl
        && windows.executable.is_some()
        && match static_route {
            Route::Raw => true,
            Route::NativeRtk => windows.native_rtk.is_some(),
            // WSL routes are legacy route suggestions, not a reason to skip
            // generic candidate resolution when a Windows tool is verified.
            Route::Wsl1 | Route::Wsl2 | Route::Auto => false,
        }
}

fn provider_dispatch_decision(
    arguments: &[OsString],
    config: &Config,
    static_route: Route,
) -> ProviderDispatchDecision {
    if !is_dispatchable_provider_tool(arguments) || has_wsl_path(arguments) {
        return ProviderDispatchDecision::KeepStaticRoute;
    }
    let tool = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .expect("dispatchable provider tools have a safe Unicode name");
    let project = current_project_location(config);
    // A Windows project always probes its native executable first. This keeps
    // an unknown command such as `code`, `nvm`, or a user tool out of WSL when
    // Windows already owns it. Only a missing native candidate expands to the
    // WSL inventory. A WSL project still needs the complete inventory first so
    // its same-distro provider keeps precedence over a compatible Windows one.
    let (windows_discovery, windows_cache) =
        cached_or_discovered_tool(tool, config, false, false, false);
    if project.kind != ProjectLocationKind::Wsl && windows_discovery.windows.executable.is_some() {
        return provider_dispatch_decision_from_resolution(
            arguments,
            config,
            static_route,
            resolve_tool_provider_from_discovery_with_user(
                tool,
                project,
                windows_discovery,
                windows_cache,
                config.user.as_deref(),
            ),
        );
    }
    let (discovery, cache) = cached_or_discovered_tool(tool, config, false, true, true);
    let resolution = resolve_tool_provider_from_discovery_with_user(
        tool,
        project,
        discovery,
        cache,
        config.user.as_deref(),
    );
    provider_dispatch_decision_from_resolution(arguments, config, static_route, resolution)
}

fn provider_dispatch_decision_from_resolution(
    arguments: &[OsString],
    config: &Config,
    static_route: Route,
    resolution: ProviderResolution,
) -> ProviderDispatchDecision {
    let windows_is_usable = windows_tool_is_usable(
        &resolution.project,
        static_route,
        &resolution.availability.windows,
    );
    if windows_is_usable {
        return ProviderDispatchDecision::KeepStaticRoute;
    }
    let eligible = |candidate: &&ProviderCandidate| {
        candidate.usable
            && match candidate.distro.as_deref() {
                None => true,
                Some(_) => wsl_route_for_version(candidate.wsl_version).is_some(),
            }
            && (config.output_adapter != OutputAdapterPreference::Rtk || candidate.rtk.is_some())
    };
    let preferred_wsl = resolution.project.distro.as_deref();
    let ordered_candidates: Vec<&ProviderCandidate> = if resolution.project.kind
        == ProjectLocationKind::Wsl
    {
        resolution
            .candidates
            .iter()
            .filter(|candidate| eligible(candidate) && candidate.distro.as_deref() == preferred_wsl)
            .chain(resolution.candidates.iter().filter(|candidate| {
                eligible(candidate)
                    && candidate.distro.is_some()
                    && candidate.distro.as_deref() != preferred_wsl
            }))
            .chain(
                resolution
                    .candidates
                    .iter()
                    .filter(|candidate| eligible(candidate) && candidate.distro.is_none()),
            )
            .collect()
    } else {
        resolution.candidates.iter().filter(eligible).collect()
    };
    let Some((tool, tool_arguments)) = arguments.split_first() else {
        return ProviderDispatchDecision::Missing {
            reason: "Provider execution request has no executable".to_owned(),
        };
    };
    let Some(tool) = tool.to_str() else {
        return ProviderDispatchDecision::Missing {
            reason: "Provider executable name is not valid Unicode".to_owned(),
        };
    };
    let mut planning_errors = Vec::new();
    let mut planned_candidates = Vec::new();
    for candidate in ordered_candidates {
        match execution_plan_for_provider_candidate(tool, tool_arguments, config, candidate) {
            Ok(plan) => planned_candidates.push((candidate.clone(), plan)),
            Err(error) => planning_errors.push(format!("{}: {error}", candidate.executable)),
        }
    }
    let Some((candidate, plan)) = planned_candidates.first().cloned() else {
        let detail = if planning_errors.is_empty() {
            "no compatible candidate was discovered".to_owned()
        } else {
            planning_errors.join("; ")
        };
        return ProviderDispatchDecision::Missing {
            reason: format!(
                "command `{}` was not found in verified Windows or WSL providers ({detail}); run `{PRODUCT_COMMAND} doctor {}` for details. Installation is disabled in P7.",
                resolution.tool, resolution.tool
            ),
        };
    };
    let fallbacks = planned_candidates
        .into_iter()
        .skip(1)
        .map(|(_, plan)| plan)
        .collect();
    let adapter_name = plan.adapter.as_str();
    let location = candidate
        .distro
        .as_deref()
        .map_or_else(|| "Windows".to_owned(), |distro| format!("WSL {distro}"));
    let reason = format!(
        "on-demand {} discovery selected {} on {} with a verified project path and {} output adapter",
        resolution.tool,
        candidate.kind.as_str(),
        location,
        adapter_name,
    );
    ProviderDispatchDecision::UsePlan {
        plan: Box::new(plan),
        fallbacks,
        reason,
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
        "windows_{}_identity={};version={};capabilities={}",
        resolution.tool,
        binary_identity_display(resolution.availability.windows.executable_identity.as_ref()),
        resolution
            .availability
            .windows
            .executable_version
            .as_deref()
            .unwrap_or("unknown"),
        resolution
            .availability
            .windows
            .executable_capabilities
            .join(","),
    );
    println!(
        "windows_rtk_identity={}",
        binary_identity_display(resolution.availability.windows.native_rtk_identity.as_ref())
    );
    for probe in &resolution.availability.wsl {
        println!(
            "inspected_distro={};wsl_version={};{}_path={};{}_identity={};version={};capabilities={};rtk_path={};rtk_identity={}",
            probe.distro,
            probe
                .wsl_version
                .map_or_else(|| "unknown".to_owned(), |version| version.to_string()),
            resolution.tool,
            probe.executable.as_deref().unwrap_or("missing"),
            resolution.tool,
            binary_identity_display(probe.executable_identity.as_ref()),
            probe.executable_version.as_deref().unwrap_or("unknown"),
            probe.executable_capabilities.join(","),
            probe.rtk.as_deref().unwrap_or("missing"),
            binary_identity_display(probe.rtk_identity.as_ref())
        );
    }
    if resolution.candidates.is_empty() {
        println!("recommended=none");
        if doctor {
            println!("diagnosis={}", resolution.diagnosis);
        }
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
    if doctor {
        println!("diagnosis={}", resolution.diagnosis);
    }
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

fn windows_path_tool_names() -> Vec<String> {
    let mut tools = HashSet::new();
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path) {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_windows_launchable_path(&path.to_string_lossy()) {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|name| name.to_str())
                && is_safe_provider_tool_name(name)
            {
                tools.insert(name.to_ascii_lowercase());
            }
        }
    }
    let mut tools: Vec<_> = tools.into_iter().collect();
    tools.sort_unstable();
    tools
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

fn provider_scan_command(arguments: &[OsString], config: &Config) -> ExitCode {
    if arguments.len() == 1 {
        let windows_tools = windows_path_tool_names();
        let wsl_distros = installed_wsl_distributions()
            .into_iter()
            .map(|(distro, version)| format!("{distro}:{}", version.unwrap_or_default()))
            .collect::<Vec<_>>();
        println!("scan=complete; windows_tools={}", windows_tools.len());
        println!(
            "wsl_distros={}",
            if wsl_distros.is_empty() {
                "none".to_owned()
            } else {
                wsl_distros.join(",")
            }
        );
        println!(
            "provider_cache=on-demand; use `{PRODUCT_COMMAND} scan <tool>...` to refresh named providers"
        );
        return ExitCode::SUCCESS;
    }

    let requested_tools: Vec<&str> = {
        let mut tools = Vec::new();
        for argument in arguments.iter().skip(1) {
            let Some(tool) = argument
                .to_str()
                .filter(|tool| is_safe_provider_tool_name(tool))
            else {
                eprintln!(
                    "rtk-wad: usage: scan [<tool>...]; tool names must contain only ASCII letters, digits, '.', '_', or '-'"
                );
                return ExitCode::FAILURE;
            };
            if !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        tools
    };

    for tool in &requested_tools {
        let resolution = resolve_tool_provider(tool, config, true);
        let recommended = resolution
            .recommended
            .and_then(|index| resolution.candidates.get(index))
            .map_or("missing", |candidate| candidate.kind.as_str());
        println!(
            "tool={tool}; cache={}; candidates={}; recommended={recommended}",
            resolution.cache,
            resolution.candidates.len()
        );
    }
    println!("scan=complete; tools={}", requested_tools.len());
    ExitCode::SUCCESS
}

fn has_complete_go_provider(resolution: &ProviderResolution) -> bool {
    if resolution.project.kind != ProjectLocationKind::Wsl
        && resolution.availability.windows.executable.is_some()
    {
        return true;
    }
    resolution.candidates.iter().any(|candidate| {
        candidate.usable
            && candidate.distro.is_some()
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

fn setup_generic_plan_from_resolution(resolution: &ProviderResolution) -> SetupPlan {
    let verification_command = vec![
        "rtk-wad".to_owned(),
        "doctor".to_owned(),
        resolution.tool.clone(),
        "--refresh".to_owned(),
    ];
    if resolution.recommended.is_some() {
        return SetupPlan {
            schema_version: 1,
            tool: resolution.tool.clone(),
            mode: "diagnostic-only",
            status: "ready",
            reason: "a verified existing provider is available; no setup action is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    SetupPlan {
        schema_version: 1,
        tool: resolution.tool.clone(),
        mode: "diagnostic-only",
        status: "blocked",
        reason: format!(
            "{}; WAD will not guess an installer, package manager, or dependency chain for a generic tool",
            resolution.diagnosis
        ),
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_for_generic_tool",
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
            "rtk-wad: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("rtk-wad: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
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
            "rtk-wad: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    let json = flags.contains(&"--json");
    let refresh = flags.contains(&"--refresh");
    let status = flags.contains(&"--status");
    let recover = flags.contains(&"--recover");
    let apply = flags.contains(&"--apply");
    let confirm = flags.contains(&"--confirm");
    if tool != "go" {
        if status || recover || apply || confirm {
            eprintln!(
                "rtk-wad: generic setup is diagnostic-only; `--apply`, `--confirm`, `--status`, and `--recover` are available only for the explicit Go transaction"
            );
            return ExitCode::FAILURE;
        }
        let resolution = resolve_tool_provider(tool, config, refresh);
        return print_setup_plan(&setup_generic_plan_from_resolution(&resolution), json);
    }
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
        .unwrap_or_else(|| wad_data_root().join("route-policy-v2.json"))
}

fn load_route_policy() -> Option<RoutePolicyFile> {
    let path = wad_policy_path();
    let contents = fs::read_to_string(path).ok()?;
    let policy = serde_json::from_str(&contents).ok()?;
    validate_route_policy(&policy).ok()?;
    Some(policy)
}

fn validate_route_policy(policy: &RoutePolicyFile) -> Result<(), String> {
    if policy.schema_version != ROUTE_POLICY_SCHEMA_VERSION
        || policy.manifest_version != command_manifest().upstream_rtk_version
        || policy.context_signature.len() != 16
        || policy.evidence.is_empty()
    {
        return Err("policy evidence must use the current schema, manifest, context, and non-empty evidence".to_owned());
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
    let RoutePolicyFile {
        manifest_version,
        context_signature,
        evidence: incoming_evidence,
        ..
    } = incoming;
    let mut evidence = existing.map_or_else(Vec::new, |policy| policy.evidence);
    for next in incoming_evidence {
        if let Some(index) = evidence.iter().position(|current| current.key == next.key) {
            evidence[index] = next;
        } else {
            evidence.push(next);
        }
    }
    evidence.sort_by(|left, right| left.key.cmp(&right.key));
    RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version,
        context_signature,
        evidence,
    }
}

fn import_route_policy(source: &Path, config: &Config) -> Result<(), String> {
    let contents = fs::read_to_string(source)
        .map_err(|error| format!("unable to read policy evidence: {error}"))?;
    let incoming: RoutePolicyFile = serde_json::from_str(&contents)
        .map_err(|error| format!("invalid policy evidence: {error}"))?;
    validate_route_policy(&incoming)?;
    let expected_context = adaptive_context_signature(config);
    if incoming.context_signature != expected_context {
        return Err("policy evidence was measured for a different local adapter context; run `rtk-wad policy context` and re-benchmark".to_owned());
    }
    let destination = wad_policy_path();
    let existing = if destination.exists() {
        let contents = fs::read_to_string(&destination)
            .map_err(|error| format!("unable to read existing route policy: {error}"))?;
        let policy = serde_json::from_str(&contents)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        validate_route_policy(&policy)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        if policy.context_signature != incoming.context_signature {
            return Err("existing policy belongs to a different local adapter context; remove or relocate it before importing new evidence".to_owned());
        }
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
    wad_data_root().join("calibration-v2.json")
}

fn validate_calibration(file: &CalibrationFile) -> Result<(), String> {
    if file.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }
    let mut signatures = HashSet::new();
    for entry in &file.entries {
        if entry.signature.len() != 16
            || entry.key.trim().is_empty()
            || entry.manifest_version != command_manifest().upstream_rtk_version
            || entry.context_signature.len() != 16
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
    if file.schema_version == 1 {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
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

fn adaptive_context_signature(config: &Config) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut append = |value: &str| {
        for byte in value.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    append(&command_manifest().upstream_rtk_version);
    append(config.environment.as_str());
    append(&config.native_rtk_path);
    append(&env::var_os("PATH").unwrap_or_default().to_string_lossy());
    format!("{hash:016x}")
}

fn calibration_plan(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: &str,
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
        .and_then(|policy_key| {
            policy.and_then(|policy| policy.route_for(policy_key, context_signature))
        })
        .is_some()
    {
        return Ok(None);
    }
    let signature = calibration_signature(arguments, current_directory);
    let state = load_calibration()?;
    let entry = state
        .entries
        .iter()
        .find(|entry| calibration_entry_matches(entry, &signature, context_signature));
    let (route, reason) = calibration_route_for(entry);
    Ok(Some(CalibrationPlan {
        signature,
        key: key.to_owned(),
        manifest_version: command_manifest().upstream_rtk_version.clone(),
        context_signature: context_signature.to_owned(),
        route,
        reason,
    }))
}

fn calibration_entry_matches(
    entry: &CalibrationEntry,
    signature: &str,
    context_signature: &str,
) -> bool {
    entry.signature == signature
        && entry.manifest_version == command_manifest().upstream_rtk_version
        && entry.context_signature == context_signature
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
        .iter()
        .position(|entry| entry.signature == plan.signature)
    {
        Some(index)
            if state.entries[index].manifest_version == plan.manifest_version
                && state.entries[index].context_signature == plan.context_signature =>
        {
            &mut state.entries[index]
        }
        Some(index) => {
            state.entries[index] = CalibrationEntry {
                signature: plan.signature.clone(),
                key: plan.key.clone(),
                manifest_version: plan.manifest_version.clone(),
                context_signature: plan.context_signature.clone(),
                raw_samples_ms: Vec::new(),
                native_samples_ms: Vec::new(),
                native_input_tokens: 0,
                native_saved_tokens: 0,
            };
            &mut state.entries[index]
        }
        None => {
            state.entries.push(CalibrationEntry {
                signature: plan.signature.clone(),
                key: plan.key.clone(),
                manifest_version: plan.manifest_version.clone(),
                context_signature: plan.context_signature.clone(),
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

fn is_wsl_path(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

#[cfg(test)]
fn git_uses_wsl_directory(arguments: &[OsString]) -> bool {
    arguments.windows(2).any(|pair| {
        (pair[0] == "-C" || pair[0] == "--git-dir" || pair[0] == "--work-tree")
            && is_wsl_path(&pair[1])
    })
}

#[cfg(test)]
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

#[cfg(test)]
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
        OsString::from("rtk-wad"),
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

#[cfg(test)]
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
        OsString::from("rtk-wad-wsl1"),
        OsString::from(config.rtk_path.as_deref().unwrap_or("")),
        OsString::from(metrics_db_path.unwrap_or("")),
        OsString::from(config.extra_path.as_deref().unwrap_or("")),
        OsString::from(test_ready_wsl_path().unwrap_or_default()),
    ]);
    command.extend(forwarded);
    command
}

fn wsl_environment_assignments(
    environment: &[(OsString, OsString)],
) -> Result<Vec<OsString>, std::io::Error> {
    environment
        .iter()
        .map(|(key, value)| {
            let key = key.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable names must be valid Unicode",
                )
            })?;
            let value = value.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable values must be valid Unicode",
                )
            })?;
            let valid_name = key.bytes().enumerate().all(|(index, byte)| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
                b'0'..=b'9' => index > 0,
                _ => false,
            });
            if key.is_empty() || !valid_name {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL environment variable names must use POSIX identifier syntax",
                ));
            }
            Ok(OsString::from(format!("{key}={value}")))
        })
        .collect()
}

fn plan_wsl_arguments_with_metrics(
    executable: &OsString,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    cancel_token: Option<&str>,
    metrics_db_path: Option<&str>,
) -> Result<Vec<OsString>, std::io::Error> {
    let environment = wsl_environment_assignments(environment)?;
    let mut command = wsl_launch_prefix(config);
    match route {
        Route::Wsl1 => {
            command.extend([
                OsString::from("--exec"),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(WSL1_PLAN_LAUNCH_SCRIPT),
                OsString::from("rtk-wad-wsl1-plan"),
                OsString::from(metrics_db_path.unwrap_or("")),
                OsString::from(config.extra_path.as_deref().unwrap_or("")),
                OsString::from(test_ready_wsl_path().unwrap_or_default()),
            ]);
        }
        Route::Wsl2 => {
            let cancel_token = cancel_token.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "WSL2 execution plans require a cancellation token",
                )
            })?;
            command.extend([
                OsString::from("--exec"),
                OsString::from("/usr/bin/setsid"),
                OsString::from("-w"),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(PLAN_LAUNCH_SCRIPT),
                OsString::from("rtk-wad-plan"),
                OsString::from(&config.lock_wait),
                OsString::from(&config.lock_path),
                OsString::from(cancel_token),
                OsString::from(metrics_db_path.unwrap_or("")),
                OsString::from(config.extra_path.as_deref().unwrap_or("")),
                OsString::from(test_ready_wsl_path().unwrap_or_default()),
            ]);
        }
        Route::Auto | Route::Raw | Route::NativeRtk => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "only WSL routes can execute a WSL plan",
            ));
        }
    }
    command.extend(environment);
    command.push(executable.clone());
    command.extend(arguments.iter().cloned());
    Ok(command)
}

fn cancel_token() -> String {
    format!("/tmp/rtk-wad-{}.cancel", std::process::id())
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
        OsString::from("rtk-wad-cancel"),
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
    const MUTEX_NAME: &str = r"Local\rtk-wad-wsl1-global-lock";

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

#[cfg(test)]
fn auto_wad_route(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
) -> (Route, &'static str) {
    auto_wad_route_with_context(arguments, current_directory, policy, None)
}

fn auto_wad_route_with_context(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: Option<&str>,
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
        context_signature
            .and_then(|context| policy.and_then(|policy| policy.route_for(key, context)))
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
    match command_surface(wad_command_family(arguments)) {
        CommandSurface::RawNative => (
            Route::Raw,
            "command manifest selects the validated Windows raw provider",
        ),
        CommandSurface::NativeStructured if wad_command_family(arguments) == "git" => {
            if is_verified_read_only_git(arguments) {
                (
                    Route::NativeRtk,
                    "command manifest permits structured native RTK for read-only Git",
                )
            } else {
                (
                    Route::Raw,
                    "Git mutation is excluded from native RTK auto-routing and executes once with native Git",
                )
            }
        }
        CommandSurface::NativeStructured => (
            Route::NativeRtk,
            "command manifest selects the structured native RTK adapter",
        ),
        CommandSurface::Wsl1Conservative => (
            Route::Wsl1,
            "command manifest retains the conservative isolated Linux RTK contract",
        ),
        CommandSurface::WadInternal => (
            Route::Wsl1,
            "RTK command is internal to WAD only when invoked through its dedicated interface",
        ),
        CommandSurface::Unknown => match wad_command_family(arguments) {
            "dart" | "flutter" => (
                Route::Raw,
                "WAD-owned Windows SDK shim executes once without an RTK adapter",
            ),
            _ => (
                Route::Wsl1,
                "unknown command has no manifest contract; use isolated Linux RTK",
            ),
        },
    }
}

fn is_rtk_meta_command(command: &str) -> bool {
    matches!(
        command,
        "smart"
            | "err"
            | "test"
            | "json"
            | "deps"
            | "env"
            | "log"
            | "summary"
            | "init"
            | "wget"
            | "wc"
            | "cc-economics"
            | "config"
            | "discover"
            | "session"
            | "telemetry"
            | "learn"
            | "run"
            | "proxy"
            | "pipe"
            | "trust"
            | "untrust"
            | "verify"
            | "hook-audit"
            | "rewrite"
            | "hook"
    )
}

fn auto_wad_route_for_environment(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: Option<&str>,
    environment: ExecutionEnvironment,
) -> (Route, &'static str) {
    if environment == ExecutionEnvironment::Adaptive {
        return auto_wad_route_with_context(
            arguments,
            current_directory,
            policy,
            context_signature,
        );
    }

    let command = wad_command_family(arguments);
    if is_rtk_meta_command(command) || command_surface(command) == CommandSurface::WadInternal {
        return (
            Route::NativeRtk,
            "windows-only environment requires native RTK for an RTK meta command",
        );
    }
    match command_surface(command) {
        CommandSurface::NativeStructured
            if command == "git" && !is_verified_read_only_git(arguments) =>
        {
            (
                Route::Raw,
                "windows-only environment executes Git mutation once with native Git",
            )
        }
        CommandSurface::NativeStructured => (
            Route::NativeRtk,
            "windows-only environment selects the structured native RTK adapter",
        ),
        CommandSurface::RawNative | CommandSurface::Wsl1Conservative | CommandSurface::Unknown => (
            Route::Raw,
            "windows-only environment disables automatic WSL routing and uses the native command",
        ),
        CommandSurface::WadInternal => unreachable!("WAD internal commands were handled above"),
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
    println!("adapter={PRODUCT_COMMAND}");
    println!("command={PRODUCT_COMMAND}");
    println!("legacy_command={LEGACY_COMMAND}");
    println!("profile={}", config.profile.as_str());
    println!("route_preference={}", config.wad_route.as_str());
    println!("environment={}", config.environment.as_str());
    println!("native_rtk_path={}", config.native_rtk_path);
    println!("metrics=local-aggregate-only");
}

fn run_native_rtk(
    arguments: &[OsString],
    config: &Config,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    adapters::windows::run_rtk_at(&config.native_rtk_path, arguments, None, metrics)
}

fn has_foreign_absolute_path(arguments: &[OsString], route: &dispatcher::RouteCandidate) -> bool {
    match route {
        dispatcher::RouteCandidate::Windows { .. } => arguments.iter().any(is_wsl_path),
        dispatcher::RouteCandidate::Wsl1 { .. } | dispatcher::RouteCandidate::Wsl2 { .. } => {
            arguments.iter().any(|argument| {
                argument
                    .to_str()
                    .and_then(windows_path_to_wsl_path)
                    .is_some()
            })
        }
    }
}

fn execution_route(route: &dispatcher::RouteCandidate) -> Route {
    match route {
        dispatcher::RouteCandidate::Windows { .. } => Route::Raw,
        dispatcher::RouteCandidate::Wsl1 { .. } => Route::Wsl1,
        dispatcher::RouteCandidate::Wsl2 { .. } => Route::Wsl2,
    }
}

fn provider_adapter(
    candidate: &ProviderCandidate,
    preference: OutputAdapterPreference,
) -> Result<dispatcher::OutputAdapter, std::io::Error> {
    match (preference, candidate.rtk.as_deref()) {
        (OutputAdapterPreference::Raw, _) | (OutputAdapterPreference::Auto, None) => {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Auto | OutputAdapterPreference::Rtk, Some(executable)) => {
            Ok(dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(executable),
            })
        }
        (OutputAdapterPreference::Rtk, None) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "RTK output adapter was requested but this provider has no RTK executable",
        )),
    }
}

fn provider_execution_config(
    config: &Config,
    route: &dispatcher::RouteCandidate,
    adapter: &dispatcher::OutputAdapter,
) -> Result<Config, std::io::Error> {
    let (wsl_route, distro, cwd, raw_executable) = match route {
        dispatcher::RouteCandidate::Wsl1 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl1, distro, cwd, executable),
        dispatcher::RouteCandidate::Wsl2 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl2, distro, cwd, executable),
        dispatcher::RouteCandidate::Windows { .. } => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a Windows execution plan has no WSL transport configuration",
            ));
        }
    };
    let mut selected = configured_wsl_backend(config, wsl_route);
    selected.distro = distro.clone();
    selected.cwd = Some(cwd.to_string_lossy().into_owned());
    selected.rtk_path = Some(match adapter {
        dispatcher::OutputAdapter::Raw => raw_executable.to_string_lossy().into_owned(),
        dispatcher::OutputAdapter::Rtk { executable } => executable.to_string_lossy().into_owned(),
    });
    Ok(selected)
}

fn execution_plan_for_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
) -> Result<dispatcher::ExecutionPlan, std::io::Error> {
    let cwd = candidate.project_path.as_deref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider candidate has no verified project directory",
        )
    })?;
    let adapter = provider_adapter(candidate, config.output_adapter)?;
    let request = dispatcher::CommandSpec {
        executable: OsString::from(tool),
        arguments: arguments.to_vec(),
        cwd: Some(PathBuf::from(cwd)),
        environment: Vec::new(),
        interactive: false,
    };
    let route = match (candidate.distro.as_deref(), candidate.wsl_version) {
        (None, _) => dispatcher::RouteCandidate::Windows {
            executable: OsString::from(&candidate.executable),
            cwd: Some(PathBuf::from(cwd)),
        },
        (Some(distro), Some(1)) => dispatcher::RouteCandidate::Wsl1 {
            distro: distro.to_owned(),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
        (Some(distro), Some(2)) => dispatcher::RouteCandidate::Wsl2 {
            distro: distro.to_owned(),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
        (Some(_), _) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "WSL provider has no supported WSL version",
            ));
        }
    };
    Ok(dispatcher::ExecutionPlan {
        request,
        candidate: route,
        adapter,
        explanation: vec![dispatcher::DecisionReason(candidate.reason.clone())],
    })
}

fn run_execution_plan(
    plan: &dispatcher::ExecutionPlan,
    config: &Config,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let rtk_arguments = || {
        let mut forwarded = Vec::with_capacity(plan.request.arguments.len() + 1);
        forwarded.push(plan.request.executable.clone());
        forwarded.extend(plan.request.arguments.iter().cloned());
        forwarded
    };
    match (&plan.candidate, &plan.adapter) {
        (
            dispatcher::RouteCandidate::Windows { executable, .. },
            dispatcher::OutputAdapter::Raw,
        ) => adapters::windows::run_plan(executable, &plan.request),
        (
            dispatcher::RouteCandidate::Windows { .. },
            dispatcher::OutputAdapter::Rtk { executable },
        ) => adapters::windows::run_rtk_plan(executable, &rtk_arguments(), &plan.request, metrics),
        (
            dispatcher::RouteCandidate::Wsl1 { .. } | dispatcher::RouteCandidate::Wsl2 { .. },
            adapter,
        ) => {
            let selected = provider_execution_config(config, &plan.candidate, adapter)?;
            let forwarded = match adapter {
                dispatcher::OutputAdapter::Raw => plan.request.arguments.clone(),
                dispatcher::OutputAdapter::Rtk { .. } => rtk_arguments(),
            };
            let raw_executable = match &plan.candidate {
                dispatcher::RouteCandidate::Wsl1 { executable, .. }
                | dispatcher::RouteCandidate::Wsl2 { executable, .. } => executable,
                dispatcher::RouteCandidate::Windows { .. } => {
                    unreachable!("WSL arm has a WSL candidate")
                }
            };
            let executable = match adapter {
                dispatcher::OutputAdapter::Raw => raw_executable,
                dispatcher::OutputAdapter::Rtk { executable } => executable,
            };
            let measured = matches!(adapter, dispatcher::OutputAdapter::Rtk { .. })
                .then_some(metrics)
                .flatten();
            run_wsl_execution_plan(
                executable,
                &forwarded,
                &plan.request.environment,
                &selected,
                execution_route(&plan.candidate),
                measured,
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
    let plan = match execution_plan_for_provider_candidate(tool, forwarded, config, candidate) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!(
                "rtk-wad: provider candidate {index} cannot produce an execution plan: {error}"
            );
            return ExitCode::from(127);
        }
    };
    if has_foreign_absolute_path(forwarded, &plan.candidate) {
        eprintln!(
            "rtk-wad: provider execution does not translate foreign absolute arguments; run from the verified project directory with relative paths"
        );
        return ExitCode::FAILURE;
    }
    let route = execution_route(&plan.candidate);
    let needs_console_handler = matches!(route, Route::Wsl1 | Route::Wsl2);
    if needs_console_handler && !console::install() {
        eprintln!("rtk-wad: unable to register the Windows console cancellation handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let metrics = match if matches!(plan.adapter, dispatcher::OutputAdapter::Raw) {
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
    let result = run_execution_plan(&plan, config, metrics.as_ref());
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
        if let Err(error) = metrics.finish(
            route.as_str(),
            &command_family,
            started.elapsed(),
            exit_code,
        ) {
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
        adapters::wsl1::process(wsl1_rtk_arguments_with_metrics(
            arguments,
            config,
            metrics_path.as_deref(),
        ))
        .spawn()
        .and_then(|child| wait_for_wsl1_child(child, config))
    } else {
        let token = cancel_token();
        adapters::wsl2::process(rtk_arguments_with_metrics(
            arguments,
            config,
            &token,
            metrics_path.as_deref(),
        ))
        .spawn()
        .and_then(|child| wait_for_wsl_child(child, config, &token))
    }
}

fn run_wsl_execution_plan(
    executable: &OsString,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    config: &Config,
    route: Route,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let metrics_path = metrics.and_then(|metrics| {
        windows_path_to_wsl_path(&metrics.scratch_windows_path().to_string_lossy())
    });
    if route == Route::Wsl1 {
        let _lock = windows_lock::acquire(&config.lock_wait).map_err(std::io::Error::other)?;
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            None,
            metrics_path.as_deref(),
        )?;
        adapters::wsl1::process(command)
            .spawn()
            .and_then(|child| wait_for_wsl1_child(child, config))
    } else {
        let token = cancel_token();
        let command = plan_wsl_arguments_with_metrics(
            executable,
            arguments,
            environment,
            config,
            route,
            Some(&token),
            metrics_path.as_deref(),
        )?;
        adapters::wsl2::process(command)
            .spawn()
            .and_then(|child| wait_for_wsl_child(child, config, &token))
    }
}

fn parse_wad_options(
    mut arguments: Vec<OsString>,
    configured: Route,
    configured_environment: ExecutionEnvironment,
) -> Result<(Vec<OsString>, Route, ExecutionEnvironment, bool), String> {
    let mut route = configured;
    let mut environment = configured_environment;
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
            Some("--environment") => {
                if arguments.len() < 2 {
                    return Err("--environment requires adaptive or windows-only".to_owned());
                }
                environment = ExecutionEnvironment::parse(&arguments[1].to_string_lossy())?;
                arguments.drain(0..2);
            }
            Some(EXPLAIN_ROUTE_ARGUMENT) => {
                explain = true;
                arguments.remove(0);
            }
            _ => return Ok((arguments, route, environment, explain)),
        }
    }
}

fn is_version_command(arguments: &[OsString]) -> bool {
    arguments.len() == 1
        && matches!(
            arguments[0].to_str(),
            Some(VERSION_ARGUMENT | "version" | "-V")
        )
}

fn wad_main(arguments: Vec<OsString>, config: &Config) -> ExitCode {
    if is_version_command(&arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == AGENT_ARGUMENT)
    {
        return agent::command(&arguments, &config.native_rtk_path);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SURFACE_ARGUMENT)
    {
        return print_command_surface(&arguments);
    }
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
        .is_some_and(|argument| argument == WHICH_ARGUMENT)
    {
        let mut resolve_arguments = arguments.clone();
        resolve_arguments[0] = OsString::from(RESOLVE_ARGUMENT);
        return provider_command(&resolve_arguments, config, false);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == DOCTOR_ARGUMENT)
    {
        return provider_command(&arguments, config, true);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == SCAN_ARGUMENT)
    {
        return provider_scan_command(&arguments, config);
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
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "context")
            && arguments.len() == 2
        {
            return match serde_json::to_string_pretty(&policy_context_report(config)) {
                Ok(rendered) => {
                    println!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rtk-wad: unable to render policy context: {error}");
                    ExitCode::FAILURE
                }
            };
        }
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
            return match import_route_policy(Path::new(&arguments[2]), config) {
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
        eprintln!(
            "{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} policy [show|context] | policy import <evidence.json>"
        );
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
        eprintln!("{PRODUCT_COMMAND}: usage: {PRODUCT_COMMAND} calibration [show]");
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
    let (arguments, requested_route, environment, explain) =
        match parse_wad_options(arguments, config.wad_route, config.environment) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("rtk-wad: {error}");
                return ExitCode::FAILURE;
            }
        };
    let mut invocation_config = config.clone();
    invocation_config.environment = environment;
    let current_directory = env::current_dir().ok();
    let started = Instant::now();
    let adaptive_context = adaptive_context_signature(&invocation_config);
    let policy = load_route_policy();
    let (initial_route, initial_reason) = if requested_route == Route::Auto {
        auto_wad_route_for_environment(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            policy.as_ref(),
            Some(&adaptive_context),
            environment,
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
            &adaptive_context,
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
    let selected_config = configured_wsl_backend(&invocation_config, route);
    let mut provider_missing = None;
    let mut execution_plan = None;
    let mut fallback_execution_plans = Vec::new();
    let mut selected_adapter = match route {
        Route::Raw => dispatcher::OutputAdapter::Raw,
        Route::NativeRtk | Route::Wsl1 | Route::Wsl2 | Route::Auto => {
            dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(&selected_config.native_rtk_path),
            }
        }
    };
    if requested_route == Route::Auto && environment == ExecutionEnvironment::Adaptive {
        match provider_dispatch_decision(&arguments, &invocation_config, route) {
            ProviderDispatchDecision::KeepStaticRoute => {}
            ProviderDispatchDecision::UsePlan {
                plan,
                fallbacks,
                reason: provider_reason,
            } => {
                route = execution_route(&plan.candidate);
                selected_adapter = plan.adapter.clone();
                execution_plan = Some(*plan);
                fallback_execution_plans = fallbacks;
                reason = provider_reason;
            }
            ProviderDispatchDecision::Missing {
                reason: missing_reason,
            } => {
                provider_missing = Some(missing_reason.clone());
                reason = missing_reason;
            }
        }
    }
    if explain {
        println!("route={}", route.as_str());
        println!("output_adapter={}", selected_adapter.as_str());
        println!("reason={reason}");
        println!("command_family={}", wad_command_family(&arguments));
        return if provider_missing.is_some() {
            ExitCode::from(127)
        } else {
            ExitCode::SUCCESS
        };
    }
    if arguments.is_empty() {
        eprintln!(
            "{PRODUCT_COMMAND}: no command supplied; use {PRODUCT_COMMAND} --adapter-info for configuration"
        );
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
    let metrics = match if matches!(selected_adapter, dispatcher::OutputAdapter::Raw) {
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
    let result = if let Some(plan) = execution_plan.as_ref() {
        let mut result = run_execution_plan(plan, &invocation_config, metrics.as_ref());
        for fallback in &fallback_execution_plans {
            if !result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                break;
            }
            let fallback_route = execution_route(&fallback.candidate);
            trace(format!(
                "selected provider executable was unavailable before child start; retrying {} candidate",
                fallback_route.as_str()
            ));
            if matches!(fallback_route, Route::Wsl1 | Route::Wsl2) && !console_installed {
                if !console::install() {
                    eprintln!(
                        "rtk-wad: unable to register the Windows console cancellation handler for provider fallback"
                    );
                    return ExitCode::FAILURE;
                }
                console_installed = true;
            }
            executed_route = fallback_route;
            result = run_execution_plan(fallback, &invocation_config, metrics.as_ref());
        }
        result
    } else {
        match route {
            Route::Raw => adapters::windows::run(&arguments),
            Route::NativeRtk => {
                match run_native_rtk(&arguments, &selected_config, metrics.as_ref()) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && requested_route == Route::Auto
                            && environment == ExecutionEnvironment::Adaptive =>
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
                        let fallback_config =
                            configured_wsl_backend(&invocation_config, Route::Wsl1);
                        run_wsl_route(
                            arguments.clone(),
                            &fallback_config,
                            Route::Wsl1,
                            metrics.as_ref(),
                        )
                    }
                    result => result,
                }
            }
            Route::Wsl1 | Route::Wsl2 => {
                run_wsl_route(arguments.clone(), &selected_config, route, metrics.as_ref())
            }
            Route::Auto => unreachable!("auto route is resolved before execution"),
        }
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
            executed_route.as_str(),
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
    let original_arguments: Vec<OsString> = env::args_os().skip(1).collect();
    // This is intentionally before bridge decoding and environment parsing:
    // a local version query must remain instant even when WSL is unavailable
    // or a caller has an invalid dispatcher configuration.
    if is_version_command(&original_arguments) {
        println!("{PRODUCT_COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let bridge = match wsl_bridge_request(&original_arguments) {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("rtk-wad: invalid WSL bridge payload: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("rtk-wad: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    let arguments = if let Some(bridge) = bridge {
        config.distro = bridge.distro;
        config.cwd = Some(bridge.cwd);
        config.bridge_windows_cwd = bridge.windows_cwd;
        config.extra_path = bridge.extra_path;
        config.output_adapter = bridge.output_adapter;
        bridge.arguments
    } else {
        original_arguments
    };
    wad_main(arguments, &config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config::from_lookup(|_| None).expect("default config is valid")
    }

    #[test]
    fn version_commands_are_owned_by_the_dispatcher() {
        for argument in [VERSION_ARGUMENT, "version", "-V"] {
            assert!(
                is_version_command(&[OsString::from(argument)]),
                "{argument}"
            );
        }
        assert!(!is_version_command(&[
            OsString::from("go"),
            OsString::from("version")
        ]));
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
    fn wsl_bridge_payload_preserves_literal_utf8_argv_without_shell_parsing() {
        let fields = decode_wsl_bridge_fields("Z28AcnVuAHNwYWNlICYgJGRvbGxhclzmvKLlrZcA")
            .expect("valid base64 payload decodes");
        assert_eq!(
            fields,
            vec![
                "go".to_owned(),
                "run".to_owned(),
                "space & $dollar\\\u{6f22}\u{5b57}".to_owned(),
            ]
        );
        assert!(decode_wsl_bridge_fields("Z28=").is_err());
        assert!(decode_wsl_bridge_fields("not base64!").is_err());
    }

    #[test]
    fn wsl_bridge_request_carries_context_and_arguments_without_environment() {
        let request = wsl_bridge_request(&[OsString::from(
            "--wsl-bridge=djIAVWJ1bnR1AC90bXAARDpcZml4dHVyZQAvdG1wL2ZpeHR1cmUAcmF3AC0tZXhwbGFpbi1yb3V0ZQBnbwBydW4AeAA=",
        )])
        .expect("bridge payload is valid")
        .expect("argument selects the bridge");
        assert_eq!(request.distro, "Ubuntu");
        assert_eq!(request.cwd, "/tmp");
        assert_eq!(request.windows_cwd.as_deref(), Some(r"D:\fixture"));
        assert_eq!(request.extra_path.as_deref(), Some("/tmp/fixture"));
        assert_eq!(request.output_adapter, OutputAdapterPreference::Raw);
        assert_eq!(
            request.arguments,
            vec![
                OsString::from("--explain-route"),
                OsString::from("go"),
                OsString::from("run"),
                OsString::from("x"),
            ]
        );
    }

    #[test]
    fn wsl_plan_launcher_forwards_environment_as_structured_assignments() {
        let config = default_config();
        let arguments = plan_wsl_arguments_with_metrics(
            &OsString::from("/tmp/go"),
            &[OsString::from("run"), OsString::from("$literal & text")],
            &[(
                OsString::from("P7_OVERLAY"),
                OsString::from("value with spaces"),
            )],
            &config,
            Route::Wsl2,
            Some("/tmp/rtk-wad-plan-test.cancel"),
            None,
        )
        .expect("WSL plan arguments are valid");
        let executable = arguments
            .iter()
            .position(|argument| argument == "/tmp/go")
            .expect("plan includes executable");
        let overlay = arguments
            .iter()
            .position(|argument| argument == "P7_OVERLAY=value with spaces")
            .expect("plan includes environment overlay");
        let user_argument = arguments
            .iter()
            .position(|argument| argument == "$literal & text")
            .expect("plan includes literal user argument");
        assert!(arguments.contains(&OsString::from(PLAN_LAUNCH_SCRIPT)));
        assert!(overlay < executable && executable < user_argument);
        assert!(
            wsl_environment_assignments(&[(
                OsString::from("INVALID-NAME"),
                OsString::from("value"),
            )])
            .is_err()
        );
    }

    #[test]
    fn execution_plan_applies_command_environment_and_cwd_to_windows_processes() {
        let request = dispatcher::CommandSpec {
            executable: OsString::from("fixture.exe"),
            arguments: vec![OsString::from("space value"), OsString::from("$literal")],
            cwd: Some(PathBuf::from(r"E:\work")),
            environment: vec![(OsString::from("P7_OVERLAY"), OsString::from("enabled"))],
            interactive: true,
        };
        let mut command = Command::new("fixture.exe");
        apply_command_spec(&mut command, &request);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![&OsString::from("space value"), &OsString::from("$literal")]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new(r"E:\work")));
        assert!(command.get_envs().any(|(key, value)| {
            key == "P7_OVERLAY" && value == Some(std::ffi::OsStr::new("enabled"))
        }));
    }

    #[test]
    fn explicit_wsl1_route_uses_the_windows_mutex_without_redundant_linux_locking() {
        let config = Config::from_lookup(|name| match name {
            "RTK_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
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
    fn explicit_wsl1_backend_selects_the_isolated_distro_without_affecting_default_wad() {
        let default = default_config();
        assert_eq!(default.backend, WslBackend::Auto);
        assert_eq!(default.distro, DEFAULT_DISTRO);

        let wsl1 = Config::from_lookup(|name| match name {
            "RTK_WSL_BACKEND" => Some("wsl1".to_owned()),
            _ => None,
        })
        .expect("explicit WSL1 configuration is valid");
        assert_eq!(wsl1.backend, WslBackend::Wsl1);
        assert_eq!(wsl1.distro, DEFAULT_WSL1_DISTRO);
    }

    #[test]
    fn explicit_backend_and_distro_select_the_wad_wsl_provider() {
        let config = Config::from_lookup(|name| match name {
            "RTK_WSL_BACKEND" => Some("wsl2".to_owned()),
            "RTK_WSL_DISTRO" => Some("Ubuntu-24.04".to_owned()),
            _ => None,
        })
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
    fn canonical_wad_configuration_is_adaptive_by_default() {
        let wad = default_config();
        assert_eq!(wad.profile, ExecutableProfile::Wad);
        assert_eq!(wad.backend, WslBackend::Auto);
        assert_eq!(wad.wad_route, Route::Auto);
    }

    #[test]
    fn embedded_command_surface_is_complete_and_non_overlapping() {
        let report = command_surface_report();
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.upstream_rtk_version, "0.43.0");
        assert_eq!(report.upstream_command_count, 69);
        let names = report
            .commands
            .iter()
            .map(|row| row.command.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), report.upstream_command_count);
        assert!(
            report
                .commands
                .iter()
                .all(|row| row.classification != CommandSurface::Unknown)
        );
        assert_eq!(command_surface("git"), CommandSurface::NativeStructured);
        assert_eq!(command_surface("go"), CommandSurface::RawNative);
        assert_eq!(command_surface("proxy"), CommandSurface::Wsl1Conservative);
        assert_eq!(command_surface("gain"), CommandSurface::WadInternal);
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
        let context = adaptive_context_signature(&default_config());
        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: context.clone(),
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
            auto_wad_route_with_context(
                &[OsString::from("git"), OsString::from("status")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route_with_context(
                &[OsString::from("rg"), OsString::from("needle")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context)
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route_with_context(
                &[OsString::from("cargo"), OsString::from("check")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context)
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route_with_context(
                &[OsString::from("npm"), OsString::from("run")],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context)
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route_with_context(
                &[
                    OsString::from("go"),
                    OsString::from("test"),
                    OsString::from("./...")
                ],
                Some(r"E:\work"),
                Some(&policy),
                Some(&context)
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
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: "0123456789abcdef".to_owned(),
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
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: "0123456789abcdef".to_owned(),
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
        assert_eq!(
            merged.route_for("cargo:check", "0123456789abcdef"),
            Some(Route::Raw)
        );
        assert_eq!(
            merged.route_for("npm:run-list", "0123456789abcdef"),
            Some(Route::Raw)
        );
    }

    #[test]
    fn adaptive_evidence_is_bound_to_manifest_and_local_adapter_context() {
        let default = default_config();
        let context = adaptive_context_signature(&default);
        let mut different = default.clone();
        different.native_rtk_path = r"C:\tools\other-rtk.exe".to_owned();
        assert_ne!(context, adaptive_context_signature(&different));

        let policy = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: context.clone(),
            evidence: vec![RoutePolicyEvidence {
                key: "rg".to_owned(),
                raw_median_ms: 10.0,
                candidate_median_ms: 20.0,
                token_savings_percent: 0.0,
                sample_count: 5,
            }],
        };
        assert_eq!(policy.route_for("rg", &context), Some(Route::Raw));
        assert_eq!(policy.route_for("rg", "0123456789abcdef"), None);

        let entry = CalibrationEntry {
            signature: "fedcba9876543210".to_owned(),
            key: "rg".to_owned(),
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: context.clone(),
            raw_samples_ms: vec![1.0],
            native_samples_ms: vec![2.0],
            native_input_tokens: 0,
            native_saved_tokens: 0,
        };
        assert!(calibration_entry_matches(
            &entry,
            "fedcba9876543210",
            &context
        ));
        assert!(!calibration_entry_matches(
            &entry,
            "fedcba9876543210",
            "0123456789abcdef"
        ));
    }

    #[test]
    fn wad_route_options_are_explicit_and_validate_values() {
        let (arguments, route, environment, explain) = parse_wad_options(
            vec![
                OsString::from("--route"),
                OsString::from("native-rtk"),
                OsString::from("--explain-route"),
                OsString::from("rg"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("route options are valid");
        assert_eq!(route, Route::NativeRtk);
        assert_eq!(environment, ExecutionEnvironment::Adaptive);
        assert!(explain);
        assert_eq!(arguments, vec![OsString::from("rg")]);
        assert!(
            parse_wad_options(
                vec![OsString::from("--route"), OsString::from("unsafe")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );

        let (arguments, route, environment, explain) = parse_wad_options(
            vec![
                OsString::from("--environment"),
                OsString::from("windows-only"),
                OsString::from("pytest"),
            ],
            Route::Auto,
            ExecutionEnvironment::Adaptive,
        )
        .expect("windows-only option is valid");
        assert_eq!(arguments, vec![OsString::from("pytest")]);
        assert_eq!(route, Route::Auto);
        assert_eq!(environment, ExecutionEnvironment::WindowsOnly);
        assert!(!explain);
        assert!(
            parse_wad_options(
                vec![OsString::from("--environment"), OsString::from("hybrid")],
                Route::Auto,
                ExecutionEnvironment::Adaptive,
            )
            .is_err()
        );
    }

    #[test]
    fn windows_only_routes_external_commands_raw_and_keeps_rtk_meta_native() {
        assert_eq!(
            auto_wad_route_for_environment(
                &[OsString::from("pytest"), OsString::from("-q")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
            )
            .0,
            Route::Raw
        );
        assert_eq!(
            auto_wad_route_for_environment(
                &[OsString::from("init"), OsString::from("-g")],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
            )
            .0,
            Route::NativeRtk
        );
        assert_eq!(
            auto_wad_route_for_environment(
                &[
                    OsString::from("git"),
                    OsString::from("commit"),
                    OsString::from("-m"),
                    OsString::from("x")
                ],
                Some(r"E:\work"),
                None,
                None,
                ExecutionEnvironment::WindowsOnly,
            )
            .0,
            Route::Raw
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
    fn windows_provider_discovery_recognizes_native_launchable_extensions() {
        assert!(is_windows_launchable_path(r"C:\tools\go.exe"));
        assert!(is_windows_launchable_path(r"C:\tools\npm.cmd"));
        assert!(is_windows_launchable_path(r"C:\tools\gradle.bat"));
        assert!(is_windows_launchable_path(r"C:\tools\legacy.com"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm"));
        assert!(!is_windows_launchable_path(r"C:\tools\npm.ps1"));
        assert_eq!(
            select_windows_executable(vec![
                r"C:\tools\npm".to_owned(),
                r"C:\tools\npm.cmd".to_owned(),
                r"C:\tools\npm.ps1".to_owned(),
            ]),
            Some(r"C:\tools\npm.cmd".to_owned())
        );
    }

    #[test]
    fn provider_cache_uses_a_bounded_freshness_window() {
        let entry = ProviderCacheEntry {
            tool: "go".to_owned(),
            observed_unix_seconds: 100,
            context_signature: "fixture".to_owned(),
            windows: WindowsToolProbe {
                executable: None,
                native_rtk: None,
                executable_version: None,
                executable_capabilities: Vec::new(),
                executable_identity: None,
                native_rtk_identity: None,
            },
            wsl_probe_complete: true,
            wsl: Vec::new(),
        };
        assert!(cache_entry_is_fresh(
            &entry,
            100 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
        assert!(
            !cache_entry_is_fresh(&entry, 100, "changed-path-or-git-revision", true),
            "a changed discovery fingerprint invalidates even a new entry"
        );
        assert!(!cache_entry_is_fresh(
            &entry,
            101 + PROVIDER_CACHE_TTL_SECONDS,
            "fixture",
            true
        ));
    }

    #[test]
    fn provider_cache_fingerprint_changes_with_wsl_extra_path() {
        let default = default_config();
        let configured = Config::from_lookup(|name| match name {
            "RTK_WSL_EXTRA_PATH" => Some("/tmp/rtk-wad-go/bin".to_owned()),
            _ => None,
        })
        .expect("extra path configuration is valid");
        assert_ne!(
            discovery_context_signature(&default, false),
            discovery_context_signature(&configured, false),
            "changing the executable search overlay must invalidate discovery"
        );
    }

    #[test]
    fn provider_resolution_requires_a_verified_cross_host_project_mapping() {
        let probe = WslToolProbe {
            distro: "Ubuntu".to_owned(),
            wsl_version: Some(2),
            executable: Some("/usr/bin/go".to_owned()),
            rtk: Some("/home/test/.local/bin/rtk".to_owned()),
            executable_version: None,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
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
            windows_path: None,
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

        let bridged_other_distro_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/mnt/host/d/work".to_owned(),
            distro: Some("docker-desktop".to_owned()),
            windows_path: Some(r"D:\work".to_owned()),
        };
        assert_eq!(
            wsl_project_path_with(
                &bridged_other_distro_project,
                &probe,
                |distro, path| {
                    assert_eq!(distro, "Ubuntu");
                    assert_eq!(path, r"D:\work");
                    Some("/mnt/d/work".to_owned())
                },
                |distro, path| distro == "Ubuntu" && path == "/mnt/d/work",
            ),
            Some("/mnt/d/work".to_owned()),
            "a WSL-origin bridge may cross distros only through a verified Windows-mounted path"
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
            windows_path: None,
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
            windows_path: None,
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
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
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
            diagnosis: "fixture: a verified WSL provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter.as_str(), "rtk");
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref distro, ref cwd, .. }
                        if distro == "Ubuntu-22.04" && cwd == Path::new("/mnt/e/work")
                ));
                assert!(reason.contains("verified project path"));
            }
            _ => panic!("expected verified WSL provider selection"),
        }
    }

    #[test]
    fn provider_aware_go_routing_runs_a_wsl_only_go_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "RTK_WAD_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                kind: ProviderKind::WslRaw,
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/usr/local/go/bin/go".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Go exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };
        assert!(
            has_complete_go_provider(&resolution),
            "a verified WSL raw Go binary is ready and must not trigger setup"
        );
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/usr/local/go/bin/go")
                ));
                assert!(reason.contains("raw output adapter"));
            }
            _ => panic!("expected the WSL-only raw Go provider"),
        }
    }

    #[test]
    fn generic_windows_executable_overrides_an_unavailable_legacy_wsl_route() {
        let project_path = env::current_dir()
            .expect("test project directory exists")
            .to_string_lossy()
            .to_string();
        let resolution = resolve_tool_provider_from_discovery_with_user(
            "nvm",
            ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: project_path.clone(),
                distro: None,
                windows_path: None,
            },
            ProviderCacheEntry {
                tool: "nvm".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\\Users\\test\\AppData\\Local\\nvm\\nvm.exe".to_owned()),
                    native_rtk: None,
                    executable_version: Some("1.2.2".to_owned()),
                    executable_capabilities: vec!["version".to_owned()],
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: vec![WslToolProbe {
                    distro: "Ubuntu-RTK-WSL1".to_owned(),
                    wsl_version: Some(1),
                    executable: None,
                    rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    rtk_identity: None,
                }],
            },
            "miss",
            None,
        );

        assert_eq!(resolution.candidates.len(), 1);
        assert_eq!(resolution.availability.wsl[0].executable, None);
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("nvm"), OsString::from("ls")],
            &default_config(),
            Route::Wsl1,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { .. }
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(fallbacks.is_empty());
            }
            _ => {
                panic!("an unavailable WSL1 provider must not block generic Windows raw execution")
            }
        }
    }

    #[test]
    fn provider_planning_retains_the_next_eligible_route_for_pre_start_fallback() {
        let raw_config = Config::from_lookup(|name| match name {
            "RTK_WAD_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let candidate = |distro: &str, version, executable: &str| ProviderCandidate {
            kind: ProviderKind::WslRaw,
            distro: Some(distro.to_owned()),
            wsl_version: Some(version),
            executable: executable.to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![
                candidate("Ubuntu-RTK-WSL1", 1, "/opt/go-wsl1/bin/go"),
                candidate("Ubuntu", 2, "/opt/go-wsl2/bin/go"),
            ],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &raw_config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan {
                plan, fallbacks, ..
            } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl1);
                assert_eq!(fallbacks.len(), 1);
                assert_eq!(execution_route(&fallbacks[0].candidate), Route::Wsl2);
            }
            _ => panic!("the usable WSL2 candidate must remain available as fallback"),
        }
    }

    #[test]
    fn generic_dispatcher_routes_a_wsl_only_cargo_binary_without_rtk() {
        let config = Config::from_lookup(|name| match name {
            "RTK_WAD_OUTPUT_ADAPTER" => Some("raw".to_owned()),
            _ => None,
        })
        .expect("raw adapter configuration is valid");
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                kind: ProviderKind::WslRaw,
                distro: Some("Ubuntu".to_owned()),
                wsl_version: Some(2),
                executable: "/home/test/.cargo/bin/cargo".to_owned(),
                rtk: None,
                project_path: Some("/mnt/e/work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists only in WSL".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &config,
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert_eq!(execution_route(&plan.candidate), Route::Wsl2);
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Wsl2 { ref executable, .. }
                        if executable == &OsString::from("/home/test/.cargo/bin/cargo")
                ));
                assert!(reason.contains("cargo discovery"));
            }
            _ => panic!("expected the WSL-only raw Cargo provider"),
        }
    }

    #[test]
    fn generic_dispatcher_falls_back_to_verified_windows_raw_when_rtk_is_absent() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "cargo".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "cargo".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Users\test\.cargo\bin\cargo.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: vec![ProviderCandidate {
                kind: ProviderKind::WindowsRaw,
                distro: None,
                wsl_version: None,
                executable: r"C:\Users\test\.cargo\bin\cargo.exe".to_owned(),
                rtk: None,
                project_path: Some(r"E:\work".to_owned()),
                usable: true,
                reason: "fixture: Cargo exists on Windows without RTK".to_owned(),
            }],
            recommended: Some(0),
            diagnosis: "fixture".to_owned(),
            install: "disabled_in_p7",
        };

        match provider_dispatch_decision_from_resolution(
            &[OsString::from("cargo"), OsString::from("--version")],
            &default_config(),
            Route::NativeRtk,
            resolution,
        ) {
            ProviderDispatchDecision::UsePlan { plan, reason, .. } => {
                assert!(matches!(
                    plan.candidate,
                    dispatcher::RouteCandidate::Windows { ref executable, .. }
                        if executable == &OsString::from(r"C:\Users\test\.cargo\bin\cargo.exe")
                ));
                assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
                assert!(reason.contains("on Windows"));
            }
            _ => panic!("expected Windows raw fallback when native RTK is absent"),
        }
    }

    #[test]
    fn generic_dispatcher_discovers_every_safe_executable_name() {
        for tool in [
            "go",
            "cargo",
            "rustc",
            "node",
            "nvm",
            "npm",
            "pnpm",
            "python",
            "python3",
            "pytest",
            "java",
            "gradle",
            "mvn",
            "dotnet",
            "git",
            "tool.name",
            "cargo-next",
        ] {
            assert!(
                is_dispatchable_provider_tool(&[OsString::from(tool)]),
                "{tool}"
            );
        }
        assert!(!is_dispatchable_provider_tool(&[OsString::from("cmd /c")]));
        assert!(!is_dispatchable_provider_tool(&[OsString::from("go;exit")]));
    }

    #[test]
    fn execution_plan_uses_location_and_adapter_capability_not_legacy_provider_label() {
        let candidate = ProviderCandidate {
            // This intentionally contradictory legacy label proves that the
            // executor derives its route from provider location and its
            // adapter from the RTK capability, not from this display label.
            kind: ProviderKind::WindowsRtk,
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        let plan = execution_plan_for_provider_candidate(
            "go",
            &[OsString::from("version")],
            &default_config(),
            &candidate,
        )
        .expect("an auto plan can use raw output when RTK is absent");
        assert_eq!(plan.adapter, dispatcher::OutputAdapter::Raw);
        assert!(matches!(
            plan.candidate,
            dispatcher::RouteCandidate::Wsl2 { ref distro, ref executable, .. }
                if distro == "Ubuntu" && executable == &OsString::from("/usr/local/go/bin/go")
        ));
    }

    #[test]
    fn requested_rtk_adapter_does_not_silently_downgrade_to_raw() {
        let candidate = ProviderCandidate {
            kind: ProviderKind::WslRaw,
            distro: Some("Ubuntu".to_owned()),
            wsl_version: Some(2),
            executable: "/usr/local/go/bin/go".to_owned(),
            rtk: None,
            project_path: Some("/mnt/e/work".to_owned()),
            usable: true,
            reason: "fixture".to_owned(),
        };
        assert!(provider_adapter(&candidate, OutputAdapterPreference::Rtk).is_err());
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
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        match provider_dispatch_decision_from_resolution(
            &[OsString::from("go"), OsString::from("version")],
            &default_config(),
            Route::Raw,
            resolution,
        ) {
            ProviderDispatchDecision::Missing { reason } => {
                assert!(reason.contains("Installation is disabled in P7"));
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
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: Some(r"C:\tools\rtk.exe".to_owned()),
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
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
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Go\bin\go.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: Windows Go is available".to_owned(),
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
                    executable_version: None,
                    executable_capabilities: Vec::new(),
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
            executable_version: None,
            executable_capabilities: Vec::new(),
            executable_identity: None,
            native_rtk_identity: None,
        };
        let windows_project = ProjectLocation {
            kind: ProjectLocationKind::Windows,
            path: r"E:\work".to_owned(),
            distro: None,
            windows_path: None,
        };
        assert!(windows_tool_is_usable(
            &windows_project,
            Route::Raw,
            &windows
        ));
        assert!(!windows_tool_is_usable(
            &windows_project,
            Route::NativeRtk,
            &windows
        ));
        assert!(
            !windows_tool_is_usable(&windows_project, Route::Wsl1, &windows),
            "a conservative WSL fallback must not suppress Windows provider resolution"
        );
        let wsl_project = ProjectLocation {
            kind: ProjectLocationKind::Wsl,
            path: "/home/test/work".to_owned(),
            distro: Some("Ubuntu".to_owned()),
            windows_path: None,
        };
        assert!(!windows_tool_is_usable(&wsl_project, Route::Raw, &windows));
    }

    #[test]
    fn local_calibration_bootstraps_then_requires_validation_before_stable() {
        assert_eq!(calibration_route_for(None).0, Route::NativeRtk);

        let mut entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: "0123456789abcdef".to_owned(),
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
            manifest_version: command_manifest().upstream_rtk_version.clone(),
            context_signature: "0123456789abcdef".to_owned(),
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
