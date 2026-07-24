#![cfg(target_os = "windows")]

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CTRL_BREAK_EVENT: u32 = 1;

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
    let directory =
        std::env::temp_dir().join(format!("rtk-wad-process-contract-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary WAD directory is created");
    let wad = directory.join("rtk-wad.exe");
    std::fs::copy(launcher(), &wad).expect("test launcher is copied under the WAD command name");
    (wad, directory)
}

#[test]
fn preserves_stdout_stderr_exit_codes_and_literal_arguments() {
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
    assert!(gain_stdout.contains("RTK-WAD Token Savings"));
    assert!(gain_stdout.contains("Invocations: 1"));

    std::fs::remove_dir_all(directory).expect("temporary WAD directory is removed");
}

#[test]
fn provisioned_wsl1_bridge_preserves_the_process_contract_when_requested() {
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
