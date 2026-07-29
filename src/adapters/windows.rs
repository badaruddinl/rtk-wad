//! Native Windows process execution.

use std::ffi::OsString;
use std::process::{Command, ExitStatus};

use crate::dispatcher::{CommandSpec, EnvironmentPolicy};
use crate::metrics::XuvaMetrics;

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
    metrics: Option<&XuvaMetrics>,
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
    apply_environment(command, request);
    // Stdio inherits the invoking console by default. That preserves both
    // interactive TTY commands and ordinary stdin/stdout/stderr forwarding.
    let _ = request.interactive;
}

fn apply_environment(command: &mut Command, request: &CommandSpec) {
    if request.environment_policy == EnvironmentPolicy::Isolated {
        command.env_clear();
        for name in [
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATH",
            "PATHEXT",
            "TEMP",
            "TMP",
            "USERPROFILE",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    command.envs(request.environment.iter().map(|(key, value)| (key, value)));
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
    metrics: Option<&XuvaMetrics>,
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new(executable);
    command.args(arguments);
    if let Some(current_directory) = &request.cwd {
        command.current_dir(current_directory);
    }
    apply_environment(&mut command, request);
    if let Some(metrics) = metrics {
        command.env("RTK_DB_PATH", metrics.scratch_windows_path());
    }
    let _ = request.interactive;
    command.spawn().and_then(|mut child| child.wait())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_process_drops_ambient_credentials_and_keeps_windows_baseline() {
        let request = CommandSpec {
            executable: OsString::from("cmd.exe"),
            arguments: vec![
                OsString::from("/d"),
                OsString::from("/s"),
                OsString::from("/c"),
                OsString::from(
                    "if defined GITHUB_TOKEN (exit /b 7) else if defined AWS_SECRET_ACCESS_KEY (exit /b 8) else if defined KUBECONFIG (exit /b 9) else if defined DOCKER_CONFIG (exit /b 10) else if defined NPM_CONFIG_USERCONFIG (exit /b 11) else if defined SSH_AUTH_SOCK (exit /b 12) else if not defined SYSTEMROOT (exit /b 13) else (exit /b 0)",
                ),
            ],
            cwd: std::env::current_dir().ok(),
            environment: Vec::new(),
            environment_policy: EnvironmentPolicy::Isolated,
            interactive: false,
        };
        let mut command = Command::new("cmd.exe");
        for name in [
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "KUBECONFIG",
            "DOCKER_CONFIG",
            "NPM_CONFIG_USERCONFIG",
            "SSH_AUTH_SOCK",
        ] {
            command.env(name, "must-not-cross");
        }
        apply_command_spec(&mut command, &request);
        let status = command.status().expect("isolated fixture starts");
        assert_eq!(status.code(), Some(0));
    }

    #[test]
    fn inherited_process_policy_remains_available_for_same_host_tools() {
        let request = CommandSpec {
            executable: OsString::from("cmd.exe"),
            arguments: Vec::new(),
            cwd: None,
            environment: vec![(OsString::from("XUVA_TEST_OVERLAY"), OsString::from("yes"))],
            environment_policy: EnvironmentPolicy::Inherit,
            interactive: false,
        };
        let mut command = Command::new("cmd.exe");
        apply_command_spec(&mut command, &request);
        assert!(command.get_envs().any(|(key, value)| {
            key == "XUVA_TEST_OVERLAY" && value == Some(std::ffi::OsStr::new("yes"))
        }));
    }
}
