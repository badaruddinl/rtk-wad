#![cfg(target_os = "windows")]

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    Mutex, MutexGuard, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CTRL_BREAK_EVENT: u32 = 1;
static XUVA_LAUNCHER_NONCE: AtomicU64 = AtomicU64::new(0);
// Every contract test probes and controls the same WSL host. Serializing the
// external boundary keeps `cargo test` deterministic without changing the
// application's own process-concurrency behavior.
static PROCESS_CONTRACT_LOCK: Mutex<()> = Mutex::new(());
static WSL_ROUTE_PROOF: OnceLock<()> = OnceLock::new();

unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(ctrl_type: u32, process_group_id: u32) -> i32;
}

fn launcher() -> &'static str {
    static LAUNCHER: OnceLock<String> = OnceLock::new();
    LAUNCHER
        .get_or_init(|| {
            std::env::var("XUVA_TEST_BINARY")
                .unwrap_or_else(|_| env!("CARGO_BIN_EXE_xuva").to_owned())
        })
        .as_str()
}

fn command(program: &str) -> Command {
    let mut command = Command::new(launcher());
    command.env("XUVA_WSL_RTK_PATH", program);
    if let Ok(distro) = std::env::var("XUVA_WSL1_TEST_DISTRO") {
        assert_wsl_version(&distro, 1);
        command
            .env("XUVA_ROUTE", "wsl1")
            .env("XUVA_WSL_BACKEND", "wsl1")
            .env("XUVA_WSL_DISTRO", distro);
    } else {
        let distro = std::env::var("XUVA_WSL2_TEST_DISTRO").unwrap_or_else(|_| "Ubuntu".to_owned());
        assert_wsl_version(&distro, 2);
        command
            .env("XUVA_ROUTE", "wsl2")
            .env("XUVA_WSL_BACKEND", "wsl2")
            .env("XUVA_WSL_DISTRO", distro);
    }
    command
}

fn decode_wsl_list(bytes: &[u8]) -> String {
    if bytes.chunks_exact(2).any(|pair| pair[1] == 0) {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn assert_wsl_version(distro: &str, expected: u8) {
    WSL_ROUTE_PROOF.get_or_init(|| {
        let output = Command::new("wsl.exe")
            .args(["--list", "--verbose"])
            .output()
            .expect("wsl.exe --list --verbose starts");
        assert!(
            output.status.success(),
            "unable to prove the WSL test distro version"
        );
        let rendered = decode_wsl_list(&output.stdout).replace('\0', "");
        let actual = rendered.lines().find_map(|line| {
            let fields = line
                .trim()
                .trim_start_matches('*')
                .split_whitespace()
                .collect::<Vec<_>>();
            (fields.len() >= 3 && fields[0] == distro)
                .then(|| fields.last().and_then(|value| value.parse::<u8>().ok()))
                .flatten()
        });
        assert_eq!(
            actual,
            Some(expected),
            "configured test distro `{distro}` is not WSL {expected}; inventory:\n{rendered}"
        );
    });
}

fn xuva_launcher() -> (PathBuf, PathBuf) {
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "xuva-process-contract-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary XUVA directory is created");
    let xuva = directory.join("xuva.exe");
    std::fs::copy(launcher(), &xuva).expect("test launcher is copied under the XUVA command name");
    (xuva, directory)
}

fn process_contract_guard() -> MutexGuard<'static, ()> {
    PROCESS_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unique_temp_directory(label: &str) -> PathBuf {
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("xuva-{label}-{}-{nonce}", std::process::id()))
}

#[test]
fn dispatcher_owned_version_never_enters_environment_resolution() {
    let _guard = process_contract_guard();
    let output = Command::new(launcher())
        .env("XUVA_WSL_DISTRO", "missing-version-test-distro")
        .env("XUVA_ROUTE", "not-a-route")
        .args(["--version"])
        .output()
        .expect("version command starts");

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("xuva {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn lifecycle_status_remains_available_with_invalid_routing_configuration() {
    let _guard = process_contract_guard();
    let output = Command::new(launcher())
        .env("XUVA_ROUTE", "not-a-route")
        .args(["install", "--status"])
        .output()
        .expect("lifecycle status starts");

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status output is valid JSON");
    let executable = status["executable"]
        .as_str()
        .expect("status has an executable path");
    assert_eq!(
        PathBuf::from(executable)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("xuva.exe")
    );
    assert!(PathBuf::from(executable).is_file());
}

#[test]
fn metrics_opt_out_keeps_explicit_raw_execution_ledger_free() {
    let _guard = process_contract_guard();
    let state = unique_temp_directory("metrics-off-state");
    let system_root = std::env::var_os("SYSTEMROOT").expect("Windows has SYSTEMROOT");
    let cmd = PathBuf::from(system_root).join("System32").join("cmd.exe");
    let output = Command::new(launcher())
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_METRICS", "off")
        .arg(&cmd)
        .args(["/d", "/c", "exit", "0"])
        .output()
        .expect("explicit raw command starts");

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!state.join("metrics-v1.sqlite").exists());
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn local_front_door_commands_have_a_bounded_latency_budget() {
    let _guard = process_contract_guard();
    let state = unique_temp_directory("latency-state");
    let started = Instant::now();
    let output = Command::new(launcher())
        .env("XUVA_STATE_DIR", &state)
        .args(["--explain-route", "git", "status", "--short"])
        .output()
        .expect("cold native Git route explanation starts");
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "cold native route explanation exceeded the 5 second budget: {elapsed:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("route=raw"), "{stdout}");
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn help_update_and_shell_syntax_have_local_actionable_ux() {
    let _guard = process_contract_guard();
    let help = Command::new(launcher())
        .arg("--help")
        .output()
        .expect("help starts");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("never rebuilds a pipeline"));

    let update = Command::new(launcher())
        .arg("self-update")
        .output()
        .expect("self-update diagnosis starts");
    assert!(update.status.success());
    assert!(String::from_utf8_lossy(&update.stdout).contains("self-update --check"));

    let operator = Command::new(launcher())
        .arg("&&")
        .output()
        .expect("shell syntax diagnosis starts");
    assert_eq!(operator.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&operator.stderr).contains("shell syntax"));
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
    let directory =
        std::env::temp_dir().join(format!("xuva-windows-cwd-contract-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary Windows worktree is created");
    let windows_path = directory.to_string_lossy().replace('\\', "/");
    let (drive, remainder) = windows_path
        .split_once(':')
        .expect("temporary worktree has a Windows drive prefix");
    let expected = format!("/mnt/{}{}", drive.to_lowercase(), remainder);
    let output = command("/bin/sh")
        .args(["-c", "pwd"])
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
        .env("XUVA_WSL_DISTRO", "missing-test-distro")
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
fn dispatches_wsl_only_go_raw_from_a_windows_shell() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!("/tmp/xuva-p7-go-{}-{nonce}", std::process::id());
    assert!(
        fixture.starts_with("/tmp/xuva-p7-go-"),
        "fixture cleanup target is constrained to the test namespace"
    );
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '#!/bin/sh\nprintf "go version go-fixture linux/amd64\n"\n' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-go-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary WSL Go fixture setup starts");
    assert!(
        setup.status.success(),
        "fixture setup stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-go-state-{nonce}"));
    let output = Command::new(launcher())
        .env("PATH", system32)
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_WSL_DISTRO", "Ubuntu")
        .env("XUVA_WSL_EXTRA_PATH", &fixture)
        .args(["go", "version"])
        .output()
        .expect("Go dispatcher starts");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL Go fixture cleanup starts");
    assert!(cleanup.success(), "temporary WSL Go fixture is removed");
    let _ = std::fs::remove_dir_all(&state);

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "go version go-fixture linux/amd64\n"
    );
}

#[test]
fn dispatches_wsl_only_go_from_powershell_cmd_and_git_bash() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!(
        "/tmp/xuva-p7-go-shell-matrix-{}-{nonce}",
        std::process::id()
    );
    assert!(fixture.starts_with("/tmp/xuva-p7-go-shell-matrix-"));
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'if [ "$1" = "run" ]; then printf "arg:%s\n" "$2"; else printf "go version shell-matrix-fixture linux/amd64\n"; fi' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-go-shell-matrix-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary WSL shell-matrix Go fixture setup starts");
    assert!(setup.status.success());

    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-go-shell-matrix-state-{nonce}"));
    let configure = |command: &mut Command| {
        command
            .env("PATH", &system32)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .env("XUVA_WSL_EXTRA_PATH", &fixture)
            .env("XUVA_OUTPUT_ADAPTER", "raw")
            .env("XUVA_TEST_LAUNCHER", launcher());
    };
    let literal = "space & $dollar\\漢字";

    let mut powershell = Command::new("powershell.exe");
    configure(&mut powershell);
    let powershell = powershell
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "& $env:XUVA_TEST_LAUNCHER go version",
        ])
        .output()
        .expect("PowerShell Go dispatch starts");
    let mut powershell_literal = Command::new("powershell.exe");
    configure(&mut powershell_literal);
    let powershell_literal = powershell_literal
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "& $env:XUVA_TEST_LAUNCHER go run 'space & $dollar\\漢字'",
        ])
        .output()
        .expect("PowerShell literal Go dispatch starts");

    let mut cmd = Command::new("cmd.exe");
    configure(&mut cmd);
    let cmd = cmd
        .args(["/d", "/s", "/c", &format!("{} go version", launcher())])
        .output()
        .expect("CMD Go dispatch starts");
    let cmd_wrapper = state.join("invoke-go-literal.cmd");
    std::fs::write(
        &cmd_wrapper,
        "@echo off\r\n\"%XUVA_TEST_LAUNCHER%\" go run \"%XUVA_TEST_LITERAL%\"\r\n",
    )
    .expect("CMD literal wrapper is written");
    let mut cmd_literal = Command::new("cmd.exe");
    configure(&mut cmd_literal);
    cmd_literal.env("XUVA_TEST_LITERAL", literal);
    let cmd_literal = cmd_literal
        .args([
            "/d",
            "/s",
            "/c",
            cmd_wrapper.to_str().expect("CMD wrapper path"),
        ])
        .output()
        .expect("CMD literal Go dispatch starts");

    let git_bash = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
    ]
    .into_iter()
    .find(|path| PathBuf::from(path).is_file())
    .expect("Git Bash is installed for the Windows shell contract");
    let mut bash = Command::new(git_bash);
    configure(&mut bash);
    bash.env("MSYS_NO_PATHCONV", "1");
    let bash = bash
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "exec \"$XUVA_TEST_LAUNCHER\" go version",
        ])
        .output()
        .expect("Git Bash Go dispatch starts");
    let mut bash_literal = Command::new(git_bash);
    configure(&mut bash_literal);
    bash_literal.env("MSYS_NO_PATHCONV", "1");
    let bash_literal = bash_literal
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "exec \"$XUVA_TEST_LAUNCHER\" go run 'space & $dollar\\漢字'",
        ])
        .output()
        .expect("Git Bash literal Go dispatch starts");

    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL shell-matrix Go fixture cleanup starts");
    assert!(cleanup.success());
    let _ = std::fs::remove_dir_all(&state);

    for (shell, output) in [("PowerShell", powershell), ("CMD", cmd), ("Git Bash", bash)] {
        assert!(
            output.status.success(),
            "{shell} stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "go version shell-matrix-fixture linux/amd64\n",
            "{shell} must reach the WSL-only Go binary"
        );
    }
    for (shell, output) in [
        ("PowerShell", powershell_literal),
        ("CMD", cmd_literal),
        ("Git Bash", bash_literal),
    ] {
        assert!(
            output.status.success(),
            "{shell} literal stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("arg:{literal}\n"),
            "{shell} must preserve a literal shell argument"
        );
    }
}

#[test]
fn wsl_shim_preserves_same_distro_context_and_literal_argv() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!("/tmp/xuva-p7-wsl-shim-{}-{nonce}", std::process::id());
    assert!(fixture.starts_with("/tmp/xuva-p7-wsl-shim-"));
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'printf "cwd:%s\n" "$PWD"' 'printf "args:%s|%s\n" "$1" "$2"' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-wsl-shim-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary WSL shim Go fixture setup starts");
    assert!(setup.status.success());

    let map_path = |path: &str| {
        let output = Command::new("wsl.exe")
            .args(["-d", "Ubuntu", "--exec", "wslpath", "-a", path])
            .output()
            .expect("Windows path maps into WSL");
        assert!(
            output.status.success(),
            "path mapping stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let launcher_path = map_path(launcher());
    let shim_windows_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("xuva-wsl.sh");
    let shim_path = map_path(&shim_windows_path.to_string_lossy());
    let literal = "space & $dollar\\\u{6f22}\u{5b57}";
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"cd /tmp; export XUVA_WINDOWS_EXE="$1" XUVA_WSL_EXTRA_PATH="$2" XUVA_OUTPUT_ADAPTER=raw; exec sh "$3" go run "$4""###,
            "xuva-p7-wsl-shim-run",
            &launcher_path,
            &fixture,
            &shim_path,
            literal,
        ])
        .output()
        .expect("WSL shim starts the dispatcher");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL shim Go fixture cleanup starts");
    assert!(cleanup.success());

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cwd:/tmp"));
    assert!(stdout.contains(&format!("args:run|{literal}")));
}

#[test]
fn wsl_shim_maps_a_windows_backed_project_to_another_distro() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!(
        "/tmp/xuva-p7-cross-distro-shim-{}-{nonce}",
        std::process::id()
    );
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'printf "cwd:%s\n" "$PWD"' 'printf "args:%s|%s\n" "$1" "$2"' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-cross-distro-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary cross-distro Go fixture setup starts");
    assert!(setup.status.success());

    let map_path = |path: &str| {
        let output = Command::new("wsl.exe")
            .args(["-d", "docker-desktop", "--exec", "wslpath", "-a", path])
            .output()
            .expect("Windows path maps into the source distro");
        assert!(
            output.status.success(),
            "source mapping stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let launcher_path = map_path(launcher());
    let shim_windows_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("xuva-wsl.sh");
    let shim_path = map_path(&shim_windows_path.to_string_lossy());
    let project_path = map_path(env!("CARGO_MANIFEST_DIR"));
    let literal = "cross distro & $dollar\\\u{6f22}\u{5b57}";
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            "docker-desktop",
            "--exec",
            "sh",
            "-c",
            r###"cd "$4"; export XUVA_WINDOWS_EXE="$1" XUVA_WSL_EXTRA_PATH="$2" XUVA_OUTPUT_ADAPTER=raw; exec sh "$3" go run "$5""###,
            "xuva-p7-cross-distro-run",
            &launcher_path,
            &fixture,
            &shim_path,
            &project_path,
            literal,
        ])
        .output()
        .expect("cross-distro WSL shim starts the dispatcher");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary cross-distro Go fixture cleanup starts");
    assert!(cleanup.success());

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected_cwd = env!("CARGO_MANIFEST_DIR").replace('\\', "/");
    let (drive, remainder) = expected_cwd
        .split_once(':')
        .expect("manifest directory has a Windows drive prefix");
    assert!(stdout.contains(&format!(
        "cwd:/mnt/{}{}",
        drive.to_ascii_lowercase(),
        remainder
    )));
    assert!(stdout.contains(&format!("args:run|{literal}")));
}

#[test]
fn wsl_shim_uses_a_compatible_windows_go_from_a_windows_backed_project() {
    let _guard = process_contract_guard();
    let map_path = |path: &str| {
        let output = Command::new("wsl.exe")
            .args(["-d", "Ubuntu", "--exec", "wslpath", "-a", path])
            .output()
            .expect("Windows path maps into Ubuntu");
        assert!(
            output.status.success(),
            "path mapping stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let launcher_path = map_path(launcher());
    let shim_windows_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("xuva-wsl.sh");
    let shim_path = map_path(&shim_windows_path.to_string_lossy());
    let project_path = map_path(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"cd "$3"; export XUVA_WINDOWS_EXE="$1" XUVA_OUTPUT_ADAPTER=raw; exec sh "$2" go version"###,
            "xuva-p7-windows-go-run",
            &launcher_path,
            &shim_path,
            &project_path,
        ])
        .output()
        .expect("WSL shim starts compatible Windows Go");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(" windows/amd64"),
        "the selected compatible provider must be the Windows Go binary"
    );

    let explained = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"cd "$3"; export XUVA_WINDOWS_EXE="$1" XUVA_OUTPUT_ADAPTER=raw; exec sh "$2" --explain-route go version"###,
            "xuva-p7-windows-go-explain",
            &launcher_path,
            &shim_path,
            &project_path,
        ])
        .output()
        .expect("WSL shim explains compatible Windows Go");
    assert!(explained.status.success());
    assert!(
        String::from_utf8_lossy(&explained.stdout).contains("selected windows"),
        "explanation: {}",
        String::from_utf8_lossy(&explained.stdout)
    );
}

#[test]
fn native_wsl_origin_runs_explicit_windows_provider_once_with_isolated_environment() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let project = format!("/tmp/xuva-native-wsl-origin-{}-{nonce}", std::process::id());
    let setup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "mkdir", "-p", &project])
        .status()
        .expect("native WSL project setup starts");
    assert!(setup.success());

    let map_path = |path: &str| {
        let output = Command::new("wsl.exe")
            .args(["-d", "Ubuntu", "--exec", "wslpath", "-a", path])
            .output()
            .expect("Windows path maps into Ubuntu");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let launcher_path = map_path(launcher());
    let shim_path = map_path(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("xuva-wsl.sh")
            .to_string_lossy(),
    );
    let node = Command::new("where.exe")
        .arg("node.exe")
        .output()
        .expect("Windows Node lookup starts");
    assert!(node.status.success());
    let node_windows_path = String::from_utf8_lossy(&node.stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.to_ascii_lowercase().ends_with("node.exe"))
        .expect("Windows Node is installed")
        .to_owned();
    let node_wsl_path = map_path(&node_windows_path);
    let script = concat!(
        "const fs=require('fs');",
        "fs.appendFileSync('invocations.txt','1\\n');",
        "console.log(JSON.stringify({cwd:process.cwd(),",
        "github:process.env.GITHUB_TOKEN||null,",
        "aws:process.env.AWS_SECRET_ACCESS_KEY||null}));"
    );
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r#"cd "$3"; export GITHUB_TOKEN=must-not-cross AWS_SECRET_ACCESS_KEY=must-not-cross XUVA_WINDOWS_EXE="$1"; exec sh "$2" --route raw node.exe -e "$4""#,
            "xuva-native-wsl-origin",
            &launcher_path,
            &shim_path,
            &project,
            script,
        ])
        .output()
        .expect("native WSL to Windows provider dispatch starts");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("provider emits one JSON report");
    assert!(report["github"].is_null());
    assert!(report["aws"].is_null());
    let cwd = report["cwd"]
        .as_str()
        .expect("provider reports its Windows CWD")
        .to_ascii_lowercase();
    assert!(cwd.starts_with(r"\\wsl"));
    assert!(cwd.ends_with(&project.replace('/', "\\").to_ascii_lowercase()));

    let explicit_script = concat!(
        "const fs=require('fs');",
        "fs.appendFileSync('invocations.txt','2\\n');",
        "process.stdout.write(process.env.GITHUB_TOKEN||'isolated');"
    );
    let explicit = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r#"cd "$3"; export GITHUB_TOKEN=must-not-cross XUVA_WINDOWS_EXE="$1"; exec sh "$2" --route raw "$4" -e "$5""#,
            "xuva-explicit-mounted-executable",
            &launcher_path,
            &shim_path,
            &project,
            &node_wsl_path,
            explicit_script,
        ])
        .output()
        .expect("mounted /mnt/.../node.exe dispatch starts");
    assert!(
        explicit.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&explicit.stdout),
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&explicit.stdout), "isolated");

    let count = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "cat",
            &format!("{project}/invocations.txt"),
        ])
        .output()
        .expect("invocation count is readable");
    assert!(count.status.success());
    assert_eq!(String::from_utf8_lossy(&count.stdout), "1\n2\n");

    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &project])
        .status()
        .expect("native WSL project cleanup starts");
    assert!(cleanup.success());
}

#[test]
fn wsl_provider_version_changes_require_explicit_refresh_within_ttl() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!(
        "/tmp/xuva-p7-go-version-cache-{}-{nonce}",
        std::process::id()
    );
    assert!(fixture.starts_with("/tmp/xuva-p7-go-version-cache-"));
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'if [ "$1" = "version" ]; then printf "go version fixture-v1\n"; else printf "fixture command\n"; fi' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-go-version-cache-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary WSL version-cache Go fixture setup starts");
    assert!(setup.status.success());

    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-go-version-cache-state-{nonce}"));
    let configure = |command: &mut Command| {
        command
            .env("PATH", &system32)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .env("XUVA_WSL_EXTRA_PATH", &fixture)
            .env("XUVA_OUTPUT_ADAPTER", "raw");
    };
    let mut initial = Command::new(launcher());
    configure(&mut initial);
    let initial = initial
        .args(["which", "go"])
        .output()
        .expect("initial Go lookup starts");
    assert!(initial.status.success());
    assert!(
        String::from_utf8_lossy(&initial.stdout).contains("cache=miss"),
        "initial lookup must populate the isolated cache"
    );

    let mutate = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"before=$(stat -Lc '%s:%Y' -- "$1/go") || exit 1; timestamp=$(printf '%s' "$before" | cut -d: -f2); printf '%s\n' '#!/bin/sh' 'if [ "$1" = "version" ]; then printf "go version fixture-v2\n"; else printf "fixture command\n"; fi' > "$1/go"; touch -d "@$timestamp" "$1/go"; after=$(stat -Lc '%s:%Y' -- "$1/go") || exit 1; test "$before" = "$after""###,
            "xuva-p7-go-version-cache-mutate",
            &fixture,
        ])
        .output()
        .expect("temporary WSL version-cache Go fixture mutation starts");
    assert!(
        mutate.status.success(),
        "fixture mutation stderr: {}",
        String::from_utf8_lossy(&mutate.stderr)
    );

    let mut after_version_change = Command::new(launcher());
    configure(&mut after_version_change);
    let after_version_change = after_version_change
        .args(["which", "go"])
        .output()
        .expect("version-changed Go lookup starts");
    let mut refreshed = Command::new(launcher());
    configure(&mut refreshed);
    let refreshed = refreshed
        .args(["which", "go", "--refresh"])
        .output()
        .expect("refreshed Go lookup starts");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL version-cache Go fixture cleanup starts");
    assert!(cleanup.success());
    let _ = std::fs::remove_dir_all(&state);

    assert!(after_version_change.status.success());
    assert!(
        String::from_utf8_lossy(&after_version_change.stdout).contains("cache=hit"),
        "warm lookups must honor the bounded TTL without a version subprocess: {}",
        String::from_utf8_lossy(&after_version_change.stdout)
    );
    assert!(refreshed.status.success());
    assert!(
        String::from_utf8_lossy(&refreshed.stdout).contains("cache=miss"),
        "explicit refresh must observe the changed provider version: {}",
        String::from_utf8_lossy(&refreshed.stdout)
    );
    assert!(
        String::from_utf8_lossy(&refreshed.stdout).contains("fixture-v2"),
        "refreshed provider evidence must include the new version: {}",
        String::from_utf8_lossy(&refreshed.stdout)
    );
}

#[test]
fn provider_cache_ignores_project_revision_until_explicit_refresh() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "xuva-p7-git-revision-cache-{}-{nonce}",
        std::process::id()
    ));
    let state = directory.join("state");
    std::fs::create_dir_all(&directory).expect("temporary Git project is created");
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&directory)
            .output()
            .expect("Git fixture command starts");
        assert!(
            output.status.success(),
            "git {arguments:?} stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init"]);
    git(&[
        "-c",
        "user.name=XUVA P7",
        "-c",
        "user.email=p7@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "first",
    ]);

    let which = || {
        Command::new(launcher())
            .env("XUVA_STATE_DIR", &state)
            .current_dir(&directory)
            .args(["which", "go"])
            .output()
            .expect("provider lookup starts")
    };
    let first = which();
    let second = which();
    git(&[
        "-c",
        "user.name=XUVA P7",
        "-c",
        "user.email=p7@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "second",
    ]);
    let after_revision_change = which();
    let refreshed = Command::new(launcher())
        .env("XUVA_STATE_DIR", &state)
        .current_dir(&directory)
        .args(["which", "go", "--refresh"])
        .output()
        .expect("refreshed provider lookup starts");
    std::fs::remove_dir_all(&directory).expect("temporary Git project is removed");

    assert!(first.status.success());
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("cache=miss"),
        "first lookup: {}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("cache=hit"),
        "second lookup: {}",
        String::from_utf8_lossy(&second.stdout)
    );
    assert!(after_revision_change.status.success());
    assert!(
        String::from_utf8_lossy(&after_revision_change.stdout).contains("cache=hit"),
        "unrelated Git revisions must not put a subprocess on the provider hot path: {}",
        String::from_utf8_lossy(&after_revision_change.stdout)
    );
    assert!(refreshed.status.success());
    assert!(
        String::from_utf8_lossy(&refreshed.stdout).contains("cache=miss"),
        "explicit refresh must revalidate provider discovery: {}",
        String::from_utf8_lossy(&refreshed.stdout)
    );
}

#[test]
fn invalidates_provider_cache_when_path_or_configured_distro_changes() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "xuva-p7-path-distro-cache-{}-{nonce}",
        std::process::id()
    ));
    let first_path = directory.join("first");
    let second_path = directory.join("second");
    let state = directory.join("state");
    std::fs::create_dir_all(&first_path).expect("first fixture path is created");
    std::fs::create_dir_all(&second_path).expect("second fixture path is created");
    std::fs::write(
        first_path.join("npm.cmd"),
        "@echo off\r\necho 11.0.0-first-fixture\r\n",
    )
    .expect("first npm fixture is written");
    std::fs::write(
        second_path.join("npm.cmd"),
        "@echo off\r\necho 11.0.0-second-fixture\r\n",
    )
    .expect("second npm fixture is written");
    let system32 =
        PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system root is available"))
            .join("System32");
    let lookup = |fixture_path: &PathBuf, distro: &str| {
        let path = std::env::join_paths([fixture_path.as_os_str(), system32.as_os_str()])
            .expect("fixture PATH is valid");
        Command::new(launcher())
            .env("PATH", path)
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", distro)
            .current_dir(&directory)
            .args(["which", "npm"])
            .output()
            .expect("provider lookup starts")
    };
    let first = lookup(&first_path, "Ubuntu");
    let second = lookup(&first_path, "Ubuntu");
    let after_path_change = lookup(&second_path, "Ubuntu");
    let after_distro_change = lookup(&second_path, "docker-desktop");
    std::fs::remove_dir_all(&directory).expect("temporary path fixture is removed");

    for output in [&first, &second, &after_path_change, &after_distro_change] {
        assert!(
            output.status.success(),
            "stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(String::from_utf8_lossy(&first.stdout).contains("cache=miss"));
    assert!(String::from_utf8_lossy(&second.stdout).contains("cache=hit"));
    assert!(
        String::from_utf8_lossy(&after_path_change.stdout).contains("cache=miss"),
        "a changed PATH must invalidate provider discovery: {}",
        String::from_utf8_lossy(&after_path_change.stdout)
    );
    assert!(
        String::from_utf8_lossy(&after_distro_change.stdout).contains("cache=miss"),
        "a changed configured distro must invalidate provider discovery: {}",
        String::from_utf8_lossy(&after_distro_change.stdout)
    );
}

#[test]
fn dispatches_wsl_only_cargo_raw_from_a_windows_shell() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!("/tmp/xuva-p7-cargo-{}-{nonce}", std::process::id());
    assert!(
        fixture.starts_with("/tmp/xuva-p7-cargo-"),
        "fixture cleanup target is constrained to the test namespace"
    );
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '#!/bin/sh\nprintf "cargo 1.99.0-wsl-fixture\\n"\n' > "$1/cargo"; chmod 755 "$1/cargo""###,
            "xuva-p7-cargo-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary WSL Cargo fixture setup starts");
    assert!(
        setup.status.success(),
        "fixture setup stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-cargo-state-{nonce}"));
    let output = Command::new(launcher())
        .env("PATH", system32)
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_WSL_DISTRO", "Ubuntu")
        .env("XUVA_WSL_EXTRA_PATH", &fixture)
        .env("XUVA_OUTPUT_ADAPTER", "raw")
        .args(["cargo", "--version"])
        .output()
        .expect("Cargo dispatcher starts");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL Cargo fixture cleanup starts");
    assert!(cleanup.success(), "temporary WSL Cargo fixture is removed");
    let _ = std::fs::remove_dir_all(&state);

    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "cargo 1.99.0-wsl-fixture\n"
    );
}

#[test]
fn dispatches_each_remaining_supported_wsl_only_toolchain_raw() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!(
        "/tmp/xuva-p7-generic-toolchain-{}-{nonce}",
        std::process::id()
    );
    assert!(fixture.starts_with("/tmp/xuva-p7-generic-toolchain-"));
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; for tool in node npm pnpm python python3 pytest java gradle mvn dotnet git; do printf '#!/bin/sh\nprintf "fixture-tool:%%s\\n" "$0"\n' > "$1/$tool"; chmod 755 "$1/$tool"; done"###,
            "xuva-p7-generic-toolchain-fixture",
            &fixture,
        ])
        .output()
        .expect("temporary generic WSL toolchain fixture setup starts");
    assert!(
        setup.status.success(),
        "fixture setup stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-generic-toolchain-state-{nonce}"));
    for tool in [
        "node", "npm", "pnpm", "python", "python3", "pytest", "java", "gradle", "mvn", "dotnet",
        "git",
    ] {
        let output = Command::new(launcher())
            .env("PATH", &system32)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .env("XUVA_WSL_EXTRA_PATH", &fixture)
            .env("XUVA_OUTPUT_ADAPTER", "raw")
            .args([tool, "--version"])
            .output()
            .unwrap_or_else(|error| panic!("{tool} dispatcher starts: {error}"));
        assert!(
            output.status.success(),
            "{tool} stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("fixture-tool:{fixture}/{tool}\n"),
            "{tool} must execute the discovered WSL-only binary"
        );
    }

    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary generic WSL toolchain fixture cleanup starts");
    assert!(
        cleanup.success(),
        "temporary generic WSL fixture is removed"
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn wsl_only_go_preserves_route_cwd_arguments_and_exit_code() {
    let _guard = process_contract_guard();
    let nonce = XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed);
    let fixture = format!("/tmp/xuva-p7-go-contract-{}-{nonce}", std::process::id());
    assert!(fixture.starts_with("/tmp/xuva-p7-go-contract-"));
    let identity = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "id", "-un"])
        .output()
        .expect("Ubuntu user lookup starts");
    assert!(identity.status.success());
    let wsl_user = String::from_utf8_lossy(&identity.stdout).trim().to_owned();
    let setup = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'printf "user:%s\n" "$(id -un)"' 'printf "cwd:%s\n" "$PWD"' 'printf "args:%s|%s\n" "$1" "$2"' 'exit 42' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-go-contract",
            &fixture,
        ])
        .output()
        .expect("temporary WSL Go contract fixture setup starts");
    assert!(setup.status.success());

    let directory = std::env::temp_dir().join(format!(
        "xuva p7 go cwd {} \u{6f22}\u{5b57}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary Windows worktree is created");
    let windows_path = directory.to_string_lossy().replace('\\', "/");
    let (drive, remainder) = windows_path
        .split_once(':')
        .expect("temporary worktree has a Windows drive prefix");
    let expected_cwd = format!("/mnt/{}{}", drive.to_lowercase(), remainder);
    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("Windows system root is available")
        .join("System32");
    let state = std::env::temp_dir().join(format!("xuva-p7-go-contract-state-{nonce}"));
    let literal = "space;and&dollar$HOME\\\u{6f22}\u{5b57}";
    let configure = |command: &mut Command| {
        command
            .env("PATH", &system32)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .env("XUVA_WSL_USER", &wsl_user)
            .env("XUVA_WSL_EXTRA_PATH", &fixture)
            .env("XUVA_OUTPUT_ADAPTER", "raw")
            .env_remove("XUVA_NATIVE_RTK_PATH")
            .env_remove("XUVA_WSL_RTK_PATH")
            .current_dir(&directory);
    };
    let mut explain = Command::new(launcher());
    configure(&mut explain);
    let explain = explain
        .args(["--explain-route", "go", "run", literal])
        .output()
        .expect("Go route explanation starts");
    let mut which = Command::new(launcher());
    configure(&mut which);
    let which = which
        .args(["which", "go"])
        .output()
        .expect("Go lookup starts");
    let mut doctor = Command::new(launcher());
    configure(&mut doctor);
    let doctor = doctor
        .args(["doctor", "go"])
        .output()
        .expect("Go provider diagnosis starts");
    let mut execution = Command::new(launcher());
    configure(&mut execution);
    let execution = execution
        .args(["go", "run", literal])
        .output()
        .expect("Go dispatcher starts");
    let mutate = Command::new("wsl.exe")
        .args([
            "-d",
            "Ubuntu",
            "--exec",
            "sh",
            "-c",
            r###"printf '%s\n' '#!/bin/sh' 'printf "go fixture identity changed substantially\\n"' 'exit 42' > "$1/go"; chmod 755 "$1/go""###,
            "xuva-p7-go-contract-mutate",
            &fixture,
        ])
        .output()
        .expect("temporary WSL Go fixture mutation starts");
    assert!(mutate.status.success());
    let mut which_after_mutation = Command::new(launcher());
    configure(&mut which_after_mutation);
    let which_after_mutation = which_after_mutation
        .args(["which", "go"])
        .output()
        .expect("Go lookup after fixture mutation starts");
    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--", &fixture])
        .status()
        .expect("temporary WSL Go contract fixture cleanup starts");
    assert!(cleanup.success());
    std::fs::remove_dir_all(&directory).expect("temporary Windows worktree is removed");
    let _ = std::fs::remove_dir_all(&state);

    assert!(explain.status.success());
    let explain_stdout = String::from_utf8_lossy(&explain.stdout);
    assert!(explain_stdout.contains("route=wsl2"));
    assert!(explain_stdout.contains("output_adapter=raw"));
    let which_stdout = String::from_utf8_lossy(&which.stdout);
    assert!(
        which.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&which.stderr)
    );
    assert!(which_stdout.contains("cache=hit"));
    assert!(which_stdout.contains(&format!("go_path={fixture}/go")));
    assert!(
        doctor.status.success(),
        "doctor stderr: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let doctor_stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(doctor_stdout.contains("tool=go"));
    assert!(doctor_stdout.contains(&format!(
        "inspected_distro=Ubuntu;user={wsl_user};wsl_version=2"
    )));
    assert!(doctor_stdout.contains(&format!("go_path={fixture}/go")));
    assert!(
        doctor_stdout.contains("candidate_0=Wsl2;adapters=[Raw, Rtk];distro=Ubuntu;usable=true")
    );
    assert!(doctor_stdout.contains(&format!("candidate_0_project_path={expected_cwd}")));
    assert!(doctor_stdout.contains("diagnosis=candidate 0 is verified"));
    assert!(which_after_mutation.status.success());
    assert!(
        String::from_utf8_lossy(&which_after_mutation.stdout).contains("cache=hit"),
        "warm lookup must remain bounded by TTL; use doctor --refresh for identity revalidation: {}",
        String::from_utf8_lossy(&which_after_mutation.stdout)
    );
    assert_eq!(execution.status.code(), Some(42));
    let execution_stdout = String::from_utf8_lossy(&execution.stdout);
    assert!(execution_stdout.contains(&format!("user:{wsl_user}")));
    assert!(
        execution_stdout.contains(&format!("cwd:{expected_cwd}")),
        "expected CWD {expected_cwd}; stdout: {execution_stdout}; stderr: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert!(execution_stdout.contains(&format!("args:run|{literal}")));
}

#[test]
fn xuva_raw_fast_path_avoids_scratch_and_gain_ledger_io() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let local_app_data = directory.join("local-app-data");

    let info = Command::new(&launcher)
        .arg("--adapter-info")
        .output()
        .expect("XUVA diagnostics start");
    assert!(info.status.success());
    assert!(String::from_utf8_lossy(&info.stdout).contains("adapter=xuva"));

    let explained = Command::new(&launcher)
        .args(["--explain-route", "git", "commit", "-m", "contract"])
        .output()
        .expect("XUVA route diagnostics start");
    assert!(explained.status.success());
    assert!(String::from_utf8_lossy(&explained.stdout).contains("route=raw"));

    let raw = Command::new(&launcher)
        .env("LOCALAPPDATA", &local_app_data)
        .args(["--route", "raw", "git", "--version"])
        .output()
        .expect("XUVA raw route starts");
    assert!(raw.status.success());
    assert!(String::from_utf8_lossy(&raw.stdout).starts_with("git version "));
    let scratch = local_app_data.join("xuva").join("scratch");
    assert!(
        !scratch.exists(),
        "the raw fast path must not create RTK tracker scratch state"
    );

    let gain = Command::new(&launcher)
        .env("LOCALAPPDATA", &local_app_data)
        .arg("gain")
        .output()
        .expect("XUVA gain starts");
    assert!(gain.status.success());
    let gain_stdout = String::from_utf8_lossy(&gain.stdout);
    assert!(gain_stdout.contains("XUVA Measured Token Accounting"));
    assert!(gain_stdout.contains("No RTK-measured commands yet."));

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn xuva_calibrates_safe_commands_across_natural_invocations() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let state = directory.join("state");
    let fake_rtk = directory.join("fake-rtk.cmd");
    std::fs::write(
        &fake_rtk,
        "@echo off\r\necho fake-native-rtk %*\r\nexit /b 0\r\n",
    )
    .expect("fake native RTK is written");

    let run = || {
        Command::new(&launcher)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
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

    let state_path = state.join("calibration-v3.json");
    let recorded = std::fs::read_to_string(state_path).expect("calibration state is written");
    assert!(recorded.contains("\"raw_samples_ms\": ["));
    assert!(recorded.contains("\"native_samples\": ["));
    assert!(!recorded.contains("git status"));

    let inspection = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
        .arg("calibration")
        .output()
        .expect("calibration inspection starts");
    assert!(inspection.status.success());
    assert!(String::from_utf8_lossy(&inspection.stdout).contains("phase=stable"));

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn agent_hook_preserves_rewrite_defer_deny_and_invalid_json_contracts() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let fake_rtk = directory.join("agent-rtk.cmd");
    std::fs::write(
        &fake_rtk,
        concat!(
            "@echo off\r\n",
            "if \"%XUVA_HOOK_MODE%\"==\"rewrite\" (echo {\"updatedInput\":{\"command\":\"rtk git status\"}} & exit /b 0)\r\n",
            "if \"%XUVA_HOOK_MODE%\"==\"invalid\" (echo not-json & exit /b 0)\r\n",
            "if \"%XUVA_HOOK_MODE%\"==\"stderr\" (echo upstream-note 1>&2 & exit /b 0)\r\n",
            "if \"%XUVA_HOOK_MODE%\"==\"stall\" (ping -n 30 127.0.0.1 >nul & exit /b 0)\r\n",
            "exit /b 0\r\n",
        ),
    )
    .expect("fake RTK hook is written");

    let rewrite = Command::new(&launcher)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .env("XUVA_HOOK_MODE", "rewrite")
        .args(["agent", "hook", "claude"])
        .output()
        .expect("rewrite hook starts");
    assert!(rewrite.status.success());
    assert!(String::from_utf8_lossy(&rewrite.stdout).contains("xuva git status"));

    for agent in ["claude", "cursor", "gemini", "copilot"] {
        let empty = Command::new(&launcher)
            .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
            .env("XUVA_HOOK_MODE", "empty")
            .args(["agent", "hook", agent])
            .output()
            .expect("pass-through hook starts");
        assert!(empty.status.success(), "{agent}");
        assert!(empty.stdout.is_empty(), "{agent}");
    }

    let stderr = Command::new(&launcher)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .env("XUVA_HOOK_MODE", "stderr")
        .args(["agent", "hook", "claude"])
        .output()
        .expect("stderr hook starts");
    assert!(stderr.status.success());
    assert!(String::from_utf8_lossy(&stderr.stderr).contains("upstream-note"));

    let invalid = Command::new(&launcher)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .env("XUVA_HOOK_MODE", "invalid")
        .args(["agent", "hook", "claude"])
        .output()
        .expect("invalid hook starts");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid JSON"));

    let stalled = Command::new(&launcher)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .env("XUVA_HOOK_MODE", "stall")
        .env("XUVA_AGENT_HOOK_TIMEOUT_MS", "50")
        .args(["agent", "hook", "claude"])
        .output()
        .expect("stalled hook starts");
    assert!(!stalled.status.success());
    assert!(String::from_utf8_lossy(&stalled.stderr).contains("timed out"));

    let mut oversized = Command::new(&launcher)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .args(["agent", "hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("oversized hook starts");
    oversized
        .stdin
        .take()
        .expect("hook stdin is piped")
        .write_all(&vec![b'x'; 1024 * 1024 + 1])
        .expect("oversized fixture is written");
    let oversized = oversized
        .wait_with_output()
        .expect("oversized hook completes");
    assert!(!oversized.status.success());
    assert!(String::from_utf8_lossy(&oversized.stderr).contains("exceeds"));

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn xuva_rejects_unsafe_generic_provider_names_before_discovery() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let output = Command::new(&launcher)
        .args(["resolve", "tool;not-run"])
        .output()
        .expect("provider validation starts");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tool names must contain only ASCII"));
    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
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
        .find(|candidate| candidate["host"] == "windows")
        .expect("Windows Git provider is discovered")
        .clone()
}

#[test]
fn xuva_resolve_verifies_wsl_project_paths_for_windows_providers() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let state = directory.join("state");
    let project_directory = std::env::current_dir().expect("test project directory is available");
    let expected_project_path = project_directory.to_string_lossy().to_string();
    let project_path = expected_project_path.replace('\\', "/");
    let (drive, remainder) = project_path
        .split_once(':')
        .expect("test project directory has a Windows drive prefix");
    let mounted_project_path = format!("/mnt/{}{}", drive.to_lowercase(), remainder);
    let mut distros = vec!["Ubuntu".to_owned()];
    if let Ok(wsl1_distro) = std::env::var("XUVA_WSL1_TEST_DISTRO")
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
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", &distro)
            .env("XUVA_WSL_USER", &user)
            .env("XUVA_WSL_CWD", &mounted_project_path)
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
            "/tmp/xuva-p13-native-{}-{}",
            std::process::id(),
            distro.replace(|character: char| !character.is_ascii_alphanumeric(), "-")
        );
        let created = Command::new("wsl.exe")
            .args(["-d", &distro, "--exec", "mkdir", "-p", &native_path])
            .status()
            .expect("temporary native WSL project is created");
        assert!(created.success());
        let native = Command::new(&launcher)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", &distro)
            .env("XUVA_WSL_USER", &user)
            .env("XUVA_WSL_CWD", &native_path)
            .args(["resolve", "git", "--json", "--refresh"])
            .output()
            .expect("native WSL project resolution starts");
        let native_candidate = resolved_windows_candidate(&native);
        assert_eq!(native_candidate["usable"], true);
        let native_windows_path = native_candidate["project_path"]
            .as_str()
            .expect("native WSL project has a Windows path");
        let expected_prefix = format!(
            r"\\wsl.localhost\{}\tmp\xuva-p13-native-{}-",
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
    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

fn available_provider_candidate_index(
    resolution: &serde_json::Value,
    host: &str,
    distro: Option<&str>,
) -> Option<usize> {
    resolution["candidates"]
        .as_array()
        .expect("provider resolution lists candidates")
        .iter()
        .position(|candidate| {
            candidate["host"] == host
                && distro.is_none_or(|distro| candidate["distro"] == distro)
                && candidate["usable"] == true
        })
}

#[test]
fn xuva_provider_exec_runs_each_available_verified_provider_without_replay() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
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
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_WSL_DISTRO", "Ubuntu")
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
        .env("XUVA_STATE_DIR", &native_state)
        .env("XUVA_NATIVE_RTK_PATH", &fake_rtk)
        .env("XUVA_WSL_DISTRO", "Ubuntu")
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
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_WSL_DISTRO", "Ubuntu")
        .args(["resolve", "git", "--json", "--refresh"])
        .output()
        .expect("Git provider resolution starts");
    assert!(resolution.status.success());
    let resolution: serde_json::Value =
        serde_json::from_slice(&resolution.stdout).expect("Git provider resolution is JSON");
    assert!(
        available_provider_candidate_index(&resolution, "windows", None).is_some(),
        "a missing WSL/RTK fixture must leave the verified Windows raw provider usable"
    );
    if std::env::var_os("XUVA_WSL1_TEST_DISTRO").is_none()
        && let Some(wsl_rtk_index) =
            available_provider_candidate_index(&resolution, "wsl2", Some("Ubuntu"))
    {
        let wsl_rtk = Command::new(&launcher)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
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
    }

    if std::env::var_os("XUVA_WSL1_TEST_DISTRO").is_some()
        && let Some(wsl_raw_index) =
            available_provider_candidate_index(&resolution, "wsl1", Some("Ubuntu-RTK-WSL1"))
    {
        let wsl_raw = Command::new(&launcher)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
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
        assert!(
            wsl_raw.status.success(),
            "stdout: {}; stderr: {}",
            String::from_utf8_lossy(&wsl_raw.stdout),
            String::from_utf8_lossy(&wsl_raw.stderr)
        );
        assert!(String::from_utf8_lossy(&wsl_raw.stdout).starts_with("git version "));

        let windows_project = env!("CARGO_MANIFEST_DIR");
        let rendered = windows_project.replace('\\', "/");
        let (drive, remainder) = rendered
            .split_once(':')
            .expect("manifest directory has a Windows drive");
        let expected_wsl_project = format!("/mnt/{}{}", drive.to_ascii_lowercase(), remainder);
        let translated = Command::new(&launcher)
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .args([
                "provider",
                "exec",
                "git",
                "--candidate",
                &wsl_raw_index.to_string(),
                "--",
                "-C",
                windows_project,
                "rev-parse",
                "--show-toplevel",
            ])
            .output()
            .expect("foreign-path translation starts");
        assert!(
            translated.status.success(),
            "stdout: {}; stderr: {}",
            String::from_utf8_lossy(&translated.stdout),
            String::from_utf8_lossy(&translated.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&translated.stdout).trim(),
            expected_wsl_project
        );
    }

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn windows_batch_boundary_preserves_supported_literals_and_rejects_line_injection() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let capture = directory.join("capture.js");
    let wrapper = directory.join("capture.cmd");
    std::fs::write(
        &capture,
        "process.stdout.write(JSON.stringify(process.argv.slice(2)));",
    )
    .expect("batch capture script is written");
    std::fs::write(&wrapper, "@echo off\r\nnode.exe \"%~dp0capture.js\" %*\r\n")
        .expect("batch wrapper is written");
    let literals = ["%NAME%", "!NAME!", "^&", "\"", r"trailing\", "", "ending\""];
    let output = Command::new(&launcher)
        .env("NAME", "must-not-expand")
        .arg(&wrapper)
        .args(literals)
        .output()
        .expect("batch literal contract starts");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let observed: Vec<String> =
        serde_json::from_slice(&output.stdout).expect("batch arguments are JSON");
    assert_eq!(observed, literals);

    let rejected = Command::new(&launcher)
        .arg(&wrapper)
        .arg("safe\r\nwhoami")
        .output()
        .expect("batch line-injection rejection starts");
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("Windows batch arguments must not contain CR or LF")
    );

    std::fs::remove_dir_all(directory).expect("batch fixture is removed");
}

#[test]
fn xuva_surface_matches_the_live_wsl_rtk_command_inventory_when_provider_is_available() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let surface = Command::new(&launcher)
        .args(["surface", "--json"])
        .output()
        .expect("surface report starts");
    assert!(surface.status.success());
    let surface: serde_json::Value =
        serde_json::from_slice(&surface.stdout).expect("surface report is JSON");
    assert_eq!(surface["adapter"]["name"], "rtk");
    assert_eq!(surface["adapter"]["version"], "0.43.0");
    assert_eq!(surface["adapter"]["protocol_version"], 1);
    assert_eq!(surface["upstream_command_count"], 69);
    let xuva_commands = surface["commands"]
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
    if !help.status.success() {
        std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
        return;
    }
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
    assert_eq!(xuva_commands, live_commands);
    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn xuva_policy_requires_a_matching_local_adapter_context() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let state = directory.join("state");
    let context = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
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
    assert_eq!(context["manifest_version"], "rtk:0.43.0:protocol-1");

    let policy = serde_json::json!({
        "schema_version": 2,
        "manifest_version": "rtk:0.43.0:protocol-1",
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
        .env("XUVA_STATE_DIR", &state)
        .args(["policy", "import", source.to_str().expect("policy path")])
        .output()
        .expect("policy import starts");
    assert!(imported.status.success());

    let selected = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
        .args(["--explain-route", "rg", "needle"])
        .output()
        .expect("matching policy explanation starts");
    assert!(selected.status.success());
    assert!(String::from_utf8_lossy(&selected.stdout).contains("route=raw"));

    let alternate_rtk = directory.join("other-rtk.cmd");
    std::fs::write(&alternate_rtk, "@echo off\r\nexit /b 0\r\n")
        .expect("alternate RTK fixture is written");
    let invalidated = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
        .env("XUVA_NATIVE_RTK_PATH", &alternate_rtk)
        .args(["--explain-route", "rg", "needle"])
        .output()
        .expect("changed-context explanation starts");
    assert!(invalidated.status.success());
    assert!(String::from_utf8_lossy(&invalidated.stdout).contains("route=native-rtk"));

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn xuva_generic_setup_is_diagnostic_only_and_never_creates_an_install_transaction() {
    let _guard = process_contract_guard();
    let (launcher, directory) = xuva_launcher();
    let state = directory.join("state");

    let ready = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
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
        .env("XUVA_STATE_DIR", &state)
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
        .env("XUVA_STATE_DIR", &state)
        .args(["doctor", missing_tool, "--refresh"])
        .output()
        .expect("generic missing-provider doctor starts");
    assert!(!doctor.status.success());
    let doctor_stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(doctor_stdout.contains("recommended=none"));
    assert!(doctor_stdout.contains("diagnosis=no verified provider is available"));
    assert!(doctor_stdout.contains("setup p17-tool-that-is-not-installed"));

    let forced = Command::new(&launcher)
        .env("XUVA_STATE_DIR", &state)
        .args(["setup", missing_tool, "--apply", "--confirm"])
        .output()
        .expect("generic forced setup starts");
    assert!(!forced.status.success());
    assert!(String::from_utf8_lossy(&forced.stderr).contains("diagnostic-only"));
    assert!(!state.join("setup-transaction-v1.json").exists());

    std::fs::remove_dir_all(directory).expect("temporary XUVA directory is removed");
}

#[test]
fn provisioned_wsl1_bridge_preserves_the_process_contract_when_requested() {
    let _guard = process_contract_guard();
    let Ok(distro) = std::env::var("XUVA_WSL1_TEST_DISTRO") else {
        return;
    };
    let literal = "wsl1 space/漢字;and&dollar$HOME\\tail";
    let output = Command::new(launcher())
        .env("XUVA_ROUTE", "wsl1")
        .env("XUVA_WSL_BACKEND", "wsl1")
        .env("XUVA_WSL_DISTRO", distro)
        .env("XUVA_WSL_RTK_PATH", "/usr/bin/printf")
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
fn wsl1_route_rejects_a_non_dedicated_or_wrong_version_override() {
    let _guard = process_contract_guard();
    let output = Command::new(launcher())
        .env("XUVA_ROUTE", "wsl1")
        .env("XUVA_WSL_BACKEND", "wsl1")
        .env("XUVA_WSL_DISTRO", "Ubuntu")
        .env("XUVA_WSL_RTK_PATH", "/usr/bin/printf")
        .arg("must-not-run")
        .output()
        .expect("unsafe WSL1 override validation starts");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("WSL1 route requires a version-1 distro")
    );
    let still_available = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "/usr/bin/env", "true"])
        .status()
        .expect("ordinary Ubuntu remains available");
    assert!(
        still_available.success(),
        "unsafe WSL1 override validation must not terminate Ubuntu"
    );
}

#[test]
fn ctrl_break_releases_the_global_lock_for_waiting_children() {
    let _guard = process_contract_guard();
    let ready_file = std::env::temp_dir().join(format!("xuva-ready-{}", std::process::id()));
    let first_stderr_path =
        std::env::temp_dir().join(format!("xuva-first-stderr-{}", std::process::id()));
    let _ = std::fs::remove_file(&ready_file);
    let _ = std::fs::remove_file(&first_stderr_path);
    let first_stderr_file =
        std::fs::File::create(&first_stderr_path).expect("first stderr file is created");
    let mut first = command("/bin/sh")
        .args(["-c", "sleep 30"])
        .env("XUVA_WSL_TRACE", "1")
        .env("XUVA_METRICS", "off")
        .env("XUVA_WSL_TEST_READY_FILE", &ready_file)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stderr(Stdio::from(first_stderr_file))
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
        let stderr = std::fs::read_to_string(&first_stderr_path)
            .expect("first stderr is readable after exit");
        panic!("first launcher exited before cancellation: status={status}; stderr={stderr}");
    }

    let mut second = command("/usr/bin/printf")
        .args(["released"])
        .env("XUVA_WSL_TRACE", "1")
        .env("XUVA_METRICS", "off")
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
        let first_stderr =
            std::fs::read_to_string(&first_stderr_path).unwrap_or_else(|_| "<unreadable>".into());
        panic!(
            "second launcher did not wait for the lock: status={status}; stderr={stderr}; first_stderr={first_stderr}"
        );
    }

    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, first.id()) };
    assert_ne!(
        sent, 0,
        "failed to send CTRL_BREAK_EVENT to launcher process group"
    );
    let cancellation_started = Instant::now();
    let first_status = first.wait().expect("interrupted launcher exits");
    let first_stderr =
        std::fs::read_to_string(&first_stderr_path).expect("first stderr reads after exit");
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
    let _ = std::fs::remove_file(first_stderr_path);
}

#[test]
fn wsl2_cancellation_escalates_past_ignored_sigint_and_leaves_no_worker() {
    if std::env::var_os("XUVA_WSL1_TEST_DISTRO").is_some() {
        return;
    }
    let _guard = process_contract_guard();
    let distro = std::env::var("XUVA_WSL2_TEST_DISTRO").unwrap_or_else(|_| "Ubuntu".to_owned());
    let directory = unique_temp_directory("wsl2-signal-escalation");
    std::fs::create_dir_all(&directory).expect("signal fixture directory is created");
    let ready_file = directory.join("ready");
    let pid_file = directory.join("worker.pid");
    let mapped_pid = Command::new("wsl.exe")
        .args(["-d", &distro, "--exec", "wslpath", "-a"])
        .arg(&pid_file)
        .output()
        .expect("PID path mapping starts");
    assert!(mapped_pid.status.success());
    let mapped_pid = String::from_utf8_lossy(&mapped_pid.stdout)
        .trim()
        .to_owned();
    let mut child = command("/bin/sh")
        .args([
            "-c",
            "trap '' INT; printf '%s' \"$$\" > \"$1\"; while :; do sleep 1; done",
            "xuva-ignore-int",
            &mapped_pid,
        ])
        .env("XUVA_WSL_TEST_READY_FILE", &ready_file)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stderr(Stdio::piped())
        .spawn()
        .expect("SIGINT-resistant launcher starts");
    let ready_deadline = Instant::now() + Duration::from_secs(30);
    while !ready_file.exists() || !pid_file.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "SIGINT-resistant worker did not become ready"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let worker_pid = std::fs::read_to_string(&pid_file)
        .expect("worker PID is readable")
        .trim()
        .to_owned();
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    assert_ne!(sent, 0, "failed to send CTRL_BREAK_EVENT");
    let started = Instant::now();
    let status = child.wait().expect("escalated cancellation completes");
    assert!(!status.success());
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "SIGINT escalation exceeded its contract"
    );
    let probe = Command::new("wsl.exe")
        .args([
            "-d",
            &distro,
            "--exec",
            "/bin/sh",
            "-c",
            "! /bin/kill -0 \"$1\" 2>/dev/null",
            "xuva-worker-probe",
            &worker_pid,
        ])
        .status()
        .expect("worker liveness probe starts");
    assert!(
        probe.success(),
        "Linux worker survived cancellation escalation"
    );
    std::fs::remove_dir_all(directory).expect("signal fixture directory is removed");
}

#[test]
fn ctrl_break_cancels_from_a_temp_windows_worktree() {
    let _guard = process_contract_guard();
    let directory = std::env::temp_dir().join(format!(
        "xuva-windows-cancel-contract-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("temporary Windows worktree is created");
    let ready_file = directory.join("ready");
    let mut child = command("/bin/sh")
        .current_dir(&directory)
        .args(["-c", "sleep 30"])
        .env("XUVA_WSL_TEST_READY_FILE", &ready_file)
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

#[test]
fn ctrl_break_cancels_a_raw_windows_node_child() {
    let _guard = process_contract_guard();
    let ready_file = std::env::temp_dir().join(format!(
        "xuva-p7-raw-node-ready-{}-{}",
        std::process::id(),
        XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let node_program =
        "require('fs').writeFileSync(process.env.P7_READY, 'ready'); setInterval(() => {}, 1000)";
    let mut child = Command::new(launcher())
        .env("XUVA_OUTPUT_ADAPTER", "raw")
        .env("P7_READY", &ready_file)
        .args(["node", "-e", node_program])
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stderr(Stdio::piped())
        .spawn()
        .expect("raw Windows Node launcher starts");
    let ready_deadline = Instant::now() + Duration::from_secs(20);
    while !ready_file.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "raw Windows Node child did not become ready"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    assert_ne!(sent, 0, "failed to send CTRL_BREAK_EVENT to raw launcher");
    let cancelled = Instant::now();
    let status = child.wait().expect("interrupted raw launcher exits");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("raw launcher stderr is piped")
        .read_to_string(&mut stderr)
        .expect("raw launcher stderr reads");
    assert!(
        cancelled.elapsed() < Duration::from_secs(5),
        "raw launcher exceeded the cancellation deadline: stderr={stderr}"
    );
    assert!(
        !status.success(),
        "raw launcher unexpectedly succeeded after Ctrl+Break: stderr={stderr}"
    );
    std::fs::remove_file(&ready_file).expect("temporary raw Node readiness file is removed");
}

#[test]
fn native_git_creates_commit_objects_in_an_ntfs_worktree() {
    let _guard = process_contract_guard();
    let directory = unique_temp_directory("ntfs-git");
    std::fs::create_dir_all(&directory).expect("temporary NTFS worktree is created");
    let init = Command::new("git.exe")
        .args(["init", "--quiet"])
        .arg(&directory)
        .status()
        .expect("Git for Windows starts");
    assert!(init.success());
    for (key, value) in [
        ("user.name", "XUVA Contract"),
        ("user.email", "xuva-contract@example.invalid"),
    ] {
        let status = Command::new("git.exe")
            .arg("-C")
            .arg(&directory)
            .args(["config", key, value])
            .status()
            .expect("Git for Windows configuration starts");
        assert!(status.success());
    }
    std::fs::write(directory.join("fixture.txt"), "native object write\n")
        .expect("fixture file writes");
    let state = directory.join("state");
    for arguments in [
        vec![
            "git",
            "-C",
            directory.to_str().unwrap(),
            "add",
            "fixture.txt",
        ],
        vec![
            "git",
            "-C",
            directory.to_str().unwrap(),
            "commit",
            "--quiet",
            "-m",
            "NTFS object contract",
        ],
    ] {
        let output = Command::new(launcher())
            .env("XUVA_STATE_DIR", &state)
            .args(arguments)
            .output()
            .expect("XUVA Git mutation starts");
        assert!(
            output.status.success(),
            "stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let verify = Command::new("git.exe")
        .arg("-C")
        .arg(&directory)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
        .expect("native Git commit verification starts");
    assert!(
        verify.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&verify.stderr)
    );

    let explain = Command::new(launcher())
        .env("XUVA_STATE_DIR", &state)
        .args(["--explain-route", "git", "push", "origin", "HEAD"])
        .output()
        .expect("Git network route explanation starts");
    let stdout = String::from_utf8_lossy(&explain.stdout);
    assert!(explain.status.success(), "{stdout}");
    assert!(stdout.contains("route=raw"), "{stdout}");
    assert!(stdout.contains("Windows DNS"), "{stdout}");
    std::fs::remove_dir_all(&directory).expect("temporary NTFS worktree is removed");
}

#[test]
fn read_accepts_relative_windows_and_wsl_drive_paths() {
    let _guard = process_contract_guard();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join("Cargo.toml");
    let rendered = manifest.to_string_lossy();
    let bytes = rendered.as_bytes();
    assert!(bytes.len() > 3 && bytes[1] == b':');
    let wsl_path = format!(
        "/mnt/{}/{}",
        (bytes[0] as char).to_ascii_lowercase(),
        rendered[3..].replace('\\', "/")
    );
    let state = unique_temp_directory("read-state");
    for argument in ["Cargo.toml".to_owned(), rendered.into_owned(), wsl_path] {
        let output = Command::new(launcher())
            .current_dir(&root)
            .env("XUVA_STATE_DIR", &state)
            .args(["read", &argument])
            .output()
            .expect("XUVA read starts");
        assert!(
            output.status.success(),
            "argument={argument}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("name = \"xuva\""));
    }
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn posix_find_head_and_tail_use_raw_wsl_semantics() {
    let _guard = process_contract_guard();
    let state = unique_temp_directory("find-state");
    let explain = Command::new(launcher())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XUVA_STATE_DIR", &state)
        .args(["--explain-route", "find", ".", "-maxdepth", "0", "-print"])
        .output()
        .expect("find route explanation starts");
    let stdout = String::from_utf8_lossy(&explain.stdout);
    assert!(explain.status.success(), "{stdout}");
    assert!(stdout.contains("output_adapter=raw"), "{stdout}");
    assert!(stdout.contains("WSL Ubuntu"), "{stdout}");

    let output = Command::new(launcher())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XUVA_STATE_DIR", &state)
        .args(["find", ".", "-maxdepth", "0", "-print"])
        .output()
        .expect("POSIX find starts");
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), ".");

    for (arguments, expected) in [
        (["head", "-n", "1", "Cargo.toml"], "[package]"),
        (["tail", "-n", "1", "Cargo.toml"], "serde_json = \"1\""),
    ] {
        let output = Command::new(launcher())
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("XUVA_STATE_DIR", &state)
            .args(arguments)
            .output()
            .expect("POSIX line utility starts");
        assert!(
            output.status.success(),
            "argv={arguments:?}; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
    }
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn wsl_provider_receives_only_safe_forwarded_environment() {
    let _guard = process_contract_guard();
    let fixture = format!(
        "/tmp/xuva-env-contract-{}-{}",
        std::process::id(),
        XUVA_LAUNCHER_NONCE.fetch_add(1, Ordering::Relaxed)
    );
    assert!(fixture.starts_with("/tmp/xuva-env-contract-"));
    let setup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "sh", "-c"])
        .arg(
            r#"mkdir -p "$1"; printf '%s\n' '#!/bin/sh' 'exec /usr/bin/printenv "$@"' > "$1/xuva-env-contract"; chmod 755 "$1/xuva-env-contract""#,
        )
        .arg("xuva-env-fixture")
        .arg(&fixture)
        .status()
        .expect("WSL environment fixture setup starts");
    assert!(setup.success());

    let state = unique_temp_directory("env-state");
    let run = |name: &str, value: &str, allowlist: Option<&str>| {
        let mut command = Command::new(launcher());
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("XUVA_STATE_DIR", &state)
            .env("XUVA_WSL_DISTRO", "Ubuntu")
            .env("XUVA_WSL_EXTRA_PATH", &fixture)
            .env(name, value)
            .args(["xuva-env-contract", name]);
        if let Some(allowlist) = allowlist {
            command.env("XUVA_ENV_ALLOWLIST", allowlist);
        }
        command.output().expect("environment fixture starts")
    };

    let automatic = run("XPDE_RUN_TRAINING_E2E", "1", None);
    assert!(
        automatic.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&automatic.stdout),
        String::from_utf8_lossy(&automatic.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&automatic.stdout).trim(), "1");

    let explicit = run("PROJECT_MODE", "training", Some("PROJECT_MODE"));
    assert!(
        explicit.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&explicit.stdout),
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&explicit.stdout).trim(), "training");

    let secret = run("PROJECT_RUN_SECRET_TOKEN", "1", None);
    assert_eq!(secret.status.code(), Some(1));
    assert!(secret.stdout.is_empty());

    let cleanup = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--exec", "rm", "-rf", "--"])
        .arg(&fixture)
        .status()
        .expect("WSL environment fixture cleanup starts");
    assert!(cleanup.success());
    let _ = std::fs::remove_dir_all(state);
}
