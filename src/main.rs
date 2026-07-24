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

fn wad_data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("rtk-wad")
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
        let root = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("rtk-wad");
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
    ) -> Result<(), String> {
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
        Ok(())
    }

    fn print_gain() -> Result<(), String> {
        let root = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("rtk-wad");
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
    let policy_key = match wad_command_family(arguments) {
        "git" => git_subcommand(arguments).map(|subcommand| format!("git:{subcommand}")),
        "rg" => Some("rg".to_owned()),
        "cargo" => arguments
            .get(1)
            .and_then(|subcommand| subcommand.to_str())
            .map(|subcommand| format!("cargo:{subcommand}")),
        "npm" if is_verified_npm_run_list_operation(arguments) => Some("npm:run-list".to_owned()),
        _ => None,
    };
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
            }
            Route::NativeRtk => {
                wad_command_family(arguments) == "rg"
                    || is_verified_read_only_git(arguments)
                    || is_verified_cargo_operation(arguments)
                    || is_verified_npm_run_list_operation(arguments)
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
        "npm" | "npx" | "pnpm" | "go" | "dotnet" => (
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
    let mut command = Command::new(&config.native_rtk_path);
    command.args(arguments);
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
        _ => program.clone(),
    };
    Command::new(executable)
        .args(arguments.iter().skip(1))
        .spawn()
        .and_then(|mut child| child.wait())
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
    let (route, reason) = if requested_route == Route::Auto {
        auto_wad_route(
            &arguments,
            current_directory.as_deref().and_then(|path| path.to_str()),
            load_route_policy().as_ref(),
        )
    } else {
        (requested_route, "explicit route preference")
    };
    if explain {
        println!("route={}", route.as_str());
        println!("reason={reason}");
        println!("command_family={}", wad_command_family(&arguments));
        return ExitCode::SUCCESS;
    }
    if arguments.is_empty() {
        eprintln!("rtk-wad: no command supplied; use rtk-wad --adapter-info for configuration");
        return ExitCode::FAILURE;
    }
    let selected_config = configured_wsl_backend(config, route);
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
    let started = Instant::now();
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
    if let Some(metrics) = metrics
        && let Err(error) = metrics.finish(
            executed_route,
            wad_command_family(&arguments),
            started.elapsed(),
            exit_code,
        )
    {
        eprintln!("rtk-wad: metrics were not recorded: {error}");
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
}
