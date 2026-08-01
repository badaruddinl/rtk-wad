//! Read-only stable release checks and manual update guidance.

use std::ffi::OsString;
use std::process::{Command, ExitStatus};
use std::time::Duration;

use crate::cli_exit::CliExit as ExitCode;
use crate::process;

const RELEASE_TAGS_URL: &str = "https://github.com/badsleepyday/xuva.git";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn parsed_version(value: &str) -> Option<((u64, u64, u64), bool)> {
    let value = value.trim().trim_start_matches('v');
    let (core, prerelease) = match value.split_once('-') {
        Some((core, suffix)) if !suffix.trim().is_empty() => (core, true),
        Some(_) => return None,
        None => (value, false),
    };
    let mut fields = core.split('.');
    let major = fields.next()?.parse().ok()?;
    let minor = fields.next()?.parse().ok()?;
    let patch = fields.next()?.parse().ok()?;
    fields
        .next()
        .is_none()
        .then_some(((major, minor, patch), prerelease))
}

pub(crate) fn parsed_stable_version(value: &str) -> Option<(u64, u64, u64)> {
    parsed_version(value)
        .filter(|(_, prerelease)| !prerelease)
        .map(|(version, _)| version)
}

pub(crate) fn stable_release_is_newer(latest: &str, current: &str) -> bool {
    parsed_stable_version(latest)
        .zip(parsed_version(current))
        .is_some_and(|(latest, (current, prerelease))| {
            latest > current || (latest == current && prerelease)
        })
}

pub(crate) fn latest_release_from_ls_remote(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/tags/"))
        .filter_map(|tag| parsed_stable_version(tag).map(|version| (version, tag)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag.to_owned())
}

fn native_git_output_with_timeout(
    arguments: &[&str],
    timeout: Duration,
) -> std::io::Result<(ExitStatus, String, String)> {
    let mut command = Command::new("git.exe");
    command.args(arguments);
    let output = process::run_bounded(&mut command, None, timeout, process::PROBE_OUTPUT_LIMIT)?;
    if output.stdout_truncated || output.stderr_truncated {
        return Err(std::io::Error::other(
            "native Git release check exceeded the output limit",
        ));
    }
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

pub(crate) fn command(arguments: &[OsString]) -> ExitCode {
    if arguments == [OsString::from("self-update")] {
        println!("current={}", env!("CARGO_PKG_VERSION"));
        println!("status=manual-update-required");
        println!(
            "action=download a verified release or run scripts/install.ps1 from a trusted XUVA checkout"
        );
        println!("check=xuva self-update --check");
        return ExitCode::SUCCESS;
    }
    if arguments != [OsString::from("self-update"), OsString::from("--check")] {
        eprintln!("xuva: invalid self-update options");
        eprintln!("  Usage: xuva self-update [--check]");
        eprintln!("  Try: xuva --help");
        return ExitCode::FAILURE;
    }
    let current = env!("CARGO_PKG_VERSION");
    let result = native_git_output_with_timeout(
        &["ls-remote", "--tags", "--refs", RELEASE_TAGS_URL],
        UPDATE_CHECK_TIMEOUT,
    );
    let (status, stdout, stderr) = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!(
                "xuva: update check unavailable via native Git: {error}; verify Git for Windows and Windows DNS, then retry"
            );
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Git for Windows could not query the release tags");
        eprintln!(
            "xuva: update check failed via native Git: {detail}; verify Windows DNS, proxy, and Git credentials, then retry"
        );
        return ExitCode::FAILURE;
    }
    let Some(latest) = latest_release_from_ls_remote(&stdout) else {
        eprintln!("xuva: update check returned no stable vMAJOR.MINOR.PATCH release tags");
        return ExitCode::FAILURE;
    };
    let update_available = stable_release_is_newer(&latest, current);
    println!("current={current}");
    println!("latest={}", latest.trim_start_matches('v'));
    println!(
        "status={}",
        if update_available {
            "update-available"
        } else {
            "up-to-date"
        }
    );
    println!("route=windows-native-git");
    ExitCode::SUCCESS
}
