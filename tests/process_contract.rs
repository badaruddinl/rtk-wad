#![cfg(target_os = "windows")]

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
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
    command
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
fn ctrl_break_releases_the_global_lock_for_waiting_children() {
    let mut first = command("/bin/sh")
        .args(["-c", "sleep 30"])
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .expect("first launcher starts");
    thread::sleep(Duration::from_secs(3));

    let mut second = command("/usr/bin/printf")
        .args(["released"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("second launcher starts");
    thread::sleep(Duration::from_secs(1));
    assert!(
        second
            .try_wait()
            .expect("second status is available")
            .is_none(),
        "second launcher did not wait for the lock"
    );

    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, first.id()) };
    assert_ne!(
        sent, 0,
        "failed to send CTRL_BREAK_EVENT to launcher process group"
    );
    let first_status = first.wait().expect("interrupted launcher exits");
    assert!(
        !first_status.success(),
        "interrupted launcher unexpectedly succeeded"
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
        assert!(
            Instant::now() < deadline,
            "waiting launcher did not continue after cancellation"
        );
        thread::sleep(Duration::from_millis(100));
    }
}
