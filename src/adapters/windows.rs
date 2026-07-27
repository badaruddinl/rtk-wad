//! Native Windows process execution.

use std::ffi::OsString;
use std::process::{Command, ExitStatus};

use crate::dispatcher::CommandSpec;
use crate::metrics::WadMetrics;

pub(crate) fn run(arguments: &[OsString]) -> std::io::Result<ExitStatus> {
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
    run_at(&executable, &arguments[1..], None)
}

pub(crate) fn run_at(
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

pub(crate) fn run_rtk_at(
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

pub(crate) fn apply_command_spec(command: &mut Command, request: &CommandSpec) {
    command.args(&request.arguments);
    if let Some(current_directory) = &request.cwd {
        command.current_dir(current_directory);
    }
    command.envs(request.environment.iter().map(|(key, value)| (key, value)));
    // Stdio inherits the invoking console by default. That preserves both
    // interactive TTY commands and ordinary stdin/stdout/stderr forwarding.
    let _ = request.interactive;
}

pub(crate) fn run_plan(
    executable: &OsString,
    request: &CommandSpec,
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new(executable);
    apply_command_spec(&mut command, request);
    command.spawn().and_then(|mut child| child.wait())
}

pub(crate) fn run_rtk_plan(
    executable: &OsString,
    arguments: &[OsString],
    request: &CommandSpec,
    metrics: Option<&WadMetrics>,
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new(executable);
    command.args(arguments);
    if let Some(current_directory) = &request.cwd {
        command.current_dir(current_directory);
    }
    command.envs(request.environment.iter().map(|(key, value)| (key, value)));
    if let Some(metrics) = metrics {
        command.env("RTK_DB_PATH", metrics.scratch_windows_path());
    }
    let _ = request.interactive;
    command.spawn().and_then(|mut child| child.wait())
}
