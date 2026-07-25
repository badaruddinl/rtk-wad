#![cfg(target_os = "windows")]

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CTRL_BREAK_EVENT: u32 = 1;
static WAD_LAUNCHER_NONCE: AtomicU64 = AtomicU64::new(0);
// Every contract test probes and controls the same WSL host. Serializing the
// external boundary keeps `cargo test` deterministic without changing the
// application's own process-concurrency behavior.
static PROCESS_CONTRACT_LOCK: Mutex<()> = Mutex::new(());

unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(ctrl_type: u32, process_group_id: u32) -> i32;
}

fn launcher() -> &'static str {
    env!("CARGO_BIN_EXE_rtk-wsl")
}

fn command(program: &str) -> Command {
    let mut command = Command::new(launcher());
    command.env("RTK_WSL_RTK_PATH", program);
    if let Ok(distro) = std::env::var("RTK_WSL1_TEST_DISTRO") {
        command
            .env("RTK_WSL_BACKEND", "wsl1")
            .env("RTK_WSL_DISTRO", distro);
    }
    command
}

fn wad_launcher() -> (PathBuf, PathBuf) {
    let nonce = WAD_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "rtk-wad-process-contract-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary WAD directory is created");
    let wad = directory.join("rtk-wad.exe");
    std::fs::copy(launcher(), &wad).expect("test launcher is copied under the WAD command name");
    (wad, directory)
}

fn process_contract_guard() -> MutexGuard<'static, ()> {
    PROCESS_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn preserves_stdout_stderr_exit_codes_and_literal_arguments() {
    let _guard = process_contract_guard();
    let literal = "space path/漢字;and&dollar$HOME\\tail";
    let output = command("/usr/bin/printf")
        .args(["%s", literal])
        .output()
        .expect("launcher starts");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), literal);

    let output = command("/bin/sh")
        .args(["-c", "printf diagnostic >&2; exit 42"])
        .output()
        .expect("launcher starts");
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "diagnostic");

    let output = command("/bin/sh")
        .args(["-c", "exit 127"])
        .output()
        .expect("launcher starts");
    assert_eq!(output.status.code(), Some(127));
}

#[test]
fn supports_stdin_for_a_simple_interactive_command() {
    let _guard = process_contract_guard();
    let mut child = command("/bin/sh")
        .args(["-c", "read line; printf 'received:%s' \"$line\""])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("launcher starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"hello\n")
        .expect("stdin writes");

    let output = child.wait_with_output().expect("launcher exits");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "received:hello");
}

#[test]
fn maps_a_temp_windows_worktree_to_the_wsl_current_directory() {
    let _guard = process_contract_guard();
    let directory = std::env::temp_dir().join(format!(
        "rtk-wsl-windows-cwd-contract-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary Windows worktree is created");
    let windows_path = directory.to_string_lossy().replace('\\', "/");
    let (drive, remainder) = windows_path
        .split_once(':')
        .expect("temporary worktree has a Windows drive prefix");
    let expected = format!("/mnt/{}{}", drive.to_lowercase(), remainder);
    let output = command("/bin/pwd")
        .current_dir(&directory)
        .output()
        .expect("launcher starts from the temporary Windows worktree");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => break,
            Err(error) if Instant::now() < cleanup_deadline => {
                thread::sleep(Duration::from_millis(100));
                let _ = error;
            }
            Err(error) => panic!("temporary Windows worktree remains locked: {error}"),
        }
    }
}

#[test]
fn routes_git_from_a_windows_worktree_to_native_git_with_structured_arguments() {
    let _guard = process_contract_guard();
    let output = Command::new(launcher())
        .env("RTK_WSL_DISTRO", "missing-test-distro")
        .args(["git", "--version"])
        .output()
        .expect("native Git route starts");

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version "));
}

#[test]
fn bridge_info_reports_the_selected_default_distribution() {
    let _guard = process_contract_guard();
    let output = Command::new(launcher())
        .arg("--bridge-info")
        .output()
        .expect("bridge diagnostics start");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("bridge=rtk-wsl"));
    assert!(stdout.contains("backend=auto"));
    assert!(stdout.contains("distro=Ubuntu"));
    assert!(stdout.contains("detected_wsl_version=2"));
}

#[test]
fn wad_profile_selects_one_route_and_uses_a_local_gain_ledger() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let local_app_data = directory.join("local-app-data");

    let info = Command::new(&launcher)
        .arg("--adapter-info")
        .output()
        .expect("WAD diagnostics start");
    assert!(info.status.success());
    assert!(String::from_utf8_lossy(&info.stdout).contains("adapter=rtk-wad"));

    let explained = Command::new(&launcher)
        .args(["--explain-route", "git", "commit", "-m", "contract"])
        .output()
        .expect("WAD route diagnostics start");
    assert!(explained.status.success());
    assert!(String::from_utf8_lossy(&explained.stdout).contains("route=raw"));

    let raw = Command::new(&launcher)
        .env("LOCALAPPDATA", &local_app_data)
        .args(["--route", "raw", "git", "--version"])
        .output()
        .expect("WAD raw route starts");
    assert!(raw.status.success());
    assert!(String::from_utf8_lossy(&raw.stdout).starts_with("git version "));
    let scratch = local_app_data.join("rtk-wad").join("scratch");
    let scratch_files = std::fs::read_dir(&scratch)
        .expect("WAD scratch directory exists")
        .count();
    assert_eq!(
        scratch_files, 0,
        "raw routes do not create RTK tracker scratch databases"
    );

    let gain = Command::new(&launcher)
        .env("LOCALAPPDATA", &local_app_data)
        .arg("gain")
        .output()
        .expect("WAD gain starts");
    assert!(gain.status.success());
    let gain_stdout = String::from_utf8_lossy(&gain.stdout);
    assert!(gain_stdout.contains("RTK-WAD Measured Token Accounting"));
    assert!(gain_stdout.contains("Invocations: 1"));

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn wad_calibrates_safe_commands_across_natural_invocations() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let state = directory.join("state");
    let fake_rtk = directory.join("fake-rtk.cmd");
    std::fs::write(
        &fake_rtk,
        "@echo off\r\necho fake-native-rtk %*\r\nexit /b 0\r\n",
    )
    .expect("fake native RTK is written");

    let run = || {
        Command::new(&launcher)
            .env("RTK_WAD_STATE_DIR", &state)
            .env("RTK_WAD_NATIVE_RTK_PATH", &fake_rtk)
            .args(["git", "status", "--short"])
            .output()
            .expect("calibration command starts")
    };

    let first = run();
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stdout).contains("fake-native-rtk"));
    let second = run();
    assert!(second.status.success());
    assert!(!String::from_utf8_lossy(&second.stdout).contains("fake-native-rtk"));
    let third = run();
    assert!(third.status.success());
    assert!(String::from_utf8_lossy(&third.stdout).contains("fake-native-rtk"));
    let fourth = run();
    assert!(fourth.status.success());
    let fifth = run();
    assert!(fifth.status.success());

    let state_path = state.join("calibration-v2.json");
    let recorded = std::fs::read_to_string(state_path).expect("calibration state is written");
    assert!(recorded.contains("\"raw_samples_ms\": ["));
    assert!(recorded.contains("\"native_samples_ms\": ["));
    assert!(!recorded.contains("git status"));

    let inspection = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .arg("calibration")
        .output()
        .expect("calibration inspection starts");
    assert!(inspection.status.success());
    assert!(String::from_utf8_lossy(&inspection.stdout).contains("phase=stable"));

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn wad_rejects_unsafe_generic_provider_names_before_discovery() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let output = Command::new(&launcher)
        .args(["resolve", "tool;not-run"])
        .output()
        .expect("provider validation starts");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tool names must contain only ASCII"));
    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

fn resolved_windows_candidate(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let resolution: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("provider resolution is JSON");
    resolution["candidates"]
        .as_array()
        .expect("provider resolution lists candidates")
        .iter()
        .find(|candidate| {
            candidate["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("windows_"))
        })
        .expect("Windows Git provider is discovered")
        .clone()
}

#[test]
fn wad_resolve_verifies_wsl_project_paths_for_windows_providers() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let state = directory.join("state");
    let project_directory = std::env::current_dir().expect("test project directory is available");
    let expected_project_path = project_directory.to_string_lossy().to_string();
    let project_path = expected_project_path.replace('\\', "/");
    let (drive, remainder) = project_path
        .split_once(':')
        .expect("test project directory has a Windows drive prefix");
    let mounted_project_path = format!("/mnt/{}{}", drive.to_lowercase(), remainder);
    let mut distros = vec!["Ubuntu".to_owned()];
    if let Ok(wsl1_distro) = std::env::var("RTK_WSL1_TEST_DISTRO")
        && !distros.contains(&wsl1_distro)
    {
        distros.push(wsl1_distro);
    }

    for distro in distros {
        let user = Command::new("wsl.exe")
            .args(["-d", &distro, "--exec", "id", "-un"])
            .output()
            .expect("selected WSL user is inspected");
        assert!(user.status.success());
        let user = String::from_utf8_lossy(&user.stdout).trim().to_owned();
        assert!(!user.is_empty());
        let mounted = Command::new(&launcher)
            .env("RTK_WAD_STATE_DIR", &state)
            .env("RTK_WSL_DISTRO", &distro)
            .env("RTK_WSL_USER", &user)
            .env("RTK_WSL_CWD", &mounted_project_path)
            .args(["resolve", "git", "--json", "--refresh"])
            .output()
            .expect("mounted WSL project resolution starts");
        let mounted_candidate = resolved_windows_candidate(&mounted);
        assert_eq!(mounted_candidate["usable"], true);
        assert!(
            mounted_candidate["project_path"]
                .as_str()
                .is_some_and(|path| path.eq_ignore_ascii_case(&expected_project_path)),
            "mounted {distro} project mapping: {mounted_candidate}"
        );

        let native_path = format!(
            "/tmp/rtk-wad-p13-native-{}-{}",
            std::process::id(),
            distro.replace(|character: char| !character.is_ascii_alphanumeric(), "-")
        );
        let created = Command::new("wsl.exe")
            .args(["-d", &distro, "--exec", "mkdir", "-p", &native_path])
            .status()
            .expect("temporary native WSL project is created");
        assert!(created.success());
        let native = Command::new(&launcher)
            .env("RTK_WAD_STATE_DIR", &state)
            .env("RTK_WSL_DISTRO", &distro)
            .env("RTK_WSL_USER", &user)
            .env("RTK_WSL_CWD", &native_path)
            .args(["resolve", "git", "--json", "--refresh"])
            .output()
            .expect("native WSL project resolution starts");
        let native_candidate = resolved_windows_candidate(&native);
        assert_eq!(native_candidate["usable"], true);
        let native_windows_path = native_candidate["project_path"]
            .as_str()
            .expect("native WSL project has a Windows path");
        let expected_prefix = format!(
            r"\\wsl.localhost\{}\tmp\rtk-wad-p13-native-{}-",
            distro.to_ascii_lowercase(),
            std::process::id()
        );
        assert!(
            native_windows_path
                .to_ascii_lowercase()
                .starts_with(&expected_prefix),
            "native {distro} project mapping: {native_windows_path}"
        );

        let removed = Command::new("wsl.exe")
            .args(["-d", &distro, "--exec", "rmdir", &native_path])
            .status()
            .expect("temporary native WSL project cleanup starts");
        assert!(removed.success());
    }
    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

fn provider_candidate_index(
    resolution: &serde_json::Value,
    kind: &str,
    distro: Option<&str>,
) -> usize {
    resolution["candidates"]
        .as_array()
        .expect("provider resolution lists candidates")
        .iter()
        .position(|candidate| {
            candidate["kind"] == kind
                && distro.is_none_or(|distro| candidate["distro"] == distro)
                && candidate["usable"] == true
        })
        .expect("requested verified provider candidate is present")
}

#[test]
fn wad_provider_exec_runs_each_verified_provider_without_replay() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let state = directory.join("state");
    let native_state = directory.join("native-state");
    let fake_tool = directory.join("p14-tool.cmd");
    let fake_rtk = directory.join("p14-rtk.cmd");
    std::fs::write(
        &fake_tool,
        "@echo off\r\necho tool-cwd:%CD%\r\necho tool-args:%*\r\nexit /b 42\r\n",
    )
    .expect("fake raw provider is written");
    std::fs::write(
        &fake_rtk,
        "@echo off\r\necho rtk-cwd:%CD%\r\necho rtk-args:%*\r\nexit /b 43\r\n",
    )
    .expect("fake native RTK provider is written");
    let inherited_path = std::env::var_os("PATH").expect("PATH is available");
    let path = std::env::join_paths([directory.as_os_str(), inherited_path.as_ref()])
        .expect("test PATH is valid");
    let expected_cwd = std::env::current_dir()
        .expect("test project directory is available")
        .to_string_lossy()
        .to_string();
    let literal = "space;and&dollar$HOME\\漢字";

    let raw = Command::new(&launcher)
        .env("PATH", &path)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args(["provider", "exec", "p14-tool", "--", literal])
        .output()
        .expect("Windows raw provider starts");
    assert_eq!(raw.status.code(), Some(42));
    let raw_stdout = String::from_utf8_lossy(&raw.stdout);
    assert!(raw_stdout.contains(&format!("tool-cwd:{expected_cwd}")));
    assert!(raw_stdout.contains(literal));
    assert_eq!(raw_stdout.matches("tool-args:").count(), 1);

    let native = Command::new(&launcher)
        .env("PATH", &path)
        .env("RTK_WAD_STATE_DIR", &native_state)
        .env("RTK_WAD_NATIVE_RTK_PATH", &fake_rtk)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args(["provider", "exec", "p14-tool", "--", literal])
        .output()
        .expect("Windows RTK provider starts");
    assert_eq!(native.status.code(), Some(43));
    let native_stdout = String::from_utf8_lossy(&native.stdout);
    assert!(native_stdout.contains(&format!("rtk-cwd:{expected_cwd}")));
    assert!(native_stdout.contains("p14-tool"));
    assert!(native_stdout.contains(literal));
    assert_eq!(native_stdout.matches("rtk-args:").count(), 1);

    let resolution = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args(["resolve", "git", "--json", "--refresh"])
        .output()
        .expect("Git provider resolution starts");
    assert!(resolution.status.success());
    let resolution: serde_json::Value =
        serde_json::from_slice(&resolution.stdout).expect("Git provider resolution is JSON");
    let wsl_rtk_index = provider_candidate_index(&resolution, "wsl_rtk", Some("Ubuntu"));
    let wsl_raw_index = provider_candidate_index(&resolution, "wsl_raw", Some("Ubuntu-RTK-WSL1"));

    let wsl_rtk = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args([
            "provider",
            "exec",
            "git",
            "--candidate",
            &wsl_rtk_index.to_string(),
            "--",
            "--version",
        ])
        .output()
        .expect("WSL RTK provider starts");
    assert!(wsl_rtk.status.success());
    assert!(String::from_utf8_lossy(&wsl_rtk.stdout).starts_with("git version "));

    let wsl_raw = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args([
            "provider",
            "exec",
            "git",
            "--candidate",
            &wsl_raw_index.to_string(),
            "--",
            "--version",
        ])
        .output()
        .expect("WSL raw provider starts");
    assert!(wsl_raw.status.success());
    assert!(String::from_utf8_lossy(&wsl_raw.stdout).starts_with("git version "));

    let rejected = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WSL_DISTRO", "Ubuntu")
        .args([
            "provider",
            "exec",
            "git",
            "--candidate",
            &wsl_raw_index.to_string(),
            "--",
            r"E:\foreign\path",
        ])
        .output()
        .expect("foreign-path rejection starts");
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("does not translate foreign absolute arguments")
    );

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn wad_surface_matches_the_live_wsl_rtk_command_inventory() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let surface = Command::new(&launcher)
        .args(["surface", "--json"])
        .output()
        .expect("surface report starts");
    assert!(surface.status.success());
    let surface: serde_json::Value =
        serde_json::from_slice(&surface.stdout).expect("surface report is JSON");
    assert_eq!(surface["upstream_rtk_version"], "0.43.0");
    assert_eq!(surface["upstream_command_count"], 69);
    let wad_commands = surface["commands"]
        .as_array()
        .expect("surface report contains commands")
        .iter()
        .map(|row| {
            assert_ne!(row["classification"], "unknown");
            row["command"]
                .as_str()
                .expect("surface command name")
                .to_owned()
        })
        .collect::<std::collections::BTreeSet<_>>();

    let help = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "/usr/local/bin/rtk", "--help"])
        .output()
        .expect("live WSL RTK help starts");
    assert!(help.status.success());
    let live_commands = String::from_utf8_lossy(&help.stdout)
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let command = trimmed.split_whitespace().next()?;
            (line.starts_with("  ")
                && command != "help"
                && command
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_lowercase())
                && command
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
            .then(|| command.to_owned())
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(live_commands.len(), 69);
    assert_eq!(wad_commands, live_commands);
    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn wad_policy_requires_a_matching_local_adapter_context() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let state = directory.join("state");
    let context = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["policy", "context"])
        .output()
        .expect("policy context starts");
    assert!(context.status.success());
    let context: serde_json::Value =
        serde_json::from_slice(&context.stdout).expect("policy context is JSON");
    let signature = context["context_signature"]
        .as_str()
        .expect("opaque context signature")
        .to_owned();
    assert_eq!(signature.len(), 16);
    assert_eq!(context["manifest_version"], "0.43.0");

    let policy = serde_json::json!({
        "schema_version": 2,
        "manifest_version": "0.43.0",
        "context_signature": signature,
        "evidence": [{
            "key": "rg",
            "raw_median_ms": 1.0,
            "candidate_median_ms": 100.0,
            "token_savings_percent": 0.0,
            "sample_count": 5
        }]
    });
    let source = directory.join("policy.json");
    std::fs::write(
        &source,
        serde_json::to_vec_pretty(&policy).expect("policy JSON is encoded"),
    )
    .expect("policy fixture is written");
    let imported = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["policy", "import", source.to_str().expect("policy path")])
        .output()
        .expect("policy import starts");
    assert!(imported.status.success());

    let selected = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["--explain-route", "rg", "needle"])
        .output()
        .expect("matching policy explanation starts");
    assert!(selected.status.success());
    assert!(String::from_utf8_lossy(&selected.stdout).contains("route=raw"));

    let alternate_rtk = directory.join("other-rtk.cmd");
    std::fs::write(&alternate_rtk, "@echo off\r\nexit /b 0\r\n")
        .expect("alternate RTK fixture is written");
    let invalidated = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .env("RTK_WAD_NATIVE_RTK_PATH", &alternate_rtk)
        .args(["--explain-route", "rg", "needle"])
        .output()
        .expect("changed-context explanation starts");
    assert!(invalidated.status.success());
    assert!(String::from_utf8_lossy(&invalidated.stdout).contains("route=native-rtk"));

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn wad_generic_setup_is_diagnostic_only_and_never_creates_an_install_transaction() {
    let _guard = process_contract_guard();
    let (launcher, directory) = wad_launcher();
    let state = directory.join("state");

    let ready = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["setup", "git", "--json", "--refresh"])
        .output()
        .expect("generic ready setup diagnosis starts");
    assert!(ready.status.success());
    let ready: serde_json::Value =
        serde_json::from_slice(&ready.stdout).expect("generic ready setup is JSON");
    assert_eq!(ready["tool"], "git");
    assert_eq!(ready["mode"], "diagnostic-only");
    assert_eq!(ready["status"], "ready");
    assert!(ready["proposed_command"].is_null());
    assert_eq!(ready["apply"], "not_needed");

    let missing_tool = "p17-tool-that-is-not-installed";
    let blocked = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["setup", missing_tool, "--json", "--refresh"])
        .output()
        .expect("generic blocked setup diagnosis starts");
    assert!(blocked.status.success());
    let blocked: serde_json::Value =
        serde_json::from_slice(&blocked.stdout).expect("generic blocked setup is JSON");
    assert_eq!(blocked["mode"], "diagnostic-only");
    assert_eq!(blocked["status"], "blocked");
    assert!(blocked["proposed_command"].is_null());
    assert_eq!(blocked["apply"], "unavailable_for_generic_tool");
    assert!(
        blocked["reason"]
            .as_str()
            .expect("blocked reason is text")
            .contains("will not guess an installer")
    );

    let doctor = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["doctor", missing_tool, "--refresh"])
        .output()
        .expect("generic missing-provider doctor starts");
    assert!(!doctor.status.success());
    let doctor_stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(doctor_stdout.contains("recommended=none"));
    assert!(doctor_stdout.contains("diagnosis=no verified provider is available"));
    assert!(doctor_stdout.contains("setup p17-tool-that-is-not-installed"));

    let forced = Command::new(&launcher)
        .env("RTK_WAD_STATE_DIR", &state)
        .args(["setup", missing_tool, "--apply", "--confirm"])
        .output()
        .expect("generic forced setup starts");
    assert!(!forced.status.success());
    assert!(String::from_utf8_lossy(&forced.stderr).contains("diagnostic-only"));
    assert!(!state.join("setup-transaction-v1.json").exists());

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn provisioned_wsl1_bridge_preserves_the_process_contract_when_requested() {
    let _guard = process_contract_guard();
    let Ok(distro) = std::env::var("RTK_WSL1_TEST_DISTRO") else {
        return;
    };
    let literal = "wsl1 space/漢字;and&dollar$HOME\\tail";
    let output = Command::new(launcher())
        .env("RTK_WSL_BACKEND", "wsl1")
        .env("RTK_WSL_DISTRO", distro)
        .env("RTK_WSL_RTK_PATH", "/usr/bin/printf")
        .args(["%s", literal])
        .output()
        .expect("WSL1 bridge starts");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), literal);
}

#[test]
fn ctrl_break_releases_the_global_lock_for_waiting_children() {
    let _guard = process_contract_guard();
    let ready_file = std::env::temp_dir().join(format!("rtk-wsl-ready-{}", std::process::id()));
    let _ = std::fs::remove_file(&ready_file);
    let mut first = command("/bin/sh")
        .args(["-c", "sleep 30"])
        .env("RTK_WSL_TEST_READY_FILE", &ready_file)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stderr(Stdio::piped())
        .spawn()
        .expect("first launcher starts");
    let ready_deadline = Instant::now() + Duration::from_secs(45);
    while !ready_file.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "launcher did not register its Ctrl+Break handler"
        );
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_secs(3));
    if let Some(status) = first.try_wait().expect("first status is available") {
        let mut stderr = String::new();
        first
            .stderr
            .take()
            .expect("first stderr is piped")
            .read_to_string(&mut stderr)
            .expect("first stderr reads");
        panic!("first launcher exited before cancellation: status={status}; stderr={stderr}");
    }

    let mut second = command("/usr/bin/printf")
        .args(["released"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("second launcher starts");
    thread::sleep(Duration::from_secs(1));
    if let Some(status) = second.try_wait().expect("second status is available") {
        let mut stderr = String::new();
        second
            .stderr
            .take()
            .expect("stderr is piped")
            .read_to_string(&mut stderr)
            .expect("stderr reads");
        panic!("second launcher did not wait for the lock: status={status}; stderr={stderr}");
    }

    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, first.id()) };
    assert_ne!(
        sent, 0,
        "failed to send CTRL_BREAK_EVENT to launcher process group"
    );
    let cancellation_started = Instant::now();
    let first_status = first.wait().expect("interrupted launcher exits");
    let mut first_stderr = String::new();
    first
        .stderr
        .take()
        .expect("first stderr is piped")
        .read_to_string(&mut first_stderr)
        .expect("first stderr reads");
    assert!(
        cancellation_started.elapsed() < Duration::from_secs(5),
        "interrupted launcher exceeded the cancellation deadline: stderr={first_stderr}"
    );
    assert!(
        !first_status.success(),
        "interrupted launcher unexpectedly succeeded: stderr={first_stderr}"
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = second.try_wait().expect("second status is available") {
            assert!(
                status.success(),
                "waiting launcher failed after lock release"
            );
            let mut output = String::new();
            second
                .stdout
                .take()
                .expect("stdout is piped")
                .read_to_string(&mut output)
                .expect("stdout reads");
            assert_eq!(output, "released");
            break;
        }
        assert!(Instant::now() < deadline, "{}", {
            let _ = second.kill();
            let _ = second.wait();
            let mut stderr = String::new();
            second
                .stderr
                .take()
                .expect("stderr is piped")
                .read_to_string(&mut stderr)
                .expect("stderr reads");
            format!(
                "waiting launcher did not continue after cancellation: first_stderr={first_stderr}; second_stderr={stderr}"
            )
        });
        thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(ready_file);
}

#[test]
fn ctrl_break_cancels_from_a_temp_windows_worktree() {
    let _guard = process_contract_guard();
    let directory = std::env::temp_dir().join(format!(
        "rtk-wsl-windows-cancel-contract-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary Windows worktree is created");
    let ready_file = directory.join("ready");
    let mut child = command("/bin/sh")
        .current_dir(&directory)
        .args(["-c", "sleep 30"])
        .env("RTK_WSL_TEST_READY_FILE", &ready_file)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stderr(Stdio::piped())
        .spawn()
        .expect("launcher starts from the temporary Windows worktree");
    let ready_deadline = Instant::now() + Duration::from_secs(45);
    while !ready_file.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "launcher did not register its Ctrl+Break handler"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    assert_ne!(sent, 0, "failed to send CTRL_BREAK_EVENT to launcher");
    let cancelled = Instant::now();
    let status = child.wait().expect("interrupted launcher exits");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr is piped")
        .read_to_string(&mut stderr)
        .expect("stderr reads");
    assert!(
        cancelled.elapsed() < Duration::from_secs(5),
        "interrupted launcher exceeded the cancellation deadline: stderr={stderr}"
    );
    assert!(
        !status.success(),
        "interrupted launcher unexpectedly succeeded"
    );
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => break,
            Err(error) if Instant::now() < cleanup_deadline => {
                thread::sleep(Duration::from_millis(100));
                let _ = error;
            }
            Err(error) => panic!("temporary Windows worktree remains locked: {error}"),
        }
    }
}
