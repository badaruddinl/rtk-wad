//! Bounded child-process helpers used by diagnostic and adapter subprocesses.
//!
//! Probes must never inherit an unbounded wait or collect unbounded output.
//! The implementation uses only the standard library and drains both pipes
//! concurrently, keeping XUVA's runtime dependency and memory footprint small.

use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const PROBE_OUTPUT_LIMIT: usize = 64 * 1024;

#[cfg(windows)]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = Command::new("taskkill.exe")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(windows))]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

fn drain_bounded(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = remaining.min(count);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((retained, truncated))
}

pub(crate) fn run_bounded(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    output_limit: usize,
) -> io::Result<BoundedOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr pipe was not created"))?;
    let stdout_reader = thread::spawn(move || drain_bounded(stdout, output_limit));
    let stderr_reader = thread::spawn(move || drain_bounded(stderr, output_limit));
    let stdin_writer = input.map(|input| {
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was requested before the child was spawned");
        thread::spawn(move || {
            let result = stdin.write_all(&input);
            drop(stdin);
            result
        })
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            if let Some(writer) = stdin_writer {
                let _ = writer.join();
            }
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("child process timed out after {} ms", timeout.as_millis()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };

    if let Some(writer) = stdin_writer {
        writer
            .join()
            .map_err(|_| io::Error::other("child stdin writer panicked"))??;
    }
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| io::Error::other("child stdout reader panicked"))??;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| io::Error::other("child stderr reader panicked"))??;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

pub(crate) fn run_probe(command: &mut Command) -> io::Result<BoundedOutput> {
    run_bounded(command, None, PROBE_TIMEOUT, PROBE_OUTPUT_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn bounded_runner_times_out_a_stalled_child() {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/s", "/c", "ping -n 30 127.0.0.1 >nul"]);
        let error = run_bounded(
            &mut command,
            None,
            Duration::from_millis(50),
            PROBE_OUTPUT_LIMIT,
        )
        .expect_err("the stalled child must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[cfg(windows)]
    #[test]
    fn bounded_runner_drains_but_limits_retained_output() {
        let mut command = Command::new("cmd.exe");
        command.args([
            "/d",
            "/s",
            "/c",
            "for /L %i in (1,1,200) do @echo 0123456789",
        ]);
        let output = run_bounded(&mut command, None, Duration::from_secs(5), 128)
            .expect("fixture completes");
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 128);
        assert!(output.stdout_truncated);
    }
}
