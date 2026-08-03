//! Bounded child-process helpers used by diagnostic and adapter subprocesses.
//!
//! Probes must never inherit an unbounded wait or collect unbounded output.
//! The implementation uses only the standard library and drains both pipes
//! concurrently, keeping XUVA's runtime dependency and memory footprint small.

use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const PROBE_OUTPUT_LIMIT: usize = 64 * 1024;

#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::io;
    use std::mem;
    use std::os::windows::io::AsRawHandle;

    type Handle = *mut c_void;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            information_class: i32,
            information: *const c_void,
            information_length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub(crate) struct ProcessJob {
        handle: Handle,
    }

    impl ProcessJob {
        pub(crate) fn assign(child: &std::process::Child) -> io::Result<Self> {
            Self::assign_with_limits(child, true)
        }

        pub(crate) fn assign_for_timeout(child: &std::process::Child) -> io::Result<Self> {
            Self::assign_with_limits(child, false)
        }

        fn assign_with_limits(
            child: &std::process::Child,
            kill_on_close: bool,
        ) -> io::Result<Self> {
            // SAFETY: all pointers are null or point to initialized FFI structs
            // for the duration of each call. The returned handle is owned here.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut limits = ExtendedLimitInformation::default();
                if kill_on_close {
                    limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                }
                if SetInformationJobObject(
                    handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&raw const limits).cast(),
                    u32::try_from(mem::size_of::<ExtendedLimitInformation>())
                        .expect("job information fits in a DWORD"),
                ) == 0
                {
                    let error = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(error);
                }
                if AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0 {
                    let error = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(error);
                }
                Ok(Self { handle })
            }
        }

        pub(crate) fn terminate(&self) {
            // SAFETY: handle remains valid until Drop.
            unsafe {
                let _ = TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for ProcessJob {
        fn drop(&mut self) {
            // SAFETY: this object uniquely owns the job handle.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut std::process::Child, process_job: Option<&job::ProcessJob>) {
    if let Some(process_job) = process_job {
        process_job.terminate();
    }
    let _ = Command::new("taskkill.exe")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(windows))]
fn terminate_process_tree(child: &mut std::process::Child, _process_job: Option<&()>) {
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

/// Waits for a bounded child whose protocol is carried entirely by its exit
/// status. Stdio is deliberately disconnected before spawn: Windows/WSL can
/// keep inherited pipe handles alive after the direct proxy has exited, while
/// status-only control probes have no output payload to drain.
pub(crate) fn run_status_bounded(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<ExitStatus> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let process_job = match job::ProcessJob::assign_for_timeout(&child) {
        Ok(job) => Some(job),
        Err(error) => {
            terminate_process_tree(&mut child, None);
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "unable to supervise the bounded subprocess tree: {error}"
            )));
        }
    };
    #[cfg(not(windows))]
    let process_job: Option<()> = None;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child, process_job.as_ref());
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("child process timed out after {} ms", timeout.as_millis()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
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
    } else {
        // Discovery/version probes and other bounded helpers must never consume
        // bytes intended for the eventual interactive provider process.
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let process_job = match job::ProcessJob::assign(&child) {
        Ok(job) => Some(job),
        Err(error) => {
            terminate_process_tree(&mut child, None);
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "unable to supervise the bounded subprocess tree: {error}"
            )));
        }
    };
    #[cfg(not(windows))]
    let process_job: Option<()> = None;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr pipe was not created"))?;
    let (stdout_sender, stdout_receiver) = mpsc::channel();
    let (stderr_sender, stderr_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = stdout_sender.send(drain_bounded(stdout, output_limit));
    });
    thread::spawn(move || {
        let _ = stderr_sender.send(drain_bounded(stderr, output_limit));
    });
    let (stdin_sender, stdin_receiver) = mpsc::channel();
    let has_input = input.is_some();
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was requested before the child was spawned");
        thread::spawn(move || {
            let result = stdin.write_all(&input);
            drop(stdin);
            let _ = stdin_sender.send(result);
        });
    }

    let started = Instant::now();
    let mut status = None;
    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut stdin_result = (!has_input).then_some(Ok(()));
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        receive_once(&stdout_receiver, &mut stdout_result, "stdout reader")?;
        receive_once(&stderr_receiver, &mut stderr_result, "stderr reader")?;
        if has_input {
            receive_once(&stdin_receiver, &mut stdin_result, "stdin writer")?;
        }
        if status.is_some()
            && stdout_result.is_some()
            && stderr_result.is_some()
            && stdin_result.is_some()
        {
            break;
        }

        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child, process_job.as_ref());
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "child process or inherited pipe tree timed out after {} ms",
                    timeout.as_millis()
                ),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }

    stdin_result.expect("completion was checked")?;
    let (stdout, stdout_truncated) = stdout_result.expect("completion was checked")?;
    let (stderr, stderr_truncated) = stderr_result.expect("completion was checked")?;
    Ok(BoundedOutput {
        status: status.expect("completion was checked"),
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn receive_once<T>(
    receiver: &mpsc::Receiver<io::Result<T>>,
    result: &mut Option<io::Result<T>>,
    label: &str,
) -> io::Result<()> {
    if result.is_some() {
        return Ok(());
    }
    match receiver.try_recv() {
        Ok(value) => {
            *result = Some(value);
            Ok(())
        }
        Err(TryRecvError::Empty) => Ok(()),
        Err(TryRecvError::Disconnected) => Err(io::Error::other(format!(
            "{label} exited without reporting completion"
        ))),
    }
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
    fn status_runner_does_not_wait_for_descendant_lifetime() {
        let root =
            std::env::temp_dir().join(format!("xuva-status-descendant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("status fixture directory is created");
        let descendant = root.join("xuva-status-descendant.exe");
        let ping = std::env::var_os("SYSTEMROOT")
            .map(std::path::PathBuf::from)
            .expect("SYSTEMROOT is available")
            .join("System32")
            .join("ping.exe");
        std::fs::copy(ping, &descendant).expect("status descendant is copied");
        let parent_script = root.join("parent.cmd");
        std::fs::write(
            &parent_script,
            format!(
                "@start \"\" /b \"{}\" -n 2 127.0.0.1\r\n@exit /b 7\r\n",
                descendant.display()
            ),
        )
        .expect("status parent fixture is written");

        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/c"]).arg(&parent_script);
        let started = Instant::now();
        let status = run_status_bounded(&mut command, Duration::from_secs(2))
            .expect("status-only fixture completes");
        assert_eq!(status.code(), Some(7));
        assert!(started.elapsed() < Duration::from_secs(2));
        let cleanup_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if std::fs::remove_file(&descendant).is_ok() {
                break;
            }
            assert!(
                Instant::now() < cleanup_deadline,
                "short-lived status descendant did not exit naturally"
            );
            thread::sleep(Duration::from_millis(25));
        }
        std::fs::remove_dir_all(root).expect("status fixture directory is removed");
    }

    #[cfg(windows)]
    #[test]
    fn status_runner_terminates_a_stalled_child_at_the_deadline() {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/s", "/c", "ping -n 30 127.0.0.1 >nul"]);
        let started = Instant::now();
        let error = run_status_bounded(&mut command, Duration::from_millis(50))
            .expect_err("stalled status-only child must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
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

    #[cfg(windows)]
    #[test]
    fn bounded_runner_preserves_batch_output_and_wide_exit_status() {
        let root = std::env::temp_dir().join(format!("xuva-bounded-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("batch fixture directory is created");
        let script = root.join("fixture.cmd");
        std::fs::write(
            &script,
            "@echo {\"ok\":true}\r\n@if \"%1\"==\"wide\" exit /b 3010\r\n@exit /b 0\r\n",
        )
        .expect("batch fixture is written");

        let mut success = Command::new(&script);
        let success = run_bounded(
            &mut success,
            Some(Vec::new()),
            Duration::from_secs(5),
            PROBE_OUTPUT_LIMIT,
        )
        .expect("successful batch fixture completes");
        assert!(success.status.success());
        assert_eq!(
            String::from_utf8_lossy(&success.stdout).trim(),
            "{\"ok\":true}"
        );

        let mut wide = Command::new(&script);
        wide.arg("wide");
        let wide = run_bounded(
            &mut wide,
            Some(Vec::new()),
            Duration::from_secs(5),
            PROBE_OUTPUT_LIMIT,
        )
        .expect("wide-exit batch fixture completes");
        assert_eq!(wide.status.code(), Some(3010));
        std::fs::remove_dir_all(root).expect("batch fixture directory is removed");
    }

    #[cfg(windows)]
    #[test]
    fn bounded_runner_kills_a_descendant_that_keeps_inherited_pipes_open() {
        let root =
            std::env::temp_dir().join(format!("xuva-bounded-descendant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture directory is created");
        let descendant = root.join("xuva-descendant.exe");
        let ping = std::env::var_os("SYSTEMROOT")
            .map(std::path::PathBuf::from)
            .expect("SYSTEMROOT is available")
            .join("System32")
            .join("ping.exe");
        std::fs::copy(ping, &descendant).expect("unique descendant executable is copied");
        let parent_script = root.join("parent.cmd");
        std::fs::write(
            &parent_script,
            format!(
                "@start \"\" /b \"{}\" -n 30 127.0.0.1\r\n@exit /b 0\r\n",
                descendant.display()
            ),
        )
        .expect("parent fixture is written");

        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/c"]).arg(&parent_script);
        let started = Instant::now();
        let error = run_bounded(
            &mut command,
            None,
            Duration::from_millis(750),
            PROBE_OUTPUT_LIMIT,
        )
        .expect_err("inherited descendant pipe must keep the bounded call active");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if std::fs::remove_file(&descendant).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "descendant executable remained locked after the bounded runner"
            );
            thread::sleep(Duration::from_millis(25));
        }
        std::fs::remove_dir_all(root).expect("fixture directory is removed");
    }
}
